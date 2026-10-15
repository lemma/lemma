//! List command results over evaluated operands. The frame machine in
//! [`super::tree`] evaluates operands and row reads; the functions here combine them.

use crate::computation::range::check_containment;
use crate::computation::rational::{rational_new, rational_zero, RationalInteger};
use crate::computation::{arithmetic_operation, comparison_operation, OperationResult, VetoType};
use crate::evaluation::rows::checked_row_field;
use crate::parsing::ast::RowCondition;
use crate::planning::execution_plan::ExecutionPlan;
use crate::planning::instance_list::{
    describe_key, key_conflict, row_end_type, typed_from_bound, KeyedRow,
};
use crate::planning::normalize::{NormalFormId, NormalFormListOp};
use crate::planning::semantics::{
    row_literal, ArithmeticComputation, BoundValueKind, ComparisonComputation, DataPath, LemmaType,
    ListKey, LiteralValue, RowInstance, RowOrigin, SpecValue, TypeSpecification, TypedLiteral,
    ValueKind,
};
use indexmap::IndexMap;
use std::cmp::Ordering;
use std::sync::Arc;

pub(crate) fn row_instance(row: &TypedLiteral) -> &Arc<RowInstance> {
    match &row.value {
        ValueKind::Spec(SpecValue::Row(instance)) => instance,
        other => panic!("BUG: instance list row is {other:?}, not a row"),
    }
}

pub(crate) fn list_key(list_type: &LemmaType) -> &ListKey {
    match &list_type.specifications {
        TypeSpecification::List { key, .. } => key,
        other => panic!("BUG: list command operand type is {other:?}"),
    }
}

pub(crate) fn list_result(rows: Vec<TypedLiteral>, unit: Option<Arc<str>>) -> OperationResult {
    OperationResult::from_bound(BoundValueKind {
        value: ValueKind::List(rows),
        unit,
    })
}

pub(crate) fn count_rows(rows: &[TypedLiteral]) -> OperationResult {
    OperationResult::from_literal(LiteralValue::number(rational_new(
        i64::try_from(rows.len()).expect("BUG: list len fits i64"),
        1,
    )))
}

pub(crate) fn first_or_last(rows: &[TypedLiteral], first: bool) -> OperationResult {
    let row = if first { rows.first() } else { rows.last() };
    match row {
        Some(row) => OperationResult::from_literal(row.to_literal()),
        None => OperationResult::Veto(VetoType::computation(format!(
            "Cannot take the {} element of an empty list.",
            if first { "first" } else { "last" }
        ))),
    }
}

/// `None` when `n` is not a whole number >= 0; counts beyond `usize` saturate.
pub(crate) fn whole_count(n: &RationalInteger) -> Option<usize> {
    let reduced = RationalInteger::try_reduce_ref(n).expect("BUG: planned number reduces");
    if !reduced.is_integer() || reduced.numer_is_negative() {
        return None;
    }
    Some(reduced.numer_to_usize().unwrap_or(usize::MAX))
}

pub(crate) enum ListFold {
    Sum,
    Product,
    Avg,
}

pub(crate) fn reduce(
    rows: &[TypedLiteral],
    fold: ListFold,
    result_type: &Arc<LemmaType>,
    plan: &ExecutionPlan,
) -> OperationResult {
    if rows.is_empty() {
        return match fold {
            ListFold::Sum => empty_sum(result_type),
            ListFold::Avg => OperationResult::Veto(VetoType::computation(
                "Cannot compute the average of an empty list.".to_string(),
            )),
            ListFold::Product => OperationResult::Veto(VetoType::computation(
                "Cannot compute the product of an empty list.".to_string(),
            )),
        };
    }
    let mut acc = BoundValueKind::unbound(rows[0].value.clone());
    let mut acc_type = Arc::clone(&rows[0].lemma_type);
    for row in rows.iter().skip(1) {
        let op = match fold {
            ListFold::Sum | ListFold::Avg => ArithmeticComputation::Add,
            ListFold::Product => ArithmeticComputation::Multiply,
        };
        let next = arithmetic_operation(
            &acc,
            &acc_type,
            &op,
            &BoundValueKind::unbound(row.value.clone()),
            &row.lemma_type,
            &plan.signature_index,
        );
        match next {
            OperationResult::Value(bound) => {
                acc = bound;
                acc_type = Arc::clone(result_type);
            }
            veto @ OperationResult::Veto(_) => return veto,
        }
    }
    if matches!(fold, ListFold::Avg) {
        let count = BoundValueKind::unbound(ValueKind::Number(rational_new(
            i64::try_from(rows.len()).expect("BUG: list len fits i64"),
            1,
        )));
        return arithmetic_operation(
            &acc,
            result_type,
            &ArithmeticComputation::Divide,
            &count,
            crate::planning::semantics::primitive_number_arc(),
            &plan.signature_index,
        );
    }
    OperationResult::from_bound(acc)
}

fn empty_sum(result_type: &Arc<LemmaType>) -> OperationResult {
    let value = match &result_type.specifications {
        TypeSpecification::Number { .. } => ValueKind::Number(rational_zero()),
        TypeSpecification::Measure { .. } => ValueKind::Measure(rational_zero()),
        TypeSpecification::Ratio { .. } => ValueKind::Ratio(rational_zero()),
        other => panic!("BUG: `sum` on non-numeric element {other} after planning"),
    };
    OperationResult::from_literal(LiteralValue { value })
}

pub(crate) fn extrema(rows: &[TypedLiteral], want: Ordering) -> OperationResult {
    let Some(mut best) = rows.first() else {
        return OperationResult::Veto(VetoType::computation(
            "Cannot compute the minimum or maximum of an empty list.".to_string(),
        ));
    };
    for row in rows.iter().skip(1) {
        match order_rows(row, best) {
            Ok(order) if order == want => best = row,
            Ok(_) => {}
            Err(veto) => return OperationResult::Veto(veto),
        }
    }
    OperationResult::from_literal(best.to_literal())
}

fn row_less_than(left: &TypedLiteral, right: &TypedLiteral) -> Result<bool, VetoType> {
    match comparison_operation(
        &left.value,
        &left.lemma_type,
        &ComparisonComputation::LessThan,
        &right.value,
        &right.lemma_type,
    ) {
        OperationResult::Value(flag) => match flag.value {
            ValueKind::Boolean(holds) => Ok(holds),
            other => panic!("BUG: row comparison returned {other:?}"),
        },
        OperationResult::Veto(veto) => Err(veto),
    }
}

pub(crate) fn order_rows(left: &TypedLiteral, right: &TypedLiteral) -> Result<Ordering, VetoType> {
    if row_less_than(left, right)? {
        Ok(Ordering::Less)
    } else if row_less_than(right, left)? {
        Ok(Ordering::Greater)
    } else {
        Ok(Ordering::Equal)
    }
}

pub(crate) fn sort_rows(
    rows: &[TypedLiteral],
    descending: bool,
    unit: Option<Arc<str>>,
) -> OperationResult {
    let mut kept = rows.to_vec();
    let mut first_veto = None;
    kept.sort_by(|a, b| match order_rows(a, b) {
        Ok(order) if descending => order.reverse(),
        Ok(order) => order,
        Err(veto) => {
            first_veto.get_or_insert(veto);
            Ordering::Equal
        }
    });
    if let Some(veto) = first_veto {
        return OperationResult::Veto(veto);
    }
    list_result(kept, unit)
}

pub(crate) fn top_rows(
    rows: &[TypedLiteral],
    n_result: OperationResult,
    unit: Option<Arc<str>>,
) -> OperationResult {
    let OperationResult::Value(n_bound) = n_result else {
        return n_result;
    };
    let ValueKind::Number(n) = &n_bound.value else {
        panic!("BUG: top n is not a number after planning");
    };
    let Some(count) = whole_count(n) else {
        return OperationResult::Veto(VetoType::computation(format!(
            "`top` requires a whole number greater than or equal to 0; got {}.",
            n.display_str()
        )));
    };
    list_result(rows.iter().take(count).cloned().collect(), unit)
}

pub(crate) fn find_numbered(rows: &[TypedLiteral], key_result: OperationResult) -> OperationResult {
    let OperationResult::Value(key_bound) = &key_result else {
        return key_result;
    };
    let ValueKind::Number(n) = &key_bound.value else {
        panic!("BUG: numbered find key is not a number after planning");
    };
    let index = match whole_count(n) {
        Some(index) if index >= 1 => index,
        _ => {
            return OperationResult::Veto(VetoType::computation(format!(
                "Row number must be a whole number greater than or equal to 1; got {}.",
                n.display_str()
            )));
        }
    };
    match rows.get(index - 1) {
        Some(row) => OperationResult::from_literal(row.to_literal()),
        None => OperationResult::Veto(VetoType::computation(format!(
            "There is no row number {index}."
        ))),
    }
}

/// Per-row truth of a value-list condition against the right operand `right`; the first
/// vetoed row (or a vetoed right operand) is the `Err`.
pub(crate) fn condition_flags(
    rows: &[TypedLiteral],
    condition: &RowCondition,
    right: &OperationResult,
    right_type: &Arc<LemmaType>,
) -> Result<Vec<bool>, OperationResult> {
    let OperationResult::Value(right_bound) = right else {
        return Err(right.clone());
    };
    let mut flags = Vec::with_capacity(rows.len());
    for row in rows {
        let result = match condition {
            RowCondition::Compare(op) => comparison_operation(
                &row.value,
                &row.lemma_type,
                op,
                &right_bound.value,
                right_type,
            ),
            RowCondition::In => {
                let ValueKind::Range(lower, upper) = &right_bound.value else {
                    panic!("BUG: `in` operand of a list condition is not a range after planning");
                };
                let endpoint_type = right_type
                    .specifications
                    .element_from_range()
                    .map(|element| Arc::new(LemmaType::primitive(element)))
                    .expect("BUG: `in` operand of a list condition has a range type");
                check_containment(
                    &row.value,
                    &row.lemma_type,
                    &lower.value,
                    &upper.value,
                    &endpoint_type,
                )
            }
        };
        match result {
            OperationResult::Value(flag) => {
                let ValueKind::Boolean(b) = flag.value else {
                    panic!("BUG: list condition must evaluate to boolean");
                };
                flags.push(b);
            }
            veto @ OperationResult::Veto(_) => return Err(veto),
        }
    }
    Ok(flags)
}

pub(crate) fn predicate_result(flags: &[bool], any: bool) -> OperationResult {
    let holds = if any {
        flags.iter().any(|b| *b)
    } else {
        flags.iter().all(|b| *b)
    };
    OperationResult::from_literal(LiteralValue::from_bool(holds))
}

pub(crate) fn filter_rows(
    rows: &[TypedLiteral],
    flags: Vec<bool>,
    unit: Option<Arc<str>>,
) -> OperationResult {
    let kept = rows
        .iter()
        .zip(flags)
        .filter(|(_, keep)| *keep)
        .map(|(row, _)| row.clone())
        .collect();
    list_result(kept, unit)
}

/// Value-list `with`: the operands appended to `rows`, each typed by its own cell.
pub(crate) fn append_values(
    rows: &[TypedLiteral],
    values: Vec<OperationResult>,
    args: &[NormalFormId],
    plan: &ExecutionPlan,
    unit: Option<Arc<str>>,
) -> OperationResult {
    let mut kept = rows.to_vec();
    for (value, arg) in values.into_iter().zip(args) {
        let OperationResult::Value(bound) = value else {
            panic!("BUG: `with` operand vetoed after the veto check");
        };
        kept.push(TypedLiteral {
            value: bound.value,
            lemma_type: Arc::clone(&plan.normal_form(*arg).result_type),
        });
    }
    list_result(kept, unit)
}

/// Result of an instance-list command whose per-row operand values are `values`
/// (one per row of `rows`, in row order).
pub(crate) fn per_row_result(
    op: &NormalFormListOp,
    rows: &[TypedLiteral],
    values: &[TypedLiteral],
    unit: Option<Arc<str>>,
    result_type: &Arc<LemmaType>,
    plan: &ExecutionPlan,
) -> OperationResult {
    match op {
        NormalFormListOp::Sum => reduce(values, ListFold::Sum, result_type, plan),
        NormalFormListOp::Product => reduce(values, ListFold::Product, result_type, plan),
        NormalFormListOp::Avg => reduce(values, ListFold::Avg, result_type, plan),
        NormalFormListOp::Min => extrema(values, Ordering::Less),
        NormalFormListOp::Max => extrema(values, Ordering::Greater),
        NormalFormListOp::AnyRow | NormalFormListOp::AllRow | NormalFormListOp::WhereRow => {
            let flags: Vec<bool> = values
                .iter()
                .map(|value| match value.value {
                    ValueKind::Boolean(flag) => flag,
                    ref other => panic!("BUG: row condition evaluated to {other:?}"),
                })
                .collect();
            match op {
                NormalFormListOp::AnyRow => {
                    OperationResult::from_literal(LiteralValue::from_bool(flags.contains(&true)))
                }
                NormalFormListOp::AllRow => {
                    OperationResult::from_literal(LiteralValue::from_bool(!flags.contains(&false)))
                }
                _ => filter_rows(rows, flags, unit),
            }
        }
        NormalFormListOp::Sort { descending } => {
            let mut order: Vec<usize> = (0..rows.len()).collect();
            let mut first_veto = None;
            order.sort_by(|&a, &b| match order_rows(&values[a], &values[b]) {
                Ok(ordering) if *descending => ordering.reverse(),
                Ok(ordering) => ordering,
                Err(veto) => {
                    first_veto.get_or_insert(veto);
                    Ordering::Equal
                }
            });
            if let Some(veto) = first_veto {
                return OperationResult::Veto(veto);
            }
            list_result(
                order.into_iter().map(|index| rows[index].clone()).collect(),
                unit,
            )
        }
        other => unreachable!("BUG: {other:?} is not an instance-row reduction"),
    }
}

/// Whether `key` (a row's key values) matches `wanted`; the first vetoed comparison decides.
pub(crate) fn keys_match(
    key: &[TypedLiteral],
    wanted: &[TypedLiteral],
) -> Result<bool, OperationResult> {
    for (row_value, value) in key.iter().zip(wanted) {
        match key_matches(row_value, value) {
            OperationResult::Value(flag) => match flag.value {
                ValueKind::Boolean(true) => {}
                ValueKind::Boolean(false) => return Ok(false),
                other => panic!("BUG: key match returned {other:?}"),
            },
            veto @ OperationResult::Veto(_) => return Err(veto),
        }
    }
    Ok(true)
}

/// Whether `value` selects a row whose key field holds `row_value`: equal, or inside a range key.
fn key_matches(row_value: &TypedLiteral, value: &TypedLiteral) -> OperationResult {
    match &row_value.value {
        ValueKind::Range(lower, upper) => {
            let endpoint_type = row_value
                .lemma_type
                .specifications
                .element_from_range()
                .map(|element| Arc::new(LemmaType::primitive(element)))
                .expect("BUG: range key field has a range type");
            check_containment(
                &value.value,
                &value.lemma_type,
                &lower.value,
                &upper.value,
                &endpoint_type,
            )
        }
        _ => comparison_operation(
            &row_value.value,
            &row_value.lemma_type,
            &ComparisonComputation::Is,
            &value.value,
            &value.lemma_type,
        ),
    }
}

pub(crate) fn no_row_with_key(fields: &[String], wanted: &[TypedLiteral]) -> OperationResult {
    let described = describe_key(fields, wanted);
    OperationResult::Veto(VetoType::computation(format!(
        "No row found where {described}."
    )))
}

/// The row `with` appends: one value per field, typed by the row spec of `list_type`.
pub(crate) fn appended_instance_row(
    plan: &ExecutionPlan,
    list_type: &LemmaType,
    fields: &[DataPath],
    values: Vec<OperationResult>,
    rows_before: usize,
) -> TypedLiteral {
    assert_eq!(
        fields.len(),
        values.len(),
        "BUG: appended row has one value per field"
    );
    let identity = match &list_type.specifications {
        TypeSpecification::List { element, .. } => match &element.specifications {
            TypeSpecification::Spec { spec: identity } => identity.clone(),
            other => panic!("BUG: instance list element type is {other:?}"),
        },
        other => panic!("BUG: list command operand type is {other:?}"),
    };
    let template = plan.row_template(&identity);
    let mut bindings = IndexMap::new();
    for (field, value) in fields.iter().zip(values) {
        bindings.insert(field.clone(), checked_row_field(template, field, value));
    }
    row_literal(
        identity.clone(),
        RowOrigin::Appended {
            number: rows_before + 1,
        },
        bindings,
        Arc::new(LemmaType::primitive(TypeSpecification::Spec {
            spec: identity,
        })),
    )
}

/// Type of the key field `field` of `row`, and its end on the row spec's template.
pub(crate) fn key_field_type(
    plan: &ExecutionPlan,
    row: &RowInstance,
    field: &str,
) -> (crate::planning::semantics::RowEnd, Arc<LemmaType>) {
    let template = plan.row_template(&row.identity);
    let end = crate::planning::instance_list::row_end(template, field)
        .unwrap_or_else(|| panic!("BUG: key field `{field}` is not a field of the row spec"));
    let field_type = row_end_type(template, &end);
    (end, field_type)
}

pub(crate) fn key_conflict_result(
    fields: &[String],
    keyed: &[KeyedRow],
) -> Option<OperationResult> {
    key_conflict(fields, keyed)
        .map(|conflict| OperationResult::Veto(VetoType::computation(conflict)))
}

pub(crate) fn typed_operand(value: OperationResult, lemma_type: &Arc<LemmaType>) -> TypedLiteral {
    match value {
        OperationResult::Value(bound) => typed_from_bound(&bound, lemma_type),
        OperationResult::Veto(_) => panic!("BUG: keyed operand vetoed after the veto check"),
    }
}
