//! Rewrite driver: one post-order walk over a lowered [`NormalForm`] graph, then
//! an ordered chain of local rewrites at every cell.
//!
//! A pass sees a cell whose children are already normalized and returns
//! `Some(new_id)` when it rewrote the cell, `None` when its pattern does not
//! apply. Children a pass builds are normalized before the next pass runs, so
//! every pass reads normalized children. Passes never descend, never enter a
//! rule reference, and never re-intern an unchanged cell.

use super::{
    literal_from_folded_rational, normalization_error, Cells, LeafKind, NormalFormId,
    NormalFormKind, NormalizeContext,
};
use crate::computation::comparison::comparison_operation;
use crate::computation::rational::{
    rational_is_zero, rational_new, rational_one, rational_operation, rational_zero,
    NumericOperation, RationalInteger,
};
use crate::parsing::ast::PrimitiveKind;
use crate::planning::ordered_dispatch::{
    classify_dispatch, dispatch_key_of_literal, paint_dispatch_regions, region_for_value,
    sorted_unique_boundaries, DispatchClass, DispatchKeyBuildError,
};
use crate::planning::semantics::{
    mirrored_comparison, negated_comparison, primitive_number_arc, ComparisonComputation,
    LemmaType, LiteralValue, MathematicalComputation, SemanticConversionTarget, Source, ValueKind,
};
use crate::Error;
use indexmap::IndexMap;
use std::collections::HashMap;
use std::sync::Arc;

/// Normalize `root`: every reachable cell, children first, through [`PASSES`].
pub(super) fn normalize(
    root: NormalFormId,
    cells: Cells<'_>,
    _ctx: &NormalizeContext<'_>,
    source: Option<Source>,
) -> Result<NormalFormId, Error> {
    let mut normalizer = Normalizer {
        cells,
        source,
        normalized: HashMap::new(),
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
}

/// One local rewrite. `Ok(None)`: pattern does not apply, cell untouched.
type Pass = fn(&mut Normalizer<'_>, NormalFormId) -> Result<Option<NormalFormId>, Error>;

/// Rewrites applied at every cell, in this order, once each.
const PASSES: [Pass; 13] = [
    expand_numeric_subtract_divide,
    flatten_associative,
    power_laws,
    eliminate_identities,
    double_negate_reciprocal,
    math_identities,
    constant_fold,
    fold_number_conversion,
    negated_comparisons,
    logical_idempotency,
    collapse_piecewise,
    canonical_order,
    ordered_dispatch,
];

impl Normalizer<'_> {
    fn normalize(&mut self, id: NormalFormId) -> Result<NormalFormId, Error> {
        // A rule reference shares the referenced body's Kind, normalized when
        // that rule completed. Entering it would rebuild without `rule_ref` and
        // re-walk the body (Θ(2ⁿ) on `r and not r` chains).
        if self.cells.get(id).rule_ref.is_some() {
            return Ok(id);
        }
        if let Some(&result) = self.normalized.get(&id) {
            return Ok(result);
        }
        let children = self.cells.get(id).kind.children();
        let with_children = self.with_normalized_children(id, children, |_| true)?;
        let result = self.rewrite(with_children)?;
        self.normalized.insert(id, result);
        self.normalized.insert(result, result);
        Ok(result)
    }

    fn rewrite(&mut self, id: NormalFormId) -> Result<NormalFormId, Error> {
        let mut current = id;
        for pass in PASSES {
            let first_fresh = self.cells.len();
            if let Some(rewritten) = pass(self, current)? {
                // Cells the pass built under its result have not been walked.
                let children = self.cells.get(rewritten).kind.children();
                current = self.with_normalized_children(rewritten, children, |child| {
                    child.index() >= first_fresh
                })?;
            }
        }
        Ok(current)
    }

    /// `id` with every child accepted by `select` normalized. Returns `id`
    /// itself when no child changed; otherwise a rebuilt cell carrying `id`'s
    /// source and origin.
    fn with_normalized_children(
        &mut self,
        id: NormalFormId,
        children: Vec<NormalFormId>,
        select: impl Fn(NormalFormId) -> bool,
    ) -> Result<NormalFormId, Error> {
        let mut replaced = Vec::with_capacity(children.len());
        for child in &children {
            replaced.push(if select(*child) {
                self.normalize(*child)?
            } else {
                *child
            });
        }
        if replaced == children {
            return Ok(id);
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
        Ok(self.cells.rebuild(kind, source, origin))
    }
}

fn expand_numeric_subtract_divide(
    n: &mut Normalizer<'_>,
    id: NormalFormId,
) -> Result<Option<NormalFormId>, Error> {
    let rewritten = match &n.cells.get(id).kind {
        NormalFormKind::Subtract(a, b)
            if is_numeric_only(&n.cells, *a) && is_numeric_only(&n.cells, *b) =>
        {
            let (a, b) = (*a, *b);
            let negate_b = n.cells.rebuild(NormalFormKind::Negate(b), None, None);
            n.cells
                .fold_into(NormalFormKind::Sum(vec![a, negate_b]), id)
        }
        NormalFormKind::Divide(a, b)
            if is_numeric_only(&n.cells, *a) && is_numeric_only(&n.cells, *b) =>
        {
            let (a, b) = (*a, *b);
            let reciprocal_b = n.cells.rebuild(NormalFormKind::Reciprocal(b), None, None);
            n.cells
                .fold_into(NormalFormKind::Product(vec![a, reciprocal_b]), id)
        }
        _ => return Ok(None),
    };
    Ok(Some(rewritten))
}

fn is_numeric_only(cells: &Cells<'_>, id: NormalFormId) -> bool {
    if cells.get(id).rule_ref.is_some() {
        return false;
    }
    match &cells.get(id).kind {
        NormalFormKind::Leaf(LeafKind::Literal(literal)) => {
            matches!(literal.value, ValueKind::Number(_))
        }
        NormalFormKind::Sum(children) | NormalFormKind::Product(children) => {
            children.iter().all(|c| is_numeric_only(cells, *c))
        }
        NormalFormKind::Subtract(a, b)
        | NormalFormKind::Divide(a, b)
        | NormalFormKind::Power(a, b) => is_numeric_only(cells, *a) && is_numeric_only(cells, *b),
        NormalFormKind::Negate(x) | NormalFormKind::Reciprocal(x) => is_numeric_only(cells, *x),
        _ => false,
    }
}

/// Merge a nested Sum into its Sum parent (same for Product).
fn flatten_associative(
    n: &mut Normalizer<'_>,
    id: NormalFormId,
) -> Result<Option<NormalFormId>, Error> {
    let Some((operator, children)) = Associative::split(&n.cells.get(id).kind) else {
        return Ok(None);
    };
    let children = children.to_vec();
    let mut flat = Vec::with_capacity(children.len());
    let mut flattened = false;
    for child in children {
        let cell = n.cells.get(child);
        match Associative::split(&cell.kind) {
            Some((inner_operator, inner))
                if inner_operator == operator && cell.rule_ref.is_none() =>
            {
                flat.extend_from_slice(inner);
                flattened = true;
            }
            _ => flat.push(child),
        }
    }
    if !flattened {
        return Ok(None);
    }
    Ok(Some(n.cells.fold_into(operator.wrap(flat), id)))
}

/// The two associative arithmetic operators whose n-ary cells flatten and sort.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Associative {
    Sum,
    Product,
}

impl Associative {
    fn split(kind: &NormalFormKind) -> Option<(Self, &[NormalFormId])> {
        match kind {
            NormalFormKind::Sum(children) => Some((Associative::Sum, children)),
            NormalFormKind::Product(children) => Some((Associative::Product, children)),
            _ => None,
        }
    }

    fn wrap(self, children: Vec<NormalFormId>) -> NormalFormKind {
        match self {
            Associative::Sum => NormalFormKind::Sum(children),
            Associative::Product => NormalFormKind::Product(children),
        }
    }
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
        NormalFormKind::Product(children) => {
            let children = children.clone();
            Ok(collect_like_base_powers(&mut n.cells, children, id))
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
    children: Vec<NormalFormId>,
    destroyed_id: NormalFormId,
) -> Option<NormalFormId> {
    let mut powers: Vec<(NormalFormId, NormalFormId, Option<Source>)> = Vec::new();
    let mut other = Vec::new();
    let mut merged = false;
    for c in children {
        let power_shape = match &cells.get(c).kind {
            NormalFormKind::Power(base, exp) => Some((*base, *exp)),
            _ => None,
        };
        let Some((base_val, exp_val)) = power_shape else {
            other.push(c);
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
        other.push(cells.rebuild(NormalFormKind::Power(base, exp), factor_source, None));
    }
    Some(match other.len() {
        0 => unreachable!("BUG: merged powers leave at least one factor"),
        1 => {
            let survivor = other.into_iter().next().expect("BUG: single product term");
            cells.fold_into_survivor(survivor, destroyed_id)
        }
        _ => cells.fold_into(NormalFormKind::Product(other), destroyed_id),
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
        NormalFormKind::Sum(children) => {
            let kept: Vec<NormalFormId> = children
                .iter()
                .copied()
                .filter(|c| !is_numeric_zero(cells, *c))
                .collect();
            if kept.len() == children.len() {
                return Ok(None);
            }
            match kept.len() {
                0 => cells.fold_into(
                    NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::number(rational_zero()))),
                    id,
                ),
                1 => cells.fold_into_survivor(kept[0], id),
                _ => cells.fold_into(NormalFormKind::Sum(kept), id),
            }
        }
        NormalFormKind::Product(children) => {
            let children = children.clone();
            if children.iter().any(|c| is_numeric_zero(cells, *c)) {
                if let Some(zero) = typed_product_zero(cells, &children) {
                    let zero_kind = cells.get(zero).kind.clone();
                    return Ok(Some(cells.fold_into(zero_kind, id)));
                }
            }
            let kept: Vec<NormalFormId> = children
                .iter()
                .copied()
                .filter(|c| !is_numeric_one(cells, *c))
                .collect();
            if kept.len() == children.len() {
                return Ok(None);
            }
            match kept.len() {
                0 => cells.fold_into(
                    NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::number(rational_one()))),
                    id,
                ),
                1 => cells.fold_into_survivor(kept[0], id),
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
        NormalFormKind::And(left, right) => {
            let (left, right) = (*left, *right);
            // Left `false` decides before the right can veto.
            if is_literal_bool(cells, left, false) {
                cells.fold_into(
                    NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::from_bool(false))),
                    id,
                )
            } else if is_literal_bool(cells, left, true) {
                cells.fold_into_survivor(right, id)
            } else if is_literal_bool(cells, right, true) {
                cells.fold_into_survivor(left, id)
            } else if is_literal_bool(cells, right, false) && is_total(cells, left) {
                cells.fold_into(
                    NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::from_bool(false))),
                    id,
                )
            } else {
                return Ok(None);
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
            Some(cells.intern_literal_leaf(
                LiteralValue::measure_with_signature(rational_zero(), Arc::clone(&measure_type)),
                measure_type,
            ))
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

fn constant_fold(n: &mut Normalizer<'_>, id: NormalFormId) -> Result<Option<NormalFormId>, Error> {
    let source = n.source.clone();
    let cells = &mut n.cells;
    let rewritten = match &cells.get(id).kind {
        NormalFormKind::Sum(children) => {
            if !children
                .iter()
                .all(|c| as_rational_literal(cells, *c).is_some())
            {
                return Ok(None);
            }
            let mut sorted = children.clone();
            let mut compare_failure: Option<crate::computation::rational::NumericFailure> = None;
            sorted.sort_by(|a, b| match canonical_numeric_cmp(cells, *a, *b) {
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
            for child in &sorted {
                let rational = as_rational_literal(cells, *child).expect("BUG: all numeric");
                acc = rational_operation(&acc, NumericOperation::Add, &rational).map_err(
                    |failure| normalization_error(source.clone(), failure, "constant fold sum"),
                )?;
            }
            // The sorted sum is the recorded pre-image so explain reads terms
            // in canonical order.
            let cell = cells.get(id);
            let (cell_source, origin) = (cell.source.clone(), cell.origin);
            let destroyed_id = cells.rebuild(NormalFormKind::Sum(sorted), cell_source, origin);
            fold_number(cells, acc, destroyed_id)
        }
        NormalFormKind::Product(children) => {
            if !children
                .iter()
                .all(|c| as_rational_literal(cells, *c).is_some())
            {
                return Ok(None);
            }
            let mut acc = rational_new(1, 1);
            for child in children {
                let rational = as_rational_literal(cells, *child).expect("BUG: all numeric");
                acc = rational_operation(&acc, NumericOperation::Multiply, &rational).map_err(
                    |failure| normalization_error(source.clone(), failure, "constant fold product"),
                )?;
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
            // A power the rational domain cannot express (fractional exponent
            // of a non-perfect power) stays for evaluation.
            let Ok(rational) =
                rational_operation(&base_rational, NumericOperation::Power, &exp_rational)
            else {
                return Ok(None);
            };
            fold_number(cells, rational, id)
        }
        NormalFormKind::Negate(x) => {
            let Some(rational) = as_rational_literal(cells, *x) else {
                return Ok(None);
            };
            let negated =
                rational_operation(&rational_new(0, 1), NumericOperation::Subtract, &rational)
                    .map_err(|failure| {
                        normalization_error(source.clone(), failure, "constant fold negate")
                    })?;
            fold_number(cells, negated, id)
        }
        NormalFormKind::Reciprocal(x) => {
            let Some(rational) = as_rational_literal(cells, *x) else {
                return Ok(None);
            };
            let reciprocal =
                rational_operation(&rational_one(), NumericOperation::Divide, &rational).map_err(
                    |failure| {
                        normalization_error(source.clone(), failure, "constant fold reciprocal")
                    },
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
            let quotient =
                rational_operation(&left_rational, NumericOperation::Divide, &right_rational)
                    .map_err(|failure| {
                        normalization_error(source.clone(), failure, "constant fold divide")
                    })?;
            fold_number(cells, quotient, id)
        }
        NormalFormKind::Subtract(left, right) => {
            let (Some(left_rational), Some(right_rational)) = (
                as_rational_literal(cells, *left),
                as_rational_literal(cells, *right),
            ) else {
                return Ok(None);
            };
            let difference =
                rational_operation(&left_rational, NumericOperation::Subtract, &right_rational)
                    .map_err(|failure| {
                        normalization_error(source.clone(), failure, "constant fold subtract")
                    })?;
            fold_number(cells, difference, id)
        }
        NormalFormKind::Modulo(left, right) => {
            let (Some(left_rational), Some(right_rational)) = (
                as_rational_literal(cells, *left),
                as_rational_literal(cells, *right),
            ) else {
                return Ok(None);
            };
            let remainder =
                rational_operation(&left_rational, NumericOperation::Modulo, &right_rational)
                    .map_err(|failure| {
                        normalization_error(source.clone(), failure, "constant fold modulo")
                    })?;
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
                left_literal,
                cells.result_type(left),
                &op,
                right_literal,
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
        NormalFormKind::Leaf(LeafKind::Literal(LiteralValue::number_with_type(
            number,
            primitive_number_arc().clone(),
        ))),
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
    let NormalFormKind::And(left, right) = &cells.get(id).kind else {
        return Ok(None);
    };
    // Id equality only: Kind equality would collapse distinct rule references
    // that share body Kind after cons.
    if left != right {
        return Ok(None);
    }
    let left = *left;
    Ok(Some(cells.fold_into_survivor(left, id)))
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
    if cells.get(id).rule_ref.is_some() {
        return false;
    }
    if is_literal_bool(cells, id, false) {
        return true;
    }
    match &cells.get(id).kind {
        NormalFormKind::And(left, right) => {
            unless_condition_contains_literal_false(cells, *left)
                || unless_condition_contains_literal_false(cells, *right)
        }
        _ => false,
    }
}

/// Sort the terms of an all-numeric Sum / Product: literals first, ascending.
fn canonical_order(
    n: &mut Normalizer<'_>,
    id: NormalFormId,
) -> Result<Option<NormalFormId>, Error> {
    let cells = &mut n.cells;
    let Some((operator, children)) = Associative::split(&cells.get(id).kind) else {
        return Ok(None);
    };
    let children = children.to_vec();
    if !children.iter().all(|c| is_numeric_only(cells, *c)) {
        return Ok(None);
    }
    let mut sorted = children.clone();
    let mut compare_failure: Option<crate::computation::rational::NumericFailure> = None;
    sorted.sort_by(|a, b| match canonical_numeric_cmp(cells, *a, *b) {
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
    if sorted == children {
        return Ok(None);
    }
    Ok(Some(cells.fold_into(operator.wrap(sorted), id)))
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
/// the rewrite driver and recursively fold when residuals are themselves
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
    let bodies: Vec<NormalFormId> = arms.iter().skip(1).map(|(_, body)| *body).collect();
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
    let Some(partition) = scan_conjunctive_key_arms(&n.cells, arms) else {
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

    let mut group_bodies = Vec::with_capacity(partition.groups.len());
    let mut keys = Vec::with_capacity(partition.groups.len());
    let mut operators = Vec::with_capacity(partition.groups.len());
    for (key, group_arms) in &partition.groups {
        let mut piecewise_arms = Vec::with_capacity(group_arms.len() + 1);
        piecewise_arms.push((true_condition, default_body));
        piecewise_arms.extend(group_arms.iter().copied());
        let group_cell = n
            .cells
            .rebuild(NormalFormKind::Piecewise(piecewise_arms), None, None);
        group_bodies.push(group_cell);
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
    bodies: &'a [NormalFormId],
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
    let regions = paint_dispatch_regions(boundaries.len(), default_body, &paint_arms);

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
    cells: &Cells<'_>,
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
/// the other conjunct as residual. Empty when the condition is not an `And` of
/// that shape (including rule-ref cells).
fn equality_conjuncts(cells: &Cells<'_>, condition: NormalFormId) -> Vec<EqualityConjunct> {
    let cell = cells.get(condition);
    if cell.rule_ref.is_some() {
        return Vec::new();
    }
    let NormalFormKind::And(left, right) = &cell.kind else {
        return Vec::new();
    };
    let (left, right) = (*left, *right);
    let mut out = Vec::new();
    if let Some((scrutinee, key)) = is_equality_against_literal(cells, left) {
        out.push(EqualityConjunct {
            scrutinee,
            key,
            residual: right,
        });
    }
    if let Some((scrutinee, key)) = is_equality_against_literal(cells, right) {
        out.push(EqualityConjunct {
            scrutinee,
            key,
            residual: left,
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
