//! Result envelope for computation operations: a produced value or a domain veto.

use std::fmt;

use crate::planning::semantics::{
    BoundValueKind, DataPath, LemmaType, LiteralValue, SemanticDateTime, SemanticTime,
    TypeSpecification,
};
use serde::Serialize;

/// Why an operation yielded no value (domain veto).
///
/// JSON serialization is a single string (see [`fmt::Display`]). There is intentionally no
/// `Deserialize` implementation: veto payloads are engine output only.
#[derive(Debug, Clone, PartialEq)]
pub enum VetoType {
    /// Evaluation needed a data that was not provided
    MissingData {
        data: DataPath,
        suggestion: Option<String>,
    },
    /// Explicit `veto "reason"` in Lemma source
    UserDefined { message: Option<String> },
    /// Runtime domain failure (division by zero, date overflow, bad Data override, etc.)
    Computation { message: String },
}

impl fmt::Display for VetoType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VetoType::MissingData {
                data,
                suggestion: Some(suggestion),
            } => write!(f, "Missing data: {data} (did you mean '{suggestion}'?)"),
            VetoType::MissingData {
                data,
                suggestion: None,
            } => write!(f, "Missing data: {data}"),
            VetoType::UserDefined { message: Some(msg) } => write!(f, "{msg}"),
            VetoType::UserDefined { message: None } => write!(f, "Vetoed"),
            VetoType::Computation { message } => write!(f, "{message}"),
        }
    }
}

impl VetoType {
    #[must_use]
    pub fn computation(message: impl Into<String>) -> Self {
        VetoType::Computation {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn missing_data(data: DataPath, suggestion: Option<String>) -> Self {
        VetoType::MissingData { data, suggestion }
    }
}

impl Serialize for VetoType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Result of evaluating a rule, expression, or data leaf.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationResult {
    /// Operation produced a value (canonical magnitude + optional written unit)
    Value(BoundValueKind),
    /// Operation was vetoed (valid result, no value)
    Veto(VetoType),
}

impl OperationResult {
    /// Wrap a typeless literal with no written-unit binding.
    pub fn from_literal(value: LiteralValue) -> Self {
        Self::Value(BoundValueKind::unbound(value.value))
    }

    pub fn from_bound(bound: BoundValueKind) -> Self {
        Self::Value(bound)
    }

    pub fn vetoed(&self) -> bool {
        matches!(self, OperationResult::Veto(_))
    }

    /// True when this result is a [`VetoType::MissingData`] veto.
    #[must_use]
    pub fn is_missing_data(&self) -> bool {
        matches!(self, OperationResult::Veto(VetoType::MissingData { .. }))
    }

    #[must_use]
    pub fn value(&self) -> Option<&BoundValueKind> {
        match self {
            OperationResult::Value(value) => Some(value),
            OperationResult::Veto(_) => None,
        }
    }

    /// Typeless payload (written unit dropped). Prefer [`Self::value`] when the binding matters.
    #[must_use]
    pub fn literal_value(&self) -> Option<LiteralValue> {
        self.value().map(BoundValueKind::to_literal)
    }

    pub fn number(number: rust_decimal::Decimal) -> Self {
        Self::from_literal(LiteralValue::number_from_decimal(number))
    }

    pub fn measure(
        value: rust_decimal::Decimal,
        unit: impl Into<String>,
        lemma_type: Option<LemmaType>,
    ) -> Self {
        use crate::computation::rational::checked_mul;
        let lemma_type = std::sync::Arc::new(
            lemma_type.unwrap_or_else(|| LemmaType::primitive(TypeSpecification::measure())),
        );
        let unit_name = unit.into();
        let rational = crate::literals::rational_from_parsed_decimal(value)
            .expect("BUG: operation result measure must lift at boundary");
        let factor = if let TypeSpecification::Measure { units, .. } = &lemma_type.specifications {
            units
                .get(&unit_name)
                .map(|u| u.factor.clone())
                .unwrap_or_else(|_| {
                    panic!(
                        "BUG: OperationResult::measure unit '{}' not declared on type",
                        unit_name
                    )
                })
        } else {
            crate::computation::rational::rational_one()
        };
        let canonical = checked_mul(&rational, &factor)
            .expect("BUG: measure canonicalization overflow in OperationResult::measure");
        Self::from_bound(BoundValueKind {
            value: crate::planning::semantics::ValueKind::Measure(canonical),
            measure_binding_unit: Some(std::sync::Arc::from(unit_name)),
        })
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::from_literal(LiteralValue::text(text.into()))
    }

    pub fn date(date: impl Into<SemanticDateTime>) -> Self {
        Self::from_literal(LiteralValue::date(date.into()))
    }

    pub fn time(time: impl Into<SemanticTime>) -> Self {
        Self::from_literal(LiteralValue::time(time.into()))
    }

    pub fn boolean(boolean: bool) -> Self {
        Self::from_literal(LiteralValue::from_bool(boolean))
    }

    pub fn ratio(rational: rust_decimal::Decimal) -> Self {
        Self::from_literal(LiteralValue::ratio_from_decimal(rational))
    }

    pub fn veto(veto: impl Into<String>) -> Self {
        Self::Veto(VetoType::UserDefined {
            message: Some(veto.into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::VetoType;
    use crate::planning::semantics::DataPath;

    #[test]
    fn veto_type_serializes_as_display_string() {
        let v = VetoType::missing_data(DataPath::new(vec![], "product".to_string()), None);
        let json = serde_json::to_string(&v).expect("serialize");
        assert_eq!(json, "\"Missing data: product\"");
    }
}
