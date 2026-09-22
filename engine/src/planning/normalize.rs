//! Expression normalization: algebraic simplification for drift-free evaluation.
//!
//! Internal [`NormalForm`] IR used by planning; sealed into [`ExecutionPlan`] tables.

use crate::computation::rational::{NumericFailure, RationalInteger};
use crate::parsing::ast::{
    arithmetic_associativity, arithmetic_precedence, operand_needs_parentheses, Associativity,
    CalendarPeriodUnit, DateCalendarKind, DateRelativeKind, OperandSide,
};
use crate::planning::ordered_dispatch::DispatchKey;
use crate::planning::semantics::{
    primitive_boolean_arc, primitive_date_arc, primitive_number_arc, primitive_ratio_arc,
    primitive_text_arc, primitive_time_arc, ArithmeticComputation, ComparisonComputation,
    DataDefinition, DataPath, Expression, ExpressionKind, LemmaType, LiteralValue,
    MathematicalComputation, ReferenceEnd, RulePath, SemanticConversionTarget, Source,
    TypedLiteral, ValueKind, VetoExpression,
};
use crate::planning::typing::{
    comparison_type, compute_arithmetic_result_type, date_predicate_type,
    infer_range_type_from_endpoint_types, logical_and_type, logical_not_type, math_op_type,
    past_future_range_type, piecewise_type, result_is_veto_type, unit_conversion_type,
    MeasureScope,
};
use crate::Error;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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
    LiteralValue::number_with_type(rational, primitive_number_arc().clone())
}

/// Build unless branches as a single expression (`Piecewise` or lone result).
///
/// Piecewise arms are stored in source order: default first, then each `unless` clause.
/// Compilation walks unless arms in reverse and returns on the first true condition
/// (last match in source order wins).
pub(crate) fn unless_branches_to_piecewise(
    branches: &[(Option<Expression>, Expression)],
) -> Expression {
    assert!(
        !branches.is_empty(),
        "BUG: rule must have at least one branch"
    );
    if branches.len() == 1 {
        return branches[0].1.clone();
    }

    let (_, default_result) = &branches[0];
    let source = default_result.source_location.clone();
    let mut arms: Vec<(Arc<Expression>, Arc<Expression>)> = Vec::with_capacity(branches.len());
    arms.push((
        Arc::new(literal_bool_expression(true, source.clone())),
        Arc::new(default_result.clone()),
    ));
    for (condition, result) in branches.iter().skip(1) {
        let unless_condition = condition
            .as_ref()
            .expect("BUG: non-default branch missing condition");
        arms.push((Arc::new(unless_condition.clone()), Arc::new(result.clone())));
    }

    Expression::with_source(ExpressionKind::Piecewise(arms), source)
}

/// One rule after unless→Piecewise, lower into the shared NormalForm graph, normalize.
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
    /// Measure scope cells are typed in: the plan's unit + signature indexes.
    pub(crate) measure_scope: MeasureScope<'a>,
    pub(crate) max_normalized_expression_nodes: usize,
    pub(crate) max_normal_form_depth: usize,
}

/// Completed rule roots, graph rule types, and reference-chain ends for lower.
struct LowerCtx<'a> {
    completed_rules: &'a HashMap<RulePath, NormalFormId>,
    /// [`crate::planning::graph::RuleNode::rule_type`] for every rule (pre-topo).
    rule_types: &'a HashMap<RulePath, Arc<LemmaType>>,
    reference_ends: &'a IndexMap<DataPath, ReferenceEnd>,
    data: &'a IndexMap<DataPath, DataDefinition>,
}

pub(crate) fn build_normalized_rule(
    ctx: &NormalizeContext<'_>,
    completed_rules: &HashMap<RulePath, NormalFormId>,
    rule_types: &HashMap<RulePath, Arc<LemmaType>>,
    reference_ends: &IndexMap<DataPath, ReferenceEnd>,
    branches: &[(Option<Expression>, Expression)],
    source: Option<Source>,
    interner: &mut NormalFormInterner,
) -> Result<NormalizedRule, Error> {
    let max_normalized_expression_nodes = ctx.max_normalized_expression_nodes;
    let piecewise = unless_branches_to_piecewise(branches);
    let lower = LowerCtx {
        completed_rules,
        rule_types,
        reference_ends,
        data: ctx.data,
    };

    let mut cells = Cells::new(interner, ctx.measure_scope);
    let root = to_normal_form(&piecewise, &mut cells, &lower);
    let source_type = Arc::clone(cells.result_type(root));
    let nf = rewrite::normalize(root, cells, ctx, source.clone())?;

    if normal_form_exceeds_node_budget(&interner.forms, nf, max_normalized_expression_nodes) {
        return Err(expression_node_limit_error(
            max_normalized_expression_nodes,
            source,
        ));
    }

    let depth = normal_form_depth(&interner.forms, nf);
    if depth > ctx.max_normal_form_depth {
        return Err(Error::resource_limit_exceeded(
            "max_normal_form_depth",
            format!("{} levels", ctx.max_normal_form_depth),
            format!("{depth} levels in the normalized graph"),
            "Restructure the rule or reduce repeated references to other rules",
            source,
            None,
            None,
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
        worklist.extend(cell.kind.children());
        if let Some(origin) = cell.origin {
            worklist.push(origin);
        }
    }
    false
}

/// Maximum nesting depth of a NormalForm DAG (leaves and rule references have
/// depth 1). Rule references are evaluation boundaries, so planning measures only
/// intra-rule Kind nesting. Used to guarantee the recursive per-rule evaluator
/// never overflows the stack.
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
            let mut children = cell.kind.children();
            if let Some(origin) = cell.origin {
                children.push(origin);
            }
            for child in children {
                if !memo.contains_key(&child) {
                    stack.push((child, false));
                }
            }
            continue;
        }
        let child_ids = cell.kind.children();
        let mut depth = 1usize;
        for child in &child_ids {
            let child_depth = *memo.get(child).unwrap_or_else(|| {
                panic!(
                    "BUG: child NormalFormId {} missing from depth memo after post-order push",
                    child.0
                )
            });
            depth = depth.max(1 + child_depth);
        }
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

fn literal_bool_expression(value: bool, source: Option<Source>) -> Expression {
    Expression::with_source(
        ExpressionKind::Literal(Box::new(TypedLiteral::from_bool(value))),
        source,
    )
}

/// Leaves in the shipped equation DAG: literals and data only.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum LeafKind {
    Literal(LiteralValue),
    DataPath(crate::planning::semantics::DataPath),
}

/// Index into a [`NormalFormInterner`] / shipped [`crate::planning::execution_plan::ExecutionPlan::normal_forms`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct NormalFormId(u32);

impl NormalFormId {
    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }

    pub(crate) fn from_index(index: usize) -> Self {
        Self(u32::try_from(index).expect("BUG: normal_forms table exceeds u32::MAX"))
    }
}

/// Algebraic structure only. Children are [`NormalFormId`]. Hash/Eq drive cons —
/// no source spans or rule-ref identity here (those live on [`NormalForm`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum NormalFormKind {
    Leaf(LeafKind),
    Sum(Vec<NormalFormId>),
    Product(Vec<NormalFormId>),
    Subtract(NormalFormId, NormalFormId),
    Divide(NormalFormId, NormalFormId),
    Power(NormalFormId, NormalFormId),
    Modulo(NormalFormId, NormalFormId),
    Negate(NormalFormId),
    Reciprocal(NormalFormId),
    Comparison(NormalFormId, ComparisonComputation, NormalFormId),
    And(NormalFormId, NormalFormId),
    Not(NormalFormId),
    MathOp(MathematicalComputation, NormalFormId),
    UnitConversion(NormalFormId, SemanticConversionTarget),
    Veto(VetoExpression),
    DateRelative(DateRelativeKind, NormalFormId),
    DateCalendar(DateCalendarKind, CalendarPeriodUnit, NormalFormId),
    RangeLiteral(NormalFormId, NormalFormId),
    PastFutureRange(DateRelativeKind, NormalFormId),
    RangeContainment(NormalFormId, NormalFormId),
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
        regions: Vec<NormalFormId>,
    },
}

impl NormalFormKind {
    /// Child cell ids in evaluation order. Piecewise yields `cond, body` per arm
    /// from index 0; OrderedDispatch yields the scrutinee then every region body.
    pub(crate) fn children(&self) -> Vec<NormalFormId> {
        match self {
            NormalFormKind::Leaf(_) | NormalFormKind::Veto(_) | NormalFormKind::Now => Vec::new(),
            NormalFormKind::Sum(children) | NormalFormKind::Product(children) => children.clone(),
            NormalFormKind::Subtract(a, b)
            | NormalFormKind::Divide(a, b)
            | NormalFormKind::Power(a, b)
            | NormalFormKind::Modulo(a, b)
            | NormalFormKind::Comparison(a, _, b)
            | NormalFormKind::RangeLiteral(a, b)
            | NormalFormKind::RangeContainment(a, b)
            | NormalFormKind::And(a, b) => vec![*a, *b],
            NormalFormKind::Negate(x)
            | NormalFormKind::Reciprocal(x)
            | NormalFormKind::Not(x)
            | NormalFormKind::MathOp(_, x)
            | NormalFormKind::UnitConversion(x, _)
            | NormalFormKind::DateRelative(_, x)
            | NormalFormKind::DateCalendar(_, _, x)
            | NormalFormKind::PastFutureRange(_, x)
            | NormalFormKind::ResultIsVeto(x) => vec![*x],
            NormalFormKind::Piecewise(arms) => arms
                .iter()
                .flat_map(|(condition, body)| [*condition, *body])
                .collect(),
            NormalFormKind::OrderedDispatch {
                scrutinee, regions, ..
            } => std::iter::once(*scrutinee)
                .chain(regions.iter().copied())
                .collect(),
        }
    }

    /// Same Kind with every child id passed through `map`. Payloads that are not
    /// child ids (operators, literals, targets, boundaries) are cloned.
    pub(crate) fn map_children(&self, mut map: impl FnMut(NormalFormId) -> NormalFormId) -> Self {
        match self {
            NormalFormKind::Leaf(leaf) => NormalFormKind::Leaf(leaf.clone()),
            NormalFormKind::Sum(children) => {
                NormalFormKind::Sum(children.iter().copied().map(map).collect())
            }
            NormalFormKind::Product(children) => {
                NormalFormKind::Product(children.iter().copied().map(map).collect())
            }
            NormalFormKind::And(a, b) => NormalFormKind::And(map(*a), map(*b)),
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
                regions: regions.iter().copied().map(map).collect(),
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
#[derive(Clone, Debug, Serialize, Deserialize)]
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
/// leaves already distinguish via [`LiteralValue::lemma_type`] inside Kind).
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
            worklist.extend(cell.kind.children());
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

pub(crate) fn data_path_result_type(
    data: &IndexMap<DataPath, DataDefinition>,
    path: &DataPath,
) -> Arc<LemmaType> {
    match data.get(path) {
        Some(DataDefinition::Value { resolved_type, .. })
        | Some(DataDefinition::TypeDeclaration { resolved_type, .. })
        | Some(DataDefinition::Reference { resolved_type, .. }) => Arc::clone(resolved_type),
        // Import, or absent from the map (normalize unit tests with an empty
        // data scope): same as graph `infer_data_type` — undetermined.
        Some(DataDefinition::Import { .. }) | None => Arc::new(LemmaType::undetermined_type()),
    }
}

/// The interner together with the measure scope every new cell is typed in.
/// Lowering and every rewrite pass build cells through this, so a cell's type
/// is fixed once, at creation, by [`Cells::type_of`].
pub(crate) struct Cells<'a> {
    interner: &'a mut NormalFormInterner,
    measure_scope: MeasureScope<'a>,
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

    /// Data leaf, typed from its definition in `plan.data`.
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
        children: &[NormalFormId],
        op: ArithmeticComputation,
    ) -> Vec<Arc<LemmaType>> {
        assert!(
            !children.is_empty(),
            "BUG: arithmetic NormalForm with zero children"
        );
        let mut fold_types = Vec::with_capacity(children.len());
        let mut acc = Arc::clone(self.result_type(children[0]));
        fold_types.push(Arc::clone(&acc));
        for &child in &children[1..] {
            acc = compute_arithmetic_result_type(
                acc,
                &op,
                Arc::clone(self.result_type(child)),
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
        compute_arithmetic_result_type(
            Arc::clone(self.result_type(left)),
            &op,
            Arc::clone(self.result_type(right)),
            &self.measure_scope,
        )
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
            NormalFormKind::Now => (primitive_date_arc().clone(), Vec::new()),
            NormalFormKind::Veto(_) => (Arc::new(LemmaType::veto_type()), Vec::new()),
            NormalFormKind::Sum(children) => {
                let fold_types =
                    self.fold_arithmetic_child_types(children, ArithmeticComputation::Add);
                let result_type = Arc::clone(
                    fold_types
                        .last()
                        .expect("BUG: Sum fold_types empty after fold_arithmetic_child_types"),
                );
                (result_type, fold_types)
            }
            NormalFormKind::Product(children) => {
                let fold_types =
                    self.fold_arithmetic_child_types(children, ArithmeticComputation::Multiply);
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
            NormalFormKind::And(a, b) => (
                logical_and_type(self.result_type(*a), self.result_type(*b)),
                Vec::new(),
            ),
            NormalFormKind::Comparison(a, _, b) | NormalFormKind::RangeContainment(a, b) => (
                comparison_type(self.result_type(*a), self.result_type(*b)),
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
            NormalFormKind::Piecewise(arms) => (
                piecewise_type(
                    arms.iter()
                        .map(|(_, body)| Arc::clone(self.result_type(*body))),
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
                NormalFormKind::Sum(children) | NormalFormKind::Product(children) => children
                    .iter()
                    .any(|id| self.result_type(*id).is_undetermined()),
                NormalFormKind::Subtract(a, b)
                | NormalFormKind::Divide(a, b)
                | NormalFormKind::Power(a, b)
                | NormalFormKind::Modulo(a, b)
                | NormalFormKind::And(a, b)
                | NormalFormKind::Comparison(a, _, b)
                | NormalFormKind::RangeContainment(a, b)
                | NormalFormKind::RangeLiteral(a, b) => {
                    self.result_type(*a).is_undetermined() || self.result_type(*b).is_undetermined()
                }
                NormalFormKind::Negate(x)
                | NormalFormKind::Reciprocal(x)
                | NormalFormKind::Not(x)
                | NormalFormKind::DateRelative(_, x)
                | NormalFormKind::DateCalendar(_, _, x)
                | NormalFormKind::MathOp(_, x)
                | NormalFormKind::UnitConversion(x, _)
                | NormalFormKind::PastFutureRange(_, x)
                | NormalFormKind::ResultIsVeto(x) => self.result_type(*x).is_undetermined(),
                NormalFormKind::Piecewise(arms) => arms.iter().any(|(condition, body)| {
                    self.result_type(*condition).is_undetermined()
                        || self.result_type(*body).is_undetermined()
                }),
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

fn to_normal_form(expr: &Expression, cells: &mut Cells<'_>, lower: &LowerCtx<'_>) -> NormalFormId {
    match &expr.kind {
        ExpressionKind::Literal(lit) => {
            cells.intern_literal_leaf(lit.to_literal(), Arc::clone(&lit.lemma_type))
        }
        ExpressionKind::DataPath(p) => {
            if let Some(ReferenceEnd::Rule(rule_path)) = lower.reference_ends.get(p) {
                let body = *lower.completed_rules.get(rule_path).unwrap_or_else(|| {
                    panic!(
                        "BUG: rule-target data '{}' maps to rule '{}' with no completed NormalFormId",
                        p, rule_path.rule
                    )
                });
                cells.intern_rule_ref(
                    body,
                    rule_path.clone(),
                    data_path_result_type(lower.data, p),
                )
            } else {
                cells.intern_data_leaf(p.clone(), data_path_result_type(lower.data, p))
            }
        }
        ExpressionKind::RulePath(path) => {
            let body = *lower.completed_rules.get(path).unwrap_or_else(|| {
                panic!(
                    "BUG: in-plan rule reference '{}' has no completed NormalFormId (topo order broken)",
                    path.rule
                )
            });
            let rule_type = Arc::clone(lower.rule_types.get(path).unwrap_or_else(|| {
                panic!(
                    "BUG: in-plan rule reference '{}' has no RuleNode.rule_type",
                    path.rule
                )
            }));
            cells.intern_rule_ref(body, path.clone(), rule_type)
        }
        ExpressionKind::LogicalAnd(left, right) => {
            let left_id = to_normal_form(left, cells, lower);
            let right_id = to_normal_form(right, cells, lower);
            cells.intern_empty(NormalFormKind::And(left_id, right_id))
        }
        ExpressionKind::Arithmetic(left, ArithmeticComputation::Subtract, right) => {
            let left_id = to_normal_form(left, cells, lower);
            let right_id = to_normal_form(right, cells, lower);
            cells.intern_empty(NormalFormKind::Subtract(left_id, right_id))
        }
        ExpressionKind::Arithmetic(left, ArithmeticComputation::Divide, right) => {
            let left_id = to_normal_form(left, cells, lower);
            let right_id = to_normal_form(right, cells, lower);
            cells.intern_empty(NormalFormKind::Divide(left_id, right_id))
        }
        ExpressionKind::Arithmetic(left, ArithmeticComputation::Add, right) => {
            let left_id = to_normal_form(left, cells, lower);
            let right_id = to_normal_form(right, cells, lower);
            cells.intern_empty(NormalFormKind::Sum(vec![left_id, right_id]))
        }
        ExpressionKind::Arithmetic(left, ArithmeticComputation::Multiply, right) => {
            let left_id = to_normal_form(left, cells, lower);
            let right_id = to_normal_form(right, cells, lower);
            cells.intern_empty(NormalFormKind::Product(vec![left_id, right_id]))
        }
        ExpressionKind::Arithmetic(left, ArithmeticComputation::Power, right) => {
            let left_id = to_normal_form(left, cells, lower);
            let right_id = to_normal_form(right, cells, lower);
            cells.intern_empty(NormalFormKind::Power(left_id, right_id))
        }
        ExpressionKind::Arithmetic(left, ArithmeticComputation::Modulo, right) => {
            let left_id = to_normal_form(left, cells, lower);
            let right_id = to_normal_form(right, cells, lower);
            cells.intern_empty(NormalFormKind::Modulo(left_id, right_id))
        }
        ExpressionKind::Comparison(left, op, right) => {
            let left_id = to_normal_form(left, cells, lower);
            let right_id = to_normal_form(right, cells, lower);
            cells.intern_empty(NormalFormKind::Comparison(left_id, op.clone(), right_id))
        }
        ExpressionKind::UnitConversion(inner, target) => {
            let inner_id = to_normal_form(inner, cells, lower);
            cells.rebuild(
                NormalFormKind::UnitConversion(inner_id, target.clone()),
                expr.source_location.clone(),
                None,
            )
        }
        ExpressionKind::LogicalNegation(inner, _) => {
            let inner_id = to_normal_form(inner, cells, lower);
            cells.intern_empty(NormalFormKind::Not(inner_id))
        }
        ExpressionKind::MathematicalComputation(op, inner) => {
            let inner_id = to_normal_form(inner, cells, lower);
            cells.intern_empty(NormalFormKind::MathOp(op.clone(), inner_id))
        }
        ExpressionKind::Veto(v) => cells.intern_empty(NormalFormKind::Veto(v.clone())),
        ExpressionKind::Now => cells.intern_empty(NormalFormKind::Now),
        ExpressionKind::DateRelative(kind, inner) => {
            let inner_id = to_normal_form(inner, cells, lower);
            cells.intern_empty(NormalFormKind::DateRelative(*kind, inner_id))
        }
        ExpressionKind::DateCalendar(kind, unit, inner) => {
            let inner_id = to_normal_form(inner, cells, lower);
            cells.intern_empty(NormalFormKind::DateCalendar(*kind, *unit, inner_id))
        }
        ExpressionKind::RangeLiteral(left, right) => {
            let left_id = to_normal_form(left, cells, lower);
            let right_id = to_normal_form(right, cells, lower);
            cells.intern_empty(NormalFormKind::RangeLiteral(left_id, right_id))
        }
        ExpressionKind::PastFutureRange(kind, inner) => {
            let inner_id = to_normal_form(inner, cells, lower);
            cells.intern_empty(NormalFormKind::PastFutureRange(*kind, inner_id))
        }
        ExpressionKind::RangeContainment(left, right) => {
            let left_id = to_normal_form(left, cells, lower);
            let right_id = to_normal_form(right, cells, lower);
            cells.intern_empty(NormalFormKind::RangeContainment(left_id, right_id))
        }
        ExpressionKind::ResultIsVeto(operand) => {
            let operand_id = to_normal_form(operand, cells, lower);
            cells.intern_empty(NormalFormKind::ResultIsVeto(operand_id))
        }
        ExpressionKind::Piecewise(arms) => {
            let mut mapped = Vec::with_capacity(arms.len());
            for (condition, result) in arms {
                mapped.push((
                    to_normal_form(condition, cells, lower),
                    to_normal_form(result, cells, lower),
                ));
            }
            cells.intern_empty(NormalFormKind::Piecewise(mapped))
        }
    }
}

/// Precedence for a normal-form kind. Levels match [`crate::parsing::ast::expression_precedence`].
fn normal_form_precedence(kind: &NormalFormKind) -> u8 {
    match kind {
        NormalFormKind::And(..) => 2,
        NormalFormKind::Not(_) | NormalFormKind::Negate(_) => 3,
        NormalFormKind::Comparison(..)
        | NormalFormKind::ResultIsVeto(_)
        | NormalFormKind::RangeContainment(..)
        | NormalFormKind::DateRelative(..)
        | NormalFormKind::DateCalendar(..) => 4,
        NormalFormKind::Sum(_) | NormalFormKind::Subtract(..) => {
            arithmetic_precedence(&ArithmeticComputation::Add)
        }
        NormalFormKind::Product(_)
        | NormalFormKind::Divide(..)
        | NormalFormKind::Modulo(..)
        | NormalFormKind::Reciprocal(_) => arithmetic_precedence(&ArithmeticComputation::Multiply),
        NormalFormKind::Power(..) => arithmetic_precedence(&ArithmeticComputation::Power),
        NormalFormKind::UnitConversion(..) => 8,
        NormalFormKind::RangeLiteral(..) => 9,
        NormalFormKind::Leaf(_)
        | NormalFormKind::MathOp(..)
        | NormalFormKind::Veto(_)
        | NormalFormKind::Now
        | NormalFormKind::PastFutureRange(..)
        | NormalFormKind::Piecewise(_)
        | NormalFormKind::OrderedDispatch { .. } => 10,
    }
}

fn explain_child(
    forms: &[NormalForm],
    id: NormalFormId,
    parent_prec: u8,
    side: OperandSide,
    parent_assoc: Option<Associativity>,
) -> String {
    let text = explanation_display_inner(forms, id);
    let child_prec = {
        let mut focus = id;
        loop {
            let nf = cell(forms, focus);
            if let Some(origin) = nf.origin {
                focus = origin;
                continue;
            }
            if nf.rule_ref.is_some() {
                break 10;
            }
            break normal_form_precedence(&nf.kind);
        }
    };
    if operand_needs_parentheses(child_prec, parent_prec, side, parent_assoc) {
        format!("({text})")
    } else {
        text
    }
}

fn explain_nary(
    forms: &[NormalForm],
    children: &[NormalFormId],
    sep: &str,
    prec: u8,
    assoc: Associativity,
) -> String {
    children
        .iter()
        .enumerate()
        .map(|(i, child)| {
            let side = if i == 0 {
                OperandSide::Left
            } else {
                OperandSide::Right
            };
            explain_child(forms, *child, prec, side, Some(assoc))
        })
        .collect::<Vec<_>>()
        .join(sep)
}

fn explain_binary(
    forms: &[NormalForm],
    left: NormalFormId,
    sep: &str,
    right: NormalFormId,
    prec: u8,
    assoc: Associativity,
) -> String {
    format!(
        "{}{}{}",
        explain_child(forms, left, prec, OperandSide::Left, Some(assoc)),
        sep,
        explain_child(forms, right, prec, OperandSide::Right, Some(assoc))
    )
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
pub(crate) fn explanation_display(forms: &[NormalForm], id: NormalFormId) -> String {
    explanation_display_inner(forms, id)
}

fn explanation_display_inner(forms: &[NormalForm], id: NormalFormId) -> String {
    let nf = cell(forms, id);
    if let Some(origin) = nf.origin {
        return explanation_display_inner(forms, origin);
    }
    if let Some(path) = &nf.rule_ref {
        return path.input_key();
    }
    match &nf.kind {
        NormalFormKind::Leaf(LeafKind::Literal(lit)) => {
            lit.display_value_with_type(nf.result_type.as_ref())
        }
        NormalFormKind::Leaf(LeafKind::DataPath(p)) => p.input_key(),
        NormalFormKind::Comparison(a, op, b) => {
            let prec = normal_form_precedence(&nf.kind);
            format!(
                "{} {op} {}",
                explain_child(forms, *a, prec, OperandSide::Left, None),
                explain_child(forms, *b, prec, OperandSide::Right, None)
            )
        }
        NormalFormKind::And(a, b) => {
            let prec = normal_form_precedence(&nf.kind);
            format!(
                "{} and {}",
                explain_child(
                    forms,
                    *a,
                    prec,
                    OperandSide::Left,
                    Some(Associativity::Left)
                ),
                explain_child(
                    forms,
                    *b,
                    prec,
                    OperandSide::Right,
                    Some(Associativity::Left)
                )
            )
        }
        NormalFormKind::Not(x) => {
            let prec = normal_form_precedence(&nf.kind);
            format!(
                "not {}",
                explain_child(forms, *x, prec, OperandSide::Right, None)
            )
        }
        NormalFormKind::ResultIsVeto(x) => {
            let prec = normal_form_precedence(&nf.kind);
            format!(
                "is veto {}",
                explain_child(forms, *x, prec, OperandSide::Right, None)
            )
        }
        NormalFormKind::Sum(children) => explain_nary(
            forms,
            children,
            " + ",
            normal_form_precedence(&nf.kind),
            Associativity::Left,
        ),
        NormalFormKind::Product(children) => explain_nary(
            forms,
            children,
            " * ",
            normal_form_precedence(&nf.kind),
            Associativity::Left,
        ),
        NormalFormKind::MathOp(op, x) => {
            format!("{op}({})", explanation_display_inner(forms, *x))
        }
        NormalFormKind::UnitConversion(x, target) => {
            let prec = normal_form_precedence(&nf.kind);
            format!(
                "{} as {target}",
                explain_child(forms, *x, prec, OperandSide::Left, None)
            )
        }
        NormalFormKind::Veto(_) => "veto".to_string(),
        NormalFormKind::Piecewise(arms) => {
            let mut parts = Vec::with_capacity(arms.len());
            if let Some((_, default)) = arms.first() {
                parts.push(explanation_display_inner(forms, *default));
            }
            for (cond, result) in arms.iter().skip(1) {
                parts.push(format!(
                    "unless {}: {}",
                    explanation_display_inner(forms, *cond),
                    explanation_display_inner(forms, *result)
                ));
            }
            parts.join(" ")
        }
        NormalFormKind::Subtract(a, b) => explain_binary(
            forms,
            *a,
            " - ",
            *b,
            normal_form_precedence(&nf.kind),
            arithmetic_associativity(&ArithmeticComputation::Subtract),
        ),
        NormalFormKind::Divide(a, b) => explain_binary(
            forms,
            *a,
            " / ",
            *b,
            normal_form_precedence(&nf.kind),
            arithmetic_associativity(&ArithmeticComputation::Divide),
        ),
        NormalFormKind::Power(a, b) => explain_binary(
            forms,
            *a,
            " ^ ",
            *b,
            normal_form_precedence(&nf.kind),
            arithmetic_associativity(&ArithmeticComputation::Power),
        ),
        NormalFormKind::Modulo(a, b) => explain_binary(
            forms,
            *a,
            " % ",
            *b,
            normal_form_precedence(&nf.kind),
            arithmetic_associativity(&ArithmeticComputation::Modulo),
        ),
        NormalFormKind::Negate(x) => {
            let prec = normal_form_precedence(&nf.kind);
            format!(
                "-{}",
                explain_child(forms, *x, prec, OperandSide::Right, None)
            )
        }
        NormalFormKind::Reciprocal(x) => {
            let prec = normal_form_precedence(&nf.kind);
            format!(
                "1/{}",
                explain_child(
                    forms,
                    *x,
                    prec,
                    OperandSide::Right,
                    Some(Associativity::Left)
                )
            )
        }
        NormalFormKind::DateRelative(kind, x) => {
            let prec = normal_form_precedence(&nf.kind);
            format!(
                "{kind} {}",
                explain_child(forms, *x, prec, OperandSide::Right, None)
            )
        }
        NormalFormKind::DateCalendar(kind, unit, x) => {
            let prec = normal_form_precedence(&nf.kind);
            format!(
                "{kind} {unit} {}",
                explain_child(forms, *x, prec, OperandSide::Right, None)
            )
        }
        NormalFormKind::RangeLiteral(a, b) => {
            let prec = normal_form_precedence(&nf.kind);
            format!(
                "{} to {}",
                explain_child(forms, *a, prec, OperandSide::Left, None),
                explain_child(forms, *b, prec, OperandSide::Right, None)
            )
        }
        NormalFormKind::PastFutureRange(kind, x) => {
            let prec = normal_form_precedence(&nf.kind);
            format!(
                "{kind} {}",
                explain_child(forms, *x, prec, OperandSide::Right, None)
            )
        }
        NormalFormKind::RangeContainment(value, range) => {
            let prec = normal_form_precedence(&nf.kind);
            format!(
                "{} in {}",
                explain_child(forms, *value, prec, OperandSide::Left, None),
                explain_child(forms, *range, prec, OperandSide::Right, None)
            )
        }
        NormalFormKind::Now => "now".to_string(),
        NormalFormKind::OrderedDispatch { .. } => unreachable!(
            "BUG: OrderedDispatch always carries its Piecewise pre-image as origin, \
             so the origin return above fires before this arm"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computation::arithmetic::SignatureIndex;
    use crate::computation::rational::{rational_new, rational_one, rational_zero};
    use crate::parsing::ast::PrimitiveKind;
    use crate::planning::semantics::{
        ComparisonComputation, DataPath, ExpressionKind, NegationType, SemanticConversionTarget,
        ValueKind,
    };
    use crate::planning::unit_index::UnitIndex;

    fn interner_forms(interner: &NormalFormInterner) -> &[NormalForm] {
        &interner.forms
    }

    fn empty_measure_scope<'a>(
        unit_index: &'a UnitIndex,
        signature_index: &'a SignatureIndex,
    ) -> MeasureScope<'a> {
        MeasureScope {
            unit_index,
            signature_index,
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
                    declared_fill: None,
                    source: source.clone(),
                },
            );
        }
        data
    }

    /// Normalize through the production pipeline with typed algebraic data in scope.
    fn normalize_expression(expr: &Expression) -> Result<Expression, Error> {
        let data = algebraic_scope();
        normalize_expression_with_data(expr, &data).map(|(expr, _)| expr)
    }

    /// Normalize with a resolved data map in scope, returning the normalized
    /// expression alongside the interner so tests can inspect the shipped shape.
    fn normalize_expression_with_data(
        expr: &Expression,
        data: &IndexMap<DataPath, DataDefinition>,
    ) -> Result<(Expression, NormalFormInterner), Error> {
        let source = expr.source_location.clone();
        let mut interner = NormalFormInterner::new();
        let root = to_normal_form_with_data(expr, &mut interner, data);
        let limits = crate::limits::ResourceLimits::default();
        let unit_index = UnitIndex::new();
        let signature_index = SignatureIndex::new();
        let measure_scope = empty_measure_scope(&unit_index, &signature_index);
        let ctx = NormalizeContext {
            data,
            measure_scope,
            max_normalized_expression_nodes: limits.max_normalized_expression_nodes,
            max_normal_form_depth: limits.max_normal_form_depth,
        };
        let nf = rewrite::normalize(
            root,
            Cells::new(&mut interner, measure_scope),
            &ctx,
            source.clone(),
        )?;
        let expression = to_expression(interner_forms(&interner), nf, source);
        Ok((expression, interner))
    }

    fn to_normal_form_empty(expr: &Expression, interner: &mut NormalFormInterner) -> NormalFormId {
        let data = algebraic_scope();
        to_normal_form_with_data(expr, interner, &data)
    }

    fn to_normal_form_with_data(
        expr: &Expression,
        interner: &mut NormalFormInterner,
        data: &IndexMap<DataPath, DataDefinition>,
    ) -> NormalFormId {
        let completed_rules = HashMap::new();
        let rule_types = HashMap::new();
        let reference_ends = IndexMap::new();
        let lower = LowerCtx {
            completed_rules: &completed_rules,
            rule_types: &rule_types,
            reference_ends: &reference_ends,
            data,
        };
        let unit_index = UnitIndex::new();
        let signature_index = SignatureIndex::new();
        let mut cells = Cells::new(interner, empty_measure_scope(&unit_index, &signature_index));
        to_normal_form(expr, &mut cells, &lower)
    }

    fn to_expression(forms: &[NormalForm], id: NormalFormId, source: Option<Source>) -> Expression {
        let kind = nf_to_kind(forms, id, source.clone());
        Expression::with_source(kind, source)
    }

    fn nf_to_kind(
        forms: &[NormalForm],
        id: NormalFormId,
        source: Option<Source>,
    ) -> ExpressionKind {
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
            NormalFormKind::Sum(children) => sum_to_kind(forms, children, source),
            NormalFormKind::Product(children) => product_to_kind(forms, children, source),
            NormalFormKind::Subtract(a, b) => ExpressionKind::Arithmetic(
                Arc::new(to_expression(forms, *a, source.clone())),
                ArithmeticComputation::Subtract,
                Arc::new(to_expression(forms, *b, source)),
            ),
            NormalFormKind::Divide(a, b) => ExpressionKind::Arithmetic(
                Arc::new(to_expression(forms, *a, source.clone())),
                ArithmeticComputation::Divide,
                Arc::new(to_expression(forms, *b, source)),
            ),
            NormalFormKind::Power(base, exp) => ExpressionKind::Arithmetic(
                Arc::new(to_expression(forms, *base, source.clone())),
                ArithmeticComputation::Power,
                Arc::new(to_expression(forms, *exp, source.clone())),
            ),
            NormalFormKind::Modulo(a, b) => ExpressionKind::Arithmetic(
                Arc::new(to_expression(forms, *a, source.clone())),
                ArithmeticComputation::Modulo,
                Arc::new(to_expression(forms, *b, source.clone())),
            ),
            NormalFormKind::Negate(x) => {
                let zero = Expression::with_source(
                    ExpressionKind::Literal(Box::new(TypedLiteral::number(rational_zero()))),
                    source.clone(),
                );
                ExpressionKind::Arithmetic(
                    Arc::new(zero),
                    ArithmeticComputation::Subtract,
                    Arc::new(to_expression(forms, *x, source)),
                )
            }
            NormalFormKind::Reciprocal(x) => {
                let one = Expression::with_source(
                    ExpressionKind::Literal(Box::new(TypedLiteral::number(rational_one()))),
                    source.clone(),
                );
                ExpressionKind::Arithmetic(
                    Arc::new(one),
                    ArithmeticComputation::Divide,
                    Arc::new(to_expression(forms, *x, source)),
                )
            }
            NormalFormKind::Comparison(a, op, b) => ExpressionKind::Comparison(
                Arc::new(to_expression(forms, *a, source.clone())),
                op.clone(),
                Arc::new(to_expression(forms, *b, source)),
            ),
            NormalFormKind::And(a, b) => ExpressionKind::LogicalAnd(
                Arc::new(to_expression(forms, *a, source.clone())),
                Arc::new(to_expression(forms, *b, source)),
            ),
            NormalFormKind::Not(x) => ExpressionKind::LogicalNegation(
                Arc::new(to_expression(forms, *x, source)),
                NegationType::Not,
            ),
            NormalFormKind::MathOp(op, x) => ExpressionKind::MathematicalComputation(
                op.clone(),
                Arc::new(to_expression(forms, *x, source)),
            ),
            NormalFormKind::UnitConversion(x, target) => ExpressionKind::UnitConversion(
                Arc::new(to_expression(forms, *x, source)),
                target.clone(),
            ),
            NormalFormKind::Veto(v) => ExpressionKind::Veto(v.clone()),
            NormalFormKind::Now => ExpressionKind::Now,
            NormalFormKind::DateRelative(kind, x) => {
                ExpressionKind::DateRelative(*kind, Arc::new(to_expression(forms, *x, source)))
            }
            NormalFormKind::DateCalendar(kind, unit, x) => ExpressionKind::DateCalendar(
                *kind,
                *unit,
                Arc::new(to_expression(forms, *x, source)),
            ),
            NormalFormKind::RangeLiteral(a, b) => ExpressionKind::RangeLiteral(
                Arc::new(to_expression(forms, *a, source.clone())),
                Arc::new(to_expression(forms, *b, source)),
            ),
            NormalFormKind::PastFutureRange(kind, x) => {
                ExpressionKind::PastFutureRange(*kind, Arc::new(to_expression(forms, *x, source)))
            }
            NormalFormKind::RangeContainment(a, b) => ExpressionKind::RangeContainment(
                Arc::new(to_expression(forms, *a, source.clone())),
                Arc::new(to_expression(forms, *b, source)),
            ),
            NormalFormKind::ResultIsVeto(operand) => {
                ExpressionKind::ResultIsVeto(Arc::new(to_expression(forms, *operand, source)))
            }
            NormalFormKind::Piecewise(arms) => ExpressionKind::Piecewise(
                arms.iter()
                    .map(|(condition, result)| {
                        (
                            Arc::new(to_expression(forms, *condition, source.clone())),
                            Arc::new(to_expression(forms, *result, source.clone())),
                        )
                    })
                    .collect(),
            ),
            // A dispatch table has no Expression spelling; its pre-image does.
            NormalFormKind::OrderedDispatch { .. } => nf_to_kind(
                forms,
                nf.origin
                    .expect("BUG: OrderedDispatch without its Piecewise pre-image"),
                source,
            ),
        }
    }

    fn sum_to_kind(
        forms: &[NormalForm],
        children: &[NormalFormId],
        source: Option<Source>,
    ) -> ExpressionKind {
        match children {
            [] => ExpressionKind::Literal(Box::new(TypedLiteral::number(rational_zero()))),
            [one] => nf_to_kind(forms, *one, source),
            [first, rest @ ..] => {
                let mut acc = Expression::with_source(
                    nf_to_kind(forms, *first, source.clone()),
                    source.clone(),
                );
                for c in rest {
                    acc = Expression::with_source(
                        ExpressionKind::Arithmetic(
                            Arc::new(acc),
                            ArithmeticComputation::Add,
                            Arc::new(to_expression(forms, *c, source.clone())),
                        ),
                        source.clone(),
                    );
                }
                acc.kind
            }
        }
    }

    fn product_to_kind(
        forms: &[NormalForm],
        children: &[NormalFormId],
        source: Option<Source>,
    ) -> ExpressionKind {
        match children {
            [] => ExpressionKind::Literal(Box::new(TypedLiteral::number(rational_one()))),
            [one] => nf_to_kind(forms, *one, source),
            [first, rest @ ..] => {
                let mut acc = Expression::with_source(
                    nf_to_kind(forms, *first, source.clone()),
                    source.clone(),
                );
                for c in rest {
                    acc = Expression::with_source(
                        ExpressionKind::Arithmetic(
                            Arc::new(acc),
                            ArithmeticComputation::Multiply,
                            Arc::new(to_expression(forms, *c, source.clone())),
                        ),
                        source.clone(),
                    );
                }
                acc.kind
            }
        }
    }

    fn num_expr(n: i64) -> Expression {
        Expression::with_source(
            ExpressionKind::Literal(Box::new(TypedLiteral::number(rational_new(n, 1)))),
            None,
        )
    }

    fn bool_expr(b: bool) -> Expression {
        Expression::with_source(
            ExpressionKind::Literal(Box::new(TypedLiteral::from_bool(b))),
            None,
        )
    }

    fn dx() -> Expression {
        Expression::with_source(
            ExpressionKind::DataPath(DataPath::new(vec![], "x".into())),
            None,
        )
    }

    fn dflag() -> Expression {
        Expression::with_source(
            ExpressionKind::DataPath(DataPath::new(vec![], "flag".into())),
            None,
        )
    }

    fn dflag2() -> Expression {
        Expression::with_source(
            ExpressionKind::DataPath(DataPath::new(vec![], "flag2".into())),
            None,
        )
    }

    fn add_expr(a: Expression, b: Expression) -> Expression {
        Expression::with_source(
            ExpressionKind::Arithmetic(Arc::new(a), ArithmeticComputation::Add, Arc::new(b)),
            None,
        )
    }

    fn mul_expr(a: Expression, b: Expression) -> Expression {
        Expression::with_source(
            ExpressionKind::Arithmetic(Arc::new(a), ArithmeticComputation::Multiply, Arc::new(b)),
            None,
        )
    }

    fn div_expr(a: Expression, b: Expression) -> Expression {
        Expression::with_source(
            ExpressionKind::Arithmetic(Arc::new(a), ArithmeticComputation::Divide, Arc::new(b)),
            None,
        )
    }

    fn pow_expr(a: Expression, b: Expression) -> Expression {
        Expression::with_source(
            ExpressionKind::Arithmetic(Arc::new(a), ArithmeticComputation::Power, Arc::new(b)),
            None,
        )
    }

    fn and_expr(a: Expression, b: Expression) -> Expression {
        Expression::with_source(ExpressionKind::LogicalAnd(Arc::new(a), Arc::new(b)), None)
    }

    fn not_expr(inner: Expression) -> Expression {
        Expression::with_source(
            ExpressionKind::LogicalNegation(Arc::new(inner), NegationType::Not),
            None,
        )
    }

    fn lt_expr(a: Expression, b: Expression) -> Expression {
        Expression::with_source(
            ExpressionKind::Comparison(Arc::new(a), ComparisonComputation::LessThan, Arc::new(b)),
            None,
        )
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
                    declared_fill: None,
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

        fn path_expr(name: &str) -> Expression {
            Expression::with_source(ExpressionKind::DataPath(data_path(name)), None)
        }

        fn text_expr(text: &str) -> Expression {
            Expression::with_source(
                ExpressionKind::Literal(Box::new(TypedLiteral::text(text.to_string()))),
                None,
            )
        }

        fn compare(left: Expression, op: ComparisonComputation, right: Expression) -> Expression {
            Expression::with_source(
                ExpressionKind::Comparison(Arc::new(left), op, Arc::new(right)),
                None,
            )
        }

        /// `default` followed by `unless <condition> then <result>` arms.
        fn chain(default: Expression, arms: Vec<(Expression, Expression)>) -> Expression {
            let mut branches: Vec<(Option<Expression>, Expression)> = vec![(None, default)];
            for (condition, result) in arms {
                branches.push((Some(condition), result));
            }
            unless_branches_to_piecewise(&branches)
        }

        /// Normalize `expr` against `scope()` and return the root cell.
        fn fold(expr: &Expression) -> (NormalFormId, NormalFormInterner) {
            let data = scope();
            let mut interner = NormalFormInterner::new();
            let root = to_normal_form_with_data(expr, &mut interner, &data);
            let limits = crate::limits::ResourceLimits::default();
            let unit_index = UnitIndex::new();
            let signature_index = SignatureIndex::new();
            let measure_scope = empty_measure_scope(&unit_index, &signature_index);
            let ctx = NormalizeContext {
                data: &data,
                measure_scope,
                max_normalized_expression_nodes: limits.max_normalized_expression_nodes,
                max_normal_form_depth: limits.max_normal_form_depth,
            };
            let nf = rewrite::normalize(root, Cells::new(&mut interner, measure_scope), &ctx, None)
                .expect("normalize");
            (nf, interner)
        }

        /// Boundaries and the result-per-region table, as literal display strings.
        fn table(expr: &Expression) -> (Vec<DispatchKey>, Vec<String>) {
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
                .map(|region| explanation_display(interner_forms(&interner), *region))
                .collect();
            (boundaries.to_vec(), rendered)
        }

        fn assert_declines(expr: &Expression, reason: &str) {
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
                Expression::with_source(
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
            let invalid_day = Expression::with_source(
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
            let limits = crate::limits::ResourceLimits::default();
            let unit_index = UnitIndex::new();
            let signature_index = SignatureIndex::new();
            let measure_scope = empty_measure_scope(&unit_index, &signature_index);
            let ctx = NormalizeContext {
                data: &data,
                measure_scope,
                max_normalized_expression_nodes: limits.max_normalized_expression_nodes,
                max_normal_form_depth: limits.max_normal_form_depth,
            };
            let err =
                rewrite::normalize(root, Cells::new(&mut interner, measure_scope), &ctx, None)
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
                .map(|(_, id)| &interner.get(*id).kind)
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
                        explanation_display(interner_forms(&interner), *region),
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
                        let arms: Vec<(Expression, Expression)> = arm_operators
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
            norm.kind,
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
            norm.kind,
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(24, 1))
        ));
    }

    #[test]
    fn nested_and_stays_binary() {
        let inner = and_expr(bool_expr(true), bool_expr(true));
        let outer = and_expr(inner, bool_expr(false));
        let mut interner = NormalFormInterner::new();
        let id = to_normal_form_empty(&outer, &mut interner);
        let NormalFormKind::And(left, right) = &interner.get(id).kind else {
            panic!("expected And(..), got {:?}", interner.get(id).kind);
        };
        assert!(matches!(
            &interner.get(*left).kind,
            NormalFormKind::And(_, _)
        ));
        assert!(matches!(
            &interner.get(*right).kind,
            NormalFormKind::Leaf(LeafKind::Literal(_))
        ));
    }

    #[test]
    fn identity_add_zero() {
        let expr = add_expr(dx(), num_expr(0));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(norm.kind, ExpressionKind::DataPath(ref p) if p.data == "x"));
    }

    #[test]
    fn identity_mul_one() {
        let expr = mul_expr(dx(), num_expr(1));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(norm.kind, ExpressionKind::DataPath(ref p) if p.data == "x"));
    }

    #[test]
    fn identity_mul_zero_with_data_path_is_not_folded() {
        // `x * 0` must stay a runtime multiply: folding it to `0` would erase
        // a missing-data veto for `x` and drop `x` from the data manifest.
        let expr = mul_expr(dx(), num_expr(0));
        let norm = normalize_expression(&expr).expect("normalize");
        let ExpressionKind::Arithmetic(left, ArithmeticComputation::Multiply, right) = norm.kind
        else {
            panic!("expected preserved multiply, got {:?}", norm.kind);
        };
        assert!(matches!(left.kind, ExpressionKind::DataPath(ref p) if p.data == "x"));
        assert!(matches!(
            right.kind,
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
            norm.kind,
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(0, 1))
        ));
    }

    #[test]
    fn identity_pow_one_and_zero() {
        let p1 = normalize_expression(&pow_expr(dx(), num_expr(1))).expect("normalize");
        assert!(matches!(p1.kind, ExpressionKind::DataPath(ref p) if p.data == "x"));
        // `x ^ 0` must stay a runtime power: folding it to `1` would erase a
        // missing-data veto for `x`.
        let p0 = normalize_expression(&pow_expr(dx(), num_expr(0))).expect("normalize");
        let ExpressionKind::Arithmetic(base, ArithmeticComputation::Power, exp) = p0.kind else {
            panic!("expected preserved power, got {:?}", p0.kind);
        };
        assert!(matches!(base.kind, ExpressionKind::DataPath(ref p) if p.data == "x"));
        assert!(matches!(
            exp.kind,
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(0, 1))
        ));
        // A total, nonzero literal base still folds: `7 ^ 0 → 1`.
        let literal_pow_zero =
            normalize_expression(&pow_expr(num_expr(7), num_expr(0))).expect("normalize");
        assert!(matches!(
            literal_pow_zero.kind,
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(1, 1))
        ));
    }

    #[test]
    fn double_logical_negation() {
        let norm = normalize_expression(&not_expr(not_expr(bool_expr(true)))).expect("normalize");
        assert!(matches!(
            norm.kind,
            ExpressionKind::Literal(ref l) if matches!(l.value, crate::planning::semantics::ValueKind::Boolean(true))
        ));
    }

    #[test]
    fn double_numeric_negation() {
        let zero = num_expr(0);
        let inner = Expression::with_source(
            ExpressionKind::Arithmetic(
                Arc::new(zero.clone()),
                ArithmeticComputation::Subtract,
                Arc::new(dx()),
            ),
            None,
        );
        let expr = Expression::with_source(
            ExpressionKind::Arithmetic(
                Arc::new(zero),
                ArithmeticComputation::Subtract,
                Arc::new(inner),
            ),
            None,
        );
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(norm.kind, ExpressionKind::DataPath(ref p) if p.data == "x"));
    }

    #[test]
    fn double_reciprocal_with_data_path_is_not_collapsed() {
        // `1 / (1 / x)` must stay nested: collapsing it to `x` would erase
        // the inner division-by-zero veto for `x = 0`.
        let one = num_expr(1);
        let inner = Expression::with_source(
            ExpressionKind::Arithmetic(
                Arc::new(one.clone()),
                ArithmeticComputation::Divide,
                Arc::new(dx()),
            ),
            None,
        );
        let expr = Expression::with_source(
            ExpressionKind::Arithmetic(
                Arc::new(one),
                ArithmeticComputation::Divide,
                Arc::new(inner),
            ),
            None,
        );
        let norm = normalize_expression(&expr).expect("normalize");
        let ExpressionKind::Arithmetic(_, ArithmeticComputation::Divide, outer_denominator) =
            norm.kind
        else {
            panic!("expected preserved outer division, got {:?}", norm.kind);
        };
        let ExpressionKind::Arithmetic(_, ArithmeticComputation::Divide, inner_denominator) =
            &outer_denominator.kind
        else {
            panic!(
                "expected preserved inner division, got {:?}",
                outer_denominator.kind
            );
        };
        assert!(matches!(inner_denominator.kind, ExpressionKind::DataPath(ref p) if p.data == "x"));
    }

    #[test]
    fn sqrt_squared_numeric_folds_to_exact_rational() {
        let sqrt2 = Expression::with_source(
            ExpressionKind::MathematicalComputation(
                MathematicalComputation::Sqrt,
                Arc::new(num_expr(2)),
            ),
            None,
        );
        let expr = pow_expr(sqrt2, num_expr(2));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.kind,
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(2, 1))
        ));
    }

    /// `2^(1/2)` has no exact rational result: constant_fold must not substitute a decimal.
    #[test]
    fn literal_power_with_irrational_result_stays_power() {
        let half_lit = literal_from_folded_rational(rational_new(1, 2));
        let half = Expression::with_source(
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
                norm.kind,
                ExpressionKind::Arithmetic(_, ArithmeticComputation::Power, _)
            ),
            "expected symbolic Power, got {:?}",
            norm.kind
        );
    }

    #[test]
    fn negated_conjunction_preserved_for_data_paths() {
        // Non-total conjunctions under negation must not be rewritten:
        // AND propagates every conjunct's veto, so algebraic rewrites change
        // veto observability.
        let expr = not_expr(and_expr(dflag(), dflag2()));
        let norm = normalize_expression(&expr).expect("normalize");
        let ExpressionKind::LogicalNegation(inner, _) = norm.kind else {
            panic!("expected preserved negation, got {:?}", norm.kind);
        };
        assert!(
            matches!(inner.kind, ExpressionKind::LogicalAnd(_, _)),
            "expected preserved conjunction under the negation, got {:?}",
            inner.kind
        );
    }

    #[test]
    fn power_law_nested_power() {
        let expr = pow_expr(pow_expr(dx(), num_expr(2)), num_expr(3));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.kind,
            ExpressionKind::Arithmetic(_, ArithmeticComputation::Power, _)
        ));
        let ExpressionKind::Arithmetic(base, ArithmeticComputation::Power, exp) = norm.kind else {
            unreachable!();
        };
        assert!(matches!(
            base.kind,
            ExpressionKind::DataPath(ref p) if p.data == "x"
        ));
        assert!(matches!(
            exp.kind,
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(6, 1))
        ));
    }

    #[test]
    fn power_law_like_base_product() {
        let expr = mul_expr(pow_expr(dx(), num_expr(2)), pow_expr(dx(), num_expr(3)));
        let norm = normalize_expression(&expr).expect("normalize");
        let ExpressionKind::Arithmetic(base, ArithmeticComputation::Power, exp) = norm.kind else {
            panic!("expected single power, got {:?}", norm.kind);
        };
        assert!(matches!(base.kind, ExpressionKind::DataPath(ref p) if p.data == "x"));
        assert!(matches!(
            exp.kind,
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(5, 1))
        ));
    }

    #[test]
    fn negated_comparison_not_less_than() {
        let cmp = lt_expr(dx(), num_expr(0));
        let norm = normalize_expression(&not_expr(cmp)).expect("normalize");
        let ExpressionKind::Comparison(_, op, _) = norm.kind else {
            panic!("expected Comparison, got {:?}", norm.kind);
        };
        assert_eq!(op, ComparisonComputation::GreaterThanOrEqual);
    }

    #[test]
    fn logical_short_circuit_and_false_with_data_path_folds() {
        // Left `false` decides before the right can veto (same as evaluation
        // short-circuit), so `false and flag` folds to `false`.
        let expr = and_expr(bool_expr(false), dflag());
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.kind,
            ExpressionKind::Literal(ref l) if matches!(l.value, crate::planning::semantics::ValueKind::Boolean(false))
        ));
    }

    #[test]
    fn logical_short_circuit_and_false_all_literal_folds() {
        let expr = and_expr(bool_expr(false), bool_expr(true));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.kind,
            ExpressionKind::Literal(ref l) if matches!(l.value, crate::planning::semantics::ValueKind::Boolean(false))
        ));
    }

    #[test]
    fn logical_idempotency_and_duplicate_paths() {
        let a = dflag();
        let expr = and_expr(a.clone(), a);
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(norm.kind, ExpressionKind::DataPath(ref p) if p.data == "flag"));
    }

    #[test]
    fn unit_conversion_fold_number_to_number() {
        let inner = num_expr(42);
        let expr = Expression::with_source(
            ExpressionKind::UnitConversion(
                Arc::new(inner),
                SemanticConversionTarget::Type(PrimitiveKind::Number),
            ),
            None,
        );
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.kind,
            ExpressionKind::Literal(ref l)
                if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(42, 1))
        ));
    }

    #[test]
    fn piecewise_single_branch() {
        let branches = vec![(None as Option<Expression>, num_expr(9))];
        let expr = unless_branches_to_piecewise(&branches);
        assert!(matches!(expr.kind, ExpressionKind::Literal(_)));
    }

    #[test]
    fn piecewise_unless_arms_in_source_order() {
        let c_big = lt_expr(num_expr(10), dx());
        let c_small = lt_expr(num_expr(5), dx());
        let branches = vec![
            (None, num_expr(0)),
            (Some(c_big.clone()), num_expr(100)),
            (Some(c_small.clone()), num_expr(50)),
        ];
        let ExpressionKind::Piecewise(arms) = unless_branches_to_piecewise(&branches).kind else {
            panic!("expected Piecewise for unless chain");
        };
        assert_eq!(arms.len(), 3);
        assert!(matches!(
            arms[0].0.kind,
            ExpressionKind::Literal(ref l) if matches!(l.value, ValueKind::Boolean(true))
        ));
        assert!(
            matches!(arms[0].1.kind, ExpressionKind::Literal(ref l) if matches!(l.value, ValueKind::Number(ref n) if n == &rational_new(0, 1)))
        );
        assert!(matches!(
            arms[1].0.kind,
            ExpressionKind::Comparison(_, _, _)
        ));
        assert!(
            matches!(arms[1].1.kind, ExpressionKind::Literal(ref l) if matches!(l.value, ValueKind::Number(ref n) if n == &rational_new(100, 1)))
        );
        assert!(matches!(
            arms[2].0.kind,
            ExpressionKind::Comparison(_, _, _)
        ));
        assert!(
            matches!(arms[2].1.kind, ExpressionKind::Literal(ref l) if matches!(l.value, ValueKind::Number(ref n) if n == &rational_new(50, 1)))
        );
    }

    #[test]
    fn piecewise_collapse_last_true_unless_keeps_only_that_result() {
        let branches = vec![(None, num_expr(0)), (Some(bool_expr(true)), num_expr(42))];
        let piecewise = unless_branches_to_piecewise(&branches);
        let norm = normalize_expression(&piecewise).expect("normalize");
        assert!(matches!(
            norm.kind,
            ExpressionKind::Literal(ref l)
                if matches!(l.value, ValueKind::Number(ref n) if n == &rational_new(42, 1))
        ));
    }

    #[test]
    fn collapsed_unless_false_retains_piecewise_origin() {
        let branches = vec![(None, num_expr(1)), (Some(bool_expr(false)), num_expr(2))];
        let piecewise = unless_branches_to_piecewise(&branches);
        let mut interner = NormalFormInterner::new();
        let completed_rules = HashMap::new();
        let rule_types = HashMap::new();
        let reference_ends = IndexMap::new();
        let data = IndexMap::new();
        let unit_index = UnitIndex::new();
        let signature_index = SignatureIndex::new();
        let measure_scope = empty_measure_scope(&unit_index, &signature_index);
        let lower = LowerCtx {
            completed_rules: &completed_rules,
            rule_types: &rule_types,
            reference_ends: &reference_ends,
            data: &data,
        };
        let root = to_normal_form(
            &piecewise,
            &mut Cells::new(&mut interner, measure_scope),
            &lower,
        );
        let limits = crate::limits::ResourceLimits::default();
        let ctx = NormalizeContext {
            data: &data,
            measure_scope,
            max_normalized_expression_nodes: limits.max_normalized_expression_nodes,
            max_normal_form_depth: limits.max_normal_form_depth,
        };
        let nf = rewrite::normalize(root, Cells::new(&mut interner, measure_scope), &ctx, None)
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
        let branches = vec![
            (None, num_expr(7)),
            (Some(lt_expr(num_expr(1), num_expr(0))), num_expr(99)),
        ];
        let piecewise = unless_branches_to_piecewise(&branches);
        let norm = normalize_expression(&piecewise).expect("normalize");
        assert!(matches!(
            norm.kind,
            ExpressionKind::Literal(ref l)
                if matches!(l.value, ValueKind::Number(ref n) if n == &rational_new(7, 1))
        ));
    }

    #[test]
    fn constant_fold_divide_of_literals_to_number() {
        let expr = div_expr(num_expr(5), num_expr(2));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.kind,
            ExpressionKind::Literal(ref l)
                if matches!(l.value, ValueKind::Number(ref n) if n == &rational_new(5, 2))
        ));
    }

    #[test]
    fn constant_fold_divide_comparison_to_true() {
        let expr = lt_expr(div_expr(num_expr(5), num_expr(2)), num_expr(4));
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.kind,
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
            norm.kind,
            ExpressionKind::Literal(ref l) if matches!(l.value, ValueKind::Boolean(true))
        ));
    }

    #[test]
    fn power_laws_sqrt_squared_is_not_merged_for_data_paths() {
        let x = Expression::with_source(
            ExpressionKind::DataPath(DataPath::new(vec![], "x".into())),
            None,
        );
        let expr = Expression::with_source(
            ExpressionKind::Arithmetic(
                Arc::new(Expression::with_source(
                    ExpressionKind::MathematicalComputation(
                        MathematicalComputation::Sqrt,
                        Arc::new(x.clone()),
                    ),
                    None,
                )),
                ArithmeticComputation::Multiply,
                Arc::new(Expression::with_source(
                    ExpressionKind::MathematicalComputation(
                        MathematicalComputation::Sqrt,
                        Arc::new(x),
                    ),
                    None,
                )),
            ),
            None,
        );
        let norm = normalize_expression(&expr).expect("normalize");
        let mut interner = NormalFormInterner::new();
        let id = to_normal_form_empty(&norm, &mut interner);
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
        let inner = Expression::with_source(
            ExpressionKind::MathematicalComputation(MathematicalComputation::Log, Arc::new(dx())),
            None,
        );
        let expr = Expression::with_source(
            ExpressionKind::MathematicalComputation(MathematicalComputation::Exp, Arc::new(inner)),
            None,
        );
        let norm = normalize_expression(&expr).expect("normalize");
        let ExpressionKind::MathematicalComputation(MathematicalComputation::Exp, inner) =
            norm.kind
        else {
            panic!("expected preserved exp, got {:?}", norm.kind);
        };
        assert!(
            matches!(
                inner.kind,
                ExpressionKind::MathematicalComputation(MathematicalComputation::Log, _)
            ),
            "expected preserved log under exp, got {:?}",
            inner.kind
        );
    }

    #[test]

    fn constant_fold_add() {
        let expr = Expression::with_source(
            ExpressionKind::Arithmetic(
                Arc::new(num_expr(2)),
                ArithmeticComputation::Add,
                Arc::new(num_expr(3)),
            ),
            None,
        );
        let norm = normalize_expression(&expr).expect("normalize");
        assert!(matches!(
            norm.kind,
            ExpressionKind::Literal(ref l) if matches!(l.value, crate::planning::semantics::ValueKind::Number(ref n) if n == &rational_new(5, 1))
        ));
    }
}
