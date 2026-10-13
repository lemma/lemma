//! Structured explanation steps for unit conversions (`as`).

use std::cmp::Ordering;
use std::sync::Arc;

use crate::computation::rational::RationalInteger;
use crate::planning::explanation::{ConversionTraceRole, SerializedConversionTraceStep};
use crate::planning::semantics::{
    compare_semantic_dates, LemmaType, LiteralValue, SemanticConversionTarget, TypeSpecification,
    ValueKind,
};

/// Build ordered explanation steps (outcome → rule) after a successful unit conversion.
///
/// Measure Rule steps are declared factors against the type's reference unit (factor 1).
/// Identity and both-reference conversions omit Rule steps.
pub(crate) fn build_conversion_steps(
    value: &LiteralValue,
    value_type: &Arc<LemmaType>,
    target: &SemanticConversionTarget,
    result: &LiteralValue,
    result_type: &Arc<LemmaType>,
) -> Vec<SerializedConversionTraceStep> {
    let mut steps = Vec::new();
    steps.push(SerializedConversionTraceStep {
        role: ConversionTraceRole::Outcome,
        text: result.display_value_with_type(result_type.as_ref()),
    });

    for rule_text in conversion_rule_step_texts(value, value_type, target, result) {
        steps.push(SerializedConversionTraceStep {
            role: ConversionTraceRole::Rule,
            text: rule_text,
        });
    }

    steps
}

fn conversion_rule_step_texts(
    value: &LiteralValue,
    value_type: &LemmaType,
    target: &SemanticConversionTarget,
    result: &LiteralValue,
) -> Vec<String> {
    match &value.value {
        ValueKind::Range(left, right) => range_span_rule_step_text(left, right, result)
            .into_iter()
            .collect(),
        ValueKind::Measure(_) if !value_type.is_calendar_like() => match target {
            SemanticConversionTarget::Unit {
                unit_name,
                owning_type,
            } => {
                let from_signature = value_type.measure_runtime_signature();
                measure_unit_declared_factor_steps(&from_signature, unit_name, owning_type)
            }
            _ => Vec::new(),
        },
        ValueKind::Number(_) | ValueKind::Ratio(_) => Vec::new(),
        ValueKind::Measure(_) if value_type.is_calendar_like() => Vec::new(),
        _ => Vec::new(),
    }
}

fn range_span_rule_step_text(
    left: &crate::planning::semantics::TypedLiteral,
    right: &crate::planning::semantics::TypedLiteral,
    result: &LiteralValue,
) -> Option<String> {
    match (&left.value, &right.value) {
        (ValueKind::Date(left_date), ValueKind::Date(right_date)) => {
            let (lower, upper) = ordered_date_pair(left_date, right_date);
            let lower_literal = LiteralValue::date(lower.clone());
            let upper_literal = LiteralValue::date(upper.clone());
            Some(format!(
                "{} − {} = {}",
                upper_literal.display_value(),
                lower_literal.display_value(),
                result.display_value()
            ))
        }
        (ValueKind::Number(_), ValueKind::Number(_)) => {
            let left_lit = left.to_literal();
            let right_lit = right.to_literal();
            let (lower, upper) = ordered_number_pair(&left_lit, &right_lit);
            Some(format!(
                "{} − {} = {}",
                upper.display_value(),
                lower.display_value(),
                result.display_value()
            ))
        }
        (ValueKind::Measure(_), ValueKind::Measure(_)) => {
            let left_lit = left.to_literal();
            let right_lit = right.to_literal();
            let (lower, upper) = ordered_measure_pair(&left_lit, &right_lit);
            Some(format!(
                "{} − {} = {}",
                upper.display_value(),
                lower.display_value(),
                result.display_value()
            ))
        }
        _ => None,
    }
}

fn ordered_date_pair<'a>(
    left: &'a crate::planning::semantics::SemanticDateTime,
    right: &'a crate::planning::semantics::SemanticDateTime,
) -> (
    &'a crate::planning::semantics::SemanticDateTime,
    &'a crate::planning::semantics::SemanticDateTime,
) {
    match compare_semantic_dates(left, right) {
        Ordering::Less | Ordering::Equal => (left, right),
        Ordering::Greater => (right, left),
    }
}

fn ordered_number_pair<'a>(
    left: &'a LiteralValue,
    right: &'a LiteralValue,
) -> (&'a LiteralValue, &'a LiteralValue) {
    let ValueKind::Number(left_number) = &left.value else {
        unreachable!("BUG: ordered_number_pair called with non-number operand");
    };
    let ValueKind::Number(right_number) = &right.value else {
        unreachable!("BUG: ordered_number_pair called with non-number operand");
    };
    match left_number.try_cmp(right_number) {
        Ok(Ordering::Greater) => (right, left),
        Ok(Ordering::Less | Ordering::Equal) => (left, right),
        Err(_) => unreachable!("BUG: range endpoint compare after successful span conversion"),
    }
}

fn ordered_measure_pair<'a>(
    left: &'a LiteralValue,
    right: &'a LiteralValue,
) -> (&'a LiteralValue, &'a LiteralValue) {
    let ValueKind::Measure(left_magnitude) = &left.value else {
        unreachable!("BUG: ordered_measure_pair called with non-measure operand");
    };
    let ValueKind::Measure(right_magnitude) = &right.value else {
        unreachable!("BUG: ordered_measure_pair called with non-measure operand");
    };
    match left_magnitude.try_cmp(right_magnitude) {
        Ok(Ordering::Greater) => (right, left),
        Ok(Ordering::Less | Ordering::Equal) => (left, right),
        Err(_) => unreachable!("BUG: range endpoint compare after successful span conversion"),
    }
}

fn format_explanation_multiplier(rational: &RationalInteger) -> String {
    rational.display_str()
}

/// Declared-factor Rule lines: `1 {unit} is {unit.factor} {reference}` for each of
/// `{from, to}` that is not the reference unit (factor 1). Source first, then target.
///
/// Same-family only: when `from_unit` is absent from `owning_type` (cross-family
/// relabel), there is no factor equivalence to narrate. Identity / both-reference
/// conversions yield no lines.
fn measure_unit_declared_factor_steps(
    from_signature: &[(String, i32)],
    to_unit: &str,
    owning_type: &LemmaType,
) -> Vec<String> {
    let from_unit = from_signature
        .first()
        .map(|(name, _)| name.as_str())
        .unwrap_or("");
    if from_unit.is_empty() || from_signature.len() != 1 {
        return Vec::new();
    }
    if from_unit == to_unit {
        return Vec::new();
    }

    let units = match &owning_type.specifications {
        TypeSpecification::Measure { units, .. }
        | TypeSpecification::MeasureRange { units, .. } => units,
        _ => return Vec::new(),
    };
    if units.get(from_unit).is_err() || units.get(to_unit).is_err() {
        return Vec::new();
    }

    let Some(reference) = units.iter().find(|unit| unit.is_canonical_factor()) else {
        // Valid measures may omit a factor-1 unit; no declared-factor narration then.
        return Vec::new();
    };

    let mut steps = Vec::new();
    let mut emitted: Vec<&str> = Vec::new();
    for unit_name in [from_unit, to_unit] {
        if emitted.contains(&unit_name) {
            continue;
        }
        let unit = units
            .get(unit_name)
            .expect("BUG: unit presence checked above");
        if unit.is_canonical_factor() {
            continue;
        }
        let multiplier_display = format_explanation_multiplier(&unit.factor);
        steps.push(format!(
            "1 {unit_name} is {multiplier_display} {}",
            reference.name
        ));
        emitted.push(unit_name);
    }
    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computation::rational::{decimal_to_rational, rational_one};
    use crate::literals::{MeasureUnit, MeasureUnits};
    use crate::planning::semantics::TypeExtends;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn measure_type(name: &str, units: &[(&str, &str)]) -> Arc<LemmaType> {
        let units = MeasureUnits(
            units
                .iter()
                .map(|(unit_name, factor)| MeasureUnit {
                    name: (*unit_name).to_string(),
                    factor: decimal_to_rational(Decimal::from_str(factor).expect("factor decimal"))
                        .expect("BUG: test factor must be rational"),
                    derived_measure_factors: Vec::new(),
                    decomposition: Default::default(),
                    minimum: None,
                    maximum: None,
                    suggestion_magnitude: None,
                })
                .collect(),
        );
        Arc::new(LemmaType::new(
            name.to_string(),
            TypeSpecification::Measure {
                minimum: None,
                maximum: None,
                decimals: None,
                units,
                traits: Vec::new(),
                decomposition: None,
                unit: None,
                help: String::new(),
            },
            TypeExtends::custom_local(
                "measure".to_string(),
                crate::planning::semantics::tests::test_type_family(name),
            ),
        ))
    }

    fn rule_texts(from_unit: &str, to_unit: &str, ty: &Arc<LemmaType>) -> Vec<String> {
        let value_type = Arc::new(ty.as_ref().clone().with_unit(from_unit));
        let result_type = Arc::new(ty.as_ref().clone().with_unit(to_unit));
        let value = LiteralValue::measure(rational_one());
        let result = LiteralValue::measure(rational_one());
        let target = SemanticConversionTarget::Unit {
            unit_name: to_unit.to_string(),
            owning_type: Arc::clone(ty),
        };
        build_conversion_steps(&value, &value_type, &target, &result, &result_type)
            .into_iter()
            .filter(|step| matches!(step.role, ConversionTraceRole::Rule))
            .map(|step| step.text)
            .collect()
    }

    #[test]
    fn money_both_directions_share_one_declared_factor_line() {
        let money = measure_type("money", &[("eur", "1"), ("usd", "0.84")]);
        let expected = vec!["1 usd is 0.84 eur".to_string()];
        assert_eq!(rule_texts("eur", "usd", &money), expected);
        assert_eq!(rule_texts("usd", "eur", &money), expected);
    }

    #[test]
    fn mass_both_directions_share_one_declared_factor_line() {
        let mass = measure_type("mass", &[("kilogram", "1"), ("gram", "0.001")]);
        let expected = vec!["1 gram is 0.001 kilogram".to_string()];
        assert_eq!(rule_texts("kilogram", "gram", &mass), expected);
        assert_eq!(rule_texts("gram", "kilogram", &mass), expected);
    }

    #[test]
    fn length_two_non_reference_units_emit_source_then_target() {
        let length = measure_type(
            "length",
            &[("meter", "1"), ("mile", "1609.344"), ("kilometer", "1000")],
        );
        assert_eq!(
            rule_texts("mile", "kilometer", &length),
            vec![
                "1 mile is 1609.344 meter".to_string(),
                "1 kilometer is 1000 meter".to_string(),
            ]
        );
        assert_eq!(
            rule_texts("kilometer", "mile", &length),
            vec![
                "1 kilometer is 1000 meter".to_string(),
                "1 mile is 1609.344 meter".to_string(),
            ]
        );
    }

    #[test]
    fn identity_conversion_emits_outcome_only() {
        let money = measure_type("money", &[("eur", "1"), ("usd", "0.84")]);
        let value_type = Arc::new(money.as_ref().clone().with_unit("eur"));
        let value = LiteralValue::measure(rational_one());
        let target = SemanticConversionTarget::Unit {
            unit_name: "eur".to_string(),
            owning_type: Arc::clone(&money),
        };
        let steps = build_conversion_steps(&value, &value_type, &target, &value, &value_type);
        assert_eq!(steps.len(), 1);
        assert!(matches!(steps[0].role, ConversionTraceRole::Outcome));
        assert!(rule_texts("eur", "eur", &money).is_empty());
    }
}
