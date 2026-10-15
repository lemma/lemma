//! API value for a rule result, `ShowData.value`, or `ShowData.suggestion`.
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
    SemanticDateTime, SemanticTime, SpecInstance, SpecValue, TypeSpecification, TypedLiteral,
    UnitFactorSource, ValueKind,
};
use crate::planning::unit_family::{declared_bare_names_only, FamilyUnitCatalog, FamilyUnitEntry};
use indexmap::IndexMap;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
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
/// `ShowData.value` / `suggestion`, and explanation `Data` / `Rule` nodes.
///
/// When present: `result` (from [`LiteralValue::display_value`]), plus exactly
/// one typed field for a non-range value; `range` is set instead for a range value, and
/// every other typed field stays `None`. A list row carries `row` in rule results and
/// in [`RowResult`] members; in explanation nodes it carries `result` only. The one
/// value without `result` is an instance list in Show: the list carries `list` only and
/// each source row `fields` only, in row order.
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
    /// Written unit for a measure or ratio value (including range endpoints).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Box<RangeResult>>,
    /// Set when the value is a spec instance. `data` holds one evaluated entry per
    /// data slot of that instance, keyed relative to that spec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<Box<SpecResult>>,
    /// Set when the value is a row of an instance list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row: Option<RowReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<Vec<RuleResultValue>>,
    /// Set on a source row of an instance list in Show: the fields the row sets,
    /// keyed relative to the row spec, in the order the row writes them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<IndexMap<String, RuleResultValue>>,
    /// Show only: on a source-row field whose value is a reference; the input key
    /// of the referenced data in the spec declaring the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// A spec instance returned by a rule, including the values needed to run it again.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpecResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    pub spec: String,
    /// Resolved row's effective date. Empty when the row is the origin row.
    pub effective: String,
    /// `uses` path of this instance in the consumer (`basic`, `basic.finance`).
    pub instance: String,
    /// One segment per `uses` hop of `instance`.
    pub path: Vec<crate::api::PathSegment>,
    /// Evaluated data slots, keyed relative to `spec`, declaration order. A slot
    /// with a value carries that value; a vetoed slot carries `vetoed` and
    /// `veto_reason`. Unbound slots are omitted.
    pub data: IndexMap<String, SpecRuleResult>,
    /// Rule results relative to `spec` (`tax`, `finance.fee`), plan rule order.
    pub rules: IndexMap<String, SpecRuleResult>,
}

/// A row of an instance list. Its data and rules are `Response.rows[index]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RowReference {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    pub spec: String,
    /// Resolved row's effective date. Empty when the row is the origin row.
    pub effective: String,
    /// Row path relative to the spec holding this value (`lines.2`, `components.1`).
    pub instance: String,
    pub index: usize,
}

/// Data and rules of one distinct row reached by a response, filled once.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RowResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    pub spec: String,
    /// Resolved row's effective date. Empty when the row is the origin row.
    pub effective: String,
    /// Evaluated data slots, keyed relative to `spec`, declaration order.
    /// Unbound slots are omitted.
    pub data: IndexMap<String, SpecRuleResult>,
    /// Rule results relative to `spec`, plan rule order.
    pub rules: IndexMap<String, SpecRuleResult>,
}

/// One rule of a [`SpecResult`], or one evaluated data slot of a [`SpecResult`].
/// A spec-valued rule nests another [`SpecResult`]
/// in [`RuleResultValue::spec`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpecRuleResult {
    pub vetoed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub veto_reason: Option<String>,
    pub rule_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<RuleResultValue>,
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
        RuleResultValueFailure::DecimalLimit => {
            "The calculated result exceeds the decimal value limit."
        }
        RuleResultValueFailure::NumericOverflow => "The numeric result overflowed.",
        RuleResultValueFailure::OutOfMemory => "The calculation ran out of memory.",
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
        NumericFailure::MagnitudeLimit => {
            panic!("BUG: decimal commit does not apply the arithmetic magnitude limit")
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
        NumericFailure::MagnitudeLimit => {
            panic!("BUG: unit conversion does not apply the arithmetic magnitude limit")
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

/// Type of a node in a value tree walked with an explicit stack: the root type is borrowed,
/// a list row's type is shared with the row's [`TypedLiteral`].
enum NodeType<'a> {
    Borrowed(&'a LemmaType),
    Shared(Arc<LemmaType>),
}

impl NodeType<'_> {
    fn get(&self) -> &LemmaType {
        match self {
            Self::Borrowed(lemma_type) => lemma_type,
            Self::Shared(lemma_type) => lemma_type.as_ref(),
        }
    }
}

enum ResultValueTask<'a> {
    Visit {
        literal: Cow<'a, LiteralValue>,
        lemma_type: NodeType<'a>,
    },
    AssembleList {
        display: String,
        rows: usize,
    },
}

fn result_value_from_literal(
    literal: &LiteralValue,
    lemma_type: &LemmaType,
    expansion: &UnitExpansion<'_>,
    catalog: Option<&FamilyUnitCatalog>,
) -> Result<RuleResultValue, RuleResultValueFailure> {
    let mut tasks = vec![ResultValueTask::Visit {
        literal: Cow::Borrowed(literal),
        lemma_type: NodeType::Borrowed(lemma_type),
    }];
    let mut converted: Vec<RuleResultValue> = Vec::new();
    while let Some(task) = tasks.pop() {
        match task {
            ResultValueTask::Visit {
                literal,
                lemma_type,
            } => {
                let lemma_type = lemma_type.get();
                match &literal.value {
                    ValueKind::Range(from, to) => {
                        let from_value = result_value_from_range_endpoint(
                            &from.to_literal(),
                            from.lemma_type.as_ref(),
                            expansion,
                            catalog,
                        )?;
                        let to_value = result_value_from_range_endpoint(
                            &to.to_literal(),
                            to.lemma_type.as_ref(),
                            expansion,
                            catalog,
                        )?;
                        converted.push(RuleResultValue {
                            result: Some(literal.display_value_with_type(lemma_type)),
                            range: Some(Box::new(RangeResult {
                                from: from_value,
                                to: to_value,
                            })),
                            ..RuleResultValue::default()
                        });
                    }
                    ValueKind::List(rows) => {
                        tasks.push(ResultValueTask::AssembleList {
                            display: literal.display_value_with_type(lemma_type),
                            rows: rows.len(),
                        });
                        for row in rows.iter().rev() {
                            tasks.push(ResultValueTask::Visit {
                                literal: Cow::Owned(row.to_literal()),
                                lemma_type: NodeType::Shared(Arc::clone(&row.lemma_type)),
                            });
                        }
                    }
                    _ => converted.push(result_value_from_non_range_literal(
                        &literal, lemma_type, expansion, catalog,
                    )?),
                }
            }
            ResultValueTask::AssembleList { display, rows } => {
                let list = converted.split_off(converted.len() - rows);
                converted.push(RuleResultValue {
                    result: Some(display),
                    list: Some(list),
                    ..RuleResultValue::default()
                });
            }
        }
    }
    Ok(converted
        .pop()
        .expect("BUG: literal conversion yields exactly one RuleResultValue"))
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
                unit: result_type.unit().map(str::to_string),
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
                unit: result_type.unit().map(str::to_string),
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
            Ok(scalar_result_value_non_numeric(result, value))
        }
        ValueKind::Measure(_) | ValueKind::Ratio(_) => {
            unreachable!("BUG: measure and ratio must be handled by caller")
        }
        ValueKind::Range(_, _) => {
            unreachable!("BUG: range must be handled by result_value_from_literal")
        }
        ValueKind::List(_) => {
            unreachable!("BUG: list must be handled by result_value_from_literal")
        }
        ValueKind::Spec(SpecValue::Fixed(instance)) => {
            let identity = result_type.spec_identity().unwrap_or_else(|| {
                panic!(
                    "BUG: spec value whose type is not a spec ({})",
                    result_type.name()
                )
            });
            Ok(RuleResultValue {
                result: Some(instance.to_string()),
                spec: Some(Box::new(SpecResult {
                    repository: identity.repository.clone(),
                    spec: identity.spec.clone(),
                    effective: identity.effective.to_string(),
                    instance: instance.to_string(),
                    path: instance
                        .prefix
                        .iter()
                        .map(crate::api::PathSegment::from)
                        .collect(),
                    data: IndexMap::new(),
                    rules: IndexMap::new(),
                })),
                ..RuleResultValue::default()
            })
        }
        ValueKind::Spec(SpecValue::Row(row)) => Ok(RuleResultValue {
            result: Some(row.origin.to_string()),
            ..RuleResultValue::default()
        }),
    }
}

fn scalar_result_value_non_numeric(result: Option<String>, value: &ValueKind) -> RuleResultValue {
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
    let unit_name = rule_type
        .unit()
        .map(str::to_string)
        .or_else(|| unit_names.first().map(|name| (*name).to_string()))
        .expect("BUG: measure rule result type must declare at least one unit");
    let magnitude = measure
        .get(&unit_name)
        .unwrap_or_else(|| panic!("BUG: measure map missing unit '{unit_name}'"));
    let rational = rational_from_parsed_decimal(*magnitude)
        .expect("BUG: measure rule result value must lift to rational");
    let factor = rule_type.measure_unit_factor(&unit_name);
    let canonical = checked_mul(&rational, factor).unwrap_or_else(|failure| {
        panic!("BUG: measure canonicalization from RuleResultValue fields failed: {failure}")
    });
    LiteralValue::measure(canonical)
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
    let unit = rule_type
        .unit()
        .and_then(|name| units.get(name).ok())
        .or_else(|| units.iter().next())
        .expect("BUG: ratio rule result type must declare at least one unit");
    let magnitude = ratio
        .get(&unit.name)
        .unwrap_or_else(|| panic!("BUG: ratio map missing unit '{}'", unit.name));
    let display_rational = rational_from_parsed_decimal(*magnitude)
        .expect("BUG: ratio rule result value must lift to rational");
    let canonical = checked_div(&display_rational, &unit.value).unwrap_or_else(|failure| {
        panic!("BUG: ratio canonicalization from RuleResultValue fields failed: {failure}")
    });
    LiteralValue::ratio(canonical)
}

impl RuleResultValue {
    /// Reconstruct the [`LiteralValue`] from this API value's fields.
    ///
    /// Panics if the fields cannot reconstruct a literal, or if a range endpoint is
    /// itself a range (ranges do not nest; enforced here, not by convention).
    /// A list row is not rebuilt: its value is its bindings, which the API value does not
    /// carry. Read a row through its fields instead.
    pub fn to_literal(&self, rule_type: &LemmaType) -> LiteralValue {
        if let Some(row) = &self.row {
            panic!(
                "BUG: list row '{}' cannot be rebuilt from its API value",
                row.instance
            );
        }
        if let Some(spec) = &self.spec {
            assert!(
                !spec.path.is_empty(),
                "BUG: spec result '{}' has no uses hop",
                spec.instance
            );
            return LiteralValue::new(ValueKind::Spec(SpecValue::Fixed(SpecInstance {
                prefix: spec
                    .path
                    .iter()
                    .map(|segment| crate::planning::semantics::PathSegment {
                        uses: segment.uses.clone(),
                        repository: segment.repository.clone(),
                        spec: segment.spec.clone(),
                    })
                    .collect(),
            })));
        }
        if let Some(rows) = &self.list {
            let TypeSpecification::List { element, .. } = &rule_type.specifications else {
                panic!(
                    "BUG: list result whose rule type is not a list ({})",
                    rule_type.name()
                );
            };
            return LiteralValue::new(ValueKind::List(
                rows.iter()
                    .map(|row| {
                        let row_type =
                            endpoint_type_with_written_unit(element, row.unit.as_deref());
                        TypedLiteral {
                            value: row.to_literal(&row_type).value,
                            lemma_type: Arc::new(row_type),
                        }
                    })
                    .collect(),
            ));
        }
        if let Some(range) = &self.range {
            if range.from.range.is_some() || range.to.range.is_some() {
                panic!("BUG: range endpoint must not itself be a range");
            }
            let endpoint_schema =
                element_type_from_range_rule(rule_type).unwrap_or_else(|| rule_type.clone());
            let left_type =
                endpoint_type_with_written_unit(&endpoint_schema, range.from.unit.as_deref());
            let right_type =
                endpoint_type_with_written_unit(&endpoint_schema, range.to.unit.as_deref());
            let left = range.from.to_literal(&left_type);
            let right = range.to.to_literal(&right_type);
            return LiteralValue::range(
                crate::planning::semantics::TypedLiteral {
                    value: left.value,
                    lemma_type: Arc::new(left_type),
                },
                crate::planning::semantics::TypedLiteral {
                    value: right.value,
                    lemma_type: Arc::new(right_type),
                },
            );
        }

        if let Some(b) = self.boolean {
            return LiteralValue::from_bool(b);
        }
        let owned_rule_type = match self.unit.as_deref() {
            Some(unit)
                if matches!(
                    &rule_type.specifications,
                    TypeSpecification::Measure { .. } | TypeSpecification::Ratio { .. }
                ) && rule_type.unit() != Some(unit) =>
            {
                Arc::new(rule_type.clone().with_unit(unit))
            }
            _ => Arc::new(rule_type.clone()),
        };
        if let Some(number) = self.number {
            return LiteralValue::number_from_decimal(number);
        }
        if let Some(calendar) = &self.calendar {
            let rational = rational_from_parsed_decimal(calendar.value)
                .expect("BUG: calendar rule result value must lift to rational");
            return LiteralValue::measure(rational);
        }
        if let Some(measure) = &self.measure {
            return literal_from_measure_map(measure, owned_rule_type.as_ref());
        }
        if let Some(ratio) = &self.ratio {
            return literal_from_ratio_map(ratio, owned_rule_type.as_ref());
        }
        if let Some(date) = &self.date {
            return LiteralValue::date(date.clone());
        }
        if let Some(time) = &self.time {
            return LiteralValue::time(time.clone());
        }
        if let Some(text) = &self.text {
            return LiteralValue::text(text.clone());
        }
        panic!("BUG: rule result value fields cannot reconstruct literal");
    }
}

fn endpoint_type_with_written_unit(endpoint_schema: &LemmaType, unit: Option<&str>) -> LemmaType {
    match unit {
        Some(unit)
            if matches!(
                &endpoint_schema.specifications,
                TypeSpecification::Measure { .. } | TypeSpecification::Ratio { .. }
            ) && endpoint_schema.unit() != Some(unit) =>
        {
            endpoint_schema.clone().with_unit(unit)
        }
        _ => endpoint_schema.clone(),
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
        if let Some(reference) = &self.reference {
            return write!(f, "{reference}");
        }
        write!(f, "")
    }
}
