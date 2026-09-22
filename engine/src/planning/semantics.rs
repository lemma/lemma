//! Resolved semantic types for Lemma
//!
//! This module contains all types that represent resolved semantics after planning.
//! These types are created during the planning phase and used by evaluation, inversion, etc.

// Re-exported parsing types: downstream modules (evaluation, inversion, computation,
// serialization) import these from `planning::semantics`, never from `parsing` directly.
pub use crate::parsing::ast::{
    ArithmeticComputation, ComparisonComputation, MathematicalComputation, NegationType,
    VetoExpression,
};
pub use crate::parsing::source::Source;

/// Logical computation operators (defined in semantics, not used by the parser).
/// Returns the logical negation of a comparison (used by De Morgan / NOT-normal-form rewriting).
#[must_use]
pub fn negated_comparison(op: ComparisonComputation) -> ComparisonComputation {
    match op {
        ComparisonComputation::LessThan => ComparisonComputation::GreaterThanOrEqual,
        ComparisonComputation::LessThanOrEqual => ComparisonComputation::GreaterThan,
        ComparisonComputation::GreaterThan => ComparisonComputation::LessThanOrEqual,
        ComparisonComputation::GreaterThanOrEqual => ComparisonComputation::LessThan,
        ComparisonComputation::Is => ComparisonComputation::IsNot,
        ComparisonComputation::IsNot => ComparisonComputation::Is,
    }
}

/// Returns the operator that states the same fact with the operands swapped:
/// `k < x` and `x > k` hold on exactly the same values.
#[must_use]
pub fn mirrored_comparison(op: ComparisonComputation) -> ComparisonComputation {
    match op {
        ComparisonComputation::LessThan => ComparisonComputation::GreaterThan,
        ComparisonComputation::LessThanOrEqual => ComparisonComputation::GreaterThanOrEqual,
        ComparisonComputation::GreaterThan => ComparisonComputation::LessThan,
        ComparisonComputation::GreaterThanOrEqual => ComparisonComputation::LessThanOrEqual,
        ComparisonComputation::Is => ComparisonComputation::Is,
        ComparisonComputation::IsNot => ComparisonComputation::IsNot,
    }
}

// Internal-only parsing imports (used only within this module for value/type resolution).
use crate::computation::rational::{checked_div, rational_new, RationalInteger};
use crate::parsing::ast::Constraint;
use crate::parsing::ast::{
    BooleanValue, CalendarPeriodUnit, CommandArg, ConversionTarget, DateCalendarKind,
    DateRelativeKind, DateTimeValue, PrimitiveKind, TimeValue, TimezoneValue,
    TypeConstraintCommand,
};
use crate::Error;
use indexmap::IndexMap;
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;
use std::hash::Hash;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};

// -----------------------------------------------------------------------------
// Type specification and units (resolved type shape; apply constraints is planning)
// -----------------------------------------------------------------------------

// Unit tables live in `crate::literals` (no dependency on parsing/ast). Re-exported
// here so downstream modules importing from `planning::semantics` keep working.
pub use crate::literals::{BaseMeasureVector, MeasureUnit, MeasureUnits, RatioUnit, RatioUnits};

/// Combine two `BaseMeasureVector`s by adding (for multiply) or subtracting (for divide) exponents.
/// Entries that reach zero exponent are removed (they cancel out).
pub fn combine_decompositions(
    left: &BaseMeasureVector,
    right: &BaseMeasureVector,
    is_multiply: bool,
) -> BaseMeasureVector {
    let mut result = left.clone();
    for (dim, &exp) in right {
        let delta = if is_multiply { exp } else { -exp };
        let entry = result.entry(dim.clone()).or_insert(0);
        *entry += delta;
        if *entry == 0 {
            result.remove(dim);
        }
    }
    result
}

/// Combine two symbolic unit signatures (sorted-by-unit-name, no-zero-exponent vectors)
/// under multiplication or division. The result is in canonical form: sorted by unit name
/// ascending, no zero exponents.
///
/// Planning resolves measure products via decompositions; this remains the pinned algebra
/// for signature-index tests.
#[cfg(test)]
pub fn combine_signatures(
    left: &[(String, i32)],
    right: &[(String, i32)],
    is_multiply: bool,
) -> Vec<(String, i32)> {
    use std::collections::BTreeMap;
    let mut accumulator: BTreeMap<String, i32> = BTreeMap::new();
    for (name, exponent) in left {
        *accumulator.entry(name.clone()).or_insert(0) += exponent;
    }
    for (name, exponent) in right {
        let delta = if is_multiply { *exponent } else { -*exponent };
        *accumulator.entry(name.clone()).or_insert(0) += delta;
    }
    accumulator
        .into_iter()
        .filter(|(_, exponent)| *exponent != 0)
        .collect()
}

/// Canonicalize a unit signature: sum duplicate entries by name, drop zero exponents,
/// sort ascending by unit name. Idempotent.
/// Format a canonical unit signature into human-readable operator style.
///
/// Rules:
/// - Numerator units (positive exponents) sorted alphabetically, joined by `*`.
/// - Denominator units (negative exponents) sorted alphabetically, joined by `*`, exponents shown
///   as positive values.
/// - Separated by `/`. Empty numerator: `1/<denominator>`. No denominator: numerator only.
/// - Exponents > 1 suffixed as `^n`.
///
/// Examples: `eur/hour`, `kilogram*meter^2/second^2`, `1/meter`.
pub fn format_signature_operator_style(signature: &[(String, i32)]) -> String {
    let canonical = canonicalize_signature(signature);
    let mut numerator: Vec<(String, i32)> = Vec::new();
    let mut denominator: Vec<(String, i32)> = Vec::new();
    for (name, exponent) in canonical {
        if exponent > 0 {
            numerator.push((name, exponent));
        } else if exponent < 0 {
            denominator.push((name, -exponent));
        }
    }
    let render = |terms: &[(String, i32)]| -> String {
        terms
            .iter()
            .map(|(name, exp)| {
                if *exp == 1 {
                    name.clone()
                } else {
                    format!("{name}^{exp}")
                }
            })
            .collect::<Vec<_>>()
            .join("*")
    };
    match (numerator.is_empty(), denominator.is_empty()) {
        (true, true) => String::new(),
        (false, true) => render(&numerator),
        (true, false) => format!("1/{}", render(&denominator)),
        (false, false) => format!("{}/{}", render(&numerator), render(&denominator)),
    }
}

/// Returns the intra-calendar-dimension factor for `name`, if it is a known calendar unit.
///
/// Month is canonical (factor 1). Year = 12.
/// Keys are **singular** (`"month"`, `"year"`).
///
/// Returns `None` for names that are not calendar units.
pub fn calendar_unit_factor(name: &str) -> Option<crate::computation::rational::RationalInteger> {
    use crate::computation::rational::rational_one;
    match name {
        "month" => Some(rational_one()),
        "year" => Some(rational_new(12, 1)),
        _ => None,
    }
}

fn reject_negative_width_magnitude(magnitude: &RationalInteger, cmd: &str) -> Result<(), String> {
    use crate::computation::rational::rational_zero;
    use std::cmp::Ordering;
    match magnitude.try_cmp(&rational_zero()) {
        Ok(Ordering::Less) => Err(format!("{cmd} width must not be negative")),
        Ok(Ordering::Equal | Ordering::Greater) => Ok(()),
        Err(failure) => Err(format!("{cmd} width compare failed: {failure}")),
    }
}

/// Store a width bound as declared `(magnitude, unit)`. Family and factors are resolved
/// later against `unit_index` during planning validation.
fn parse_unresolved_width_bound(
    args: &[CommandArg],
    cmd: &str,
) -> Result<(RationalInteger, String), String> {
    use crate::computation::rational::decimal_to_rational;
    let lit = require_literal(args, cmd)?;
    let (magnitude, unit_name) = match lit {
        crate::literals::Value::NumberWithUnit(n, unit) => (*n, unit.clone()),
        other => {
            return Err(format!(
                "{cmd} requires a measure literal with a unit, got {}",
                value_kind_name(other)
            ));
        }
    };
    let magnitude_rational = decimal_to_rational(magnitude)
        .map_err(|failure| format!("{cmd} literal failed rational lift: {failure}"))?;
    reject_negative_width_magnitude(&magnitude_rational, cmd)?;
    Ok((magnitude_rational, unit_name))
}

/// Planning consistency for range endpoint and width bound declarations.
///
/// `unit_index` resolves date/time width units (duration vs calendar). Pass empty for
/// callers that only check number/ratio/measure ranges.
pub(crate) fn check_range_bound_consistency(
    spec: &TypeSpecification,
    unit_index: &crate::planning::unit_index::UnitIndex,
) -> Result<(), String> {
    use std::cmp::Ordering;

    fn endpoint_order_ok_dates(lo: &DateTimeValue, hi: &DateTimeValue) -> bool {
        compare_semantic_dates(&date_time_to_semantic(lo), &date_time_to_semantic(hi))
            != Ordering::Greater
    }
    fn endpoint_order_ok_times(lo: &TimeValue, hi: &TimeValue) -> bool {
        compare_semantic_times(&time_to_semantic(lo), &time_to_semantic(hi)) != Ordering::Greater
    }

    match spec {
        TypeSpecification::NumberRange {
            lower,
            upper,
            minimum,
            maximum,
            ..
        }
        | TypeSpecification::RatioRange {
            lower,
            upper,
            minimum,
            maximum,
            ..
        } => {
            if let (Some(lo), Some(hi)) = (lower, upper) {
                if lo
                    .try_cmp(hi)
                    .map_err(|failure| format!("range endpoint compare failed: {failure}"))?
                    == Ordering::Greater
                {
                    return Err(format!(
                        "invalid range: lower {} is greater than upper {}",
                        lo.display_str(),
                        hi.display_str()
                    ));
                }
            }
            if let (Some(min_w), Some(max_w)) = (minimum, maximum) {
                if min_w
                    .try_cmp(max_w)
                    .map_err(|failure| format!("range width compare failed: {failure}"))?
                    == Ordering::Greater
                {
                    return Err(format!(
                        "invalid range: minimum width {} is greater than maximum width {}",
                        min_w.display_str(),
                        max_w.display_str()
                    ));
                }
            }
            Ok(())
        }
        TypeSpecification::MeasureRange {
            lower,
            upper,
            minimum,
            maximum,
            units,
            ..
        } => {
            if let (Some(lo), Some(hi)) = (lower, upper) {
                let lo_c =
                    measure_declared_bound_to_canonical(&lo.0, &lo.1, units, "range", "lower")?;
                let hi_c =
                    measure_declared_bound_to_canonical(&hi.0, &hi.1, units, "range", "upper")?;
                if lo_c
                    .try_cmp(&hi_c)
                    .map_err(|failure| format!("range endpoint compare failed: {failure}"))?
                    == Ordering::Greater
                {
                    return Err(format!(
                        "invalid range: lower {} {} is greater than upper {} {}",
                        lo.0.display_str(),
                        lo.1,
                        hi.0.display_str(),
                        hi.1
                    ));
                }
            }
            if let (Some(min_w), Some(max_w)) = (minimum, maximum) {
                let min_c = measure_declared_bound_to_canonical(
                    &min_w.0, &min_w.1, units, "range", "minimum",
                )?;
                let max_c = measure_declared_bound_to_canonical(
                    &max_w.0, &max_w.1, units, "range", "maximum",
                )?;
                if min_c
                    .try_cmp(&max_c)
                    .map_err(|failure| format!("range width compare failed: {failure}"))?
                    == Ordering::Greater
                {
                    return Err(format!(
                        "invalid range: minimum width {} {} is greater than maximum width {} {}",
                        min_w.0.display_str(),
                        min_w.1,
                        max_w.0.display_str(),
                        max_w.1
                    ));
                }
            }
            Ok(())
        }
        TypeSpecification::DateRange {
            lower,
            upper,
            minimum,
            maximum,
            ..
        } => {
            if let (Some(lo), Some(hi)) = (lower, upper) {
                if !endpoint_order_ok_dates(lo, hi) {
                    return Err(format!(
                        "invalid range: lower {lo} is greater than upper {hi}"
                    ));
                }
            }
            check_temporal_width_pair_consistency(minimum, maximum, unit_index, true)
        }
        TypeSpecification::TimeRange {
            lower,
            upper,
            minimum,
            maximum,
            ..
        } => {
            if let (Some(lo), Some(hi)) = (lower, upper) {
                if !endpoint_order_ok_times(lo, hi) {
                    return Err(format!(
                        "invalid range: lower {lo} is greater than upper {hi}"
                    ));
                }
            }
            check_temporal_width_pair_consistency(minimum, maximum, unit_index, false)
        }
        _ => Ok(()),
    }
}

fn check_temporal_width_pair_consistency(
    minimum: &Option<(RationalInteger, String)>,
    maximum: &Option<(RationalInteger, String)>,
    unit_index: &crate::planning::unit_index::UnitIndex,
    allow_calendar: bool,
) -> Result<(), String> {
    use std::cmp::Ordering;
    let resolve = |bound: &(RationalInteger, String),
                   command: &str|
     -> Result<(RationalInteger, Arc<LemmaType>), String> {
        let (bare, owner) = unit_index.resolve(bound.1.as_str()).map_err(|err| {
            format!(
                "{command} width unit '{}': {err} (add `uses lemma units` or declare the unit)",
                bound.1
            )
        })?;
        if allow_calendar {
            if !owner.is_duration_like() && !owner.is_calendar_like() {
                return Err(format!(
                    "{command} width unit '{bare}' must be a duration or calendar unit",
                ));
            }
        } else if !owner.is_duration_like() {
            return Err(format!(
                "{command} width unit '{bare}' must be a duration unit",
            ));
        }
        let TypeSpecification::Measure { units, .. } = &owner.specifications else {
            return Err(format!(
                "{command} width unit '{bare}' must resolve to a measure type",
            ));
        };
        let canonical = measure_declared_bound_to_canonical(
            &bound.0,
            &bare,
            units,
            owner.name().as_str(),
            command,
        )?;
        Ok((canonical, Arc::clone(&owner)))
    };

    match (minimum, maximum) {
        (None, None) => Ok(()),
        (Some(min_w), None) => {
            let _ = resolve(min_w, "minimum")?;
            Ok(())
        }
        (None, Some(max_w)) => {
            let _ = resolve(max_w, "maximum")?;
            Ok(())
        }
        (Some(min_w), Some(max_w)) => {
            let (min_c, min_owner) = resolve(min_w, "minimum")?;
            let (max_c, max_owner) = resolve(max_w, "maximum")?;
            if min_owner.is_calendar_like() != max_owner.is_calendar_like() {
                return Err(
                    "invalid range: minimum and maximum width must not mix calendar and duration units"
                        .to_string(),
                );
            }
            if min_c
                .try_cmp(&max_c)
                .map_err(|failure| format!("range width compare failed: {failure}"))?
                == Ordering::Greater
            {
                return Err(format!(
                    "invalid range: minimum width {} {} is greater than maximum width {} {}",
                    min_w.0.display_str(),
                    min_w.1,
                    max_w.0.display_str(),
                    max_w.1
                ));
            }
            Ok(())
        }
    }
}

fn owner_declares_measure_unit(owner: &LemmaType, unit_name: &str) -> bool {
    owner
        .measure_unit_names()
        .is_some_and(|names| names.contains(&unit_name))
}

/// Compute the numeric factor of a symbolic unit signature relative to canonical bases.
///
/// For each `(unit_name, exponent)` in `signature`:
/// 1. When `owner` declares the unit, use `owner.measure_unit_factor(unit_name)`.
/// 2. Fall back to `expression_units[unit_name].measure_unit_factor(unit_name)`.
/// 3. Unknown names panic with `"BUG: signature_factor called with unresolved unit name"`.
///    Ambiguous multi-owner names panic asking the caller to pass a declaring owner.
///
/// Returns the product of `factor^exponent` over all pairs, or `NumericFailure` on overflow.
pub fn signature_factor(
    signature: &[(String, i32)],
    expression_units: &crate::planning::unit_index::UnitIndex,
    owner: Option<&LemmaType>,
) -> Result<
    crate::computation::rational::RationalInteger,
    crate::computation::rational::NumericFailure,
> {
    use crate::computation::rational::{checked_div, checked_mul, rational_one};
    let mut acc = rational_one();
    for (name, exponent) in signature {
        let factor =
            if let Some(owner) = owner.filter(|owner| owner_declares_measure_unit(owner, name)) {
                owner.measure_unit_factor(name).clone()
            } else if let Some(lemma_type) = expression_units.unique_owner(name) {
                lemma_type.measure_unit_factor(name).clone()
            } else if !expression_units.owners_for(name).is_empty() {
                panic!(
                "BUG: signature_factor called with ambiguous unit name '{}' (pass declaring owner)",
                name
            );
            } else {
                panic!(
                    "BUG: signature_factor called with unresolved unit name '{}'",
                    name
                );
            };
        let mut term = rational_one();
        let abs_exp = exponent.unsigned_abs();
        for _ in 0..abs_exp {
            term = checked_mul(&term, &factor)?;
        }
        if *exponent >= 0 {
            acc = checked_mul(&acc, &term)?;
        } else {
            acc = checked_div(&acc, &term)?;
        }
    }
    Ok(acc)
}

pub fn canonicalize_signature(signature: &[(String, i32)]) -> Vec<(String, i32)> {
    use std::collections::BTreeMap;
    let mut accumulator: BTreeMap<String, i32> = BTreeMap::new();
    for (name, exponent) in signature {
        *accumulator.entry(name.clone()).or_insert(0) += exponent;
    }
    accumulator
        .into_iter()
        .filter(|(_, exponent)| *exponent != 0)
        .collect()
}

/// Convert a `BaseMeasureVector` decomposition into canonical signature form.
pub fn base_measure_vector_as_signature(decomposition: &BaseMeasureVector) -> Vec<(String, i32)> {
    decomposition
        .iter()
        .filter(|(_, exponent)| **exponent != 0)
        .map(|(name, exponent)| (name.clone(), *exponent))
        .collect()
}

pub const DURATION_DIMENSION: &str = "duration";
pub const CALENDAR_DIMENSION: &str = "calendar";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasureTrait {
    Duration,
    Calendar,
}

pub fn duration_decomposition() -> BaseMeasureVector {
    [(DURATION_DIMENSION.to_string(), 1i32)]
        .into_iter()
        .collect()
}

pub fn calendar_decomposition() -> BaseMeasureVector {
    [(CALENDAR_DIMENSION.to_string(), 1i32)]
        .into_iter()
        .collect()
}

/// Marker `LemmaType` for a Measure value whose signature has not been resolved to
/// a named measure type. Carries an empty decomposition; runtime signature is
/// derived via [`LemmaType::measure_runtime_signature`].
pub fn anonymous_measure_type() -> LemmaType {
    LemmaType::anonymous_for_decomposition(BaseMeasureVector::new())
}

/// Return a copy of `signature` with every exponent negated. Used by Number/Measure
/// reciprocal construction (`1 / Q`).
pub fn negate_signature(signature: &[(String, i32)]) -> Vec<(String, i32)> {
    signature.iter().map(|(n, e)| (n.clone(), -*e)).collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TypeSpecification {
    Boolean {
        help: String,
    },
    Measure {
        minimum: Option<(RationalInteger, String)>,
        maximum: Option<(RationalInteger, String)>,
        decimals: Option<u8>,
        units: MeasureUnits,
        traits: Vec<MeasureTrait>,
        /// Common dimensional decomposition vector shared by all units in this measure.
        /// `None` until the decomposition pass runs. Base measures (no compound unit expression)
        /// are assigned `Some({measure_name: 1})` by the pass. `Some(empty_map)` means resolved
        /// to dimensionless (e.g. `kg/kg`).
        decomposition: Option<BaseMeasureVector>,
        help: String,
    },
    Number {
        minimum: Option<RationalInteger>,
        maximum: Option<RationalInteger>,
        decimals: Option<u8>,
        help: String,
    },
    NumberRange {
        lower: Option<RationalInteger>,
        upper: Option<RationalInteger>,
        minimum: Option<RationalInteger>,
        maximum: Option<RationalInteger>,
        help: String,
    },
    Ratio {
        minimum: Option<RationalInteger>,
        maximum: Option<RationalInteger>,
        decimals: Option<u8>,
        units: RatioUnits,
        help: String,
    },
    RatioRange {
        lower: Option<RationalInteger>,
        upper: Option<RationalInteger>,
        minimum: Option<RationalInteger>,
        maximum: Option<RationalInteger>,
        units: RatioUnits,
        help: String,
    },
    Text {
        length: Option<usize>,
        options: Vec<String>,
        help: String,
    },
    Date {
        minimum: Option<DateTimeValue>,
        maximum: Option<DateTimeValue>,
        help: String,
    },
    DateRange {
        lower: Option<DateTimeValue>,
        upper: Option<DateTimeValue>,
        minimum: Option<(RationalInteger, String)>,
        maximum: Option<(RationalInteger, String)>,
        help: String,
    },
    Time {
        minimum: Option<TimeValue>,
        maximum: Option<TimeValue>,
        help: String,
    },
    TimeRange {
        lower: Option<TimeValue>,
        upper: Option<TimeValue>,
        minimum: Option<(RationalInteger, String)>,
        maximum: Option<(RationalInteger, String)>,
        help: String,
    },
    MeasureRange {
        lower: Option<(RationalInteger, String)>,
        upper: Option<(RationalInteger, String)>,
        minimum: Option<(RationalInteger, String)>,
        maximum: Option<(RationalInteger, String)>,
        units: MeasureUnits,
        decomposition: Option<BaseMeasureVector>,
        help: String,
    },
    Veto {
        message: Option<String>,
    },
    /// Sentinel used during type inference when the type could not be determined.
    /// Propagates through expressions without generating cascading errors.
    /// Must never appear in a successfully validated graph or execution plan.
    Undetermined,
}

impl std::fmt::Display for TypeSpecification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::Boolean { .. } => "boolean",
            Self::Measure { .. } => "measure",
            Self::MeasureRange { .. } => "measure range",
            Self::Number { .. } => "number",
            Self::NumberRange { .. } => "number range",
            Self::Text { .. } => "text",
            Self::Date { .. } => "date",
            Self::DateRange { .. } => "date range",
            Self::Time { .. } => "time",
            Self::TimeRange { .. } => "time range",
            Self::Ratio { .. } => "ratio",
            Self::RatioRange { .. } => "ratio range",
            Self::Veto { .. } => "veto",
            Self::Undetermined => "undetermined",
        };
        f.write_str(label)
    }
}

impl TypeSpecification {
    /// Returns the help text associated with this type, or an empty string if none.
    pub fn help(&self) -> &str {
        match self {
            Self::Boolean { help, .. }
            | Self::Measure { help, .. }
            | Self::Number { help, .. }
            | Self::NumberRange { help, .. }
            | Self::Text { help, .. }
            | Self::Date { help, .. }
            | Self::DateRange { help, .. }
            | Self::Time { help, .. }
            | Self::TimeRange { help, .. }
            | Self::Ratio { help, .. }
            | Self::RatioRange { help, .. }
            | Self::MeasureRange { help, .. } => help.as_str(),
            Self::Veto { .. } | Self::Undetermined => "",
        }
    }
}

/// Extract a typed [`Value`] from the first `CommandArg`, requiring `Literal` shape.
///
/// `Label` args carry identifiers (unit names, option keywords) and never satisfy a
/// command position that wants a literal value. Returning a typed `Value` keeps the
/// caller's match exhaustive over [`Value`] variants — no string coercion path.
fn require_literal<'a>(
    args: &'a [CommandArg],
    cmd: &str,
) -> Result<&'a crate::literals::Value, String> {
    let arg = args
        .first()
        .ok_or_else(|| format!("{} requires an argument", cmd))?;
    match arg {
        CommandArg::Literal(v) => Ok(v),
        CommandArg::Label(name) => Err(format!(
            "{} requires a literal value, got identifier '{}'",
            cmd, name
        )),
        CommandArg::UnitExpr(_) => Err(format!(
            "{} requires a literal value, got a unit expression (only valid for 'unit' command)",
            cmd
        )),
    }
}

fn apply_type_help_command(help: &mut String, args: &[CommandArg]) -> Result<(), String> {
    match require_literal(args, "help")? {
        crate::literals::Value::Text(s) => {
            *help = s.clone();
            Ok(())
        }
        other => Err(format!(
            "help requires a text literal (quoted string), got {}",
            value_kind_name(other)
        )),
    }
}

fn format_measure_units_list(units: &MeasureUnits) -> String {
    units
        .iter()
        .map(|u| u.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// What kind of value `-> suggest` expects when rejecting a calendar literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SuggestionExpectation {
    MeasureUnits,
    Text,
    Number,
    Boolean,
    Date,
    Time,
    Ratio,
    NumberRange,
    DateRange,
    TimeRange,
    MeasureRange,
    RatioRange,
}

pub(crate) fn suggestion_value_mismatch_error(
    calendar_unit: &str,
    type_name: &str,
    expectation: SuggestionExpectation,
    measure_units: Option<&MeasureUnits>,
) -> String {
    let unit_label = calendar_unit;
    let first = format!("Unit '{unit_label}' is for calendar data.");
    match expectation {
        SuggestionExpectation::MeasureUnits => {
            let list = measure_units
                .map(format_measure_units_list)
                .unwrap_or_default();
            format!("{first} Valid '{type_name}' units are: {list}.")
        }
        SuggestionExpectation::Text => format!(
            "{first} Please provide a text value in double quotes, for example `-> suggest \"my default value\"`."
        ),
        SuggestionExpectation::Number => format!(
            "{first} Please provide a number, for example `-> suggest 42`."
        ),
        SuggestionExpectation::Boolean => format!(
            "{first} Please provide true or false, for example `-> suggest true`."
        ),
        SuggestionExpectation::Date => format!(
            "{first} Please provide a date, for example `-> suggest 2024-06-15`."
        ),
        SuggestionExpectation::Time => format!(
            "{first} Please provide a time, for example `-> suggest 09:00:00`."
        ),
        SuggestionExpectation::Ratio | SuggestionExpectation::RatioRange => format!(
            "{first} Please provide a ratio, for example `-> suggest 25%`."
        ),
        SuggestionExpectation::NumberRange => format!(
            "{first} Please provide a number range, for example `-> suggest 10...100`."
        ),
        SuggestionExpectation::DateRange => format!(
            "{first} Please provide a date range, for example `-> suggest 2024-01-01...2024-12-31`."
        ),
        SuggestionExpectation::TimeRange => format!(
            "{first} Please provide a time range, for example `-> suggest 09:00...17:00`."
        ),
        SuggestionExpectation::MeasureRange => format!(
            "{first} Please provide a range with units valid for '{type_name}', for example `-> suggest 30 kilogram...35 kilogram`."
        ),
    }
}

fn measure_suggestion_wrong_shape_error(type_name: &str, traits: &[MeasureTrait]) -> String {
    let example = if traits.contains(&MeasureTrait::Duration) {
        "4 week"
    } else if traits.contains(&MeasureTrait::Calendar) {
        "3 month"
    } else {
        "30 kilogram"
    };
    format!(
        "Please provide a value with a unit valid for '{type_name}', for example `-> suggest {example}`."
    )
}

fn reject_calendar_for_suggestion(
    value: &crate::literals::Value,
    type_name: &str,
    expectation: SuggestionExpectation,
    measure_units: Option<&MeasureUnits>,
) -> Result<(), String> {
    if let crate::literals::Value::NumberWithUnit(_, unit) = value {
        if calendar_unit_factor(unit).is_some() {
            return Err(suggestion_value_mismatch_error(
                unit,
                type_name,
                expectation,
                measure_units,
            ));
        }
    }
    Ok(())
}

/// Human-readable name for a [`Value`] variant — used in mismatch error messages.
fn value_kind_name(v: &crate::literals::Value) -> &'static str {
    use crate::literals::Value;
    match v {
        Value::Number(_) => "number",
        Value::NumberWithUnit(_, _) => "number_with_unit",
        Value::Text(_) => "text",
        Value::Date(_) => "date",
        Value::Time(_) => "time",
        Value::Boolean(_) => "boolean",
        Value::Range(_, _) => "range",
    }
}

fn require_suggestion_range_endpoints<'a>(
    args: &'a [CommandArg],
    type_name: &str,
    expectation: SuggestionExpectation,
    measure_units: Option<&MeasureUnits>,
) -> Result<(&'a crate::literals::Value, &'a crate::literals::Value), String> {
    match require_literal(args, "suggest")? {
        crate::literals::Value::NumberWithUnit(_, unit)
            if calendar_unit_factor(unit).is_some() =>
        {
            Err(suggestion_value_mismatch_error(
                unit,
                type_name,
                expectation,
                measure_units,
            ))
        }
        crate::literals::Value::Range(left, right) => Ok((left.as_ref(), right.as_ref())),
        _ => Err(match expectation {
            SuggestionExpectation::NumberRange => {
                "Please provide a number range, for example `-> suggest 10...100`.".to_string()
            }
            SuggestionExpectation::DateRange => {
                "Please provide a date range, for example `-> suggest 2024-01-01...2024-12-31`."
                    .to_string()
            }
            SuggestionExpectation::RatioRange => {
                "Please provide a ratio range, for example `-> suggest 10%...50%`.".to_string()
            }
            SuggestionExpectation::MeasureRange => format!(
                "Please provide a range with units valid for '{type_name}', for example `-> suggest 30 kilogram...35 kilogram`."
            ),
            _ => unreachable!("BUG: require_suggestion_range_endpoints called with non-range expectation"),
        }),
    }
}

fn lift_parser_decimal(decimal: rust_decimal::Decimal) -> Result<RationalInteger, String> {
    crate::computation::rational::decimal_to_rational(decimal)
        .map_err(|failure| format!("literal failed rational lift: {failure}"))
}

/// Element spec for a range type, used for parsing endpoints and lifting literal endpoints.
pub fn range_element_type_specification(
    range_spec: &TypeSpecification,
) -> Option<TypeSpecification> {
    range_spec.element_from_range()
}

fn range_endpoints_compatible(left: &LemmaType, right: &LemmaType) -> bool {
    match (&left.specifications, &right.specifications) {
        (TypeSpecification::Date { .. }, TypeSpecification::Date { .. }) => true,
        (TypeSpecification::Time { .. }, TypeSpecification::Time { .. }) => true,
        (TypeSpecification::Number { .. }, TypeSpecification::Number { .. }) => true,
        (TypeSpecification::Measure { .. }, TypeSpecification::Measure { .. }) => {
            left.same_measure_family(right)
                || left.compatible_with_anonymous_measure(right)
                || right.compatible_with_anonymous_measure(left)
        }
        (TypeSpecification::Ratio { .. }, TypeSpecification::Ratio { .. }) => true,
        _ => false,
    }
}

/// Infer the range type specification from two compatible endpoint types.
pub fn range_type_specification_from_endpoints(
    left: &LemmaType,
    right: &LemmaType,
) -> Option<TypeSpecification> {
    if !range_endpoints_compatible(left, right) {
        return None;
    }
    left.specifications.range_from_element()
}

/// Lift a parser literal range endpoint to a [`LiteralValue`] with the element's primitive type.
/// Routes [`Value::NumberWithUnit`] through [`parser_value_to_value_kind`] so ratio endpoints
/// (e.g. `10%` in a `ratio range`) canonicalize to ratios, not anonymous quantities.
fn lift_range_endpoint(
    value: &crate::parsing::ast::Value,
    element_spec: &TypeSpecification,
) -> Result<TypedLiteral, String> {
    use crate::parsing::ast::Value;
    match value {
        Value::NumberWithUnit(_, unit_name) => {
            let kind = parser_value_to_value_kind(value, element_spec)?;
            let lemma_type = match &kind {
                ValueKind::Measure(_) | ValueKind::Ratio(_) => Arc::new(
                    LemmaType::primitive(element_spec.clone())
                        .with_measure_binding_unit(unit_name.clone()),
                ),
                _ => Arc::new(LemmaType::primitive(element_spec.clone())),
            };
            Ok(TypedLiteral {
                value: kind,
                lemma_type,
            })
        }
        _ => {
            let kind = value_to_semantic(value)?;
            Ok(TypedLiteral {
                value: kind,
                lemma_type: Arc::new(LemmaType::primitive(element_spec.clone())),
            })
        }
    }
}

fn literal_value_from_parser_value(
    value: &crate::parsing::ast::Value,
) -> Result<TypedLiteral, String> {
    use crate::parsing::ast::Value;

    match value {
        Value::Number(n) => Ok(TypedLiteral::number(lift_parser_decimal(*n)?)),
        Value::Text(s) => Ok(TypedLiteral::text(s.clone())),
        Value::Date(dt) => Ok(TypedLiteral::date(date_time_to_semantic(dt))),
        Value::Time(t) => Ok(TypedLiteral::time(time_to_semantic(t))),
        Value::Boolean(b) => Ok(TypedLiteral::from_bool(bool::from(*b))),
        Value::NumberWithUnit(n, unit) => Ok(TypedLiteral::number_interpreted_as_measure(
            lift_parser_decimal(*n)?,
            unit.clone(),
        )),
        Value::Range(left, right) => {
            let left = literal_value_from_parser_value(left)?;
            let right = literal_value_from_parser_value(right)?;
            let compatible = match (
                &left.lemma_type.specifications,
                &right.lemma_type.specifications,
            ) {
                (TypeSpecification::Date { .. }, TypeSpecification::Date { .. }) => true,
                (TypeSpecification::Time { .. }, TypeSpecification::Time { .. }) => true,
                (TypeSpecification::Number { .. }, TypeSpecification::Number { .. }) => true,
                (TypeSpecification::Measure { .. }, TypeSpecification::Measure { .. }) => {
                    left.lemma_type.same_measure_family(&right.lemma_type)
                        || left
                            .lemma_type
                            .compatible_with_anonymous_measure(&right.lemma_type)
                        || right
                            .lemma_type
                            .compatible_with_anonymous_measure(&left.lemma_type)
                }
                (TypeSpecification::Ratio { .. }, TypeSpecification::Ratio { .. }) => true,
                _ => false,
            };
            if !compatible {
                return Err(format!(
                    "range endpoints must have the same supported base type, got {} and {}",
                    left.lemma_type.name(),
                    right.lemma_type.name()
                ));
            }
            Ok(TypedLiteral::range(left, right))
        }
    }
}

/// Cast a [`RationalInteger`] to `u8`, requiring it to be a non-negative whole number that fits.
fn decimal_to_u8(d: RationalInteger, ctx: &str) -> Result<u8, String> {
    if !d.is_integer() {
        return Err(format!(
            "{} requires a whole number, got fractional value",
            ctx
        ));
    }
    d.numer_to_u8()
        .ok_or_else(|| format!("{} value out of range for u8", ctx))
}

/// Cast a [`RationalInteger`] to `usize`, requiring it to be a non-negative whole number that fits.
fn decimal_to_usize(d: RationalInteger, ctx: &str) -> Result<usize, String> {
    if !d.is_integer() {
        return Err(format!(
            "{} requires a whole number, got fractional value",
            ctx
        ));
    }
    d.numer_to_usize()
        .ok_or_else(|| format!("{} value out of range for usize", ctx))
}

/// Extract a number literal from a [`Value::Number`] arg and lift it to [`RationalInteger`].
///
/// Numeric meta-constraints (`decimals`, `length`, `minimum`/`maximum`
/// on `Number` and `Measure`) take a bare number literal — not a ratio, not a measure. Reject
/// any other variant to honour the no-coercion contract.
fn ratio_bound_to_canonical_rational(
    args: &[CommandArg],
    cmd: &str,
    units: &RatioUnits,
) -> Result<RationalInteger, String> {
    use crate::computation::rational::{checked_div, decimal_to_rational};
    let lit = require_literal(args, cmd)?;
    match lit {
        crate::literals::Value::NumberWithUnit(magnitude, unit_name) => {
            let unit = units.get(unit_name.as_str())?;
            let magnitude_rational = decimal_to_rational(*magnitude)
                .map_err(|failure| format!("{cmd} literal failed rational lift: {failure}"))?;
            checked_div(&magnitude_rational, &unit.value)
                .map_err(|failure| format!("{cmd}: unit conversion failed: {failure}"))
        }
        other => Err(format!(
            "{cmd} requires a ratio literal with a unit, got {}",
            value_kind_name(other)
        )),
    }
}

fn require_decimal_literal(args: &[CommandArg], cmd: &str) -> Result<RationalInteger, String> {
    use crate::computation::rational::decimal_to_rational;
    match require_literal(args, cmd)? {
        crate::literals::Value::Number(d) => decimal_to_rational(*d)
            .map_err(|failure| format!("{} literal failed rational lift: {}", cmd, failure)),
        other => Err(format!(
            "{} requires a number literal, got {}",
            cmd,
            value_kind_name(other)
        )),
    }
}

enum UnitConstraintField {
    Minimum,
    Maximum,
    SuggestionMagnitude,
}

pub(crate) fn measure_declared_bound_to_canonical(
    magnitude: &RationalInteger,
    unit_name: &str,
    units: &MeasureUnits,
    type_name: &str,
    command: &str,
) -> Result<RationalInteger, String> {
    use crate::computation::rational::checked_mul;
    let unit = units.get(unit_name).map_err(|_| {
        format!(
            "Unit '{unit_name}' is not defined on '{type_name}'. Valid units are: {}.",
            format_measure_units_list(units)
        )
    })?;
    checked_mul(magnitude, &unit.factor)
        .map_err(|failure| format!("{command}: unit conversion overflow: {failure}"))
}

fn parse_measure_declared_bound(
    args: &[CommandArg],
    cmd: &str,
    units: &MeasureUnits,
    type_name: &str,
) -> Result<(RationalInteger, String), String> {
    use crate::computation::rational::decimal_to_rational;
    let lit = require_literal(args, cmd)?;
    let (magnitude, unit_name) = match lit {
        crate::literals::Value::NumberWithUnit(n, unit) => (*n, unit.clone()),
        other => {
            return Err(format!(
                "{cmd} requires a measure literal with a unit, got {}",
                value_kind_name(other)
            ));
        }
    };
    units.get(unit_name.as_str()).map_err(|_| {
        format!(
            "Unit '{unit_name}' is not defined on '{type_name}'. Valid units are: {}.",
            format_measure_units_list(units)
        )
    })?;
    let magnitude_rational = decimal_to_rational(magnitude)
        .map_err(|failure| format!("{cmd} literal failed rational lift: {failure}"))?;
    Ok((magnitude_rational, unit_name))
}

fn sync_measure_units_from_canonical(
    units: &mut MeasureUnits,
    canonical: &RationalInteger,
    field: UnitConstraintField,
) -> Result<(), String> {
    use crate::computation::rational::checked_div;
    for unit in &mut units.0 {
        let magnitude = checked_div(canonical, &unit.factor).map_err(|failure| {
            format!(
                "cannot derive per-unit constraint for unit '{}': {failure}",
                unit.name
            )
        })?;
        match field {
            UnitConstraintField::Minimum => unit.minimum = Some(magnitude),
            UnitConstraintField::Maximum => unit.maximum = Some(magnitude),
            UnitConstraintField::SuggestionMagnitude => unit.suggestion_magnitude = Some(magnitude),
        }
    }
    Ok(())
}

fn sync_ratio_units_from_canonical(
    units: &mut RatioUnits,
    canonical: &RationalInteger,
    field: UnitConstraintField,
) -> Result<(), String> {
    use crate::computation::rational::checked_mul;
    for unit in &mut units.0 {
        let magnitude = checked_mul(canonical, &unit.value).map_err(|failure| {
            format!(
                "cannot derive per-unit constraint for ratio unit '{}': {failure}",
                unit.name
            )
        })?;
        match field {
            UnitConstraintField::Minimum => unit.minimum = Some(magnitude),
            UnitConstraintField::Maximum => unit.maximum = Some(magnitude),
            UnitConstraintField::SuggestionMagnitude => unit.suggestion_magnitude = Some(magnitude),
        }
    }
    Ok(())
}

fn sync_measure_suggestion_units(
    units: &mut MeasureUnits,
    default: &ValueKind,
    type_name: &str,
) -> Result<(), String> {
    let ValueKind::Measure(magnitude) = default else {
        return Ok(());
    };
    let unit_name = units
        .iter()
        .find(|unit| unit.is_canonical_factor())
        .or_else(|| units.iter().next())
        .map(|unit| unit.name.as_str())
        .expect(
            "BUG: Measure suggestion value requires at least one declared unit on the measure type",
        );
    units.get(unit_name).map_err(|_| {
        format!("Suggestion unit '{unit_name}' is not defined on measure type '{type_name}'.")
    })?;
    sync_measure_units_from_canonical(units, magnitude, UnitConstraintField::SuggestionMagnitude)
}

pub(crate) fn finalize_measure_unit_constraint_magnitudes(
    specification: &mut TypeSpecification,
    declared_suggestion: Option<&ValueKind>,
    type_name: &str,
) -> Result<(), String> {
    let TypeSpecification::Measure {
        minimum,
        maximum,
        units,
        ..
    } = specification
    else {
        return Ok(());
    };

    if let Some(bound) = minimum.as_ref() {
        let canonical =
            measure_declared_bound_to_canonical(&bound.0, &bound.1, units, type_name, "minimum")?;
        sync_measure_units_from_canonical(units, &canonical, UnitConstraintField::Minimum)?;
    }
    if let Some(bound) = maximum.as_ref() {
        let canonical =
            measure_declared_bound_to_canonical(&bound.0, &bound.1, units, type_name, "maximum")?;
        sync_measure_units_from_canonical(units, &canonical, UnitConstraintField::Maximum)?;
    }
    if let Some(default) = declared_suggestion {
        sync_measure_suggestion_units(units, default, type_name)?;
    }

    if minimum.is_some() {
        for unit in units.iter() {
            assert!(
                unit.minimum.is_some(),
                "BUG: type '{type_name}' has minimum but unit '{}' missing per-unit minimum after finalize",
                unit.name
            );
        }
    }
    if maximum.is_some() {
        for unit in units.iter() {
            assert!(
                unit.maximum.is_some(),
                "BUG: type '{type_name}' has maximum but unit '{}' missing per-unit maximum after finalize",
                unit.name
            );
        }
    }
    if declared_suggestion.is_some() {
        for unit in units.iter() {
            assert!(
                unit.suggestion_magnitude.is_some(),
                "BUG: type '{type_name}' has default but unit '{}' missing per-unit default after finalize",
                unit.name
            );
        }
    }

    Ok(())
}

fn sync_ratio_suggestion_units(units: &mut RatioUnits, default: &ValueKind) -> Result<(), String> {
    let ValueKind::Ratio(canonical) = default else {
        return Ok(());
    };
    sync_ratio_units_from_canonical(units, canonical, UnitConstraintField::SuggestionMagnitude)
}

/// Extract an option name from a single arg.
///
/// Both `option red` (bare identifier, parsed as `Label`) and `option "red"`
/// (quoted text literal) are valid lemma syntax for option enumeration; the
/// grammar accepts either form. All other variants are rejected.
fn option_name(arg: &CommandArg, cmd: &str) -> Result<String, String> {
    match arg {
        CommandArg::Literal(crate::literals::Value::Text(s)) => Ok(s.clone()),
        CommandArg::Label(name) => Ok(name.clone()),
        CommandArg::Literal(other) => Err(format!(
            "{} requires a text literal or identifier, got {}",
            cmd,
            value_kind_name(other)
        )),
        CommandArg::UnitExpr(_) => Err(format!(
            "{} requires a text literal or identifier, got a unit expression",
            cmd
        )),
    }
}

fn label_name(arg: &CommandArg, cmd: &str) -> Result<String, String> {
    match arg {
        CommandArg::Label(name) => Ok(name.clone()),
        CommandArg::Literal(other) => Err(format!(
            "{} requires an identifier, got {}",
            cmd,
            value_kind_name(other)
        )),
        CommandArg::UnitExpr(_) => Err(format!(
            "{} requires an identifier, got a unit expression",
            cmd
        )),
    }
}

fn measure_trait_name(measure_trait: MeasureTrait) -> &'static str {
    match measure_trait {
        MeasureTrait::Duration => "duration",
        MeasureTrait::Calendar => "calendar",
    }
}

fn parse_measure_trait(args: &[CommandArg]) -> Result<MeasureTrait, String> {
    if args.len() != 1 {
        return Err("trait requires exactly one identifier argument".to_string());
    }
    match label_name(&args[0], "trait")?
        .trim()
        .to_lowercase()
        .as_str()
    {
        "duration" => Ok(MeasureTrait::Duration),
        "calendar" => Ok(MeasureTrait::Calendar),
        other => Err(format!("Unknown measure trait '{}'", other)),
    }
}

fn validate_calendar_trait_requirements(units: &MeasureUnits) -> Result<(), String> {
    let month_unit = units
        .iter()
        .find(|unit| unit.name == "month")
        .ok_or_else(|| {
            "trait calendar requires a canonical 'month' unit declared before 'trait calendar'"
                .to_string()
        })?;
    if !month_unit.is_canonical_factor() {
        return Err("trait calendar requires unit month 1".to_string());
    }
    Ok(())
}

fn validate_duration_trait_requirements(units: &MeasureUnits) -> Result<(), String> {
    let second_unit = units
        .iter()
        .find(|unit| unit.name == "second")
        .ok_or_else(|| {
            "trait duration requires a canonical 'second' unit declared before 'trait duration'"
                .to_string()
        })?;
    if !second_unit.is_canonical_factor() {
        return Err("trait duration requires unit second 1".to_string());
    }
    Ok(())
}

/// Extract a [`DateTimeValue`] from a [`Value::Date`] literal arg.
fn require_date_literal(args: &[CommandArg], cmd: &str) -> Result<DateTimeValue, String> {
    match require_literal(args, cmd)? {
        crate::literals::Value::Date(dt) => Ok(dt.clone()),
        other => Err(format!(
            "{} requires a date literal (e.g. 2024-01-01), got {}",
            cmd,
            value_kind_name(other)
        )),
    }
}

/// Extract a [`TimeValue`] from a [`Value::Time`] literal arg.
fn require_time_literal(args: &[CommandArg], cmd: &str) -> Result<TimeValue, String> {
    match require_literal(args, cmd)? {
        crate::literals::Value::Time(t) => Ok(t.clone()),
        other => Err(format!(
            "{} requires a time literal (e.g. 12:30:00), got {}",
            cmd,
            value_kind_name(other)
        )),
    }
}

/// Default `help` for a built-in primitive (goal-oriented; syntax lives in [`LemmaType::example_value`]).
#[must_use]
pub fn default_help_for_primitive(kind: PrimitiveKind) -> &'static str {
    use PrimitiveKind::*;
    match kind {
        Boolean => "Whether this holds (true or false).",
        Number => "A dimensionless number.",
        NumberRange => "The lower and upper bound of the number range.",
        Text => "A text value.",
        Measure => "A numeric amount in one of this type's units.",
        MeasureRange => "The lower and upper bound of the measure range in the same unit.",
        Ratio => "A ratio in one of this type's units (e.g. percent).",
        RatioRange => "The lower and upper bound of the ratio range.",
        Date => "A date, or a date and time with optional timezone.",
        DateRange => "The start date and end date of the date range.",
        Time => "A time of day, with optional timezone.",
        TimeRange => "The start time and end time of the time range.",
    }
}

impl TypeSpecification {
    pub fn boolean() -> Self {
        TypeSpecification::Boolean {
            help: default_help_for_primitive(PrimitiveKind::Boolean).to_string(),
        }
    }
    pub fn measure() -> Self {
        TypeSpecification::Measure {
            minimum: None,
            maximum: None,
            decimals: None,
            units: MeasureUnits::new(),
            traits: Vec::new(),
            decomposition: None,
            help: default_help_for_primitive(PrimitiveKind::Measure).to_string(),
        }
    }
    pub fn number() -> Self {
        TypeSpecification::Number {
            minimum: None,
            maximum: None,
            decimals: None,
            help: default_help_for_primitive(PrimitiveKind::Number).to_string(),
        }
    }
    pub fn number_range() -> Self {
        TypeSpecification::NumberRange {
            lower: None,
            upper: None,
            minimum: None,
            maximum: None,
            help: default_help_for_primitive(PrimitiveKind::NumberRange).to_string(),
        }
    }
    pub fn ratio() -> Self {
        TypeSpecification::Ratio {
            minimum: None,
            maximum: None,
            decimals: None,
            units: RatioUnits(vec![
                RatioUnit {
                    name: "percent".to_string(),
                    value: crate::computation::rational::rational_new(100, 1),
                    minimum: None,
                    maximum: None,
                    suggestion_magnitude: None,
                },
                RatioUnit {
                    name: "permille".to_string(),
                    value: crate::computation::rational::rational_new(1000, 1),
                    minimum: None,
                    maximum: None,
                    suggestion_magnitude: None,
                },
            ]),
            help: default_help_for_primitive(PrimitiveKind::Ratio).to_string(),
        }
    }
    pub fn ratio_range() -> Self {
        TypeSpecification::RatioRange {
            lower: None,
            upper: None,
            minimum: None,
            maximum: None,
            units: match TypeSpecification::ratio() {
                TypeSpecification::Ratio { units, .. } => units,
                _ => unreachable!("BUG: ratio constructor must return a ratio type"),
            },
            help: default_help_for_primitive(PrimitiveKind::RatioRange).to_string(),
        }
    }
    pub fn text() -> Self {
        TypeSpecification::Text {
            length: None,
            options: vec![],
            help: default_help_for_primitive(PrimitiveKind::Text).to_string(),
        }
    }
    pub fn date() -> Self {
        TypeSpecification::Date {
            minimum: None,
            maximum: None,
            help: default_help_for_primitive(PrimitiveKind::Date).to_string(),
        }
    }
    pub fn date_range() -> Self {
        TypeSpecification::DateRange {
            lower: None,
            upper: None,
            minimum: None,
            maximum: None,
            help: default_help_for_primitive(PrimitiveKind::DateRange).to_string(),
        }
    }
    pub fn time() -> Self {
        TypeSpecification::Time {
            minimum: None,
            maximum: None,
            help: default_help_for_primitive(PrimitiveKind::Time).to_string(),
        }
    }
    pub fn time_range() -> Self {
        TypeSpecification::TimeRange {
            lower: None,
            upper: None,
            minimum: None,
            maximum: None,
            help: default_help_for_primitive(PrimitiveKind::TimeRange).to_string(),
        }
    }
    pub fn measure_range() -> Self {
        TypeSpecification::MeasureRange {
            lower: None,
            upper: None,
            minimum: None,
            maximum: None,
            units: MeasureUnits::new(),
            decomposition: None,
            help: default_help_for_primitive(PrimitiveKind::MeasureRange).to_string(),
        }
    }

    /// Element spec for a range type (e.g. `MeasureRange` → `Measure`).
    #[must_use]
    pub fn element_from_range(&self) -> Option<Self> {
        match self {
            TypeSpecification::NumberRange { lower, upper, .. } => {
                Some(TypeSpecification::Number {
                    minimum: lower.clone(),
                    maximum: upper.clone(),
                    decimals: None,
                    help: String::new(),
                })
            }
            TypeSpecification::MeasureRange {
                lower,
                upper,
                units,
                decomposition,
                ..
            } => Some(TypeSpecification::Measure {
                minimum: lower.clone(),
                maximum: upper.clone(),
                decimals: None,
                units: units.clone(),
                traits: Vec::new(),
                decomposition: decomposition.clone(),
                help: String::new(),
            }),
            TypeSpecification::DateRange { lower, upper, .. } => Some(TypeSpecification::Date {
                minimum: lower.clone(),
                maximum: upper.clone(),
                help: String::new(),
            }),
            TypeSpecification::TimeRange { lower, upper, .. } => Some(TypeSpecification::Time {
                minimum: lower.clone(),
                maximum: upper.clone(),
                help: String::new(),
            }),
            TypeSpecification::RatioRange {
                lower,
                upper,
                units,
                ..
            } => Some(TypeSpecification::Ratio {
                minimum: lower.clone(),
                maximum: upper.clone(),
                decimals: None,
                units: units.clone(),
                help: String::new(),
            }),
            _ => None,
        }
    }

    /// Range spec for an element type (e.g. `Measure` → `MeasureRange`).
    #[must_use]
    pub fn range_from_element(&self) -> Option<Self> {
        match self {
            TypeSpecification::Number {
                minimum, maximum, ..
            } => Some(TypeSpecification::NumberRange {
                lower: minimum.clone(),
                upper: maximum.clone(),
                minimum: None,
                maximum: None,
                help: default_help_for_primitive(PrimitiveKind::NumberRange).to_string(),
            }),
            TypeSpecification::Measure {
                minimum,
                maximum,
                units,
                decomposition,
                ..
            } => Some(TypeSpecification::MeasureRange {
                lower: minimum.clone(),
                upper: maximum.clone(),
                minimum: None,
                maximum: None,
                units: units.clone(),
                decomposition: decomposition.clone(),
                help: default_help_for_primitive(PrimitiveKind::MeasureRange).to_string(),
            }),
            TypeSpecification::Date {
                minimum, maximum, ..
            } => Some(TypeSpecification::DateRange {
                lower: minimum.clone(),
                upper: maximum.clone(),
                minimum: None,
                maximum: None,
                help: default_help_for_primitive(PrimitiveKind::DateRange).to_string(),
            }),
            TypeSpecification::Time {
                minimum, maximum, ..
            } => Some(TypeSpecification::TimeRange {
                lower: minimum.clone(),
                upper: maximum.clone(),
                minimum: None,
                maximum: None,
                help: default_help_for_primitive(PrimitiveKind::TimeRange).to_string(),
            }),
            TypeSpecification::Ratio {
                minimum,
                maximum,
                units,
                ..
            } => Some(TypeSpecification::RatioRange {
                lower: minimum.clone(),
                upper: maximum.clone(),
                minimum: None,
                maximum: None,
                units: units.clone(),
                help: default_help_for_primitive(PrimitiveKind::RatioRange).to_string(),
            }),
            _ => None,
        }
    }

    /// Minimum bound as decimal for interactive numeric prompts (number, measure, ratio).
    #[must_use]
    pub fn minimum_decimal(&self) -> Option<Decimal> {
        match self {
            TypeSpecification::Number { minimum, .. }
            | TypeSpecification::Ratio { minimum, .. } => minimum.as_ref().map(|bound| {
                bound
                    .try_to_decimal()
                    .expect("BUG: planned minimum must convert to decimal")
            }),
            TypeSpecification::Measure { minimum, .. } => minimum.as_ref().map(|(bound, _unit)| {
                bound
                    .try_to_decimal()
                    .expect("BUG: planned minimum must convert to decimal")
            }),
            _ => None,
        }
    }

    /// Maximum bound as decimal for interactive numeric prompts (number, measure, ratio).
    #[must_use]
    pub fn maximum_decimal(&self) -> Option<Decimal> {
        match self {
            TypeSpecification::Number { maximum, .. }
            | TypeSpecification::Ratio { maximum, .. } => maximum.as_ref().map(|bound| {
                bound
                    .try_to_decimal()
                    .expect("BUG: planned maximum must convert to decimal")
            }),
            TypeSpecification::Measure { maximum, .. } => maximum.as_ref().map(|(bound, _unit)| {
                bound
                    .try_to_decimal()
                    .expect("BUG: planned maximum must convert to decimal")
            }),
            _ => None,
        }
    }

    pub fn veto() -> Self {
        TypeSpecification::Veto { message: None }
    }

    /// Apply a single constraint command to this spec.
    ///
    /// The `declared_suggestion` and `declared_fill` out-parameters receive default values
    /// (if the command is `Suggest` or `Fill`), encoded as [`RawSuggestion`]. Defaults are
    /// owned by the data binding or typedef entry, not by the type specification itself;
    /// callers thread `&mut Option<RawSuggestion>` for each across constraint applications
    /// for one declaration. Duplicate `-> suggest` / `-> fill` / `minimum` / `maximum` /
    /// `decimals` on the same declaration are rejected by the caller seen-set before this
    /// runs. A child typedef may override an inherited suggest, fill, or bound with one
    /// command of that kind. Measure scalars stay raw until unit factors are resolved;
    /// callers convert via [`bound_value_kind_from_raw_suggestion`].
    pub fn apply_constraint(
        &mut self,
        type_name: &str,
        command: TypeConstraintCommand,
        args: &[CommandArg],
        declared_suggestion: &mut Option<RawSuggestion>,
        declared_fill: &mut Option<RawSuggestion>,
    ) -> Result<(), String> {
        if command == TypeConstraintCommand::Trait
            && !matches!(&self, TypeSpecification::Measure { .. })
        {
            return Err("trait command is only valid on measure types".to_string());
        }
        match self {
            TypeSpecification::Boolean { help } => match command {
                TypeConstraintCommand::Help => {
                    apply_type_help_command(help, args)?;
                }
                TypeConstraintCommand::Suggest | TypeConstraintCommand::Fill => {
                    let (target, cmd) = match command {
                        TypeConstraintCommand::Suggest => (&mut *declared_suggestion, "suggest"),
                        TypeConstraintCommand::Fill => (&mut *declared_fill, "fill"),
                        _ => unreachable!("BUG: only Suggest or Fill in this arm"),
                    };
                    let lit = require_literal(args, cmd)?;
                    reject_calendar_for_suggestion(
                        lit,
                        type_name,
                        SuggestionExpectation::Boolean,
                        None,
                    )?;
                    match lit {
                        crate::literals::Value::Boolean(bv) => {
                            *target =
                                Some(RawSuggestion::Value(ValueKind::Boolean(bool::from(bv))));
                        }
                        _ => {
                            return Err(
                                "Please provide true or false, for example `-> suggest true`."
                                    .to_string(),
                            );
                        }
                    }
                }
                other => {
                    return Err(format!(
                        "Invalid command '{}' for boolean type. Valid commands: help, suggest, fill",
                        other
                    ));
                }
            },
            TypeSpecification::Measure {
                decimals,
                minimum,
                maximum,
                units,
                traits,
                help,
                ..
            } => match command {
                TypeConstraintCommand::Decimals => {
                    let d = require_decimal_literal(args, "decimals")?;
                    *decimals = Some(decimal_to_u8(d, "decimals")?);
                }
                TypeConstraintCommand::Unit => {
                    let (unit_name, value, derived_measure_factors) = match args {
                        [CommandArg::Label(name), CommandArg::UnitExpr(crate::parsing::ast::UnitArg::Factor(v))] => {
                            (name.clone(), *v, Vec::new())
                        }
                        [CommandArg::Label(name), CommandArg::UnitExpr(crate::parsing::ast::UnitArg::Expr(
                            prefix,
                            factors,
                        ))] => {
                            let raw: Vec<(String, i32)> = factors
                                .iter()
                                .map(|f| (f.measure_ref.clone(), f.exp))
                                .collect();
                            (name.clone(), *prefix, raw)
                        }
                        _ => {
                            return Err(
                                "unit requires a unit name followed by a conversion factor or compound unit expression (e.g., 'unit eur 1.00' or 'unit mps meter/second')"
                                    .to_string(),
                            );
                        }
                    };
                    if let Some(existing) = units.0.iter().find(|u| u.name == unit_name) {
                        let new_factor = crate::computation::rational::decimal_to_rational(value)
                            .map_err(|failure| failure.to_string())?;
                        if existing.factor != new_factor
                            || existing.derived_measure_factors != derived_measure_factors
                        {
                            return Err(format!(
                                "Unit '{unit_name}' is already defined in this type's inherited units; \
                                 cannot change factor or decomposition. Add a new unit name instead."
                            ));
                        }
                    } else {
                        units.0.push(MeasureUnit::from_decimal_factor(
                            unit_name,
                            value,
                            derived_measure_factors,
                        )?);
                    }
                }
                TypeConstraintCommand::Trait => {
                    let measure_trait = parse_measure_trait(args)?;
                    if traits.contains(&measure_trait) {
                        return Err(format!(
                            "Duplicate trait '{}' for measure type.",
                            measure_trait_name(measure_trait)
                        ));
                    }
                    if measure_trait == MeasureTrait::Duration {
                        validate_duration_trait_requirements(units)?;
                    }
                    if measure_trait == MeasureTrait::Calendar {
                        validate_calendar_trait_requirements(units)?;
                    }
                    traits.push(measure_trait);
                }
                TypeConstraintCommand::Minimum => {
                    *minimum = Some(parse_measure_declared_bound(
                        args, "minimum", units, type_name,
                    )?);
                }
                TypeConstraintCommand::Maximum => {
                    *maximum = Some(parse_measure_declared_bound(
                        args, "maximum", units, type_name,
                    )?);
                }
                TypeConstraintCommand::Help => {
                    apply_type_help_command(help, args)?;
                }
                TypeConstraintCommand::Suggest | TypeConstraintCommand::Fill => {
                    let (target, cmd) = match command {
                        TypeConstraintCommand::Suggest => (&mut *declared_suggestion, "suggest"),
                        TypeConstraintCommand::Fill => (&mut *declared_fill, "fill"),
                        _ => unreachable!("BUG: only Suggest or Fill in this arm"),
                    };
                    let lit = require_literal(args, cmd)?;
                    if !traits.contains(&MeasureTrait::Calendar) {
                        reject_calendar_for_suggestion(
                            lit,
                            type_name,
                            SuggestionExpectation::MeasureUnits,
                            Some(units),
                        )?;
                    }
                    match lit {
                        crate::literals::Value::NumberWithUnit(_, _) => {
                            let (magnitude, unit_name) =
                                parse_measure_declared_bound(args, cmd, units, type_name)?;
                            *target = Some(RawSuggestion::Measure {
                                magnitude,
                                unit_name,
                            });
                        }
                        _ => {
                            return Err(measure_suggestion_wrong_shape_error(type_name, traits));
                        }
                    }
                }
                _ => {
                    return Err(format!(
                        "Invalid command '{}' for measure type. Valid commands: unit, trait, minimum, maximum, decimals, help, suggest, fill",
                        command
                    ));
                }
            },
            TypeSpecification::Number {
                decimals,
                minimum,
                maximum,
                help,
            } => match command {
                TypeConstraintCommand::Decimals => {
                    let d = require_decimal_literal(args, "decimals")?;
                    *decimals = Some(decimal_to_u8(d, "decimals")?);
                }
                TypeConstraintCommand::Unit => {
                    return Err(
                        "Invalid command 'unit' for number type. Number types are dimensionless and cannot have units. Use 'measure' type instead.".to_string()
                    );
                }
                TypeConstraintCommand::Minimum => {
                    *minimum = Some(require_decimal_literal(args, "minimum")?);
                }
                TypeConstraintCommand::Maximum => {
                    *maximum = Some(require_decimal_literal(args, "maximum")?);
                }
                TypeConstraintCommand::Help => {
                    apply_type_help_command(help, args)?;
                }
                TypeConstraintCommand::Suggest | TypeConstraintCommand::Fill => {
                    let (target, cmd) = match command {
                        TypeConstraintCommand::Suggest => (&mut *declared_suggestion, "suggest"),
                        TypeConstraintCommand::Fill => (&mut *declared_fill, "fill"),
                        _ => unreachable!("BUG: only Suggest or Fill in this arm"),
                    };
                    let lit = require_literal(args, cmd)?;
                    reject_calendar_for_suggestion(
                        lit,
                        type_name,
                        SuggestionExpectation::Number,
                        None,
                    )?;
                    match lit {
                        crate::literals::Value::Number(d) => {
                            *target = Some(RawSuggestion::Value(ValueKind::Number(
                                lift_parser_decimal(*d)?,
                            )));
                        }
                        _ => {
                            return Err(
                                "Please provide a number, for example `-> suggest 42`.".to_string()
                            );
                        }
                    }
                }
                _ => {
                    return Err(format!(
                        "Invalid command '{}' for number type. Valid commands: minimum, maximum, decimals, help, suggest, fill",
                        command
                    ));
                }
            },
            TypeSpecification::NumberRange {
                lower,
                upper,
                minimum,
                maximum,
                help,
            } => match command {
                TypeConstraintCommand::Lower => {
                    *lower = Some(require_decimal_literal(args, "lower")?);
                }
                TypeConstraintCommand::Upper => {
                    *upper = Some(require_decimal_literal(args, "upper")?);
                }
                TypeConstraintCommand::Minimum => {
                    let width = require_decimal_literal(args, "minimum")?;
                    reject_negative_width_magnitude(&width, "minimum")?;
                    *minimum = Some(width);
                }
                TypeConstraintCommand::Maximum => {
                    let width = require_decimal_literal(args, "maximum")?;
                    reject_negative_width_magnitude(&width, "maximum")?;
                    *maximum = Some(width);
                }
                TypeConstraintCommand::Help => {
                    apply_type_help_command(help, args)?;
                }
                TypeConstraintCommand::Suggest | TypeConstraintCommand::Fill => {
                    let target = match command {
                        TypeConstraintCommand::Suggest => &mut *declared_suggestion,
                        TypeConstraintCommand::Fill => &mut *declared_fill,
                        _ => unreachable!("BUG: only Suggest or Fill in this arm"),
                    };
                    let (left, right) = require_suggestion_range_endpoints(
                        args,
                        type_name,
                        SuggestionExpectation::NumberRange,
                        None,
                    )?;
                    let left = literal_value_from_parser_value(left)?;
                    let right = literal_value_from_parser_value(right)?;
                    if !left.lemma_type.is_number() || !right.lemma_type.is_number() {
                        return Err(
                            "Please provide a number range, for example `-> suggest 10...100`."
                                .to_string(),
                        );
                    }
                    *target = Some(RawSuggestion::Value(ValueKind::Range(
                        Box::new(left.to_literal()),
                        Box::new(right.to_literal()),
                    )));
                }
                _ => {
                    return Err(format!(
                        "Invalid command '{}' for number range type. Valid commands: lower, upper, minimum, maximum, help, suggest, fill",
                        command
                    ));
                }
            },
            TypeSpecification::Ratio {
                decimals,
                minimum,
                maximum,
                units,
                help,
            } => match command {
                TypeConstraintCommand::Decimals => {
                    let d = require_decimal_literal(args, "decimals")?;
                    *decimals = Some(decimal_to_u8(d, "decimals")?);
                }
                TypeConstraintCommand::Unit => {
                    let (unit_name, value_dec) = match args {
                        [CommandArg::Label(name), CommandArg::UnitExpr(crate::parsing::ast::UnitArg::Factor(v))] => {
                            (name.clone(), *v)
                        }
                        _ => {
                            return Err(
                                "unit requires a unit name followed by a numeric conversion factor (e.g., 'unit percent 100'). Compound unit expressions are not supported for ratio types."
                                    .to_string(),
                            );
                        }
                    };
                    let value = crate::computation::rational::decimal_to_rational(value_dec)
                        .map_err(|failure| {
                            format!(
                                "ratio unit value is not exactly representable as a rational: {}",
                                failure
                            )
                        })?;
                    if let Some(existing) = units.0.iter().find(|u| u.name == unit_name) {
                        if existing.value != value {
                            return Err(format!(
                                "Unit '{unit_name}' is already defined in this type's inherited units; \
                                 cannot change factor. Add a new unit name instead."
                            ));
                        }
                    } else {
                        units.0.push(RatioUnit {
                            name: unit_name,
                            value,
                            minimum: None,
                            maximum: None,
                            suggestion_magnitude: None,
                        });
                    }
                }
                TypeConstraintCommand::Minimum => {
                    let canonical = ratio_bound_to_canonical_rational(args, "minimum", units)?;
                    sync_ratio_units_from_canonical(
                        units,
                        &canonical,
                        UnitConstraintField::Minimum,
                    )?;
                    *minimum = Some(canonical);
                }
                TypeConstraintCommand::Maximum => {
                    let canonical = ratio_bound_to_canonical_rational(args, "maximum", units)?;
                    sync_ratio_units_from_canonical(
                        units,
                        &canonical,
                        UnitConstraintField::Maximum,
                    )?;
                    *maximum = Some(canonical);
                }
                TypeConstraintCommand::Help => {
                    apply_type_help_command(help, args)?;
                }
                TypeConstraintCommand::Suggest | TypeConstraintCommand::Fill => {
                    let (target, cmd) = match command {
                        TypeConstraintCommand::Suggest => (&mut *declared_suggestion, "suggest"),
                        TypeConstraintCommand::Fill => (&mut *declared_fill, "fill"),
                        _ => unreachable!("BUG: only Suggest or Fill in this arm"),
                    };
                    let lit = require_literal(args, cmd)?;
                    reject_calendar_for_suggestion(
                        lit,
                        type_name,
                        SuggestionExpectation::Ratio,
                        None,
                    )?;
                    match lit {
                        crate::literals::Value::NumberWithUnit(_, unit_name) => {
                            let element_spec = TypeSpecification::Ratio {
                                decimals: *decimals,
                                minimum: minimum.clone(),
                                maximum: maximum.clone(),
                                units: units.clone(),
                                help: help.clone(),
                            };
                            let value = parser_value_to_value_kind(lit, &element_spec)?;
                            sync_ratio_suggestion_units(units, &value)?;
                            *target = Some(RawSuggestion::UnitBound {
                                value,
                                unit_name: unit_name.clone(),
                            });
                        }
                        other => {
                            return Err(format!(
                                "suggest requires a ratio literal with a unit, got {}. Please provide a ratio value with a unit, for example `-> suggest 25%`.",
                                value_kind_name(other)
                            ));
                        }
                    }
                }
                _ => {
                    return Err(format!(
                        "Invalid command '{}' for ratio type. Valid commands: unit, minimum, maximum, decimals, help, suggest, fill",
                        command
                    ));
                }
            },
            TypeSpecification::RatioRange {
                lower,
                upper,
                minimum,
                maximum,
                units,
                help,
            } => match command {
                TypeConstraintCommand::Unit => {
                    let (unit_name, value_dec) = match args {
                        [CommandArg::Label(name), CommandArg::UnitExpr(crate::parsing::ast::UnitArg::Factor(v))] => {
                            (name.clone(), *v)
                        }
                        _ => {
                            return Err(
                                "unit requires a unit name followed by a numeric conversion factor (e.g., 'unit percent 100'). Compound unit expressions are not supported for ratio range types."
                                    .to_string(),
                            );
                        }
                    };
                    let value = crate::computation::rational::decimal_to_rational(value_dec)
                        .map_err(|e| {
                            format!(
                                "ratio unit value is not exactly representable as a rational: {e}"
                            )
                        })?;
                    if let Some(existing) = units.0.iter().find(|u| u.name == unit_name) {
                        if existing.value != value {
                            return Err(format!(
                                "Unit '{unit_name}' is already defined in this type's inherited units; \
                                 cannot change factor. Add a new unit name instead."
                            ));
                        }
                    } else {
                        units.0.push(RatioUnit {
                            name: unit_name,
                            value,
                            minimum: None,
                            maximum: None,
                            suggestion_magnitude: None,
                        });
                    }
                }
                TypeConstraintCommand::Lower => {
                    *lower = Some(ratio_bound_to_canonical_rational(args, "lower", units)?);
                }
                TypeConstraintCommand::Upper => {
                    *upper = Some(ratio_bound_to_canonical_rational(args, "upper", units)?);
                }
                TypeConstraintCommand::Minimum => {
                    let width = ratio_bound_to_canonical_rational(args, "minimum", units)?;
                    reject_negative_width_magnitude(&width, "minimum")?;
                    *minimum = Some(width);
                }
                TypeConstraintCommand::Maximum => {
                    let width = ratio_bound_to_canonical_rational(args, "maximum", units)?;
                    reject_negative_width_magnitude(&width, "maximum")?;
                    *maximum = Some(width);
                }
                TypeConstraintCommand::Help => {
                    apply_type_help_command(help, args)?;
                }
                TypeConstraintCommand::Suggest | TypeConstraintCommand::Fill => {
                    let target = match command {
                        TypeConstraintCommand::Suggest => &mut *declared_suggestion,
                        TypeConstraintCommand::Fill => &mut *declared_fill,
                        _ => unreachable!("BUG: only Suggest or Fill in this arm"),
                    };
                    let (left, right) = require_suggestion_range_endpoints(
                        args,
                        type_name,
                        SuggestionExpectation::RatioRange,
                        None,
                    )?;
                    let element_spec = TypeSpecification::RatioRange {
                        lower: lower.clone(),
                        upper: upper.clone(),
                        minimum: minimum.clone(),
                        maximum: maximum.clone(),
                        units: units.clone(),
                        help: help.clone(),
                    }
                    .element_from_range()
                    .expect("BUG: RatioRange must define element_from_range");
                    let left = lift_range_endpoint(left, &element_spec)?;
                    let right = lift_range_endpoint(right, &element_spec)?;
                    if !left.lemma_type.is_ratio() || !right.lemma_type.is_ratio() {
                        return Err(
                            "Please provide a ratio range, for example `-> suggest 10%...50%`."
                                .to_string(),
                        );
                    }
                    let value =
                        ValueKind::Range(Box::new(left.to_literal()), Box::new(right.to_literal()));
                    *target = match (
                        left.lemma_type.measure_binding_unit.as_ref(),
                        right.lemma_type.measure_binding_unit.as_ref(),
                    ) {
                        (Some(left_unit), Some(right_unit)) if left_unit == right_unit => {
                            Some(RawSuggestion::UnitBound {
                                value,
                                unit_name: left_unit.clone(),
                            })
                        }
                        _ => Some(RawSuggestion::Value(value)),
                    };
                }
                _ => {
                    return Err(format!(
                        "Invalid command '{}' for ratio range type. Valid commands: unit, lower, upper, minimum, maximum, help, suggest, fill",
                        command
                    ));
                }
            },
            TypeSpecification::Text {
                length,
                options,
                help,
            } => match command {
                TypeConstraintCommand::Option => {
                    if args.len() != 1 {
                        return Err("option takes exactly one argument".to_string());
                    }
                    options.push(option_name(&args[0], "option")?);
                }
                TypeConstraintCommand::Options => {
                    let mut collected = Vec::with_capacity(args.len());
                    for arg in args {
                        collected.push(option_name(arg, "options")?);
                    }
                    *options = collected;
                }
                TypeConstraintCommand::Length => {
                    let d = require_decimal_literal(args, "length")?;
                    *length = Some(decimal_to_usize(d, "length")?);
                }
                TypeConstraintCommand::Help => {
                    apply_type_help_command(help, args)?;
                }
                TypeConstraintCommand::Suggest | TypeConstraintCommand::Fill => {
                    let (target, cmd) = match command {
                        TypeConstraintCommand::Suggest => (&mut *declared_suggestion, "suggest"),
                        TypeConstraintCommand::Fill => (&mut *declared_fill, "fill"),
                        _ => unreachable!("BUG: only Suggest or Fill in this arm"),
                    };
                    let lit = require_literal(args, cmd)?;
                    reject_calendar_for_suggestion(
                        lit,
                        type_name,
                        SuggestionExpectation::Text,
                        None,
                    )?;
                    match lit {
                        crate::literals::Value::Text(s) => {
                            *target = Some(RawSuggestion::Value(ValueKind::Text(s.clone())));
                        }
                        _ => {
                            return Err(
                                "Please provide a text value in double quotes, for example `-> suggest \"my default value\"`."
                                    .to_string(),
                            );
                        }
                    }
                }
                _ => {
                    return Err(format!(
                        "Invalid command '{}' for text type. Valid commands: options, length, help, suggest, fill",
                        command
                    ));
                }
            },
            TypeSpecification::Date {
                minimum,
                maximum,
                help,
            } => match command {
                TypeConstraintCommand::Minimum => {
                    let dt = require_date_literal(args, "minimum")?;
                    *minimum = Some(dt);
                }
                TypeConstraintCommand::Maximum => {
                    let dt = require_date_literal(args, "maximum")?;
                    *maximum = Some(dt);
                }
                TypeConstraintCommand::Help => {
                    apply_type_help_command(help, args)?;
                }
                TypeConstraintCommand::Suggest | TypeConstraintCommand::Fill => {
                    let (target, cmd) = match command {
                        TypeConstraintCommand::Suggest => (&mut *declared_suggestion, "suggest"),
                        TypeConstraintCommand::Fill => (&mut *declared_fill, "fill"),
                        _ => unreachable!("BUG: only Suggest or Fill in this arm"),
                    };
                    let lit = require_literal(args, cmd)?;
                    reject_calendar_for_suggestion(
                        lit,
                        type_name,
                        SuggestionExpectation::Date,
                        None,
                    )?;
                    match lit {
                        crate::literals::Value::Date(dt) => {
                            *target = Some(RawSuggestion::Value(ValueKind::Date(
                                date_time_to_semantic(dt),
                            )));
                        }
                        _ => {
                            return Err(
                                "Please provide a date, for example `-> suggest 2024-06-15`."
                                    .to_string(),
                            );
                        }
                    }
                }
                _ => {
                    return Err(format!(
                        "Invalid command '{}' for date type. Valid commands: minimum, maximum, help, suggest, fill",
                        command
                    ));
                }
            },
            TypeSpecification::DateRange {
                lower,
                upper,
                minimum,
                maximum,
                help,
            } => match command {
                TypeConstraintCommand::Lower => {
                    *lower = Some(require_date_literal(args, "lower")?);
                }
                TypeConstraintCommand::Upper => {
                    *upper = Some(require_date_literal(args, "upper")?);
                }
                TypeConstraintCommand::Minimum => {
                    *minimum = Some(parse_unresolved_width_bound(args, "minimum")?);
                }
                TypeConstraintCommand::Maximum => {
                    *maximum = Some(parse_unresolved_width_bound(args, "maximum")?);
                }
                TypeConstraintCommand::Help => {
                    apply_type_help_command(help, args)?;
                }
                TypeConstraintCommand::Suggest | TypeConstraintCommand::Fill => {
                    let target = match command {
                        TypeConstraintCommand::Suggest => &mut *declared_suggestion,
                        TypeConstraintCommand::Fill => &mut *declared_fill,
                        _ => unreachable!("BUG: only Suggest or Fill in this arm"),
                    };
                    let (left, right) = require_suggestion_range_endpoints(
                        args,
                        type_name,
                        SuggestionExpectation::DateRange,
                        None,
                    )?;
                    let left = literal_value_from_parser_value(left)?;
                    let right = literal_value_from_parser_value(right)?;
                    if !left.lemma_type.is_date() || !right.lemma_type.is_date() {
                        return Err(
                            "Please provide a date range, for example `-> suggest 2024-01-01...2024-12-31`."
                                .to_string(),
                        );
                    }
                    *target = Some(RawSuggestion::Value(ValueKind::Range(
                        Box::new(left.to_literal()),
                        Box::new(right.to_literal()),
                    )));
                }
                _ => {
                    return Err(format!(
                        "Invalid command '{}' for date range type. Valid commands: lower, upper, minimum, maximum, help, suggest, fill",
                        command
                    ));
                }
            },
            TypeSpecification::Time {
                minimum,
                maximum,
                help,
            } => match command {
                TypeConstraintCommand::Minimum => {
                    let t = require_time_literal(args, "minimum")?;
                    *minimum = Some(t);
                }
                TypeConstraintCommand::Maximum => {
                    let t = require_time_literal(args, "maximum")?;
                    *maximum = Some(t);
                }
                TypeConstraintCommand::Help => {
                    apply_type_help_command(help, args)?;
                }
                TypeConstraintCommand::Suggest | TypeConstraintCommand::Fill => {
                    let (target, cmd) = match command {
                        TypeConstraintCommand::Suggest => (&mut *declared_suggestion, "suggest"),
                        TypeConstraintCommand::Fill => (&mut *declared_fill, "fill"),
                        _ => unreachable!("BUG: only Suggest or Fill in this arm"),
                    };
                    let lit = require_literal(args, cmd)?;
                    reject_calendar_for_suggestion(
                        lit,
                        type_name,
                        SuggestionExpectation::Time,
                        None,
                    )?;
                    match lit {
                        crate::literals::Value::Time(t) => {
                            *target =
                                Some(RawSuggestion::Value(ValueKind::Time(time_to_semantic(t))));
                        }
                        _ => {
                            return Err(
                                "Please provide a time, for example `-> suggest 09:00:00`."
                                    .to_string(),
                            );
                        }
                    }
                }
                _ => {
                    return Err(format!(
                        "Invalid command '{}' for time type. Valid commands: minimum, maximum, help, suggest, fill",
                        command
                    ));
                }
            },
            TypeSpecification::TimeRange {
                lower,
                upper,
                minimum,
                maximum,
                help,
            } => match command {
                TypeConstraintCommand::Lower => {
                    *lower = Some(require_time_literal(args, "lower")?);
                }
                TypeConstraintCommand::Upper => {
                    *upper = Some(require_time_literal(args, "upper")?);
                }
                TypeConstraintCommand::Minimum => {
                    *minimum = Some(parse_unresolved_width_bound(args, "minimum")?);
                }
                TypeConstraintCommand::Maximum => {
                    *maximum = Some(parse_unresolved_width_bound(args, "maximum")?);
                }
                TypeConstraintCommand::Help => {
                    apply_type_help_command(help, args)?;
                }
                TypeConstraintCommand::Suggest | TypeConstraintCommand::Fill => {
                    let target = match command {
                        TypeConstraintCommand::Suggest => &mut *declared_suggestion,
                        TypeConstraintCommand::Fill => &mut *declared_fill,
                        _ => unreachable!("BUG: only Suggest or Fill in this arm"),
                    };
                    let (left, right) = require_suggestion_range_endpoints(
                        args,
                        type_name,
                        SuggestionExpectation::TimeRange,
                        None,
                    )?;
                    let left = literal_value_from_parser_value(left)?;
                    let right = literal_value_from_parser_value(right)?;
                    if !left.lemma_type.is_time() || !right.lemma_type.is_time() {
                        return Err(
                            "Please provide a time range, for example `-> suggest 09:00...17:00`."
                                .to_string(),
                        );
                    }
                    *target = Some(RawSuggestion::Value(ValueKind::Range(
                        Box::new(left.to_literal()),
                        Box::new(right.to_literal()),
                    )));
                }
                _ => {
                    return Err(format!(
                        "Invalid command '{}' for time range type. Valid commands: lower, upper, minimum, maximum, help, suggest, fill",
                        command
                    ));
                }
            },
            TypeSpecification::MeasureRange {
                lower,
                upper,
                minimum,
                maximum,
                units,
                decomposition,
                help,
            } => match command {
                TypeConstraintCommand::Unit => {
                    let (unit_name, value, derived_measure_factors) = match args {
                        [CommandArg::Label(name), CommandArg::UnitExpr(crate::parsing::ast::UnitArg::Factor(v))] => {
                            (name.clone(), *v, Vec::new())
                        }
                        [CommandArg::Label(name), CommandArg::UnitExpr(crate::parsing::ast::UnitArg::Expr(
                            prefix,
                            factors,
                        ))] => {
                            let raw: Vec<(String, i32)> = factors
                                .iter()
                                .map(|f| (f.measure_ref.clone(), f.exp))
                                .collect();
                            (name.clone(), *prefix, raw)
                        }
                        _ => {
                            return Err(
                                "unit requires a unit name followed by a conversion factor or compound unit expression (e.g., 'unit eur 1.00' or 'unit mps meter/second')"
                                    .to_string(),
                            );
                        }
                    };
                    if let Some(existing) = units.0.iter().find(|u| u.name == unit_name) {
                        let new_factor = crate::computation::rational::decimal_to_rational(value)
                            .map_err(|failure| failure.to_string())?;
                        if existing.factor != new_factor
                            || existing.derived_measure_factors != derived_measure_factors
                        {
                            return Err(format!(
                                "Unit '{unit_name}' is already defined in this type's inherited units; \
                                 cannot change factor or decomposition. Add a new unit name instead."
                            ));
                        }
                    } else {
                        units.0.push(MeasureUnit::from_decimal_factor(
                            unit_name,
                            value,
                            derived_measure_factors,
                        )?);
                    }
                }
                TypeConstraintCommand::Lower => {
                    *lower = Some(parse_measure_declared_bound(
                        args, "lower", units, type_name,
                    )?);
                }
                TypeConstraintCommand::Upper => {
                    *upper = Some(parse_measure_declared_bound(
                        args, "upper", units, type_name,
                    )?);
                }
                TypeConstraintCommand::Minimum => {
                    let width = parse_measure_declared_bound(args, "minimum", units, type_name)?;
                    reject_negative_width_magnitude(&width.0, "minimum")?;
                    *minimum = Some(width);
                }
                TypeConstraintCommand::Maximum => {
                    let width = parse_measure_declared_bound(args, "maximum", units, type_name)?;
                    reject_negative_width_magnitude(&width.0, "maximum")?;
                    *maximum = Some(width);
                }
                TypeConstraintCommand::Help => {
                    apply_type_help_command(help, args)?;
                }
                TypeConstraintCommand::Suggest | TypeConstraintCommand::Fill => {
                    let target = match command {
                        TypeConstraintCommand::Suggest => &mut *declared_suggestion,
                        TypeConstraintCommand::Fill => &mut *declared_fill,
                        _ => unreachable!("BUG: only Suggest or Fill in this arm"),
                    };
                    let (left, right) = require_suggestion_range_endpoints(
                        args,
                        type_name,
                        SuggestionExpectation::MeasureRange,
                        Some(units),
                    )?;
                    let element_spec = TypeSpecification::MeasureRange {
                        lower: lower.clone(),
                        upper: upper.clone(),
                        minimum: minimum.clone(),
                        maximum: maximum.clone(),
                        units: units.clone(),
                        decomposition: decomposition.clone(),
                        help: help.clone(),
                    }
                    .element_from_range()
                    .expect("BUG: MeasureRange must define element_from_range");
                    let left = lift_range_endpoint(left, &element_spec)?;
                    let right = lift_range_endpoint(right, &element_spec)?;
                    if !left.lemma_type.is_measure() || !right.lemma_type.is_measure() {
                        return Err(format!(
                            "Please provide a range with units valid for '{type_name}', for example `-> suggest 30 kilogram...35 kilogram`."
                        ));
                    }
                    *target = Some(RawSuggestion::Value(ValueKind::Range(
                        Box::new(left.to_literal()),
                        Box::new(right.to_literal()),
                    )));
                }
                _ => {
                    return Err(format!(
                        "Invalid command '{}' for measure range type. Valid commands: unit, lower, upper, minimum, maximum, help, suggest, fill",
                        command
                    ));
                }
            },
            TypeSpecification::Veto { .. } => {
                return Err(format!(
                    "Invalid command '{}' for veto type. Veto is not a user-declarable type and cannot have constraints",
                    command
                ));
            }
            TypeSpecification::Undetermined => {
                return Err(format!(
                    "Invalid command '{}' for undetermined sentinel type. Undetermined is an internal type used during type inference and cannot have constraints",
                    command
                ));
            }
        }
        Ok(())
    }
}

/// Parse a "number unit" string into a Measure or Ratio value according to the type.
/// Caller must have obtained the TypeSpecification via unit_index from the unit in the string.
pub fn parse_number_unit(
    value_str: &str,
    type_spec: &TypeSpecification,
) -> Result<crate::parsing::ast::Value, String> {
    use crate::literals::{NumberWithUnit, RatioLiteral};
    use crate::parsing::ast::Value;

    let trimmed = value_str.trim();
    match type_spec {
        TypeSpecification::Measure { units, .. } => {
            if units.is_empty() {
                unreachable!(
                    "BUG: Measure type has no units; should have been validated during planning"
                );
            }
            match trimmed.parse::<NumberWithUnit>() {
                Ok(n) => {
                    let unit = units.get(&n.1).map_err(|e| e.to_string())?;
                    Ok(Value::NumberWithUnit(n.0, unit.name.clone()))
                }
                Err(e) => {
                    if trimmed.split_whitespace().count() == 1 && !trimmed.is_empty() {
                        let valid: Vec<&str> = units.iter().map(|u| u.name.as_str()).collect();
                        let example_unit = units
                            .iter()
                            .next()
                            .expect("BUG: units non-empty after guard")
                            .name
                            .as_str();
                        Err(format!(
                            "Measure value must include a unit, for example: '{} {}'. Valid units: {}.",
                            trimmed,
                            example_unit,
                            valid.join(", ")
                        ))
                    } else {
                        Err(e)
                    }
                }
            }
        }
        TypeSpecification::Ratio { units, .. } => {
            if units.is_empty() {
                unreachable!(
                    "BUG: Ratio type has no units; should have been validated during planning"
                );
            }
            match trimmed.parse::<RatioLiteral>()? {
                RatioLiteral::Bare(_) => {
                    Err("Ratio value requires a unit (e.g. '50%', '500 basis_points').".to_string())
                }
                RatioLiteral::Percent(n) => {
                    let unit = units.get("percent").map_err(|e| e.to_string())?;
                    Ok(Value::NumberWithUnit(n, unit.name.clone()))
                }
                RatioLiteral::Permille(n) => {
                    let unit = units.get("permille").map_err(|e| e.to_string())?;
                    Ok(Value::NumberWithUnit(n, unit.name.clone()))
                }
                RatioLiteral::Named { value, unit } => {
                    let resolved = units.get(&unit).map_err(|e| e.to_string())?;
                    Ok(Value::NumberWithUnit(value, resolved.name.clone()))
                }
            }
        }
        _ => Err("parse_number_unit only accepts Measure or Ratio type".to_string()),
    }
}

/// Parse a string value according to a TypeSpecification.
/// Used to parse runtime user input into typed values.
pub fn parse_value_from_string(
    value_str: &str,
    type_spec: &TypeSpecification,
    source: &Source,
) -> Result<crate::parsing::ast::Value, Error> {
    use crate::parsing::ast::Value;

    let to_err = |msg: String| Error::validation(msg, Some(source.clone()), None::<String>);

    let parse_range_value = |element_spec: TypeSpecification| -> Result<Value, Error> {
        let (left_str, right_str) = value_str.split_once("...").ok_or_else(|| {
            to_err("Range value must use '...' between the two endpoints".to_string())
        })?;
        if left_str.trim().is_empty() || right_str.trim().is_empty() {
            return Err(to_err(
                "Range value must contain a non-empty left and right endpoint".to_string(),
            ));
        }
        let left = parse_value_from_string(left_str.trim(), &element_spec, source)?;
        let right = parse_value_from_string(right_str.trim(), &element_spec, source)?;
        Ok(Value::Range(Box::new(left), Box::new(right)))
    };

    match type_spec {
        TypeSpecification::Text { .. } => value_str
            .parse::<crate::literals::TextLiteral>()
            .map(|t| Value::Text(t.0))
            .map_err(to_err),
        TypeSpecification::Number { .. } => value_str
            .parse::<crate::literals::NumberLiteral>()
            .map(|n| Value::Number(n.0))
            .map_err(to_err),
        TypeSpecification::Measure { .. } => {
            parse_number_unit(value_str, type_spec).map_err(to_err)
        }
        TypeSpecification::Boolean { .. } => value_str
            .parse::<BooleanValue>()
            .map(Value::Boolean)
            .map_err(to_err),
        TypeSpecification::Date { .. } => {
            let date = value_str.parse::<DateTimeValue>().map_err(to_err)?;
            Ok(Value::Date(date))
        }
        TypeSpecification::Time { .. } => {
            let time = value_str.parse::<TimeValue>().map_err(to_err)?;
            Ok(Value::Time(time))
        }
        TypeSpecification::Ratio { .. } => {
            parse_number_unit(value_str, type_spec).map_err(to_err)
        }
        TypeSpecification::NumberRange { .. }
        | TypeSpecification::MeasureRange { .. }
        | TypeSpecification::DateRange { .. }
        | TypeSpecification::TimeRange { .. }
        | TypeSpecification::RatioRange { .. } => {
            let element_spec = range_element_type_specification(type_spec).unwrap_or_else(|| {
                unreachable!("BUG: range_element_type_specification missing arm for known range type")
            });
            parse_range_value(element_spec)
        }
        TypeSpecification::Veto { .. } => Err(to_err(
            "Veto type cannot be parsed from string".to_string(),
        )),
        TypeSpecification::Undetermined => unreachable!(
            "BUG: parse_value_from_string called with Undetermined sentinel type; this type exists only during type inference"
        ),
    }
}

// -----------------------------------------------------------------------------
// Semantic value types (no parser dependency - used by evaluation, inversion, etc.)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticCalendarUnit {
    Month,
    Year,
}

impl fmt::Display for SemanticCalendarUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            SemanticCalendarUnit::Month => "month",
            SemanticCalendarUnit::Year => "year",
        };
        write!(f, "{}", s)
    }
}

pub fn semantic_calendar_unit_from_unit_name(unit_name: &str) -> SemanticCalendarUnit {
    match unit_name {
        "month" => SemanticCalendarUnit::Month,
        "year" => SemanticCalendarUnit::Year,
        other => unreachable!(
            "BUG: calendar measure signature unit must be month or year, got '{other}'"
        ),
    }
}

pub fn semantic_calendar_unit_from_measure_signature(
    signature: &[(String, i32)],
) -> SemanticCalendarUnit {
    let unit_name = signature
        .first()
        .map(|(name, _)| name.as_str())
        .expect("BUG: calendar measure must carry a unit signature");
    semantic_calendar_unit_from_unit_name(unit_name)
}

pub fn semantic_calendar_unit_from_measure_type(lemma_type: &LemmaType) -> SemanticCalendarUnit {
    if !lemma_type.is_calendar_like() {
        unreachable!(
            "BUG: semantic_calendar_unit_from_measure_type called on non-calendar type {}",
            lemma_type.name()
        );
    }
    let signature = lemma_type.measure_runtime_signature();
    if signature.is_empty() {
        return SemanticCalendarUnit::Month;
    }
    semantic_calendar_unit_from_measure_signature(&signature)
}

/// Target type for `as` casts (semantic; used by evaluation/computation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SemanticConversionTarget {
    Type(PrimitiveKind),
    /// `number as eur` — construct, convert, relabel, or range-span into `unit_name`.
    Unit {
        unit_name: String,
        /// Measure/ratio type resolved in the spec where the conversion was written.
        owning_type: Arc<LemmaType>,
    },
}

impl std::hash::Hash for SemanticConversionTarget {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Self::Type(kind) => {
                0u8.hash(state);
                kind.hash(state);
            }
            Self::Unit {
                unit_name,
                owning_type,
            } => {
                1u8.hash(state);
                unit_name.hash(state);
                owning_type.hash(state);
            }
        }
    }
}

impl SemanticConversionTarget {}

impl fmt::Display for SemanticConversionTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SemanticConversionTarget::Type(kind) => write!(f, "{kind}"),
            SemanticConversionTarget::Unit { unit_name, .. } => write!(f, "{unit_name}"),
        }
    }
}

/// Timezone for semantic date/time values
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemanticTimezone {
    pub offset_hours: i8,
    pub offset_minutes: u8,
}

impl fmt::Display for SemanticTimezone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.offset_hours == 0 && self.offset_minutes == 0 {
            write!(f, "Z")
        } else {
            let sign = if self.offset_hours >= 0 { "+" } else { "-" };
            let hour = self.offset_hours.abs();
            write!(f, "{}{:02}:{:02}", sign, hour, self.offset_minutes)
        }
    }
}

impl Serialize for SemanticTimezone {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for SemanticTimezone {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl FromStr for SemanticTimezone {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let tz = TimezoneValue::from_str(s)?;
        Ok(Self {
            offset_hours: tz.offset_hours,
            offset_minutes: tz.offset_minutes,
        })
    }
}

/// Time-of-day for semantic values
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemanticTime {
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub microsecond: u32,
    pub timezone: Option<SemanticTimezone>,
}

impl fmt::Display for SemanticTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}:{:02}", self.hour, self.minute, self.second)?;
        if self.microsecond != 0 {
            write!(f, ".{:06}", self.microsecond)?;
        }
        if let Some(timezone) = &self.timezone {
            write!(f, "{}", timezone)?;
        }
        Ok(())
    }
}

impl Serialize for SemanticTime {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for SemanticTime {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl FromStr for SemanticTime {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(time_to_semantic(&TimeValue::from_str(s)?))
    }
}

/// Date-time for semantic values
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SemanticDateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub microsecond: u32,
    pub timezone: Option<SemanticTimezone>,
}

impl fmt::Display for SemanticDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let has_time = self.hour != 0
            || self.minute != 0
            || self.second != 0
            || self.microsecond != 0
            || self.timezone.is_some();
        if !has_time {
            write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
        } else {
            write!(
                f,
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
                self.year, self.month, self.day, self.hour, self.minute, self.second
            )?;
            if self.microsecond != 0 {
                write!(f, ".{:06}", self.microsecond)?;
            }
            if let Some(tz) = &self.timezone {
                write!(f, "{}", tz)?;
            }
            Ok(())
        }
    }
}

impl Serialize for SemanticDateTime {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for SemanticDateTime {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl FromStr for SemanticDateTime {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(date_time_to_semantic(&DateTimeValue::from_str(s)?))
    }
}

/// Default captured during type constraint application, before measure unit factors are final.
/// Converted into [`ValueKind`] after `resolve_measure_decompositions` (or immediately for
/// reference-local defaults, which run after that pass).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RawSuggestion {
    Value(ValueKind),
    /// Measure suggest/fill: magnitude in the written unit (canonicalized later).
    Measure {
        magnitude: RationalInteger,
        unit_name: String,
    },
    /// Ratio (or other) suggest/fill that already holds a canonical [`ValueKind`]
    /// plus the written unit name for display binding.
    UnitBound {
        value: ValueKind,
        unit_name: String,
    },
}

/// Canonical suggestion/fill value plus the written measure unit when one was declared.
///
/// `measure_binding_unit` is the unit from `-> suggest 100 inr` / `-> fill 5 eur`; Show
/// display stamps it so the one-liner matches the written unit, not the type's first unit.
/// Shared via [`Arc`] so value-table clones bump a refcount instead of allocating.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BoundValueKind {
    pub value: ValueKind,
    pub measure_binding_unit: Option<Arc<str>>,
}

impl BoundValueKind {
    pub fn unbound(value: ValueKind) -> Self {
        Self {
            value,
            measure_binding_unit: None,
        }
    }

    pub fn with_binding(value: ValueKind, measure_binding_unit: Option<Arc<str>>) -> Self {
        Self {
            value,
            measure_binding_unit,
        }
    }

    pub fn to_literal(&self) -> LiteralValue {
        LiteralValue {
            value: self.value.clone(),
        }
    }

    /// Written-unit agreement for ± on same-family measure/ratio: equal keep; one Some keep that;
    /// conflict → left wins.
    #[must_use]
    pub fn agree_or_left_binding(
        left: Option<&Arc<str>>,
        right: Option<&Arc<str>>,
    ) -> Option<Arc<str>> {
        match (left, right) {
            (Some(a), Some(b)) if a.as_ref() == b.as_ref() => Some(Arc::clone(a)),
            (Some(a), Some(_)) => Some(Arc::clone(a)),
            (Some(a), None) => Some(Arc::clone(a)),
            (None, Some(b)) => Some(Arc::clone(b)),
            (None, None) => None,
        }
    }
}

pub fn bound_value_kind_from_raw_suggestion(
    raw: RawSuggestion,
    specifications: &TypeSpecification,
    type_name: &str,
) -> Result<BoundValueKind, String> {
    match raw {
        RawSuggestion::Value(vk) => Ok(BoundValueKind::unbound(vk)),
        RawSuggestion::UnitBound { value, unit_name } => Ok(BoundValueKind {
            value,
            measure_binding_unit: Some(Arc::from(unit_name)),
        }),
        RawSuggestion::Measure {
            magnitude,
            unit_name,
        } => {
            let TypeSpecification::Measure { units, .. } = specifications else {
                return Err(format!(
                    "BUG: RawSuggestion::Measure for non-measure type '{type_name}'"
                ));
            };
            let canonical = measure_declared_bound_to_canonical(
                &magnitude, &unit_name, units, type_name, "suggest",
            )?;
            Ok(BoundValueKind {
                value: ValueKind::Measure(canonical),
                measure_binding_unit: Some(Arc::from(unit_name)),
            })
        }
    }
}

/// Display one-liner unit: `as` / schema > settled > fill > suggest > none (first declared).
#[must_use]
pub fn display_binding_unit(
    schema: &LemmaType,
    settled: Option<&BoundValueKind>,
    fill: Option<&BoundValueKind>,
    suggestion: Option<&BoundValueKind>,
) -> Option<String> {
    if let Some(unit) = schema.measure_binding_unit.clone() {
        return Some(unit);
    }
    if let Some(unit) = settled.and_then(|bound| bound.measure_binding_unit.as_ref()) {
        return Some(unit.to_string());
    }
    if let Some(unit) = fill.and_then(|bound| bound.measure_binding_unit.as_ref()) {
        return Some(unit.to_string());
    }
    suggestion
        .and_then(|bound| bound.measure_binding_unit.as_ref())
        .map(|unit| unit.to_string())
}

/// Schema type stamped with [`display_binding_unit`] when a binding wins.
///
/// Clones only when the winning unit differs from what `schema` already carries.
#[must_use]
pub fn lemma_type_with_display_binding(
    schema: &LemmaType,
    settled: Option<&BoundValueKind>,
    fill: Option<&BoundValueKind>,
    suggestion: Option<&BoundValueKind>,
) -> LemmaType {
    match display_binding_unit(schema, settled, fill, suggestion) {
        Some(unit) if schema.measure_binding_unit.as_deref() != Some(unit.as_str()) => {
            schema.clone().with_measure_binding_unit(unit)
        }
        Some(_) | None => schema.clone(),
    }
}

/// Like [`lemma_type_with_display_binding`] but reuses `schema` when no override.
#[must_use]
pub fn lemma_type_arc_with_display_binding(
    schema: &Arc<LemmaType>,
    settled: Option<&BoundValueKind>,
    fill: Option<&BoundValueKind>,
    suggestion: Option<&BoundValueKind>,
) -> Arc<LemmaType> {
    match display_binding_unit(schema.as_ref(), settled, fill, suggestion) {
        Some(unit) if schema.measure_binding_unit.as_deref() != Some(unit.as_str()) => {
            Arc::new(schema.as_ref().clone().with_measure_binding_unit(unit))
        }
        Some(_) | None => Arc::clone(schema),
    }
}

/// Value payload (shape of a literal). No type attached.
/// Measure unit is required; Ratio unit is optional (see plan ratio-units-optional.md).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ValueKind {
    Number(RationalInteger),
    /// Measure: magnitude in canonical (base-unit) space.
    ///
    /// At bind time the user-facing value is multiplied by `unit.factor` to produce
    /// the stored magnitude; API unit maps divide back. Unit/signature identity
    /// comes from the node/`DataDefinition` type via [`LemmaType::measure_runtime_signature`].
    Measure(RationalInteger),
    Text(String),
    Date(SemanticDateTime),
    Time(SemanticTime),
    Boolean(bool),
    /// Ratio: canonical magnitude. Display unit comes from the node/`DataDefinition` type.
    Ratio(RationalInteger),
    Range(Box<LiteralValue>, Box<LiteralValue>),
}

impl ValueKind {
    /// Decimal magnitude for numeric variants (number, measure, ratio).
    pub fn as_decimal_magnitude(&self) -> Result<Decimal, String> {
        match self {
            ValueKind::Number(n) | ValueKind::Measure(n) | ValueKind::Ratio(n) => {
                n.try_to_decimal().map_err(|failure| failure.to_string())
            }
            other => Err(format!("expected numeric value kind, got {other}")),
        }
    }

    /// Cheap structural byte-size estimate (no formatting, no rational→decimal).
    ///
    /// Used by resource-limit checks where the exact string length is not
    /// required — only an upper-bound within a constant factor.
    pub fn structural_byte_size(&self) -> usize {
        fn rational_byte_estimate(r: &RationalInteger) -> usize {
            let numer_bytes = (r.numer_magnitude_bits() as usize).div_ceil(8);
            let denom_bytes = (r.denom_magnitude_bits() as usize).div_ceil(8);
            numer_bytes.max(1) + denom_bytes.max(1)
        }
        match self {
            ValueKind::Number(r) => rational_byte_estimate(r),
            ValueKind::Measure(r) => rational_byte_estimate(r),
            ValueKind::Ratio(r) => rational_byte_estimate(r),
            ValueKind::Text(s) => s.len(),
            ValueKind::Date(_) => 30, // "2026-01-01T00:00:00+02:00" upper bound
            ValueKind::Time(_) => 12, // "23:59:59" upper bound
            ValueKind::Boolean(_) => 5, // "false"
            ValueKind::Range(left, right) => {
                left.value.structural_byte_size() + 3 + right.value.structural_byte_size()
            }
        }
    }
}

fn format_rational_magnitude_for_display(rational: &RationalInteger) -> String {
    rational.display_str()
}

fn format_number_with_unit_for_display(rational: &RationalInteger, unit: &str) -> String {
    use crate::parsing::ast::Value;
    match rational.try_to_decimal() {
        Ok(decimal) => format!("{}", Value::NumberWithUnit(decimal, unit.to_string())),
        Err(_) => format!("{} {}", rational.display_str(), unit),
    }
}

impl fmt::Display for ValueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValueKind::Number(rational) => {
                write!(f, "{}", format_rational_magnitude_for_display(rational))
            }
            ValueKind::Measure(rational) => {
                write!(f, "{}", format_rational_magnitude_for_display(rational))
            }
            ValueKind::Text(s) => write!(f, "{}", crate::parsing::ast::Value::Text(s.clone())),
            ValueKind::Ratio(rational) => {
                write!(f, "{}", format_rational_magnitude_for_display(rational))
            }
            ValueKind::Date(dt) => write!(f, "{}", dt),
            ValueKind::Time(t) => write!(
                f,
                "{}",
                crate::parsing::ast::Value::Time(crate::parsing::ast::TimeValue {
                    hour: t.hour as u8,
                    minute: t.minute as u8,
                    second: t.second as u8,
                    microsecond: t.microsecond,
                    timezone: t
                        .timezone
                        .as_ref()
                        .map(|tz| crate::parsing::ast::TimezoneValue {
                            offset_hours: tz.offset_hours,
                            offset_minutes: tz.offset_minutes,
                        }),
                })
            ),
            ValueKind::Boolean(b) => write!(f, "{}", b),
            ValueKind::Range(left, right) => write!(f, "{}...{}", left, right),
        }
    }
}

// -----------------------------------------------------------------------------
// Resolved path types (moved from parsing::ast)
// -----------------------------------------------------------------------------

/// A single segment in a resolved path traversal
///
/// Used in both DataPath and RulePath for cross-spec traversal.
/// Each segment contains a `uses` alias, the resolved target repository name
/// (`repository`; `None` = unnamed workspace), and the resolved target spec
/// name (`spec`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PathSegment {
    /// The `uses` alias for this hop (binding key component).
    pub uses: String,
    /// Target repository name from `Arc<LemmaRepository>.name` at resolve.
    /// `None` is the unnamed workspace. Always encoded for postcard (no skip).
    pub repository: Option<String>,
    /// The spec this hop resolves to (resolved during planning)
    pub spec: String,
}

/// Resolved path to a data (created during planning from AST DataReference)
///
/// Represents a fully resolved path through specs to reach a datum.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DataPath {
    /// Path segments (each is a cross-spec step)
    pub segments: Vec<PathSegment>,
    /// Final data name
    pub data: String,
}

impl DataPath {
    /// Create a data path from segments and data name (matches AST DataReference shape)
    pub fn new(segments: Vec<PathSegment>, data: String) -> Self {
        Self { segments, data }
    }

    /// Create a local data path (no cross-spec steps)
    pub fn local(data: String) -> Self {
        Self {
            segments: vec![],
            data,
        }
    }

    /// Dot-separated key used for matching user-provided data values (e.g. `"order.payment_method"`).
    /// Same string as [`Display`]: alias hops only, no repository or spec names.
    pub fn input_key(&self) -> String {
        let mut s = String::new();
        for segment in &self.segments {
            s.push_str(&segment.uses);
            s.push('.');
        }
        s.push_str(&self.data);
        s
    }
}

/// Resolved path to a rule (created during planning from a rule reference).
///
/// Represents a fully resolved path through specs to reach a rule.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RulePath {
    /// Path segments (each is a cross-spec step)
    pub segments: Vec<PathSegment>,
    /// Final rule name
    pub rule: String,
}

impl RulePath {
    /// Create a rule path from segments and rule name.
    pub fn new(segments: Vec<PathSegment>, rule: String) -> Self {
        Self { segments, rule }
    }

    /// Dot-separated key for Show/run (`alias.rule`), twin of [`DataPath::input_key`].
    /// Same string as [`Display`]: alias hops only, no repository or spec names.
    pub fn input_key(&self) -> String {
        if self.segments.is_empty() {
            return self.rule.clone();
        }
        let mut s = String::new();
        for segment in &self.segments {
            s.push_str(&segment.uses);
            s.push('.');
        }
        s.push_str(&self.rule);
        s
    }
}

// -----------------------------------------------------------------------------
// Resolved expression types (created during planning)
// -----------------------------------------------------------------------------

/// Resolved expression (all references resolved to paths, all literals typed)
///
/// Created during planning from AST Expression. All unresolved references
/// are converted to DataPath/RulePath, and all literals are typed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expression {
    pub kind: ExpressionKind,
    pub source_location: Option<Source>,
}

impl Expression {
    /// Create an expression with an optional source location.
    pub fn with_source(kind: ExpressionKind, source_location: Option<Source>) -> Self {
        Self {
            kind,
            source_location,
        }
    }

    /// Collect all DataPath references from this resolved expression tree
    pub fn collect_data_paths(&self, data: &mut std::collections::HashSet<DataPath>) {
        self.kind.collect_data_paths(data);
    }
}

/// Resolved expression kind (only resolved variants, no unresolved references)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpressionKind {
    /// Resolved literal with type (boxed to keep enum small)
    Literal(Box<TypedLiteral>),
    /// Resolved data path
    DataPath(DataPath),
    /// Resolved rule path
    RulePath(RulePath),
    LogicalAnd(Arc<Expression>, Arc<Expression>),
    Arithmetic(Arc<Expression>, ArithmeticComputation, Arc<Expression>),
    Comparison(Arc<Expression>, ComparisonComputation, Arc<Expression>),
    UnitConversion(Arc<Expression>, SemanticConversionTarget),
    LogicalNegation(Arc<Expression>, NegationType),
    MathematicalComputation(MathematicalComputation, Arc<Expression>),
    Veto(VetoExpression),
    /// The `now` keyword — resolved at evaluation to the effective datetime.
    Now,
    /// Date-relative sugar: `<date_expr> in past` / `in future`
    DateRelative(DateRelativeKind, Arc<Expression>),
    /// Calendar-period sugar: `<date_expr> in [past|future] calendar year|month|week`
    DateCalendar(DateCalendarKind, CalendarPeriodUnit, Arc<Expression>),
    RangeLiteral(Arc<Expression>, Arc<Expression>),
    PastFutureRange(DateRelativeKind, Arc<Expression>),
    RangeContainment(Arc<Expression>, Arc<Expression>),
    /// Whether evaluating the operand produced a veto (no value). Parses as `is veto` syntax.
    ResultIsVeto(Arc<Expression>),
    /// Unless structure: (condition, result) pairs in source order; last true condition wins.
    /// First arm is the default (condition is always-true literal).
    Piecewise(Vec<(Arc<Expression>, Arc<Expression>)>),
}

impl ExpressionKind {
    /// Collect all DataPath references from this expression kind
    pub(crate) fn collect_data_paths(&self, data: &mut std::collections::HashSet<DataPath>) {
        match self {
            ExpressionKind::DataPath(fp) => {
                data.insert(fp.clone());
            }
            ExpressionKind::LogicalAnd(left, right) => {
                left.collect_data_paths(data);
                right.collect_data_paths(data);
            }
            ExpressionKind::Arithmetic(left, _, right)
            | ExpressionKind::Comparison(left, _, right)
            | ExpressionKind::RangeLiteral(left, right)
            | ExpressionKind::RangeContainment(left, right) => {
                left.collect_data_paths(data);
                right.collect_data_paths(data);
            }
            ExpressionKind::UnitConversion(inner, _)
            | ExpressionKind::LogicalNegation(inner, _)
            | ExpressionKind::MathematicalComputation(_, inner)
            | ExpressionKind::PastFutureRange(_, inner) => {
                inner.collect_data_paths(data);
            }
            ExpressionKind::DateRelative(_, date_expr) => {
                date_expr.collect_data_paths(data);
            }
            ExpressionKind::DateCalendar(_, _, date_expr) => {
                date_expr.collect_data_paths(data);
            }
            ExpressionKind::Literal(_)
            | ExpressionKind::RulePath(_)
            | ExpressionKind::Veto(_)
            | ExpressionKind::Now => {}
            ExpressionKind::ResultIsVeto(operand) => {
                operand.collect_data_paths(data);
            }
            ExpressionKind::Piecewise(arms) => {
                for (condition, result) in arms {
                    condition.collect_data_paths(data);
                    result.collect_data_paths(data);
                }
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Resolved types and values
// -----------------------------------------------------------------------------

/// Where the custom extension chain is rooted: same spec as this type, or imported from another resolved spec.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TypeDefiningSpec {
    /// Parent type is defined in the same spec as this type.
    Local,
    /// Parent type was resolved from types loaded from another spec.
    Import,
}

/// What this type extends (primitive built-in or custom type by name).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum TypeExtends {
    /// Extends a primitive built-in type (number, boolean, text, etc.)
    Primitive,
    /// Extends a custom type: parent is the immediate parent type name; family is the root of the extension chain (topmost custom type name).
    /// `defining_spec` records whether the parent chain is local or imported from another spec.
    Custom {
        parent: String,
        family: String,
        defining_spec: TypeDefiningSpec,
    },
}

impl PartialEq for TypeExtends {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (TypeExtends::Primitive, TypeExtends::Primitive) => true,
            (
                TypeExtends::Custom {
                    parent: lp,
                    family: lf,
                    defining_spec: ld,
                },
                TypeExtends::Custom {
                    parent: rp,
                    family: rf,
                    defining_spec: rd,
                },
            ) => lp == rp && lf == rf && ld == rd,
            _ => false,
        }
    }
}

impl Eq for TypeExtends {}

impl std::hash::Hash for TypeExtends {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            TypeExtends::Primitive => {
                0u8.hash(state);
            }
            TypeExtends::Custom {
                parent,
                family,
                defining_spec,
            } => {
                1u8.hash(state);
                parent.hash(state);
                family.hash(state);
                defining_spec.hash(state);
            }
        }
    }
}

impl TypeExtends {
    /// Custom extension in the same spec as the defining type (no cross-spec import for the parent chain).
    #[must_use]
    pub fn custom_local(parent: String, family: String) -> Self {
        TypeExtends::Custom {
            parent,
            family,
            defining_spec: TypeDefiningSpec::Local,
        }
    }

    /// Returns the parent type name if this type extends a custom type.
    #[must_use]
    pub fn parent_name(&self) -> Option<&str> {
        match self {
            TypeExtends::Primitive => None,
            TypeExtends::Custom { parent, .. } => Some(parent.as_str()),
        }
    }
}

/// Resolved type after planning
///
/// Contains a type specification and optional name. Created during planning
/// from TypeSpecification in the AST.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LemmaType {
    /// Optional type name (e.g., "age", "temperature")
    pub name: Option<String>,
    /// The type specification (Boolean, Number, Measure, etc.).
    pub specifications: TypeSpecification,
    /// What this type extends (primitive or custom from a spec)
    pub extends: TypeExtends,
    /// Bound display/arithmetic unit for measure values (from literal bind or
    /// signature-index hit). When set, [`LemmaType::measure_runtime_signature`]
    /// returns `[(this, 1)]` instead of the type's canonical unit.
    pub measure_binding_unit: Option<String>,
}

impl LemmaType {
    /// Functional update of the `Measure` payload (units + decomposition).
    /// Non-Measure variants pass through unchanged. The transform receives the owned
    /// units and decomposition and returns the replacements.
    pub fn map_measure<F>(self, f: F) -> Self
    where
        F: FnOnce(
            MeasureUnits,
            Option<BaseMeasureVector>,
        ) -> (MeasureUnits, Option<BaseMeasureVector>),
    {
        let LemmaType {
            name,
            specifications,
            extends,
            measure_binding_unit,
        } = self;
        let specifications = match specifications {
            TypeSpecification::Measure {
                minimum,
                maximum,
                decimals,
                units,
                traits,
                decomposition,
                help,
            } => {
                let (units, decomposition) = f(units, decomposition);
                TypeSpecification::Measure {
                    minimum,
                    maximum,
                    decimals,
                    units,
                    traits,
                    decomposition,
                    help,
                }
            }
            other => other,
        };
        LemmaType {
            name,
            specifications,
            extends,
            measure_binding_unit,
        }
    }

    /// Create a new type with a name
    pub fn new(name: String, specifications: TypeSpecification, extends: TypeExtends) -> Self {
        Self {
            name: Some(name),
            specifications,
            extends,
            measure_binding_unit: None,
        }
    }

    /// Create a type without a name (anonymous/inline type)
    pub fn without_name(specifications: TypeSpecification, extends: TypeExtends) -> Self {
        Self {
            name: None,
            specifications,
            extends,
            measure_binding_unit: None,
        }
    }

    /// Create a primitive type (no name, extends Primitive)
    pub fn primitive(specifications: TypeSpecification) -> Self {
        Self {
            name: None,
            specifications,
            extends: TypeExtends::Primitive,
            measure_binding_unit: None,
        }
    }

    /// Prefer `unit_name` for [`measure_runtime_signature`] (bind / signature-index hit).
    #[must_use]
    pub fn with_measure_binding_unit(mut self, unit_name: impl Into<String>) -> Self {
        self.measure_binding_unit = Some(unit_name.into());
        self
    }

    /// Get the type name, or a default based on the type specification
    pub fn name(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| self.specifications.to_string())
    }

    /// Check if this type is boolean
    pub fn is_boolean(&self) -> bool {
        matches!(&self.specifications, TypeSpecification::Boolean { .. })
    }

    pub fn matches_primitive_kind(&self, kind: PrimitiveKind) -> bool {
        matches!(
            (kind, &self.specifications),
            (PrimitiveKind::Number, TypeSpecification::Number { .. })
                | (PrimitiveKind::Text, TypeSpecification::Text { .. })
                | (PrimitiveKind::Boolean, TypeSpecification::Boolean { .. })
                | (PrimitiveKind::Date, TypeSpecification::Date { .. })
                | (PrimitiveKind::Time, TypeSpecification::Time { .. })
                | (PrimitiveKind::Ratio, TypeSpecification::Ratio { .. })
                | (PrimitiveKind::Measure, TypeSpecification::Measure { .. })
        )
    }

    /// Check if this type is measure
    pub fn is_measure(&self) -> bool {
        matches!(&self.specifications, TypeSpecification::Measure { .. })
    }

    pub fn is_measure_range(&self) -> bool {
        matches!(&self.specifications, TypeSpecification::MeasureRange { .. })
    }

    /// Check if this type is number (dimensionless)
    pub fn is_number(&self) -> bool {
        matches!(&self.specifications, TypeSpecification::Number { .. })
    }

    pub fn is_number_range(&self) -> bool {
        matches!(&self.specifications, TypeSpecification::NumberRange { .. })
    }

    /// Check if this type is numeric (either measure or number)
    pub fn is_numeric(&self) -> bool {
        matches!(
            &self.specifications,
            TypeSpecification::Measure { .. } | TypeSpecification::Number { .. }
        )
    }

    /// Check if this type is text
    pub fn is_text(&self) -> bool {
        matches!(&self.specifications, TypeSpecification::Text { .. })
    }

    /// Check if this type is date
    pub fn is_date(&self) -> bool {
        matches!(&self.specifications, TypeSpecification::Date { .. })
    }

    pub fn is_date_range(&self) -> bool {
        matches!(&self.specifications, TypeSpecification::DateRange { .. })
    }

    pub fn is_time_range(&self) -> bool {
        matches!(&self.specifications, TypeSpecification::TimeRange { .. })
    }

    /// Check if this type is time
    pub fn is_time(&self) -> bool {
        matches!(&self.specifications, TypeSpecification::Time { .. })
    }

    pub fn has_trait_duration(&self) -> bool {
        match &self.specifications {
            TypeSpecification::Measure { traits, .. } => traits.contains(&MeasureTrait::Duration),
            _ => false,
        }
    }

    pub fn is_duration_like_measure(&self) -> bool {
        if !self.is_measure() {
            return false;
        }
        if self.has_trait_duration() {
            return true;
        }
        self.is_anonymous_measure()
            && self
                .measure_type_decomposition()
                .is_some_and(|d| *d == duration_decomposition())
    }

    pub fn is_duration_like(&self) -> bool {
        self.is_duration_like_measure()
    }

    pub fn has_trait_calendar(&self) -> bool {
        match &self.specifications {
            TypeSpecification::Measure { traits, .. } => traits.contains(&MeasureTrait::Calendar),
            _ => false,
        }
    }

    pub fn is_calendar_like_measure(&self) -> bool {
        if !self.is_measure() {
            return false;
        }
        if self.has_trait_calendar() {
            return true;
        }
        self.is_anonymous_measure()
            && self
                .measure_type_decomposition()
                .is_some_and(|d| *d == calendar_decomposition())
    }

    pub fn is_calendar_like(&self) -> bool {
        self.is_calendar_like_measure()
    }

    /// Check if this type is ratio
    pub fn is_ratio(&self) -> bool {
        matches!(&self.specifications, TypeSpecification::Ratio { .. })
    }

    pub fn is_ratio_range(&self) -> bool {
        matches!(&self.specifications, TypeSpecification::RatioRange { .. })
    }

    pub fn is_calendar_measure_range(&self) -> bool {
        matches!(
            &self.specifications,
            TypeSpecification::MeasureRange { decomposition: Some(decomposition), .. }
                if *decomposition == calendar_decomposition()
        )
    }

    pub fn is_calendar_like_range(&self) -> bool {
        self.is_calendar_measure_range()
    }

    pub fn is_range(&self) -> bool {
        matches!(
            &self.specifications,
            TypeSpecification::DateRange { .. }
                | TypeSpecification::TimeRange { .. }
                | TypeSpecification::NumberRange { .. }
                | TypeSpecification::MeasureRange { .. }
                | TypeSpecification::RatioRange { .. }
        )
    }

    /// Check if this type is veto
    pub fn vetoed(&self) -> bool {
        matches!(&self.specifications, TypeSpecification::Veto { .. })
    }

    /// True if this type is the undetermined sentinel (type could not be inferred).
    pub fn is_undetermined(&self) -> bool {
        matches!(&self.specifications, TypeSpecification::Undetermined)
    }

    /// Check if two types have the same base type specification (ignoring constraints)
    pub fn has_same_base_type(&self, other: &LemmaType) -> bool {
        use TypeSpecification::*;
        matches!(
            (&self.specifications, &other.specifications),
            (Boolean { .. }, Boolean { .. })
                | (Number { .. }, Number { .. })
                | (NumberRange { .. }, NumberRange { .. })
                | (Measure { .. }, Measure { .. })
                | (MeasureRange { .. }, MeasureRange { .. })
                | (Text { .. }, Text { .. })
                | (Date { .. }, Date { .. })
                | (DateRange { .. }, DateRange { .. })
                | (Time { .. }, Time { .. })
                | (TimeRange { .. }, TimeRange { .. })
                | (Ratio { .. }, Ratio { .. })
                | (RatioRange { .. }, RatioRange { .. })
                | (Veto { .. }, Veto { .. })
                | (Undetermined, Undetermined)
        )
    }

    /// For measure types, returns the family name (root of the extension chain). For Custom extends, returns the family field; for Primitive, returns the type's own name (the type is the root). For non-measure types, returns None.
    #[must_use]
    pub fn measure_family_name(&self) -> Option<&str> {
        if !self.is_measure() {
            return None;
        }
        match &self.extends {
            TypeExtends::Custom { family, .. } => Some(family.as_str()),
            TypeExtends::Primitive => self.name.as_deref(),
        }
    }

    /// For ratio types, returns the family name (root of the extension chain).
    #[must_use]
    pub fn ratio_family_name(&self) -> Option<&str> {
        if !self.is_ratio() {
            return None;
        }
        match &self.extends {
            TypeExtends::Custom { family, .. } => Some(family.as_str()),
            TypeExtends::Primitive => self.name.as_deref(),
        }
    }

    /// Measure or ratio family root name.
    #[must_use]
    pub(crate) fn unit_family_name(&self) -> Option<&str> {
        self.measure_family_name()
            .or_else(|| self.ratio_family_name())
    }

    /// Same base kind and, for measures / measure ranges, the same dimensions:
    /// the relation under which two types describe the same values. Names and
    /// constraints may differ. RatioRange stays base-kind only (endpoint policy).
    #[must_use]
    pub(crate) fn same_value_type(&self, other: &LemmaType) -> bool {
        if !self.has_same_base_type(other) {
            return false;
        }
        if self.is_measure() {
            return self.measure_type_decomposition() == other.measure_type_decomposition();
        }
        if self.is_measure_range() {
            let self_element = self
                .specifications
                .element_from_range()
                .expect("BUG: MeasureRange always defines element_from_range");
            let other_element = other
                .specifications
                .element_from_range()
                .expect("BUG: MeasureRange always defines element_from_range");
            match (&self_element, &other_element) {
                (
                    TypeSpecification::Measure {
                        decomposition: self_decomp,
                        ..
                    },
                    TypeSpecification::Measure {
                        decomposition: other_decomp,
                        ..
                    },
                ) => self_decomp == other_decomp,
                _ => unreachable!("BUG: element_from_range of MeasureRange yields Measure"),
            }
        } else {
            true
        }
    }

    /// Returns true if both types are measure and belong to the same named measure family.
    #[must_use]
    pub fn same_measure_family(&self, other: &LemmaType) -> bool {
        if !self.is_measure() || !other.is_measure() {
            return false;
        }
        match (self.measure_family_name(), other.measure_family_name()) {
            (Some(self_family), Some(other_family)) => self_family == other_family,
            _ => false,
        }
    }

    #[must_use]
    pub fn compatible_with_anonymous_measure(&self, other: &LemmaType) -> bool {
        if !self.is_measure() || !other.is_measure() {
            return false;
        }
        if !self.is_anonymous_measure() && !other.is_anonymous_measure() {
            return false;
        }
        match (
            self.measure_type_decomposition(),
            other.measure_type_decomposition(),
        ) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }

    /// Create a Veto LemmaType
    pub fn veto_type() -> Self {
        Self::primitive(TypeSpecification::veto())
    }

    /// LemmaType sentinel for undetermined type (used during inference when a type cannot be determined).
    /// Propagates through expressions and is never present in a validated graph.
    pub fn undetermined_type() -> Self {
        Self::primitive(TypeSpecification::Undetermined)
    }

    /// Decimal places for display (Number, Measure, and Ratio). Used by formatters.
    /// Ratio: optional, no default; when None display is normalized (no trailing zeros).
    pub fn decimal_places(&self) -> Option<u8> {
        match &self.specifications {
            TypeSpecification::Number { decimals, .. } => *decimals,
            TypeSpecification::Measure { decimals, .. } => *decimals,
            TypeSpecification::Ratio { decimals, .. } => *decimals,
            _ => None,
        }
    }

    /// Convert a rational magnitude to an API [`Decimal`] (28-scale boundary).
    ///
    /// Applies this type's `decimal_places` when set. Returns [`NumericFailure::Overflow`]
    /// when |magnitude| > Decimal::MAX (callers map this to a decimal-limit Veto).
    pub fn try_rational_as_decimal(
        &self,
        magnitude: &crate::computation::rational::RationalInteger,
    ) -> Result<rust_decimal::Decimal, crate::computation::rational::NumericFailure> {
        let decimal = magnitude.try_to_decimal()?;
        Ok(format_decimal_for_api(decimal, self.decimal_places()))
    }

    /// Convert a canonical measure magnitude in the named declared unit to an API [`Decimal`].
    pub fn try_measure_canonical_as_decimal_in_unit(
        &self,
        canonical_magnitude: &crate::computation::rational::RationalInteger,
        unit_name: &str,
    ) -> Result<rust_decimal::Decimal, crate::computation::rational::NumericFailure> {
        use crate::computation::rational::checked_div;
        let unit_factor = self.measure_unit_factor(unit_name);
        let magnitude_in_unit = checked_div(canonical_magnitude, unit_factor)?;
        self.try_rational_as_decimal(&magnitude_in_unit)
    }

    /// Convert a canonical ratio magnitude in the named declared unit to an API [`Decimal`].
    pub fn try_ratio_canonical_as_decimal_in_unit(
        &self,
        canonical_magnitude: &crate::computation::rational::RationalInteger,
        unit_name: &str,
    ) -> Result<rust_decimal::Decimal, crate::computation::rational::NumericFailure> {
        use crate::computation::rational::checked_mul;
        let units = match &self.specifications {
            TypeSpecification::Ratio { units, .. } => units,
            _ => unreachable!(
                "BUG: try_ratio_canonical_as_decimal_in_unit called on non-ratio type {}",
                self.name()
            ),
        };
        let ratio_unit = units
            .iter()
            .find(|unit| unit.name == unit_name)
            .unwrap_or_else(|| {
                let valid: Vec<&str> = units.iter().map(|unit| unit.name.as_str()).collect();
                unreachable!(
                    "BUG: unknown ratio unit '{}' for type {} (valid: {}); planning must reject invalid units",
                    unit_name,
                    self.name(),
                    valid.join(", ")
                )
            });
        let magnitude_in_unit = checked_mul(canonical_magnitude, &ratio_unit.value)?;
        self.try_rational_as_decimal(&magnitude_in_unit)
    }

    /// Get an example value string for this type, suitable for UI help text
    pub fn example_value(&self) -> &'static str {
        match &self.specifications {
            TypeSpecification::Text { .. } => "\"hello world\"",
            TypeSpecification::Measure { .. } => "12.50 eur",
            TypeSpecification::MeasureRange { .. } => "30 kilogram...35 kilogram",
            TypeSpecification::Number { .. } => "3.14",
            TypeSpecification::NumberRange { .. } => "0...100",
            TypeSpecification::Boolean { .. } => "true",
            TypeSpecification::Date { .. } => "2023-12-25T14:30:00Z",
            TypeSpecification::DateRange { .. } => "2024-01-01...2024-12-31",
            TypeSpecification::TimeRange { .. } => "09:00...17:00",
            TypeSpecification::Veto { .. } => "veto",
            TypeSpecification::Time { .. } => "14:30:00",
            TypeSpecification::Ratio { .. } => "50%",
            TypeSpecification::RatioRange { .. } => "10%...50%",
            TypeSpecification::Undetermined => unreachable!(
                "BUG: example_value called on Undetermined sentinel type; this type must never reach user-facing code"
            ),
        }
    }

    /// Factor for a unit of this measure type (for unit conversion during evaluation only).
    /// Planning must validate conversions first and return Error for invalid units.
    /// If called with a non-measure type or unknown unit name, panics (invariant violation).
    #[must_use]
    /// Returns the resolved `BaseMeasureVector` for Measure types, or `None` if
    /// the decomposition pass has not yet resolved this type.
    /// Panics if called on non-Measure types.
    pub fn measure_type_decomposition(&self) -> Option<&BaseMeasureVector> {
        match &self.specifications {
            TypeSpecification::Measure { decomposition, .. } => decomposition.as_ref(),
            _ => unreachable!(
                "BUG: measure_type_decomposition called on non-measure type {}",
                self.name()
            ),
        }
    }

    /// Runtime unit signature for measure arithmetic and display.
    ///
    /// - Named measure with units: `[(canonical_or_first_unit_name, 1)]`. Arithmetic
    ///   expands this via `expand_signature_to_base_units` using the unit table.
    /// - Anonymous / no units: decomposition converted to signature form.
    #[must_use]
    pub fn measure_runtime_signature(&self) -> Vec<(String, i32)> {
        if let Some(binding) = &self.measure_binding_unit {
            return vec![(binding.clone(), 1)];
        }
        match &self.specifications {
            TypeSpecification::Measure {
                units,
                decomposition,
                ..
            } => {
                if let Some(canonical) = units.iter().find(|unit| unit.is_canonical_factor()) {
                    return vec![(canonical.name.clone(), 1)];
                }
                if let Some(first) = units.iter().next() {
                    return vec![(first.name.clone(), 1)];
                }
                decomposition
                    .as_ref()
                    .map(base_measure_vector_as_signature)
                    .unwrap_or_default()
            }
            TypeSpecification::MeasureRange {
                units,
                decomposition,
                ..
            } => {
                if let Some(canonical) = units.iter().find(|unit| unit.is_canonical_factor()) {
                    return vec![(canonical.name.clone(), 1)];
                }
                if let Some(first) = units.iter().next() {
                    return vec![(first.name.clone(), 1)];
                }
                decomposition
                    .as_ref()
                    .map(base_measure_vector_as_signature)
                    .unwrap_or_default()
            }
            _ => unreachable!(
                "BUG: measure_runtime_signature called on non-measure type {}",
                self.name()
            ),
        }
    }

    /// Primary declared ratio unit name when the type carries a non-empty unit table.
    #[must_use]
    pub fn ratio_primary_unit(&self) -> Option<&str> {
        match &self.specifications {
            TypeSpecification::Ratio { units, .. } if !units.is_empty() => {
                units.iter().next().map(|unit| unit.name.as_str())
            }
            TypeSpecification::RatioRange { units, .. } if !units.is_empty() => {
                units.iter().next().map(|unit| unit.name.as_str())
            }
            _ => None,
        }
    }

    /// Returns true if this is an anonymous (no-name) Measure — i.e. an anonymous
    /// intermediate produced by cross-axis arithmetic.
    pub fn is_anonymous_measure(&self) -> bool {
        self.name.is_none() && matches!(&self.specifications, TypeSpecification::Measure { .. })
    }

    /// Build an anonymous `LemmaType` for a given dimensional decomposition.
    /// Used at plan time to represent the inferred type of cross-axis intermediates.
    /// Runtime unit signature is derived from this decomposition via
    /// [`LemmaType::measure_runtime_signature`].
    pub fn anonymous_for_decomposition(decomposition: BaseMeasureVector) -> Self {
        Self {
            name: None,
            specifications: TypeSpecification::Measure {
                minimum: None,
                maximum: None,
                decimals: None,
                units: crate::literals::MeasureUnits::new(),
                traits: Vec::new(),
                decomposition: Some(decomposition),
                help: String::new(),
            },
            extends: TypeExtends::Primitive,
            measure_binding_unit: None,
        }
    }

    /// Declared ratio unit names when the type carries a non-empty unit table (`None` otherwise).
    #[must_use]
    pub fn ratio_unit_names(&self) -> Option<Vec<&str>> {
        match &self.specifications {
            TypeSpecification::Ratio { units, .. } if !units.is_empty() => {
                Some(units.iter().map(|unit| unit.name.as_str()).collect())
            }
            TypeSpecification::RatioRange { units, .. } if !units.is_empty() => {
                Some(units.iter().map(|unit| unit.name.as_str()).collect())
            }
            _ => None,
        }
    }

    /// Declared unit names when the type carries a non-empty unit table (`None` otherwise).
    #[must_use]
    pub fn measure_unit_names(&self) -> Option<Vec<&str>> {
        match &self.specifications {
            TypeSpecification::Measure { units, .. } if !units.is_empty() => {
                Some(units.iter().map(|unit| unit.name.as_str()).collect())
            }
            TypeSpecification::MeasureRange { units, .. } if !units.is_empty() => {
                Some(units.iter().map(|unit| unit.name.as_str()).collect())
            }
            _ => None,
        }
    }

    /// `age [number]` or `gender [gender_code]` — brackets omitted when type adds nothing.
    #[must_use]
    pub fn label_for_data_input(&self, input_key: &str) -> String {
        let type_label = if let Some(parent) = self.extends.parent_name() {
            parent.to_string()
        } else {
            let type_name = self.name();
            if type_name == input_key {
                self.specifications.to_string()
            } else {
                type_name
            }
        };
        if type_label == input_key {
            input_key.to_string()
        } else {
            format!("{input_key} [{type_label}]")
        }
    }

    /// `Data age [number]: {detail}` for runtime data override vetoes.
    #[must_use]
    pub fn data_veto_message(&self, input_key: &str, detail: &str) -> String {
        format!("Data {}: {}", self.label_for_data_input(input_key), detail)
    }

    /// Whether an empty [`RunDataValue`] should veto before parse (text may accept `""`).
    #[must_use]
    pub fn empty_runtime_input_vetoes(&self) -> bool {
        !matches!(self.specifications, TypeSpecification::Text { .. })
    }

    /// Return the conversion factor for a declared unit name on this measure type.
    pub fn measure_unit_factor(
        &self,
        unit_name: &str,
    ) -> &crate::computation::rational::RationalInteger {
        let units = match &self.specifications {
            TypeSpecification::Measure { units, .. } => units,
            TypeSpecification::MeasureRange { units, .. } => units,
            _ => unreachable!(
                "BUG: measure_unit_factor called with non-measure type {}; only call during evaluation after planning validated measure conversion",
                self.name()
            ),
        };
        match units.get(unit_name) {
            Ok(MeasureUnit { factor, .. }) => factor,
            Err(_) => {
                let valid: Vec<&str> = units.iter().map(|u| u.name.as_str()).collect();
                unreachable!(
                    "BUG: unknown unit '{}' for measure type {} (valid: {}); planning must reject invalid conversions with Error",
                    unit_name,
                    self.name(),
                    valid.join(", ")
                );
            }
        }
    }

    pub fn ratio_unit_factor(
        &self,
        unit_name: &str,
    ) -> &crate::computation::rational::RationalInteger {
        let units = match &self.specifications {
            TypeSpecification::Ratio { units, .. } => units,
            _ => unreachable!(
                "BUG: ratio_unit_factor called with non-ratio type {}; only call during evaluation after planning validated ratio conversion",
                self.name()
            ),
        };
        match units.get(unit_name) {
            Ok(RatioUnit { value, .. }) => value,
            Err(_) => {
                let valid: Vec<&str> = units.0.iter().map(|u| u.name.as_str()).collect();
                unreachable!(
                    "BUG: unknown unit '{}' for ratio type {} (valid: {}); planning must reject invalid conversions with Error",
                    unit_name,
                    self.name(),
                    valid.join(", ")
                );
            }
        }
    }

    /// Convert a measure literal to API [`Decimal`]s in the given unit names.
    pub(crate) fn measure_literal_unit_map(
        &self,
        literal: &LiteralValue,
        unit_names: &[&str],
        factor_source: UnitFactorSource<'_>,
    ) -> Result<BTreeMap<String, rust_decimal::Decimal>, LiteralUnitMapFailure> {
        use crate::computation::rational::checked_div;

        let ValueKind::Measure(magnitude) = &literal.value else {
            panic!("BUG: measure_literal_unit_map called with non-measure value");
        };
        let mut map = BTreeMap::new();
        for &unit_name in unit_names {
            let unit_factor = factor_source.measure_unit_factor(unit_name);
            let magnitude_in_unit = checked_div(magnitude, unit_factor)
                .map_err(LiteralUnitMapFailure::UnitConversion)?;
            let decimal = self
                .try_rational_as_decimal(&magnitude_in_unit)
                .map_err(LiteralUnitMapFailure::Commit)?;
            map.insert(unit_name.to_string(), decimal);
        }
        Ok(map)
    }

    /// Convert a ratio literal to API [`Decimal`]s in the given unit names.
    pub(crate) fn ratio_literal_unit_map(
        &self,
        literal: &LiteralValue,
        unit_names: &[&str],
        factor_source: UnitFactorSource<'_>,
    ) -> Result<BTreeMap<String, rust_decimal::Decimal>, LiteralUnitMapFailure> {
        use crate::computation::rational::checked_mul;

        let ratio_api_type = match &self.specifications {
            TypeSpecification::Ratio { .. } => self,
            TypeSpecification::RatioRange { .. } => {
                return ratio_element_type_for_api(self).ratio_literal_unit_map(
                    literal,
                    unit_names,
                    factor_source,
                );
            }
            _ => {
                panic!(
                    "BUG: ratio_literal_unit_map called with non-ratio type {}",
                    self.name()
                );
            }
        };
        let ValueKind::Ratio(canonical) = &literal.value else {
            panic!("BUG: ratio_literal_unit_map called with non-ratio value");
        };
        if unit_names.is_empty() {
            panic!(
                "BUG: ratio literal type '{}' must have at least one unit name",
                self.name()
            );
        }
        let mut map = BTreeMap::new();
        for &unit_name in unit_names {
            let unit_factor = factor_source.ratio_unit_factor(unit_name);
            let magnitude_in_unit = checked_mul(canonical, unit_factor)
                .map_err(LiteralUnitMapFailure::UnitConversion)?;
            let decimal = ratio_api_type
                .try_rational_as_decimal(&magnitude_in_unit)
                .map_err(LiteralUnitMapFailure::Commit)?;
            map.insert(unit_name.to_string(), decimal);
        }
        Ok(map)
    }
}

/// Where to read measure/ratio unit factors when building a per-unit decimal map.
pub(crate) enum UnitFactorSource<'a> {
    DeclaredOn(&'a LemmaType),
    Merged {
        measure: Option<&'a MeasureUnits>,
        ratio: Option<&'a RatioUnits>,
    },
}

impl UnitFactorSource<'_> {
    fn measure_unit_factor(
        &self,
        unit_name: &str,
    ) -> &crate::computation::rational::RationalInteger {
        match self {
            UnitFactorSource::DeclaredOn(lemma_type) => lemma_type.measure_unit_factor(unit_name),
            UnitFactorSource::Merged { measure, .. } => {
                let units = measure.unwrap_or_else(|| {
                    panic!(
                        "BUG: family measure expansion missing merged table for unit '{unit_name}'"
                    )
                });
                &units
                    .get(unit_name)
                    .unwrap_or_else(|_| {
                        panic!("BUG: family unit '{unit_name}' missing from merged measure table")
                    })
                    .factor
            }
        }
    }

    fn ratio_unit_factor(&self, unit_name: &str) -> &crate::computation::rational::RationalInteger {
        match self {
            UnitFactorSource::DeclaredOn(lemma_type) => lemma_type.ratio_unit_factor(unit_name),
            UnitFactorSource::Merged { ratio, .. } => {
                let units = ratio.unwrap_or_else(|| {
                    panic!(
                        "BUG: family ratio expansion missing merged table for unit '{unit_name}'"
                    )
                });
                &units
                    .get(unit_name)
                    .unwrap_or_else(|_| {
                        panic!("BUG: family unit '{unit_name}' missing from merged ratio table")
                    })
                    .value
            }
        }
    }
}

/// Primitive ratio type for API unit-map conversion on ratio range wrappers.
pub(crate) fn ratio_element_type_for_api(lemma_type: &LemmaType) -> LemmaType {
    match &lemma_type.specifications {
        TypeSpecification::Ratio { .. } => lemma_type.clone(),
        TypeSpecification::RatioRange { .. } => {
            let element = range_element_type_specification(&lemma_type.specifications)
                .expect("BUG: ratio range type must have ratio element specification");
            let TypeSpecification::Ratio {
                units, decimals, ..
            } = element
            else {
                panic!("BUG: ratio range element spec must be Ratio");
            };
            LemmaType::primitive(TypeSpecification::Ratio {
                minimum: None,
                maximum: None,
                decimals,
                units,
                help: String::new(),
            })
        }
        _ => panic!(
            "BUG: ratio_element_type_for_api called with non-ratio type {}",
            lemma_type.name()
        ),
    }
}

/// Failure while converting a literal to decimal strings across declared units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LiteralUnitMapFailure {
    Commit(crate::computation::rational::NumericFailure),
    UnitConversion(crate::computation::rational::NumericFailure),
}

/// Runtime literal payload. Type lives on `NormalForm.result_type` / `DataDefinition` /
/// explicit computation parameters — not on the value.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LiteralValue {
    pub value: ValueKind,
}

impl LiteralValue {
    #[inline]
    pub fn new(value: ValueKind) -> Self {
        Self { value }
    }

    fn single_measure_signature_unit_name(signature: &[(String, i32)]) -> Option<&str> {
        match signature {
            [(unit_name, 1)] => Some(unit_name.as_str()),
            _ => None,
        }
    }
}

/// Planning-time literal: value plus resolved type.
///
/// Used by `ExpressionKind::Literal` and coerce/typing paths. Runtime
/// `OperationResult` / value-table slots store bare [`LiteralValue`]; the type
/// is on the node (`NormalForm.result_type`) or data definition.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TypedLiteral {
    pub value: ValueKind,
    pub lemma_type: Arc<LemmaType>,
}

impl TypedLiteral {
    #[inline]
    pub fn to_literal(&self) -> LiteralValue {
        LiteralValue {
            value: self.value.clone(),
        }
    }

    #[inline]
    pub fn into_literal(self) -> LiteralValue {
        LiteralValue { value: self.value }
    }
}

impl From<TypedLiteral> for LiteralValue {
    fn from(typed: TypedLiteral) -> Self {
        typed.into_literal()
    }
}

impl From<&TypedLiteral> for LiteralValue {
    fn from(typed: &TypedLiteral) -> Self {
        typed.to_literal()
    }
}

impl LiteralValue {
    pub fn text(s: String) -> Self {
        Self {
            value: ValueKind::Text(s),
        }
    }

    pub fn number(n: RationalInteger) -> Self {
        Self {
            value: ValueKind::Number(n),
        }
    }

    pub fn number_from_decimal(decimal: Decimal) -> Self {
        Self::number(
            crate::literals::rational_from_parsed_decimal(decimal)
                .expect("BUG: literal number from decimal must lift at boundary"),
        )
    }

    pub fn measure(n: RationalInteger) -> Self {
        Self {
            value: ValueKind::Measure(n),
        }
    }

    /// Type arg ignored — type lives on the node / caller.
    pub fn measure_with_type(n: RationalInteger, _lemma_type: Arc<LemmaType>) -> Self {
        Self::measure(n)
    }

    pub fn measure_with_bound_unit(
        n: RationalInteger,
        _unit_name: impl Into<String>,
        _lemma_type: Arc<LemmaType>,
    ) -> Self {
        Self::measure(n)
    }

    pub fn measure_with_signature(n: RationalInteger, _lemma_type: Arc<LemmaType>) -> Self {
        Self::measure(n)
    }

    pub fn number_with_type(n: RationalInteger, _lemma_type: Arc<LemmaType>) -> Self {
        Self::number(n)
    }

    pub fn number_with_type_from_decimal(decimal: Decimal, _lemma_type: Arc<LemmaType>) -> Self {
        Self::number_from_decimal(decimal)
    }

    pub fn ratio_with_type(r: RationalInteger, _lemma_type: Arc<LemmaType>) -> Self {
        Self::ratio(r)
    }

    pub fn ratio_with_bound_unit(
        r: RationalInteger,
        _unit_name: impl Into<String>,
        _lemma_type: Arc<LemmaType>,
    ) -> Self {
        Self::ratio(r)
    }

    pub fn text_with_type(s: String, _lemma_type: Arc<LemmaType>) -> Self {
        Self::text(s)
    }

    pub fn date_with_type(dt: SemanticDateTime, _lemma_type: Arc<LemmaType>) -> Self {
        Self::date(dt)
    }

    pub fn time_with_type(t: SemanticTime, _lemma_type: Arc<LemmaType>) -> Self {
        Self::time(t)
    }

    pub fn calendar(
        value: RationalInteger,
        _unit: SemanticCalendarUnit,
        _lemma_type: Arc<LemmaType>,
    ) -> Self {
        Self::measure(value)
    }

    pub fn calendar_from_decimal(
        value: Decimal,
        unit: SemanticCalendarUnit,
        lemma_type: Arc<LemmaType>,
    ) -> Self {
        Self::calendar(
            crate::literals::rational_from_parsed_decimal(value)
                .expect("BUG: calendar literal from decimal must lift at boundary"),
            unit,
            lemma_type,
        )
    }

    pub fn calendar_with_type(
        value: RationalInteger,
        unit: SemanticCalendarUnit,
        lemma_type: Arc<LemmaType>,
    ) -> Self {
        Self::calendar(value, unit, lemma_type)
    }

    pub fn number_interpreted_as_measure(value: RationalInteger, _unit_name: String) -> Self {
        Self::measure(value)
    }

    pub fn from_bool(b: bool) -> Self {
        Self {
            value: ValueKind::Boolean(b),
        }
    }

    pub fn from_datetime(dt: &crate::parsing::ast::DateTimeValue) -> Self {
        Self::date(date_time_to_semantic(dt))
    }

    pub fn date(dt: SemanticDateTime) -> Self {
        Self {
            value: ValueKind::Date(dt),
        }
    }

    pub fn time(t: SemanticTime) -> Self {
        Self {
            value: ValueKind::Time(t),
        }
    }

    pub fn ratio(r: RationalInteger) -> Self {
        Self {
            value: ValueKind::Ratio(r),
        }
    }

    pub fn ratio_from_decimal(r: Decimal) -> Self {
        Self::ratio(
            crate::literals::rational_from_parsed_decimal(r)
                .expect("BUG: ratio literal from decimal must lift at boundary"),
        )
    }

    pub fn range(left: LiteralValue, right: LiteralValue) -> Self {
        Self {
            value: ValueKind::Range(Box::new(left), Box::new(right)),
        }
    }

    /// Display without an explicit type. Measure/ratio fall back to magnitude-only.
    pub fn display_value(&self) -> String {
        match &self.value {
            ValueKind::Measure(_) | ValueKind::Ratio(_) => format!("{}", self.value),
            ValueKind::Range(left, right) => {
                format!("{}...{}", left.display_value(), right.display_value())
            }
            _ => format!("{}", self.value),
        }
    }

    /// Display string given an explicit type (measure/ratio need unit identity).
    pub fn display_value_with_type(&self, lemma_type: &LemmaType) -> String {
        match &self.value {
            ValueKind::Measure(n) => {
                let signature = lemma_type.measure_runtime_signature();
                format_measure_canonical_for_display(n, lemma_type, &signature)
            }
            ValueKind::Ratio(n) => format_ratio_canonical_for_display(n, lemma_type),
            ValueKind::Range(left, right) => {
                let endpoint_ty = range_element_type_specification(&lemma_type.specifications)
                    .map(LemmaType::primitive)
                    .unwrap_or_else(|| lemma_type.clone());
                format!(
                    "{}...{}",
                    left.display_value_with_type(&endpoint_ty),
                    right.display_value_with_type(&endpoint_ty)
                )
            }
            _ => format!("{}", self.value),
        }
    }

    /// Structural byte-size estimate for resource limit checks.
    pub fn byte_size(&self) -> usize {
        self.value.structural_byte_size()
    }

    /// Magnitude for decimal input prompts (API [`Decimal`]; format at call site).
    #[must_use]
    pub fn magnitude_suggestion_for_decimal_prompt(
        &self,
        lemma_type: &LemmaType,
    ) -> Option<rust_decimal::Decimal> {
        match &self.value {
            ValueKind::Number(n) => Some(
                lemma_type
                    .try_rational_as_decimal(n)
                    .expect("BUG: stored number literal must convert to decimal for prompt"),
            ),
            ValueKind::Measure(n) => {
                let signature = lemma_type.measure_runtime_signature();
                let unit_name = Self::single_measure_signature_unit_name(&signature).expect(
                    "BUG: measure prompt requires exactly one signature unit with exponent 1",
                );
                Some(
                    lemma_type
                        .try_measure_canonical_as_decimal_in_unit(n, unit_name)
                        .expect("BUG: stored measure literal must convert to decimal for prompt"),
                )
            }
            ValueKind::Ratio(n) => {
                if let Some(unit_name) = lemma_type.measure_binding_unit.as_deref() {
                    Some(
                        lemma_type
                            .try_ratio_canonical_as_decimal_in_unit(n, unit_name)
                            .expect("BUG: stored ratio literal must convert to decimal for prompt"),
                    )
                } else {
                    Some(lemma_type.try_rational_as_decimal(n).expect(
                        "BUG: stored bare ratio literal must convert to decimal for prompt",
                    ))
                }
            }
            _ => None,
        }
    }

    /// Per-unit magnitudes when this literal is a measure with declared units.
    #[must_use]
    pub fn measure_units(
        &self,
        lemma_type: &LemmaType,
    ) -> Option<BTreeMap<String, rust_decimal::Decimal>> {
        if !matches!(self.value, ValueKind::Measure(_)) {
            return None;
        }
        lemma_type.measure_unit_names()?;
        let declared: Vec<&str> = lemma_type
            .measure_unit_names()
            .expect("BUG: measure_unit_names checked above");
        lemma_type
            .measure_literal_unit_map(self, &declared, UnitFactorSource::DeclaredOn(lemma_type))
            .ok()
    }

    /// Per-unit magnitudes when this literal is a ratio with declared units.
    #[must_use]
    pub fn ratio_units(
        &self,
        lemma_type: &LemmaType,
    ) -> Option<BTreeMap<String, rust_decimal::Decimal>> {
        if !matches!(self.value, ValueKind::Ratio(_)) {
            return None;
        }
        let has_declared_units = match &lemma_type.specifications {
            TypeSpecification::Ratio { units, .. } => !units.is_empty(),
            TypeSpecification::RatioRange { .. } => true,
            _ => return None,
        };
        if !has_declared_units {
            return None;
        }
        let declared: Vec<&str> = lemma_type
            .ratio_unit_names()
            .expect("BUG: ratio units checked above");
        lemma_type
            .ratio_literal_unit_map(self, &declared, UnitFactorSource::DeclaredOn(lemma_type))
            .ok()
    }

    /// Magnitude in a declared unit when this literal is measure or ratio.
    #[must_use]
    pub fn magnitude_in_unit(
        &self,
        lemma_type: &LemmaType,
        unit: &str,
    ) -> Option<rust_decimal::Decimal> {
        self.measure_units(lemma_type)
            .and_then(|map| map.get(unit).copied())
            .or_else(|| {
                self.ratio_units(lemma_type)
                    .and_then(|map| map.get(unit).copied())
            })
    }

    /// Derive second from a duration measure's canonical magnitude.
    pub fn duration_canonical_seconds(&self, lemma_type: &LemmaType) -> RationalInteger {
        let ValueKind::Measure(magnitude) = &self.value else {
            unreachable!(
                "BUG: duration_canonical_seconds called with {:?}",
                self.value
            );
        };
        if !lemma_type.is_duration_like_measure() {
            unreachable!(
                "BUG: duration_canonical_seconds called with type {}",
                lemma_type.name()
            );
        }
        let factor = lemma_type.measure_unit_factor("second");
        checked_div(magnitude, factor).expect("BUG: duration unit factor cannot be zero")
    }

    /// Derive month from a calendar measure's canonical magnitude.
    pub fn calendar_canonical_months(&self, lemma_type: &LemmaType) -> RationalInteger {
        let ValueKind::Measure(magnitude) = &self.value else {
            unreachable!(
                "BUG: calendar_canonical_months called with {:?}",
                self.value
            );
        };
        if !lemma_type.is_calendar_like() {
            unreachable!(
                "BUG: calendar_canonical_months called with type {}",
                lemma_type.name()
            );
        }
        let factor = lemma_type.measure_unit_factor("month");
        checked_div(magnitude, factor).expect("BUG: calendar unit factor cannot be zero")
    }
}

impl TypedLiteral {
    pub fn text(s: String) -> Self {
        Self {
            value: ValueKind::Text(s),
            lemma_type: primitive_text_arc().clone(),
        }
    }

    pub fn text_with_type(s: String, lemma_type: Arc<LemmaType>) -> Self {
        Self {
            value: ValueKind::Text(s),
            lemma_type,
        }
    }

    pub fn number(n: RationalInteger) -> Self {
        Self {
            value: ValueKind::Number(n),
            lemma_type: primitive_number_arc().clone(),
        }
    }

    pub fn number_from_decimal(decimal: Decimal) -> Self {
        Self::number(
            crate::literals::rational_from_parsed_decimal(decimal)
                .expect("BUG: literal number from decimal must lift at boundary"),
        )
    }

    pub fn number_with_type(n: RationalInteger, lemma_type: Arc<LemmaType>) -> Self {
        Self {
            value: ValueKind::Number(n),
            lemma_type,
        }
    }

    pub fn number_with_type_from_decimal(decimal: Decimal, lemma_type: Arc<LemmaType>) -> Self {
        Self::number_with_type(
            crate::literals::rational_from_parsed_decimal(decimal)
                .expect("BUG: literal number from decimal must lift at boundary"),
            lemma_type,
        )
    }

    /// Build a Measure literal bound to a specific declared unit.
    pub fn measure_with_bound_unit(
        n: RationalInteger,
        unit_name: impl Into<String>,
        lemma_type: Arc<LemmaType>,
    ) -> Self {
        Self::measure_with_type(
            n,
            Arc::new(
                lemma_type
                    .as_ref()
                    .clone()
                    .with_measure_binding_unit(unit_name),
            ),
        )
    }

    pub fn measure_with_type(n: RationalInteger, lemma_type: Arc<LemmaType>) -> Self {
        Self {
            value: ValueKind::Measure(n),
            lemma_type,
        }
    }

    pub fn measure_with_signature(n: RationalInteger, lemma_type: Arc<LemmaType>) -> Self {
        Self {
            value: ValueKind::Measure(n),
            lemma_type,
        }
    }

    /// Number interpreted as a measure value in the given unit.
    pub fn number_interpreted_as_measure(value: RationalInteger, unit_name: String) -> Self {
        let lemma_type = if unit_name.is_empty() {
            Arc::new(anonymous_measure_type())
        } else {
            let mut decomp = BaseMeasureVector::new();
            decomp.insert(unit_name, 1);
            Arc::new(LemmaType::anonymous_for_decomposition(decomp))
        };
        Self {
            value: ValueKind::Measure(value),
            lemma_type,
        }
    }

    pub fn from_bool(b: bool) -> Self {
        Self {
            value: ValueKind::Boolean(b),
            lemma_type: primitive_boolean_arc().clone(),
        }
    }

    pub fn from_datetime(dt: &crate::parsing::ast::DateTimeValue) -> Self {
        Self::date(date_time_to_semantic(dt))
    }

    pub fn date(dt: SemanticDateTime) -> Self {
        Self {
            value: ValueKind::Date(dt),
            lemma_type: primitive_date_arc().clone(),
        }
    }

    pub fn date_with_type(dt: SemanticDateTime, lemma_type: Arc<LemmaType>) -> Self {
        Self {
            value: ValueKind::Date(dt),
            lemma_type,
        }
    }

    pub fn time(t: SemanticTime) -> Self {
        Self {
            value: ValueKind::Time(t),
            lemma_type: primitive_time_arc().clone(),
        }
    }

    pub fn time_with_type(t: SemanticTime, lemma_type: Arc<LemmaType>) -> Self {
        Self {
            value: ValueKind::Time(t),
            lemma_type,
        }
    }

    pub fn calendar(
        value: RationalInteger,
        unit: SemanticCalendarUnit,
        lemma_type: Arc<LemmaType>,
    ) -> Self {
        let unit_name = unit.to_string();
        debug_assert_eq!(
            semantic_calendar_unit_from_unit_name(&unit_name),
            unit,
            "BUG: calendar unit name must round-trip"
        );
        Self::measure_with_bound_unit(value, unit_name, lemma_type)
    }

    pub fn calendar_from_decimal(
        value: Decimal,
        unit: SemanticCalendarUnit,
        lemma_type: Arc<LemmaType>,
    ) -> Self {
        Self::calendar(
            crate::literals::rational_from_parsed_decimal(value)
                .expect("BUG: calendar literal from decimal must lift at boundary"),
            unit,
            lemma_type,
        )
    }

    pub fn calendar_with_type(
        value: RationalInteger,
        unit: SemanticCalendarUnit,
        lemma_type: Arc<LemmaType>,
    ) -> Self {
        Self::calendar(value, unit, lemma_type)
    }

    pub fn ratio(r: RationalInteger) -> Self {
        Self {
            value: ValueKind::Ratio(r),
            lemma_type: primitive_ratio_arc().clone(),
        }
    }

    pub fn ratio_from_decimal(r: Decimal) -> Self {
        Self::ratio(
            crate::literals::rational_from_parsed_decimal(r)
                .expect("BUG: ratio literal from decimal must lift at boundary"),
        )
    }

    pub fn ratio_with_type(r: RationalInteger, lemma_type: Arc<LemmaType>) -> Self {
        Self {
            value: ValueKind::Ratio(r),
            lemma_type,
        }
    }

    pub fn ratio_with_bound_unit(
        r: RationalInteger,
        unit_name: impl Into<String>,
        lemma_type: Arc<LemmaType>,
    ) -> Self {
        Self::ratio_with_type(
            r,
            Arc::new(
                lemma_type
                    .as_ref()
                    .clone()
                    .with_measure_binding_unit(unit_name),
            ),
        )
    }

    pub fn range(left: TypedLiteral, right: TypedLiteral) -> Self {
        let specifications =
            range_type_specification_from_endpoints(&left.lemma_type, &right.lemma_type)
                .unwrap_or_else(|| {
                    unreachable!(
                "BUG: attempted to construct a range literal from incompatible endpoint types"
            )
                });

        Self {
            value: ValueKind::Range(Box::new(left.to_literal()), Box::new(right.to_literal())),
            lemma_type: Arc::new(LemmaType::primitive(specifications)),
        }
    }

    pub fn display_value(&self) -> String {
        self.to_literal().display_value_with_type(&self.lemma_type)
    }

    pub fn get_type(&self) -> &LemmaType {
        &self.lemma_type
    }

    pub fn byte_size(&self) -> usize {
        self.value.structural_byte_size()
    }
}

impl fmt::Display for TypedLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_value())
    }
}

/// What a [`DataDefinition::Reference`] copies its value from: either another data path
/// or a rule whose result becomes this data's value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReferenceTarget {
    Data(DataPath),
    Rule(RulePath),
}

/// Where a [`DataDefinition::Reference`] chain ends. Computed once per slice in
/// [`crate::planning::graph::Graph::validate`] after `compute_data_reference_order`
/// rejected cycles.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ReferenceEnd {
    Promptable(DataPath),
    Rule(RulePath),
    Import,
}

/// Resolved data value for the execution plan: aligned with [`DataValue`] but with source per variant.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DataDefinition {
    /// Value-holding data: current literal (spec or `with` binding).
    Value {
        value: LiteralValue,
        resolved_type: Arc<LemmaType>,
        source: Source,
    },
    /// Type-only data: type known, value to be supplied (e.g. via run data).
    /// `declared_suggestion` carries the `-> suggest ...` payload for this binding or
    /// the suggestion inherited from the parent type chain, if any; value-promoting code
    /// uses it instead of re-deriving suggestions from [`TypeSpecification`].
    /// The evaluator never commits a suggestion — unbound stays MissingData.
    TypeDeclaration {
        resolved_type: Arc<LemmaType>,
        declared_suggestion: Option<BoundValueKind>,
        declared_fill: Option<BoundValueKind>,
        source: Source,
    },
    /// Import (`uses`): alias for another spec; nested members are flattened onto the plan.
    Import { target_name: String, source: Source },
    /// Value-copy reference to another data or a rule result.
    ///
    /// `resolved_type` is the merged type that the copied value must satisfy at
    /// evaluation time. Merging folds together: (1) the LHS's own declared type,
    /// if any; (2) the target's type (data declared type or rule return type);
    /// (3) any `local_constraints` written after the `->` on the reference itself.
    /// Merging happens in a dedicated pass once all data and rule types are
    /// known; before that pass, `resolved_type` holds a provisional value and
    /// must not be consumed for type checking.
    ///
    /// `local_constraints` preserves the raw constraint list from the reference's
    /// `-> ...` tail (e.g. `minimum 5` in `data license2: law.other -> minimum 5`)
    /// for that merging pass. It is `None` when the reference has no trailing
    /// constraints.
    ///
    /// `local_suggestion` carries any `suggest <value>` constraint from the
    /// reference's `-> ...` tail. The reference-merge pass extracts it from the
    /// constraint list during type resolution. It is a UI hint only
    /// ([`Self::suggestion`] / show); the evaluator never commits it — unbound
    /// stays MissingData.
    ///
    /// The reference itself is evaluated by copying the target's value (data path)
    /// or the target rule's result in topological order; caller values in
    /// [`crate::evaluation::run_data::RunData`] override the reference.
    Reference {
        target: ReferenceTarget,
        resolved_type: Arc<LemmaType>,
        local_constraints: Option<Vec<Constraint>>,
        local_suggestion: Option<BoundValueKind>,
        local_fill: Option<BoundValueKind>,
        source: Source,
    },
}

impl DataDefinition {
    /// Declared lemma type for value, type-declaration, and reference data; `None` for imports.
    pub fn lemma_type(&self) -> Option<&LemmaType> {
        self.resolved_type_arc().map(|arc| arc.as_ref())
    }

    /// Shared [`Arc`] for the declared type; `None` for imports.
    pub fn resolved_type_arc(&self) -> Option<&Arc<LemmaType>> {
        match self {
            DataDefinition::Value { resolved_type, .. } => Some(resolved_type),
            DataDefinition::TypeDeclaration { resolved_type, .. } => Some(resolved_type),
            DataDefinition::Reference { resolved_type, .. } => Some(resolved_type),
            DataDefinition::Import { .. } => None,
        }
    }

    /// Alias for [`Self::lemma_type`] (historical name).
    #[inline]
    pub fn schema_type(&self) -> Option<&LemmaType> {
        self.lemma_type()
    }

    /// Returns the literal value when the data already holds one, including `-> fill`
    /// on a type declaration or reference. A data-target `Reference`'s copied value
    /// is produced by the evaluator at runtime, so at plan-time it has no value yet.
    pub fn value(&self) -> Option<LiteralValue> {
        match self {
            DataDefinition::Value { value, .. } => Some(value.clone()),
            DataDefinition::TypeDeclaration {
                declared_fill: Some(dv),
                ..
            } => Some(dv.to_literal()),
            DataDefinition::Reference {
                local_fill: Some(dv),
                resolved_type: _,
                ..
            } => Some(dv.to_literal()),
            DataDefinition::TypeDeclaration { .. }
            | DataDefinition::Import { .. }
            | DataDefinition::Reference { .. } => None,
        }
    }

    /// Fill value with optional written measure unit binding (Show display).
    pub(crate) fn bound_fill(&self) -> Option<&BoundValueKind> {
        match self {
            DataDefinition::TypeDeclaration {
                declared_fill: Some(dv),
                ..
            }
            | DataDefinition::Reference {
                local_fill: Some(dv),
                ..
            } => Some(dv),
            DataDefinition::Value { .. }
            | DataDefinition::TypeDeclaration { .. }
            | DataDefinition::Reference { .. }
            | DataDefinition::Import { .. } => None,
        }
    }

    /// Suggestion with optional written measure unit binding (Show display).
    /// Surfaces in [`crate::planning::execution_plan::ShowData::suggestion`] for
    /// show/response/UI; the evaluator never commits it — unbound stays MissingData.
    pub(crate) fn bound_suggestion(&self) -> Option<&BoundValueKind> {
        match self {
            DataDefinition::TypeDeclaration {
                declared_suggestion: Some(dv),
                ..
            }
            | DataDefinition::Reference {
                local_suggestion: Some(dv),
                ..
            } => Some(dv),
            DataDefinition::Value { .. }
            | DataDefinition::TypeDeclaration { .. }
            | DataDefinition::Reference { .. }
            | DataDefinition::Import { .. } => None,
        }
    }

    /// Returns the source location for this data.
    pub fn source(&self) -> &Source {
        match self {
            DataDefinition::Value { source, .. } => source,
            DataDefinition::TypeDeclaration { source, .. } => source,
            DataDefinition::Import { source, .. } => source,
            DataDefinition::Reference { source, .. } => source,
        }
    }
}

/// Bind a type-agnostic [`Value::NumberWithUnit`] using the unit index entry for `unit_name`.
pub fn number_with_unit_to_value_kind(
    magnitude: rust_decimal::Decimal,
    unit_name: &str,
    lemma_type: &LemmaType,
) -> Result<ValueKind, String> {
    match &lemma_type.specifications {
        TypeSpecification::Ratio { units, .. } => {
            use crate::computation::rational::{checked_div, decimal_to_rational};
            let unit = units.get(unit_name)?;
            let magnitude_rational = decimal_to_rational(magnitude)
                .map_err(|failure| format!("ratio literal failed rational lift: {failure}"))?;
            let canonical_rational = checked_div(&magnitude_rational, &unit.value)
                .map_err(|failure| format!("ratio literal: unit conversion failed: {failure}"))?;
            Ok(ValueKind::Ratio(canonical_rational))
        }
        TypeSpecification::Measure { units, .. } => {
            use crate::computation::rational::checked_mul;
            let rational = lift_parser_decimal(magnitude)?;
            let unit = units.get(unit_name)?;
            let canonical = checked_mul(&rational, &unit.factor)
                .map_err(|failure| format!("measure canonicalization overflow: {failure}"))?;
            Ok(ValueKind::Measure(canonical))
        }
        _ => Err(format!(
            "Unit '{}' is defined on type '{}' which is not measure or ratio",
            unit_name,
            lemma_type.name()
        )),
    }
}

/// Whether a [`ValueKind`] is structurally compatible with a [`TypeSpecification`].
/// Bound validation (min/max/decimals) is separate; this only checks shape.
pub(crate) fn value_kind_matches_spec(value: &ValueKind, type_spec: &TypeSpecification) -> bool {
    matches!(
        (type_spec, value),
        (TypeSpecification::Number { .. }, ValueKind::Number(_))
            | (TypeSpecification::Text { .. }, ValueKind::Text(_))
            | (TypeSpecification::Boolean { .. }, ValueKind::Boolean(_))
            | (TypeSpecification::Date { .. }, ValueKind::Date(_))
            | (TypeSpecification::Time { .. }, ValueKind::Time(_))
            | (TypeSpecification::Measure { .. }, ValueKind::Measure(_))
            | (TypeSpecification::Ratio { .. }, ValueKind::Ratio(_))
            | (TypeSpecification::Ratio { .. }, ValueKind::Number(_))
            | (
                TypeSpecification::NumberRange { .. },
                ValueKind::Range(_, _)
            )
            | (TypeSpecification::DateRange { .. }, ValueKind::Range(_, _))
            | (TypeSpecification::TimeRange { .. }, ValueKind::Range(_, _))
            | (TypeSpecification::RatioRange { .. }, ValueKind::Range(_, _))
            | (
                TypeSpecification::MeasureRange { .. },
                ValueKind::Range(_, _)
            )
            | (TypeSpecification::Veto { .. }, _)
            | (TypeSpecification::Undetermined, _)
    )
}

fn parser_value_type_mismatch(
    value: &crate::literals::Value,
    type_spec: &TypeSpecification,
) -> String {
    use crate::parsing::ast::AsLemmaSource;
    let value_str = format!("{}", AsLemmaSource(value));
    match type_spec {
        TypeSpecification::Measure { units, .. } => {
            let unit_hint = units
                .iter()
                .find(|u| u.factor == crate::computation::rational::rational_one())
                .map(|u| u.name.as_str())
                .or_else(|| units.iter().next().map(|u| u.name.as_str()))
                .unwrap_or("unit");
            format!("cannot use {value_str} as {type_spec}: expected `<n> {unit_hint}`")
        }
        TypeSpecification::Ratio { units, .. } if !units.is_empty() => {
            let unit_hint = units
                .iter()
                .next()
                .map(|u| u.name.as_str())
                .unwrap_or("unit");
            format!(
                "cannot use {value_str} as {type_spec}: expected `<n> {unit_hint}` or bare ratio"
            )
        }
        _ => format!("cannot use {value_str} as {type_spec}"),
    }
}

/// Re-canonicalize a measure literal after compound unit factors were resolved.
///
/// Literals parsed before derived unit resolution were canonicalized with prefix-only
/// factors; multiply by `resolved_factor / stored_factor` to align with final factors.
pub fn refresh_measure_literal_canonical_magnitude(
    lit: &mut LiteralValue,
    previous_type: &LemmaType,
    resolved_type: &LemmaType,
) -> Result<(), crate::computation::rational::NumericFailure> {
    use crate::computation::rational::{checked_div, checked_mul};
    let ValueKind::Measure(magnitude) = &mut lit.value else {
        return Ok(());
    };
    // Binding unit is no longer on ValueKind; magnitude was canonicalized at bind time
    // against the then-current unit table. When a single-term runtime signature unit
    // exists on both tables, rescale if that unit's factor changed.
    let signature = previous_type.measure_runtime_signature();
    let Some((unit_name, 1)) = signature.first().map(|(n, e)| (n.as_str(), *e)) else {
        return Ok(());
    };
    if signature.len() != 1 {
        return Ok(());
    }
    let stored_factor = previous_type.measure_unit_factor(unit_name);
    let resolved_factor = resolved_type.measure_unit_factor(unit_name);
    if stored_factor == resolved_factor {
        return Ok(());
    }
    let scaled = checked_mul(magnitude, resolved_factor)?;
    *magnitude = checked_div(&scaled, stored_factor)?;
    Ok(())
}

/// Convert parser [`Value`] to [`ValueKind`] using the target type (canonicalizes ratio at bind).
pub fn parser_value_to_value_kind(
    value: &crate::literals::Value,
    type_spec: &TypeSpecification,
) -> Result<ValueKind, String> {
    use crate::computation::rational::decimal_to_rational;
    use crate::literals::Value;
    match (value, type_spec) {
        (Value::NumberWithUnit(magnitude, unit_name), TypeSpecification::Ratio { units, .. }) => {
            use crate::computation::rational::checked_div;
            let unit = units.get(unit_name.as_str())?;
            let magnitude_rational = decimal_to_rational(*magnitude)
                .map_err(|failure| format!("ratio literal failed rational lift: {failure}"))?;
            let canonical_rational = checked_div(&magnitude_rational, &unit.value)
                .map_err(|failure| format!("ratio literal: unit conversion failed: {failure}"))?;
            Ok(ValueKind::Ratio(canonical_rational))
        }
        (Value::NumberWithUnit(magnitude, unit_name), TypeSpecification::Measure { units, .. }) => {
            use crate::computation::rational::checked_mul;
            let rational = lift_parser_decimal(*magnitude)?;
            let unit = units.get(unit_name.as_str())?;
            let canonical = checked_mul(&rational, &unit.factor)
                .map_err(|failure| format!("measure canonicalization overflow: {failure}"))?;
            Ok(ValueKind::Measure(canonical))
        }
        (Value::NumberWithUnit(_, _), _) => {
            Err("number_with_unit literal requires a measure or ratio type".to_string())
        }
        (Value::Number(n), TypeSpecification::Number { .. }) => {
            Ok(ValueKind::Number(lift_parser_decimal(*n)?))
        }
        (Value::Number(n), TypeSpecification::Ratio { .. }) => {
            let r = decimal_to_rational(*n)
                .map_err(|failure| format!("ratio literal failed rational lift: {failure}"))?;
            Ok(ValueKind::Ratio(r))
        }
        (Value::Text(s), TypeSpecification::Text { .. }) => Ok(ValueKind::Text(s.clone())),
        (Value::Boolean(b), TypeSpecification::Boolean { .. }) => Ok(ValueKind::Boolean(b.into())),
        (Value::Date(dt), TypeSpecification::Date { .. }) => {
            Ok(ValueKind::Date(date_time_to_semantic(dt)))
        }
        (Value::Time(t), TypeSpecification::Time { .. }) => {
            Ok(ValueKind::Time(time_to_semantic(t)))
        }
        (
            Value::Range(left, right),
            range_spec @ (TypeSpecification::NumberRange { .. }
            | TypeSpecification::DateRange { .. }
            | TypeSpecification::TimeRange { .. }
            | TypeSpecification::RatioRange { .. }
            | TypeSpecification::MeasureRange { .. }),
        ) => {
            let endpoint = range_element_type_specification(range_spec).ok_or_else(|| {
                "BUG: range_element_type_specification missing arm for range type".to_string()
            })?;
            let left_lit = lift_range_endpoint(left, &endpoint)?;
            let right_lit = lift_range_endpoint(right, &endpoint)?;
            Ok(ValueKind::Range(
                Box::new(left_lit.to_literal()),
                Box::new(right_lit.to_literal()),
            ))
        }
        (value, type_spec) => Err(parser_value_type_mismatch(value, type_spec)),
    }
}

/// Convert parser Value to ValueKind for primitives and ranges only.
///
/// [`Value::NumberWithUnit`] requires [`parser_value_to_value_kind`] with a measure or ratio type.
pub fn value_to_semantic(value: &crate::parsing::ast::Value) -> Result<ValueKind, String> {
    use crate::parsing::ast::Value;
    Ok(match value {
        Value::Number(n) => ValueKind::Number(lift_parser_decimal(*n)?),
        Value::Text(s) => ValueKind::Text(s.clone()),
        Value::Boolean(b) => ValueKind::Boolean(bool::from(*b)),
        Value::Date(dt) => ValueKind::Date(date_time_to_semantic(dt)),
        Value::Time(t) => ValueKind::Time(time_to_semantic(t)),
        Value::NumberWithUnit(_, _) => {
            return Err(
                "number_with_unit literal requires type context (measure or ratio)".to_string(),
            );
        }
        Value::Range(_, _) => literal_value_from_parser_value(value)?.value,
    })
}

/// Convert AST date-time to semantic (for tests and planning).
pub(crate) fn date_time_to_semantic(dt: &crate::parsing::ast::DateTimeValue) -> SemanticDateTime {
    SemanticDateTime {
        year: dt.year,
        month: dt.month,
        day: dt.day,
        hour: dt.hour,
        minute: dt.minute,
        second: dt.second,
        microsecond: dt.microsecond,
        timezone: dt.timezone.as_ref().map(|tz| SemanticTimezone {
            offset_hours: tz.offset_hours,
            offset_minutes: tz.offset_minutes,
        }),
    }
}

/// Convert AST time to semantic (for tests and planning).
pub(crate) fn time_to_semantic(t: &crate::parsing::ast::TimeValue) -> SemanticTime {
    SemanticTime {
        hour: t.hour.into(),
        minute: t.minute.into(),
        second: t.second.into(),
        microsecond: t.microsecond,
        timezone: t.timezone.as_ref().map(|tz| SemanticTimezone {
            offset_hours: tz.offset_hours,
            offset_minutes: tz.offset_minutes,
        }),
    }
}

/// Compare two semantic date-time values by year, month, day, hour, minute,
/// second, then microsecond. Timezone normalisation is a separate concern
/// handled at evaluation time.
pub(crate) fn compare_semantic_dates(
    left: &SemanticDateTime,
    right: &SemanticDateTime,
) -> std::cmp::Ordering {
    left.year
        .cmp(&right.year)
        .then_with(|| left.month.cmp(&right.month))
        .then_with(|| left.day.cmp(&right.day))
        .then_with(|| left.hour.cmp(&right.hour))
        .then_with(|| left.minute.cmp(&right.minute))
        .then_with(|| left.second.cmp(&right.second))
        .then_with(|| left.microsecond.cmp(&right.microsecond))
}

/// Compare two semantic time values by hour, minute, second, then microsecond.
/// Timezone is excluded for the same reason as [`compare_semantic_dates`].
pub(crate) fn compare_semantic_times(
    left: &SemanticTime,
    right: &SemanticTime,
) -> std::cmp::Ordering {
    left.hour
        .cmp(&right.hour)
        .then_with(|| left.minute.cmp(&right.minute))
        .then_with(|| left.second.cmp(&right.second))
        .then_with(|| left.microsecond.cmp(&right.microsecond))
}

/// Convert AST conversion target to semantic (planning boundary; evaluation/computation use only semantic).
pub fn conversion_target_to_semantic(
    ct: &ConversionTarget,
    unit_index: Option<&crate::planning::unit_index::UnitIndex>,
    resolved_types: Option<&IndexMap<String, Arc<LemmaType>>>,
) -> Result<SemanticConversionTarget, String> {
    match ct {
        ConversionTarget::Type(kind) => Ok(SemanticConversionTarget::Type(*kind)),
        ConversionTarget::Unit { unit_name } => {
            let index = unit_index.ok_or_else(|| format!("Unknown unit '{unit_name}'."))?;
            let (bare, owning_type) = match resolved_types {
                Some(resolved) => index.resolve_with_named_types(unit_name, resolved)?,
                None => index.resolve(unit_name)?,
            };
            Ok(SemanticConversionTarget::Unit {
                unit_name: bare,
                owning_type,
            })
        }
    }
}

// -----------------------------------------------------------------------------
// Primitive type constructors (moved from parsing::ast)
// -----------------------------------------------------------------------------

// Statics for lazy initialization of production-used primitive types.
static PRIMITIVE_BOOLEAN: OnceLock<Arc<LemmaType>> = OnceLock::new();
static PRIMITIVE_NUMBER: OnceLock<Arc<LemmaType>> = OnceLock::new();
static PRIMITIVE_TEXT: OnceLock<Arc<LemmaType>> = OnceLock::new();
static PRIMITIVE_DATE: OnceLock<Arc<LemmaType>> = OnceLock::new();
static PRIMITIVE_DATE_RANGE: OnceLock<Arc<LemmaType>> = OnceLock::new();
static PRIMITIVE_TIME: OnceLock<Arc<LemmaType>> = OnceLock::new();
static PRIMITIVE_RATIO: OnceLock<Arc<LemmaType>> = OnceLock::new();

#[must_use]
pub fn primitive_boolean_arc() -> &'static Arc<LemmaType> {
    PRIMITIVE_BOOLEAN.get_or_init(|| Arc::new(LemmaType::primitive(TypeSpecification::boolean())))
}

#[must_use]
pub fn primitive_number_arc() -> &'static Arc<LemmaType> {
    PRIMITIVE_NUMBER.get_or_init(|| Arc::new(LemmaType::primitive(TypeSpecification::number())))
}

#[must_use]
pub fn primitive_text_arc() -> &'static Arc<LemmaType> {
    PRIMITIVE_TEXT.get_or_init(|| Arc::new(LemmaType::primitive(TypeSpecification::text())))
}

#[must_use]
pub fn primitive_date_arc() -> &'static Arc<LemmaType> {
    PRIMITIVE_DATE.get_or_init(|| Arc::new(LemmaType::primitive(TypeSpecification::date())))
}

#[must_use]
pub fn primitive_date_range_arc() -> &'static Arc<LemmaType> {
    PRIMITIVE_DATE_RANGE
        .get_or_init(|| Arc::new(LemmaType::primitive(TypeSpecification::date_range())))
}

#[must_use]
pub fn primitive_time_arc() -> &'static Arc<LemmaType> {
    PRIMITIVE_TIME.get_or_init(|| Arc::new(LemmaType::primitive(TypeSpecification::time())))
}

#[must_use]
pub fn primitive_ratio_arc() -> &'static Arc<LemmaType> {
    PRIMITIVE_RATIO.get_or_init(|| Arc::new(LemmaType::primitive(TypeSpecification::ratio())))
}

/// Map PrimitiveKind to TypeSpecification. Single source of truth for primitive type resolution.
#[must_use]
pub fn type_spec_for_primitive(kind: PrimitiveKind) -> TypeSpecification {
    match kind {
        PrimitiveKind::Boolean => TypeSpecification::boolean(),
        PrimitiveKind::Measure => TypeSpecification::measure(),
        PrimitiveKind::MeasureRange => TypeSpecification::measure_range(),
        PrimitiveKind::Number => TypeSpecification::number(),
        PrimitiveKind::NumberRange => TypeSpecification::number_range(),
        PrimitiveKind::Ratio => TypeSpecification::ratio(),
        PrimitiveKind::RatioRange => TypeSpecification::ratio_range(),
        PrimitiveKind::Text => TypeSpecification::text(),
        PrimitiveKind::Date => TypeSpecification::date(),
        PrimitiveKind::DateRange => TypeSpecification::date_range(),
        PrimitiveKind::Time => TypeSpecification::time(),
        PrimitiveKind::TimeRange => TypeSpecification::time_range(),
    }
}

// -----------------------------------------------------------------------------
// Display implementations
// -----------------------------------------------------------------------------

impl fmt::Display for PathSegment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repository {
            Some(repository) => write!(f, "{} → {} {}", self.uses, repository, self.spec),
            None => write!(f, "{} → {}", self.uses, self.spec),
        }
    }
}

impl fmt::Display for DataPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.input_key())
    }
}

impl fmt::Display for RulePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.input_key())
    }
}

impl fmt::Display for LemmaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

pub(crate) fn format_decimal_for_api(
    decimal: rust_decimal::Decimal,
    decimal_places: Option<u8>,
) -> rust_decimal::Decimal {
    match decimal_places {
        Some(decimal_places) => {
            let places = u32::from(decimal_places);
            let mut rounded = decimal.round_dp(places);
            rounded.rescale(places);
            rounded
        }
        None => {
            let normalized = decimal.normalize();
            if normalized.fract().is_zero() {
                normalized.trunc()
            } else {
                normalized
            }
        }
    }
}

fn format_decimal_for_human_display(
    decimal: rust_decimal::Decimal,
    decimal_places: Option<u8>,
) -> String {
    match decimal_places {
        Some(decimal_places) => {
            let rounded = decimal.round_dp(u32::from(decimal_places));
            format!("{:.prec$}", rounded, prec = decimal_places as usize)
        }
        None => decimal.normalize().to_string(),
    }
}

fn format_rational_for_human_display(
    magnitude: &crate::computation::rational::RationalInteger,
    decimal_places: Option<u8>,
) -> String {
    match magnitude.try_to_decimal() {
        Ok(decimal) => format_decimal_for_human_display(decimal, decimal_places),
        Err(crate::computation::rational::NumericFailure::Overflow) => magnitude.display_str(),
        Err(_) => magnitude.display_str(),
    }
}

fn format_measure_canonical_for_display(
    canonical: &crate::computation::rational::RationalInteger,
    lemma_type: &LemmaType,
    signature: &[(String, i32)],
) -> String {
    use crate::computation::rational::checked_div;

    let decimals = lemma_type.decimal_places();

    if let TypeSpecification::Measure { units, .. } = &lemma_type.specifications {
        if !units.is_empty() {
            // Binding (`as` / suggest / fill) must name a declared unit; otherwise first declared.
            let unit = match lemma_type.measure_binding_unit.as_deref() {
                Some(binding) => units
                    .iter()
                    .find(|unit| unit.name == binding)
                    .unwrap_or_else(|| {
                        panic!(
                            "BUG: measure display binding '{binding}' is not a declared unit on type {}",
                            lemma_type.name()
                        )
                    }),
                None => units
                    .iter()
                    .next()
                    .expect("BUG: measure type with non-empty units must have a first unit"),
            };
            let in_unit = checked_div(canonical, &unit.factor)
                .expect("BUG: de-canonicalization for measure display must not fail");
            let formatted = format_rational_for_human_display(&in_unit, decimals);
            return format!("{} {}", formatted, unit.name);
        }
    }

    let unit_label = match signature {
        [] => String::new(),
        [(name, 1)] => name.clone(),
        _ => format_signature_operator_style(signature),
    };
    let formatted = format_rational_for_human_display(canonical, decimals);
    if unit_label.is_empty() {
        formatted
    } else {
        format!("{formatted} {unit_label}")
    }
}

fn format_ratio_canonical_for_display(
    canonical: &crate::computation::rational::RationalInteger,
    lemma_type: &LemmaType,
) -> String {
    use crate::computation::rational::{checked_mul, rational_new};

    let display_unit = lemma_type
        .measure_binding_unit
        .as_deref()
        .or_else(|| lemma_type.ratio_primary_unit());

    match display_unit {
        Some("percent") => match checked_mul(canonical, &rational_new(100, 1)) {
            Ok(scaled) => format_number_with_unit_for_display(&scaled, "percent"),
            Err(_) => format!("{} percent", canonical.display_str()),
        },
        Some("permille") => match checked_mul(canonical, &rational_new(1000, 1)) {
            Ok(scaled) => format_number_with_unit_for_display(&scaled, "permille"),
            Err(_) => format!("{} permille", canonical.display_str()),
        },
        Some(unit_name) => format_number_with_unit_for_display(canonical, unit_name),
        None => format_rational_magnitude_for_display(canonical),
    }
}

impl fmt::Display for LiteralValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Measure/ratio display needs an explicit type; callers must use
        // `display_value_with_type`. Bare Display only covers type-free kinds and
        // ranges of those kinds.
        match &self.value {
            ValueKind::Measure(_) | ValueKind::Ratio(_) => {
                write!(f, "{}", self.value)
            }
            ValueKind::Range(left, right) => write!(f, "{}...{}", left, right),
            _ => write!(f, "{}", self.value),
        }
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::computation::rational::decimal_to_rational;
    use crate::literals::DateGranularity;
    use crate::literals::Value;
    use crate::parsing::ast::{BooleanValue, DateTimeValue, PrimitiveKind, TimeValue};
    use rust_decimal::Decimal;
    use std::str::FromStr;
    use std::sync::{Arc, OnceLock};

    static PRIMITIVE_MEASURE: OnceLock<Arc<LemmaType>> = OnceLock::new();

    #[must_use]
    pub(crate) fn primitive_measure_arc() -> &'static Arc<LemmaType> {
        PRIMITIVE_MEASURE
            .get_or_init(|| Arc::new(LemmaType::primitive(TypeSpecification::measure())))
    }

    #[must_use]
    pub(crate) fn primitive_measure() -> &'static LemmaType {
        primitive_measure_arc().as_ref()
    }

    #[test]
    fn default_primitive_help_is_goal_oriented() {
        let kinds = [
            PrimitiveKind::Boolean,
            PrimitiveKind::Measure,
            PrimitiveKind::MeasureRange,
            PrimitiveKind::Number,
            PrimitiveKind::NumberRange,
            PrimitiveKind::Ratio,
            PrimitiveKind::RatioRange,
            PrimitiveKind::Text,
            PrimitiveKind::Date,
            PrimitiveKind::DateRange,
            PrimitiveKind::Time,
            PrimitiveKind::TimeRange,
        ];
        for kind in kinds {
            let spec = type_spec_for_primitive(kind);
            let help = match &spec {
                TypeSpecification::Boolean { help, .. }
                | TypeSpecification::Number { help, .. }
                | TypeSpecification::NumberRange { help, .. }
                | TypeSpecification::Text { help, .. }
                | TypeSpecification::Measure { help, .. }
                | TypeSpecification::MeasureRange { help, .. }
                | TypeSpecification::Ratio { help, .. }
                | TypeSpecification::RatioRange { help, .. }
                | TypeSpecification::Date { help, .. }
                | TypeSpecification::DateRange { help, .. }
                | TypeSpecification::TimeRange { help, .. }
                | TypeSpecification::Time { help, .. } => help,
                TypeSpecification::Veto { .. } | TypeSpecification::Undetermined => {
                    unreachable!(
                        "BUG: primitive kind {:?} mapped to non-primitive spec",
                        kind
                    )
                }
            };
            assert!(!help.is_empty(), "help for {:?}", kind);
            assert!(
                !help.to_ascii_lowercase().contains("format:"),
                "help for {:?} must not describe syntax: {:?}",
                kind,
                help
            );
            assert_eq!(help, default_help_for_primitive(kind));
        }
    }

    #[test]
    fn data_path_and_rule_path_display_are_input_key() {
        let implicit = PathSegment {
            uses: "bag".to_string(),
            repository: None,
            spec: "bag".to_string(),
        };
        let explicit = PathSegment {
            uses: "bag".to_string(),
            repository: None,
            spec: "nut_bag".to_string(),
        };
        let named_repo = PathSegment {
            uses: "calc".to_string(),
            repository: Some("alpha".to_string()),
            spec: "pricing".to_string(),
        };

        assert_eq!(
            DataPath::new(vec![implicit.clone()], "weight".to_string()).to_string(),
            "bag.weight"
        );
        assert_eq!(
            DataPath::new(vec![explicit.clone()], "weight".to_string()).to_string(),
            "bag.weight"
        );
        assert_eq!(
            DataPath::new(vec![named_repo.clone()], "price".to_string()).to_string(),
            "calc.price"
        );
        assert_eq!(
            RulePath::new(vec![implicit], "cost".to_string()).to_string(),
            "bag.cost"
        );
        assert_eq!(
            RulePath::new(vec![explicit], "cost".to_string()).to_string(),
            "bag.cost"
        );
        assert_eq!(
            RulePath::new(vec![named_repo], "price".to_string()).to_string(),
            "calc.price"
        );

        assert_eq!(
            PathSegment {
                uses: "bag".to_string(),
                repository: None,
                spec: "nut_bag".to_string(),
            }
            .to_string(),
            "bag → nut_bag"
        );
        assert_eq!(
            PathSegment {
                uses: "calc".to_string(),
                repository: Some("alpha".to_string()),
                spec: "pricing".to_string(),
            }
            .to_string(),
            "calc → alpha pricing"
        );
    }

    #[test]
    fn test_negated_comparison() {
        assert_eq!(
            negated_comparison(ComparisonComputation::LessThan),
            ComparisonComputation::GreaterThanOrEqual
        );
        assert_eq!(
            negated_comparison(ComparisonComputation::GreaterThanOrEqual),
            ComparisonComputation::LessThan
        );
        assert_eq!(
            negated_comparison(ComparisonComputation::Is),
            ComparisonComputation::IsNot
        );
        assert_eq!(
            negated_comparison(ComparisonComputation::IsNot),
            ComparisonComputation::Is
        );
    }

    #[test]
    fn value_to_semantic_number_is_decimal() {
        let kind = value_to_semantic(&Value::Number(Decimal::from(42))).unwrap();
        assert!(matches!(kind, ValueKind::Number(d) if d == rational_new(42, 1)));
    }

    #[test]
    fn value_kind_measure_serializes_magnitude_only() {
        let kind =
            ValueKind::Measure(decimal_to_rational(Decimal::from_str("99.50").unwrap()).unwrap());
        let json = serde_json::to_value(crate::api::ValueKind::from(&kind)).unwrap();
        assert_eq!(json["measure"]["value"], "99.5");
        assert!(json["measure"].get("signature").is_none());
    }

    #[test]
    fn value_kind_measure_roundtrips() {
        let original =
            ValueKind::Measure(decimal_to_rational(Decimal::from_str("4800").unwrap()).unwrap());
        let api = crate::api::ValueKind::from(&original);
        let json = serde_json::to_string(&api).unwrap();
        let parsed: crate::api::ValueKind = serde_json::from_str(&json).unwrap();
        assert_eq!(api, parsed);
    }

    #[test]
    fn value_kind_measure_empty_roundtrips() {
        let original =
            ValueKind::Measure(decimal_to_rational(Decimal::from_str("12.5").unwrap()).unwrap());
        let api = crate::api::ValueKind::from(&original);
        let json = serde_json::to_string(&api).unwrap();
        let parsed: crate::api::ValueKind = serde_json::from_str(&json).unwrap();
        assert_eq!(api, parsed);
    }

    #[test]
    fn literal_value_number_serde_not_rational_array() {
        let lit = LiteralValue::number_from_decimal(Decimal::from(20));
        let json = serde_json::to_value(crate::api::LiteralValue::from(&lit)).unwrap();
        let number = json
            .get("value")
            .and_then(|v| v.get("number"))
            .expect("number field");
        assert!(number.is_string());
        assert_eq!(number.as_str(), Some("20"));
        assert!(
            !number.is_array(),
            "stored number must not serialize as [n,d]"
        );
    }

    #[test]
    fn test_literal_value_to_primitive_type() {
        let one = rational_new(1, 1);

        assert_eq!(TypedLiteral::text("".to_string()).lemma_type.name(), "text");
        assert_eq!(
            TypedLiteral::number(one.clone()).lemma_type.name(),
            "number"
        );
        assert_eq!(
            TypedLiteral::from_bool(bool::from(BooleanValue::True))
                .lemma_type
                .name(),
            "boolean"
        );

        let dt = DateTimeValue {
            year: 2024,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,

            granularity: DateGranularity::Full,
        };
        assert_eq!(
            TypedLiteral::date(date_time_to_semantic(&dt))
                .lemma_type
                .name(),
            "date"
        );
        assert_eq!(
            TypedLiteral::ratio_from_decimal(Decimal::new(1, 2))
                .lemma_type
                .name(),
            "ratio"
        );
        let dur_type = LemmaType::new(
            "duration".to_string(),
            TypeSpecification::Measure {
                minimum: None,
                maximum: None,
                decimals: None,
                units: MeasureUnits::from(vec![MeasureUnit {
                    name: "second".to_string(),
                    factor: crate::computation::rational::rational_one(),
                    derived_measure_factors: Vec::new(),
                    decomposition: BaseMeasureVector::new(),
                    minimum: None,
                    maximum: None,
                    suggestion_magnitude: None,
                }]),
                traits: vec![MeasureTrait::Duration],
                decomposition: None,
                help: String::new(),
            },
            TypeExtends::Primitive,
        );
        assert_eq!(
            TypedLiteral::measure_with_type(one.clone(), Arc::new(dur_type))
                .lemma_type
                .name(),
            "duration"
        );
    }

    #[test]
    fn test_type_display() {
        let specs = TypeSpecification::text();
        let lemma_type = LemmaType::new("name".to_string(), specs, TypeExtends::Primitive);
        assert_eq!(format!("{}", lemma_type), "name");
    }

    #[test]
    fn test_type_serialization() {
        let specs = TypeSpecification::number();
        let lemma_type = LemmaType::new("dice".to_string(), specs, TypeExtends::Primitive);
        let api = crate::api::LemmaType::from(&lemma_type);
        let serialized = serde_json::to_string(&api).unwrap();
        let deserialized: crate::api::LemmaType = serde_json::from_str(&serialized).unwrap();
        assert_eq!(api, deserialized);
    }

    #[test]
    fn test_literal_value_display_value() {
        let ten = rational_new(10, 1);

        assert_eq!(
            LiteralValue::text("hello".to_string()).display_value(),
            "hello"
        );
        assert_eq!(LiteralValue::number(ten).display_value(), "10");
        assert_eq!(LiteralValue::from_bool(true).display_value(), "true");
        assert_eq!(LiteralValue::from_bool(false).display_value(), "false");

        // 0.10 ratio with "percent" binding displays as 10% (unit conversion applied)
        let ratio_ty = Arc::new(
            primitive_ratio_arc()
                .as_ref()
                .clone()
                .with_measure_binding_unit("percent"),
        );
        let ten_percent_ratio = LiteralValue::ratio(
            crate::literals::rational_from_parsed_decimal(Decimal::new(1, 1))
                .expect("ratio decimal"),
        );
        assert_eq!(
            ten_percent_ratio.display_value_with_type(ratio_ty.as_ref()),
            "10%"
        );

        let time = TimeValue {
            hour: 14,
            minute: 30,
            second: 0,
            microsecond: 0,
            timezone: None,
        };
        let time_display = LiteralValue::time(time_to_semantic(&time)).display_value();
        assert!(time_display.contains("14"));
        assert!(time_display.contains("30"));
    }

    #[test]
    fn test_measure_display_respects_type_decimals() {
        let money_type = LemmaType {
            name: Some("money".to_string()),
            specifications: TypeSpecification::Measure {
                minimum: None,
                maximum: None,
                decimals: Some(2),
                units: MeasureUnits::from(vec![MeasureUnit {
                    name: "eur".to_string(),
                    factor: crate::computation::rational::rational_one(),
                    derived_measure_factors: Vec::new(),
                    decomposition: BaseMeasureVector::new(),
                    minimum: None,
                    maximum: None,
                    suggestion_magnitude: None,
                }]),
                traits: Vec::new(),
                decomposition: None,
                help: String::new(),
            },
            extends: TypeExtends::Primitive,
            measure_binding_unit: None,
        };
        let money_type = Arc::new(money_type);
        let val = LiteralValue::measure_with_type(
            decimal_to_rational(Decimal::from_str("1.8").unwrap()).unwrap(),
            money_type.clone(),
        );
        assert_eq!(val.display_value_with_type(money_type.as_ref()), "1.80 eur");
        let more_precision = LiteralValue::measure_with_type(
            decimal_to_rational(Decimal::from_str("1.80000").unwrap()).unwrap(),
            money_type.clone(),
        );
        assert_eq!(
            more_precision.display_value_with_type(money_type.as_ref()),
            "1.80 eur"
        );
        let measure_no_decimals = Arc::new(LemmaType {
            name: Some("count".to_string()),
            specifications: TypeSpecification::Measure {
                minimum: None,
                maximum: None,
                decimals: None,
                units: MeasureUnits::from(vec![MeasureUnit {
                    name: "items".to_string(),
                    factor: crate::computation::rational::rational_one(),
                    derived_measure_factors: Vec::new(),
                    decomposition: BaseMeasureVector::new(),
                    minimum: None,
                    maximum: None,
                    suggestion_magnitude: None,
                }]),
                traits: Vec::new(),
                decomposition: None,
                help: String::new(),
            },
            extends: TypeExtends::Primitive,
            measure_binding_unit: None,
        });
        let val_any = LiteralValue::measure_with_type(
            decimal_to_rational(Decimal::from_str("42.50").unwrap()).unwrap(),
            Arc::clone(&measure_no_decimals),
        );
        assert_eq!(
            val_any.display_value_with_type(measure_no_decimals.as_ref()),
            "42.5 items"
        );
    }

    #[test]
    fn test_literal_value_time_type() {
        let time = TimeValue {
            hour: 14,
            minute: 30,
            second: 0,
            microsecond: 0,
            timezone: None,
        };
        let lit = TypedLiteral::time(time_to_semantic(&time));
        assert_eq!(lit.lemma_type.name(), "time");
    }

    #[test]
    fn test_measure_family_name_primitive_root() {
        let measure_spec = TypeSpecification::measure();
        let money_primitive = LemmaType::new(
            "money".to_string(),
            measure_spec.clone(),
            TypeExtends::Primitive,
        );
        assert_eq!(money_primitive.measure_family_name(), Some("money"));
    }

    #[test]
    fn test_measure_family_name_custom() {
        let measure_spec = TypeSpecification::measure();
        let money_custom = LemmaType::new(
            "money".to_string(),
            measure_spec,
            TypeExtends::custom_local("money".to_string(), "money".to_string()),
        );
        assert_eq!(money_custom.measure_family_name(), Some("money"));
    }

    #[test]
    fn test_same_measure_family_same_name_different_extends() {
        let measure_spec = TypeSpecification::measure();
        let money_primitive = LemmaType::new(
            "money".to_string(),
            measure_spec.clone(),
            TypeExtends::Primitive,
        );
        let money_custom = LemmaType::new(
            "money".to_string(),
            measure_spec,
            TypeExtends::custom_local("money".to_string(), "money".to_string()),
        );
        assert!(money_primitive.same_measure_family(&money_custom));
        assert!(money_custom.same_measure_family(&money_primitive));
    }

    #[test]
    fn test_same_measure_family_parent_and_child() {
        let measure_spec = TypeSpecification::measure();
        let type_x = LemmaType::new(
            "x".to_string(),
            measure_spec.clone(),
            TypeExtends::Primitive,
        );
        let type_x2 = LemmaType::new(
            "x2".to_string(),
            measure_spec,
            TypeExtends::custom_local("x".to_string(), "x".to_string()),
        );
        assert_eq!(type_x.measure_family_name(), Some("x"));
        assert_eq!(type_x2.measure_family_name(), Some("x"));
        assert!(type_x.same_measure_family(&type_x2));
        assert!(type_x2.same_measure_family(&type_x));
    }

    #[test]
    fn test_same_measure_family_siblings() {
        let measure_spec = TypeSpecification::measure();
        let type_x2_a = LemmaType::new(
            "x2a".to_string(),
            measure_spec.clone(),
            TypeExtends::custom_local("x".to_string(), "x".to_string()),
        );
        let type_x2_b = LemmaType::new(
            "x2b".to_string(),
            measure_spec,
            TypeExtends::custom_local("x".to_string(), "x".to_string()),
        );
        assert!(type_x2_a.same_measure_family(&type_x2_b));
    }

    #[test]
    fn test_same_measure_family_different_families() {
        let measure_spec = TypeSpecification::measure();
        let money = LemmaType::new(
            "money".to_string(),
            measure_spec.clone(),
            TypeExtends::Primitive,
        );
        let temperature = LemmaType::new(
            "temperature".to_string(),
            measure_spec,
            TypeExtends::Primitive,
        );
        assert!(!money.same_measure_family(&temperature));
        assert!(!temperature.same_measure_family(&money));
    }

    #[test]
    fn test_same_measure_family_measure_vs_non_measure() {
        let measure_spec = TypeSpecification::measure();
        let number_spec = TypeSpecification::number();
        let measure_type =
            LemmaType::new("money".to_string(), measure_spec, TypeExtends::Primitive);
        let number_type = LemmaType::new("amount".to_string(), number_spec, TypeExtends::Primitive);
        assert!(!measure_type.same_measure_family(&number_type));
        assert!(!number_type.same_measure_family(&measure_type));
    }

    #[test]
    fn test_same_measure_family_anonymous_measures_are_not_family_compatible() {
        let left = LemmaType::anonymous_for_decomposition(duration_decomposition());
        let right = LemmaType::anonymous_for_decomposition(duration_decomposition());

        assert!(!left.same_measure_family(&right));
        assert!(left.compatible_with_anonymous_measure(&right));
    }

    #[test]
    fn test_measure_family_name_non_measure_returns_none() {
        let number_spec = TypeSpecification::number();
        let number_type = LemmaType::new("amount".to_string(), number_spec, TypeExtends::Primitive);
        assert_eq!(number_type.measure_family_name(), None);
    }

    #[test]
    fn test_lemma_type_inequality_local_vs_import_same_shape() {
        let measure_spec = TypeSpecification::measure();
        let local = LemmaType::new(
            "t".to_string(),
            measure_spec.clone(),
            TypeExtends::custom_local("money".to_string(), "money".to_string()),
        );
        let imported = LemmaType::new(
            "t".to_string(),
            measure_spec,
            TypeExtends::Custom {
                parent: "money".to_string(),
                family: "money".to_string(),
                defining_spec: TypeDefiningSpec::Import,
            },
        );
        assert_ne!(local, imported);
    }

    #[test]
    fn test_lemma_type_equality_import_unit_variant() {
        let measure_spec = TypeSpecification::measure();
        let left = LemmaType::new(
            "t".to_string(),
            measure_spec.clone(),
            TypeExtends::Custom {
                parent: "money".to_string(),
                family: "money".to_string(),
                defining_spec: TypeDefiningSpec::Import,
            },
        );
        let right = LemmaType::new(
            "t".to_string(),
            measure_spec,
            TypeExtends::Custom {
                parent: "money".to_string(),
                family: "money".to_string(),
                defining_spec: TypeDefiningSpec::Import,
            },
        );
        assert_eq!(left, right);
    }

    fn month_suggestion_arg() -> CommandArg {
        CommandArg::Literal(crate::literals::Value::NumberWithUnit(
            Decimal::ONE,
            "month".to_string(),
        ))
    }

    fn unit_factor_arg(name: &str, factor: i64) -> [CommandArg; 2] {
        [
            CommandArg::Label(name.to_string()),
            CommandArg::UnitExpr(crate::parsing::ast::UnitArg::Factor(Decimal::from(factor))),
        ]
    }

    #[test]
    fn default_calendar_on_text_reports_hint() {
        let mut specs = TypeSpecification::text();
        let mut default = None;
        let err = specs
            .apply_constraint(
                "notes",
                TypeConstraintCommand::Suggest,
                &[month_suggestion_arg()],
                &mut default,
                &mut None,
            )
            .unwrap_err();
        assert!(err.contains("Unit 'month' is for calendar data"));
        assert!(err.contains("double quotes"));
    }

    #[test]
    fn default_calendar_on_duration_reports_valid_units() {
        let mut specs = TypeSpecification::measure();
        specs
            .apply_constraint(
                "duration",
                TypeConstraintCommand::Unit,
                &unit_factor_arg("second", 1),
                &mut None,
                &mut None,
            )
            .unwrap();
        specs
            .apply_constraint(
                "duration",
                TypeConstraintCommand::Unit,
                &unit_factor_arg("week", 604_800),
                &mut None,
                &mut None,
            )
            .unwrap();
        specs
            .apply_constraint(
                "duration",
                TypeConstraintCommand::Trait,
                &[CommandArg::Label("duration".to_string())],
                &mut None,
                &mut None,
            )
            .unwrap();
        let mut default = None;
        let err = specs
            .apply_constraint(
                "duration",
                TypeConstraintCommand::Suggest,
                &[month_suggestion_arg()],
                &mut default,
                &mut None,
            )
            .unwrap_err();
        assert!(err.contains("Unit 'month' is for calendar data"));
        assert!(err.contains("Valid 'duration' units are"));
        assert!(err.contains("week"));
    }

    #[test]
    fn default_valid_duration_weeks_accepted() {
        let mut specs = TypeSpecification::measure();
        specs
            .apply_constraint(
                "duration",
                TypeConstraintCommand::Unit,
                &unit_factor_arg("second", 1),
                &mut None,
                &mut None,
            )
            .unwrap();
        specs
            .apply_constraint(
                "duration",
                TypeConstraintCommand::Unit,
                &unit_factor_arg("week", 604_800),
                &mut None,
                &mut None,
            )
            .unwrap();
        specs
            .apply_constraint(
                "duration",
                TypeConstraintCommand::Trait,
                &[CommandArg::Label("duration".to_string())],
                &mut None,
                &mut None,
            )
            .unwrap();
        let mut default = None;
        specs
            .apply_constraint(
                "duration",
                TypeConstraintCommand::Suggest,
                &[CommandArg::Literal(crate::literals::Value::NumberWithUnit(
                    Decimal::from(4),
                    "week".to_string(),
                ))],
                &mut default,
                &mut None,
            )
            .unwrap();
        assert!(matches!(
            default,
            Some(RawSuggestion::Measure {
                unit_name,
                ..
            }) if unit_name == "week"
        ));
    }

    #[test]
    fn default_unknown_unit_on_duration_lists_valid_units() {
        let mut specs = TypeSpecification::measure();
        specs
            .apply_constraint(
                "duration",
                TypeConstraintCommand::Unit,
                &unit_factor_arg("second", 1),
                &mut None,
                &mut None,
            )
            .unwrap();
        specs
            .apply_constraint(
                "duration",
                TypeConstraintCommand::Trait,
                &[CommandArg::Label("duration".to_string())],
                &mut None,
                &mut None,
            )
            .unwrap();
        let mut default = None;
        let err = specs
            .apply_constraint(
                "duration",
                TypeConstraintCommand::Suggest,
                &[CommandArg::Literal(crate::literals::Value::NumberWithUnit(
                    Decimal::ONE,
                    "fortnight".to_string(),
                ))],
                &mut default,
                &mut None,
            )
            .unwrap_err();
        assert!(err.contains("fortnight"));
        assert!(err.contains("not defined on 'duration'"));
        assert!(err.contains("Valid units are"));
    }

    fn money_measure_type() -> LemmaType {
        LemmaType::new(
            "Money".to_string(),
            TypeSpecification::Measure {
                minimum: None,
                maximum: None,
                decimals: None,
                units: MeasureUnits::from(vec![
                    MeasureUnit {
                        name: "eur".to_string(),
                        factor: crate::computation::rational::rational_one(),
                        derived_measure_factors: Vec::new(),
                        decomposition: BaseMeasureVector::new(),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    },
                    MeasureUnit {
                        name: "usd".to_string(),
                        factor: crate::computation::rational::decimal_to_rational(Decimal::new(
                            91, 2,
                        ))
                        .expect("factor"),
                        derived_measure_factors: Vec::new(),
                        decomposition: BaseMeasureVector::new(),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    },
                ]),
                traits: Vec::new(),
                decomposition: None,
                help: String::new(),
            },
            TypeExtends::Primitive,
        )
    }

    #[test]
    fn measure_unit_names_for_named_measure() {
        let money = money_measure_type();
        assert_eq!(money.measure_unit_names(), Some(vec!["eur", "usd"]));
    }

    // ---------------------------------------------------------------------------
    // Phase 0 — pin combine_signatures and canonicalize_signature behavior
    // ---------------------------------------------------------------------------

    fn sig(pairs: &[(&str, i32)]) -> Vec<(String, i32)> {
        pairs.iter().map(|(s, e)| (s.to_string(), *e)).collect()
    }

    #[test]
    fn combine_signatures_multiply_adds_exponents() {
        let left = sig(&[("eur", 1)]);
        let right = sig(&[("hour", -1)]);
        let result = combine_signatures(&left, &right, true);
        assert_eq!(result, sig(&[("eur", 1), ("hour", -1)]));
    }

    #[test]
    fn combine_signatures_divide_subtracts_exponents() {
        let left = sig(&[("eur", 1)]);
        let right = sig(&[("hour", 1)]);
        let result = combine_signatures(&left, &right, false);
        assert_eq!(result, sig(&[("eur", 1), ("hour", -1)]));
    }

    #[test]
    fn combine_signatures_cancels_to_empty() {
        let left = sig(&[("ce", 1), ("minute", -1)]);
        let right = sig(&[("minute", 1)]);
        let result = combine_signatures(&left, &right, true);
        // ce * (ce/min * min) = ce; minute cancels
        assert_eq!(result, sig(&[("ce", 1)]));
    }

    #[test]
    fn combine_signatures_output_is_canonical_form() {
        let left = sig(&[("eur", 1), ("hour", 1)]);
        let right = sig(&[("minute", 1)]);
        let result = combine_signatures(&left, &right, false); // divide
                                                               // [("eur",1),("hour",1)] / [("minute",1)] = [("eur",1),("hour",1),("minute",-1)]
        let expected = sig(&[("eur", 1), ("hour", 1), ("minute", -1)]);
        assert_eq!(result, expected);
    }

    #[test]
    fn canonicalize_signature_drops_zero_exponents() {
        let sig_with_zero = sig(&[("eur", 1), ("hour", 0), ("minute", -1)]);
        let result = canonicalize_signature(&sig_with_zero);
        assert_eq!(result, sig(&[("eur", 1), ("minute", -1)]));
    }

    #[test]
    fn canonicalize_signature_sorts_by_name() {
        let unsorted = sig(&[("minute", -1), ("eur", 1)]);
        let result = canonicalize_signature(&unsorted);
        assert_eq!(result, sig(&[("eur", 1), ("minute", -1)]));
    }

    // ---------------------------------------------------------------------------
    // Phase 0 — format_signature_operator_style (to be implemented in
    // signature_factor_and_display todo)
    // ---------------------------------------------------------------------------

    #[test]
    fn format_signature_operator_style_numerator_only() {
        let signature = sig(&[("eur", 1)]);
        let result = format_signature_operator_style(&signature);
        assert_eq!(result, "eur");
    }

    #[test]
    fn format_signature_operator_style_with_denominator() {
        let signature = sig(&[("eur", 1), ("hour", -1)]);
        let result = format_signature_operator_style(&signature);
        assert_eq!(result, "eur/hour");
    }

    #[test]
    fn format_signature_operator_style_denominator_only() {
        let signature = sig(&[("meter", -1)]);
        let result = format_signature_operator_style(&signature);
        assert_eq!(result, "1/meter");
    }

    #[test]
    fn format_signature_operator_style_with_exponents() {
        let signature = sig(&[("meter", 2), ("second", -2)]);
        let result = format_signature_operator_style(&signature);
        assert_eq!(result, "meter^2/second^2");
    }

    // ---------------------------------------------------------------------------
    // Phase 0 — calendar_unit_factor (to be implemented in builtin_calendar_factor_table)
    // ---------------------------------------------------------------------------

    #[test]
    fn calendar_unit_factor_table_completeness() {
        // Every SemanticCalendarUnit Display string must resolve to a factor.
        // Today SemanticCalendarUnit only has Month and Year; more may be added.
        for unit in &[SemanticCalendarUnit::Month, SemanticCalendarUnit::Year] {
            let name = unit.to_string();
            assert!(
                calendar_unit_factor(&name).is_some(),
                "calendar_unit_factor('{}') must return Some",
                name
            );
        }
    }

    #[test]
    fn semantic_calendar_unit_display_returns_singular() {
        // Today Month => "month", Year => "year" (plural).
        // After singular_calendar_names_everywhere, must be "month" and "year".
        assert_eq!(SemanticCalendarUnit::Month.to_string(), "month");
        assert_eq!(SemanticCalendarUnit::Year.to_string(), "year");
    }

    // ---------------------------------------------------------------------------
    // Phase 0 — signature_factor (to be implemented in signature_factor_and_display)
    // ---------------------------------------------------------------------------

    #[test]
    fn signature_factor_with_calendar_units() {
        let calendar = test_calendar_type_for_signature_factor();
        let unit_index = crate::planning::unit_index::UnitIndex::new();
        // month factor = 1, year factor = 12.
        // [(month,1),(year,-1)] = 1/12
        let sig_month_per_year = sig(&[("month", 1), ("year", -1)]);
        let factor = signature_factor(&sig_month_per_year, &unit_index, Some(&calendar))
            .expect("must not overflow");
        let expected = rational_new(1, 12);
        assert_eq!(factor, expected, "month/year factor must be 1/12");
    }

    fn test_calendar_type_for_signature_factor() -> LemmaType {
        use crate::computation::rational::{decimal_to_rational, rational_one};
        use crate::literals::{MeasureUnit, MeasureUnits};
        use rust_decimal::Decimal;
        LemmaType::new(
            "calendar".to_string(),
            TypeSpecification::Measure {
                minimum: None,
                maximum: None,
                decimals: None,
                units: MeasureUnits::from(vec![
                    MeasureUnit {
                        name: "month".to_string(),
                        factor: rational_one(),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                        decomposition: calendar_decomposition(),
                        derived_measure_factors: Vec::new(),
                    },
                    MeasureUnit {
                        name: "year".to_string(),
                        factor: decimal_to_rational(Decimal::from(12)).expect("year factor"),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                        decomposition: calendar_decomposition(),
                        derived_measure_factors: Vec::new(),
                    },
                ]),
                traits: vec![MeasureTrait::Calendar],
                decomposition: Some(calendar_decomposition()),
                help: String::new(),
            },
            TypeExtends::Primitive,
        )
    }

    #[test]
    #[should_panic(expected = "BUG: signature_factor called with unresolved unit name")]
    fn signature_factor_panics_on_unresolved_name() {
        let unit_index = crate::planning::unit_index::UnitIndex::new();
        let bad_sig = sig(&[("nonexistent_unit_xyz", 1)]);
        let _ = signature_factor(&bad_sig, &unit_index, None);
    }

    #[test]
    fn signature_factor_uses_owner_when_expression_index_empty() {
        let money = test_money_type_for_signature_factor();
        let expression_units = crate::planning::unit_index::UnitIndex::new();
        let sig_usd = sig(&[("usd", 1)]);
        let factor =
            signature_factor(&sig_usd, &expression_units, Some(&money)).expect("must not overflow");
        assert_eq!(factor, rational_new(91, 100));
    }

    fn test_money_type_for_signature_factor() -> LemmaType {
        use crate::computation::rational::decimal_to_rational;
        use crate::literals::{MeasureUnit, MeasureUnits};
        use rust_decimal::Decimal;
        LemmaType::new(
            "money".to_string(),
            TypeSpecification::Measure {
                minimum: None,
                maximum: None,
                decimals: Some(2),
                units: MeasureUnits::from(vec![
                    MeasureUnit {
                        name: "eur".to_string(),
                        factor: crate::computation::rational::rational_one(),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                        decomposition: BaseMeasureVector::new(),
                        derived_measure_factors: Vec::new(),
                    },
                    MeasureUnit {
                        name: "usd".to_string(),
                        factor: decimal_to_rational(Decimal::new(91, 2)).expect("usd factor"),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                        decomposition: BaseMeasureVector::new(),
                        derived_measure_factors: Vec::new(),
                    },
                ]),
                traits: Vec::new(),
                decomposition: None,
                help: String::new(),
            },
            TypeExtends::Primitive,
        )
    }

    fn measure_type_with_kilogram() -> TypeSpecification {
        use crate::computation::rational::rational_one;
        use crate::literals::{MeasureUnit, MeasureUnits};
        let mut units = MeasureUnits::new();
        units.push(MeasureUnit {
            name: "kilogram".to_string(),
            factor: rational_one(),
            minimum: None,
            maximum: None,
            suggestion_magnitude: None,
            decomposition: BaseMeasureVector::new(),
            derived_measure_factors: Vec::new(),
        });
        TypeSpecification::Measure {
            minimum: None,
            maximum: None,
            decimals: None,
            units,
            traits: Vec::new(),
            decomposition: None,
            help: String::new(),
        }
    }

    #[test]
    fn parser_value_to_value_kind_rejects_bare_number_for_measure() {
        let ten = Value::Number(Decimal::from(10));
        let err = parser_value_to_value_kind(&ten, &measure_type_with_kilogram())
            .expect_err("bare number must not bind to measure");
        assert!(
            err.contains("kilogram"),
            "error must hint expected unit, got: {err}"
        );
    }

    #[test]
    fn parser_value_to_value_kind_accepts_number_with_unit_for_measure() {
        let ten_kg = Value::NumberWithUnit(Decimal::from(10), "kilogram".to_string());
        let kind = parser_value_to_value_kind(&ten_kg, &measure_type_with_kilogram())
            .expect("10 kilogram must bind to measure");
        assert!(matches!(kind, ValueKind::Measure(_)));
    }

    #[test]
    fn parser_value_to_value_kind_accepts_bare_number_for_ratio() {
        let ten = Value::Number(Decimal::from(10));
        let kind =
            parser_value_to_value_kind(&ten, &TypeSpecification::ratio()).expect("number -> ratio");
        assert!(matches!(kind, ValueKind::Ratio(_)));
    }

    #[test]
    fn value_kind_matches_spec_rejects_number_for_measure() {
        let n = ValueKind::Number(rational_new(10, 1));
        assert!(!value_kind_matches_spec(&n, &measure_type_with_kilogram()));
    }

    #[test]
    fn apply_constraint_rejects_inherited_unit_factor_change() {
        let mut specs = TypeSpecification::measure();
        specs
            .apply_constraint(
                "money",
                TypeConstraintCommand::Unit,
                &unit_factor_arg("eur", 1),
                &mut None,
                &mut None,
            )
            .expect("seed eur");
        let err = specs
            .apply_constraint(
                "money",
                TypeConstraintCommand::Unit,
                &[
                    CommandArg::Label("eur".to_string()),
                    CommandArg::UnitExpr(crate::parsing::ast::UnitArg::Factor(Decimal::new(11, 1))),
                ],
                &mut None,
                &mut None,
            )
            .expect_err("must not change inherited unit factor");
        assert!(err.contains("eur"), "error must name unit, got: {err}");
        assert!(
            err.contains("inherited") || err.contains("cannot change"),
            "error must reject factor change, got: {err}"
        );
    }

    #[test]
    fn apply_constraint_allows_additive_unit_on_inherited_spec() {
        let mut specs = TypeSpecification::measure();
        specs
            .apply_constraint(
                "money",
                TypeConstraintCommand::Unit,
                &unit_factor_arg("eur", 1),
                &mut None,
                &mut None,
            )
            .expect("seed eur");
        specs
            .apply_constraint(
                "money",
                TypeConstraintCommand::Unit,
                &unit_factor_arg("usd", 1),
                &mut None,
                &mut None,
            )
            .expect("add usd");
        match &specs {
            TypeSpecification::Measure { units, .. } => assert_eq!(units.len(), 2),
            other => panic!("expected Measure, got {other:?}"),
        }
    }

    #[test]
    fn apply_constraint_idempotent_inherited_unit_redeclare() {
        let mut specs = TypeSpecification::measure();
        specs
            .apply_constraint(
                "money",
                TypeConstraintCommand::Unit,
                &unit_factor_arg("eur", 1),
                &mut None,
                &mut None,
            )
            .expect("seed eur");
        specs
            .apply_constraint(
                "money",
                TypeConstraintCommand::Unit,
                &unit_factor_arg("eur", 1),
                &mut None,
                &mut None,
            )
            .expect("idempotent eur");
        match &specs {
            TypeSpecification::Measure { units, .. } => {
                assert_eq!(units.len(), 1);
                assert_eq!(
                    units.iter().find(|u| u.name == "eur").expect("eur").factor,
                    crate::computation::rational::rational_one()
                );
            }
            other => panic!("expected Measure, got {other:?}"),
        }
    }

    #[test]
    fn element_from_range_returns_element_for_every_range_primitive() {
        type RangeElementMatcher = fn(&TypeSpecification) -> bool;
        let cases: [(PrimitiveKind, RangeElementMatcher); 5] = [
            (PrimitiveKind::NumberRange, |element| {
                matches!(element, TypeSpecification::Number { .. })
            }),
            (PrimitiveKind::MeasureRange, |element| {
                matches!(element, TypeSpecification::Measure { .. })
            }),
            (PrimitiveKind::RatioRange, |element| {
                matches!(element, TypeSpecification::Ratio { .. })
            }),
            (PrimitiveKind::DateRange, |element| {
                matches!(element, TypeSpecification::Date { .. })
            }),
            (PrimitiveKind::TimeRange, |element| {
                matches!(element, TypeSpecification::Time { .. })
            }),
        ];
        for (kind, matches_element) in cases {
            let range_spec = type_spec_for_primitive(kind);
            let element = range_spec
                .element_from_range()
                .unwrap_or_else(|| panic!("{kind:?} must define element_from_range"));
            assert!(
                matches_element(&element),
                "{kind:?} element must match documented mapping, got {element:?}"
            );
        }
    }

    #[test]
    fn element_from_range_returns_none_for_non_range_primitives() {
        let non_range = [
            type_spec_for_primitive(PrimitiveKind::Boolean),
            type_spec_for_primitive(PrimitiveKind::Measure),
            TypeSpecification::Undetermined,
            TypeSpecification::veto(),
        ];
        for spec in non_range {
            assert!(
                spec.element_from_range().is_none(),
                "{spec:?} must not define element_from_range"
            );
        }
    }
}
