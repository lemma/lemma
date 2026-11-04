//! List command results over evaluated operands. The frame machine in
//! [`super::tree`] evaluates operands and row reads; the functions here combine them.

use crate::computation::rational::{rational_new, rational_zero, RationalInteger};
use crate::computation::{arithmetic_operation, comparison_operation, OperationResult, VetoType};
use crate::evaluation::rows::checked_row_field;
use crate::planning::execution_plan::ExecutionPlan;
use crate::planning::instance_list::{describe_key, row_end_type, typed_from_bound};
use crate::planning::normalize::{NormalFormId, NormalFormListOp};
use crate::planning::semantics::{
    primitive_number_arc, row_literal, ArithmeticComputation, BoundValueKind,
    ComparisonComputation, DataPath, KeyField, LemmaType, ListKey, ListPredKind, ListValue,
    LiteralValue, RowBinding, RowEnd, RowInstance, RowOrigin, SpecIdentity, TypeSpecification,
    TypedLiteral, ValueKind,
};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::sync::Arc;

pub(crate) fn list_key(list_type: &LemmaType) -> &ListKey {
    match &list_type.specifications {
        TypeSpecification::List { key, .. } => key,
        other => panic!("BUG: list command operand type is {other:?}"),
    }
}

pub(crate) fn list_result(rows: impl Into<ListValue>, unit: Option<Arc<str>>) -> OperationResult {
    OperationResult::from_bound(BoundValueKind {
        value: ValueKind::List(rows.into()),
        unit,
    })
}

pub(crate) fn count_rows(rows: &ListValue) -> OperationResult {
    OperationResult::from_literal(LiteralValue::number(rational_new(
        i64::try_from(rows.len()).expect("BUG: list len fits i64"),
        1,
    )))
}

pub(crate) fn first_or_last(rows: &ListValue, first: bool) -> OperationResult {
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
            primitive_number_arc(),
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
    rows: &ListValue,
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
    list_result(
        rows.iter()
            .take(count)
            .map(Cow::into_owned)
            .collect::<Arc<[_]>>(),
        unit,
    )
}

pub(crate) fn find_numbered(rows: &ListValue, key_result: OperationResult) -> OperationResult {
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

pub(crate) fn filter_rows(
    rows: &ListValue,
    flags: Vec<bool>,
    unit: Option<Arc<str>>,
) -> OperationResult {
    let kept = rows
        .iter()
        .zip(flags)
        .filter(|(_, keep)| *keep)
        .map(|(row, _)| row.into_owned())
        .collect::<Arc<[_]>>();
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

/// Result of a list command whose per-row operand values are `values`
/// (one per row of `rows`, in row order). Used for value-list and instance-list
/// predicates, reductions, and sorts.
pub(crate) fn per_row_result(
    op: &NormalFormListOp,
    rows: &ListValue,
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
        NormalFormListOp::Predicate { kind, .. } => {
            let flags: Vec<bool> = values
                .iter()
                .map(|value| match value.value {
                    ValueKind::Boolean(flag) => flag,
                    ref other => panic!("BUG: predicate evaluated to {other:?}"),
                })
                .collect();
            match kind {
                ListPredKind::Any => {
                    OperationResult::from_literal(LiteralValue::from_bool(flags.contains(&true)))
                }
                ListPredKind::All => {
                    OperationResult::from_literal(LiteralValue::from_bool(!flags.contains(&false)))
                }
                ListPredKind::Where => filter_rows(rows, flags, unit),
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
                order
                    .into_iter()
                    .map(|index| {
                        rows.get(index)
                            .expect("BUG: sort order holds positions of the list")
                            .into_owned()
                    })
                    .collect::<Arc<[_]>>(),
                unit,
            )
        }
        other => unreachable!("BUG: {other:?} is not a per-row list reduction"),
    }
}

pub(crate) fn no_row_with_key(fields: &[KeyField], wanted: &[TypedLiteral]) -> OperationResult {
    let names: Vec<String> = fields.iter().map(|field| field.name.clone()).collect();
    let described = describe_key(&names, wanted);
    OperationResult::Veto(VetoType::computation(format!(
        "No row found where {described}."
    )))
}

/// Append local instances (`-> with kim`) and/or other instance lists onto `rows`. An
/// appended instance is the instance itself: its fields are `kim.*`, bound or data.
pub(crate) fn append_instance_operands(
    rows: &ListValue,
    values: Vec<OperationResult>,
    list_type: &LemmaType,
    unit: Option<Arc<str>>,
) -> OperationResult {
    let identity = match &list_type.specifications {
        TypeSpecification::List { element, .. } => match &element.specifications {
            TypeSpecification::Spec { spec: identity } => identity.clone(),
            other => panic!("BUG: instance list element type is {other:?}"),
        },
        other => panic!("BUG: list command operand type is {other:?}"),
    };
    let row_type = Arc::new(LemmaType::primitive(TypeSpecification::Spec {
        spec: identity,
    }));
    let mut kept = rows.to_vec();
    for value in values {
        let OperationResult::Value(bound) = value else {
            panic!("BUG: `with` operand vetoed after the veto check");
        };
        match bound.value {
            row @ ValueKind::Spec(_) => kept.push(TypedLiteral {
                value: row,
                lemma_type: Arc::clone(&row_type),
            }),
            ValueKind::List(items) => {
                for item in items.iter() {
                    match &item.value {
                        row @ ValueKind::Spec(_) => kept.push(TypedLiteral {
                            value: row.clone(),
                            lemma_type: Arc::clone(&row_type),
                        }),
                        other => panic!("BUG: instance list `with` list item is {other:?}"),
                    }
                }
            }
            other => panic!("BUG: instance list `with` operand is {other:?}"),
        }
    }
    list_result(kept, unit)
}

/// The row `-> with field: value` writes inline: one value per field, checked against
/// the row spec `identity`. It has only the fields the command sets.
pub(crate) fn inline_row(
    plan: &ExecutionPlan,
    identity: &Arc<SpecIdentity>,
    fields: &[DataPath],
    values: Vec<OperationResult>,
    origin: RowOrigin,
) -> TypedLiteral {
    assert_eq!(
        fields.len(),
        values.len(),
        "BUG: inline row has one value per field"
    );
    assert!(
        origin.is_inline(),
        "BUG: inline row built with origin {origin}"
    );
    let template = plan.row_template(identity);
    let mut bindings = vec![None; template.data.len()];
    for (field, value) in fields.iter().zip(values) {
        let slot = template
            .data
            .get_index_of(field)
            .expect("BUG: inline row field is template data");
        bindings[slot] = Some(RowBinding::Bound(checked_row_field(template, field, value)));
    }
    row_literal(
        Arc::clone(identity),
        origin,
        bindings,
        Arc::new(LemmaType::primitive(TypeSpecification::Spec {
            spec: SpecIdentity::clone(identity),
        })),
    )
}

/// Type of a list key field on `row`, and its end on the row spec's template.
pub(crate) fn key_field_type(
    plan: &ExecutionPlan,
    row: &RowInstance,
    key_field: &KeyField,
) -> (RowEnd, Arc<LemmaType>) {
    let template = plan.row_template(&row.identity);
    let end = RowEnd::Data(key_field.path.clone());
    let field_type = row_end_type(template, &end);
    (end, field_type)
}

pub(crate) fn typed_operand(value: OperationResult, lemma_type: &Arc<LemmaType>) -> TypedLiteral {
    match value {
        OperationResult::Value(bound) => typed_from_bound(&bound, lemma_type),
        OperationResult::Veto(_) => panic!("BUG: keyed operand vetoed after the veto check"),
    }
}
