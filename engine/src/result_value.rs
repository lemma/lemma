//! API value for a rule result, `ShowData.fill`, or `ShowData.suggestion`.
//!
//! This is the single API-facing value representation shared by all three sites, expanded
//! into every declared unit for measure/ratio on data inputs, and into every unit in the
//! result type's family for rule results. It sits at the crate root (not under
//! `evaluation/`) because `planning::execution_plan::ShowData` needs it too, and planning
//! must not import evaluation. The plan/eval-internal representation is the canonical
//! `planning::semantics::LiteralValue`; this module is the boundary between the two.
//!
//! Magnitudes are [`Decimal`] (28-scale API boundary). JSON still emits decimal strings via
//! `Decimal`'s default serde.

use crate::computation::rational::{
    checked_div, checked_mul, decimal_to_display_str, NumericFailure,
};
use crate::literals::rational_from_parsed_decimal;
use crate::planning::semantics::{
    format_decimal_for_api, range_element_type_specification, ratio_element_type_for_api,
    semantic_calendar_unit_from_measure_type, LemmaType, LiteralUnitMapFailure, LiteralValue,
    SemanticDateTime, SemanticTime, TypeSpecification, UnitFactorSource, ValueKind,
};
use crate::planning::unit_family::{declared_bare_names_only, FamilyUnitCatalog, FamilyUnitEntry};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

/// Calendar value (a measure whose unit is a calendar unit, e.g. `3 months`) on a result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalendarResult {
    pub value: Decimal,
    pub unit: String,
}

/// Both endpoints of a range result.
///
/// Each endpoint is itself a [`RuleResultValue`], but an endpoint's own `range` field is
/// always `None` — a range endpoint must never itself be a range. Building a
/// [`RuleResultValue`] and reconstructing a literal both panic if this invariant is
/// violated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RangeResult {
    pub from: RuleResultValue,
    pub to: RuleResultValue,
}

/// API value shared by flattened [`crate::evaluation::response::RuleResult`],
/// `ShowData.fill` / `suggestion`, and explanation `Data` / `Rule` nodes.
///
/// When present: always `result` (from [`LiteralValue::display_value`]), plus exactly
/// one typed field for a non-range value; `range` is set instead for a range value, and
/// every other typed field stays `None`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RuleResultValue {
    /// Engine-rendered string for UI (`LiteralValue::display_value`). Present whenever
    /// this value is present, including range endpoints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measure: Option<BTreeMap<String, Decimal>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio: Option<BTreeMap<String, Decimal>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<Decimal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boolean: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<SemanticDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<SemanticTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<CalendarResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Box<RangeResult>>,
}

/// Why building a [`RuleResultValue`] from a canonical [`LiteralValue`] failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleResultValueFailure {
    DecimalLimit,
    NumericOverflow,
    OutOfMemory,
}

/// Which units to expand when building a [`RuleResultValue`] from a measure or ratio literal.
pub(crate) enum UnitExpansion<'a> {
    /// Show data fill/suggestion: units declared on `lemma_type` only.
    Declared,
    /// Rule results: precomputed family entry from the plan catalog.
    Family(&'a FamilyUnitEntry),
}

/// Human-readable veto message for a [`RuleResultValueFailure`].
pub fn rule_result_value_failure_message(failure: RuleResultValueFailure) -> &'static str {
    match failure {
        RuleResultValueFailure::DecimalLimit => "Calculated result exceeds decimal value limit",
        RuleResultValueFailure::NumericOverflow => "numeric overflow",
        RuleResultValueFailure::OutOfMemory => "out of memory",
    }
}

fn map_numeric_to_rule_result_value_failure(failure: NumericFailure) -> RuleResultValueFailure {
    match failure {
        NumericFailure::Overflow => RuleResultValueFailure::DecimalLimit,
        NumericFailure::OutOfMemory => RuleResultValueFailure::OutOfMemory,
        NumericFailure::DivisionByZero => {
            panic!(
                "BUG: decimal commit encountered division by zero while building RuleResultValue"
            )
        }
        NumericFailure::Irrational => {
            panic!(
                "BUG: decimal commit encountered irrational result while building RuleResultValue"
            )
        }
    }
}

fn map_unit_conversion_failure(failure: NumericFailure) -> RuleResultValueFailure {
    match failure {
        NumericFailure::Overflow => RuleResultValueFailure::NumericOverflow,
        NumericFailure::OutOfMemory => RuleResultValueFailure::OutOfMemory,
        NumericFailure::DivisionByZero => {
            panic!("BUG: unit conversion division by zero while building RuleResultValue")
        }
        NumericFailure::Irrational => {
            panic!("BUG: unit conversion irrational while building RuleResultValue")
        }
    }
}

fn map_literal_unit_map_failure(failure: LiteralUnitMapFailure) -> RuleResultValueFailure {
    match failure {
        LiteralUnitMapFailure::Commit(failure) => map_numeric_to_rule_result_value_failure(failure),
        LiteralUnitMapFailure::UnitConversion(failure) => map_unit_conversion_failure(failure),
    }
}

fn expansion_for_type<'a>(
    rule_type: &LemmaType,
    catalog: Option<&'a FamilyUnitCatalog>,
) -> UnitExpansion<'a> {
    let Some(catalog) = catalog else {
        return UnitExpansion::Declared;
    };
    match catalog.entry_for_type(rule_type) {
        Some(entry) => UnitExpansion::Family(entry),
        None => UnitExpansion::Declared,
    }
}

fn unit_names_for_expansion(
    result_type: &LemmaType,
    expansion: &UnitExpansion<'_>,
    catalog: Option<&FamilyUnitCatalog>,
) -> Arc<[String]> {
    match expansion {
        UnitExpansion::Declared => Arc::from(declared_bare_names_only(result_type)),
        UnitExpansion::Family(_) => catalog
            .expect("BUG: family expansion requires FamilyUnitCatalog")
            .ordered_bare_names_for_type(result_type),
    }
}

fn measure_factor_source<'a>(
    result_type: &'a LemmaType,
    expansion: &'a UnitExpansion<'a>,
) -> UnitFactorSource<'a> {
    match expansion {
        UnitExpansion::Declared => UnitFactorSource::DeclaredOn(result_type),
        UnitExpansion::Family(entry) => UnitFactorSource::Merged {
            measure: entry.merged_measure_units.as_ref(),
            ratio: entry.merged_ratio_units.as_ref(),
        },
    }
}

fn ratio_factor_source<'a>(
    result_type: &'a LemmaType,
    expansion: &'a UnitExpansion<'a>,
) -> UnitFactorSource<'a> {
    measure_factor_source(result_type, expansion)
}

fn result_value_from_literal(
    literal: &LiteralValue,
    lemma_type: &LemmaType,
    expansion: &UnitExpansion<'_>,
    catalog: Option<&FamilyUnitCatalog>,
) -> Result<RuleResultValue, RuleResultValueFailure> {
    match &literal.value {
        ValueKind::Range(from, to) => {
            let endpoint_ty = range_element_type_specification(&lemma_type.specifications)
                .map(LemmaType::primitive)
                .unwrap_or_else(|| lemma_type.clone());
            let from_value =
                result_value_from_range_endpoint(from, &endpoint_ty, expansion, catalog)?;
            let to_value = result_value_from_range_endpoint(to, &endpoint_ty, expansion, catalog)?;
            Ok(RuleResultValue {
                result: Some(literal.display_value_with_type(lemma_type)),
                range: Some(Box::new(RangeResult {
                    from: from_value,
                    to: to_value,
                })),
                ..RuleResultValue::default()
            })
        }
        _ => result_value_from_non_range_literal(literal, lemma_type, expansion, catalog),
    }
}

/// Build a [`RuleResultValue`] from a canonical [`LiteralValue`] for a rule result.
///
/// Measure and ratio expand into every unit in the result type's family.
pub(crate) fn rule_result_value_from_literal(
    literal: &LiteralValue,
    rule_type: &LemmaType,
    catalog: &FamilyUnitCatalog,
) -> Result<RuleResultValue, RuleResultValueFailure> {
    let expansion = expansion_for_type(rule_type, Some(catalog));
    result_value_from_literal(literal, rule_type, &expansion, Some(catalog))
}

/// Build a [`RuleResultValue`] using only units declared on `lemma_type` (Show data fill/suggestion).
pub fn type_scoped_result_value_from_literal(
    literal: &LiteralValue,
    lemma_type: &LemmaType,
) -> Result<RuleResultValue, RuleResultValueFailure> {
    let expansion = UnitExpansion::Declared;
    result_value_from_literal(literal, lemma_type, &expansion, None)
}

fn result_value_from_range_endpoint(
    endpoint: &LiteralValue,
    endpoint_type: &LemmaType,
    expansion: &UnitExpansion<'_>,
    catalog: Option<&FamilyUnitCatalog>,
) -> Result<RuleResultValue, RuleResultValueFailure> {
    if matches!(&endpoint.value, ValueKind::Range(_, _)) {
        panic!("BUG: range endpoint must not itself be a range");
    }
    result_value_from_non_range_literal(endpoint, endpoint_type, expansion, catalog)
}

fn result_value_from_non_range_literal(
    literal: &LiteralValue,
    result_type: &LemmaType,
    expansion: &UnitExpansion<'_>,
    catalog: Option<&FamilyUnitCatalog>,
) -> Result<RuleResultValue, RuleResultValueFailure> {
    match &literal.value {
        ValueKind::Measure(rational) if result_type.is_calendar_like() => {
            let unit = semantic_calendar_unit_from_measure_type(result_type);
            let value = result_type
                .try_rational_as_decimal(rational)
                .map_err(map_numeric_to_rule_result_value_failure)?;
            let result = Some(literal.display_value_with_type(result_type));
            Ok(RuleResultValue {
                result,
                calendar: Some(CalendarResult {
                    value,
                    unit: unit.to_string(),
                }),
                ..RuleResultValue::default()
            })
        }
        ValueKind::Measure(_) => {
            let result = Some(literal.display_value_with_type(result_type));
            let unit_names = unit_names_for_expansion(result_type, expansion, catalog);
            let unit_name_refs: Vec<&str> = unit_names.iter().map(String::as_str).collect();
            Ok(RuleResultValue {
                result,
                measure: Some(
                    result_type
                        .measure_literal_unit_map(
                            literal,
                            &unit_name_refs,
                            measure_factor_source(result_type, expansion),
                        )
                        .map_err(map_literal_unit_map_failure)?,
                ),
                ..RuleResultValue::default()
            })
        }
        ValueKind::Ratio(_) => {
            let result = Some(literal.display_value_with_type(result_type));
            let unit_names = unit_names_for_expansion(result_type, expansion, catalog);
            let unit_name_refs: Vec<&str> = unit_names.iter().map(String::as_str).collect();
            Ok(RuleResultValue {
                result,
                ratio: Some(
                    result_type
                        .ratio_literal_unit_map(
                            literal,
                            &unit_name_refs,
                            ratio_factor_source(result_type, expansion),
                        )
                        .map_err(map_literal_unit_map_failure)?,
                ),
                ..RuleResultValue::default()
            })
        }
        other => scalar_result_value(result_type, other),
    }
}

fn scalar_result_value(
    result_type: &LemmaType,
    value: &ValueKind,
) -> Result<RuleResultValue, RuleResultValueFailure> {
    match value {
        ValueKind::Number(rational) => {
            let decimal = rational
                .try_to_decimal()
                .map_err(map_numeric_to_rule_result_value_failure)?;
            let api_decimal = format_decimal_for_api(decimal, result_type.decimal_places());
            let display_string = decimal_to_display_str(&decimal);
            Ok(RuleResultValue {
                result: Some(display_string),
                number: Some(api_decimal),
                ..RuleResultValue::default()
            })
        }
        ValueKind::Boolean(b) => {
            let result = Some(value.to_string());
            Ok(RuleResultValue {
                result,
                boolean: Some(*b),
                ..RuleResultValue::default()
            })
        }
        ValueKind::Text(_) | ValueKind::Date(_) | ValueKind::Time(_) => {
            let result = Some(value.to_string());
            Ok(scalar_result_value_non_numeric(result_type, result, value))
        }
        ValueKind::Measure(_) | ValueKind::Ratio(_) => {
            unreachable!("BUG: measure and ratio must be handled by caller")
        }
        ValueKind::Range(_, _) => {
            unreachable!("BUG: range must be handled by result_value_from_literal")
        }
    }
}

fn scalar_result_value_non_numeric(
    _result_type: &LemmaType,
    result: Option<String>,
    value: &ValueKind,
) -> RuleResultValue {
    match value {
        ValueKind::Text(s) => RuleResultValue {
            result,
            text: Some(s.clone()),
            ..RuleResultValue::default()
        },
        ValueKind::Date(d) => RuleResultValue {
            result,
            date: Some(d.clone()),
            ..RuleResultValue::default()
        },
        ValueKind::Time(t) => RuleResultValue {
            result,
            time: Some(t.clone()),
            ..RuleResultValue::default()
        },
        _ => unreachable!("BUG: scalar_result_value_non_numeric called with non-scalar type"),
    }
}

fn literal_from_measure_map(
    measure: &BTreeMap<String, Decimal>,
    rule_type: &LemmaType,
) -> LiteralValue {
    let unit_names = rule_type
        .measure_unit_names()
        .expect("BUG: measure rule result must have declared units");
    let unit_name = unit_names
        .first()
        .expect("BUG: measure rule result type must declare at least one unit");
    let magnitude = measure
        .get(*unit_name)
        .unwrap_or_else(|| panic!("BUG: measure map missing unit '{unit_name}'"));
    let rational = rational_from_parsed_decimal(*magnitude)
        .expect("BUG: measure rule result value must lift to rational");
    let factor = rule_type.measure_unit_factor(unit_name);
    let canonical = checked_mul(&rational, factor).unwrap_or_else(|failure| {
        panic!("BUG: measure canonicalization from RuleResultValue fields failed: {failure}")
    });
    LiteralValue::measure_with_type(canonical, Arc::new(rule_type.clone()))
}

fn literal_from_ratio_map(
    ratio: &BTreeMap<String, Decimal>,
    rule_type: &LemmaType,
) -> LiteralValue {
    let ratio_type = ratio_element_type_for_api(rule_type);
    let units = match &ratio_type.specifications {
        TypeSpecification::Ratio { units, .. } => units,
        _ => panic!(
            "BUG: ratio rule result type must be Ratio, got {}",
            rule_type.name()
        ),
    };
    let unit = units
        .iter()
        .next()
        .expect("BUG: ratio rule result type must declare at least one unit");
    let magnitude = ratio
        .get(&unit.name)
        .unwrap_or_else(|| panic!("BUG: ratio map missing unit '{}'", unit.name));
    let display_rational = rational_from_parsed_decimal(*magnitude)
        .expect("BUG: ratio rule result value must lift to rational");
    let canonical = checked_div(&display_rational, &unit.value).unwrap_or_else(|failure| {
        panic!("BUG: ratio canonicalization from RuleResultValue fields failed: {failure}")
    });
    LiteralValue::ratio_with_type(canonical, Arc::new(rule_type.clone()))
}

impl RuleResultValue {
    /// Reconstruct the [`LiteralValue`] from this API value's fields.
    ///
    /// Panics if the fields cannot reconstruct a literal, or if a range endpoint is
    /// itself a range (ranges do not nest — enforced here, not by convention).
    pub fn to_literal(&self, rule_type: &LemmaType) -> LiteralValue {
        if let Some(range) = &self.range {
            if range.from.range.is_some() || range.to.range.is_some() {
                panic!("BUG: range endpoint must not itself be a range");
            }
            let endpoint_type =
                element_type_from_range_rule(rule_type).unwrap_or_else(|| rule_type.clone());
            let left = range.from.to_literal(&endpoint_type);
            let right = range.to.to_literal(&endpoint_type);
            return LiteralValue::range(left, right);
        }

        if let Some(b) = self.boolean {
            return LiteralValue::from_bool(b);
        }
        let owned_rule_type = Arc::new(rule_type.clone());
        if let Some(number) = self.number {
            return LiteralValue::number_with_type_from_decimal(number, owned_rule_type);
        }
        if let Some(calendar) = &self.calendar {
            let rational = rational_from_parsed_decimal(calendar.value)
                .expect("BUG: calendar rule result value must lift to rational");
            return LiteralValue::measure_with_type(rational, owned_rule_type);
        }
        if let Some(measure) = &self.measure {
            return literal_from_measure_map(measure, rule_type);
        }
        if let Some(ratio) = &self.ratio {
            return literal_from_ratio_map(ratio, rule_type);
        }
        if let Some(date) = &self.date {
            return LiteralValue::date_with_type(date.clone(), owned_rule_type);
        }
        if let Some(time) = &self.time {
            return LiteralValue::time_with_type(time.clone(), owned_rule_type);
        }
        if let Some(text) = &self.text {
            return LiteralValue::text_with_type(text.clone(), owned_rule_type);
        }
        panic!("BUG: rule result value fields cannot reconstruct literal");
    }
}

fn element_type_from_range_rule(rule_type: &LemmaType) -> Option<LemmaType> {
    range_element_type_specification(&rule_type.specifications).map(LemmaType::primitive)
}

fn format_unit_map(map: &BTreeMap<String, Decimal>) -> String {
    map.iter()
        .map(|(unit, value)| format!("{value} {unit}"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl fmt::Display for RuleResultValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(range) = &self.range {
            return write!(f, "{}...{}", range.from, range.to);
        }
        if let Some(measure) = &self.measure {
            return write!(f, "{}", format_unit_map(measure));
        }
        if let Some(ratio) = &self.ratio {
            return write!(f, "{}", format_unit_map(ratio));
        }
        if let Some(number) = &self.number {
            return write!(f, "{number}");
        }
        if let Some(b) = self.boolean {
            return write!(f, "{b}");
        }
        if let Some(text) = &self.text {
            return write!(f, "{text}");
        }
        if let Some(date) = &self.date {
            return write!(f, "{date}");
        }
        if let Some(time) = &self.time {
            return write!(f, "{time}");
        }
        if let Some(calendar) = &self.calendar {
            return write!(f, "{} {}", calendar.value, calendar.unit);
        }
        if let Some(display) = &self.result {
            return write!(f, "{display}");
        }
        write!(f, "")
    }
}
