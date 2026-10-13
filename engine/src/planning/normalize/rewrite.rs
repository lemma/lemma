//! Rewrite driver: one post-order walk over a lowered [`NormalForm`] graph, then
//! an ordered chain of local rewrites at every cell.
//!
//! A pass sees a cell whose children are already normalized and returns
//! `Some(new_id)` when it rewrote the cell, `None` when its pattern does not
//! apply. Children a pass builds are normalized before the next pass runs, so
//! every pass reads normalized children. Passes never descend, never enter a
//! rule reference, and never re-intern an unchanged cell.

use super::{
    literal_from_folded_rational, normalization_error, ArithmeticTerm, Cells, DispatchRegion,
    LeafKind, NormalFormId, NormalFormKind,
};
use crate::computation::comparison::comparison_operation;
use crate::computation::rational::{
    rational_is_zero, rational_new, rational_one, rational_operation, rational_zero,
    NumericFailure, NumericOperation, RationalInteger,
};
use crate::parsing::ast::PrimitiveKind;
use crate::planning::ordered_dispatch::{
    classify_dispatch, dispatch_key_of_literal, paint_dispatch_regions, region_for_value,
    sorted_unique_boundaries, DispatchClass, DispatchKeyBuildError,
};
use crate::planning::semantics::{
    mirrored_comparison, negated_comparison, ComparisonComputation, LemmaType, LiteralValue,
    MathematicalComputation, SemanticConversionTarget, Source, ValueKind,
};
use crate::Error;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Normalize `root`: every reachable cell, children first, through [`PASSES`].
pub(super) fn normalize(
    root: NormalFormId,
    cells: Cells<'_>,
    source: Option<Source>,
) -> Result<NormalFormId, Error> {
    let mut normalizer = Normalizer {
        cells,
        source,
        normalized: HashMap::new(),
        numeric: HashMap::new(),
        in_progress: HashSet::new(),
    };
    normalizer.normalize(root)
}

struct Normalizer<'a> {
    cells: Cells<'a>,
    /// Span of the rule being normalized, for planning errors raised by folds.
    source: Option<Source>,
    /// Pre-image → normalized result for this walk. Scoped to one rule: the
    /// cells is shared across rules and temporal slices, whose contexts differ.
    normalized: HashMap<NormalFormId, NormalFormId>,
    /// Memo for [`numeric_only`]. Cell ids are immutable, so an answer stays valid
    /// for the whole walk.
    numeric: HashMap<NormalFormId, bool>,
    /// Cells visited and not yet normalized. Meeting one again means a pass
    /// builds its own pre-image back: a cycle, which must not loop.
    in_progress: HashSet<NormalFormId>,
}

/// Work item of the normalization walk. Explicit so that nesting depth costs heap,
/// not Rust stack.
enum Task {
    /// Normalize the cell: unnormalized children first, then [`Task::Rebuild`].
    Visit(NormalFormId),
    /// Children are normalized: run the passes over the cell.
    Rebuild(NormalFormId),
    /// A pass built fresh children: resume the cell with its next pass once they
    /// are normalized.
    Resume {
        origin: NormalFormId,
        rewritten: NormalFormId,
        children: Vec<NormalFormId>,
        first_fresh: usize,
        next_pass: usize,
    },
}

/// One local rewrite. `Ok(None)`: pattern does not apply, cell untouched.
#[derive(Clone, Copy)]
enum Pass {
    ExpandNumericSubtractDivide,
    FlattenAssociative,
    FlattenConjunction,
    PowerLaws,
    EliminateIdentities,
    DoubleNegateReciprocal,
    MathIdentities,
    ConstantFold,
    FoldNumberConversion,
    NegatedComparisons,
    LogicalIdempotency,
    CollapsePiecewise,
    CanonicalOrder,
    OrderedDispatch,
}

impl Pass {
    fn apply(
        self,
        normalizer: &mut Normalizer<'_>,
        id: NormalFormId,
    ) -> Result<Option<NormalFormId>, Error> {
        match self {
            Pass::ExpandNumericSubtractDivide => expand_numeric_subtract_divide(normalizer, id),
            Pass::FlattenAssociative => flatten_associative(normalizer, id),
            Pass::FlattenConjunction => flatten_conjunction(normalizer, id),
            Pass::PowerLaws => power_laws(normalizer, id),
            Pass::EliminateIdentities => eliminate_identities(normalizer, id),
            Pass::DoubleNegateReciprocal => double_negate_reciprocal(normalizer, id),
            Pass::MathIdentities => math_identities(normalizer, id),
            Pass::ConstantFold => constant_fold(normalizer, id),
            Pass::FoldNumberConversion => fold_number_conversion(normalizer, id),
            Pass::NegatedComparisons => negated_comparisons(normalizer, id),
            Pass::LogicalIdempotency => logical_idempotency(normalizer, id),
            Pass::CollapsePiecewise => collapse_piecewise(normalizer, id),
            Pass::CanonicalOrder => canonical_order(normalizer, id),
            Pass::OrderedDispatch => ordered_dispatch(normalizer, id),
        }
    }
}

/// Rewrites applied at every cell, in this order, once each.
const PASSES: [Pass; 14] = [
    Pass::ExpandNumericSubtractDivide,
    Pass::FlattenAssociative,
    Pass::FlattenConjunction,
    Pass::PowerLaws,
    Pass::EliminateIdentities,
    Pass::DoubleNegateReciprocal,
    Pass::MathIdentities,
    Pass::ConstantFold,
    Pass::FoldNumberConversion,
    Pass::NegatedComparisons,
    Pass::LogicalIdempotency,
    Pass::CollapsePiecewise,
    Pass::CanonicalOrder,
    Pass::OrderedDispatch,
];

impl Normalizer<'_> {
    /// Normalized form of `id` if this walk already has it. A rule reference is its
    /// own normal form: its body was normalized when that rule completed.
    fn normalized_form(&self, id: NormalFormId) -> Option<NormalFormId> {
        if self.cells.get(id).rule_ref.is_some() {
            return Some(id);
        }
        self.normalized.get(&id).copied()
    }

    /// Post-order walk over the graph under `root`: every child is normalized before
    /// its parent's passes run. Passes that build unnormalized children suspend their
    /// cell (see [`Task::Resume`]), so nesting depth lives on the heap.
    fn normalize(&mut self, root: NormalFormId) -> Result<NormalFormId, Error> {
        let mut tasks = vec![Task::Visit(root)];
        while let Some(task) = tasks.pop() {
            match task {
                Task::Visit(id) => {
                    if self.normalized_form(id).is_some() {
                        continue;
                    }
                    assert!(
                        self.in_progress.insert(id),
                        "BUG: normalization revisits cell {id:?} before it completes"
                    );
                    tasks.push(Task::Rebuild(id));
                    self.cells.get(id).kind.for_each_child(|child| {
                        if self.normalized_form(child).is_none() {
                            tasks.push(Task::Visit(child));
                        }
                    });
                }
                Task::Rebuild(id) => {
                    let children = self.cells.get(id).kind.children();
                    let with_children = self.rebuild_normalized(id, children);
                    self.advance(&mut tasks, id, with_children, 0)?;
                }
                Task::Resume {
                    origin,
                    rewritten,
                    children,
                    first_fresh,
                    next_pass,
                } => {
                    let replaced = self.replace_fresh(&children, first_fresh);
                    let current = self.rebuild_with_children(rewritten, children, replaced);
                    self.advance(&mut tasks, origin, current, next_pass)?;
                }
            }
        }
        Ok(self
            .normalized_form(root)
            .expect("BUG: root not normalized after post-order walk"))
    }

    /// Runs passes `next_pass..` on `current`. When they all apply, `origin` is
    /// normalized to the result. A pass that builds unnormalized children suspends
    /// `origin`: those children are queued, then `origin` resumes after them.
    fn advance(
        &mut self,
        tasks: &mut Vec<Task>,
        origin: NormalFormId,
        mut current: NormalFormId,
        next_pass: usize,
    ) -> Result<(), Error> {
        for (pass_index, &pass) in PASSES.iter().enumerate().skip(next_pass) {
            let first_fresh = self.cells.len();
            let Some(rewritten) = pass.apply(self, current)? else {
                continue;
            };
            let children = self.cells.get(rewritten).kind.children();
            let pending: Vec<NormalFormId> = children
                .iter()
                .copied()
                .filter(|child| {
                    child.index() >= first_fresh && self.normalized_form(*child).is_none()
                })
                .collect();
            if !pending.is_empty() {
                tasks.push(Task::Resume {
                    origin,
                    rewritten,
                    children,
                    first_fresh,
                    next_pass: pass_index + 1,
                });
                tasks.extend(pending.into_iter().map(Task::Visit));
                return Ok(());
            }
            let replaced = self.replace_fresh(&children, first_fresh);
            current = self.rebuild_with_children(rewritten, children, replaced);
        }
        self.in_progress.remove(&origin);
        self.normalized.insert(origin, current);
        self.normalized.insert(current, current);
        Ok(())
    }

    /// `children` with each cell built by the current pass replaced by its normalized
    /// form. Older children were normalized before the pass ran.
    fn replace_fresh(&self, children: &[NormalFormId], first_fresh: usize) -> Vec<NormalFormId> {
        children
            .iter()
            .map(|child| {
                if child.index() >= first_fresh {
                    self.normalized_form(*child)
                        .expect("BUG: fresh child not normalized before its parent resumes")
                } else {
                    *child
                }
            })
            .collect()
    }

    /// `id` with every child replaced by its normalized form. Children are
    /// normalized by [`Self::normalize`] before their parent is visited.
    fn rebuild_normalized(
        &mut self,
        id: NormalFormId,
        children: Vec<NormalFormId>,
    ) -> NormalFormId {
        let replaced: Vec<NormalFormId> = children
            .iter()
            .map(|child| {
                self.normalized_form(*child)
                    .expect("BUG: child not normalized before its parent")
            })
            .collect();
        self.rebuild_with_children(id, children, replaced)
    }

    /// `id` rebuilt over `replaced` when any child changed; otherwise `id` itself.
    /// The rebuilt cell keeps `id`'s source and origin.
    fn rebuild_with_children(
        &mut self,
        id: NormalFormId,
        children: Vec<NormalFormId>,
        replaced: Vec<NormalFormId>,
    ) -> NormalFormId {
        if replaced == children {
            return id;
        }
        let cell = self.cells.get(id);
        let mut replacement = replaced.into_iter();
        let kind = cell.kind.map_children(|_| {
            replacement
                .next()
                .expect("BUG: map_children visits more children than children()")
        });
        assert!(
            replacement.next().is_none(),
            "BUG: map_children visits fewer children than children()"
        );
        let source = cell.source.clone();
        let origin = cell.origin;
        self.cells.rebuild(kind, source, origin)
    }
}

fn expand_numeric_subtract_divide(
    n: &mut Normalizer<'_>,
    id: NormalFormId,
) -> Result<Option<NormalFormId>, Error> {
    let rewritten = match &n.cells.get(id).kind {
        NormalFormKind::Subtract(a, b)
            if numeric_only(&n.cells, &mut n.numeric, *a)
                && numeric_only(&n.cells, &mut n.numeric, *b) =>
        {
            let (a, b) = (*a, *b);
            n.cells.fold_into(
                NormalFormKind::Sum(vec![ArithmeticTerm::direct(a), ArithmeticTerm::inverted(b)]),
                id,
            )
        }
        NormalFormKind::Divide(a, b)
            if numeric_only(&n.cells, &mut n.numeric, *a)
                && numeric_only(&n.cells, &mut n.numeric, *b) =>
        {
            let (a, b) = (*a, *b);
            n.cells.fold_into(
                NormalFormKind::Product(vec![
                    ArithmeticTerm::direct(a),
                    ArithmeticTerm::inverted(b),
                ]),
                id,
            )
        }
        _ => return Ok(None),
    };
    Ok(Some(rewritten))
}

/// Whether `root` is a tree of numeric literals under sum, product, subtract,
/// divide, power, negate, reciprocal. Iterative; answers are memoized in `memo`.
pub(super) fn numeric_only(
    cells: &Cells<'_>,
    memo: &mut HashMap<NormalFormId, bool>,
    root: NormalFormId,
) -> bool {
    let mut stack = vec![(root, false)];
    while let Some((id, operands_done)) = stack.pop() {
        if memo.contains_key(&id) {
            continue;
        }
        let cell = cells.get(id);
        let operands: Vec<NormalFormId> = if cell.rule_ref.is_some() {
            memo.insert(id, false);
            continue;
        } else {
            match &cell.kind {
                NormalFormKind::Leaf(LeafKind::Literal(literal)) => {
                    memo.insert(id, matches!(literal.value, ValueKind::Number(_)));
                    continue;
                }
                NormalFormKind::Sum(terms) | NormalFormKind::Product(terms) => {
                    terms.iter().map(|term| term.operand).collect()
                }
                NormalFormKind::Subtract(a, b)
                | NormalFormKind::Divide(a, b)
                | NormalFormKind::Power(a, b) => vec![*a, *b],
                NormalFormKind::Negate(x) | NormalFormKind::Reciprocal(x) => vec![*x],
                _ => {
                    memo.insert(id, false);
                    continue;
                }
            }
        };
        if operands_done {
            let all = operands.iter().all(|operand| {
                *memo
                    .get(operand)
                    .expect("BUG: numeric operand not decided before its parent")
            });
            memo.insert(id, all);
        } else {
            stack.push((id, true));
            for operand in operands {
                if !memo.contains_key(&operand) {
                    stack.push((operand, false));
                }
            }
        }
    }
    *memo.get(&root).expect("BUG: numeric_only root not decided")
}

/// Merge a nested Sum into its Sum parent (same for Product).
/// Recovers the n-ary cell the source chain already had; not a semantic rewrite,
/// so the flat cell carries no fold origin.
///
/// A child that is already a live Sum/Product of the same operator is spliced
/// directly. A child that is only a folded literal whose origin is such a Sum/
/// Product (origin unset on that pre-image) is spliced the same way: post-order
/// constant-fold otherwise hides the nest from this pass.
fn flatten_associative(
    n: &mut Normalizer<'_>,
    id: NormalFormId,
) -> Result<Option<NormalFormId>, Error> {
    let Some((operator, terms)) = Associative::split(&n.cells.get(id).kind) else {
        return Ok(None);
    };
    let terms = terms.to_vec();
    let mut flat = Vec::with_capacity(terms.len());
    let mut flattened = false;
    for term in terms {
        let cell = n.cells.get(term.operand);
        match Associative::split(&cell.kind) {
            Some((inner_operator, inner))
                if inner_operator == operator && cell.rule_ref.is_none() =>
            {
                splice_nested_terms(&mut flat, inner, term.inverse);
                flattened = true;
            }
            _ => match folded_associative_operands(&n.cells, term.operand, operator) {
                Some(inner) => {
                    splice_nested_terms(&mut flat, inner, term.inverse);
                    flattened = true;
                }
                None => flat.push(term),
            },
        }
    }
    if !flattened {
        return Ok(None);
    }
    leading_term_direct(&mut n.cells, operator, &mut flat);
    Ok(Some(n.cells.intern_empty(operator.wrap(flat))))
}

/// Merge a nested `and` operand into its `and` parent. Operand order is kept.
fn flatten_conjunction(
    n: &mut Normalizer<'_>,
    id: NormalFormId,
) -> Result<Option<NormalFormId>, Error> {
    let NormalFormKind::And(children) = &n.cells.get(id).kind else {
        return Ok(None);
    };
    let children = children.clone();
    let mut flat = Vec::with_capacity(children.len());
    let mut flattened = false;
    for child in children {
        let cell = n.cells.get(child);
        match &cell.kind {
            NormalFormKind::And(inner) if cell.rule_ref.is_none() => {
                flat.extend_from_slice(inner);
                flattened = true;
            }
            _ => flat.push(child),
        }
    }
    if !flattened {
        return Ok(None);
    }
    Ok(Some(n.cells.intern_empty(NormalFormKind::And(flat))))
}

/// Operands of a same-operator Sum/Product that exists only as this cell's fold
/// origin (constant-fold or identity elim). `None` when the child is not that
/// shape: keeps subtract/divide expansions (origin still set on the pre-image)
/// and rule references opaque.
fn folded_associative_operands<'a>(
    cells: &'a Cells<'_>,
    child: NormalFormId,
    operator: Associative,
) -> Option<&'a [ArithmeticTerm]> {
    let cell = cells.get(child);
    if cell.rule_ref.is_some() {
        return None;
    }
    let origin = cell.origin?;
    let origin_cell = cells.get(origin);
    if origin_cell.rule_ref.is_some() || origin_cell.origin.is_some() {
        return None;
    }
    let (inner_operator, inner) = Associative::split(&origin_cell.kind)?;
    if inner_operator != operator {
        return None;
    }
    Some(inner)
}

/// The two associative arithmetic operators whose n-ary cells flatten and sort.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Associative {
    Sum,
    Product,
}

impl Associative {
    fn split(kind: &NormalFormKind) -> Option<(Self, &[ArithmeticTerm])> {
        match kind {
            NormalFormKind::Sum(terms) => Some((Associative::Sum, terms)),
            NormalFormKind::Product(terms) => Some((Associative::Product, terms)),
            _ => None,
        }
    }

    fn wrap(self, terms: Vec<ArithmeticTerm>) -> NormalFormKind {
        match self {
            Associative::Sum => NormalFormKind::Sum(terms),
            Associative::Product => NormalFormKind::Product(terms),
        }
    }

    /// The unary inverse of an operand: `-x` for Sum, `1/x` for Product.
    fn inverse_of(self, operand: NormalFormId) -> NormalFormKind {
        match self {
            Associative::Sum => NormalFormKind::Negate(operand),
            Associative::Product => NormalFormKind::Reciprocal(operand),
        }
    }
}

/// Makes the first term of `terms` direct. A Sum/Product whose first term is inverse
/// (its direct operands were eliminated) starts with the unary inverse of that operand.
fn leading_term_direct(cells: &mut Cells<'_>, operator: Associative, terms: &mut [ArithmeticTerm]) {
    let Some(first) = terms.first_mut() else {
        return;
    };
    if first.inverse {
        let inverse = cells.intern_empty(operator.inverse_of(first.operand));
        *first = ArithmeticTerm::direct(inverse);
    }
}

/// Appends the terms of a nested same-operator cell that is reached through a term
/// with flag `inverse`. Operand `t` of the nested cell is subtracted (divided) when
/// its own flag differs from `inverse`, so the nested cell's sign is carried over.
fn splice_nested_terms(flat: &mut Vec<ArithmeticTerm>, nested: &[ArithmeticTerm], inverse: bool) {
    flat.extend(nested.iter().map(|term| ArithmeticTerm {
        operand: term.operand,
        inverse: term.inverse != inverse,
    }));
}

fn power_laws(n: &mut Normalizer<'_>, id: NormalFormId) -> Result<Option<NormalFormId>, Error> {
    match &n.cells.get(id).kind {
        NormalFormKind::Power(base, exp) => {
            let (base, exp) = (*base, *exp);
            let NormalFormKind::Power(inner_base, inner_exp) = n.cells.get(base).kind else {
                return Ok(None);
            };
            let merge_preserves_domain = is_total(&n.cells, inner_base)
                || nested_power_merge_preserves_domain(&n.cells, inner_exp, exp);
            if !merge_preserves_domain {
                return Ok(None);
            }
            let Some(new_exp) = try_multiply_nf_rational(&mut n.cells, inner_exp, exp) else {
                return Ok(None);
            };
            Ok(Some(
                n.cells
                    .fold_into(NormalFormKind::Power(inner_base, new_exp), id),
            ))
        }
        NormalFormKind::Product(terms) => {
            let terms = terms.clone();
            Ok(collect_like_base_powers(&mut n.cells, terms, id))
        }
        _ => Ok(None),
    }
}

fn nested_power_merge_preserves_domain(
    cells: &Cells<'_>,
    inner_exponent: NormalFormId,
    outer_exponent: NormalFormId,
) -> bool {
    let Some(inner) = as_integer_literal(cells, inner_exponent) else {
        return false;
    };
    let Some(outer) = as_integer_literal(cells, outer_exponent) else {
        return false;
    };
    inner.numer_is_positive() && outer.numer_is_positive()
}

fn like_base_merge_preserves_domain(
    cells: &Cells<'_>,
    left_exponent: NormalFormId,
    right_exponent: NormalFormId,
) -> bool {
    let Some(left) = as_integer_literal(cells, left_exponent) else {
        return false;
    };
    let Some(right) = as_integer_literal(cells, right_exponent) else {
        return false;
    };
    (left.numer_is_positive() && right.numer_is_positive())
        || (left.numer_is_negative() && right.numer_is_negative())
}

/// `x^a * x^b` → `x^(a+b)` for literal exponents. `None` when no pair merges.
fn collect_like_base_powers(
    cells: &mut Cells<'_>,
    terms: Vec<ArithmeticTerm>,
    destroyed_id: NormalFormId,
) -> Option<NormalFormId> {
    let mut powers: Vec<(NormalFormId, NormalFormId, Option<Source>)> = Vec::new();
    let mut other: Vec<ArithmeticTerm> = Vec::new();
    let mut merged = false;
    for term in terms {
        let c = term.operand;
        let power_shape = match &cells.get(c).kind {
            NormalFormKind::Power(base, exp) if !term.inverse => Some((*base, *exp)),
            _ => None,
        };
        let Some((base_val, exp_val)) = power_shape else {
            other.push(term);
            continue;
        };
        if let Some((_, stored_exp, _)) = powers.iter_mut().find(|(b, _, _)| *b == base_val) {
            let merge_preserves_domain = is_total(cells, base_val)
                || like_base_merge_preserves_domain(cells, *stored_exp, exp_val);
            if merge_preserves_domain {
                if let Some(sum_exp) = try_add_nf_rational(cells, *stored_exp, exp_val) {
                    *stored_exp = sum_exp;
                    merged = true;
                    continue;
                }
            }
        }
        powers.push((base_val, exp_val, cells.get(c).source.clone()));
    }
    if !merged {
        return None;
    }
    for (base, exp, factor_source) in powers {
        let power = cells.rebuild(NormalFormKind::Power(base, exp), factor_source, None);
        other.push(ArithmeticTerm::direct(power));
    }
    Some(match other.len() {
        0 => unreachable!("BUG: merged powers leave at least one factor"),
        1 => {
            let survivor = other.first().expect("BUG: single product term").operand;
            cells.fold_into_survivor(survivor, destroyed_id)
        }
        _ => {
            leading_term_direct(cells, Associative::Product, &mut other);
            cells.fold_into(NormalFormKind::Product(other), destroyed_id)
        }
    })
}

fn try_multiply_nf_rational(
    cells: &mut Cells<'_>,
    a: NormalFormId,
    b: NormalFormId,
) -> Option<NormalFormId> {
    let ra = as_rational_literal(cells, a)?;
    let rb = as_rational_literal(cells, b)?;
    let rational = rational_operation(&ra, NumericOperation::Multiply, &rb).ok()?;
    Some(cells.intern_empty(NormalFormKind::Leaf(LeafKind::Literal(
        literal_from_folded_rational(rational),
    ))))
}

fn try_add_nf_rational(
    cells: &mut Cells<'_>,
    a: NormalFormId,
    b: NormalFormId,
) -> Option<NormalFormId> {
    let ra = as_rational_literal(cells, a)?;
    let rb = as_rational_literal(cells, b)?;
    let rational = rational_operation(&ra, NumericOperation::Add, &rb).ok()?;
    Some(cells.intern_empty(NormalFormKind::Leaf(LeafKind::Literal(
        literal_from_folded_rational(rational),
    ))))
}

fn eliminate_identities(
    n: &mut Normalizer<'_>,
    id: NormalFormId,
) -> Result<Option<NormalFormId>, Error> {
    let cells = &mut n.cells;
    let rewritten = match &cells.get(id).kind {
        NormalFormKind::Sum(terms) => {
            let terms = terms.clone();
            let mut kept: Vec<ArithmeticTerm> = terms
                .iter()
                .copied()
                .filter(|term| !is_numeric_zero(cells, term.operand))
                .collect();
            if kept.len() == terms.len() {
                return Ok(None);
            }
            leading_term_direct(cells, Associative::Sum, &mut kept);
            match kept.as_slice() {
                [] => cells.fold_into(
                    NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::number(rational_zero()))),
                    id,
                ),
                [only] => cells.fold_into_survivor(only.operand, id),
                _ => cells.fold_into(NormalFormKind::Sum(kept), id),
            }
        }
        NormalFormKind::Product(terms) => {
            let terms = terms.clone();
            // A zero factor decides the product unless a divisor is zero too: division
            // by zero stays for evaluation.
            let zero_divisor = terms
                .iter()
                .any(|term| term.inverse && is_numeric_zero(cells, term.operand));
            if !zero_divisor
                && terms
                    .iter()
                    .any(|term| is_numeric_zero(cells, term.operand))
            {
                let operands: Vec<NormalFormId> = terms.iter().map(|term| term.operand).collect();
                if let Some(zero) = typed_product_zero(cells, &operands) {
                    let zero_kind = cells.get(zero).kind.clone();
                    return Ok(Some(cells.fold_into(zero_kind, id)));
                }
            }
            let mut kept: Vec<ArithmeticTerm> = terms
                .iter()
                .copied()
                .filter(|term| !is_numeric_one(cells, term.operand))
                .collect();
            if kept.len() == terms.len() {
                return Ok(None);
            }
            leading_term_direct(cells, Associative::Product, &mut kept);
            match kept.as_slice() {
                [] => cells.fold_into(
                    NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::number(rational_one()))),
                    id,
                ),
                [only] => cells.fold_into_survivor(only.operand, id),
                _ => cells.fold_into(NormalFormKind::Product(kept), id),
            }
        }
        NormalFormKind::Power(base, exp) => {
            let (base, exp) = (*base, *exp);
            if is_numeric_one(cells, exp) {
                cells.fold_into_survivor(base, id)
            } else if is_numeric_zero(cells, exp)
                && as_rational_literal(cells, base)
                    .is_some_and(|rational| !rational_is_zero(&rational))
            {
                cells.fold_into(
                    NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::number(rational_one()))),
                    id,
                )
            } else {
                return Ok(None);
            }
        }
        NormalFormKind::Negate(x) if is_numeric_zero(cells, *x) => cells.fold_into(
            NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::number(rational_zero()))),
            id,
        ),
        NormalFormKind::Subtract(a, b) => {
            let (a, b) = (*a, *b);
            if is_numeric_zero(cells, a) {
                cells.fold_into(NormalFormKind::Negate(b), id)
            } else if is_numeric_zero(cells, b) {
                cells.fold_into_survivor(a, id)
            } else {
                return Ok(None);
            }
        }
        NormalFormKind::Divide(a, b) => {
            let (a, b) = (*a, *b);
            if is_numeric_one(cells, b) {
                cells.fold_into_survivor(a, id)
            } else if is_numeric_one(cells, a) {
                cells.fold_into(NormalFormKind::Reciprocal(b), id)
            } else {
                return Ok(None);
            }
        }
        NormalFormKind::And(children) => {
            let children = children.clone();
            if children
                .iter()
                .any(|child| is_literal_bool(cells, *child, false))
            {
                return Ok(Some(cells.fold_into(
                    NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::from_bool(false))),
                    id,
                )));
            }
            let kept: Vec<NormalFormId> = children
                .iter()
                .copied()
                .filter(|child| !is_literal_bool(cells, *child, true))
                .collect();
            if kept.len() == children.len() {
                return Ok(None);
            }
            match kept.len() {
                0 => cells.fold_into(
                    NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::from_bool(true))),
                    id,
                ),
                1 => cells.fold_into_survivor(kept[0], id),
                _ => cells.fold_into(NormalFormKind::And(kept), id),
            }
        }
        NormalFormKind::Not(inner) => {
            let inner = *inner;
            if is_literal_bool(cells, inner, true) {
                cells.fold_into(
                    NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::from_bool(false))),
                    id,
                )
            } else if is_literal_bool(cells, inner, false) {
                cells.fold_into(
                    NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::from_bool(true))),
                    id,
                )
            } else {
                return Ok(None);
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(rewritten))
}

/// Zero literal for a product of literals with at most one measure factor,
/// typed as that measure so the fold keeps the unit. `None`: a non-literal or
/// a second measure factor; the product is left for evaluation.
fn typed_product_zero(cells: &mut Cells<'_>, children: &[NormalFormId]) -> Option<NormalFormId> {
    let mut measure_child: Option<NormalFormId> = None;
    for &child in children {
        if cells.get(child).rule_ref.is_some() {
            return None;
        }
        let NormalFormKind::Leaf(LeafKind::Literal(literal)) = &cells.get(child).kind else {
            return None;
        };
        match &literal.value {
            ValueKind::Number(_) => {}
            ValueKind::Measure(_) => {
                if measure_child.is_some() {
                    return None;
                }
                measure_child = Some(child);
            }
            _ => return None,
        }
    }
    match measure_child {
        None => Some(cells.intern_empty(NormalFormKind::Leaf(LeafKind::Literal(
            LiteralValue::number(rational_zero()),
        )))),
        Some(child) => {
            let measure_type = Arc::clone(cells.result_type(child));
            Some(cells.intern_literal_leaf(LiteralValue::measure(rational_zero()), measure_type))
        }
    }
}

fn is_literal_bool(cells: &Cells<'_>, id: NormalFormId, expected: bool) -> bool {
    if cells.get(id).rule_ref.is_some() {
        return false;
    }
    match &cells.get(id).kind {
        NormalFormKind::Leaf(LeafKind::Literal(literal)) => {
            matches!(literal.value, ValueKind::Boolean(value) if value == expected)
        }
        _ => false,
    }
}

fn double_negate_reciprocal(
    n: &mut Normalizer<'_>,
    id: NormalFormId,
) -> Result<Option<NormalFormId>, Error> {
    let cells = &mut n.cells;
    let survivor = match &cells.get(id).kind {
        NormalFormKind::Negate(x) => {
            if cells.get(*x).rule_ref.is_some() {
                return Ok(None);
            }
            match cells.get(*x).kind {
                NormalFormKind::Negate(y) => y,
                _ => return Ok(None),
            }
        }
        NormalFormKind::Reciprocal(x) => {
            if cells.get(*x).rule_ref.is_some() {
                return Ok(None);
            }
            match cells.get(*x).kind {
                NormalFormKind::Reciprocal(y) if is_total(cells, y) => y,
                _ => return Ok(None),
            }
        }
        NormalFormKind::Not(x) => {
            if cells.get(*x).rule_ref.is_some() {
                return Ok(None);
            }
            match cells.get(*x).kind {
                NormalFormKind::Not(y) => y,
                _ => return Ok(None),
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(cells.fold_into_survivor(survivor, id)))
}

fn math_identities(
    n: &mut Normalizer<'_>,
    id: NormalFormId,
) -> Result<Option<NormalFormId>, Error> {
    let cells = &mut n.cells;
    let NormalFormKind::MathOp(op, x) = &cells.get(id).kind else {
        return Ok(None);
    };
    let x = *x;
    let rewritten = match op {
        MathematicalComputation::Abs => match &cells.get(x).kind {
            NormalFormKind::MathOp(MathematicalComputation::Abs, inner) => {
                let inner = *inner;
                cells.fold_into(
                    NormalFormKind::MathOp(MathematicalComputation::Abs, inner),
                    id,
                )
            }
            _ => return Ok(None),
        },
        MathematicalComputation::Exp => match &cells.get(x).kind {
            NormalFormKind::MathOp(MathematicalComputation::Log, inner)
                if is_total(cells, *inner) =>
            {
                cells.fold_into_survivor(*inner, id)
            }
            _ => return Ok(None),
        },
        MathematicalComputation::Log => match &cells.get(x).kind {
            NormalFormKind::MathOp(MathematicalComputation::Exp, inner)
                if is_total(cells, *inner) =>
            {
                cells.fold_into_survivor(*inner, id)
            }
            _ => return Ok(None),
        },
        MathematicalComputation::Sqrt => {
            let half = cells.intern_empty(NormalFormKind::Leaf(LeafKind::Literal(
                literal_from_folded_rational(rational_new(1, 2)),
            )));
            cells.fold_into(NormalFormKind::Power(x, half), id)
        }
        _ => return Ok(None),
    };
    Ok(Some(rewritten))
}

/// Constants that cannot be computed are planning errors, like `1 / 0`.
fn fold_constant_operation(
    left: &RationalInteger,
    operation: NumericOperation,
    right: &RationalInteger,
    source: &Option<Source>,
    context: &str,
) -> Result<RationalInteger, Error> {
    rational_operation(left, operation, right)
        .map_err(|failure| normalization_error(source.clone(), failure, context))
}

fn constant_fold(n: &mut Normalizer<'_>, id: NormalFormId) -> Result<Option<NormalFormId>, Error> {
    let source = n.source.clone();
    let cells = &mut n.cells;
    let rewritten = match &cells.get(id).kind {
        NormalFormKind::Sum(terms) => {
            if !terms
                .iter()
                .all(|term| as_rational_literal(cells, term.operand).is_some())
            {
                return Ok(None);
            }
            let mut sorted = terms.clone();
            let mut compare_failure: Option<crate::computation::rational::NumericFailure> = None;
            sorted.sort_by(|a, b| match canonical_term_cmp(cells, *a, *b) {
                Ok(ordering) => ordering,
                Err(failure) => {
                    compare_failure.get_or_insert(failure);
                    std::cmp::Ordering::Equal
                }
            });
            if let Some(failure) = compare_failure {
                return Err(normalization_error(
                    source.clone(),
                    failure,
                    "constant fold sum",
                ));
            }
            let mut acc = rational_new(0, 1);
            for term in &sorted {
                let rational = as_rational_literal(cells, term.operand).expect("BUG: all numeric");
                let operation = if term.inverse {
                    NumericOperation::Subtract
                } else {
                    NumericOperation::Add
                };
                let next = fold_constant_operation(
                    &acc,
                    operation,
                    &rational,
                    &source,
                    "constant fold sum",
                )?;
                acc = next;
            }
            // The sorted sum is the recorded pre-image so explain reads terms
            // in canonical order.
            let cell = cells.get(id);
            let (cell_source, origin) = (cell.source.clone(), cell.origin);
            let mut pre_image = sorted;
            leading_term_direct(cells, Associative::Sum, &mut pre_image);
            let destroyed_id = cells.rebuild(NormalFormKind::Sum(pre_image), cell_source, origin);
            fold_number(cells, acc, destroyed_id)
        }
        NormalFormKind::Product(terms) => {
            if !terms
                .iter()
                .all(|term| as_rational_literal(cells, term.operand).is_some())
            {
                return Ok(None);
            }
            let mut acc = rational_new(1, 1);
            for term in terms {
                let rational = as_rational_literal(cells, term.operand).expect("BUG: all numeric");
                let operation = if term.inverse {
                    NumericOperation::Divide
                } else {
                    NumericOperation::Multiply
                };
                let next = fold_constant_operation(
                    &acc,
                    operation,
                    &rational,
                    &source,
                    "constant fold product",
                )?;
                acc = next;
            }
            fold_number(cells, acc, id)
        }
        NormalFormKind::Power(base, exp) => {
            let (Some(base_rational), Some(exp_rational)) = (
                as_rational_literal(cells, *base),
                as_rational_literal(cells, *exp),
            ) else {
                return Ok(None);
            };
            // Irrational powers stay for evaluation, which approximates them.
            let rational =
                match rational_operation(&base_rational, NumericOperation::Power, &exp_rational) {
                    Ok(rational) => rational,
                    Err(NumericFailure::Irrational) => return Ok(None),
                    Err(failure) => {
                        return Err(normalization_error(
                            source.clone(),
                            failure,
                            "constant fold power",
                        ))
                    }
                };
            fold_number(cells, rational, id)
        }
        NormalFormKind::Negate(x) => {
            let Some(rational) = as_rational_literal(cells, *x) else {
                return Ok(None);
            };
            let negated = fold_constant_operation(
                &rational_new(0, 1),
                NumericOperation::Subtract,
                &rational,
                &source,
                "constant fold negate",
            )?;
            fold_number(cells, negated, id)
        }
        NormalFormKind::Reciprocal(x) => {
            let Some(rational) = as_rational_literal(cells, *x) else {
                return Ok(None);
            };
            let reciprocal = fold_constant_operation(
                &rational_one(),
                NumericOperation::Divide,
                &rational,
                &source,
                "constant fold reciprocal",
            )?;
            fold_number(cells, reciprocal, id)
        }
        NormalFormKind::Divide(left, right) => {
            let (Some(left_rational), Some(right_rational)) = (
                as_rational_literal(cells, *left),
                as_rational_literal(cells, *right),
            ) else {
                return Ok(None);
            };
            let quotient = fold_constant_operation(
                &left_rational,
                NumericOperation::Divide,
                &right_rational,
                &source,
                "constant fold divide",
            )?;
            fold_number(cells, quotient, id)
        }
        NormalFormKind::Subtract(left, right) => {
            let (Some(left_rational), Some(right_rational)) = (
                as_rational_literal(cells, *left),
                as_rational_literal(cells, *right),
            ) else {
                return Ok(None);
            };
            let difference = fold_constant_operation(
                &left_rational,
                NumericOperation::Subtract,
                &right_rational,
                &source,
                "constant fold subtract",
            )?;
            fold_number(cells, difference, id)
        }
        NormalFormKind::Modulo(left, right) => {
            let (Some(left_rational), Some(right_rational)) = (
                as_rational_literal(cells, *left),
                as_rational_literal(cells, *right),
            ) else {
                return Ok(None);
            };
            let remainder = fold_constant_operation(
                &left_rational,
                NumericOperation::Modulo,
                &right_rational,
                &source,
                "constant fold modulo",
            )?;
            fold_number(cells, remainder, id)
        }
        NormalFormKind::Not(x) => {
            let x = *x;
            if cells.get(x).rule_ref.is_some() {
                return Ok(None);
            }
            let NormalFormKind::Leaf(LeafKind::Literal(literal)) = &cells.get(x).kind else {
                return Ok(None);
            };
            let ValueKind::Boolean(boolean) = &literal.value else {
                return Ok(None);
            };
            let negated = !boolean;
            cells.fold_into(
                NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::from_bool(negated))),
                id,
            )
        }
        NormalFormKind::Comparison(left, op, right) => {
            let (left, op, right) = (*left, op.clone(), *right);
            if cells.get(left).rule_ref.is_some() || cells.get(right).rule_ref.is_some() {
                return Ok(None);
            }
            let (
                NormalFormKind::Leaf(LeafKind::Literal(left_literal)),
                NormalFormKind::Leaf(LeafKind::Literal(right_literal)),
            ) = (&cells.get(left).kind, &cells.get(right).kind)
            else {
                return Ok(None);
            };
            let folded = comparison_operation(
                &left_literal.value,
                cells.result_type(left),
                &op,
                &right_literal.value,
                cells.result_type(right),
            );
            // A comparison the planner cannot decide (incomparable literal
            // kinds veto at evaluation) stays for evaluation.
            let Some(literal) = folded.value() else {
                return Ok(None);
            };
            let ValueKind::Boolean(held) = literal.value else {
                return Ok(None);
            };
            fold_comparison_to_bool(cells, left, op, right, held)
        }
        NormalFormKind::MathOp(op, x) => {
            let Some(rational) = as_rational_literal(cells, *x) else {
                return Ok(None);
            };
            let Some(folded) = fold_math_op(op, &rational) else {
                return Ok(None);
            };
            fold_number(cells, folded, id)
        }
        _ => return Ok(None),
    };
    Ok(Some(rewritten))
}

fn fold_number(
    cells: &mut Cells<'_>,
    rational: RationalInteger,
    destroyed_id: NormalFormId,
) -> NormalFormId {
    cells.fold_into(
        NormalFormKind::Leaf(LeafKind::Literal(literal_from_folded_rational(rational))),
        destroyed_id,
    )
}

/// Fold a decidable Comparison to a bool leaf, retaining the comparison as origin.
fn fold_comparison_to_bool(
    cells: &mut Cells<'_>,
    left: NormalFormId,
    op: ComparisonComputation,
    right: NormalFormId,
    held: bool,
) -> NormalFormId {
    let destroyed = cells.rebuild(NormalFormKind::Comparison(left, op, right), None, None);
    cells.fold_into(
        NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::from_bool(held))),
        destroyed,
    )
}

fn fold_math_op(
    op: &MathematicalComputation,
    operand: &RationalInteger,
) -> Option<RationalInteger> {
    let zero = rational_new(0, 1);
    let one = rational_new(1, 1);
    match op {
        MathematicalComputation::Sin if *operand == zero => Some(zero),
        MathematicalComputation::Cos if *operand == zero => Some(one),
        MathematicalComputation::Tan if *operand == zero => Some(zero),
        MathematicalComputation::Log if *operand == one => Some(zero),
        MathematicalComputation::Exp if *operand == zero => Some(one),
        _ => None,
    }
}

/// `n as number` over a plain number literal is that literal.
fn fold_number_conversion(
    n: &mut Normalizer<'_>,
    id: NormalFormId,
) -> Result<Option<NormalFormId>, Error> {
    let cells = &mut n.cells;
    let NormalFormKind::UnitConversion(
        inner,
        SemanticConversionTarget::Type(PrimitiveKind::Number),
    ) = &cells.get(id).kind
    else {
        return Ok(None);
    };
    let inner = *inner;
    if cells.get(inner).rule_ref.is_some() {
        return Ok(None);
    }
    let NormalFormKind::Leaf(LeafKind::Literal(literal)) = &cells.get(inner).kind else {
        return Ok(None);
    };
    let ValueKind::Number(number) = &literal.value else {
        return Ok(None);
    };
    let number = number.clone();
    Ok(Some(cells.fold_into(
        NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::number(number))),
        id,
    )))
}

/// `not (a < b)` → `a >= b`.
fn negated_comparisons(
    n: &mut Normalizer<'_>,
    id: NormalFormId,
) -> Result<Option<NormalFormId>, Error> {
    let cells = &mut n.cells;
    let NormalFormKind::Not(x) = &cells.get(id).kind else {
        return Ok(None);
    };
    let NormalFormKind::Comparison(a, op, b) = &cells.get(*x).kind else {
        return Ok(None);
    };
    let (a, op, b) = (*a, negated_comparison(op.clone()), *b);
    Ok(Some(
        cells.fold_into(NormalFormKind::Comparison(a, op, b), id),
    ))
}

fn logical_idempotency(
    n: &mut Normalizer<'_>,
    id: NormalFormId,
) -> Result<Option<NormalFormId>, Error> {
    let cells = &mut n.cells;
    let NormalFormKind::And(children) = &cells.get(id).kind else {
        return Ok(None);
    };
    // Id equality only: Kind equality would collapse distinct rule references
    // that share body Kind after cons. A repeated operand is already known true
    // when reached again, so only its first occurrence is kept.
    let mut seen = HashSet::with_capacity(children.len());
    let unique: Vec<NormalFormId> = children
        .iter()
        .copied()
        .filter(|child| seen.insert(*child))
        .collect();
    if unique.len() == children.len() {
        return Ok(None);
    }
    match unique.as_slice() {
        [only] => Ok(Some(cells.fold_into_survivor(*only, id))),
        _ => Ok(Some(cells.fold_into(NormalFormKind::And(unique), id))),
    }
}

/// Last-match-wins collapse of Piecewise arms with statically decided conditions.
///
/// - Drop arms whose condition is statically false (including `flag and false`
///   after a conjunct folds: only for unless conditions, not rule-body And).
/// - Keep only arms from the last statically-true condition onward.
/// - Unwrap a single remaining arm to its result.
fn collapse_piecewise(
    n: &mut Normalizer<'_>,
    id: NormalFormId,
) -> Result<Option<NormalFormId>, Error> {
    let cells = &mut n.cells;
    let NormalFormKind::Piecewise(arms) = &cells.get(id).kind else {
        return Ok(None);
    };
    let arms: Vec<(NormalFormId, NormalFormId)> = arms.clone();

    let mut recorded: Vec<(NormalFormId, NormalFormId)> = Vec::with_capacity(arms.len());
    for (condition, result) in arms.iter().copied() {
        let forced = force_unless_condition_static_false(condition, cells);
        recorded.push((forced, result));
    }
    assert!(!recorded.is_empty(), "BUG: Piecewise collapse has no arms");

    let mut kept: Vec<(NormalFormId, NormalFormId)> = recorded
        .iter()
        .filter(|(condition, _)| !is_literal_bool(cells, *condition, false))
        .copied()
        .collect();
    assert!(
        !kept.is_empty(),
        "BUG: Piecewise collapse removed every arm"
    );
    let mut last_true_index = None;
    for (index, (condition, _)) in kept.iter().enumerate() {
        if is_literal_bool(cells, *condition, true) {
            last_true_index = Some(index);
        }
    }
    if let Some(index) = last_true_index {
        kept = kept.split_off(index);
    }

    let dropped = kept.len() != recorded.len();
    if !dropped && kept.len() > 1 {
        if kept.as_slice() == arms.as_slice() {
            return Ok(None);
        }
        return Ok(Some(cells.rebuild(
            NormalFormKind::Piecewise(kept),
            None,
            None,
        )));
    }

    let destroyed_id = cells.rebuild(NormalFormKind::Piecewise(recorded), None, None);

    if kept.len() == 1 {
        let survivor = kept[0].1;
        assert!(
            cells
                .result_type(destroyed_id)
                .same_value_type(cells.result_type(survivor)),
            "BUG: Piecewise collapse survivor value type differs from destroyed Piecewise"
        );
        return Ok(Some(cells.replace_with_survivor(survivor, destroyed_id)));
    }
    let survivor = cells.rebuild(NormalFormKind::Piecewise(kept), None, None);
    assert!(
        cells
            .result_type(destroyed_id)
            .same_value_type(cells.result_type(survivor)),
        "BUG: Piecewise collapse survivor value type differs from destroyed Piecewise"
    );
    Ok(Some(cells.replace_with_survivor(survivor, destroyed_id)))
}

fn force_unless_condition_static_false(id: NormalFormId, cells: &mut Cells<'_>) -> NormalFormId {
    if unless_condition_contains_literal_false(cells, id) {
        return cells.fold_into(
            NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::from_bool(false))),
            id,
        );
    }
    id
}

fn unless_condition_contains_literal_false(cells: &Cells<'_>, id: NormalFormId) -> bool {
    let mut pending = vec![id];
    while let Some(current) = pending.pop() {
        if cells.get(current).rule_ref.is_some() {
            continue;
        }
        if is_literal_bool(cells, current, false) {
            return true;
        }
        if let NormalFormKind::And(children) = &cells.get(current).kind {
            pending.extend(children.iter().copied());
        }
    }
    false
}

/// Sort the terms of an all-numeric Sum / Product: literals first, ascending.
fn canonical_order(
    n: &mut Normalizer<'_>,
    id: NormalFormId,
) -> Result<Option<NormalFormId>, Error> {
    let Some((operator, terms)) = Associative::split(&n.cells.get(id).kind) else {
        return Ok(None);
    };
    let terms = terms.to_vec();
    if !terms
        .iter()
        .all(|term| numeric_only(&n.cells, &mut n.numeric, term.operand))
    {
        return Ok(None);
    }
    let cells = &mut n.cells;
    let mut sorted = terms.clone();
    let mut compare_failure: Option<crate::computation::rational::NumericFailure> = None;
    sorted.sort_by(|a, b| match canonical_term_cmp(cells, *a, *b) {
        Ok(ordering) => ordering,
        Err(failure) => {
            compare_failure.get_or_insert(failure);
            std::cmp::Ordering::Equal
        }
    });
    if let Some(failure) = compare_failure {
        return Err(normalization_error(
            n.source.clone(),
            failure,
            "canonical order",
        ));
    }
    if sorted == terms {
        return Ok(None);
    }
    leading_term_direct(cells, operator, &mut sorted);
    Ok(Some(cells.fold_into(operator.wrap(sorted), id)))
}

/// Direct terms before inverse terms, then by canonical numeric order of the operand.
/// A canonically sorted Sum/Product therefore starts with a direct term whenever one exists.
fn canonical_term_cmp(
    cells: &Cells<'_>,
    a: ArithmeticTerm,
    b: ArithmeticTerm,
) -> Result<std::cmp::Ordering, crate::computation::rational::NumericFailure> {
    match a.inverse.cmp(&b.inverse) {
        std::cmp::Ordering::Equal => canonical_numeric_cmp(cells, a.operand, b.operand),
        ordering => Ok(ordering),
    }
}

fn canonical_numeric_cmp(
    cells: &Cells<'_>,
    a: NormalFormId,
    b: NormalFormId,
) -> Result<std::cmp::Ordering, crate::computation::rational::NumericFailure> {
    match (sort_key(cells, a), sort_key(cells, b)) {
        (ak, bk) if ak != bk => Ok(ak.cmp(&bk)),
        _ => match (as_rational_literal(cells, a), as_rational_literal(cells, b)) {
            (Some(ra), Some(rb)) => ra.try_cmp(&rb),
            _ => Ok(std::cmp::Ordering::Equal),
        },
    }
}

fn sort_key(cells: &Cells<'_>, id: NormalFormId) -> u8 {
    match &cells.get(id).kind {
        NormalFormKind::Leaf(LeafKind::Literal(_)) => 0,
        _ => 1,
    }
}

/// Fold a Piecewise whose arms all compare one scrutinee against constants into an
/// [`NormalFormKind::OrderedDispatch`], turning the linear arm scan into a binary
/// search over a precomputed region table.
///
/// Also folds conjunctive-key LUTs (`unless key is K and residual then body`):
/// partitions arms by the shared `Is` key into per-key sub-Piecewise cells, then
/// builds an outer point-dispatch. Fresh sub-Piecewise cells are normalized by
/// the rewrite driver and fold again when residuals are themselves
/// single-scrutinee comparisons (or further conjunctive LUTs).
///
/// Runs last, after `collapse_piecewise` has dropped statically dead arms and left
/// the default at index 0. The incoming cell becomes the fold origin, so
/// explanation keeps reading the Piecewise.
///
/// Declines whenever the shape does not hold. Declining is the pattern not applying,
/// not a fallback: the Piecewise is returned untouched and evaluates as before.
///
/// After the shape matches, key construction and boundary sort failures are planning
/// errors, not a silent return to Piecewise.
fn ordered_dispatch(
    n: &mut Normalizer<'_>,
    id: NormalFormId,
) -> Result<Option<NormalFormId>, Error> {
    if n.cells.result_type(id).is_spec() {
        return Ok(None);
    }
    let NormalFormKind::Piecewise(arms) = &n.cells.get(id).kind else {
        return Ok(None);
    };
    if arms.len() < 2 || !is_literal_bool(&n.cells, arms[0].0, true) {
        return Ok(None);
    }
    let arms: Vec<(NormalFormId, NormalFormId)> = arms.clone();
    if let Some(folded) = try_single_scrutinee_dispatch(n, id, &arms)? {
        return Ok(Some(folded));
    }
    try_conjunctive_key_dispatch(n, id, &arms)
}

/// Single-scrutinee `unless scrutinee op literal` chain → one OrderedDispatch.
fn try_single_scrutinee_dispatch(
    n: &mut Normalizer<'_>,
    id: NormalFormId,
    arms: &[(NormalFormId, NormalFormId)],
) -> Result<Option<NormalFormId>, Error> {
    let source = n.source.clone();
    let cells = &mut n.cells;
    let Some(scan) = scan_dispatch_arms(cells, arms) else {
        return Ok(None);
    };
    let scrutinee_type = match dispatch_scrutinee_type(cells, scan.scrutinee) {
        Some(ty) => ty.clone(),
        None => return Ok(None),
    };
    let bodies: Vec<DispatchRegion> = arms
        .iter()
        .skip(1)
        .map(|(_, body)| DispatchRegion::Body(*body))
        .collect();
    build_ordered_dispatch(
        cells,
        DispatchBuild {
            destroyed_id: id,
            source,
            scrutinee: scan.scrutinee,
            scrutinee_type: &scrutinee_type,
            operators: &scan.operators,
            key_literals: &scan.keys,
            default_body: arms[0].1,
            bodies: &bodies,
        },
    )
}

/// `unless key is K and residual then body` chain → outer point-dispatch whose
/// point regions are per-key sub-Piecewise cells (normalized into inner tables).
fn try_conjunctive_key_dispatch(
    n: &mut Normalizer<'_>,
    id: NormalFormId,
    arms: &[(NormalFormId, NormalFormId)],
) -> Result<Option<NormalFormId>, Error> {
    let source = n.source.clone();
    let Some(partition) = scan_conjunctive_key_arms(&mut n.cells, arms) else {
        return Ok(None);
    };
    let scrutinee_type = match dispatch_scrutinee_type(&n.cells, partition.scrutinee) {
        Some(ty) => ty.clone(),
        None => return Ok(None),
    };

    let default_body = arms[0].1;
    let true_condition = n.cells.rebuild(
        NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::from_bool(true))),
        None,
        None,
    );

    let mut group_bodies: Vec<DispatchRegion> = Vec::with_capacity(partition.groups.len());
    let mut keys = Vec::with_capacity(partition.groups.len());
    let mut operators = Vec::with_capacity(partition.groups.len());
    for (key, group_arms) in &partition.groups {
        let mut piecewise_arms = Vec::with_capacity(group_arms.len() + 1);
        piecewise_arms.push((true_condition, default_body));
        piecewise_arms.extend(group_arms.iter().copied());
        let group_cell = n
            .cells
            .rebuild(NormalFormKind::Piecewise(piecewise_arms), None, None);
        group_bodies.push(DispatchRegion::Conjunctive(group_cell));
        keys.push(key.clone());
        operators.push(ComparisonComputation::Is);
    }

    build_ordered_dispatch(
        &mut n.cells,
        DispatchBuild {
            destroyed_id: id,
            source,
            scrutinee: partition.scrutinee,
            scrutinee_type: &scrutinee_type,
            operators: &operators,
            key_literals: &keys,
            default_body,
            bodies: &group_bodies,
        },
    )
}

/// Inputs for [`build_ordered_dispatch`]: one arm body per operator/key, in arm order.
struct DispatchBuild<'a> {
    destroyed_id: NormalFormId,
    source: Option<Source>,
    scrutinee: NormalFormId,
    scrutinee_type: &'a LemmaType,
    operators: &'a [ComparisonComputation],
    key_literals: &'a [LiteralValue],
    default_body: NormalFormId,
    bodies: &'a [DispatchRegion],
}

/// Shared table build for single-scrutinee and conjunctive outer folds.
fn build_ordered_dispatch(
    cells: &mut Cells<'_>,
    build: DispatchBuild<'_>,
) -> Result<Option<NormalFormId>, Error> {
    let DispatchBuild {
        destroyed_id,
        source,
        scrutinee,
        scrutinee_type,
        operators,
        key_literals,
        default_body,
        bodies,
    } = build;
    assert_eq!(
        operators.len(),
        key_literals.len(),
        "BUG: dispatch operators and keys must align"
    );
    assert_eq!(
        operators.len(),
        bodies.len(),
        "BUG: dispatch operators and bodies must align"
    );
    let key_refs: Vec<&LiteralValue> = key_literals.iter().collect();
    let Some(class) = classify_dispatch(scrutinee_type, &key_refs) else {
        return Ok(None);
    };
    if class == DispatchClass::Text
        && operators.iter().any(|operator| {
            !matches!(
                operator,
                ComparisonComputation::Is | ComparisonComputation::IsNot
            )
        })
    {
        return Ok(None);
    }

    let mut keys = Vec::with_capacity(key_literals.len());
    for literal in key_literals {
        match dispatch_key_of_literal(&literal.value) {
            Ok(key) => keys.push(key),
            Err(DispatchKeyBuildError::CalendarFailure(message)) => {
                return Err(Error::validation(
                    format!("ordered dispatch: {message}"),
                    source,
                    None::<String>,
                ));
            }
            Err(DispatchKeyBuildError::Unsupported) => {
                panic!(
                    "BUG: classify_dispatch accepted a key that dispatch_key_of_literal rejects: {:?}",
                    literal.value
                );
            }
        }
    }
    let boundaries = sorted_unique_boundaries(keys.clone())
        .map_err(|failure| normalization_error(source.clone(), failure, "ordered dispatch"))?;

    let mut paint_arms = Vec::with_capacity(operators.len());
    for (index, operator) in operators.iter().enumerate() {
        let region = region_for_value(&boundaries, &keys[index].as_probe())
            .map_err(|failure| normalization_error(source.clone(), failure, "ordered dispatch"))?;
        assert!(
            region % 2 == 1,
            "BUG: fold key absent from the boundary list built from it"
        );
        let boundary_index = (region - 1) / 2;
        paint_arms.push((operator.clone(), boundary_index, bodies[index]));
    }
    let regions = paint_dispatch_regions(
        boundaries.len(),
        DispatchRegion::Body(default_body),
        &paint_arms,
    );

    Ok(Some(cells.fold_into(
        NormalFormKind::OrderedDispatch {
            scrutinee,
            boundaries: Arc::from(boundaries),
            regions,
        },
        destroyed_id,
    )))
}

/// The scrutinee, operators and key literals of a foldable unless chain.
struct DispatchArmScan {
    scrutinee: NormalFormId,
    /// One per unless arm in arm order, mirrored so the scrutinee reads on the left.
    operators: Vec<ComparisonComputation>,
    keys: Vec<LiteralValue>,
}

fn scan_dispatch_arms(
    cells: &Cells<'_>,
    arms: &[(NormalFormId, NormalFormId)],
) -> Option<DispatchArmScan> {
    let mut scrutinee: Option<NormalFormId> = None;
    let mut operators = Vec::with_capacity(arms.len() - 1);
    let mut keys = Vec::with_capacity(arms.len() - 1);
    for (condition, _) in arms.iter().skip(1) {
        let cell = cells.get(*condition);
        // A rule-ref cell borrows the rule body's kind; folding would erase that identity.
        if cell.rule_ref.is_some() {
            return None;
        }
        let NormalFormKind::Comparison(left, operator, right) = &cell.kind else {
            return None;
        };
        let (candidate, key, operator) = match (
            literal_operand(cells, *left),
            literal_operand(cells, *right),
        ) {
            (None, Some(key)) => (*left, key, operator.clone()),
            // `canonical_order` never reorders a Comparison, so `"AD" is code`
            // arrives literal-first and the operator has to be mirrored.
            (Some(key), None) => (*right, key, mirrored_comparison(operator.clone())),
            (Some(_), Some(_)) | (None, None) => return None,
        };
        match scrutinee {
            None => scrutinee = Some(candidate),
            Some(established) if established == candidate => {}
            Some(_) => return None,
        }
        operators.push(operator);
        keys.push(key);
    }
    Some(DispatchArmScan {
        scrutinee: scrutinee?,
        operators,
        keys,
    })
}

/// Per-key residual arms of a conjunctive LUT, in first-seen key order.
struct ConjunctiveKeyPartition {
    scrutinee: NormalFormId,
    /// `(key_literal, residual_arms)` with residual arms in source arm order.
    groups: Vec<(LiteralValue, Vec<(NormalFormId, NormalFormId)>)>,
}

/// One `Is`-literal conjunct peeled from an arm condition, with its residual.
struct EqualityConjunct {
    scrutinee: NormalFormId,
    key: LiteralValue,
    residual: NormalFormId,
}

fn scan_conjunctive_key_arms(
    cells: &mut Cells<'_>,
    arms: &[(NormalFormId, NormalFormId)],
) -> Option<ConjunctiveKeyPartition> {
    let mut arm_peels: Vec<(Vec<EqualityConjunct>, NormalFormId)> =
        Vec::with_capacity(arms.len() - 1);
    for (condition, body) in arms.iter().skip(1) {
        let peels = equality_conjuncts(cells, *condition);
        if peels.is_empty() {
            return None;
        }
        arm_peels.push((peels, *body));
    }

    let mut candidates: Vec<NormalFormId> =
        arm_peels[0].0.iter().map(|peel| peel.scrutinee).collect();
    candidates.sort_by_key(|id| id.index());
    candidates.dedup();
    for (peels, _) in arm_peels.iter().skip(1) {
        candidates.retain(|candidate| {
            peels
                .iter()
                .filter(|peel| peel.scrutinee == *candidate)
                .count()
                == 1
        });
    }
    candidates.retain(|candidate| {
        arm_peels[0]
            .0
            .iter()
            .filter(|peel| peel.scrutinee == *candidate)
            .count()
            == 1
    });
    let scrutinee = *candidates.first()?;

    // IndexMap keeps first-seen key order; arm order inside each group is push order.
    let mut groups: IndexMap<ValueKind, (LiteralValue, Vec<(NormalFormId, NormalFormId)>)> =
        IndexMap::new();
    for (peels, body) in &arm_peels {
        let peel = peels
            .iter()
            .find(|peel| peel.scrutinee == scrutinee)
            .expect("BUG: candidate scrutinee missing from arm after retain");
        groups
            .entry(peel.key.value.clone())
            .or_insert_with(|| (peel.key.clone(), Vec::new()))
            .1
            .push((peel.residual, *body));
    }

    Some(ConjunctiveKeyPartition {
        scrutinee,
        groups: groups.into_values().collect(),
    })
}

/// Every `Is` / literal comparison that is one conjunct of an `And`, paired with
/// the conjunction of the other conjuncts as residual. Empty when the condition is
/// not an `And` (including rule-ref cells).
fn equality_conjuncts(cells: &mut Cells<'_>, condition: NormalFormId) -> Vec<EqualityConjunct> {
    let cell = cells.get(condition);
    if cell.rule_ref.is_some() {
        return Vec::new();
    }
    let NormalFormKind::And(children) = &cell.kind else {
        return Vec::new();
    };
    let children = children.clone();
    let mut out = Vec::new();
    for (index, child) in children.iter().enumerate() {
        let Some((scrutinee, key)) = is_equality_against_literal(cells, *child) else {
            continue;
        };
        let rest: Vec<NormalFormId> = children
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, id)| *id)
            .collect();
        let residual = match rest.as_slice() {
            [] => unreachable!("BUG: And has at least two operands"),
            [only] => *only,
            _ => cells.intern_empty(NormalFormKind::And(rest)),
        };
        out.push(EqualityConjunct {
            scrutinee,
            key,
            residual,
        });
    }
    out
}

fn is_equality_against_literal(
    cells: &Cells<'_>,
    id: NormalFormId,
) -> Option<(NormalFormId, LiteralValue)> {
    let cell = cells.get(id);
    if cell.rule_ref.is_some() {
        return None;
    }
    let NormalFormKind::Comparison(left, operator, right) = &cell.kind else {
        return None;
    };
    if !matches!(operator, ComparisonComputation::Is) {
        return None;
    }
    match (
        literal_operand(cells, *left),
        literal_operand(cells, *right),
    ) {
        (None, Some(key)) => Some((*left, key)),
        (Some(key), None) => Some((*right, key)),
        (Some(_), Some(_)) | (None, None) => None,
    }
}

fn literal_operand(cells: &Cells<'_>, id: NormalFormId) -> Option<LiteralValue> {
    let cell = cells.get(id);
    if cell.rule_ref.is_some() {
        return None;
    }
    match &cell.kind {
        NormalFormKind::Leaf(LeafKind::Literal(literal)) => Some(literal.clone()),
        _ => None,
    }
}

/// Stamped result type of the scrutinee cell, when it is a dispatch class.
/// Declines undetermined / range / boolean stamps (no table key space).
fn dispatch_scrutinee_type<'a>(
    cells: &'a Cells<'_>,
    scrutinee: NormalFormId,
) -> Option<&'a LemmaType> {
    let ty = cells.result_type(scrutinee).as_ref();
    if ty.is_undetermined() || ty.is_range() || ty.is_boolean() {
        return None;
    }
    Some(ty)
}

fn is_numeric_zero(cells: &Cells<'_>, id: NormalFormId) -> bool {
    as_rational_literal(cells, id).is_some_and(|r| rational_is_zero(&r))
}

fn is_numeric_one(cells: &Cells<'_>, id: NormalFormId) -> bool {
    as_rational_literal(cells, id).is_some_and(|r| r == rational_new(1, 1))
}

fn as_rational_literal(cells: &Cells<'_>, id: NormalFormId) -> Option<RationalInteger> {
    if cells.get(id).rule_ref.is_some() {
        return None;
    }
    match &cells.get(id).kind {
        NormalFormKind::Leaf(LeafKind::Literal(literal)) => match &literal.value {
            ValueKind::Number(number) => Some(number.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// A literal leaf: evaluates without veto, so reordering or dropping it around
/// a partial operand cannot change which veto surfaces. Rule refs are opaque
/// even when the shared Kind is a literal.
fn is_total(cells: &Cells<'_>, id: NormalFormId) -> bool {
    let cell = cells.get(id);
    cell.rule_ref.is_none() && matches!(&cell.kind, NormalFormKind::Leaf(LeafKind::Literal(_)))
}

fn as_integer_literal(cells: &Cells<'_>, id: NormalFormId) -> Option<RationalInteger> {
    let rational = as_rational_literal(cells, id)?;
    if rational.is_integer() {
        Some(rational)
    } else {
        None
    }
}
