use crate::computation::{OperationResult, VetoType};
use crate::planning::execution_plan::{validate_value_against_type, ExecutionPlan};
use crate::planning::semantics::{
    number_with_unit_to_value_kind, parse_value_from_string, parser_value_to_value_kind,
    DataDefinition, DataPath, LemmaType, LiteralValue, Source, TypeSpecification, TypedLiteral,
    ValueKind,
};
use crate::planning::unit_index::UnitIndex;
use crate::Error;
use crate::ResourceLimits;
use indexmap::IndexMap;
use rust_decimal::Decimal;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

/// Typed data value from a client (CLI/WASM). JSON parsing stays outside [`parse_data_value`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunDataValue {
    /// Raw string, parsed against the target type's specification by [`parse_value_from_string`].
    String(String),
    Boolean(bool),
    MeasureMap(BTreeMap<String, Decimal>),
    RatioMap(BTreeMap<String, Decimal>),
}

impl RunDataValue {
    pub fn string(value: impl Into<String>) -> Self {
        Self::String(value.into())
    }

    pub(crate) fn is_empty(&self) -> bool {
        match self {
            Self::String(s) => s.trim().is_empty(),
            Self::MeasureMap(map) | Self::RatioMap(map) => map.is_empty(),
            Self::Boolean(_) => false,
        }
    }
}

impl From<String> for RunDataValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for RunDataValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

/// Parse one JSON value into a [`RunDataValue`] (SDK / MCP / WASM wire shape).
///
/// JSON `null` means omit: returns `Ok(None)`.
pub fn run_data_value_from_json_value(
    value: serde_json::Value,
) -> Result<Option<RunDataValue>, String> {
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) => Ok(Some(RunDataValue::String(s))),
        serde_json::Value::Bool(b) => Ok(Some(RunDataValue::Boolean(b))),
        serde_json::Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                Ok(Some(RunDataValue::String(n.to_string())))
            } else {
                Err("decimal values must be passed as strings to preserve exactness".to_string())
            }
        }
        serde_json::Value::Object(obj) => {
            if obj.is_empty() {
                return Err("data value object must not be empty".to_string());
            }
            if obj.len() == 2 && obj.contains_key("value") && obj.contains_key("unit") {
                return Err(
                    "the {value, unit} object shape is not supported; use a unit map like {\"eur\": \"84\"}"
                        .to_string(),
                );
            }
            if obj.values().all(|v| v.is_string()) {
                let mut map = BTreeMap::new();
                for (k, v) in obj {
                    let text = v.as_str().expect("BUG: object values checked as strings");
                    let decimal = Decimal::from_str(text.trim())
                        .map_err(|error| format!("invalid decimal '{text}': {error}"))?;
                    map.insert(k, decimal);
                }
                return Ok(Some(RunDataValue::MeasureMap(map)));
            }
            Err("data value object must be a unit map with string magnitudes".to_string())
        }
        serde_json::Value::Array(_) => Err("data value must not be an array".to_string()),
    }
}

/// Convert SDK/MCP/WASM `data` object to string map for [`crate::Engine::run`].
///
/// Single-entry unit maps become convenience strings (`"84 eur"`). Multi-key maps
/// are rejected until `Engine::run` accepts typed [`RunDataValue`] directly.
/// JSON `null` field values are omitted (same as unbound).
pub fn parse_run_data_object(
    data: &Option<serde_json::Value>,
) -> Result<HashMap<String, String>, String> {
    let Some(value) = data else {
        return Ok(HashMap::new());
    };
    if value.is_null() {
        return Ok(HashMap::new());
    }
    let map: HashMap<String, serde_json::Value> = serde_json::from_value(value.clone())
        .map_err(|e| format!("data must be a plain object: {e}"))?;
    map.into_iter()
        .filter_map(|(k, v)| match run_data_value_from_json_value(v) {
            Ok(None) => None,
            Ok(Some(input)) => Some(match input {
                RunDataValue::String(s) => Ok((k, s)),
                RunDataValue::Boolean(b) => Ok((k, b.to_string())),
                RunDataValue::MeasureMap(m) | RunDataValue::RatioMap(m) => {
                    if m.len() == 1 {
                        let (unit, mag) = m.into_iter().next().expect("BUG: single entry map");
                        Ok((k, format!("{mag} {unit}")))
                    } else {
                        Err(format!(
                            "data value '{k}' must be a convenience string for run"
                        ))
                    }
                }
            }),
            Err(e) => Some(Err(e)),
        })
        .collect()
}

/// Resolve SDK/MCP/WASM `rules` (string or string array) for [`crate::Engine::run`].
pub fn resolve_run_rules(rules: &Option<serde_json::Value>) -> Result<Option<Vec<String>>, String> {
    let Some(value) = rules else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    if let Some(s) = value.as_str() {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err("rules must not be empty".to_string());
        }
        return Ok(Some(vec![trimmed.to_string()]));
    }
    if let Some(arr) = value.as_array() {
        if arr.is_empty() {
            return Err("rules must not be empty".to_string());
        }
        let names: Vec<String> = arr
            .iter()
            .map(|v| {
                v.as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| "rules must be an array of strings".to_string())
            })
            .collect::<Result<_, _>>()?;
        return Ok(Some(names));
    }
    Err("rules must be a string or array of strings".to_string())
}

pub fn parse_data_value(
    input: &RunDataValue,
    lemma_type: &Arc<LemmaType>,
    source: &Source,
    unit_index: &UnitIndex,
    named_types: &IndexMap<String, Arc<LemmaType>>,
) -> Result<TypedLiteral, Error> {
    let to_err = |msg: String| Error::validation(msg, Some(source.clone()), None::<String>);
    let type_spec = &lemma_type.specifications;

    let (kind, binding_unit) = match (input, type_spec) {
        (RunDataValue::String(s), _) => {
            let parsed = parse_value_from_string(s, type_spec, source)?;
            let kind = parser_value_to_value_kind(&parsed, type_spec).map_err(to_err)?;
            let binding = match binding_unit_from_parser_value(&parsed) {
                Some(written) => Some(
                    resolve_passed_unit(&written, lemma_type.as_ref(), unit_index, named_types)
                        .map_err(to_err)?,
                ),
                None => None,
            };
            (kind, binding)
        }
        (RunDataValue::Boolean(b), TypeSpecification::Boolean { .. }) => {
            (ValueKind::Boolean(*b), None)
        }
        (RunDataValue::Boolean(_), _) => {
            return Err(to_err(format!(
                "boolean input is only valid for boolean data, not {}",
                type_spec
            )));
        }
        (RunDataValue::MeasureMap(map), TypeSpecification::Measure { .. }) => {
            let (kind, binding) =
                measure_from_unit_map(map, lemma_type.as_ref(), unit_index, named_types)
                    .map_err(to_err)?;
            (kind, binding)
        }
        (
            RunDataValue::MeasureMap(map) | RunDataValue::RatioMap(map),
            TypeSpecification::Ratio { .. },
        ) => {
            let (kind, binding) =
                ratio_from_unit_map(map, lemma_type.as_ref(), unit_index, named_types)
                    .map_err(to_err)?;
            (kind, binding)
        }
        (RunDataValue::MeasureMap(_), _) => {
            return Err(to_err(format!(
                "measure unit map is only valid for measure data, not {}",
                type_spec
            )));
        }
        (RunDataValue::RatioMap(_), _) => {
            return Err(to_err(format!(
                "ratio unit map is only valid for ratio data, not {}",
                type_spec
            )));
        }
    };

    let typed_type = match binding_unit {
        Some(unit) => Arc::new(lemma_type.as_ref().clone().with_measure_binding_unit(unit)),
        None => Arc::clone(lemma_type),
    };
    Ok(TypedLiteral {
        value: kind,
        lemma_type: typed_type,
    })
}

fn binding_unit_from_parser_value(value: &crate::parsing::ast::Value) -> Option<String> {
    use crate::parsing::ast::Value;
    match value {
        Value::NumberWithUnit(_, unit) => Some(unit.clone()),
        Value::Range(left, right) => match (left.as_ref(), right.as_ref()) {
            (Value::NumberWithUnit(_, left_unit), Value::NumberWithUnit(_, right_unit))
                if left_unit == right_unit =>
            {
                Some(left_unit.clone())
            }
            _ => None,
        },
        _ => None,
    }
}

/// Resolve a unit the caller actually wrote to a bare declared name.
///
/// Qualified paths go through [`UnitIndex::resolve_with_named_types`]. Bare names
/// accepted only when this data type (or range element) declares that exact name.
/// Never invents a unit the caller did not pass.
fn resolve_passed_unit(
    written: &str,
    lemma_type: &LemmaType,
    unit_index: &UnitIndex,
    named_types: &IndexMap<String, Arc<LemmaType>>,
) -> Result<String, String> {
    match unit_index.resolve_with_named_types(written, named_types) {
        Ok((bare, _)) => {
            if unit_name_declared_on_type(lemma_type, &bare)
                || range_element_declares_unit(lemma_type, &bare)
            {
                Ok(bare)
            } else {
                Err(format!(
                    "Unit '{written}' resolves to '{bare}', which is not declared on type {}",
                    lemma_type.name()
                ))
            }
        }
        Err(index_err) => {
            if unit_name_declared_on_type(lemma_type, written)
                || range_element_declares_unit(lemma_type, written)
            {
                Ok(written.to_string())
            } else {
                Err(index_err)
            }
        }
    }
}

fn range_element_declares_unit(lemma_type: &LemmaType, unit_name: &str) -> bool {
    let Some(element) = lemma_type.specifications.element_from_range() else {
        return false;
    };
    unit_name_declared_on_type(&LemmaType::primitive(element), unit_name)
}

fn unit_name_declared_on_type(lemma_type: &LemmaType, unit_name: &str) -> bool {
    match &lemma_type.specifications {
        TypeSpecification::Measure { units, .. } => units.get(unit_name).is_ok(),
        TypeSpecification::Ratio { units, .. } => units.get(unit_name).is_ok(),
        _ => false,
    }
}

fn measure_from_unit_map(
    map: &BTreeMap<String, Decimal>,
    lemma_type: &LemmaType,
    unit_index: &UnitIndex,
    named_types: &IndexMap<String, Arc<LemmaType>>,
) -> Result<(ValueKind, Option<String>), String> {
    if map.is_empty() {
        return Err("measure input map must contain at least one unit key".to_string());
    }
    if lemma_type
        .measure_unit_names()
        .is_none_or(|names| names.is_empty())
    {
        unreachable!("BUG: measure type has no units at data input");
    }

    let mut kinds: Vec<ValueKind> = Vec::with_capacity(map.len());
    let mut resolved_keys: Vec<String> = Vec::with_capacity(map.len());
    for (unit_name, magnitude) in map {
        let bare = resolve_passed_unit(unit_name, lemma_type, unit_index, named_types)?;
        kinds.push(number_with_unit_to_value_kind(
            *magnitude, &bare, lemma_type,
        )?);
        resolved_keys.push(bare);
    }

    let first = kinds.first().expect("BUG: map non-empty");
    let ValueKind::Measure(first_magnitude) = first else {
        return Err("expected measure value".to_string());
    };
    for kind in kinds.iter().skip(1) {
        let ValueKind::Measure(magnitude) = kind else {
            return Err("expected measure value".to_string());
        };
        if magnitude != first_magnitude {
            return Err(
                "measure unit map values disagree when converted to a common basis".to_string(),
            );
        }
    }
    let binding = (resolved_keys.len() == 1).then(|| {
        resolved_keys
            .into_iter()
            .next()
            .expect("BUG: single resolved key")
    });
    Ok((first.clone(), binding))
}

fn ratio_from_unit_map(
    map: &BTreeMap<String, Decimal>,
    lemma_type: &LemmaType,
    unit_index: &UnitIndex,
    named_types: &IndexMap<String, Arc<LemmaType>>,
) -> Result<(ValueKind, Option<String>), String> {
    if map.is_empty() {
        return Err("ratio input map must contain at least one unit key".to_string());
    }
    match &lemma_type.specifications {
        TypeSpecification::Ratio { units, .. } if !units.is_empty() => {}
        _ => unreachable!("BUG: ratio type has no units at data input"),
    }

    let mut kinds: Vec<ValueKind> = Vec::with_capacity(map.len());
    let mut resolved_keys: Vec<String> = Vec::with_capacity(map.len());
    for (unit_name, magnitude) in map {
        let bare = resolve_passed_unit(unit_name, lemma_type, unit_index, named_types)?;
        kinds.push(number_with_unit_to_value_kind(
            *magnitude, &bare, lemma_type,
        )?);
        resolved_keys.push(bare);
    }

    let first = kinds.first().expect("BUG: map non-empty");
    let ValueKind::Ratio(first_canonical) = first else {
        return Err("expected ratio value".to_string());
    };
    for kind in kinds.iter().skip(1) {
        let ValueKind::Ratio(canonical) = kind else {
            return Err("expected ratio value".to_string());
        };
        if canonical != first_canonical {
            return Err(
                "ratio unit map values disagree when converted to a common basis".to_string(),
            );
        }
    }
    let binding = (resolved_keys.len() == 1).then(|| {
        resolved_keys
            .into_iter()
            .next()
            .expect("BUG: single resolved key")
    });
    Ok((ValueKind::Ratio(first_canonical.clone()), binding))
}

/// User-provided data values resolved against a plan's type declarations.
///
/// Lightweight and cheap to construct — no plan cloning required. The
/// [`ExecutionPlan`] stays immutable; callers pass `(&ExecutionPlan, &RunData)`
/// to evaluation and show paths.
#[derive(Debug, Clone, Default)]
pub struct RunData {
    /// Caller bindings: successful literals or Veto (bad override) per Data.
    pub bindings: HashMap<DataPath, OperationResult>,
    /// Input keys that did not match any plan Data (including Import aliases).
    pub ignored_unknown: Vec<String>,
}

impl RunData {
    /// Parse and validate caller-supplied values against the plan's data declarations.
    ///
    /// Unknown keys and Import aliases are ignored (recorded in [`Self::ignored_unknown`]).
    /// Parse, constraint, options, decimals, and input-oversize failures bind that Data
    /// as [`OperationResult::Veto`]; evaluation still runs. Duplicate canonical keys Error.
    pub fn resolve(
        plan: &ExecutionPlan,
        raw_values: HashMap<String, RunDataValue>,
        limits: &ResourceLimits,
    ) -> Result<Self, Error> {
        let mut run_data = Self::default();
        let mut seen_canonical = HashSet::with_capacity(raw_values.len());

        for (name, raw_value) in raw_values {
            let canonical = crate::parsing::ast::ascii_lowercase_logical_name(name.clone());
            if !seen_canonical.insert(canonical.clone()) {
                return Err(Error::request(
                    format!("Duplicate data key '{canonical}'"),
                    Some("Data keys are case-insensitive; remove the duplicate"),
                ));
            }

            let Some(data_path) = plan.input_key_index.get(canonical.as_str()) else {
                run_data.ignored_unknown.push(name);
                continue;
            };

            let data_definition = plan
                .data
                .get(data_path)
                .expect("BUG: data_path was just resolved from plan.input_key_index, must exist");

            let data_source = data_definition.source();
            let type_arc = match data_definition {
                DataDefinition::TypeDeclaration { resolved_type, .. }
                | DataDefinition::Reference { resolved_type, .. }
                | DataDefinition::Value { resolved_type, .. } => Arc::clone(resolved_type),
                DataDefinition::Import { .. } => {
                    run_data.ignored_unknown.push(name);
                    continue;
                }
            };

            let input_key = canonical;

            if raw_value.is_empty() && type_arc.empty_runtime_input_vetoes() {
                run_data.bindings.insert(
                    data_path.clone(),
                    OperationResult::Veto(VetoType::computation(
                        type_arc.data_veto_message(&input_key, "cannot be empty."),
                    )),
                );
                continue;
            }

            let typed = match parse_data_value(
                &raw_value,
                &type_arc,
                data_source,
                plan.expression_unit_index(),
                &plan.resolved_types.resolved,
            ) {
                Ok(value) => value,
                Err(error) => {
                    run_data.bindings.insert(
                        data_path.clone(),
                        OperationResult::Veto(VetoType::computation(
                            type_arc.data_veto_message(&input_key, error.message()),
                        )),
                    );
                    continue;
                }
            };

            let literal_value = LiteralValue {
                value: typed.value.clone(),
            };
            let size = literal_value.byte_size();
            if size > limits.max_data_value_bytes {
                run_data.bindings.insert(
                    data_path.clone(),
                    OperationResult::Veto(VetoType::computation(
                        type_arc.data_veto_message(&input_key, "exceeds the size limit."),
                    )),
                );
                continue;
            }

            if let Err(message) = validate_value_against_type(
                typed.lemma_type.as_ref(),
                &literal_value,
                plan.expression_unit_index(),
            ) {
                run_data.bindings.insert(
                    data_path.clone(),
                    OperationResult::Veto(VetoType::computation(
                        type_arc.data_veto_message(&input_key, &message),
                    )),
                );
                continue;
            }

            run_data.bindings.insert(
                data_path.clone(),
                OperationResult::from_bound(crate::planning::semantics::BoundValueKind {
                    value: typed.value.clone(),
                    measure_binding_unit: typed
                        .lemma_type
                        .measure_binding_unit
                        .as_deref()
                        .map(std::sync::Arc::from),
                }),
            );
        }

        Ok(run_data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computation::rational::{decimal_to_rational, rational_new, rational_one};
    use crate::planning::semantics::{
        primitive_number_arc, MeasureUnit, MeasureUnits, RatioUnit, RatioUnits, TypeExtends,
    };

    fn dummy_source() -> Source {
        Source::new(
            crate::parsing::source::SourceType::Volatile,
            crate::parsing::ast::Span {
                start: 0,
                end: 0,
                line: 1,
                col: 1,
            },
        )
    }

    fn mass_measure_type() -> Arc<LemmaType> {
        Arc::new(LemmaType::new(
            "Mass".to_string(),
            TypeSpecification::Measure {
                minimum: None,
                maximum: None,
                decimals: None,
                units: MeasureUnits::from(vec![
                    MeasureUnit {
                        name: "kilogram".to_string(),
                        factor: rational_one(),
                        derived_measure_factors: Vec::new(),
                        decomposition: crate::literals::BaseMeasureVector::new(),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    },
                    MeasureUnit {
                        name: "gram".to_string(),
                        factor: decimal_to_rational(Decimal::new(1, 3)).expect("factor"),
                        derived_measure_factors: Vec::new(),
                        decomposition: crate::literals::BaseMeasureVector::new(),
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
        ))
    }

    fn ratio_with_percent_type() -> Arc<LemmaType> {
        Arc::new(LemmaType::new(
            "Rate".to_string(),
            TypeSpecification::Ratio {
                minimum: None,
                maximum: None,
                decimals: None,
                units: RatioUnits::from(vec![
                    RatioUnit {
                        name: "percent".to_string(),
                        value: decimal_to_rational(Decimal::new(100, 0)).expect("factor"),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    },
                    RatioUnit {
                        name: "fraction".to_string(),
                        value: rational_one(),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    },
                ]),
                help: String::new(),
            },
            TypeExtends::Primitive,
        ))
    }

    #[test]
    fn string_input_parsed_against_type() {
        let ty = primitive_number_arc();
        let lit = parse_data_value(
            &RunDataValue::String("42".to_string()),
            ty,
            &dummy_source(),
            &UnitIndex::new(),
            &IndexMap::new(),
        )
        .unwrap();
        assert!(matches!(lit.value, ValueKind::Number(_)));
    }

    #[test]
    fn measure_map_agreeing_units_canonicalize() {
        let ty = mass_measure_type();
        let mut map = BTreeMap::new();
        map.insert("kilogram".to_string(), Decimal::from(2));
        map.insert("gram".to_string(), Decimal::from(2000));
        let lit = parse_data_value(
            &RunDataValue::MeasureMap(map),
            &ty,
            &dummy_source(),
            &UnitIndex::new(),
            &IndexMap::new(),
        )
        .unwrap();
        let ValueKind::Measure(magnitude) = &lit.value else {
            panic!("expected measure");
        };
        assert_eq!(magnitude, &rational_new(2, 1));
        let signature = ty.measure_runtime_signature();
        assert_eq!(signature.len(), 1);
        assert_eq!(signature[0].1, 1);
    }

    #[test]
    fn measure_map_disagreeing_units_rejected() {
        let ty = mass_measure_type();
        let mut map = BTreeMap::new();
        map.insert("kilogram".to_string(), Decimal::from(2));
        map.insert("gram".to_string(), Decimal::from(3000));
        let err = parse_data_value(
            &RunDataValue::MeasureMap(map),
            &ty,
            &dummy_source(),
            &UnitIndex::new(),
            &IndexMap::new(),
        )
        .unwrap_err();
        assert!(err.message().contains("disagree"));
    }

    #[test]
    fn ratio_map_percent_and_fraction_agree() {
        let ty = ratio_with_percent_type();
        let mut map = BTreeMap::new();
        map.insert("percent".to_string(), Decimal::from(10));
        map.insert("fraction".to_string(), Decimal::new(1, 1));
        let lit = parse_data_value(
            &RunDataValue::RatioMap(map),
            &ty,
            &dummy_source(),
            &UnitIndex::new(),
            &IndexMap::new(),
        )
        .unwrap();
        let ValueKind::Ratio(canonical) = &lit.value else {
            panic!("expected ratio");
        };
        assert_eq!(
            *canonical,
            decimal_to_rational(Decimal::new(1, 1)).expect("canonical")
        );
        assert!(ty.ratio_primary_unit().is_some());
    }

    #[test]
    fn run_data_value_from_json_null_is_omitted() {
        assert_eq!(
            run_data_value_from_json_value(serde_json::Value::Null).expect("null is omit"),
            None
        );
    }

    #[test]
    fn parse_run_data_object_null_field_is_omitted() {
        let only_null = Some(serde_json::json!({ "x": null }));
        let empty = parse_run_data_object(&only_null).expect("null field omits");
        assert!(empty.is_empty(), "got: {empty:?}");

        let mixed = Some(serde_json::json!({ "x": null, "y": "1" }));
        let map = parse_run_data_object(&mixed).expect("null field omits");
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("y").map(String::as_str), Some("1"));
        assert!(!map.contains_key("x"));
    }
}
