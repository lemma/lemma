//! Expression normalization: algebraic simplification for drift-free evaluation.
//!
//! Internal [`NormalForm`] IR used by planning; sealed into [`ExecutionPlan`] tables.

use crate::computation::rational::{NumericFailure, RationalInteger};
use crate::formatting::expression::{CommandView, PredicateView, Syntax, SyntaxNode, SyntaxView};
use crate::parsing::arena::{ExprKind, ExprRef};
use crate::parsing::ast::{
    CalendarPeriodUnit, DateCalendarKind, DateRelativeKind, ListPredicateForm, LogicalConnective,
};
use crate::planning::execution_plan::ExecutionPlan;
use crate::planning::instance_list::RowHop;
use crate::planning::ordered_dispatch::DispatchKey;
use crate::planning::semantics::{
    primitive_boolean_arc, primitive_date_arc, primitive_number_arc, primitive_ratio_arc,
    primitive_text_arc, primitive_time_arc, ArithmeticComputation, ComparisonComputation,
    DataDefinition, DataPath, Expression, ExpressionKind, LemmaType, ListPredKind, LiteralValue,
    MathematicalComputation, PathSegment, RangeBound, RoundingMode, RowEnd, RulePath,
    SemanticConversionTarget, Source, SpecIdentity, ValueKind, VetoExpression,
};
use crate::planning::typing::{
    comparison_type, compute_arithmetic_result_type, date_predicate_type,
    infer_range_type_from_endpoint_types, logical_connective_type, logical_not_type, math_op_type,
    past_future_range_type, piecewise_type, quantity_integer_power_type, range_bound_result_type,
    range_containment_type, result_is_veto_type, step_rounding_type, unit_conversion_type,
    MeasureScope,
};
use crate::Error;
use indexmap::IndexMap;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

mod rewrite;

fn normalization_error(source: Option<Source>, failure: NumericFailure, context: &str) -> Error {
    Error::validation(format!("{context}: {failure}"), source, None::<String>)
}

/// Literal for an exactly folded plain-number rational.
///
/// Contract: callers fold plain numbers only. Unit-bearing operands never reach
/// this function; measure folds carry their unit signature on the result type.
fn literal_from_folded_rational(rational: RationalInteger) -> LiteralValue {
    LiteralValue::number(rational)
}

/// One rule lowered into the shared NormalForm graph and normalized.
pub(crate) struct NormalizedRule {
    pub(crate) body: NormalFormId,
    /// Type of the lowered root before any rewrite: the source expression typed
    /// by [`Cells::type_of`]. Equals the graph's inferred rule type.
    pub(crate) source_type: Arc<LemmaType>,
}

/// Plan-level inputs to per-rule normalization that do not change across the
/// execution order. Per-rule inputs (branches, source) stay explicit.
pub(crate) struct NormalizeContext<'a> {
    pub(crate) data: &'a IndexMap<DataPath, DataDefinition>,
    /// Template plans of instance-list row specs, by row spec.
    pub(crate) row_templates: &'a BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    /// The spec being planned: rows of its self-lists read this plan.
    pub(crate) self_identity: &'a SpecIdentity,
    /// Present rules in topo order (same set as the plan).
    pub(crate) rule_order: &'a [RulePath],
    /// Measure scope cells are typed in: the plan's unit + signature indexes.
    pub(crate) measure_scope: MeasureScope<'a>,
    pub(crate) max_normalized_expression_nodes: usize,
}

/// Completed rule roots, graph rule types, and data for lower.
struct LowerCtx<'a> {
    completed_rules: &'a HashMap<RulePath, NormalFormId>,
    /// [`crate::planning::graph::RuleNode::rule_type`] for every rule (pre-topo).
    rule_types: &'a HashMap<RulePath, Arc<LemmaType>>,
    data: &'a IndexMap<DataPath, DataDefinition>,
    row_templates: &'a BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    self_identity: &'a SpecIdentity,
    rule_order: &'a [RulePath],
}

impl LowerCtx<'_> {
    /// Member `path.name` on rows of `identity`, hop by hop.
    fn row_member_chain(
        &self,
        identity: &SpecIdentity,
        path: &[String],
        name: &str,
    ) -> Option<Vec<RowHop>> {
        let parts: Vec<&str> = path
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(name))
            .collect();
        let rules: BTreeSet<RulePath>;
        let view = if identity == self.self_identity {
            rules = self.rule_order.iter().cloned().collect();
            crate::planning::instance_list::RowEndView::Root {
                data: self.data,
                rules: &rules,
            }
        } else {
            crate::planning::instance_list::RowEndView::Plan(self.template(identity))
        };
        crate::planning::instance_list::row_end_chain(identity, view, self.row_templates, &parts)
    }

    fn row_end_type(&self, identity: &SpecIdentity, end: &RowEnd) -> Arc<LemmaType> {
        if identity != self.self_identity {
            return crate::planning::instance_list::row_end_type(self.template(identity), end);
        }
        match end {
            RowEnd::Data(path) => crate::planning::instance_list::row_data_type(self.data, path),
            RowEnd::Rule(rule) => Arc::clone(self.rule_types.get(rule).unwrap_or_else(|| {
                panic!("BUG: row rule '{}' has no rule type", rule.input_key())
            })),
        }
    }

    fn template(&self, identity: &SpecIdentity) -> &ExecutionPlan {
        self.row_templates
            .get(identity)
            .unwrap_or_else(|| panic!("BUG: row field of '{identity}' without a template plan"))
    }
}

pub(crate) fn build_normalized_rule(
    ctx: &NormalizeContext<'_>,
    completed_rules: &HashMap<RulePath, NormalFormId>,
    rule_types: &HashMap<RulePath, Arc<LemmaType>>,
    expression: &Expression,
    source: Option<Source>,
    interner: &mut NormalFormInterner,
) -> Result<NormalizedRule, Error> {
    let max_normalized_expression_nodes = ctx.max_normalized_expression_nodes;
    let lower = LowerCtx {
        completed_rules,
        rule_types,
        data: ctx.data,
        row_templates: ctx.row_templates,
        self_identity: ctx.self_identity,
        rule_order: ctx.rule_order,
    };

    let mut cells = Cells::new(interner, ctx.measure_scope);
    let root = to_normal_form(expression, &mut cells, &lower);
    let source_type = Arc::clone(cells.result_type(root));
    let nf = rewrite::normalize(root, cells, source.clone())?;

    if normal_form_exceeds_node_budget(&interner.forms, nf, max_normalized_expression_nodes) {
        return Err(expression_node_limit_error(
            max_normalized_expression_nodes,
            source,
        ));
    }

    Ok(NormalizedRule {
        body: nf,
        source_type,
    })
}

fn expression_node_limit_error(limit: usize, source: Option<Source>) -> Error {
    Error::resource_limit_exceeded(
        "max_normalized_expression_nodes",
        format!("{limit} expression nodes"),
        format!(
            "more than {limit} unique normal-form cells reachable from the rule root (rule references count as one cell)"
        ),
        "Restructure the rule or reduce repeated references to other rules",
        source,
        None,
        None,
    )
}

/// Whether the normal form, counted as unique DAG cells reachable from `root`,
/// exceeds the node budget (IR size = distinct cells, not tree expansion).
/// Rule references count as one cell: the target body was already budgeted when its
/// rule completed. Rule references are evaluation boundaries, so this measures only
/// intra-rule IR size.
pub(crate) fn normal_form_exceeds_node_budget(
    forms: &[NormalForm],
    root: NormalFormId,
    budget: usize,
) -> bool {
    let mut visited = HashSet::new();
    let mut worklist = vec![root];
    while let Some(current) = worklist.pop() {
        if !visited.insert(current) {
            continue;
        }
        if visited.len() > budget {
            return true;
        }
        let cell = forms.get(current.index()).unwrap_or_else(|| {
            panic!(
                "BUG: NormalFormId {} out of range during node budget walk (table len {})",
                current.0,
                forms.len()
            )
        });
        if cell.rule_ref.is_some() {
            continue;
        }
        cell.kind.for_each_child(|child| worklist.push(child));
        if let Some(origin) = cell.origin {
            worklist.push(origin);
        }
    }
    false
}

/// Maximum nesting depth of a NormalForm DAG (leaves and rule references have
/// depth 1). Rule references are evaluation boundaries, so planning measures only
/// intra-rule Kind nesting. Used by tests to check that bound.
#[cfg(test)]
pub(crate) fn normal_form_depth(forms: &[NormalForm], root: NormalFormId) -> usize {
    let mut memo: HashMap<NormalFormId, usize> = HashMap::new();
    // Explicit post-order: (id, children_pushed). Embeds are leaves (depth 1).
    let mut stack: Vec<(NormalFormId, bool)> = vec![(root, false)];
    while let Some((id, children_pushed)) = stack.pop() {
        if memo.contains_key(&id) {
            continue;
        }
        let cell = forms.get(id.index()).unwrap_or_else(|| {
            panic!(
                "BUG: NormalFormId {} out of range during depth walk (table len {})",
                id.0,
                forms.len()
            )
        });
        if cell.rule_ref.is_some() {
            memo.insert(id, 1);
            continue;
        }
        if !children_pushed {
            stack.push((id, true));
            cell.kind.for_each_child(|child| {
                if !memo.contains_key(&child) {
                    stack.push((child, false));
                }
            });
            if let Some(origin) = cell.origin {
                if !memo.contains_key(&origin) {
                    stack.push((origin, false));
                }
            }
            continue;
        }
        let mut depth = 1usize;
        cell.kind.for_each_child(|child| {
            let child_depth = *memo.get(&child).unwrap_or_else(|| {
                panic!(
                    "BUG: child NormalFormId {} missing from depth memo after post-order push",
                    child.0
                )
            });
            depth = depth.max(1 + child_depth);
        });
        if let Some(origin) = cell.origin {
            let origin_depth = *memo.get(&origin).unwrap_or_else(|| {
                panic!(
                    "BUG: origin NormalFormId {} missing from depth memo after post-order push",
                    origin.0
                )
            });
            depth = depth.max(1 + origin_depth);
        }
        memo.insert(id, depth);
    }
    *memo
        .get(&root)
        .unwrap_or_else(|| panic!("BUG: root NormalFormId {} missing from depth memo", root.0))
}

/// Leaves in the shipped equation DAG: literals and data only.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum LeafKind {
    Literal(LiteralValue),
    DataPath(crate::planning::semantics::DataPath),
    /// Value of the current value-list row at `level`.
    RowValue {
        level: u32,
    },
}

/// A member chain read on a row, through the row spec's template plans: every end
/// but the last reads a `uses` slot, as that instance; the last is the member read.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RowRead {
    pub(crate) ends: Vec<RowEnd>,
    pub(crate) result_type: Arc<LemmaType>,
}

/// Index into a [`NormalFormInterner`] / shipped [`crate::planning::execution_plan::ExecutionPlan::normal_forms`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NormalFormId(u32);

impl NormalFormId {
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }

    pub(crate) fn from_index(index: usize) -> Self {
        Self(u32::try_from(index).expect("BUG: normal_forms table exceeds u32::MAX"))
    }
}

/// One operand of a [`NormalFormKind::Sum`] or [`NormalFormKind::Product`]. `inverse`
/// marks a subtracted (Sum) or divided (Product) operand. The first term of a
/// Sum/Product is never inverse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ArithmeticTerm {
    pub(crate) operand: NormalFormId,
    pub(crate) inverse: bool,
}

impl ArithmeticTerm {
    pub(crate) fn direct(operand: NormalFormId) -> Self {
        Self {
            operand,
            inverse: false,
        }
    }

    pub(crate) fn inverted(operand: NormalFormId) -> Self {
        Self {
            operand,
            inverse: true,
        }
    }
}

/// One region body of a [`NormalFormKind::OrderedDispatch`] table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DispatchRegion {
    /// The region's value is this origin arm body.
    Body(NormalFormId),
    /// A conjunctive nest: a Piecewise over the residual arms of one key. Its arm
    /// bodies are origin arm bodies, and its winner is decided by the same scan as the
    /// origin Piecewise.
    Conjunctive(NormalFormId),
}

impl DispatchRegion {
    pub(crate) fn cell(self) -> NormalFormId {
        match self {
            DispatchRegion::Body(id) | DispatchRegion::Conjunctive(id) => id,
        }
    }

    fn map_cell(self, map: &mut impl FnMut(NormalFormId) -> NormalFormId) -> Self {
        match self {
            DispatchRegion::Body(id) => DispatchRegion::Body(map(id)),
            DispatchRegion::Conjunctive(id) => DispatchRegion::Conjunctive(map(id)),
        }
    }
}

/// Algebraic structure only. Children are [`NormalFormId`]. Hash/Eq drive cons —
/// no source spans or rule-ref identity here (those live on [`NormalForm`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum NormalFormKind {
    Leaf(LeafKind),
    Sum(Vec<ArithmeticTerm>),
    Product(Vec<ArithmeticTerm>),
    Subtract(NormalFormId, NormalFormId),
    Divide(NormalFormId, NormalFormId),
    Power(NormalFormId, NormalFormId),
    Modulo(NormalFormId, NormalFormId),
    Negate(NormalFormId),
    Reciprocal(NormalFormId),
    Comparison(NormalFormId, ComparisonComputation, NormalFormId),
    /// Logical operands in source order.
    Logical(LogicalConnective, Vec<NormalFormId>),
    Not(NormalFormId),
    MathOp(MathematicalComputation, NormalFormId),
    /// `operand -> round` or `operand -> round step`.
    StepRounding(RoundingMode, NormalFormId, Option<NormalFormId>),
    UnitConversion(NormalFormId, SemanticConversionTarget),
    Veto(VetoExpression),
    DateRelative(DateRelativeKind, NormalFormId),
    DateCalendar(DateCalendarKind, CalendarPeriodUnit, NormalFormId),
    RangeLiteral(NormalFormId, NormalFormId),
    PastFutureRange(DateRelativeKind, NormalFormId),
    RangeContainment(NormalFormId, NormalFormId),
    RangeBound(RangeBound, NormalFormId),
    ResultIsVeto(NormalFormId),
    Now,
    Piecewise(Vec<(NormalFormId, NormalFormId)>),
    /// A [`Self::Piecewise`] over one scrutinee, decided by binary search instead
    /// of a linear arm scan. Built by [`ordered_dispatch`], which always records
    /// the pre-image as `origin`, so explanation keeps reading the Piecewise.
    OrderedDispatch {
        scrutinee: NormalFormId,
        /// Sorted, deduplicated, all of one [`DispatchKey`] class.
        /// Shared so plan extract / rebuild clones are a refcount bump.
        boundaries: Arc<[DispatchKey]>,
        /// Exactly [`region_count`] entries; see [`crate::planning::ordered_dispatch`].
        regions: Vec<DispatchRegion>,
    },
    /// Read member `path.name` of the row `base` yields, in that row's context.
    SpecMember {
        base: NormalFormId,
        path: Vec<String>,
        name: String,
        row: RowRead,
    },
    /// Field `end` of the current row of the instance-list command at `level`
    /// (0 = outermost), read through the template plan of `identity`.
    RowField {
        level: u32,
        identity: SpecIdentity,
        end: RowEnd,
        /// Written list qualifier, preserved for explanation display. `None` for bare refs.
        qualifier: Option<Vec<String>>,
    },
    /// `row_level` is set when argument cells read the current list row at that
    /// level, through [`LeafKind::RowValue`] or [`Self::RowField`].
    ListCommand {
        list: NormalFormId,
        op: NormalFormListOp,
        args: Vec<NormalFormId>,
        row_level: Option<u32>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum NormalFormListOp {
    Find,
    First,
    Last,
    Count,
    Sum,
    Avg,
    Product,
    Min,
    Max,
    /// `args[0]` is the boolean expression evaluated once per row.
    Predicate {
        kind: crate::planning::semantics::ListPredKind,
        form: ListPredicateForm,
    },
    Sort {
        descending: bool,
    },
    Top,
    With,
    /// Append one row of spec `identity`; `args` are the values of `fields`, in order.
    WithRow {
        identity: Arc<SpecIdentity>,
        fields: Vec<DataPath>,
    },
    /// Set field key(s) on a Spec-element list; evaluation returns the list unchanged.
    Key {
        fields: Vec<DataPath>,
    },
}

impl NormalFormKind {
    /// Calls `f` on each child cell id in evaluation order. Piecewise yields `cond, body`
    /// per arm from index 0; OrderedDispatch yields the scrutinee then every region body.
    pub(crate) fn for_each_child(&self, mut f: impl FnMut(NormalFormId)) {
        match self {
            NormalFormKind::Leaf(_) | NormalFormKind::Veto(_) | NormalFormKind::Now => {}
            NormalFormKind::Sum(terms) | NormalFormKind::Product(terms) => {
                terms.iter().for_each(|term| f(term.operand));
            }
            NormalFormKind::Logical(_, children) => children.iter().copied().for_each(f),
            NormalFormKind::Subtract(a, b)
            | NormalFormKind::Divide(a, b)
            | NormalFormKind::Power(a, b)
            | NormalFormKind::Modulo(a, b)
            | NormalFormKind::Comparison(a, _, b)
            | NormalFormKind::RangeLiteral(a, b)
            | NormalFormKind::RangeContainment(a, b) => {
                f(*a);
                f(*b);
            }
            NormalFormKind::StepRounding(_, a, step) => {
                f(*a);
                if let Some(step) = step {
                    f(*step);
                }
            }
            NormalFormKind::Negate(x)
            | NormalFormKind::Reciprocal(x)
            | NormalFormKind::Not(x)
            | NormalFormKind::MathOp(_, x)
            | NormalFormKind::UnitConversion(x, _)
            | NormalFormKind::DateRelative(_, x)
            | NormalFormKind::DateCalendar(_, _, x)
            | NormalFormKind::PastFutureRange(_, x)
            | NormalFormKind::RangeBound(_, x)
            | NormalFormKind::ResultIsVeto(x) => f(*x),
            NormalFormKind::Piecewise(arms) => arms.iter().for_each(|(condition, body)| {
                f(*condition);
                f(*body);
            }),
            NormalFormKind::OrderedDispatch {
                scrutinee, regions, ..
            } => {
                f(*scrutinee);
                regions.iter().for_each(|region| f(region.cell()));
            }
            NormalFormKind::SpecMember { base, .. } => f(*base),
            NormalFormKind::RowField { .. } => {}
            NormalFormKind::ListCommand { list, args, .. } => {
                f(*list);
                args.iter().copied().for_each(f);
            }
        }
    }

    /// Child cell ids in [`Self::for_each_child`] order.
    pub(crate) fn children(&self) -> Vec<NormalFormId> {
        let mut children = Vec::new();
        self.for_each_child(|child| children.push(child));
        children
    }

    /// Same Kind with every child id passed through `map`. Payloads that are not
    /// child ids (operators, literals, targets, boundaries) are cloned.
    pub(crate) fn map_children(&self, mut map: impl FnMut(NormalFormId) -> NormalFormId) -> Self {
        match self {
            NormalFormKind::Leaf(leaf) => NormalFormKind::Leaf(leaf.clone()),
            NormalFormKind::Sum(terms) => NormalFormKind::Sum(
                terms
                    .iter()
                    .map(|term| ArithmeticTerm {
                        operand: map(term.operand),
                        inverse: term.inverse,
                    })
                    .collect(),
            ),
            NormalFormKind::Product(terms) => NormalFormKind::Product(
                terms
                    .iter()
                    .map(|term| ArithmeticTerm {
                        operand: map(term.operand),
                        inverse: term.inverse,
                    })
                    .collect(),
            ),
            NormalFormKind::Logical(connective, children) => {
                NormalFormKind::Logical(*connective, children.iter().copied().map(map).collect())
            }
            NormalFormKind::Subtract(a, b) => NormalFormKind::Subtract(map(*a), map(*b)),
            NormalFormKind::Divide(a, b) => NormalFormKind::Divide(map(*a), map(*b)),
            NormalFormKind::Power(a, b) => NormalFormKind::Power(map(*a), map(*b)),
            NormalFormKind::Modulo(a, b) => NormalFormKind::Modulo(map(*a), map(*b)),
            NormalFormKind::Comparison(a, op, b) => {
                NormalFormKind::Comparison(map(*a), op.clone(), map(*b))
            }
            NormalFormKind::RangeLiteral(a, b) => NormalFormKind::RangeLiteral(map(*a), map(*b)),
            NormalFormKind::RangeContainment(a, b) => {
                NormalFormKind::RangeContainment(map(*a), map(*b))
            }
            NormalFormKind::Negate(x) => NormalFormKind::Negate(map(*x)),
            NormalFormKind::Reciprocal(x) => NormalFormKind::Reciprocal(map(*x)),
            NormalFormKind::Not(x) => NormalFormKind::Not(map(*x)),
            NormalFormKind::MathOp(op, x) => NormalFormKind::MathOp(op.clone(), map(*x)),
            NormalFormKind::StepRounding(mode, x, step) => {
                NormalFormKind::StepRounding(*mode, map(*x), step.map(map))
            }
            NormalFormKind::UnitConversion(x, target) => {
                NormalFormKind::UnitConversion(map(*x), target.clone())
            }
            NormalFormKind::Veto(v) => NormalFormKind::Veto(v.clone()),
            NormalFormKind::DateRelative(kind, x) => NormalFormKind::DateRelative(*kind, map(*x)),
            NormalFormKind::DateCalendar(kind, unit, x) => {
                NormalFormKind::DateCalendar(*kind, *unit, map(*x))
            }
            NormalFormKind::PastFutureRange(kind, x) => {
                NormalFormKind::PastFutureRange(*kind, map(*x))
            }
            NormalFormKind::RangeBound(bound, x) => NormalFormKind::RangeBound(*bound, map(*x)),
            NormalFormKind::ResultIsVeto(x) => NormalFormKind::ResultIsVeto(map(*x)),
            NormalFormKind::Now => NormalFormKind::Now,
            NormalFormKind::Piecewise(arms) => NormalFormKind::Piecewise(
                arms.iter()
                    .map(|(condition, body)| (map(*condition), map(*body)))
                    .collect(),
            ),
            NormalFormKind::OrderedDispatch {
                scrutinee,
                boundaries,
                regions,
            } => NormalFormKind::OrderedDispatch {
                scrutinee: map(*scrutinee),
                boundaries: boundaries.clone(),
                regions: regions
                    .iter()
                    .map(|region| region.map_cell(&mut map))
                    .collect(),
            },
            NormalFormKind::SpecMember {
                base,
                path,
                name,
                row,
            } => NormalFormKind::SpecMember {
                base: map(*base),
                path: path.clone(),
                name: name.clone(),
                row: row.clone(),
            },
            NormalFormKind::RowField { .. } => self.clone(),
            NormalFormKind::ListCommand {
                list,
                op,
                args,
                row_level,
            } => NormalFormKind::ListCommand {
                list: map(*list),
                op: op.clone(),
                args: args.iter().copied().map(map).collect(),
                row_level: *row_level,
            },
        }
    }
}

/// One node in THE DAG: algebraic Kind, stamped result type, optional conversion
/// span, optional fold origin, optional rule use-site identity.
///
/// A rule use-site shares the completed body's Kind (same child ids) and sets
/// `rule_ref`. Type is the graph rule type (`RulePath`) or data resolved type
/// (rule-target DataPath) — not the post-rewrite body cell type. Cons keys on
/// `(kind, rule_ref, result_type)` so a rule reference never collapses into the
/// bare body cell, and DataPath leaves that differ only in resolved type stay
/// distinct.
#[derive(Clone, Debug)]
pub(crate) struct NormalForm {
    pub(crate) kind: NormalFormKind,
    /// Type of the value this cell evaluates to. Stamped at planning.
    pub(crate) result_type: Arc<LemmaType>,
    /// For Sum/Product: type after folding `children[..=k]`. Empty for other kinds.
    /// `fold_types.last()` equals `result_type` for n-ary arithmetic.
    pub(crate) fold_types: Vec<Arc<LemmaType>>,
    /// Span for unit-conversion nodes; does not affect sharing.
    pub(crate) source: Option<Source>,
    /// Pre-image destroyed by a fold; blocks sharing when present.
    pub(crate) origin: Option<NormalFormId>,
    /// Use-site of a named rule (`None` on ordinary cells).
    pub(crate) rule_ref: Option<RulePath>,
}

/// Result of [`Cells::type_of`]: stamped result type plus progressive fold types
/// for n-ary Sum/Product (empty for every other kind).
pub(crate) struct Typed {
    pub(crate) result_type: Arc<LemmaType>,
    pub(crate) fold_types: Vec<Arc<LemmaType>>,
}

/// Cons key: algebraic Kind, optional rule-ref identity, and stamped result
/// type. Type is part of the key so a shared interner across temporal slices
/// never collapses DataPath leaves that differ only in resolved type (Literal
/// leaves already distinguish via payload identity inside Kind; range endpoints
/// carry [`crate::planning::semantics::TypedLiteral`] types on [`crate::planning::semantics::ValueKind::Range`]).
type ConsKey = (NormalFormKind, Option<RulePath>, Arc<LemmaType>);

/// Build-local table + origin-free cons. Ship via [`Self::extract_reachable`].
pub(crate) struct NormalFormInterner {
    forms: Vec<NormalForm>,
    cons: HashMap<ConsKey, NormalFormId>,
}

impl NormalFormInterner {
    pub(crate) fn new() -> Self {
        Self {
            forms: Vec::new(),
            cons: HashMap::new(),
        }
    }

    pub(crate) fn intern(
        &mut self,
        kind: NormalFormKind,
        source: Option<Source>,
        origin: Option<NormalFormId>,
        rule_ref: Option<RulePath>,
        typed: Typed,
    ) -> NormalFormId {
        let Typed {
            result_type,
            fold_types,
        } = typed;
        let key = (kind, rule_ref, result_type);
        if origin.is_none() {
            if let Some(id) = self.cons.get(&key) {
                return *id;
            }
        }
        let id = NormalFormId(
            u32::try_from(self.forms.len()).expect("BUG: normal_forms table exceeds u32::MAX"),
        );
        if origin.is_none() {
            self.cons.insert(key.clone(), id);
        }
        self.forms.push(NormalForm {
            kind: key.0,
            result_type: key.2,
            fold_types,
            source,
            origin,
            rule_ref: key.1,
        });
        id
    }

    pub(crate) fn get(&self, id: NormalFormId) -> &NormalForm {
        self.forms.get(id.index()).unwrap_or_else(|| {
            panic!(
                "BUG: NormalFormId {} out of range (table len {})",
                id.0,
                self.forms.len()
            )
        })
    }

    pub(crate) fn result_type(&self, id: NormalFormId) -> &Arc<LemmaType> {
        &self.get(id).result_type
    }

    /// Number of cells interned so far; ids at or past it do not exist yet.
    pub(crate) fn len(&self) -> usize {
        self.forms.len()
    }

    /// Ship only cells reachable from `roots`. Dense-remap ids in ascending
    /// old-index order. Walks shape children and origin. Remaps both.
    ///
    /// Keeps the interner so one planning pass can share it across every
    /// temporal slice of a spec set.
    pub(crate) fn extract_reachable(
        &self,
        roots: &[NormalFormId],
    ) -> (Vec<NormalForm>, Vec<NormalFormId>) {
        let mut reachable: HashSet<u32> = HashSet::new();
        let mut worklist: Vec<NormalFormId> = roots.to_vec();
        while let Some(id) = worklist.pop() {
            if id.index() >= self.forms.len() {
                panic!(
                    "BUG: NormalFormId {} out of range during reachable extract (table len {})",
                    id.0,
                    self.forms.len()
                );
            }
            if !reachable.insert(id.0) {
                continue;
            }
            let cell = &self.forms[id.index()];
            cell.kind.for_each_child(|child| worklist.push(child));
            if let Some(origin) = cell.origin {
                worklist.push(origin);
            }
        }
        let mut old_ids: Vec<u32> = reachable.into_iter().collect();
        old_ids.sort_unstable();
        let mut remap: HashMap<u32, NormalFormId> = HashMap::with_capacity(old_ids.len());
        for (new_index, old) in old_ids.iter().copied().enumerate() {
            remap.insert(
                old,
                NormalFormId(
                    u32::try_from(new_index).expect("BUG: normal_forms table exceeds u32::MAX"),
                ),
            );
        }
        let mut table = Vec::with_capacity(old_ids.len());
        for old in &old_ids {
            let cell = &self.forms[*old as usize];
            table.push(NormalForm {
                kind: cell.kind.map_children(|child| {
                    *remap.get(&child.0).unwrap_or_else(|| {
                        panic!("BUG: child NormalFormId {} missing from remap", child.0)
                    })
                }),
                result_type: Arc::clone(&cell.result_type),
                fold_types: cell.fold_types.clone(),
                source: cell.source.clone(),
                origin: cell.origin.map(|o| {
                    *remap.get(&o.0).unwrap_or_else(|| {
                        panic!("BUG: origin NormalFormId {} missing from remap", o.0)
                    })
                }),
                rule_ref: cell.rule_ref.clone(),
            });
        }
        let remapped_roots = roots
            .iter()
            .map(|r| {
                *remap.get(&r.0).unwrap_or_else(|| {
                    panic!("BUG: root NormalFormId {} not in reachable set", r.0)
                })
            })
            .collect();
        (table, remapped_roots)
    }
}

/// Result type of a data leaf, from its definition in the flattened data map.
pub(crate) fn data_path_result_type(
    data: &IndexMap<DataPath, DataDefinition>,
    path: &DataPath,
) -> Arc<LemmaType> {
    match data.get(path) {
        Some(DataDefinition::Value { resolved_type, .. })
        | Some(DataDefinition::TypeDeclaration { resolved_type, .. })
        | Some(DataDefinition::Reference { resolved_type, .. })
        | Some(DataDefinition::InstanceList { resolved_type, .. }) => Arc::clone(resolved_type),
        Some(DataDefinition::Import { row, .. }) => Arc::new(LemmaType::primitive(
            crate::planning::semantics::TypeSpecification::Spec {
                spec: row.identity.as_ref().clone(),
            },
        )),
        // Absent: normalize unit tests with an empty data scope, as graph
        // `infer_data_type`.
        None => Arc::new(LemmaType::undetermined_type()),
    }
}

/// The interner together with the measure scope every new cell is typed in.
/// Lowering and every rewrite pass build cells through this, so a cell's type
/// is fixed once, at creation, by [`Cells::type_of`].
pub(crate) struct Cells<'a> {
    interner: &'a mut NormalFormInterner,
    measure_scope: MeasureScope<'a>,
}

/// Owned row-field end plus its optional written list qualifier
/// (`componenten` in `componenten.is_grondstof`). Bundled so [`Cells::intern_row_field`]
/// stays under the argument-count lint.
pub(crate) struct RowFieldEnd {
    pub(crate) end: RowEnd,
    pub(crate) qualifier: Option<Vec<String>>,
}

impl<'a> Cells<'a> {
    pub(crate) fn new(
        interner: &'a mut NormalFormInterner,
        measure_scope: MeasureScope<'a>,
    ) -> Self {
        Self {
            interner,
            measure_scope,
        }
    }

    pub(crate) fn get(&self, id: NormalFormId) -> &NormalForm {
        self.interner.get(id)
    }

    pub(crate) fn result_type(&self, id: NormalFormId) -> &Arc<LemmaType> {
        self.interner.result_type(id)
    }

    /// Number of cells interned so far; ids at or past it do not exist yet.
    pub(crate) fn len(&self) -> usize {
        self.interner.len()
    }

    pub(crate) fn intern_empty(&mut self, kind: NormalFormKind) -> NormalFormId {
        let typed = self.type_of(&kind, None);
        self.interner.intern(kind, None, None, None, typed)
    }

    pub(crate) fn intern_literal_leaf(
        &mut self,
        literal: LiteralValue,
        result_type: Arc<LemmaType>,
    ) -> NormalFormId {
        self.interner.intern(
            NormalFormKind::Leaf(LeafKind::Literal(literal)),
            None,
            None,
            None,
            Typed {
                result_type,
                fold_types: Vec::new(),
            },
        )
    }

    /// Row field cell, typed from the row spec (the type is the same on every row).
    fn intern_row_field(
        &mut self,
        level: u32,
        identity: SpecIdentity,
        row_end: RowFieldEnd,
        result_type: Arc<LemmaType>,
    ) -> NormalFormId {
        let RowFieldEnd { end, qualifier } = row_end;
        self.interner.intern(
            NormalFormKind::RowField {
                level,
                identity,
                end,
                qualifier,
            },
            None,
            None,
            None,
            Typed {
                result_type,
                fold_types: Vec::new(),
            },
        )
    }

    fn intern_row_value_leaf(&mut self, level: u32, result_type: Arc<LemmaType>) -> NormalFormId {
        self.interner.intern(
            NormalFormKind::Leaf(LeafKind::RowValue { level }),
            None,
            None,
            None,
            Typed {
                result_type,
                fold_types: Vec::new(),
            },
        )
    }

    fn intern_data_leaf(&mut self, path: DataPath, result_type: Arc<LemmaType>) -> NormalFormId {
        self.interner.intern(
            NormalFormKind::Leaf(LeafKind::DataPath(path)),
            None,
            None,
            None,
            Typed {
                result_type,
                fold_types: Vec::new(),
            },
        )
    }

    /// Use-site of a named rule: share the completed body's Kind, set rule_ref,
    /// stamp the caller-supplied type (graph rule type or data resolved type).
    /// Empty fold_types: eval never folds through a rule_ref cut.
    fn intern_rule_ref(
        &mut self,
        body: NormalFormId,
        rule_path: RulePath,
        result_type: Arc<LemmaType>,
    ) -> NormalFormId {
        let kind = self.interner.get(body).kind.clone();
        self.interner.intern(
            kind,
            None,
            None,
            Some(rule_path),
            Typed {
                result_type,
                fold_types: Vec::new(),
            },
        )
    }

    pub(crate) fn rebuild(
        &mut self,
        kind: NormalFormKind,
        source: Option<Source>,
        origin: Option<NormalFormId>,
    ) -> NormalFormId {
        let typed = self.type_of(&kind, origin);
        self.interner.intern(kind, source, origin, None, typed)
    }

    pub(crate) fn fold_into(
        &mut self,
        kind: NormalFormKind,
        destroyed_id: NormalFormId,
    ) -> NormalFormId {
        let typed = self.type_of(&kind, Some(destroyed_id));
        self.interner
            .intern(kind, None, Some(destroyed_id), None, typed)
    }

    pub(crate) fn fold_into_survivor(
        &mut self,
        survivor_id: NormalFormId,
        destroyed_id: NormalFormId,
    ) -> NormalFormId {
        let survivor = self.interner.get(survivor_id);
        // Identity-elim unwrap onto a node that already records a fold must not
        // erase that fold (e.g. `(exp(log(5))) + 0` → keep exp/log origin).
        if survivor.origin.is_some() {
            return survivor_id;
        }
        self.interner.intern(
            survivor.kind.clone(),
            survivor.source.clone(),
            Some(destroyed_id),
            survivor.rule_ref.clone(),
            Typed {
                result_type: Arc::clone(&survivor.result_type),
                fold_types: survivor.fold_types.clone(),
            },
        )
    }

    /// Always re-intern survivor Kind with `origin = destroyed`, even when the
    /// survivor already records an origin (Piecewise collapse must keep the
    /// recorded arms as the explanation pre-image).
    pub(crate) fn replace_with_survivor(
        &mut self,
        survivor_id: NormalFormId,
        destroyed_id: NormalFormId,
    ) -> NormalFormId {
        let survivor = self.interner.get(survivor_id);
        self.interner.intern(
            survivor.kind.clone(),
            survivor.source.clone(),
            Some(destroyed_id),
            survivor.rule_ref.clone(),
            Typed {
                result_type: Arc::clone(&survivor.result_type),
                fold_types: survivor.fold_types.clone(),
            },
        )
    }

    fn fold_arithmetic_child_types(
        &self,
        terms: &[ArithmeticTerm],
        direct: ArithmeticComputation,
        inverted: ArithmeticComputation,
    ) -> Vec<Arc<LemmaType>> {
        let (first, rest) = terms
            .split_first()
            .expect("BUG: arithmetic NormalForm with zero children");
        assert!(
            !first.inverse,
            "BUG: arithmetic NormalForm starts with an inverse term"
        );
        let mut fold_types = Vec::with_capacity(terms.len());
        let mut acc = Arc::clone(self.result_type(first.operand));
        fold_types.push(Arc::clone(&acc));
        for term in rest {
            let op = if term.inverse { &inverted } else { &direct };
            acc = compute_arithmetic_result_type(
                acc,
                op,
                Arc::clone(self.result_type(term.operand)),
                &self.measure_scope,
            );
            fold_types.push(Arc::clone(&acc));
        }
        fold_types
    }

    fn binary_arithmetic_type(
        &self,
        left: NormalFormId,
        op: ArithmeticComputation,
        right: NormalFormId,
    ) -> Arc<LemmaType> {
        let left_type = self.result_type(left);
        if op == ArithmeticComputation::Power && left_type.is_measure() {
            if let Some(exponent) = self.integer_exponent_leaf(right) {
                return quantity_integer_power_type(left_type, exponent, &self.measure_scope);
            }
        }
        compute_arithmetic_result_type(
            Arc::clone(left_type),
            &op,
            Arc::clone(self.result_type(right)),
            &self.measure_scope,
        )
    }

    fn integer_exponent_leaf(&self, id: NormalFormId) -> Option<i32> {
        let NormalFormKind::Leaf(LeafKind::Literal(literal)) = &self.get(id).kind else {
            return None;
        };
        let ValueKind::Number(number) = &literal.value else {
            return None;
        };
        if !number.is_integer() {
            return None;
        }
        number.numer_to_i32()
    }
}

/// Default primitive type for a folded/constructed literal when no richer type is available.
/// Named measure types must be stamped explicitly via [`Cells::fold_into`] /
/// [`Cells::intern_literal_leaf`].
fn literal_value_default_result_type(value: &ValueKind) -> Arc<LemmaType> {
    match value {
        ValueKind::Number(_) => primitive_number_arc().clone(),
        ValueKind::Boolean(_) => primitive_boolean_arc().clone(),
        ValueKind::Text(_) => primitive_text_arc().clone(),
        ValueKind::Date(_) => primitive_date_arc().clone(),
        ValueKind::Time(_) => primitive_time_arc().clone(),
        ValueKind::Ratio(_) => primitive_ratio_arc().clone(),
        ValueKind::Measure(_) => {
            panic!(
                "BUG: Measure literal result_type must be stamped from expression/fold survivor type"
            )
        }
        ValueKind::Range(_, _) => {
            panic!("BUG: Range literal result_type must be stamped from expression/endpoint types")
        }
        ValueKind::Spec(_) => {
            panic!("BUG: Spec literal result_type must be stamped from the spec identity")
        }
        ValueKind::List(_) => {
            panic!("BUG: List literal result_type must be stamped from the list type")
        }
    }
}

impl Cells<'_> {
    /// Type of a cell from its children's stamped types. Same rules as
    /// `infer_expression_type`, so the lowered root of a rule has the rule's
    /// inferred type (asserted when the plan is built). Rewrites preserve the
    /// value type when arms were `same_value_type` (validation invariant): base
    /// kind and measure dimensions stay; a named type may swap for its
    /// primitive or back (`x + 0` → `x`, a collapsed `unless`).
    ///
    /// `pre_image` is the cell this one replaces (a fold's destroyed cell, a
    /// rebuild's origin). Two kinds carry no typing information of their own
    /// and keep the pre-image's type: a folded literal (the fold survivor's
    /// measure family lives only on `result_type`) and an OrderedDispatch (its
    /// regions are in key order, not the source order the rule type follows).
    fn type_of(&self, kind: &NormalFormKind, pre_image: Option<NormalFormId>) -> Typed {
        let (result_type, fold_types) = match kind {
            NormalFormKind::Leaf(LeafKind::Literal(literal)) => (
                match pre_image {
                    Some(destroyed) => Arc::clone(self.result_type(destroyed)),
                    None => literal_value_default_result_type(&literal.value),
                },
                Vec::new(),
            ),
            NormalFormKind::Leaf(LeafKind::DataPath(_)) => {
                panic!("BUG: DataPath leaf result_type must be stamped from plan.data")
            }
            NormalFormKind::RowField { .. } => {
                panic!("BUG: row field result_type must be stamped from the row spec")
            }
            NormalFormKind::Leaf(LeafKind::RowValue { .. }) => {
                panic!("BUG: row value leaf result_type must be stamped from its list")
            }
            NormalFormKind::Now => (primitive_date_arc().clone(), Vec::new()),
            NormalFormKind::SpecMember { row, .. } => (Arc::clone(&row.result_type), Vec::new()),
            NormalFormKind::ListCommand { list, op, args, .. } => (
                list_command_result_type(
                    self.result_type(*list),
                    op,
                    args.first().map(|argument| self.result_type(*argument)),
                    &self.measure_scope,
                ),
                Vec::new(),
            ),
            NormalFormKind::Veto(_) => (Arc::new(LemmaType::veto_type()), Vec::new()),
            NormalFormKind::Sum(terms) => {
                let fold_types = self.fold_arithmetic_child_types(
                    terms,
                    ArithmeticComputation::Add,
                    ArithmeticComputation::Subtract,
                );
                let result_type = Arc::clone(
                    fold_types
                        .last()
                        .expect("BUG: Sum fold_types empty after fold_arithmetic_child_types"),
                );
                (result_type, fold_types)
            }
            NormalFormKind::Product(terms) => {
                let fold_types = self.fold_arithmetic_child_types(
                    terms,
                    ArithmeticComputation::Multiply,
                    ArithmeticComputation::Divide,
                );
                let result_type = Arc::clone(
                    fold_types
                        .last()
                        .expect("BUG: Product fold_types empty after fold_arithmetic_child_types"),
                );
                (result_type, fold_types)
            }
            NormalFormKind::Subtract(a, b) => (
                self.binary_arithmetic_type(*a, ArithmeticComputation::Subtract, *b),
                Vec::new(),
            ),
            NormalFormKind::Divide(a, b) => (
                self.binary_arithmetic_type(*a, ArithmeticComputation::Divide, *b),
                Vec::new(),
            ),
            NormalFormKind::Power(a, b) => (
                self.binary_arithmetic_type(*a, ArithmeticComputation::Power, *b),
                Vec::new(),
            ),
            NormalFormKind::Modulo(a, b) => (
                self.binary_arithmetic_type(*a, ArithmeticComputation::Modulo, *b),
                Vec::new(),
            ),
            // `-x` is `-1 * x`; `1/x` is `1 / x`. Typed by the kernel like the
            // source arithmetic they were rewritten from.
            NormalFormKind::Negate(x) => (
                compute_arithmetic_result_type(
                    primitive_number_arc().clone(),
                    &ArithmeticComputation::Multiply,
                    Arc::clone(self.result_type(*x)),
                    &self.measure_scope,
                ),
                Vec::new(),
            ),
            NormalFormKind::Reciprocal(x) => (
                compute_arithmetic_result_type(
                    primitive_number_arc().clone(),
                    &ArithmeticComputation::Divide,
                    Arc::clone(self.result_type(*x)),
                    &self.measure_scope,
                ),
                Vec::new(),
            ),
            NormalFormKind::Logical(_, children) => {
                let mut types = children.iter().map(|id| self.result_type(*id));
                let first = types.next().expect("BUG: And has at least two operands");
                let ty = types.fold(Arc::clone(first), |acc, next| {
                    logical_connective_type(&acc, next)
                });
                (ty, Vec::new())
            }
            NormalFormKind::Comparison(a, _, b) => (
                comparison_type(self.result_type(*a), self.result_type(*b)),
                Vec::new(),
            ),
            NormalFormKind::RangeContainment(a, b) => (
                range_containment_type(self.result_type(*a), self.result_type(*b)),
                Vec::new(),
            ),
            NormalFormKind::Not(x) => (logical_not_type(self.result_type(*x)), Vec::new()),
            NormalFormKind::DateRelative(_, x) | NormalFormKind::DateCalendar(_, _, x) => {
                (date_predicate_type(self.result_type(*x)), Vec::new())
            }
            NormalFormKind::ResultIsVeto(_) => (result_is_veto_type(), Vec::new()),
            NormalFormKind::MathOp(op, x) => (
                math_op_type(op, self.result_type(*x), &self.measure_scope),
                Vec::new(),
            ),
            NormalFormKind::StepRounding(_, x, step) => (
                step_rounding_type(
                    self.result_type(*x),
                    step.map(|step| self.result_type(step)),
                ),
                Vec::new(),
            ),
            NormalFormKind::UnitConversion(x, target) => (
                unit_conversion_type(self.result_type(*x), target),
                Vec::new(),
            ),
            NormalFormKind::RangeLiteral(a, b) => {
                let (a, b) = (self.result_type(*a), self.result_type(*b));
                let ty = if a.vetoed() || b.vetoed() {
                    Arc::new(LemmaType::veto_type())
                } else {
                    infer_range_type_from_endpoint_types(a.as_ref(), b.as_ref())
                };
                (ty, Vec::new())
            }
            NormalFormKind::PastFutureRange(_, offset) => (
                past_future_range_type(self.result_type(*offset)),
                Vec::new(),
            ),
            NormalFormKind::RangeBound(_, operand) => (
                range_bound_result_type(self.result_type(*operand), &self.measure_scope),
                Vec::new(),
            ),
            NormalFormKind::Piecewise(arms) => (
                piecewise_type(
                    arms.iter()
                        .map(|(_, body)| Arc::clone(self.result_type(*body))),
                    &self.measure_scope,
                ),
                Vec::new(),
            ),
            NormalFormKind::OrderedDispatch { .. } => (
                Arc::clone(self.result_type(pre_image.expect(
                    "BUG: OrderedDispatch is only built over the Piecewise it re-indexes",
                ))),
                Vec::new(),
            ),
        };
        // After planning validation every operand is determined, so an undetermined
        // stamp here is a validation gap. Unit tests may still lower untyped paths;
        // undetermined is then only legal when an operand already was.
        if result_type.is_undetermined() {
            let input_undetermined = match kind {
                NormalFormKind::Leaf(_) | NormalFormKind::Now | NormalFormKind::Veto(_) => false,
                NormalFormKind::OrderedDispatch { .. } => {
                    pre_image.is_some_and(|id| self.result_type(id).is_undetermined())
                }
                NormalFormKind::Sum(terms) | NormalFormKind::Product(terms) => terms
                    .iter()
                    .any(|term| self.result_type(term.operand).is_undetermined()),
                NormalFormKind::Logical(_, children) => children
                    .iter()
                    .any(|id| self.result_type(*id).is_undetermined()),
                NormalFormKind::Subtract(a, b)
                | NormalFormKind::Divide(a, b)
                | NormalFormKind::Power(a, b)
                | NormalFormKind::Modulo(a, b)
                | NormalFormKind::RangeLiteral(a, b) => {
                    self.result_type(*a).is_undetermined() || self.result_type(*b).is_undetermined()
                }
                NormalFormKind::StepRounding(_, a, step) => {
                    self.result_type(*a).is_undetermined()
                        || step.is_some_and(|step| self.result_type(step).is_undetermined())
                }
                // Infer returns Undetermined for invalid operand pairs; validation
                // rejects those before a plan is built. Normalize unit tests may
                // lower without validation, so determined operands → Undetermined
                // is allowed here.
                NormalFormKind::Comparison(_, _, _) | NormalFormKind::RangeContainment(_, _) => {
                    true
                }
                NormalFormKind::Negate(x)
                | NormalFormKind::Reciprocal(x)
                | NormalFormKind::UnitConversion(x, _)
                | NormalFormKind::RangeBound(_, x)
                | NormalFormKind::ResultIsVeto(x) => self.result_type(*x).is_undetermined(),
                NormalFormKind::Not(_)
                | NormalFormKind::DateRelative(_, _)
                | NormalFormKind::DateCalendar(_, _, _)
                | NormalFormKind::MathOp(_, _)
                | NormalFormKind::PastFutureRange(_, _) => true,
                NormalFormKind::Piecewise(arms) => arms.iter().any(|(condition, body)| {
                    self.result_type(*condition).is_undetermined()
                        || self.result_type(*body).is_undetermined()
                }),
                NormalFormKind::SpecMember { base, .. } => {
                    self.result_type(*base).is_undetermined()
                }
                NormalFormKind::RowField { .. } => false,
                NormalFormKind::ListCommand { list, args, .. } => {
                    self.result_type(*list).is_undetermined()
                        || args
                            .iter()
                            .any(|id| self.result_type(*id).is_undetermined())
                }
            };
            assert!(
                input_undetermined,
                "BUG: NormalForm cell stamped undetermined type after validation"
            );
        }
        Typed {
            result_type,
            fold_types,
        }
    }
}

impl LowerCtx<'_> {
    /// Root of an already planned rule. Every rule a cell reads precedes it in topo order.
    fn completed_rule(&self, path: &RulePath) -> NormalFormId {
        *self.completed_rules.get(path).unwrap_or_else(|| {
            panic!(
                "BUG: rule '{}' read before it was planned (missing dependency edge)",
                path.input_key()
            )
        })
    }

    fn rule_type(&self, path: &RulePath) -> Arc<LemmaType> {
        Arc::clone(self.rule_types.get(path).unwrap_or_else(|| {
            panic!("BUG: rule '{}' has no RuleNode.rule_type", path.input_key())
        }))
    }
}

/// Lower the tree into `cells`, children before parents.
fn to_normal_form(tree: &Expression, cells: &mut Cells<'_>, lower: &LowerCtx<'_>) -> NormalFormId {
    let mut lowered: Vec<NormalFormId> = Vec::with_capacity(tree.table.len());
    let mut children: Vec<NormalFormId> = Vec::new();
    for id in tree.table.ids() {
        let node = tree.at(id);
        children.clear();
        node.kind()
            .for_each_child(|child| children.push(lowered[child.index()]));
        lowered.push(lower_node(node, &children, &lowered, cells, lower));
    }
    *lowered.last().expect("BUG: expression tree has a root")
}

/// Operator group whose consecutive chain steps lower to one n-ary cell.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ArithmeticGroup {
    Sum,
    Product,
}

fn arithmetic_group(op: &ArithmeticComputation) -> Option<ArithmeticGroup> {
    match op {
        ArithmeticComputation::Add | ArithmeticComputation::Subtract => Some(ArithmeticGroup::Sum),
        ArithmeticComputation::Multiply | ArithmeticComputation::Divide => {
            Some(ArithmeticGroup::Product)
        }
        ArithmeticComputation::Power | ArithmeticComputation::Modulo => None,
    }
}

/// Lower the left fold of `first` over `steps` whose operands are lowered.
///
/// Each run of consecutive steps in one group (`a + b - c`, `a * b / c`) interns one
/// n-ary cell. Subtract and divide steps become negated and reciprocal operands only
/// when every operand of the run is numeric, as normalization expands them; otherwise
/// the run is lowered one binary step at a time.
fn lower_arithmetic_chain(
    first: NormalFormId,
    steps: &[(ArithmeticComputation, NormalFormId)],
    cells: &mut Cells<'_>,
) -> NormalFormId {
    let mut current = first;
    let mut rest = steps;
    while let Some((op, operand)) = rest.first() {
        let Some(group) = arithmetic_group(op) else {
            current = lower_binary_arithmetic(op, current, *operand, cells);
            rest = &rest[1..];
            continue;
        };
        let run_len = rest
            .iter()
            .take_while(|(op, _)| arithmetic_group(op) == Some(group))
            .count();
        current = lower_arithmetic_run(current, &rest[..run_len], group, cells);
        rest = &rest[run_len..];
    }
    current
}

fn lower_arithmetic_run(
    first: NormalFormId,
    operands: &[(ArithmeticComputation, NormalFormId)],
    group: ArithmeticGroup,
    cells: &mut Cells<'_>,
) -> NormalFormId {
    let has_subtract_or_divide = operands.iter().any(|(op, _)| {
        matches!(
            op,
            ArithmeticComputation::Subtract | ArithmeticComputation::Divide
        )
    });
    let all_numeric = {
        let mut memo = HashMap::new();
        std::iter::once(first)
            .chain(operands.iter().map(|(_, id)| *id))
            .all(|id| rewrite::numeric_only(cells, &mut memo, id))
    };
    if !has_subtract_or_divide || (all_numeric && operands.len() > 1) {
        let terms: Vec<ArithmeticTerm> = std::iter::once(ArithmeticTerm::direct(first))
            .chain(operands.iter().map(|(op, id)| ArithmeticTerm {
                operand: *id,
                inverse: matches!(
                    op,
                    ArithmeticComputation::Subtract | ArithmeticComputation::Divide
                ),
            }))
            .collect();
        let kind = match group {
            ArithmeticGroup::Sum => NormalFormKind::Sum(terms),
            ArithmeticGroup::Product => NormalFormKind::Product(terms),
        };
        return cells.intern_empty(kind);
    }
    operands.iter().fold(first, |current, (op, operand)| {
        lower_binary_arithmetic(op, current, *operand, cells)
    })
}

fn lower_binary_arithmetic(
    op: &ArithmeticComputation,
    left: NormalFormId,
    right: NormalFormId,
    cells: &mut Cells<'_>,
) -> NormalFormId {
    let kind = match op {
        ArithmeticComputation::Subtract => NormalFormKind::Subtract(left, right),
        ArithmeticComputation::Divide => NormalFormKind::Divide(left, right),
        ArithmeticComputation::Add => NormalFormKind::Sum(vec![
            ArithmeticTerm::direct(left),
            ArithmeticTerm::direct(right),
        ]),
        ArithmeticComputation::Multiply => NormalFormKind::Product(vec![
            ArithmeticTerm::direct(left),
            ArithmeticTerm::direct(right),
        ]),
        ArithmeticComputation::Power => NormalFormKind::Power(left, right),
        ArithmeticComputation::Modulo => NormalFormKind::Modulo(left, right),
    };
    cells.intern_empty(kind)
}

/// Member `path.name` read on the row `base` yields, through the template plans of
/// its spec and of every `uses` hop.
fn lower_spec_member(
    base: NormalFormId,
    path: &[String],
    name: &str,
    cells: &mut Cells<'_>,
    lower: &LowerCtx<'_>,
) -> NormalFormId {
    let crate::planning::semantics::TypeSpecification::Spec { spec } =
        &cells.result_type(base).specifications
    else {
        panic!(
            "BUG: member '{}' read on a base that is not spec-typed after typing",
            crate::planning::semantics::spec_member_input_key(path, name)
        );
    };
    let chain = lower.row_member_chain(spec, path, name).unwrap_or_else(|| {
        panic!(
            "BUG: row member '{}' of '{spec}' passed typing",
            crate::planning::semantics::spec_member_input_key(path, name)
        )
    });
    let last = chain.last().expect("BUG: a row end chain has a last hop");
    let result_type = lower.row_end_type(&last.identity, &last.end);
    cells.intern_empty(NormalFormKind::SpecMember {
        base,
        path: path.to_vec(),
        name: name.to_string(),
        row: RowRead {
            ends: chain.into_iter().map(|hop| hop.end).collect(),
            result_type,
        },
    })
}

/// Lower one node whose children are already lowered to `children`, in `children()` order.
fn lower_node(
    node: ExprRef<'_, ExpressionKind>,
    children: &[NormalFormId],
    lowered: &[NormalFormId],
    cells: &mut Cells<'_>,
    lower: &LowerCtx<'_>,
) -> NormalFormId {
    match node.kind() {
        ExpressionKind::Literal(lit) => {
            cells.intern_literal_leaf(lit.to_literal(), Arc::clone(&lit.lemma_type))
        }
        // A data slot bound to a rule (`-> with slot: big`) stays a data leaf: the read
        // takes the rule's value and checks it against the slot's declared type.
        ExpressionKind::DataPath(p) => {
            cells.intern_data_leaf(p.clone(), data_path_result_type(lower.data, p))
        }
        ExpressionKind::SpecMember { path, name, .. } => {
            lower_spec_member(children[0], path, name, cells, lower)
        }
        ExpressionKind::RulePath(path) => {
            let body = lower.completed_rule(path);
            cells.intern_rule_ref(body, path.clone(), lower.rule_type(path))
        }
        ExpressionKind::Logical { connective, .. } => {
            cells.intern_empty(NormalFormKind::Logical(*connective, children.to_vec()))
        }
        ExpressionKind::Arithmetic { steps, .. } => {
            let (first, operands) = children
                .split_first()
                .expect("BUG: arithmetic chain has a first operand");
            assert_eq!(
                operands.len(),
                steps.len(),
                "BUG: arithmetic chain lowered to one operand per step"
            );
            let steps: Vec<(ArithmeticComputation, NormalFormId)> = steps
                .iter()
                .zip(operands)
                .map(|(step, operand)| (step.op.clone(), *operand))
                .collect();
            lower_arithmetic_chain(*first, &steps, cells)
        }
        ExpressionKind::Comparison(_, op, _) => {
            let &[left_id, right_id] = children else {
                unreachable!("BUG: comparison has two operands")
            };
            cells.intern_empty(NormalFormKind::Comparison(left_id, op.clone(), right_id))
        }
        ExpressionKind::UnitConversion(_, target) => {
            let &[inner_id] = children else {
                unreachable!("BUG: unit conversion has one operand")
            };
            cells.rebuild(
                NormalFormKind::UnitConversion(inner_id, target.clone()),
                node.source().cloned(),
                None,
            )
        }
        ExpressionKind::LogicalNegation(_, _) => {
            let &[inner_id] = children else {
                unreachable!("BUG: negation has one operand")
            };
            cells.intern_empty(NormalFormKind::Not(inner_id))
        }
        ExpressionKind::MathematicalComputation(op, _) => {
            let &[inner_id] = children else {
                unreachable!("BUG: math operator has one operand")
            };
            cells.intern_empty(NormalFormKind::MathOp(op.clone(), inner_id))
        }
        ExpressionKind::StepRounding { mode, .. } => match *children {
            [operand_id] => {
                cells.intern_empty(NormalFormKind::StepRounding(*mode, operand_id, None))
            }
            [operand_id, step_id] => cells.intern_empty(NormalFormKind::StepRounding(
                *mode,
                operand_id,
                Some(step_id),
            )),
            _ => unreachable!("BUG: step rounding has an operand and an optional step"),
        },
        ExpressionKind::Veto(v) => cells.intern_empty(NormalFormKind::Veto(v.clone())),
        ExpressionKind::Now => cells.intern_empty(NormalFormKind::Now),
        ExpressionKind::DateRelative(kind, _) => {
            let &[inner_id] = children else {
                unreachable!("BUG: date relative has one operand")
            };
            cells.intern_empty(NormalFormKind::DateRelative(*kind, inner_id))
        }
        ExpressionKind::DateCalendar(kind, unit, _) => {
            let &[inner_id] = children else {
                unreachable!("BUG: date calendar has one operand")
            };
            cells.intern_empty(NormalFormKind::DateCalendar(*kind, *unit, inner_id))
        }
        ExpressionKind::RangeLiteral(_, _) => {
            let &[left_id, right_id] = children else {
                unreachable!("BUG: range literal has two operands")
            };
            cells.intern_empty(NormalFormKind::RangeLiteral(left_id, right_id))
        }
        ExpressionKind::PastFutureRange(kind, _) => {
            let &[inner_id] = children else {
                unreachable!("BUG: past/future range has one operand")
            };
            cells.intern_empty(NormalFormKind::PastFutureRange(*kind, inner_id))
        }
        ExpressionKind::RangeBound(bound, _) => {
            let &[inner_id] = children else {
                unreachable!("BUG: range bound has one operand")
            };
            cells.intern_empty(NormalFormKind::RangeBound(*bound, inner_id))
        }
        ExpressionKind::RangeContainment(_, _) => {
            let &[left_id, right_id] = children else {
                unreachable!("BUG: range containment has two operands")
            };
            cells.intern_empty(NormalFormKind::RangeContainment(left_id, right_id))
        }
        ExpressionKind::ResultIsVeto(_) => {
            let &[operand_id] = children else {
                unreachable!("BUG: result-is-veto has one operand")
            };
            cells.intern_empty(NormalFormKind::ResultIsVeto(operand_id))
        }
        ExpressionKind::Piecewise(_) => {
            let mapped = children
                .chunks_exact(2)
                .map(|pair| (pair[0], pair[1]))
                .collect();
            cells.intern_empty(NormalFormKind::Piecewise(mapped))
        }
        ExpressionKind::ListCommand { op, row_level, .. } => {
            let (list_id, args) = children
                .split_first()
                .expect("BUG: list command has a list operand");
            cells.intern_empty(NormalFormKind::ListCommand {
                list: *list_id,
                op: list_op_kind(op),
                args: args.to_vec(),
                row_level: *row_level,
            })
        }
        ExpressionKind::RowField {
            level,
            identity,
            end,
            qualifier,
        } => cells.intern_row_field(
            *level,
            identity.clone(),
            RowFieldEnd {
                end: end.clone(),
                qualifier: qualifier.clone(),
            },
            lower.row_end_type(identity, end),
        ),
        ExpressionKind::RowValue { level, list } => {
            let list_cell = *lowered.get(list.index()).expect(
                "BUG: row value type-only list reference must point to an earlier expression",
            );
            let element_type = cells.result_type(list_cell).list_element_lemma_type();
            cells.intern_row_value_leaf(*level, Arc::clone(element_type))
        }
    }
}

/// The normal-form operation of a list op. Its operands are the list command's children
/// in `children()` order.
fn list_op_kind(op: &crate::planning::semantics::ListOp) -> NormalFormListOp {
    use crate::planning::semantics::{ListOp, ListReduce};
    match op {
        ListOp::Find { .. } => NormalFormListOp::Find,
        ListOp::First => NormalFormListOp::First,
        ListOp::Last => NormalFormListOp::Last,
        ListOp::Count => NormalFormListOp::Count,
        ListOp::Reduce { kind, .. } => match kind {
            ListReduce::Sum => NormalFormListOp::Sum,
            ListReduce::Avg => NormalFormListOp::Avg,
            ListReduce::Product => NormalFormListOp::Product,
            ListReduce::Min => NormalFormListOp::Min,
            ListReduce::Max => NormalFormListOp::Max,
        },
        ListOp::Predicate { kind, form, .. } => NormalFormListOp::Predicate {
            kind: *kind,
            form: *form,
        },
        ListOp::Sort { descending, .. } => NormalFormListOp::Sort {
            descending: *descending,
        },
        ListOp::Top { .. } => NormalFormListOp::Top,
        ListOp::With { .. } => NormalFormListOp::With,
        ListOp::WithRow { identity, fields } => NormalFormListOp::WithRow {
            identity: Arc::new(identity.clone()),
            fields: fields.iter().map(|(path, _)| path.clone()).collect(),
        },
        ListOp::Key { fields } => NormalFormListOp::Key {
            fields: fields.clone(),
        },
    }
}

/// `first_argument` is the type of the first argument cell; a reduce over instance
/// rows takes the type of its row expression.
fn list_command_result_type(
    list_type: &Arc<LemmaType>,
    op: &NormalFormListOp,
    first_argument: Option<&Arc<LemmaType>>,
    measure_scope: &crate::planning::typing::MeasureScope<'_>,
) -> Arc<LemmaType> {
    use crate::planning::typing::{
        list_command_result_type as stamp, list_has_instance_rows, ListCommandResultKind,
    };
    let instance_rows = list_has_instance_rows(list_type);
    let kind = match op {
        NormalFormListOp::Count => ListCommandResultKind::Count,
        NormalFormListOp::Predicate {
            kind: ListPredKind::Any | ListPredKind::All,
            ..
        } => ListCommandResultKind::BooleanPredicate,
        NormalFormListOp::Predicate {
            kind: ListPredKind::Where,
            ..
        }
        | NormalFormListOp::Sort { .. }
        | NormalFormListOp::Top
        | NormalFormListOp::With
        | NormalFormListOp::WithRow { .. }
        | NormalFormListOp::Key { .. } => ListCommandResultKind::KeepList,
        NormalFormListOp::Sum
        | NormalFormListOp::Avg
        | NormalFormListOp::Product
        | NormalFormListOp::Min
        | NormalFormListOp::Max
            if instance_rows =>
        {
            ListCommandResultKind::InstanceReduce
        }
        NormalFormListOp::Find
        | NormalFormListOp::First
        | NormalFormListOp::Last
        | NormalFormListOp::Sum
        | NormalFormListOp::Avg
        | NormalFormListOp::Product
        | NormalFormListOp::Min
        | NormalFormListOp::Max => ListCommandResultKind::Element,
    };
    let result = stamp(list_type, kind, first_argument, measure_scope);
    match op {
        NormalFormListOp::Key { fields } => {
            crate::planning::typing::refine_list_key_type(&result, fields)
        }
        _ => result,
    }
}

fn cell(forms: &[NormalForm], id: NormalFormId) -> &NormalForm {
    forms.get(id.index()).unwrap_or_else(|| {
        panic!(
            "BUG: NormalFormId {} out of range (table len {})",
            id.0,
            forms.len()
        )
    })
}

/// Expression text for a DAG node. Follows origin when present so folded
/// nodes display their pre-image.
/// Display a cell as Lemma syntax. Data paths are qualified by the written row
/// whose context the cell narrates in (`people.1.age`), bare at the root.
pub(crate) fn explanation_display(
    forms: &[NormalForm],
    id: NormalFormId,
    qualifier: Option<&NameScope>,
) -> String {
    Syntax(NfNode {
        forms,
        id,
        qualifier,
    })
    .to_string()
}

/// Cell whose spelling a node displays: the pre-image of folded cells.
fn displayed_cell(forms: &[NormalForm], mut id: NormalFormId) -> &NormalForm {
    loop {
        let nf = cell(forms, id);
        match nf.origin {
            Some(origin) => id = origin,
            None => return nf,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SpineLevel {
    Additive,
    Multiplicative,
}

/// One same-level, left-associative arithmetic chain as its first operand and the steps
/// after it, in source order. Walks the left spine iteratively, through origins, so a
/// long chain never recurses. A child at another level or a named rule ends the spine.
fn arithmetic_chain(
    forms: &[NormalForm],
    top: NormalFormId,
) -> (
    NormalFormId,
    Vec<(&'static ArithmeticComputation, NormalFormId)>,
) {
    let level = match &displayed_cell(forms, top).kind {
        NormalFormKind::Sum(_) | NormalFormKind::Subtract(..) => SpineLevel::Additive,
        NormalFormKind::Product(_) | NormalFormKind::Divide(..) | NormalFormKind::Modulo(..) => {
            SpineLevel::Multiplicative
        }
        _ => unreachable!("BUG: arithmetic_chain on a non-arithmetic cell"),
    };
    let mut steps = Vec::new();
    let mut current = top;
    loop {
        let shown = displayed_cell(forms, current);
        if shown.rule_ref.is_some() {
            break;
        }
        match (&shown.kind, level) {
            (NormalFormKind::Sum(terms), SpineLevel::Additive) => {
                let (first, rest) = terms
                    .split_first()
                    .expect("BUG: Sum cell has at least one operand");
                assert!(!first.inverse, "BUG: Sum starts with an inverse term");
                steps.extend(rest.iter().rev().map(|term| {
                    let op = if term.inverse {
                        &ArithmeticComputation::Subtract
                    } else {
                        &ArithmeticComputation::Add
                    };
                    (op, term.operand)
                }));
                current = first.operand;
            }
            (NormalFormKind::Subtract(left, right), SpineLevel::Additive) => {
                steps.push((&ArithmeticComputation::Subtract, *right));
                current = *left;
            }
            (NormalFormKind::Product(terms), SpineLevel::Multiplicative) => {
                let (first, rest) = terms
                    .split_first()
                    .expect("BUG: Product cell has at least one operand");
                assert!(!first.inverse, "BUG: Product starts with an inverse term");
                steps.extend(rest.iter().rev().map(|term| {
                    let op = if term.inverse {
                        &ArithmeticComputation::Divide
                    } else {
                        &ArithmeticComputation::Multiply
                    };
                    (op, term.operand)
                }));
                current = first.operand;
            }
            (NormalFormKind::Divide(left, right), SpineLevel::Multiplicative) => {
                steps.push((&ArithmeticComputation::Divide, *right));
                current = *left;
            }
            (NormalFormKind::Modulo(left, right), SpineLevel::Multiplicative) => {
                steps.push((&ArithmeticComputation::Modulo, *right));
                current = *left;
            }
            _ => break,
        }
    }
    steps.reverse();
    (current, steps)
}

/// How an explanation names the data and rules of the instance context a cell narrates
/// in: as the plan holding the instance names them. Data display under the instance's
/// hops (`people.1.age`, `i.x`); rules under the `uses` hops above the nearest row or
/// the root (`i.total`), so a row's own rules stay bare.
pub(crate) struct NameScope {
    pub(crate) data: Vec<PathSegment>,
    pub(crate) rules: Vec<PathSegment>,
}

/// A normal-form cell printed as Lemma syntax.
#[derive(Clone, Copy)]
pub(crate) struct NfNode<'a> {
    pub(crate) forms: &'a [NormalForm],
    pub(crate) id: NormalFormId,
    /// Instance whose context the cell narrates in; `None` for the root plan.
    pub(crate) qualifier: Option<&'a NameScope>,
}

impl<'a> NfNode<'a> {
    fn at(self, id: NormalFormId) -> Self {
        Self {
            forms: self.forms,
            id,
            qualifier: self.qualifier,
        }
    }
}

/// Leaf of a normal-form cell.
#[derive(Clone, Copy)]
pub(crate) enum NfAtom<'a> {
    Literal(&'a LiteralValue, &'a LemmaType),
    DataPath(&'a DataPath),
    /// Data path qualified by the instance whose context it narrates in.
    QualifiedDataPath {
        prefix: &'a [PathSegment],
        path: &'a DataPath,
    },
    Rule(&'a RulePath),
    /// Rule qualified by the `uses` hops of the instance whose context it narrates in.
    QualifiedRule {
        prefix: &'a [PathSegment],
        path: &'a RulePath,
    },
    Row(&'a RowEnd),
    /// Row field with its written list qualifier: `componenten.is_grondstof`.
    QualifiedRow {
        qualifier: &'a [String],
        end: &'a RowEnd,
    },
    SpecMemberField {
        path: &'a [String],
        name: &'a str,
    },
    Now,
    Veto(&'a VetoExpression),
}

impl std::fmt::Display for NfAtom<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NfAtom::Literal(literal, lemma_type) => {
                write!(f, "{}", literal.display_value_with_type(lemma_type))
            }
            NfAtom::DataPath(path) => write!(f, "{}", path.input_key()),
            NfAtom::QualifiedDataPath { prefix, path } => {
                for segment in prefix.iter() {
                    write!(f, "{}.", segment.input_key())?;
                }
                write!(f, "{}", path.input_key())
            }
            NfAtom::Rule(path) => write!(f, "{}", path.input_key()),
            NfAtom::QualifiedRule { prefix, path } => {
                for segment in prefix.iter() {
                    write!(f, "{}.", segment.input_key())?;
                }
                write!(f, "{}", path.input_key())
            }
            NfAtom::Row(end) => write!(f, "{end}"),
            NfAtom::QualifiedRow { qualifier, end } => {
                for (i, segment) in qualifier.iter().enumerate() {
                    if i > 0 {
                        f.write_str(".")?;
                    }
                    f.write_str(segment)?;
                }
                f.write_str(".")?;
                write!(f, "{end}")
            }
            NfAtom::SpecMemberField { path, name } => {
                write!(
                    f,
                    "{}",
                    crate::planning::semantics::spec_member_input_key(path, name)
                )
            }
            NfAtom::Now => write!(f, "now"),
            NfAtom::Veto(veto) => write!(f, "{veto}"),
        }
    }
}

fn value_starts_with_sign(value: &ValueKind) -> bool {
    let mut current = value;
    loop {
        match current {
            ValueKind::Number(n) | ValueKind::Measure(n) | ValueKind::Ratio(n) => {
                return n.numer_is_negative()
            }
            ValueKind::Range(left, _) => current = &left.value,
            _ => return false,
        }
    }
}

fn single_argument(args: &[NormalFormId], command: &str) -> NormalFormId {
    match args {
        [argument] => *argument,
        other => panic!(
            "BUG: `{command}` cell must carry one argument, got {}",
            other.len()
        ),
    }
}

fn optional_argument(args: &[NormalFormId], command: &str) -> Option<NormalFormId> {
    match args {
        [] => None,
        [argument] => Some(*argument),
        other => panic!(
            "BUG: `{command}` cell carries at most one argument, got {}",
            other.len()
        ),
    }
}

impl<'a> SyntaxNode<'a> for NfNode<'a> {
    type Atom = NfAtom<'a>;

    fn view(self) -> SyntaxView<'a, Self> {
        let nf = displayed_cell(self.forms, self.id);
        let atom = |atom| SyntaxView::Atom {
            atom,
            starts_with_sign: false,
        };
        if let Some(path) = &nf.rule_ref {
            return match self.qualifier {
                Some(scope) if !scope.rules.is_empty() => atom(NfAtom::QualifiedRule {
                    prefix: &scope.rules,
                    path,
                }),
                Some(_) | None => atom(NfAtom::Rule(path)),
            };
        }
        let node = |id: &NormalFormId| self.at(*id);
        let nodes = |ids: &[NormalFormId]| ids.iter().map(node).collect::<Vec<_>>();
        match &nf.kind {
            NormalFormKind::Leaf(LeafKind::Literal(literal)) => SyntaxView::Atom {
                atom: NfAtom::Literal(literal, nf.result_type.as_ref()),
                starts_with_sign: value_starts_with_sign(&literal.value),
            },
            NormalFormKind::Leaf(LeafKind::DataPath(path)) => match self.qualifier {
                Some(scope) if !scope.data.is_empty() => atom(NfAtom::QualifiedDataPath {
                    prefix: &scope.data,
                    path,
                }),
                Some(_) | None => atom(NfAtom::DataPath(path)),
            },
            NormalFormKind::RowField {
                end,
                qualifier: Some(segments),
                ..
            } => atom(NfAtom::QualifiedRow {
                qualifier: segments,
                end,
            }),
            NormalFormKind::RowField {
                end,
                qualifier: None,
                ..
            } => atom(NfAtom::Row(end)),
            NormalFormKind::Leaf(LeafKind::RowValue { .. }) => SyntaxView::ImplicitRowValue,
            NormalFormKind::Now => atom(NfAtom::Now),
            NormalFormKind::Veto(veto) => atom(NfAtom::Veto(veto)),
            NormalFormKind::Comparison(left, op, right) => {
                SyntaxView::Comparison(node(left), op, node(right))
            }
            NormalFormKind::Logical(connective, children) => SyntaxView::Logical {
                connective: *connective,
                operands: nodes(children),
            },
            NormalFormKind::Not(operand) => {
                let shown = displayed_cell(self.forms, *operand);
                match &shown.kind {
                    NormalFormKind::ResultIsVeto(inner) if shown.rule_ref.is_none() => {
                        SyntaxView::IsNotVeto(node(inner))
                    }
                    _ => SyntaxView::Not(node(operand)),
                }
            }
            NormalFormKind::ResultIsVeto(operand) => SyntaxView::IsVeto(node(operand)),
            NormalFormKind::Sum(_)
            | NormalFormKind::Product(_)
            | NormalFormKind::Subtract(..)
            | NormalFormKind::Divide(..)
            | NormalFormKind::Modulo(..) => {
                let (first, steps) = arithmetic_chain(self.forms, self.id);
                let level = match &nf.kind {
                    NormalFormKind::Sum(_) | NormalFormKind::Subtract(..) => {
                        &ArithmeticComputation::Add
                    }
                    _ => &ArithmeticComputation::Multiply,
                };
                SyntaxView::Chain {
                    level,
                    operands: std::iter::once(node(&first))
                        .chain(steps.iter().map(|(_, operand)| node(operand)))
                        .collect(),
                    ops: steps.iter().map(|(op, _)| *op).collect(),
                }
            }
            NormalFormKind::Power(left, right) => SyntaxView::Chain {
                level: &ArithmeticComputation::Power,
                operands: vec![node(left), node(right)],
                ops: vec![&ArithmeticComputation::Power],
            },
            NormalFormKind::Negate(operand) => SyntaxView::Negate(node(operand)),
            NormalFormKind::Reciprocal(operand) => SyntaxView::Reciprocal(node(operand)),
            NormalFormKind::MathOp(op, operand) => SyntaxView::Prefix(op, node(operand)),
            NormalFormKind::StepRounding(mode, operand, step) => SyntaxView::ListCommand {
                list: node(operand),
                command: CommandView::Step(mode.keyword(), step.as_ref().map(node)),
            },
            NormalFormKind::RangeBound(bound, operand) => SyntaxView::Prefix(bound, node(operand)),
            NormalFormKind::UnitConversion(value, target) => {
                SyntaxView::UnitConversion(node(value), target)
            }
            NormalFormKind::DateRelative(kind, date) => SyntaxView::DateRelative(kind, node(date)),
            NormalFormKind::DateCalendar(kind, unit, date) => {
                SyntaxView::DateCalendar(kind, unit, node(date))
            }
            NormalFormKind::RangeLiteral(left, right) => {
                SyntaxView::RangeLiteral(node(left), node(right))
            }
            NormalFormKind::PastFutureRange(kind, offset) => {
                SyntaxView::PastFutureRange(kind, node(offset))
            }
            NormalFormKind::RangeContainment(value, range) => {
                SyntaxView::RangeContainment(node(value), node(range))
            }
            NormalFormKind::Piecewise(arms) => SyntaxView::Piecewise(
                arms.iter()
                    .map(|(condition, result)| (node(condition), node(result)))
                    .collect(),
            ),
            NormalFormKind::SpecMember {
                base, path, name, ..
            } => {
                let base_cell = cell(self.forms, *base);
                let named = base_cell.rule_ref.is_some()
                    || matches!(
                        base_cell.kind,
                        NormalFormKind::Leaf(_) | NormalFormKind::SpecMember { .. }
                    );
                SyntaxView::Member {
                    base: node(base),
                    path: NfAtom::SpecMemberField {
                        path,
                        name: name.as_str(),
                    },
                    parenthesize_base: !named,
                }
            }
            NormalFormKind::OrderedDispatch { .. } => unreachable!(
                "BUG: OrderedDispatch always carries its Piecewise pre-image as origin, \
                 so displayed_cell never stops at it"
            ),
            NormalFormKind::ListCommand { list, op, args, .. } => {
                let reduce = |name: &'static str| {
                    CommandView::Reduce(name, optional_argument(args, name).map(|id| node(&id)))
                };
                let command = match op {
                    NormalFormListOp::Find => CommandView::Find(nodes(args)),
                    NormalFormListOp::First => CommandView::First,
                    NormalFormListOp::Last => CommandView::Last,
                    NormalFormListOp::Count => CommandView::Count,
                    NormalFormListOp::Sum => reduce("sum"),
                    NormalFormListOp::Avg => reduce("avg"),
                    NormalFormListOp::Product => reduce("product"),
                    NormalFormListOp::Min => reduce("min"),
                    NormalFormListOp::Max => reduce("max"),
                    NormalFormListOp::Predicate { kind, form } => {
                        let name = kind.keyword();
                        let expr = node(&single_argument(args, name));
                        let view = match form {
                            ListPredicateForm::Value => PredicateView::Value(expr),
                            ListPredicateForm::Instance => PredicateView::Instance(expr),
                        };
                        CommandView::Predicate(name, view)
                    }
                    NormalFormListOp::Sort { descending } => CommandView::Sort {
                        descending: *descending,
                        argument: optional_argument(args, "sort").map(|id| node(&id)),
                    },
                    NormalFormListOp::Top => CommandView::Top(node(&single_argument(args, "top"))),
                    NormalFormListOp::With => {
                        CommandView::With(args.iter().map(|id| (None, node(id))).collect())
                    }
                    NormalFormListOp::WithRow { fields, .. } => {
                        assert_eq!(
                            fields.len(),
                            args.len(),
                            "BUG: `with` row cell must carry one value per field"
                        );
                        CommandView::With(
                            fields
                                .iter()
                                .zip(args)
                                .map(|(field, value)| (Some(NfAtom::DataPath(field)), node(value)))
                                .collect(),
                        )
                    }
                    NormalFormListOp::Key { fields } => {
                        CommandView::Key(fields.iter().map(NfAtom::DataPath).collect())
                    }
                };
                SyntaxView::ListCommand {
                    list: node(list),
                    command,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static EMPTY_ROW_TEMPLATES: BTreeMap<SpecIdentity, Arc<ExecutionPlan>> = BTreeMap::new();
    static TEST_SELF_IDENTITY: std::sync::LazyLock<SpecIdentity> =
        std::sync::LazyLock::new(|| SpecIdentity {
            repository: None,
            spec: "test".to_string(),
            effective: crate::parsing::ast::EffectiveDate::Origin,
            slice: crate::parsing::ast::EffectiveDate::Origin,
        });
    use crate::computation::arithmetic::SignatureIndex;
    use crate::computation::rational::{rational_new, rational_one, rational_zero};
    use crate::parsing::arena::{ExprId, ExprTable, ExprTree};
    use crate::parsing::ast::{ChainStep, PrimitiveKind};
    use crate::planning::semantics::TypedLiteral;
    use crate::planning::semantics::{
        ComparisonComputation, DataPath, ExpressionKind, NegationType, SemanticConversionTarget,
        ValueKind,
    };
    use crate::planning::unit_index::UnitIndex;
    use std::rc::Rc;

    fn interner_forms(interner: &NormalFormInterner) -> &[NormalForm] {
        &interner.forms
    }

    fn empty_named_types() -> &'static IndexMap<String, Arc<LemmaType>> {
        use std::sync::OnceLock;
        static EMPTY: OnceLock<IndexMap<String, Arc<LemmaType>>> = OnceLock::new();
        EMPTY.get_or_init(IndexMap::new)
    }

    fn empty_measure_scope<'a>(
        unit_index: &'a UnitIndex,
        signature_index: &'a SignatureIndex,
    ) -> MeasureScope<'a> {
        MeasureScope {
            unit_index,
            signature_index,
            named_types: empty_named_types(),
        }
    }

    /// Typed data map for algebraic normalize unit tests: mirrors a post-validation
    /// plan so [`Cells::type_of`] never stamps undetermined.
    fn algebraic_scope() -> IndexMap<DataPath, DataDefinition> {
        use crate::parsing::source::{Source as ParseSource, SourceType};
        use crate::Span;
        let source = ParseSource::new(
            SourceType::Volatile,
            Span {
                start: 0,
                end: 0,
                line: 1,
                col: 1,
            },
        );
        let mut data = IndexMap::new();
        for (name, resolved_type) in [
            ("x", primitive_number_arc().clone()),
            ("y", primitive_number_arc().clone()),
            ("flag", primitive_boolean_arc().clone()),
            ("flag2", primitive_boolean_arc().clone()),
        ] {
            data.insert(
                DataPath::new(vec![], name.into()),
                DataDefinition::TypeDeclaration {
                    resolved_type,
                    declared_suggestion: None,
                    declared_value: None,
                    source: source.clone(),
                },
            );
        }
        data
    }

    /// Normalize through the production pipeline with typed algebraic data in scope.
    fn normalize_expression(expr: &Ex) -> Result<Expression, Error> {
        let data = algebraic_scope();
        normalize_expression_with_data(expr, &data).map(|(expr, _)| expr)
    }

    /// Normalize with a resolved data map in scope, returning the normalized
    /// expression alongside the interner so tests can inspect the shipped shape.
    fn normalize_expression_with_data(
        expr: &Ex,
        data: &IndexMap<DataPath, DataDefinition>,
    ) -> Result<(Expression, NormalFormInterner), Error> {
        let tree = expr.tree();
        let source = tree.root_source().cloned();
        let mut interner = NormalFormInterner::new();
        let root = lower_with_data(&tree, &mut interner, data);
        let unit_index = UnitIndex::new();
        let signature_index = SignatureIndex::new();
        let measure_scope = empty_measure_scope(&unit_index, &signature_index);
        let nf = rewrite::normalize(
            root,
            Cells::new(&mut interner, measure_scope),
            source.clone(),
        )?;
        let expression = to_expression(interner_forms(&interner), nf, source);
        Ok((expression, interner))
    }

    fn to_normal_form_empty(expr: &Ex, interner: &mut NormalFormInterner) -> NormalFormId {
        let data = algebraic_scope();
        to_normal_form_with_data(expr, interner, &data)
    }

    fn to_normal_form_with_data(
        expr: &Ex,
        interner: &mut NormalFormInterner,
        data: &IndexMap<DataPath, DataDefinition>,
    ) -> NormalFormId {
        lower_with_data(&expr.tree(), interner, data)
    }

    fn lower_with_data(
        expr: &Expression,
        interner: &mut NormalFormInterner,
        data: &IndexMap<DataPath, DataDefinition>,
    ) -> NormalFormId {
        let completed_rules = HashMap::new();
        let rule_types = HashMap::new();
        let lower = LowerCtx {
            completed_rules: &completed_rules,
            rule_types: &rule_types,
            data,
            row_templates: &EMPTY_ROW_TEMPLATES,
            self_identity: &TEST_SELF_IDENTITY,
            rule_order: &[],
        };
        let unit_index = UnitIndex::new();
        let signature_index = SignatureIndex::new();
        let mut cells = Cells::new(interner, empty_measure_scope(&unit_index, &signature_index));
        to_normal_form(expr, &mut cells, &lower)
    }

    /// Lowers normal forms into one expression table; every node carries `source`.
    struct Lowering<'f> {
        forms: &'f [NormalForm],
        source: Option<Source>,
        table: ExprTable<ExpressionKind>,
    }

    fn to_expression(forms: &[NormalForm], id: NormalFormId, source: Option<Source>) -> Expression {
        let mut lowering = Lowering {
            forms,
            source,
            table: ExprTable::default(),
        };
        let root = lowering.child(id);
        ExprTree::new(lowering.table, root)
    }

    impl Lowering<'_> {
        fn child(&mut self, id: NormalFormId) -> ExprId {
            let kind = self.kind(id);
            self.table.push(kind, self.source.clone())
        }

        fn kind(&mut self, id: NormalFormId) -> ExpressionKind {
            let forms = self.forms;
            let source = self.source.clone();
            let nf = cell(forms, id);
            if let Some(path) = &nf.rule_ref {
                return ExpressionKind::RulePath(path.clone());
            }
            match &nf.kind {
                NormalFormKind::Leaf(LeafKind::Literal(lit)) => {
                    ExpressionKind::Literal(Box::new(TypedLiteral {
                        value: lit.value.clone(),
                        lemma_type: Arc::clone(&nf.result_type),
                    }))
                }
                NormalFormKind::Leaf(LeafKind::DataPath(p)) => ExpressionKind::DataPath(p.clone()),
                NormalFormKind::Sum(terms) => self.sum_kind(terms),
                NormalFormKind::Product(terms) => self.product_kind(terms),
                NormalFormKind::Subtract(a, b) => {
                    self.binary_kind(*a, ArithmeticComputation::Subtract, *b)
                }
                NormalFormKind::Divide(a, b) => {
                    self.binary_kind(*a, ArithmeticComputation::Divide, *b)
                }
                NormalFormKind::Power(base, exp) => {
                    self.binary_kind(*base, ArithmeticComputation::Power, *exp)
                }
                NormalFormKind::Modulo(a, b) => {
                    self.binary_kind(*a, ArithmeticComputation::Modulo, *b)
                }
                NormalFormKind::Negate(x) => {
                    let zero = self.table.push(
                        ExpressionKind::Literal(Box::new(TypedLiteral::number(rational_zero()))),
                        source.clone(),
                    );
                    let operand = self.child(*x);
                    arithmetic_kind(zero, vec![(ArithmeticComputation::Subtract, operand)])
                }
                NormalFormKind::Reciprocal(x) => {
                    let one = self.table.push(
                        ExpressionKind::Literal(Box::new(TypedLiteral::number(rational_one()))),
                        source.clone(),
                    );
                    let operand = self.child(*x);
                    arithmetic_kind(one, vec![(ArithmeticComputation::Divide, operand)])
                }
                NormalFormKind::Comparison(a, op, b) => {
                    ExpressionKind::Comparison(self.child(*a), op.clone(), self.child(*b))
                }
                NormalFormKind::Logical(connective, children) => {
                    self.logical_kind(*connective, children)
                }
                NormalFormKind::Not(x) => {
                    ExpressionKind::LogicalNegation(self.child(*x), NegationType::Not)
                }
                NormalFormKind::MathOp(op, x) => {
                    ExpressionKind::MathematicalComputation(op.clone(), self.child(*x))
                }
                NormalFormKind::StepRounding(mode, x, step) => ExpressionKind::StepRounding {
                    operand: self.child(*x),
                    mode: *mode,
                    step: step.map(|step| self.child(step)),
                },
                NormalFormKind::UnitConversion(x, target) => {
                    ExpressionKind::UnitConversion(self.child(*x), target.clone())
                }
                NormalFormKind::Veto(v) => ExpressionKind::Veto(v.clone()),
                NormalFormKind::Now => ExpressionKind::Now,
                NormalFormKind::DateRelative(kind, x) => {
                    ExpressionKind::DateRelative(*kind, self.child(*x))
                }
                NormalFormKind::DateCalendar(kind, unit, x) => {
                    ExpressionKind::DateCalendar(*kind, *unit, self.child(*x))
                }
                NormalFormKind::RangeLiteral(a, b) => {
                    ExpressionKind::RangeLiteral(self.child(*a), self.child(*b))
                }
                NormalFormKind::PastFutureRange(kind, x) => {
                    ExpressionKind::PastFutureRange(*kind, self.child(*x))
                }
                NormalFormKind::RangeBound(bound, x) => {
                    ExpressionKind::RangeBound(*bound, self.child(*x))
                }
                NormalFormKind::RangeContainment(a, b) => {
                    ExpressionKind::RangeContainment(self.child(*a), self.child(*b))
                }
                NormalFormKind::ResultIsVeto(operand) => {
                    ExpressionKind::ResultIsVeto(self.child(*operand))
                }
                NormalFormKind::Piecewise(arms) => ExpressionKind::Piecewise(
                    arms.iter()
                        .map(|(condition, result)| (self.child(*condition), self.child(*result)))
                        .collect(),
                ),
                NormalFormKind::SpecMember {
                    base, path, name, ..
                } => ExpressionKind::SpecMember {
                    base: self.child(*base),
                    path: path.clone(),
                    name: name.clone(),
                },
                NormalFormKind::RowField {
                    level,
                    identity,
                    end,
                    qualifier,
                    ..
                } => ExpressionKind::RowField {
                    level: *level,
                    identity: identity.clone(),
                    end: end.clone(),
                    qualifier: qualifier.clone(),
                },
                NormalFormKind::Leaf(LeafKind::RowValue { .. }) => {
                    panic!("BUG: normal-form RowValue cannot be rebuilt without its list context")
                }
                // A dispatch table has no Expression spelling; its pre-image does.
                NormalFormKind::OrderedDispatch { .. } => self.kind(
                    nf.origin
                        .expect("BUG: OrderedDispatch without its Piecewise pre-image"),
                ),
                NormalFormKind::ListCommand {
                    list,
                    op,
                    args,
                    row_level,
                } => {
                    let list_expr = self.child(*list);
                    let arg_exprs: Vec<ExprId> = args.iter().map(|id| self.child(*id)).collect();
                    use crate::planning::semantics::{ListOp, ListReduce};
                    let planned = match op {
                        NormalFormListOp::Find => ListOp::Find { keys: arg_exprs },
                        NormalFormListOp::First => ListOp::First,
                        NormalFormListOp::Last => ListOp::Last,
                        NormalFormListOp::Count => ListOp::Count,
                        NormalFormListOp::Sum => ListOp::Reduce {
                            kind: ListReduce::Sum,
                            argument: arg_exprs.into_iter().next(),
                        },
                        NormalFormListOp::Avg => ListOp::Reduce {
                            kind: ListReduce::Avg,
                            argument: arg_exprs.into_iter().next(),
                        },
                        NormalFormListOp::Product => ListOp::Reduce {
                            kind: ListReduce::Product,
                            argument: arg_exprs.into_iter().next(),
                        },
                        NormalFormListOp::Min => ListOp::Reduce {
                            kind: ListReduce::Min,
                            argument: arg_exprs.into_iter().next(),
                        },
                        NormalFormListOp::Max => ListOp::Reduce {
                            kind: ListReduce::Max,
                            argument: arg_exprs.into_iter().next(),
                        },
                        NormalFormListOp::Predicate { kind, form } => ListOp::Predicate {
                            kind: *kind,
                            form: *form,
                            expr: arg_exprs
                                .into_iter()
                                .next()
                                .expect("BUG: predicate needs a row expression"),
                        },
                        NormalFormListOp::Sort { descending } => ListOp::Sort {
                            descending: *descending,
                            argument: arg_exprs.into_iter().next(),
                        },
                        NormalFormListOp::Top => ListOp::Top {
                            n: arg_exprs.into_iter().next().expect("BUG: top needs n"),
                        },
                        NormalFormListOp::With => ListOp::With { rows: arg_exprs },
                        NormalFormListOp::WithRow { identity, fields } => ListOp::WithRow {
                            identity: SpecIdentity::clone(identity),
                            fields: fields.iter().cloned().zip(arg_exprs).collect(),
                        },
                        NormalFormListOp::Key { fields } => ListOp::Key {
                            fields: fields.clone(),
                        },
                    };
                    ExpressionKind::ListCommand {
                        list: list_expr,
                        op: planned,
                        row_level: *row_level,
                    }
                }
            }
        }

        fn binary_kind(
            &mut self,
            left: NormalFormId,
            op: ArithmeticComputation,
            right: NormalFormId,
        ) -> ExpressionKind {
            let left = self.child(left);
            let right = self.child(right);
            arithmetic_kind(left, vec![(op, right)])
        }

        fn sum_kind(&mut self, terms: &[ArithmeticTerm]) -> ExpressionKind {
            match terms {
                [] => ExpressionKind::Literal(Box::new(TypedLiteral::number(rational_zero()))),
                [one] => self.kind(one.operand),
                [first, rest @ ..] => self.terms_kind(
                    first.operand,
                    rest,
                    ArithmeticComputation::Add,
                    ArithmeticComputation::Subtract,
                ),
            }
        }

        fn product_kind(&mut self, terms: &[ArithmeticTerm]) -> ExpressionKind {
            match terms {
                [] => ExpressionKind::Literal(Box::new(TypedLiteral::number(rational_one()))),
                [one] => self.kind(one.operand),
                [first, rest @ ..] => self.terms_kind(
                    first.operand,
                    rest,
                    ArithmeticComputation::Multiply,
                    ArithmeticComputation::Divide,
                ),
            }
        }

        fn terms_kind(
            &mut self,
            first: NormalFormId,
            rest: &[ArithmeticTerm],
            direct: ArithmeticComputation,
            inverse: ArithmeticComputation,
        ) -> ExpressionKind {
            let first = self.child(first);
            let steps = rest
                .iter()
                .map(|term| {
                    let op = if term.inverse {
                        inverse.clone()
                    } else {
                        direct.clone()
                    };
                    (op, self.child(term.operand))
                })
                .collect();
            arithmetic_kind(first, steps)
        }

        fn logical_kind(
            &mut self,
            connective: LogicalConnective,
            children: &[NormalFormId],
        ) -> ExpressionKind {
            match children {
                [] => unreachable!("BUG: Logical has at least one operand"),
                [one] => self.kind(*one),
                [first, rest @ ..] => {
                    let first = self.child(*first);
                    let operands = rest.iter().map(|id| self.child(*id)).collect();
                    logical_kind(connective, first, operands)
                }
            }
        }
    }

    /// Test expressions carry no spans; chain steps require one.
    fn step_source() -> Source {
        Source::new(
            crate::parsing::source::SourceType::Volatile,
            crate::Span {
                start: 0,
                end: 0,
                line: 1,
                col: 1,
            },
        )
    }

    fn arithmetic_kind(
        first: ExprId,
        steps: Vec<(ArithmeticComputation, ExprId)>,
    ) -> ExpressionKind {
        ExpressionKind::Arithmetic {
            first,
            steps: steps
                .into_iter()
                .map(|(op, operand)| ChainStep {
                    op,
                    operand,
                    source_location: step_source(),
                })
                .collect(),
        }
    }

    /// `left op right` of a one-step arithmetic chain.
    fn binary_arithmetic(
        kind: &ExpressionKind,
    ) -> Option<(&ExprId, &ArithmeticComputation, &ExprId)> {
        match kind {
            ExpressionKind::Arithmetic { first, steps } => match steps.as_slice() {
                [step] => Some((first, &step.op, &step.operand)),
                _ => None,
            },
            _ => None,
        }
    }

    fn logical_kind(
        connective: LogicalConnective,
        first: ExprId,
        operands: Vec<ExprId>,
    ) -> ExpressionKind {
        ExpressionKind::Logical {
            connective,
            first,
            steps: operands
                .into_iter()
                .map(|operand| ChainStep {
                    op: (),
                    operand,
                    source_location: step_source(),
                })
                .collect(),
        }
    }

    fn num_expr(n: i64) -> Ex {
        Ex::leaf(
            ExpressionKind::Literal(Box::new(TypedLiteral::number(rational_new(n, 1)))),
            None,
        )
    }

    fn bool_expr(b: bool) -> Ex {
        Ex::leaf(
            ExpressionKind::Literal(Box::new(TypedLiteral::from_bool(b))),
            None,
        )
    }

    fn dx() -> Ex {
        Ex::leaf(
            ExpressionKind::DataPath(DataPath::new(vec![], "x".into())),
            None,
        )
    }

    fn dflag() -> Ex {
        Ex::leaf(
            ExpressionKind::DataPath(DataPath::new(vec![], "flag".into())),
            None,
        )
    }

    fn dflag2() -> Ex {
        Ex::leaf(
            ExpressionKind::DataPath(DataPath::new(vec![], "flag2".into())),
            None,
        )
    }

    /// Pushes a test expression's nodes into a table and returns its root.
    type PushNodes = dyn Fn(&mut ExprTable<ExpressionKind>) -> ExprId;

    /// Test expression; [`Ex::tree`] pushes every node straight into one table.
    #[derive(Clone)]
    struct Ex(Rc<PushNodes>);

    impl Ex {
        fn leaf(kind: ExpressionKind, source: Option<Source>) -> Self {
            Self(Rc::new(move |table| {
                table.push(kind.clone(), source.clone())
            }))
        }

        fn push(&self, table: &mut ExprTable<ExpressionKind>) -> ExprId {
            (self.0)(table)
        }

        fn tree(&self) -> Expression {
            let mut table = ExprTable::default();
            let root = self.push(&mut table);
            ExprTree::new(table, root)
        }
    }

    fn compose_expr(children: &[&Ex], make: impl Fn(&[ExprId]) -> ExpressionKind + 'static) -> Ex {
        let children: Vec<Ex> = children.iter().map(|child| (*child).clone()).collect();
        Ex(Rc::new(move |table| {
            let ids: Vec<ExprId> = children.iter().map(|child| child.push(table)).collect();
            table.push(make(&ids), None)
        }))
    }

    /// A rule as planning converts it: `default`, or a `Piecewise` over `default` (condition
    /// literal `true`) and each `unless <condition> then <result>` arm.
    fn unless_chain(default: Ex, arms: Vec<(Ex, Ex)>) -> Ex {
        if arms.is_empty() {
            return default;
        }
        Ex(Rc::new(move |table| {
            let default_result = default.push(table);
            let default_condition = table.push(
                ExpressionKind::Literal(Box::new(TypedLiteral::from_bool(true))),
                None,
            );
            let mut pieces = vec![(default_condition, default_result)];
            for (condition, result) in &arms {
                let condition = condition.push(table);
                let result = result.push(table);
                pieces.push((condition, result));
            }
            table.push(ExpressionKind::Piecewise(pieces), None)
        }))
    }

    fn add_expr(a: Ex, b: Ex) -> Ex {
        compose_expr(&[&a, &b], |ids| {
            arithmetic_kind(ids[0], vec![(ArithmeticComputation::Add, ids[1])])
        })
    }

    fn mul_expr(a: Ex, b: Ex) -> Ex {
        compose_expr(&[&a, &b], |ids| {
            arithmetic_kind(ids[0], vec![(ArithmeticComputation::Multiply, ids[1])])
        })
    }

    fn div_expr(a: Ex, b: Ex) -> Ex {
        compose_expr(&[&a, &b], |ids| {
            arithmetic_kind(ids[0], vec![(ArithmeticComputation::Divide, ids[1])])
        })
    }

    fn pow_expr(a: Ex, b: Ex) -> Ex {
        compose_expr(&[&a, &b], |ids| {
            arithmetic_kind(ids[0], vec![(ArithmeticComputation::Power, ids[1])])
        })
    }

    fn and_expr(a: Ex, b: Ex) -> Ex {
        compose_expr(&[&a, &b], |ids| {
            logical_kind(LogicalConnective::And, ids[0], vec![ids[1]])
        })
    }

    fn or_expr(a: Ex, b: Ex) -> Ex {
        compose_expr(&[&a, &b], |ids| {
            logical_kind(LogicalConnective::Or, ids[0], vec![ids[1]])
        })
    }

    fn not_expr(inner: Ex) -> Ex {
        compose_expr(&[&inner], |ids| {
            ExpressionKind::LogicalNegation(ids[0], NegationType::Not)
        })
    }

    fn lt_expr(a: Ex, b: Ex) -> Ex {
        compose_expr(&[&a, &b], |ids| {
            ExpressionKind::Comparison(ids[0], ComparisonComputation::LessThan, ids[1])
        })
    }

    fn sub_expr(a: Ex, b: Ex) -> Ex {
        compose_expr(&[&a, &b], |ids| {
            arithmetic_kind(ids[0], vec![(ArithmeticComputation::Subtract, ids[1])])
        })
    }

    fn math_expr(op: MathematicalComputation, operand: Ex) -> Ex {
        compose_expr(&[&operand], move |ids| {
            ExpressionKind::MathematicalComputation(op.clone(), ids[0])
        })
    }

    mod ordered_dispatch_fold {
        use super::*;
        use crate::parsing::source::{Source as ParseSource, SourceType};
        use crate::planning::ordered_dispatch::{region_count, DispatchKey};
        use crate::planning::semantics::{
            primitive_boolean_arc, primitive_date_arc, primitive_number_arc, primitive_text_arc,
            LemmaType, TypeSpecification,
        };
        use crate::Span;

        fn test_source() -> ParseSource {
            ParseSource::new(
                SourceType::Volatile,
                Span {
                    start: 0,
                    end: 0,
                    line: 1,
                    col: 1,
                },
            )
        }

        fn data_path(name: &str) -> DataPath {
            DataPath::new(vec![], name.into())
        }

        fn declared(name: &str, resolved_type: Arc<LemmaType>) -> (DataPath, DataDefinition) {
            (
                data_path(name),
                DataDefinition::TypeDeclaration {
                    resolved_type,
                    declared_suggestion: None,
                    declared_value: None,
                    source: test_source(),
                },
            )
        }

        /// `x: number`, `code: text`, `flag: boolean`, `day: date`, `rate: ratio`.
        fn scope() -> IndexMap<DataPath, DataDefinition> {
            let mut data = IndexMap::new();
            for entry in [
                declared("x", primitive_number_arc().clone()),
                declared("y", primitive_number_arc().clone()),
                declared("code", primitive_text_arc().clone()),
                declared("flag", primitive_boolean_arc().clone()),
                declared("day", primitive_date_arc().clone()),
                declared(
                    "rate",
                    Arc::new(LemmaType::primitive(TypeSpecification::ratio())),
                ),
            ] {
                data.insert(entry.0, entry.1);
            }
            data
        }

        fn path_expr(name: &str) -> Ex {
            Ex::leaf(ExpressionKind::DataPath(data_path(name)), None)
        }

        fn text_expr(text: &str) -> Ex {
            Ex::leaf(
                ExpressionKind::Literal(Box::new(TypedLiteral::text(text.to_string()))),
                None,
            )
        }

        fn compare(left: Ex, op: ComparisonComputation, right: Ex) -> Ex {
            compose_expr(&[&left, &right], move |ids| {
                ExpressionKind::Comparison(ids[0], op.clone(), ids[1])
            })
        }

        /// `default` followed by `unless <condition> then <result>` arms.
        fn chain(default: Ex, arms: Vec<(Ex, Ex)>) -> Ex {
            unless_chain(default, arms)
        }

        /// Normalize `expr` against `scope()` and return the root cell.
        fn fold(expr: &Ex) -> (NormalFormId, NormalFormInterner) {
            let data = scope();
            let mut interner = NormalFormInterner::new();
            let root = to_normal_form_with_data(expr, &mut interner, &data);
            let unit_index = UnitIndex::new();
            let signature_index = SignatureIndex::new();
            let measure_scope = empty_measure_scope(&unit_index, &signature_index);
            let nf = rewrite::normalize(root, Cells::new(&mut interner, measure_scope), None)
                .expect("normalize");
            (nf, interner)
        }

        /// Boundaries and the result-per-region table, as literal display strings.
        fn table(expr: &Ex) -> (Vec<DispatchKey>, Vec<String>) {
            let (nf, interner) = fold(expr);
            let NormalFormKind::OrderedDispatch {
                boundaries,
                regions,
                ..
            } = &interner.get(nf).kind
            else {
                panic!("expected OrderedDispatch, got {:?}", interner.get(nf).kind);
            };
            let rendered = regions
                .iter()
                .map(|region| explanation_display(interner_forms(&interner), region.cell(), None))
                .collect();
            (boundaries.to_vec(), rendered)
        }

        fn assert_declines(expr: &Ex, reason: &str) {
            let (nf, interner) = fold(expr);
            assert!(
                !matches!(
                    interner.get(nf).kind,
                    NormalFormKind::OrderedDispatch { .. }
                ),
                "must not fold ({reason}), got {:?}",
                interner.get(nf).kind
            );
        }

        #[test]
        fn equality_chain_on_text_folds_to_point_regions() {
            let expr = chain(
                num_expr(0),
                vec![
                    (
                        compare(
                            path_expr("code"),
                            ComparisonComputation::Is,
                            text_expr("NL"),
                        ),
                        num_expr(1),
                    ),
                    (
                        compare(
                            path_expr("code"),
                            ComparisonComputation::Is,
                            text_expr("BE"),
                        ),
                        num_expr(2),
                    ),
                ],
            );
            let (boundaries, regions) = table(&expr);
            assert_eq!(
                boundaries,
                vec![
                    DispatchKey::Text(std::sync::Arc::from("BE")),
                    DispatchKey::Text(std::sync::Arc::from("NL"))
                ]
            );
            // Every interval holds the default; only the two points carry results.
            assert_eq!(regions, vec!["0", "2", "0", "1", "0"]);
        }

        #[test]
        fn later_arm_wins_when_two_arms_share_a_constant() {
            let expr = chain(
                num_expr(0),
                vec![
                    (
                        compare(path_expr("x"), ComparisonComputation::Is, num_expr(5)),
                        num_expr(1),
                    ),
                    (
                        compare(path_expr("x"), ComparisonComputation::Is, num_expr(5)),
                        num_expr(2),
                    ),
                ],
            );
            let (boundaries, regions) = table(&expr);
            assert_eq!(boundaries.len(), 1, "duplicates collapse to one breakpoint");
            assert_eq!(regions, vec!["0", "2", "0"]);
        }

        #[test]
        fn greater_than_and_greater_or_equal_differ_at_the_boundary() {
            let strict = chain(
                num_expr(0),
                vec![(
                    compare(
                        path_expr("x"),
                        ComparisonComputation::GreaterThan,
                        num_expr(5),
                    ),
                    num_expr(1),
                )],
            );
            assert_eq!(table(&strict).1, vec!["0", "0", "1"]);

            let inclusive = chain(
                num_expr(0),
                vec![(
                    compare(
                        path_expr("x"),
                        ComparisonComputation::GreaterThanOrEqual,
                        num_expr(5),
                    ),
                    num_expr(1),
                )],
            );
            assert_eq!(table(&inclusive).1, vec!["0", "1", "1"]);
        }

        #[test]
        fn is_not_claims_every_region_but_its_own_point() {
            let expr = chain(
                num_expr(0),
                vec![(
                    compare(path_expr("x"), ComparisonComputation::IsNot, num_expr(5)),
                    num_expr(1),
                )],
            );
            assert_eq!(table(&expr).1, vec!["1", "0", "1"]);
        }

        #[test]
        fn overlapping_ordering_arms_resolve_by_arm_order() {
            // `x > 5 then 1` is shadowed above 10 by the later `x > 10 then 2`.
            let expr = chain(
                num_expr(0),
                vec![
                    (
                        compare(
                            path_expr("x"),
                            ComparisonComputation::GreaterThan,
                            num_expr(5),
                        ),
                        num_expr(1),
                    ),
                    (
                        compare(
                            path_expr("x"),
                            ComparisonComputation::GreaterThan,
                            num_expr(10),
                        ),
                        num_expr(2),
                    ),
                ],
            );
            let (boundaries, regions) = table(&expr);
            assert_eq!(boundaries.len(), 2);
            assert_eq!(regions.len(), region_count(2));
            // below 5 | at 5 | between | at 10 | above 10
            assert_eq!(regions, vec!["0", "0", "1", "1", "2"]);
        }

        #[test]
        fn literal_on_the_left_mirrors_the_operator() {
            let expr = chain(
                num_expr(0),
                vec![(
                    compare(num_expr(5), ComparisonComputation::LessThan, path_expr("x")),
                    num_expr(1),
                )],
            );
            // `5 < x` is `x > 5`: only the region above the boundary.
            assert_eq!(table(&expr).1, vec!["0", "0", "1"]);
        }

        #[test]
        fn dates_fold_with_ordering_operators() {
            let day = |year: i32| {
                Ex::leaf(
                    ExpressionKind::Literal(Box::new(TypedLiteral::date(
                        crate::planning::semantics::SemanticDateTime {
                            year,
                            month: 1,
                            day: 1,
                            hour: 0,
                            minute: 0,
                            second: 0,
                            microsecond: 0,
                            timezone: None,
                        },
                    ))),
                    None,
                )
            };
            let expr = chain(
                num_expr(0),
                vec![(
                    compare(
                        path_expr("day"),
                        ComparisonComputation::GreaterThanOrEqual,
                        day(2020),
                    ),
                    num_expr(1),
                )],
            );
            let (boundaries, regions) = table(&expr);
            assert!(matches!(boundaries.as_slice(), [DispatchKey::Date(_)]));
            assert_eq!(regions, vec!["0", "1", "1"]);
        }

        #[test]
        fn calendar_failure_literal_is_a_planning_error() {
            let invalid_day = Ex::leaf(
                ExpressionKind::Literal(Box::new(TypedLiteral::date(
                    crate::planning::semantics::SemanticDateTime {
                        year: 2020,
                        month: 13,
                        day: 1,
                        hour: 0,
                        minute: 0,
                        second: 0,
                        microsecond: 0,
                        timezone: None,
                    },
                ))),
                None,
            );
            let expr = chain(
                num_expr(0),
                vec![(
                    compare(path_expr("day"), ComparisonComputation::Is, invalid_day),
                    num_expr(1),
                )],
            );
            let data = scope();
            let mut interner = NormalFormInterner::new();
            let root = to_normal_form_with_data(&expr, &mut interner, &data);
            let unit_index = UnitIndex::new();
            let signature_index = SignatureIndex::new();
            let measure_scope = empty_measure_scope(&unit_index, &signature_index);
            let err = rewrite::normalize(root, Cells::new(&mut interner, measure_scope), None)
                .expect_err(
                    "invalid calendar literal must fail planning, not silently stay Piecewise",
                );
            let message = err.to_string();
            assert!(
                message.contains("ordered dispatch") && message.contains("Invalid date"),
                "unexpected error: {message}"
            );
        }

        #[test]
        fn the_pre_image_is_kept_as_the_fold_origin() {
            let expr = chain(
                num_expr(0),
                vec![(
                    compare(path_expr("x"), ComparisonComputation::Is, num_expr(5)),
                    num_expr(1),
                )],
            );
            let (nf, interner) = fold(&expr);
            let origin = interner
                .get(nf)
                .origin
                .expect("dispatch must record its pre-image");
            assert!(
                matches!(interner.get(origin).kind, NormalFormKind::Piecewise(_)),
                "origin must be the Piecewise, got {:?}",
                interner.get(origin).kind
            );
        }

        #[test]
        fn a_collapsed_arm_keeps_the_whole_origin_chain() {
            // The `1 < 0` arm is statically false, so `collapse_piecewise` folds first
            // and hands this pass a cell that already carries an origin.
            let expr = chain(
                num_expr(0),
                vec![
                    (lt_expr(num_expr(1), num_expr(0)), num_expr(9)),
                    (
                        compare(path_expr("x"), ComparisonComputation::Is, num_expr(5)),
                        num_expr(1),
                    ),
                ],
            );
            let (nf, interner) = fold(&expr);
            let origin = interner
                .get(nf)
                .origin
                .expect("dispatch must record its pre-image");
            assert!(
                matches!(interner.get(origin).kind, NormalFormKind::Piecewise(_)),
                "origin must be the Piecewise, got {:?}",
                interner.get(origin).kind
            );
            let recorded = interner
                .get(origin)
                .origin
                .expect("collapse origin must survive the second fold");
            let NormalFormKind::Piecewise(arms) = &interner.get(recorded).kind else {
                panic!("recorded pre-image must be a Piecewise");
            };
            assert_eq!(arms.len(), 3, "the dropped arm must still be recorded");
        }

        #[test]
        fn text_declines_ordering_operators() {
            assert_declines(
                &chain(
                    num_expr(0),
                    vec![(
                        compare(
                            path_expr("code"),
                            ComparisonComputation::GreaterThan,
                            text_expr("NL"),
                        ),
                        num_expr(1),
                    )],
                ),
                "text has no ordering comparison",
            );
        }

        #[test]
        fn arms_on_different_scrutinees_decline() {
            assert_declines(
                &chain(
                    num_expr(0),
                    vec![
                        (
                            compare(path_expr("x"), ComparisonComputation::Is, num_expr(1)),
                            num_expr(1),
                        ),
                        (
                            compare(path_expr("y"), ComparisonComputation::Is, num_expr(2)),
                            num_expr(2),
                        ),
                    ],
                ),
                "two scrutinees",
            );
        }

        #[test]
        fn equality_or_of_is_comparisons_declines() {
            assert_declines(
                &chain(
                    num_expr(0),
                    vec![(
                        or_expr(
                            compare(path_expr("x"), ComparisonComputation::Is, num_expr(1)),
                            compare(path_expr("y"), ComparisonComputation::Is, num_expr(2)),
                        ),
                        num_expr(1),
                    )],
                ),
                "or of equality comparisons is not an And peel",
            );
        }

        #[test]
        fn boolean_scrutinee_declines() {
            assert_declines(
                &chain(
                    num_expr(0),
                    vec![(
                        compare(
                            path_expr("flag"),
                            ComparisonComputation::Is,
                            bool_expr(true),
                        ),
                        num_expr(1),
                    )],
                ),
                "boolean has at most two keys",
            );
        }

        #[test]
        fn mismatched_key_and_scrutinee_types_decline() {
            assert_declines(
                &chain(
                    num_expr(0),
                    vec![(
                        compare(path_expr("code"), ComparisonComputation::Is, num_expr(1)),
                        num_expr(1),
                    )],
                ),
                "text scrutinee against a number key",
            );
            assert_declines(
                &chain(
                    num_expr(0),
                    vec![(
                        compare(path_expr("rate"), ComparisonComputation::Is, num_expr(1)),
                        num_expr(1),
                    )],
                ),
                "ratio scrutinee against a plain number key",
            );
        }

        #[test]
        fn a_determined_expression_scrutinee_folds() {
            // Stamped result_type on the Sum is Number; that is enough to classify.
            let expr = chain(
                num_expr(0),
                vec![(
                    compare(
                        add_expr(path_expr("x"), num_expr(1)),
                        ComparisonComputation::Is,
                        num_expr(5),
                    ),
                    num_expr(1),
                )],
            );
            let (boundaries, regions) = table(&expr);
            assert_eq!(boundaries.len(), 1);
            assert_eq!(regions, vec!["0", "1", "0"]);
        }

        #[test]
        fn conjunctive_key_lut_folds_to_nested_dispatch() {
            let expr = chain(
                num_expr(0),
                vec![
                    (
                        and_expr(
                            compare(path_expr("x"), ComparisonComputation::Is, num_expr(2)),
                            compare(
                                path_expr("y"),
                                ComparisonComputation::GreaterThanOrEqual,
                                num_expr(1),
                            ),
                        ),
                        num_expr(10),
                    ),
                    (
                        and_expr(
                            compare(path_expr("x"), ComparisonComputation::Is, num_expr(2)),
                            compare(
                                path_expr("y"),
                                ComparisonComputation::GreaterThanOrEqual,
                                num_expr(5),
                            ),
                        ),
                        num_expr(20),
                    ),
                    (
                        and_expr(
                            compare(path_expr("x"), ComparisonComputation::Is, num_expr(3)),
                            compare(
                                path_expr("y"),
                                ComparisonComputation::GreaterThanOrEqual,
                                num_expr(1),
                            ),
                        ),
                        num_expr(30),
                    ),
                ],
            );
            let (nf, interner) = fold(&expr);
            let NormalFormKind::OrderedDispatch {
                boundaries,
                regions,
                ..
            } = &interner.get(nf).kind
            else {
                panic!(
                    "expected outer OrderedDispatch, got {:?}",
                    interner.get(nf).kind
                );
            };
            assert_eq!(boundaries.len(), 2, "outer keys are the two zone values");
            // Point regions are nested dispatches; interval regions keep the default.
            let point_bodies: Vec<_> = regions
                .iter()
                .enumerate()
                .filter(|(index, _)| index % 2 == 1)
                .map(|(_, region)| &interner.get(region.cell()).kind)
                .collect();
            assert_eq!(point_bodies.len(), 2);
            for kind in &point_bodies {
                assert!(
                    matches!(kind, NormalFormKind::OrderedDispatch { .. }),
                    "point region must be an inner OrderedDispatch, got {kind:?}"
                );
            }
            for (index, region) in regions.iter().enumerate() {
                if index % 2 == 0 {
                    assert_eq!(
                        explanation_display(interner_forms(&interner), region.cell(), None),
                        "0",
                        "interval region must stay the default"
                    );
                }
            }
        }

        #[test]
        fn conjunctive_key_without_shared_is_scrutinee_declines() {
            // Residuals only; no shared Is key across arms.
            assert_declines(
                &chain(
                    num_expr(0),
                    vec![
                        (
                            and_expr(
                                compare(
                                    path_expr("x"),
                                    ComparisonComputation::GreaterThan,
                                    num_expr(1),
                                ),
                                compare(
                                    path_expr("y"),
                                    ComparisonComputation::GreaterThan,
                                    num_expr(1),
                                ),
                            ),
                            num_expr(1),
                        ),
                        (
                            and_expr(
                                compare(
                                    path_expr("x"),
                                    ComparisonComputation::GreaterThan,
                                    num_expr(2),
                                ),
                                compare(
                                    path_expr("y"),
                                    ComparisonComputation::GreaterThan,
                                    num_expr(2),
                                ),
                            ),
                            num_expr(2),
                        ),
                    ],
                ),
                "no Is key conjunct shared across arms",
            );
        }

        #[test]
        fn an_unknown_data_path_declines() {
            assert_declines(
                &chain(
                    num_expr(0),
                    vec![(
                        compare(path_expr("absent"), ComparisonComputation::Is, num_expr(5)),
                        num_expr(1),
                    )],
                ),
                "the scrutinee has no resolved type in scope",
            );
        }

        #[test]
        fn a_non_comparison_condition_declines() {
            assert_declines(
                &chain(num_expr(0), vec![(path_expr("flag"), num_expr(1))]),
                "a bare boolean condition is not a comparison",
            );
        }

        #[test]
        fn a_comparison_between_two_data_paths_declines() {
            assert_declines(
                &chain(
                    num_expr(0),
                    vec![(
                        compare(path_expr("x"), ComparisonComputation::Is, path_expr("y")),
                        num_expr(1),
                    )],
                ),
                "neither side is a constant",
            );
        }

        /// Every folded cell must agree with its pre-image for every input, which is
        /// the property the whole region model exists to preserve.
        #[test]
        fn folded_table_agrees_with_the_piecewise_it_replaced() {
            let operators = [
                ComparisonComputation::Is,
                ComparisonComputation::IsNot,
                ComparisonComputation::GreaterThan,
                ComparisonComputation::GreaterThanOrEqual,
                ComparisonComputation::LessThan,
                ComparisonComputation::LessThanOrEqual,
            ];
            // Duplicated constants and out-of-order constants are both covered.
            let constants = [5i64, 2, 5, 8];
            let mut checked = 0usize;

            for first in &operators {
                for second in &operators {
                    for third in &operators {
                        let arm_operators = [first, second, third];
                        let arms: Vec<(Ex, Ex)> = arm_operators
                            .iter()
                            .enumerate()
                            .map(|(index, operator)| {
                                (
                                    compare(
                                        path_expr("x"),
                                        (*operator).clone(),
                                        num_expr(constants[index]),
                                    ),
                                    num_expr(index as i64 + 1),
                                )
                            })
                            .collect();
                        let expr = chain(num_expr(0), arms);
                        let (boundaries, regions) = table(&expr);

                        // Sample below, at and above every constant, plus far outside.
                        for probe in [-100i64, 1, 2, 3, 5, 6, 8, 9, 100] {
                            let expected = arm_operators
                                .iter()
                                .enumerate()
                                .rev()
                                .find_map(|(index, operator)| {
                                    predicate_holds(probe, operator, constants[index])
                                        .then_some(index as i64 + 1)
                                })
                                .unwrap_or(0);
                            let region = crate::planning::ordered_dispatch::region_for_value(
                                &boundaries,
                                &DispatchKey::Rational(rational_new(probe, 1)).as_probe(),
                            )
                            .expect("integers compare");
                            assert_eq!(
                                regions[region],
                                expected.to_string(),
                                "operators {first} / {second} / {third} disagree at x = {probe}"
                            );
                            checked += 1;
                        }
                    }
                }
            }
            assert_eq!(checked, 6 * 6 * 6 * 9);
        }

        fn predicate_holds(value: i64, operator: &ComparisonComputation, constant: i64) -> bool {
            match operator {
                ComparisonComputation::Is => value == constant,
                ComparisonComputation::IsNot => value != constant,
                ComparisonComputation::GreaterThan => value > constant,
                ComparisonComputation::GreaterThanOrEqual => value >= constant,
                ComparisonComputation::LessThan => value < constant,
                ComparisonComputation::LessThanOrEqual => value <= constant,
            }
        }
    }

    #[test]
    fn flatten_associative_sum_nested() {
        let inner = add_expr(num_expr(1), num_expr(2));
        let expr = add_expr(inner, num_expr(3));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.root_kind(),
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(6, 1))
        ));
    }

    #[test]
    fn flatten_associative_product_nested() {
        let inner = mul_expr(num_expr(2), num_expr(3));
        let expr = mul_expr(inner, num_expr(4));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.root_kind(),
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(24, 1))
        ));
    }

    #[test]
    fn and_chain_lowers_to_one_flat_cell() {
        let chain = compose_expr(
            &[&bool_expr(true), &bool_expr(true), &bool_expr(false)],
            |ids| logical_kind(LogicalConnective::And, ids[0], vec![ids[1], ids[2]]),
        );
        let mut interner = NormalFormInterner::new();
        let id = to_normal_form_empty(&chain, &mut interner);
        let NormalFormKind::Logical(LogicalConnective::And, children) = &interner.get(id).kind
        else {
            panic!("expected And(..), got {:?}", interner.get(id).kind);
        };
        let [first, second, third] = children.as_slice() else {
            panic!("expected three flat operands, got {children:?}");
        };
        for operand in [first, second, third] {
            assert!(matches!(
                &interner.get(*operand).kind,
                NormalFormKind::Leaf(LeafKind::Literal(_))
            ));
        }
    }

    #[test]
    fn parenthesized_sum_flattens_into_parent() {
        let y = Ex::leaf(
            ExpressionKind::DataPath(DataPath::new(vec![], "y".into())),
            None,
        );
        for expr in [
            add_expr(dx(), add_expr(y.clone(), num_expr(2))),
            add_expr(add_expr(dx(), y.clone()), num_expr(2)),
        ] {
            let norm = normalize_expression(&expr).expect("normalize");
            let ExpressionKind::Arithmetic { steps, .. } = norm.root_kind() else {
                panic!("expected one sum, got {:?}", norm.root_kind());
            };
            assert_eq!(steps.len(), 2, "expected three flat terms, got {norm:?}");
        }
    }

    #[test]
    fn parenthesized_conjunction_flattens_into_parent() {
        let x_below_one = lt_expr(dx(), num_expr(1));
        for expr in [
            and_expr(dflag(), and_expr(dflag2(), x_below_one.clone())),
            and_expr(and_expr(dflag(), dflag2()), x_below_one.clone()),
        ] {
            let norm = normalize_expression(&expr).expect("normalize");
            let ExpressionKind::Logical {
                connective: LogicalConnective::And,
                steps,
                ..
            } = norm.root_kind()
            else {
                panic!("expected one conjunction, got {:?}", norm.root_kind());
            };
            assert_eq!(steps.len(), 2, "expected three flat operands, got {norm:?}");
        }
    }

    #[test]
    fn or_chain_lowers_to_one_flat_cell() {
        let chain = compose_expr(
            &[&bool_expr(false), &bool_expr(false), &bool_expr(true)],
            |ids| logical_kind(LogicalConnective::Or, ids[0], vec![ids[1], ids[2]]),
        );
        let mut interner = NormalFormInterner::new();
        let id = to_normal_form_empty(&chain, &mut interner);
        let NormalFormKind::Logical(LogicalConnective::Or, children) = &interner.get(id).kind
        else {
            panic!("expected Or(..), got {:?}", interner.get(id).kind);
        };
        let [first, second, third] = children.as_slice() else {
            panic!("expected three flat operands, got {children:?}");
        };
        for operand in [first, second, third] {
            assert!(matches!(
                &interner.get(*operand).kind,
                NormalFormKind::Leaf(LeafKind::Literal(_))
            ));
        }
    }

    #[test]
    fn parenthesized_disjunction_flattens_into_parent() {
        let x_below_one = lt_expr(dx(), num_expr(1));
        for expr in [
            or_expr(dflag(), or_expr(dflag2(), x_below_one.clone())),
            or_expr(or_expr(dflag(), dflag2()), x_below_one.clone()),
        ] {
            let norm = normalize_expression(&expr).expect("normalize");
            let ExpressionKind::Logical {
                connective: LogicalConnective::Or,
                steps,
                ..
            } = norm.root_kind()
            else {
                panic!("expected one disjunction, got {:?}", norm.root_kind());
            };
            assert_eq!(steps.len(), 2, "expected three flat operands, got {norm:?}");
        }
    }

    #[test]
    fn mixed_or_and_does_not_flatten() {
        let expr = or_expr(dflag(), and_expr(dflag2(), lt_expr(dx(), num_expr(1))));
        let norm = normalize_expression(&expr).expect("normalize");
        let ExpressionKind::Logical {
            connective: LogicalConnective::Or,
            first,
            steps,
        } = norm.root_kind()
        else {
            panic!("expected or root, got {:?}", norm.root_kind());
        };
        assert_eq!(steps.len(), 1);
        assert!(matches!(
            norm.at(*first).kind(),
            ExpressionKind::DataPath(ref p) if p.data == "flag"
        ));
        assert!(matches!(
            norm.at(steps[0].operand).kind(),
            ExpressionKind::Logical {
                connective: LogicalConnective::And,
                ..
            }
        ));
    }

    #[test]
    fn true_or_data_absorbs_to_true() {
        let norm = normalize_expression(&or_expr(bool_expr(true), dflag())).expect("normalize");
        assert!(matches!(
            norm.root_kind(),
            ExpressionKind::Literal(ref literal)
                if matches!(literal.value, crate::planning::semantics::ValueKind::Boolean(true))
        ));
    }

    #[test]
    fn false_or_data_folds_to_data() {
        let norm = normalize_expression(&or_expr(bool_expr(false), dflag())).expect("normalize");
        assert!(matches!(
            norm.root_kind(),
            ExpressionKind::DataPath(ref path) if path.data == "flag"
        ));
    }

    #[test]
    fn identity_add_zero() {
        let expr = add_expr(dx(), num_expr(0));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(norm.root_kind(), ExpressionKind::DataPath(ref p) if p.data == "x"));
    }

    #[test]
    fn identity_mul_one() {
        let expr = mul_expr(dx(), num_expr(1));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(norm.root_kind(), ExpressionKind::DataPath(ref p) if p.data == "x"));
    }

    #[test]
    fn identity_mul_zero_with_data_path_is_not_folded() {
        // `x * 0` must stay a runtime multiply: folding it to `0` would erase
        // a missing-data veto for `x` and drop `x` from the data manifest.
        let expr = mul_expr(dx(), num_expr(0));
        let norm = normalize_expression(&expr).expect("normalize");
        let Some((left, ArithmeticComputation::Multiply, right)) =
            binary_arithmetic(norm.root_kind())
        else {
            panic!("expected preserved multiply, got {:?}", norm.root_kind());
        };
        assert!(matches!(norm.at(*left).kind(), ExpressionKind::DataPath(ref p) if p.data == "x"));
        assert!(matches!(
            norm.at(*right).kind(),
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(0, 1))
        ));
    }

    #[test]
    fn identity_mul_zero_with_literal_folds() {
        // All-literal products still fold: `7 * 0 → 0`.
        let expr = mul_expr(num_expr(7), num_expr(0));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.root_kind(),
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(0, 1))
        ));
    }

    #[test]
    fn identity_pow_one_and_zero() {
        let p1 = normalize_expression(&pow_expr(dx(), num_expr(1))).expect("normalize");
        assert!(matches!(p1.root_kind(), ExpressionKind::DataPath(ref p) if p.data == "x"));
        // `x ^ 0` must stay a runtime power: folding it to `1` would erase a
        // missing-data veto for `x`.
        let p0 = normalize_expression(&pow_expr(dx(), num_expr(0))).expect("normalize");
        let Some((base, ArithmeticComputation::Power, exp)) = binary_arithmetic(p0.root_kind())
        else {
            panic!("expected preserved power, got {:?}", p0.root_kind());
        };
        assert!(matches!(p0.at(*base).kind(), ExpressionKind::DataPath(ref p) if p.data == "x"));
        assert!(matches!(
            p0.at(*exp).kind(),
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(0, 1))
        ));
        // A total, nonzero literal base still folds: `7 ^ 0 → 1`.
        let literal_pow_zero =
            normalize_expression(&pow_expr(num_expr(7), num_expr(0))).expect("normalize");
        assert!(matches!(
            literal_pow_zero.root_kind(),
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(1, 1))
        ));
    }

    #[test]
    fn double_logical_negation() {
        let norm = normalize_expression(&not_expr(not_expr(bool_expr(true)))).expect("normalize");
        assert!(matches!(
            norm.root_kind(),
            ExpressionKind::Literal(ref l) if matches!(l.value, crate::planning::semantics::ValueKind::Boolean(true))
        ));
    }

    #[test]
    fn double_numeric_negation() {
        let zero = num_expr(0);
        let inner = sub_expr(zero.clone(), dx());
        let expr = sub_expr(zero, inner);
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(norm.root_kind(), ExpressionKind::DataPath(ref p) if p.data == "x"));
    }

    #[test]
    fn double_reciprocal_with_data_path_is_not_collapsed() {
        // `1 / (1 / x)` must stay nested: collapsing it to `x` would erase
        // the inner division-by-zero veto for `x = 0`.
        let one = num_expr(1);
        let inner = div_expr(one.clone(), dx());
        let expr = div_expr(one, inner);
        let norm = normalize_expression(&expr).expect("normalize");
        let Some((_, ArithmeticComputation::Divide, outer_denominator)) =
            binary_arithmetic(norm.root_kind())
        else {
            panic!(
                "expected preserved outer division, got {:?}",
                norm.root_kind()
            );
        };
        let Some((_, ArithmeticComputation::Divide, inner_denominator)) =
            binary_arithmetic(norm.at(*outer_denominator).kind())
        else {
            panic!(
                "expected preserved inner division, got {:?}",
                norm.at(*outer_denominator).kind()
            );
        };
        assert!(
            matches!(norm.at(*outer_denominator).child(*inner_denominator).kind(), ExpressionKind::DataPath(ref p) if p.data == "x")
        );
    }

    #[test]
    fn sqrt_squared_numeric_folds_to_exact_rational() {
        let sqrt2 = math_expr(MathematicalComputation::Sqrt, num_expr(2));
        let expr = pow_expr(sqrt2, num_expr(2));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.root_kind(),
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(2, 1))
        ));
    }

    /// `2^(1/2)` has no exact rational result: constant_fold must not substitute a decimal.
    #[test]
    fn literal_power_with_irrational_result_stays_power() {
        let half_lit = literal_from_folded_rational(rational_new(1, 2));
        let half = Ex::leaf(
            ExpressionKind::Literal(Box::new(TypedLiteral {
                value: half_lit.value,
                lemma_type: primitive_number_arc().clone(),
            })),
            None,
        );
        let expr = pow_expr(num_expr(2), half);
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(
            matches!(
                binary_arithmetic(norm.root_kind()),
                Some((_, ArithmeticComputation::Power, _))
            ),
            "expected symbolic Power, got {:?}",
            norm.root_kind()
        );
    }

    #[test]
    fn negated_conjunction_preserved_for_data_paths() {
        // Non-total conjunctions under negation must not be rewritten:
        // AND propagates every conjunct's veto, so algebraic rewrites change
        // veto observability.
        let expr = not_expr(and_expr(dflag(), dflag2()));
        let norm = normalize_expression(&expr).expect("normalize");
        let ExpressionKind::LogicalNegation(inner, _) = norm.root_kind() else {
            panic!("expected preserved negation, got {:?}", norm.root_kind());
        };
        assert!(
            matches!(
                norm.at(*inner).kind(),
                ExpressionKind::Logical {
                    connective: LogicalConnective::And,
                    ..
                }
            ),
            "expected preserved conjunction under the negation, got {:?}",
            norm.at(*inner).kind()
        );
    }

    #[test]
    fn power_law_nested_power() {
        let expr = pow_expr(pow_expr(dx(), num_expr(2)), num_expr(3));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            binary_arithmetic(norm.root_kind()),
            Some((_, ArithmeticComputation::Power, _))
        ));
        let Some((base, ArithmeticComputation::Power, exp)) = binary_arithmetic(norm.root_kind())
        else {
            unreachable!();
        };
        assert!(matches!(
            norm.at(*base).kind(),
            ExpressionKind::DataPath(ref p) if p.data == "x"
        ));
        assert!(matches!(
            norm.at(*exp).kind(),
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(6, 1))
        ));
    }

    #[test]
    fn power_law_like_base_product() {
        let expr = mul_expr(pow_expr(dx(), num_expr(2)), pow_expr(dx(), num_expr(3)));
        let norm = normalize_expression(&expr).expect("normalize");
        let Some((base, ArithmeticComputation::Power, exp)) = binary_arithmetic(norm.root_kind())
        else {
            panic!("expected single power, got {:?}", norm.root_kind());
        };
        assert!(matches!(norm.at(*base).kind(), ExpressionKind::DataPath(ref p) if p.data == "x"));
        assert!(matches!(
            norm.at(*exp).kind(),
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(5, 1))
        ));
    }

    #[test]
    fn negated_comparison_not_less_than() {
        let cmp = lt_expr(dx(), num_expr(0));
        let norm = normalize_expression(&not_expr(cmp)).expect("normalize");
        let ExpressionKind::Comparison(_, op, _) = norm.root_kind() else {
            panic!("expected Comparison, got {:?}", norm.root_kind());
        };
        assert_eq!(*op, ComparisonComputation::GreaterThanOrEqual);
    }

    #[test]
    fn logical_short_circuit_and_false_with_data_path_folds() {
        // A `false` operand decides `and` whatever the other operands are, so
        // `false and flag` folds to `false`.
        let expr = and_expr(bool_expr(false), dflag());
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.root_kind(),
            ExpressionKind::Literal(ref l) if matches!(l.value, crate::planning::semantics::ValueKind::Boolean(false))
        ));
    }

    #[test]
    fn logical_short_circuit_and_false_all_literal_folds() {
        let expr = and_expr(bool_expr(false), bool_expr(true));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.root_kind(),
            ExpressionKind::Literal(ref l) if matches!(l.value, crate::planning::semantics::ValueKind::Boolean(false))
        ));
    }

    #[test]
    fn logical_and_false_on_right_with_data_path_folds() {
        let expr = and_expr(dflag(), bool_expr(false));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.root_kind(),
            ExpressionKind::Literal(ref l) if matches!(l.value, crate::planning::semantics::ValueKind::Boolean(false))
        ));
    }

    #[test]
    fn logical_idempotency_and_duplicate_paths() {
        let a = dflag();
        let expr = and_expr(a.clone(), a);
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(norm.root_kind(), ExpressionKind::DataPath(ref p) if p.data == "flag"));
    }

    #[test]
    fn unit_conversion_fold_number_to_number() {
        let inner = num_expr(42);
        let expr = compose_expr(&[&inner], |ids| {
            ExpressionKind::UnitConversion(
                ids[0],
                SemanticConversionTarget::Type(PrimitiveKind::Number),
            )
        });
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.root_kind(),
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(42, 1))
        ));
    }

    #[test]
    fn piecewise_collapse_last_true_unless_keeps_only_that_result() {
        let piecewise = unless_chain(num_expr(0), vec![(bool_expr(true), num_expr(42))]);
        let norm = normalize_expression(&piecewise).expect("normalize");
        assert!(matches!(
            norm.root_kind(),
            ExpressionKind::Literal(ref l)
                if matches!(l.value, ValueKind::Number(ref n) if n == &rational_new(42, 1))
        ));
    }

    #[test]
    fn collapsed_unless_false_retains_piecewise_origin() {
        let piecewise = unless_chain(num_expr(1), vec![(bool_expr(false), num_expr(2))]).tree();
        let mut interner = NormalFormInterner::new();
        let completed_rules = HashMap::new();
        let rule_types = HashMap::new();
        let data = IndexMap::new();
        let unit_index = UnitIndex::new();
        let signature_index = SignatureIndex::new();
        let measure_scope = empty_measure_scope(&unit_index, &signature_index);
        let lower = LowerCtx {
            completed_rules: &completed_rules,
            rule_types: &rule_types,
            data: &data,
            row_templates: &EMPTY_ROW_TEMPLATES,
            self_identity: &TEST_SELF_IDENTITY,
            rule_order: &[],
        };
        let root = to_normal_form(
            &piecewise,
            &mut Cells::new(&mut interner, measure_scope),
            &lower,
        );
        let nf = rewrite::normalize(root, Cells::new(&mut interner, measure_scope), None)
            .expect("normalize");
        let cell = interner.get(nf);
        assert!(
            cell.origin.is_some(),
            "collapsed unless must keep piecewise origin, kind={:?}",
            cell.kind
        );
        let origin = cell.origin.expect("origin");
        assert!(
            matches!(interner.get(origin).kind, NormalFormKind::Piecewise(_)),
            "origin must be piecewise, got {:?}",
            interner.get(origin).kind
        );
    }

    #[test]
    fn piecewise_collapse_drops_arm_with_statically_false_condition() {
        let piecewise = unless_chain(
            num_expr(7),
            vec![(lt_expr(num_expr(1), num_expr(0)), num_expr(99))],
        );
        let norm = normalize_expression(&piecewise).expect("normalize");
        assert!(matches!(
            norm.root_kind(),
            ExpressionKind::Literal(ref l)
                if matches!(l.value, ValueKind::Number(ref n) if n == &rational_new(7, 1))
        ));
    }

    #[test]
    fn constant_fold_divide_of_literals_to_number() {
        let expr = div_expr(num_expr(5), num_expr(2));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.root_kind(),
            ExpressionKind::Literal(ref l)
                if matches!(l.value, ValueKind::Number(ref n) if n == &rational_new(5, 2))
        ));
    }

    #[test]
    fn constant_fold_divide_comparison_to_true() {
        let expr = lt_expr(div_expr(num_expr(5), num_expr(2)), num_expr(4));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.root_kind(),
            ExpressionKind::Literal(ref l) if matches!(l.value, ValueKind::Boolean(true))
        ));
    }

    #[test]
    fn constant_fold_nested_arithmetic_comparison_to_true() {
        // (1 + 2) * 3 / 4 < 10
        let expr = lt_expr(
            div_expr(
                mul_expr(add_expr(num_expr(1), num_expr(2)), num_expr(3)),
                num_expr(4),
            ),
            num_expr(10),
        );
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.root_kind(),
            ExpressionKind::Literal(ref l) if matches!(l.value, ValueKind::Boolean(true))
        ));
    }

    #[test]
    fn power_laws_sqrt_squared_is_not_merged_for_data_paths() {
        let x = Ex::leaf(
            ExpressionKind::DataPath(DataPath::new(vec![], "x".into())),
            None,
        );
        let expr = mul_expr(
            math_expr(MathematicalComputation::Sqrt, x.clone()),
            math_expr(MathematicalComputation::Sqrt, x),
        );
        let norm = normalize_expression(&expr).expect("normalize");
        let mut interner = NormalFormInterner::new();
        let id = lower_with_data(&norm, &mut interner, &algebraic_scope());
        assert!(
            matches!(&interner.get(id).kind, NormalFormKind::Product(ref children) if children.len() == 2),
            "sqrt(x)*sqrt(x) must stay a product of two powers, got {:?}",
            interner.get(id).kind
        );
    }

    /// `sqrt_two * sqrt_two` (each factor inlined with `rule_origin = sqrt_two`)
    /// folds to `Leaf(2)` and records the full fold chain as provenance:
    #[test]
    fn exp_log_not_folded_for_data_path() {
        // `exp (log x)` must stay nested: folding it to `x` would erase the
        // `log` domain veto for `x <= 0`.
        let inner = math_expr(MathematicalComputation::Log, dx());
        let expr = math_expr(MathematicalComputation::Exp, inner);
        let norm = normalize_expression(&expr).expect("normalize");
        let ExpressionKind::MathematicalComputation(MathematicalComputation::Exp, inner) =
            norm.root_kind()
        else {
            panic!("expected preserved exp, got {:?}", norm.root_kind());
        };
        assert!(
            matches!(
                norm.at(*inner).kind(),
                ExpressionKind::MathematicalComputation(MathematicalComputation::Log, _)
            ),
            "expected preserved log under exp, got {:?}",
            norm.at(*inner).kind()
        );
    }

    #[test]

    fn constant_fold_add() {
        let expr = add_expr(num_expr(2), num_expr(3));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.root_kind(),
            ExpressionKind::Literal(ref l) if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(5, 1))
        ));
    }

    /// `i.sub.x` on a `uses` instance is one member read: the `uses` slot leaf, then a
    /// row end per hop through the templates, no cell of the dependency in this plan.
    #[test]
    fn member_read_on_import_lowers_to_row_ends() {
        let mut engine = crate::Engine::new();
        engine
            .load([(
                crate::SourceType::Path(Arc::new(std::path::PathBuf::from("member.lemma"))),
                r#"
spec leaf
data x: 3

spec mid
uses sub: leaf

spec outer
uses i: mid
rule r: i.sub.x
"#
                .to_string(),
            )])
            .expect("load member fixture");
        let plan = engine
            .plans
            .get_plans(None, "outer")
            .and_then(|plans| plans.values().next())
            .expect("outer plan");
        let rule = plan
            .rules
            .get(&RulePath::new(Vec::new(), "r".to_string()))
            .expect("rule r");
        let NormalFormKind::SpecMember { base, row, .. } = &plan.normal_form(rule.normal_form).kind
        else {
            panic!(
                "`i.sub.x` lowers to a member read, got {:?}",
                plan.normal_form(rule.normal_form).kind
            );
        };
        assert!(matches!(
            &plan.normal_form(*base).kind,
            NormalFormKind::Leaf(LeafKind::DataPath(path)) if path.input_key() == "i"
        ));
        assert_eq!(
            row.ends,
            vec![
                RowEnd::Data(DataPath::local("sub".to_string())),
                RowEnd::Data(DataPath::local("x".to_string())),
            ]
        );
        assert!(plan.data.keys().all(|path| path.segments.is_empty()));
    }
}
