//! LemmaType and related JSON shapes matching today's domain serde.

use crate::computation::rational::RationalInteger;
use crate::literals::{
    BaseMeasureVector, DateTimeValue, MeasureUnit as DomainMeasureUnit, MeasureUnits,
    RatioUnit as DomainRatioUnit, RatioUnits, TimeValue,
};
use crate::planning::semantics::{
    LemmaType as DomainLemmaType, MeasureTrait as DomainMeasureTrait,
    TypeDefiningSpec as DomainTypeDefiningSpec, TypeExtends as DomainTypeExtends,
    TypeSpecification as DomainTypeSpecification,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Exact rational factor as reduced numer/denom integer strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RationalFactor {
    pub numer: String,
    pub denom: String,
}

impl RationalFactor {
    fn from_ratio(value: &RationalInteger) -> Self {
        let reduced = value
            .clone()
            .try_reduce()
            .expect("BUG: stored measure unit factor must reduce");
        Self {
            numer: reduced.numer_to_string(),
            denom: reduced.denom_to_string(),
        }
    }
}

/// Unit-scoped bound `{ value, unit }` (decimal magnitude + unit name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedBound {
    pub value: Decimal,
    pub unit: String,
}

fn planned_bound_decimal(rational: &RationalInteger) -> Decimal {
    rational
        .try_to_decimal()
        .expect("BUG: planned bound must convert to decimal")
}

fn named_bound_from(bound: &(RationalInteger, String)) -> NamedBound {
    NamedBound {
        value: planned_bound_decimal(&bound.0),
        unit: bound.1.clone(),
    }
}

fn optional_named_bound(bound: &Option<(RationalInteger, String)>) -> Option<NamedBound> {
    bound.as_ref().map(named_bound_from)
}

fn optional_decimal_bound(bound: &Option<RationalInteger>) -> Option<Decimal> {
    bound.as_ref().map(planned_bound_decimal)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasureTrait {
    Duration,
    Calendar,
}

impl From<DomainMeasureTrait> for MeasureTrait {
    fn from(value: DomainMeasureTrait) -> Self {
        match value {
            DomainMeasureTrait::Duration => Self::Duration,
            DomainMeasureTrait::Calendar => Self::Calendar,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasureUnit {
    pub name: String,
    pub factor: RationalFactor,
    pub derived_measure_factors: Vec<(String, i32)>,
    pub decomposition: BaseMeasureVector,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub minimum: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub maximum: Option<Decimal>,
    #[serde(
        rename = "suggestion",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub suggestion_magnitude: Option<Decimal>,
}

impl From<&DomainMeasureUnit> for MeasureUnit {
    fn from(unit: &DomainMeasureUnit) -> Self {
        Self {
            name: unit.name.clone(),
            factor: RationalFactor::from_ratio(&unit.factor),
            derived_measure_factors: unit.derived_measure_factors.clone(),
            decomposition: unit.decomposition.clone(),
            minimum: unit.minimum.as_ref().map(|minimum| {
                minimum
                    .try_to_decimal()
                    .expect("BUG: planned measure unit minimum must convert to decimal")
            }),
            maximum: unit.maximum.as_ref().map(|maximum| {
                maximum
                    .try_to_decimal()
                    .expect("BUG: planned measure unit maximum must convert to decimal")
            }),
            suggestion_magnitude: unit.suggestion_magnitude.as_ref().map(|suggestion| {
                suggestion
                    .try_to_decimal()
                    .expect("BUG: planned measure unit suggestion must convert to decimal")
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RatioUnit {
    pub name: String,
    pub value: RationalFactor,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub minimum: Option<Decimal>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub maximum: Option<Decimal>,
    #[serde(
        rename = "suggestion",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub suggestion_magnitude: Option<Decimal>,
}

impl From<&DomainRatioUnit> for RatioUnit {
    fn from(unit: &DomainRatioUnit) -> Self {
        Self {
            name: unit.name.clone(),
            value: RationalFactor::from_ratio(&unit.value),
            minimum: unit.minimum.as_ref().map(|minimum| {
                minimum
                    .try_to_decimal()
                    .expect("BUG: planned ratio unit minimum must convert to decimal")
            }),
            maximum: unit.maximum.as_ref().map(|maximum| {
                maximum
                    .try_to_decimal()
                    .expect("BUG: planned ratio unit maximum must convert to decimal")
            }),
            suggestion_magnitude: unit.suggestion_magnitude.as_ref().map(|suggestion| {
                suggestion
                    .try_to_decimal()
                    .expect("BUG: planned ratio unit suggestion must convert to decimal")
            }),
        }
    }
}

fn measure_units_from(units: &MeasureUnits) -> Vec<MeasureUnit> {
    units.iter().map(MeasureUnit::from).collect()
}

fn ratio_units_from(units: &RatioUnits) -> Vec<RatioUnit> {
    units.iter().map(RatioUnit::from).collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypeDefiningSpec {
    Local,
    Import,
}

impl From<&DomainTypeDefiningSpec> for TypeDefiningSpec {
    fn from(value: &DomainTypeDefiningSpec) -> Self {
        match value {
            DomainTypeDefiningSpec::Local => Self::Local,
            DomainTypeDefiningSpec::Import => Self::Import,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypeExtends {
    Primitive,
    Custom {
        parent: String,
        family: String,
        defining_spec: TypeDefiningSpec,
    },
}

impl From<&DomainTypeExtends> for TypeExtends {
    fn from(value: &DomainTypeExtends) -> Self {
        match value {
            DomainTypeExtends::Primitive => Self::Primitive,
            DomainTypeExtends::Custom {
                parent,
                family,
                defining_spec,
            } => Self::Custom {
                parent: parent.clone(),
                family: family.clone(),
                defining_spec: TypeDefiningSpec::from(defining_spec),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TypeSpecification {
    Boolean {
        help: String,
    },
    Measure {
        #[serde(default)]
        minimum: Option<NamedBound>,
        #[serde(default)]
        maximum: Option<NamedBound>,
        decimals: Option<u8>,
        units: Vec<MeasureUnit>,
        #[serde(default)]
        traits: Vec<MeasureTrait>,
        #[serde(default)]
        decomposition: Option<BaseMeasureVector>,
        help: String,
    },
    Number {
        #[serde(default)]
        minimum: Option<Decimal>,
        #[serde(default)]
        maximum: Option<Decimal>,
        decimals: Option<u8>,
        help: String,
    },
    NumberRange {
        #[serde(default)]
        lower: Option<Decimal>,
        #[serde(default)]
        upper: Option<Decimal>,
        #[serde(default)]
        minimum: Option<Decimal>,
        #[serde(default)]
        maximum: Option<Decimal>,
        help: String,
    },
    Ratio {
        #[serde(default)]
        minimum: Option<Decimal>,
        #[serde(default)]
        maximum: Option<Decimal>,
        decimals: Option<u8>,
        units: Vec<RatioUnit>,
        help: String,
    },
    RatioRange {
        #[serde(default)]
        lower: Option<Decimal>,
        #[serde(default)]
        upper: Option<Decimal>,
        #[serde(default)]
        minimum: Option<Decimal>,
        #[serde(default)]
        maximum: Option<Decimal>,
        units: Vec<RatioUnit>,
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
        #[serde(default)]
        minimum: Option<NamedBound>,
        #[serde(default)]
        maximum: Option<NamedBound>,
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
        #[serde(default)]
        minimum: Option<NamedBound>,
        #[serde(default)]
        maximum: Option<NamedBound>,
        help: String,
    },
    MeasureRange {
        #[serde(default)]
        lower: Option<NamedBound>,
        #[serde(default)]
        upper: Option<NamedBound>,
        #[serde(default)]
        minimum: Option<NamedBound>,
        #[serde(default)]
        maximum: Option<NamedBound>,
        units: Vec<MeasureUnit>,
        #[serde(default)]
        decomposition: Option<BaseMeasureVector>,
        help: String,
    },
    Veto {
        message: Option<String>,
    },
    Undetermined,
}

impl From<&DomainTypeSpecification> for TypeSpecification {
    fn from(spec: &DomainTypeSpecification) -> Self {
        match spec {
            DomainTypeSpecification::Boolean { help } => Self::Boolean { help: help.clone() },
            DomainTypeSpecification::Measure {
                minimum,
                maximum,
                decimals,
                units,
                traits,
                decomposition,
                help,
            } => Self::Measure {
                minimum: optional_named_bound(minimum),
                maximum: optional_named_bound(maximum),
                decimals: *decimals,
                units: measure_units_from(units),
                traits: traits.iter().copied().map(MeasureTrait::from).collect(),
                decomposition: decomposition.clone(),
                help: help.clone(),
            },
            DomainTypeSpecification::Number {
                minimum,
                maximum,
                decimals,
                help,
            } => Self::Number {
                minimum: optional_decimal_bound(minimum),
                maximum: optional_decimal_bound(maximum),
                decimals: *decimals,
                help: help.clone(),
            },
            DomainTypeSpecification::NumberRange {
                lower,
                upper,
                minimum,
                maximum,
                help,
            } => Self::NumberRange {
                lower: optional_decimal_bound(lower),
                upper: optional_decimal_bound(upper),
                minimum: optional_decimal_bound(minimum),
                maximum: optional_decimal_bound(maximum),
                help: help.clone(),
            },
            DomainTypeSpecification::Ratio {
                minimum,
                maximum,
                decimals,
                units,
                help,
            } => Self::Ratio {
                minimum: optional_decimal_bound(minimum),
                maximum: optional_decimal_bound(maximum),
                decimals: *decimals,
                units: ratio_units_from(units),
                help: help.clone(),
            },
            DomainTypeSpecification::RatioRange {
                lower,
                upper,
                minimum,
                maximum,
                units,
                help,
            } => Self::RatioRange {
                lower: optional_decimal_bound(lower),
                upper: optional_decimal_bound(upper),
                minimum: optional_decimal_bound(minimum),
                maximum: optional_decimal_bound(maximum),
                units: ratio_units_from(units),
                help: help.clone(),
            },
            DomainTypeSpecification::Text {
                length,
                options,
                help,
            } => Self::Text {
                length: *length,
                options: options.clone(),
                help: help.clone(),
            },
            DomainTypeSpecification::Date {
                minimum,
                maximum,
                help,
            } => Self::Date {
                minimum: minimum.clone(),
                maximum: maximum.clone(),
                help: help.clone(),
            },
            DomainTypeSpecification::DateRange {
                lower,
                upper,
                minimum,
                maximum,
                help,
            } => Self::DateRange {
                lower: lower.clone(),
                upper: upper.clone(),
                minimum: optional_named_bound(minimum),
                maximum: optional_named_bound(maximum),
                help: help.clone(),
            },
            DomainTypeSpecification::Time {
                minimum,
                maximum,
                help,
            } => Self::Time {
                minimum: minimum.clone(),
                maximum: maximum.clone(),
                help: help.clone(),
            },
            DomainTypeSpecification::TimeRange {
                lower,
                upper,
                minimum,
                maximum,
                help,
            } => Self::TimeRange {
                lower: lower.clone(),
                upper: upper.clone(),
                minimum: optional_named_bound(minimum),
                maximum: optional_named_bound(maximum),
                help: help.clone(),
            },
            DomainTypeSpecification::MeasureRange {
                lower,
                upper,
                minimum,
                maximum,
                units,
                decomposition,
                help,
            } => Self::MeasureRange {
                lower: optional_named_bound(lower),
                upper: optional_named_bound(upper),
                minimum: optional_named_bound(minimum),
                maximum: optional_named_bound(maximum),
                units: measure_units_from(units),
                decomposition: decomposition.clone(),
                help: help.clone(),
            },
            DomainTypeSpecification::Veto { message } => Self::Veto {
                message: message.clone(),
            },
            DomainTypeSpecification::Undetermined => Self::Undetermined,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LemmaType {
    pub name: Option<String>,
    #[serde(flatten)]
    pub specifications: TypeSpecification,
    pub extends: TypeExtends,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measure_binding_unit: Option<String>,
}

impl From<&DomainLemmaType> for LemmaType {
    fn from(lemma_type: &DomainLemmaType) -> Self {
        Self {
            name: lemma_type.name.clone(),
            specifications: TypeSpecification::from(&lemma_type.specifications),
            extends: TypeExtends::from(&lemma_type.extends),
            measure_binding_unit: lemma_type.measure_binding_unit.clone(),
        }
    }
}
