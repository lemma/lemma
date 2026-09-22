//! Literal value types and string parsing. No dependency on parsing/ast.
//! AST and planning re-export these types where needed.

use chrono::{Datelike, Timelike};
use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use crate::computation::rational::{self, RationalInteger};

// -----------------------------------------------------------------------------
// Dimensional decomposition type
// -----------------------------------------------------------------------------

/// A dimensional decomposition vector. Maps measure-type names to integer exponents.
/// For example, velocity `{length: 1, duration: -1}` or acceleration `{length: 1, duration: -2}`.
/// An empty map indicates a base measure (no decomposition) until the decomposition pass runs,
/// after which every measure carries a non-empty vector.
pub type BaseMeasureVector = BTreeMap<String, i32>;

// -----------------------------------------------------------------------------
// Unit tables for Measure and Ratio types
// -----------------------------------------------------------------------------

pub fn rational_from_parsed_decimal(decimal: Decimal) -> Result<RationalInteger, String> {
    rational::decimal_to_rational(decimal).map_err(|failure| failure.to_string())
}

/// A single unit within a Measure type.
///
/// `factor` is the conversion factor: 1 of this unit equals `factor` canonical units.
/// `derived_measure_factors` stores `(measure_ref, exponent)` pairs from compound unit declarations
/// (e.g., `meter/second` produces `[("meter", 1), ("second", -1)]`). Empty for base units.
/// `decomposition` is the dimensional decomposition vector, populated during the planning
/// decomposition pass. It is empty until that pass completes.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MeasureUnit {
    pub name: String,
    /// Conversion factor: 1 of this unit equals `value` canonical units.
    pub factor: RationalInteger,
    pub derived_measure_factors: Vec<(String, i32)>,
    pub decomposition: BaseMeasureVector,
    /// Minimum magnitude in this unit (schema/UI); canonical bound is on the type.
    pub minimum: Option<RationalInteger>,
    /// Maximum magnitude in this unit (schema/UI).
    pub maximum: Option<RationalInteger>,
    /// Default suggestion magnitude in this unit (schema/UI).
    pub suggestion_magnitude: Option<RationalInteger>,
}

impl MeasureUnit {
    pub fn from_decimal_factor(
        name: String,
        decimal_factor: Decimal,
        derived_measure_factors: Vec<(String, i32)>,
    ) -> Result<Self, String> {
        let factor =
            rational::decimal_to_rational(decimal_factor).map_err(|failure| failure.to_string())?;
        Ok(MeasureUnit {
            name,
            factor,
            derived_measure_factors,
            decomposition: BaseMeasureVector::new(),
            minimum: None,
            maximum: None,
            suggestion_magnitude: None,
        })
    }

    pub fn clear_constraint_magnitudes(&mut self) {
        self.minimum = None;
        self.maximum = None;
        self.suggestion_magnitude = None;
    }

    pub fn is_canonical_factor(&self) -> bool {
        self.factor == rational::rational_one()
    }

    pub fn is_positive_factor(&self) -> bool {
        self.factor.numer_is_positive()
    }

    /// Conversion factor as decimal (schema unit factors always commit).
    pub fn factor_decimal(&self) -> Decimal {
        rational::RationalInteger::try_to_decimal(&self.factor)
            .expect("BUG: measure unit factor must convert to decimal")
    }

    #[must_use]
    pub fn minimum_decimal(&self) -> Option<Decimal> {
        self.minimum.as_ref().map(|bound| {
            bound
                .try_to_decimal()
                .expect("BUG: planned measure unit minimum must convert to decimal")
        })
    }

    #[must_use]
    pub fn maximum_decimal(&self) -> Option<Decimal> {
        self.maximum.as_ref().map(|bound| {
            bound
                .try_to_decimal()
                .expect("BUG: planned measure unit maximum must convert to decimal")
        })
    }

    #[must_use]
    pub fn suggestion_magnitude_decimal(&self) -> Option<Decimal> {
        self.suggestion_magnitude.as_ref().map(|bound| {
            bound
                .try_to_decimal()
                .expect("BUG: planned measure unit default must convert to decimal")
        })
    }

    /// Maximum bound lifted to canonical units via `maximum * factor`.
    #[must_use]
    pub fn maximum_canonical_decimal(&self) -> Option<Decimal> {
        self.maximum.as_ref().map(|maximum| {
            let canonical = rational::checked_mul(maximum, &self.factor)
                .expect("BUG: planned measure unit maximum canonical multiply must succeed");
            canonical
                .try_to_decimal()
                .expect("BUG: planned measure unit maximum canonical must convert to decimal")
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MeasureUnits(pub Vec<MeasureUnit>);

impl MeasureUnits {
    pub fn new() -> Self {
        MeasureUnits(Vec::new())
    }
    pub fn get(&self, name: &str) -> Result<&MeasureUnit, String> {
        self.0.iter().find(|u| u.name == name).ok_or_else(|| {
            let valid: Vec<&str> = self.0.iter().map(|u| u.name.as_str()).collect();
            format!(
                "Unknown unit '{}' for this measure type. Valid units: {}",
                name,
                valid.join(", ")
            )
        })
    }

    pub fn iter(&self) -> std::slice::Iter<'_, MeasureUnit> {
        self.0.iter()
    }
    pub fn push(&mut self, u: MeasureUnit) {
        self.0.push(u);
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn map<F: FnMut(MeasureUnit) -> MeasureUnit>(self, f: F) -> Self {
        MeasureUnits(self.0.into_iter().map(f).collect())
    }
}

impl MeasureUnit {
    pub fn with_decomposition(self, decomposition: BaseMeasureVector) -> Self {
        Self {
            decomposition,
            ..self
        }
    }
    pub fn with_factor(self, factor: RationalInteger) -> Self {
        Self { factor, ..self }
    }
    pub fn with_derived_measure_factors(self, derived_measure_factors: Vec<(String, i32)>) -> Self {
        Self {
            derived_measure_factors,
            ..self
        }
    }
}

impl Default for MeasureUnits {
    fn default() -> Self {
        MeasureUnits::new()
    }
}

impl From<Vec<MeasureUnit>> for MeasureUnits {
    fn from(v: Vec<MeasureUnit>) -> Self {
        MeasureUnits(v)
    }
}

impl<'a> IntoIterator for &'a MeasureUnits {
    type Item = &'a MeasureUnit;
    type IntoIter = std::slice::Iter<'a, MeasureUnit>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RatioUnit {
    pub name: String,
    pub value: RationalInteger,
    pub minimum: Option<RationalInteger>,
    pub maximum: Option<RationalInteger>,
    pub suggestion_magnitude: Option<RationalInteger>,
}

impl RatioUnit {
    pub fn clear_constraint_magnitudes(&mut self) {
        self.minimum = None;
        self.maximum = None;
        self.suggestion_magnitude = None;
    }

    /// Unit scale as decimal (schema ratio unit values always commit).
    pub fn value_decimal(&self) -> Decimal {
        self.value
            .try_to_decimal()
            .expect("BUG: ratio unit value must convert to decimal")
    }

    #[must_use]
    pub fn minimum_decimal(&self) -> Option<Decimal> {
        self.minimum.as_ref().map(|bound| {
            bound
                .try_to_decimal()
                .expect("BUG: planned ratio unit minimum must convert to decimal")
        })
    }

    #[must_use]
    pub fn maximum_decimal(&self) -> Option<Decimal> {
        self.maximum.as_ref().map(|bound| {
            bound
                .try_to_decimal()
                .expect("BUG: planned ratio unit maximum must convert to decimal")
        })
    }

    #[must_use]
    pub fn suggestion_magnitude_decimal(&self) -> Option<Decimal> {
        self.suggestion_magnitude.as_ref().map(|bound| {
            bound
                .try_to_decimal()
                .expect("BUG: planned ratio unit default must convert to decimal")
        })
    }

    /// Maximum bound lifted to canonical ratio space via `maximum * value`.
    #[must_use]
    pub fn maximum_canonical_decimal(&self) -> Option<Decimal> {
        self.maximum.as_ref().map(|maximum| {
            let canonical = rational::checked_mul(maximum, &self.value)
                .expect("BUG: planned ratio unit maximum canonical multiply must succeed");
            canonical
                .try_to_decimal()
                .expect("BUG: planned ratio unit maximum canonical must convert to decimal")
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RatioUnits(pub Vec<RatioUnit>);

impl RatioUnits {
    pub fn new() -> Self {
        RatioUnits(Vec::new())
    }
    pub fn get(&self, name: &str) -> Result<&RatioUnit, String> {
        self.0.iter().find(|u| u.name == name).ok_or_else(|| {
            let valid: Vec<&str> = self.0.iter().map(|u| u.name.as_str()).collect();
            format!(
                "Unknown unit '{}' for this ratio type. Valid units: {}",
                name,
                valid.join(", ")
            )
        })
    }

    pub fn iter(&self) -> std::slice::Iter<'_, RatioUnit> {
        self.0.iter()
    }
    pub fn push(&mut self, u: RatioUnit) {
        self.0.push(u);
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl Default for RatioUnits {
    fn default() -> Self {
        RatioUnits::new()
    }
}

impl From<Vec<RatioUnit>> for RatioUnits {
    fn from(v: Vec<RatioUnit>) -> Self {
        RatioUnits(v)
    }
}

impl<'a> IntoIterator for &'a RatioUnits {
    type Item = &'a RatioUnit;
    type IntoIter = std::slice::Iter<'a, RatioUnit>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

// -----------------------------------------------------------------------------
// Literal value types
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BooleanValue {
    True,
    False,
    Yes,
    No,
}

impl From<BooleanValue> for bool {
    fn from(value: BooleanValue) -> bool {
        matches!(value, BooleanValue::True | BooleanValue::Yes)
    }
}

impl From<&BooleanValue> for bool {
    fn from(value: &BooleanValue) -> bool {
        (*value).into() // Copy makes this ok
    }
}

impl From<bool> for BooleanValue {
    fn from(value: bool) -> BooleanValue {
        if value {
            BooleanValue::True
        } else {
            BooleanValue::False
        }
    }
}

impl std::ops::Not for BooleanValue {
    type Output = BooleanValue;

    fn not(self) -> Self::Output {
        if self.into() {
            BooleanValue::False
        } else {
            BooleanValue::True
        }
    }
}

impl std::ops::Not for &BooleanValue {
    type Output = BooleanValue;

    fn not(self) -> Self::Output {
        if (*self).into() {
            BooleanValue::False
        } else {
            BooleanValue::True
        }
    }
}

impl std::str::FromStr for BooleanValue {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "true" => Ok(BooleanValue::True),
            "false" => Ok(BooleanValue::False),
            "yes" => Ok(BooleanValue::Yes),
            "no" => Ok(BooleanValue::No),
            _ => Err(format!("Invalid boolean: '{}'", s)),
        }
    }
}

impl BooleanValue {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            BooleanValue::True => "true",
            BooleanValue::False => "false",
            BooleanValue::Yes => "yes",
            BooleanValue::No => "no",
        }
    }
}

impl fmt::Display for BooleanValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimezoneValue {
    pub offset_hours: i8,
    pub offset_minutes: u8,
}

impl fmt::Display for TimezoneValue {
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

impl Serialize for TimezoneValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for TimezoneValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl FromStr for TimezoneValue {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed == "Z" || trimmed == "z" {
            return Ok(Self {
                offset_hours: 0,
                offset_minutes: 0,
            });
        }
        if trimmed.len() == 6
            && (trimmed.starts_with('+') || trimmed.starts_with('-'))
            && trimmed.as_bytes()[3] == b':'
        {
            let offset_hours: i8 = trimmed[1..3]
                .parse()
                .map_err(|_| format!("Invalid timezone format: '{s}'"))?;
            let offset_minutes: u8 = trimmed[4..6]
                .parse()
                .map_err(|_| format!("Invalid timezone format: '{s}'"))?;
            if offset_hours > 23 || offset_minutes >= 60 {
                return Err(format!("Invalid timezone format: '{s}'"));
            }
            let signed_hours = if trimmed.starts_with('-') {
                -offset_hours
            } else {
                offset_hours
            };
            return Ok(Self {
                offset_hours: signed_hours,
                offset_minutes,
            });
        }
        Err(format!("Invalid timezone format: '{s}'"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TimeValue {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub microsecond: u32,
    pub timezone: Option<TimezoneValue>,
}

impl Serialize for TimeValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for TimeValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for TimeValue {
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

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DateGranularity {
    Year,
    YearMonth,
    /// ISO 8601 week date. Stores original (iso_year, week) because the ISO
    /// week year can differ from the calendar year — e.g. "2026-W01" has
    /// iso_year=2026 but the stored calendar date year=2025.
    IsoWeek {
        iso_year: i32,
        week: u32,
    },
    #[default]
    Full,
    DateTime,
}

#[derive(Debug, Clone)]
pub struct DateTimeValue {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub microsecond: u32,
    pub timezone: Option<TimezoneValue>,
    pub granularity: DateGranularity,
}

impl Serialize for DateTimeValue {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for DateTimeValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl PartialEq for DateTimeValue {
    fn eq(&self, other: &Self) -> bool {
        self.year == other.year
            && self.month == other.month
            && self.day == other.day
            && self.hour == other.hour
            && self.minute == other.minute
            && self.second == other.second
            && self.microsecond == other.microsecond
            && self.timezone == other.timezone
    }
}

impl Eq for DateTimeValue {}

impl PartialOrd for DateTimeValue {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DateTimeValue {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.year
            .cmp(&other.year)
            .then_with(|| self.month.cmp(&other.month))
            .then_with(|| self.day.cmp(&other.day))
            .then_with(|| self.hour.cmp(&other.hour))
            .then_with(|| self.minute.cmp(&other.minute))
            .then_with(|| self.second.cmp(&other.second))
            .then_with(|| self.microsecond.cmp(&other.microsecond))
            .then_with(|| self.timezone.cmp(&other.timezone))
    }
}

impl std::hash::Hash for DateTimeValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.year.hash(state);
        self.month.hash(state);
        self.day.hash(state);
        self.hour.hash(state);
        self.minute.hash(state);
        self.second.hash(state);
        self.microsecond.hash(state);
        self.timezone.hash(state);
    }
}

impl DateTimeValue {
    pub fn now() -> Self {
        let now = chrono::Local::now();
        let offset_secs = now.offset().local_minus_utc();
        Self {
            year: now.year(),
            month: now.month(),
            day: now.day(),
            hour: now.time().hour(),
            minute: now.time().minute(),
            second: now.time().second(),
            microsecond: now.time().nanosecond() / 1000 % 1_000_000,
            timezone: Some(TimezoneValue {
                offset_hours: (offset_secs / 3600) as i8,
                offset_minutes: ((offset_secs.abs() % 3600) / 60) as u8,
            }),
            granularity: DateGranularity::DateTime,
        }
    }

    fn parse_iso_week(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.split("-W").collect();
        if parts.len() != 2 {
            return None;
        }
        let iso_year: i32 = parts[0].parse().ok()?;
        let week: u32 = parts[1].parse().ok()?;
        if week == 0 || week > 53 {
            return None;
        }
        let date = chrono::NaiveDate::from_isoywd_opt(iso_year, week, chrono::Weekday::Mon)?;
        Some(Self {
            year: date.year(),
            month: date.month(),
            day: date.day(),
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,
            granularity: DateGranularity::IsoWeek { iso_year, week },
        })
    }
}

impl fmt::Display for DateTimeValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.granularity {
            DateGranularity::Year => write!(f, "{:04}", self.year),
            DateGranularity::YearMonth => write!(f, "{:04}-{:02}", self.year, self.month),
            DateGranularity::IsoWeek { iso_year, week } => {
                write!(f, "{:04}-W{:02}", iso_year, week)
            }
            DateGranularity::Full => {
                write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
            }
            DateGranularity::DateTime => {
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
}

/// Postcard / in-process serde for [`Decimal`]: mantissa + scale (no strings, no floats).
///
/// JSON API types in `lemma::api` use `serde-with-str` instead. Do not put this on
/// JSON-facing DTOs.
pub mod decimal_binary_serde {
    use rust_decimal::Decimal;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(value: &Decimal, serializer: S) -> Result<S::Ok, S::Error> {
        (value.mantissa(), value.scale()).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Decimal, D::Error> {
        let (mantissa, scale) = <(i128, u32)>::deserialize(deserializer)?;
        Decimal::try_from_i128_with_scale(mantissa, scale).map_err(serde::de::Error::custom)
    }
}

/// Literal value data (no type information). Single source of truth in literals.
///
/// `NumberWithUnit` is type-agnostic at parse time (`10 eur` and `50%` share this shape).
/// Planning resolves ratio vs measure via the unit index and target [`TypeSpecification`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Value {
    Number(#[serde(with = "decimal_binary_serde")] Decimal),
    NumberWithUnit(#[serde(with = "decimal_binary_serde")] Decimal, String),
    Text(String),
    Date(DateTimeValue),
    Time(TimeValue),
    Boolean(BooleanValue),
    Range(Box<Value>, Box<Value>),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Number(n) => write!(f, "{}", n),
            Value::Text(s) => write!(f, "{}", s),
            Value::Date(dt) => write!(f, "{}", dt),
            Value::Boolean(b) => write!(f, "{}", b),
            Value::Time(time) => write!(f, "{}", time),
            Value::NumberWithUnit(n, u) => match u.as_str() {
                "percent" => {
                    let norm = n.normalize();
                    let s = if norm.fract().is_zero() {
                        norm.trunc().to_string()
                    } else {
                        norm.to_string()
                    };
                    write!(f, "{}%", s)
                }
                "permille" => {
                    let norm = n.normalize();
                    let s = if norm.fract().is_zero() {
                        norm.trunc().to_string()
                    } else {
                        norm.to_string()
                    };
                    write!(f, "{}%%", s)
                }
                unit => {
                    let norm = n.normalize();
                    let s = if norm.fract().is_zero() {
                        norm.trunc().to_string()
                    } else {
                        norm.to_string()
                    };
                    write!(f, "{} {}", s, unit)
                }
            },
            Value::Range(left, right) => write!(f, "{}...{}", left, right),
        }
    }
}

// -----------------------------------------------------------------------------
// FromStr (single source of truth per type)
// -----------------------------------------------------------------------------

impl std::str::FromStr for DateTimeValue {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(dt) = s.parse::<chrono::DateTime<chrono::FixedOffset>>() {
            let offset = dt.offset().local_minus_utc();
            let microsecond = dt.nanosecond() / 1000 % 1_000_000;
            return Ok(DateTimeValue {
                year: dt.year(),
                month: dt.month(),
                day: dt.day(),
                hour: dt.hour(),
                minute: dt.minute(),
                second: dt.second(),
                microsecond,
                timezone: Some(TimezoneValue {
                    offset_hours: (offset / 3600) as i8,
                    offset_minutes: ((offset.abs() % 3600) / 60) as u8,
                }),
                granularity: DateGranularity::DateTime,
            });
        }
        if let Ok(dt) = s.parse::<chrono::NaiveDateTime>() {
            let microsecond = dt.nanosecond() / 1000 % 1_000_000;
            return Ok(DateTimeValue {
                year: dt.year(),
                month: dt.month(),
                day: dt.day(),
                hour: dt.hour(),
                minute: dt.minute(),
                second: dt.second(),
                microsecond,
                timezone: None,
                granularity: DateGranularity::DateTime,
            });
        }
        if let Ok(d) = s.parse::<chrono::NaiveDate>() {
            return Ok(DateTimeValue {
                year: d.year(),
                month: d.month(),
                day: d.day(),
                hour: 0,
                minute: 0,
                second: 0,
                microsecond: 0,
                timezone: None,
                granularity: DateGranularity::Full,
            });
        }
        if let Some(week_val) = Self::parse_iso_week(s) {
            return Ok(week_val);
        }
        if let Ok(ym) = chrono::NaiveDate::parse_from_str(&format!("{}-01", s), "%Y-%m-%d") {
            return Ok(Self {
                year: ym.year(),
                month: ym.month(),
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
                microsecond: 0,
                timezone: None,
                granularity: DateGranularity::YearMonth,
            });
        }
        if let Ok(year) = s.parse::<i32>() {
            if (1..=9999).contains(&year) {
                return Ok(Self {
                    year,
                    month: 1,
                    day: 1,
                    hour: 0,
                    minute: 0,
                    second: 0,
                    microsecond: 0,
                    timezone: None,
                    granularity: DateGranularity::Year,
                });
            }
        }
        Err(format!("Invalid date format: '{}'", s))
    }
}

impl std::str::FromStr for TimeValue {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();

        let (time_text, timezone) = if trimmed.ends_with('Z') || trimmed.ends_with('z') {
            (
                &trimmed[..trimmed.len() - 1],
                Some(TimezoneValue {
                    offset_hours: 0,
                    offset_minutes: 0,
                }),
            )
        } else if trimmed.len() > 1 {
            if let Some(sign_index) = trimmed[1..].rfind(['+', '-']).map(|index| index + 1) {
                let timezone_text = &trimmed[sign_index..];
                if timezone_text.len() == 6
                    && (timezone_text.starts_with('+') || timezone_text.starts_with('-'))
                    && timezone_text.as_bytes()[3] == b':'
                {
                    let timezone = TimezoneValue::from_str(timezone_text)
                        .map_err(|_| format!("Invalid time format: '{s}'"))?;
                    (&trimmed[..sign_index], Some(timezone))
                } else {
                    (trimmed, None)
                }
            } else {
                (trimmed, None)
            }
        } else {
            (trimmed, None)
        };

        if let Ok(t) = chrono::NaiveTime::parse_from_str(time_text, "%H:%M:%S%.f") {
            return Ok(TimeValue {
                hour: t.hour() as u8,
                minute: t.minute() as u8,
                second: t.second() as u8,
                microsecond: t.nanosecond() / 1000 % 1_000_000,
                timezone,
            });
        }
        if let Ok(t) = chrono::NaiveTime::parse_from_str(time_text, "%H:%M:%S") {
            return Ok(TimeValue {
                hour: t.hour() as u8,
                minute: t.minute() as u8,
                second: t.second() as u8,
                microsecond: 0,
                timezone,
            });
        }
        if let Ok(t) = chrono::NaiveTime::parse_from_str(time_text, "%H:%M") {
            return Ok(TimeValue {
                hour: t.hour() as u8,
                minute: t.minute() as u8,
                second: 0,
                microsecond: 0,
                timezone,
            });
        }
        Err(format!("Invalid time format: '{}'", s))
    }
}

/// Number literal with Lemma rules (strip _ and , separators).
///
/// Fractional digit count beyond [`Decimal::MAX_SCALE`] is rejected (no silent
/// truncate). An integer magnitude that cannot be represented is an error.
pub(crate) struct NumberLiteral(pub Decimal);

impl std::str::FromStr for NumberLiteral {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let clean = s.trim().replace(['_', ','], "");
        if let Some(dot) = clean.find('.') {
            let frac = &clean[dot + 1..];
            let frac_digits = frac.chars().take_while(|c| c.is_ascii_digit()).count();
            if frac_digits > Decimal::MAX_SCALE as usize {
                return Err(format!(
                    "Invalid number '{}': too many fractional digits (max {})",
                    s,
                    Decimal::MAX_SCALE
                ));
            }
        }
        Decimal::from_str(&clean)
            .map_err(|_| format!("Invalid number: '{}'", s))
            .map(NumberLiteral)
    }
}

/// Text literal with length limit.
pub(crate) struct TextLiteral(pub String);

impl std::str::FromStr for TextLiteral {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() > crate::limits::MAX_TEXT_VALUE_LENGTH {
            return Err(format!(
                "Text value exceeds maximum length (max {} characters)",
                crate::limits::MAX_TEXT_VALUE_LENGTH
            ));
        }
        Ok(TextLiteral(s.to_string()))
    }
}

/// Parsed `<number> <unit-name>` for runtime string input (measure and ratio types).
pub(crate) struct NumberWithUnit(pub Decimal, pub String);

impl std::str::FromStr for NumberWithUnit {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(
                "Measure value cannot be empty. Use a number followed by a unit (e.g. '10 eur')."
                    .to_string(),
            );
        }

        let mut parts = trimmed.split_whitespace();
        let number_part = parts
            .next()
            .expect("split_whitespace yields >=1 token after non-empty guard");
        let unit_part = parts.next().ok_or_else(|| {
            format!(
                "Measure value must include a unit (e.g. '{} eur').",
                number_part
            )
        })?;
        if parts.next().is_some() {
            return Err(format!(
                "Invalid measure value: '{}'. Expected exactly '<number> <unit>', got extra tokens.",
                s
            ));
        }
        let n = number_part
            .parse::<NumberLiteral>()
            .map_err(|_| format!("Invalid measure: '{}'", s))?
            .0;
        Ok(NumberWithUnit(n, unit_part.to_string()))
    }
}

/// Strict ratio runtime literal.
///
/// Grammar (all inputs trimmed first):
/// - `<number>`                      → `Bare(n)`
/// - `<number>%`  (glued, no inner whitespace) → `Percent(n)` raw magnitude
/// - `<number>%%` (glued, no inner whitespace) → `Permille(n)` raw magnitude
/// - `<number> <unit-name>`          → `Named { value: n, unit: <unit-name> }`
///
/// `<number>` is parsed by [`NumberLiteral`] (signed, allows `_`/`,` separators).
/// Whitespace between the number and a keyword unit may be any non-empty run
/// (`"50 percent"`, `"50    percent"`, `"50\tpercent"` are all accepted).
///
/// The sigils `%` / `%%` are language-level constants meaning "divide by 100 / 1000"
/// and unconditionally produce the canonical unit names `"percent"` / `"permille"`.
/// They are NOT accepted as standalone unit-position tokens (i.e. `"5 %"` is rejected).
///
/// Signedness is intentionally not constrained at this layer: bounds are the
/// type-system's job (`-> minimum 0%`), and the evaluator can produce signed
/// ratios from non-negative inputs (e.g. `this_year - last_year` on `percent`).
/// The parser must accept everything the evaluator can emit (round-trip symmetry).
///
/// `Named` carries the raw unit name; the caller in `parse_number_unit::Ratio`
/// resolves it against the type's [`RatioUnits`] table (covering built-in
/// `percent`/`permille` and any user-defined units like `basis_points`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RatioLiteral {
    Bare(Decimal),
    Percent(Decimal),
    Permille(Decimal),
    Named { value: Decimal, unit: String },
}

impl std::str::FromStr for RatioLiteral {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(
                "Ratio value cannot be empty. Use a number, optionally followed by '%', '%%', or a unit name (e.g. '0.5', '50%', '25%%', '50 percent')."
                    .to_string(),
            );
        }

        let mut parts = trimmed.split_whitespace();
        let first = parts
            .next()
            .expect("split_whitespace yields >=1 token after non-empty guard");
        let second = parts.next();
        if parts.next().is_some() {
            return Err(format!(
                "Invalid ratio value: '{}'. Expected '<number>', '<number>%', '<number>%%', or '<number> <unit>'.",
                s
            ));
        }

        match second {
            // 1-token forms: bare number, or sigil-suffixed number.
            None => {
                if let Some(rest) = first.strip_suffix("%%") {
                    if rest.is_empty() {
                        return Err(format!(
                            "Invalid ratio value: '{}'. '%%' must follow a number (e.g. '25%%').",
                            s
                        ));
                    }
                    let n = rest
                        .parse::<NumberLiteral>()
                        .map_err(|_| {
                            format!(
                            "Invalid ratio value: '{}'. '{}' is not a valid number before '%%'.",
                            s, rest
                        )
                        })?
                        .0;
                    return Ok(RatioLiteral::Permille(n));
                }
                if let Some(rest) = first.strip_suffix('%') {
                    if rest.is_empty() {
                        return Err(format!(
                            "Invalid ratio value: '{}'. '%' must follow a number (e.g. '50%').",
                            s
                        ));
                    }
                    let n = rest
                        .parse::<NumberLiteral>()
                        .map_err(|_| {
                            format!(
                                "Invalid ratio value: '{}'. '{}' is not a valid number before '%'.",
                                s, rest
                            )
                        })?
                        .0;
                    return Ok(RatioLiteral::Percent(n));
                }
                let n = first.parse::<NumberLiteral>().map_err(|_| {
                    format!(
                        "Invalid ratio value: '{}'. Must be a number, '<n>%', '<n>%%', '<n> percent', '<n> permille', or '<n> <unit>'.",
                        s
                    )
                })?.0;
                Ok(RatioLiteral::Bare(n))
            }
            // 2-token form: <number> <unit-name>. Sigils are not accepted as unit-position tokens.
            Some(unit) => {
                if unit == "%" || unit == "%%" {
                    return Err(format!(
                        "Invalid ratio value: '{}'. '{}' must be glued to the number (e.g. '{}{}'), not separated by whitespace.",
                        s, unit, first, unit
                    ));
                }
                let n = first
                    .parse::<NumberLiteral>()
                    .map_err(|_| {
                        format!(
                            "Invalid ratio value: '{}'. '{}' is not a valid number.",
                            s, first
                        )
                    })?
                    .0;
                Ok(RatioLiteral::Named {
                    value: n,
                    unit: unit.to_string(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BooleanValue;
    use std::str::FromStr;

    #[test]
    fn boolean_value_rejects_accept_and_reject_strings() {
        for invalid in ["accept", "reject"] {
            assert!(
                BooleanValue::from_str(invalid).is_err(),
                "'{invalid}' must not parse as boolean"
            );
        }
    }
}
