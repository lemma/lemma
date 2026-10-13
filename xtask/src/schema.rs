//! Emits `engine/schemas/api.v1.json`: the JSON Schema for Show, Response,
//! `specs`, and error documents (engine `Engine::show`/`evaluate`/`specs`, thrown `EngineError`
//! arrays). Hand-authored, not derived by macro, because the wire shapes involve
//! hand-written `Serialize` impls (decimal strings, externally/internally tagged enums)
//! that a derive-based schema generator cannot see.
//!
//! `cargo run -p xtask -- schema` regenerates the file. `schema --check` compares the
//! generated schema against the checked-in copy without writing it (run by
//! `engine/tests/api_schema.rs`, whose sibling tests read the file concurrently).

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const SCHEMA_REL_PATH: &str = "engine/schemas/api.v1.json";

fn decimal_string() -> Value {
    json!({"$ref": "#/$defs/DecimalString"})
}

fn nullable_decimal_string() -> Value {
    json!({"anyOf": [{"type": "null"}, {"$ref": "#/$defs/DecimalString"}]})
}

fn nullable_named_bound() -> Value {
    json!({"anyOf": [{"type": "null"}, {"$ref": "#/$defs/NamedBound"}]})
}

fn nullable_string() -> Value {
    json!({"type": ["string", "null"]})
}

fn nullable_integer() -> Value {
    json!({"type": ["integer", "null"]})
}

/// One type variant, twice: as a `LemmaType` (the flattened `TypeSpecification` fields
/// plus the always-present `name`/`kind`/`extends` that `LemmaType` adds around them),
/// and as the bare `TypeSpecification` a list type carries as its `element`.
///
/// `optional_fields` are allowed properties that are omitted when absent (not in
/// `required`): the value-specific written `unit` on measure/ratio, `repository` on spec.
fn type_variants(
    kind: &str,
    spec_fields: Vec<(&str, Value)>,
    optional_fields: Vec<(&str, Value)>,
) -> (Value, Value) {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<Value> = vec!["kind".into()];
    properties.insert("kind".to_string(), json!({"const": kind}));
    for (name, schema) in spec_fields {
        required.push(name.into());
        properties.insert(name.to_string(), schema);
    }
    for (name, schema) in optional_fields {
        properties.insert(name.to_string(), schema);
    }
    let specification = json!({
        "type": "object",
        "required": required.clone(),
        "additionalProperties": false,
        "properties": properties.clone(),
    });
    required.insert(0, "name".into());
    properties.insert("name".to_string(), nullable_string());
    required.push("extends".into());
    properties.insert(
        "extends".to_string(),
        json!({"$ref": "#/$defs/TypeExtends"}),
    );
    let lemma_type = json!({
        "type": "object",
        "required": required,
        "additionalProperties": false,
        "properties": properties,
    });
    (lemma_type, specification)
}

/// The body of one type variant as a Show property states it: `help` (`description`) and
/// text `options`/`length` (`enum`, `minLength`/`maxLength`) removed. No
/// `additionalProperties`: the element and property branches close the body by composition.
/// A list element refers to the stripped element types.
fn show_type_body(variant: &Value) -> Value {
    let mut variant = variant.clone();
    let object = variant.as_object_mut().expect("type variant is an object");
    let is_text = object["properties"]["kind"]["const"] == "text";
    let removed: &[&str] = if is_text {
        &["help", "options", "length"]
    } else {
        &["help"]
    };
    object.remove("additionalProperties");
    let properties = object["properties"]
        .as_object_mut()
        .expect("type variant has properties");
    for field in removed {
        properties.remove(*field);
    }
    if let Some(element) = properties.get_mut("element") {
        *element = json!({"$ref": "#/$defs/ShowPropertyElementType"});
    }
    object["required"]
        .as_array_mut()
        .expect("type variant has required")
        .retain(|field| !removed.contains(&field.as_str().expect("required entry is a string")));
    variant
}

/// A list element: one shared body, closed to its own properties.
fn show_element_branch(body: &str) -> Value {
    json!({"allOf": [{"$ref": format!("#/$defs/{body}")}], "unevaluatedProperties": false})
}

/// A Show property type: one shared body plus `name` and `extends`, closed to those.
fn show_property_branch(body: &str) -> Value {
    json!({
        "allOf": [
            {"$ref": format!("#/$defs/{body}")},
            {"$ref": "#/$defs/ShowTypeNameExtends"}
        ],
        "unevaluatedProperties": false
    })
}

fn with_defs(mut defs: Value, shared: serde_json::Map<String, Value>) -> Value {
    defs.as_object_mut()
        .expect("$defs is an object")
        .extend(shared);
    defs
}

/// Build the full `api.v1.json` document.
pub fn api_v1_schema() -> Value {
    let measure_unit = json!({
        "type": "object",
        "required": ["name", "factor", "derived_measure_factors", "decomposition"],
        "additionalProperties": false,
        "description": "One declared unit of a Measure type. `factor`: 1 of this unit equals `factor` canonical units.",
        "properties": {
            "name": {"type": "string"},
            "factor": {"$ref": "#/$defs/RationalFactor"},
            "derived_measure_factors": {
                "type": "array",
                "description": "(measure_ref, exponent) pairs from a compound unit declaration (e.g. meter/second). Empty for base units.",
                "items": {
                    "type": "array",
                    "prefixItems": [{"type": "string"}, {"type": "integer"}],
                    "items": false,
                    "minItems": 2,
                    "maxItems": 2
                }
            },
            "decomposition": {"type": "object", "additionalProperties": {"type": "integer"}},
            "minimum": decimal_string(),
            "maximum": decimal_string(),
            "suggestion": decimal_string()
        }
    });

    let ratio_unit = json!({
        "type": "object",
        "required": ["name", "value"],
        "additionalProperties": false,
        "properties": {
            "name": {"type": "string"},
            "value": {"$ref": "#/$defs/RationalFactor"},
            "minimum": decimal_string(),
            "maximum": decimal_string(),
            "suggestion": decimal_string()
        }
    });

    let (lemma_type_boolean, spec_boolean) =
        type_variants("boolean", vec![("help", json!({"type": "string"}))], vec![]);

    let (lemma_type_measure, spec_measure) = type_variants(
        "measure",
        vec![
            ("minimum", nullable_named_bound()),
            ("maximum", nullable_named_bound()),
            ("decimals", nullable_integer()),
            (
                "units",
                json!({"type": "array", "items": {"$ref": "#/$defs/MeasureUnit"}}),
            ),
            (
                "traits",
                json!({"type": "array", "items": {"enum": ["duration", "calendar"]}}),
            ),
            (
                "decomposition",
                json!({"anyOf": [{"type": "null"}, {"type": "object", "additionalProperties": {"type": "integer"}}]}),
            ),
            ("help", json!({"type": "string"})),
        ],
        vec![("unit", json!({"type": "string"}))],
    );

    let (lemma_type_number, spec_number) = type_variants(
        "number",
        vec![
            ("minimum", nullable_decimal_string()),
            ("maximum", nullable_decimal_string()),
            ("decimals", nullable_integer()),
            ("help", json!({"type": "string"})),
        ],
        vec![],
    );

    let (lemma_type_number_range, spec_number_range) = type_variants(
        "number_range",
        vec![
            ("lower", nullable_decimal_string()),
            ("upper", nullable_decimal_string()),
            ("minimum", nullable_decimal_string()),
            ("maximum", nullable_decimal_string()),
            ("help", json!({"type": "string"})),
        ],
        vec![],
    );

    let (lemma_type_ratio, spec_ratio) = type_variants(
        "ratio",
        vec![
            ("minimum", nullable_decimal_string()),
            ("maximum", nullable_decimal_string()),
            ("decimals", nullable_integer()),
            (
                "units",
                json!({"type": "array", "items": {"$ref": "#/$defs/RatioUnit"}}),
            ),
            ("help", json!({"type": "string"})),
        ],
        vec![("unit", json!({"type": "string"}))],
    );

    let (lemma_type_ratio_range, spec_ratio_range) = type_variants(
        "ratio_range",
        vec![
            ("lower", nullable_decimal_string()),
            ("upper", nullable_decimal_string()),
            ("minimum", nullable_decimal_string()),
            ("maximum", nullable_decimal_string()),
            (
                "units",
                json!({"type": "array", "items": {"$ref": "#/$defs/RatioUnit"}}),
            ),
            ("help", json!({"type": "string"})),
        ],
        vec![],
    );

    let (lemma_type_text, spec_text) = type_variants(
        "text",
        vec![
            ("length", nullable_integer()),
            (
                "options",
                json!({"type": "array", "items": {"type": "string"}}),
            ),
            ("help", json!({"type": "string"})),
        ],
        vec![],
    );

    let (lemma_type_date, spec_date) = type_variants(
        "date",
        vec![
            ("minimum", nullable_string()),
            ("maximum", nullable_string()),
            ("help", json!({"type": "string"})),
        ],
        vec![],
    );

    let (lemma_type_date_range, spec_date_range) = type_variants(
        "date_range",
        vec![
            ("lower", nullable_string()),
            ("upper", nullable_string()),
            ("minimum", nullable_named_bound()),
            ("maximum", nullable_named_bound()),
            ("help", json!({"type": "string"})),
        ],
        vec![],
    );

    let (lemma_type_time, spec_time) = type_variants(
        "time",
        vec![
            ("minimum", nullable_string()),
            ("maximum", nullable_string()),
            ("help", json!({"type": "string"})),
        ],
        vec![],
    );

    let (lemma_type_time_range, spec_time_range) = type_variants(
        "time_range",
        vec![
            ("lower", nullable_string()),
            ("upper", nullable_string()),
            ("minimum", nullable_named_bound()),
            ("maximum", nullable_named_bound()),
            ("help", json!({"type": "string"})),
        ],
        vec![],
    );

    let (lemma_type_measure_range, spec_measure_range) = type_variants(
        "measure_range",
        vec![
            ("lower", nullable_named_bound()),
            ("upper", nullable_named_bound()),
            ("minimum", nullable_named_bound()),
            ("maximum", nullable_named_bound()),
            (
                "units",
                json!({"type": "array", "items": {"$ref": "#/$defs/MeasureUnit"}}),
            ),
            (
                "decomposition",
                json!({"anyOf": [{"type": "null"}, {"type": "object", "additionalProperties": {"type": "integer"}}]}),
            ),
            ("help", json!({"type": "string"})),
        ],
        vec![],
    );

    let (lemma_type_spec, spec_spec) = type_variants(
        "spec",
        vec![
            ("spec", json!({"type": "string"})),
            ("effective", json!({"type": "string"})),
        ],
        vec![("repository", json!({"type": "string"}))],
    );

    // Lists do not nest, so the list variant has no bare `TypeSpecification` form.
    let (lemma_type_list, spec_list) = type_variants(
        "list",
        vec![
            ("element", json!({"$ref": "#/$defs/TypeSpecification"})),
            ("key", json!({"$ref": "#/$defs/ListKey"})),
        ],
        vec![],
    );

    // One body per type kind. A Show property type adds `name`/`extends`; a list element
    // is one of the non-list bodies (lists do not nest).
    let show_type_bodies: Vec<(&str, &Value)> = vec![
        ("ShowTypeBoolean", &spec_boolean),
        ("ShowTypeMeasure", &spec_measure),
        ("ShowTypeNumber", &spec_number),
        ("ShowTypeNumberRange", &spec_number_range),
        ("ShowTypeRatio", &spec_ratio),
        ("ShowTypeRatioRange", &spec_ratio_range),
        ("ShowTypeText", &spec_text),
        ("ShowTypeDate", &spec_date),
        ("ShowTypeDateRange", &spec_date_range),
        ("ShowTypeTime", &spec_time),
        ("ShowTypeTimeRange", &spec_time_range),
        ("ShowTypeMeasureRange", &spec_measure_range),
        ("ShowTypeSpec", &spec_spec),
        ("ShowTypeList", &spec_list),
    ];
    let mut show_type_defs: serde_json::Map<String, Value> = show_type_bodies
        .iter()
        .map(|(name, spec)| ((*name).to_string(), show_type_body(spec)))
        .collect();
    show_type_defs.insert(
        "ShowTypeNameExtends".to_string(),
        json!({
            "type": "object",
            "required": ["name", "extends"],
            "properties": {"name": nullable_string(), "extends": {"$ref": "#/$defs/TypeExtends"}}
        }),
    );
    let show_property_type_variants: Vec<Value> = show_type_bodies
        .iter()
        .filter(|(name, _)| *name != "ShowTypeSpec")
        .map(|(name, _)| show_property_branch(name))
        .collect();
    let show_property_element_type_variants: Vec<Value> = show_type_bodies
        .iter()
        .filter(|(name, _)| *name != "ShowTypeList")
        .map(|(name, _)| show_element_branch(name))
        .collect();

    let rule_result_value_fields = json!({
        "result": {"type": "string"},
        "measure": {"type": "object", "additionalProperties": decimal_string()},
        "ratio": {"type": "object", "additionalProperties": decimal_string()},
        "number": decimal_string(),
        "boolean": {"type": "boolean"},
        "text": {"type": "string"},
        "date": {"type": "string"},
        "time": {"type": "string"},
        "calendar": {"$ref": "#/$defs/CalendarResult"},
        "unit": {"type": "string"}
    });
    // Endpoint properties are RuleResultValue's fields minus `range`: a range endpoint
    // must never itself be a range (enforced by `RuleResultValue::to_literal`).
    let rule_result_value_endpoint_properties =
        rule_result_value_fields.as_object().unwrap().clone();
    let mut rule_result_value_properties = rule_result_value_endpoint_properties.clone();
    rule_result_value_properties
        .insert("range".to_string(), json!({"$ref": "#/$defs/RangeResult"}));
    rule_result_value_properties.insert("spec".to_string(), json!({"$ref": "#/$defs/SpecResult"}));
    rule_result_value_properties.insert("row".to_string(), json!({"$ref": "#/$defs/RowReference"}));
    rule_result_value_properties.insert(
        "list".to_string(),
        json!({"type": "array", "items": {"$ref": "#/$defs/RuleResultValue"}}),
    );
    rule_result_value_properties.insert(
        "fields".to_string(),
        json!({
            "type": "object",
            "additionalProperties": {"$ref": "#/$defs/RuleResultValue"},
            "description": "Show only: on a source row of an instance list value, the fields the row sets, keyed relative to the row spec. Such rows and their list carry no result; row order is array order."
        }),
    );
    rule_result_value_properties.insert(
        "reference".to_string(),
        json!({
            "type": "string",
            "description": "Show only: on a source-row field whose value is a reference; the input key of the referenced data in the spec declaring the list."
        }),
    );

    let show_literal_properties = {
        let mut m = rule_result_value_properties.clone();
        m.insert("type".to_string(), json!({"const": "literal"}));
        m
    };

    let response_result_fields = {
        let mut m = rule_result_value_properties.clone();
        m.insert("vetoed".to_string(), json!({"type": "boolean"}));
        m.insert("veto_reason".to_string(), json!({"type": "string"}));
        m.insert("rule_type".to_string(), json!({"type": "string"}));
        m.insert(
            "explanation".to_string(),
            json!({"$ref": "#/$defs/RuleNode"}),
        );
        m.insert(
            "missing_data".to_string(),
            json!({"type": "array", "items": {"type": "string"}}),
        );
        m
    };

    let rule_node_properties = {
        let mut m = rule_result_value_properties.clone();
        m.insert("type".to_string(), json!({"const": "rule"}));
        m.insert("name".to_string(), json!({"type": "string"}));
        m.insert("body".to_string(), json!({"type": "string"}));
        m.insert(
            "causes".to_string(),
            json!({"type": "array", "items": {"$ref": "#/$defs/Cause"}}),
        );
        m.insert(
            "children".to_string(),
            json!({"type": "array", "items": {"$ref": "#/$defs/ExplanationNode"}}),
        );
        m.insert(
            "rules".to_string(),
            json!({
                "type": "array",
                "items": {"$ref": "#/$defs/ExplainedRule"},
                "description": "Narrated rules reachable from this root, referenced by rule_ref nodes. Ids are dense from 0 in first-reach order."
            }),
        );
        m
    };

    let explained_rule_properties = {
        let mut m = rule_result_value_properties.clone();
        m.insert("id".to_string(), json!({"type": "integer", "minimum": 0}));
        m.insert("name".to_string(), json!({"type": "string"}));
        m.insert("body".to_string(), json!({"type": "string"}));
        m.insert(
            "causes".to_string(),
            json!({"type": "array", "items": {"$ref": "#/$defs/Cause"}}),
        );
        m.insert(
            "children".to_string(),
            json!({"type": "array", "items": {"$ref": "#/$defs/ExplanationNode"}}),
        );
        m
    };

    let rule_ref_node_properties = {
        let mut m = rule_result_value_properties.clone();
        m.insert("type".to_string(), json!({"const": "rule_ref"}));
        m.insert("id".to_string(), json!({"type": "integer", "minimum": 0}));
        m.insert("name".to_string(), json!({"type": "string"}));
        m
    };

    let data_node_properties = {
        let mut m = rule_result_value_properties.clone();
        m.insert("type".to_string(), json!({"const": "data"}));
        m.insert("name".to_string(), json!({"type": "string"}));
        m
    };

    let defs = json!({
        "DecimalString": {
            "type": "string",
            "format": "lemma-decimal",
            "description": "Arbitrary-precision decimal literal (never a JSON number, to avoid float precision loss). Maps to Java BigDecimal, a plain string in TypeScript/Elixir."
        },
        "RationalFactor": {
            "type": "object",
            "required": ["numer", "denom"],
            "additionalProperties": false,
            "description": "Exact rational as a reduced numerator/denominator pair of arbitrary-precision integer strings.",
            "properties": {
                "numer": decimal_string(),
                "denom": decimal_string()
            }
        },
        "NamedBound": {
            "type": "object",
            "required": ["value", "unit"],
            "additionalProperties": false,
            "description": "A unit-scoped bound (Measure/DateRange/TimeRange/MeasureRange minimum/maximum/lower/upper).",
            "properties": {
                "value": decimal_string(),
                "unit": {"type": "string"}
            }
        },
        "MeasureUnit": measure_unit,
        "RatioUnit": ratio_unit,
        "TypeDefiningSpec": {
            "description": "Where a custom type's extension chain is rooted.",
            "oneOf": [
                {"type": "object", "required": ["kind"], "additionalProperties": false, "properties": {"kind": {"const": "local"}}},
                {"type": "object", "required": ["kind"], "additionalProperties": false, "properties": {"kind": {"const": "import"}}}
            ]
        },
        "TypeExtends": {
            "description": "What a type extends: a primitive built-in, or a custom type by name.",
            "oneOf": [
                {"type": "object", "required": ["kind"], "additionalProperties": false, "properties": {"kind": {"const": "primitive"}}},
                {
                    "type": "object",
                    "required": ["kind", "parent", "family", "defining_spec"],
                    "additionalProperties": false,
                    "properties": {
                        "kind": {"const": "custom"},
                        "parent": {"type": "string"},
                        "family": {"type": "string"},
                        "defining_spec": {"$ref": "#/$defs/TypeDefiningSpec"}
                    }
                }
            ]
        },
        "LemmaType": {
            "description": "Resolved Lemma type: `TypeSpecification` fields flattened alongside `name` and `extends`. Sentinel-only specifications (`veto`, `undetermined`) never reach the API and are excluded.",
            "oneOf": [
                {"$ref": "#/$defs/LemmaTypeBoolean"},
                {"$ref": "#/$defs/LemmaTypeMeasure"},
                {"$ref": "#/$defs/LemmaTypeNumber"},
                {"$ref": "#/$defs/LemmaTypeNumberRange"},
                {"$ref": "#/$defs/LemmaTypeRatio"},
                {"$ref": "#/$defs/LemmaTypeRatioRange"},
                {"$ref": "#/$defs/LemmaTypeText"},
                {"$ref": "#/$defs/LemmaTypeDate"},
                {"$ref": "#/$defs/LemmaTypeDateRange"},
                {"$ref": "#/$defs/LemmaTypeTime"},
                {"$ref": "#/$defs/LemmaTypeTimeRange"},
                {"$ref": "#/$defs/LemmaTypeMeasureRange"},
                {"$ref": "#/$defs/LemmaTypeSpec"},
                {"$ref": "#/$defs/LemmaTypeList"}
            ]
        },
        "TypeSpecification": {
            "description": "Element type of a list: the `LemmaType` fields without `name` and `extends`. A spec element names the row spec of an instance list. Lists do not nest.",
            "oneOf": [
                spec_boolean,
                spec_measure,
                spec_number,
                spec_number_range,
                spec_ratio,
                spec_ratio_range,
                spec_text,
                spec_date,
                spec_date_range,
                spec_time,
                spec_time_range,
                spec_measure_range,
                spec_spec
            ]
        },
        "ListKey": {
            "description": "How a list addresses its rows: \"numbered\" (by row number), or the ordered `-> key` field names.",
            "oneOf": [
                {"const": "numbered"},
                {
                    "type": "object",
                    "required": ["fields"],
                    "additionalProperties": false,
                    "properties": {
                        "fields": {"type": "array", "items": {"type": "string"}, "minItems": 1}
                    }
                }
            ]
        },
        "LemmaTypeBoolean": lemma_type_boolean,
        "LemmaTypeMeasure": lemma_type_measure,
        "LemmaTypeNumber": lemma_type_number,
        "LemmaTypeNumberRange": lemma_type_number_range,
        "LemmaTypeRatio": lemma_type_ratio,
        "LemmaTypeRatioRange": lemma_type_ratio_range,
        "LemmaTypeText": lemma_type_text,
        "LemmaTypeDate": lemma_type_date,
        "LemmaTypeDateRange": lemma_type_date_range,
        "LemmaTypeTime": lemma_type_time,
        "LemmaTypeTimeRange": lemma_type_time_range,
        "LemmaTypeMeasureRange": lemma_type_measure_range,
        "LemmaTypeSpec": lemma_type_spec,
        "LemmaTypeList": lemma_type_list,
        "CalendarResult": {
            "type": "object",
            "required": ["value", "unit"],
            "additionalProperties": false,
            "properties": {
                "value": decimal_string(),
                "unit": {"type": "string"}
            }
        },
        "RuleResultValueEndpoint": {
            "type": "object",
            "additionalProperties": false,
            "description": "RuleResultValue fields without `range` — a range endpoint must never itself be a range.",
            "properties": rule_result_value_endpoint_properties
        },
        "RangeResult": {
            "type": "object",
            "required": ["from", "to"],
            "additionalProperties": false,
            "properties": {
                "from": {"$ref": "#/$defs/RuleResultValueEndpoint"},
                "to": {"$ref": "#/$defs/RuleResultValueEndpoint"}
            }
        },
        "SpecRuleResult": {
            "type": "object",
            "required": ["vetoed", "rule_type"],
            "additionalProperties": false,
            "description": "One rule of a spec instance, or one evaluated data slot of a spec instance. A spec-valued rule nests SpecResult in result.spec.",
            "properties": {
                "vetoed": {"type": "boolean"},
                "veto_reason": {"type": "string"},
                "rule_type": {"type": "string"},
                "result": {"$ref": "#/$defs/RuleResultValue"}
            }
        },
        "SpecResult": {
            "type": "object",
            "required": ["spec", "effective", "instance", "path", "data", "rules"],
            "additionalProperties": false,
            "description": "Spec instance returned by a rule. data holds one evaluated entry per data slot, keyed relative to the spec; unbound slots are omitted.",
            "properties": {
                "repository": {"type": "string"},
                "spec": {"type": "string"},
                "effective": {"type": "string"},
                "instance": {"type": "string"},
                "path": {
                    "type": "array",
                    "items": {"$ref": "#/$defs/PathSegment"},
                    "minItems": 1,
                    "description": "One segment per uses hop of instance."
                },
                "data": {
                    "type": "object",
                    "additionalProperties": {"$ref": "#/$defs/SpecRuleResult"}
                },
                "rules": {
                    "type": "object",
                    "additionalProperties": {"$ref": "#/$defs/SpecRuleResult"}
                }
            }
        },
        "RowReference": {
            "type": "object",
            "required": ["spec", "effective", "instance", "index"],
            "additionalProperties": false,
            "description": "A row of an instance list. Its data and rules are Response.rows[index]. instance is the row path relative to the spec holding this value (lines.2).",
            "properties": {
                "repository": {"type": "string"},
                "spec": {"type": "string"},
                "effective": {"type": "string"},
                "instance": {"type": "string"},
                "index": {"type": "integer", "minimum": 0}
            }
        },
        "RowResult": {
            "type": "object",
            "required": ["spec", "effective", "data", "rules"],
            "additionalProperties": false,
            "description": "Data and rules of one distinct row reached by a response, filled once and referenced by RowReference.index. Keys are relative to the row spec; unbound slots are omitted.",
            "properties": {
                "repository": {"type": "string"},
                "spec": {"type": "string"},
                "effective": {"type": "string"},
                "data": {
                    "type": "object",
                    "additionalProperties": {"$ref": "#/$defs/SpecRuleResult"}
                },
                "rules": {
                    "type": "object",
                    "additionalProperties": {"$ref": "#/$defs/SpecRuleResult"}
                }
            }
        },
        "RuleResultValue": {
            "type": "object",
            "additionalProperties": false,
            "description": "API value shared by RuleResult (flattened), Show property x-lemma-value/x-lemma-suggestion, and explanation Rule/Data nodes (flattened). When present: always `result`, plus exactly one typed field for a non-range value; `range` is set instead for a range value. A list sets `list`, one value per row. A list row sets `row` in rule results and RowResult members; in explanation nodes and Show it carries `result` only.",
            "properties": rule_result_value_properties
        },
        "PathSegment": {
            "type": "object",
            "required": ["uses", "spec"],
            "additionalProperties": false,
            "description": "One uses hop on a Show data or rule path. uses is the alias; repository is omitted for the unnamed workspace; spec is the resolved target name.",
            "properties": {
                "uses": {"type": "string"},
                "repository": {"type": "string"},
                "spec": {"type": "string"}
            }
        },
        "EvaluateDataJson": {
            "description": "An evaluation-data JSON value: a literal string, a boolean, a list (array), or an object (list row, or single-unit measure map). JSON numbers and null are not evaluation data.",
            "anyOf": [
                {"type": "string"},
                {"type": "boolean"},
                {"type": "array", "items": {"$ref": "#/$defs/EvaluateDataJson"}},
                {"type": "object", "additionalProperties": {"$ref": "#/$defs/EvaluateDataJson"}}
            ]
        },
        "ShowPropertyType": {
            "description": "x-lemma-type of a Show property: the LemmaType without help and text options/length, which the property states as description, enum and minLength/maxLength. Single spec types are uses aliases, never properties.",
            "oneOf": show_property_type_variants
        },
        "ShowPropertyElementType": {
            "description": "Element of a list ShowPropertyType: the TypeSpecification without help and text options/length.",
            "oneOf": show_property_element_type_variants
        },
        "ShowItems": {
            "description": "items of a list property: the element literal schema of a value list, or a $ref to the row object schema of an instance list in the document's $defs.",
            "oneOf": [
                {
                    "type": "object",
                    "anyOf": [{"required": ["type"]}, {"required": ["oneOf"]}],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"enum": ["string", "boolean"]},
                        "oneOf": {"type": "array", "minItems": 2, "items": {"type": "object"}},
                        "pattern": {"type": "string"},
                        "enum": {"type": "array", "items": {"type": "string"}},
                        "minLength": {"type": "integer", "minimum": 0},
                        "maxLength": {"type": "integer", "minimum": 0}
                    }
                },
                {
                    "type": "object",
                    "required": ["$ref"],
                    "additionalProperties": false,
                    "properties": {
                        "$ref": {"type": "string", "pattern": "^#/\\$defs/LemmaRow_[A-Za-z0-9_-]+$"}
                    }
                }
            ]
        },
        "ShowProperty": {
            "type": "object",
            "required": ["x-lemma-type", "x-lemma-path", "x-lemma-needed-by-rules"],
            "anyOf": [{"required": ["type"]}, {"required": ["oneOf"]}],
            "additionalProperties": false,
            "description": "One evaluation-data input of a Show document, keyed by its input key (dotted keys included). Standard keywords state the JSON shape; default/examples are the evaluation-data JSON of x-lemma-value/x-lemma-suggestion.",
            "properties": {
                "type": {"enum": ["boolean", "string", "array"]},
                "oneOf": {"type": "array", "minItems": 2, "items": {"type": "object"}, "description": "Alternatives: text, boolean, or a single-unit object for measure and ratio data."},
                "pattern": {"type": "string", "description": "Literal grammar of number, measure and ratio inputs (ECMA-262)."},
                "enum": {"type": "array", "items": {"type": "string"}, "description": "Text options."},
                "minLength": {"type": "integer", "minimum": 0},
                "maxLength": {"type": "integer", "minimum": 0},
                "description": {"type": "string", "description": "Type help."},
                "default": {"$ref": "#/$defs/EvaluateDataJson"},
                "examples": {"type": "array", "items": {"$ref": "#/$defs/EvaluateDataJson"}, "minItems": 1, "maxItems": 1},
                "items": {"$ref": "#/$defs/ShowItems"},
                "maxItems": {"type": "integer", "minimum": 0, "description": "max_list_rows."},
                "x-lemma-type": {"$ref": "#/$defs/ShowPropertyType"},
                "x-lemma-path": {
                    "type": "array",
                    "items": {"$ref": "#/$defs/PathSegment"},
                    "description": "Import hops to this slot. Empty means this spec."
                },
                "x-lemma-needed-by-rules": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "x-lemma-rules keys that transitively need this data after normalize. Empty = reuse catalog only (not an eval intake key for this spec)."
                },
                "x-lemma-value": {"$ref": "#/$defs/RuleResultValue"},
                "x-lemma-suggestion": {"$ref": "#/$defs/RuleResultValue"}
            }
        },
        "ShowRowSchema": {
            "type": "object",
            "required": ["title", "type", "additionalProperties", "properties"],
            "additionalProperties": false,
            "description": "Row object schema of an instance list, one per row spec in the Show document's $defs. Every row-spec data entry is a property, dotted uses fields included.",
            "properties": {
                "title": {"type": "string"},
                "type": {"const": "object"},
                "additionalProperties": {"const": true},
                "properties": {"type": "object", "additionalProperties": {"$ref": "#/$defs/ShowProperty"}},
                "required": {"type": "array", "items": {"type": "string"}}
            }
        },
        "ShowConversionTarget": {
            "description": "Target of a Show `as` conversion (inspection only).",
            "oneOf": [
                {
                    "type": "object",
                    "required": ["type"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"type": "string", "enum": [
                            "boolean", "measure", "measure_range", "number", "number_range",
                            "ratio", "ratio_range", "text", "date", "date_range", "time", "time_range"
                        ]}
                    }
                },
                {
                    "type": "object",
                    "required": ["unit"],
                    "additionalProperties": false,
                    "properties": {
                        "unit": {
                            "type": "object",
                            "required": ["unit_name"],
                            "additionalProperties": false,
                            "properties": {"unit_name": {"type": "string"}}
                        }
                    }
                }
            ]
        },
        "ShowExpression": {
            "description": "Resolved expression on a Show rule branch. Internally tagged by `type`.",
            "oneOf": [
                {
                    "type": "object",
                    "required": ["type"],
                    "additionalProperties": false,
                    "properties": show_literal_properties
                },
                {
                    "type": "object",
                    "required": ["type", "name"],
                    "additionalProperties": false,
                    "properties": {"type": {"const": "data"}, "name": {"type": "string"}}
                },
                {
                    "type": "object",
                    "required": ["type", "name"],
                    "additionalProperties": false,
                    "properties": {"type": {"const": "rule"}, "name": {"type": "string"}}
                },
                {
                    "type": "object",
                    "required": ["type", "operands"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "and"},
                        "operands": {
                            "type": "array",
                            "minItems": 2,
                            "items": {"$ref": "#/$defs/ShowExpression"}
                        }
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "operand"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "not"},
                        "operand": {"$ref": "#/$defs/ShowExpression"}
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "first", "steps"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "arithmetic"},
                        "first": {"$ref": "#/$defs/ShowExpression"},
                        "steps": {
                            "type": "array",
                            "minItems": 1,
                            "items": {
                                "type": "object",
                                "required": ["op", "operand"],
                                "additionalProperties": false,
                                "properties": {
                                    "op": {"type": "string", "enum": ["add", "subtract", "multiply", "divide", "modulo", "power"]},
                                    "operand": {"$ref": "#/$defs/ShowExpression"}
                                }
                            }
                        }
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "op", "left", "right"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "comparison"},
                        "op": {"type": "string", "enum": [
                            "greater_than", "less_than", "greater_than_or_equal",
                            "less_than_or_equal", "is", "is_not"
                        ]},
                        "left": {"$ref": "#/$defs/ShowExpression"},
                        "right": {"$ref": "#/$defs/ShowExpression"}
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "operand", "targets"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "unit_conversion"},
                        "operand": {"$ref": "#/$defs/ShowExpression"},
                        "targets": {"type": "array", "items": {"$ref": "#/$defs/ShowConversionTarget"}}
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "op", "operand"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "math"},
                        "op": {"type": "string", "enum": [
                            "sqrt", "sin", "cos", "tan", "asin", "acos", "atan",
                            "log", "exp", "abs", "floor", "ceil", "round"
                        ]},
                        "operand": {"$ref": "#/$defs/ShowExpression"}
                    }
                },
                {
                    "type": "object",
                    "required": ["type"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "veto"},
                        "message": {"type": "string"}
                    }
                },
                {
                    "type": "object",
                    "required": ["type"],
                    "additionalProperties": false,
                    "properties": {"type": {"const": "now"}}
                },
                {
                    "type": "object",
                    "required": ["type", "kind", "operand"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "date_relative"},
                        "kind": {"type": "string", "enum": ["in_past", "in_future"]},
                        "operand": {"$ref": "#/$defs/ShowExpression"}
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "kind", "unit", "operand"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "date_calendar"},
                        "kind": {"type": "string", "enum": ["current", "past", "future", "not_in"]},
                        "unit": {"type": "string", "enum": ["year", "month", "week"]},
                        "operand": {"$ref": "#/$defs/ShowExpression"}
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "from", "to"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "range_literal"},
                        "from": {"$ref": "#/$defs/ShowExpression"},
                        "to": {"$ref": "#/$defs/ShowExpression"}
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "kind", "operand"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "past_future_range"},
                        "kind": {"type": "string", "enum": ["in_past", "in_future"]},
                        "operand": {"$ref": "#/$defs/ShowExpression"}
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "value", "range"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "range_containment"},
                        "value": {"$ref": "#/$defs/ShowExpression"},
                        "range": {"$ref": "#/$defs/ShowExpression"}
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "bound", "operand"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "range_bound"},
                        "bound": {"type": "string", "enum": ["lower", "upper"]},
                        "operand": {"$ref": "#/$defs/ShowExpression"}
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "operand"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "is_veto"},
                        "operand": {"$ref": "#/$defs/ShowExpression"}
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "base", "member"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "spec_member"},
                        "base": {"type": "string"},
                        "member": {"type": "string"}
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "base", "member"],
                    "additionalProperties": false,
                    "description": "Field of a computed row value: `(menu -> find ordered).price`.",
                    "properties": {
                        "type": {"const": "member"},
                        "base": {"$ref": "#/$defs/ShowExpression"},
                        "member": {"type": "string"}
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "field"],
                    "additionalProperties": false,
                    "description": "Field of the current row inside a list command argument.",
                    "properties": {
                        "type": {"const": "row_field"},
                        "field": {"type": "string"}
                    }
                },
                {
                    "type": "object",
                    "required": ["type"],
                    "additionalProperties": false,
                    "description": "The current row's value inside a value-list command argument.",
                    "properties": {"type": {"const": "row_value"}}
                },
                {
                    "type": "object",
                    "required": ["type", "field", "value"],
                    "additionalProperties": false,
                    "description": "One `field: value` pair of an inline row appended by `-> with`.",
                    "properties": {
                        "type": {"const": "row_field_value"},
                        "field": {"type": "string"},
                        "value": {"$ref": "#/$defs/ShowExpression"}
                    }
                },
                {
                    "type": "object",
                    "required": ["type", "list", "command", "args"],
                    "additionalProperties": false,
                    "description": "Postfix list command (`staff -> sum bonus`). direction is present exactly on sort. A value-list condition is a comparison or range_containment whose left operand is row_value.",
                    "properties": {
                        "type": {"const": "list_command"},
                        "list": {"$ref": "#/$defs/ShowExpression"},
                        "command": {"type": "string", "enum": [
                            "find", "first", "last", "count", "sum", "avg", "product", "min",
                            "max", "any", "all", "where", "sort", "top", "with"
                        ]},
                        "direction": {"type": "string", "enum": ["asc", "desc"]},
                        "args": {"type": "array", "items": {"$ref": "#/$defs/ShowExpression"}}
                    }
                }
            ]
        },
        "ShowBranch": {
            "type": "object",
            "required": ["result"],
            "additionalProperties": false,
            "description": "One arm of a rule's flat last-match table. Default arm omits condition.",
            "properties": {
                "condition": {"$ref": "#/$defs/ShowExpression"},
                "result": {"$ref": "#/$defs/ShowExpression"}
            }
        },
        "ShowRule": {
            "type": "object",
            "required": ["type", "path", "branches", "depends_on_rules"],
            "additionalProperties": false,
            "description": "Rule on Show: result type, import path ([] = this spec), authored default/unless branches, and depends_on_rules (Show.rules keys).",
            "properties": {
                "type": {"$ref": "#/$defs/LemmaType"},
                "path": {
                    "type": "array",
                    "items": {"$ref": "#/$defs/PathSegment"},
                    "description": "Import hops to this rule. Empty means this spec."
                },
                "branches": {"type": "array", "items": {"$ref": "#/$defs/ShowBranch"}},
                "depends_on_rules": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Show.rules keys this rule depends on (planning topo). Always present."
                }
            }
        },
        "SourceType": {
            "description": "Provenance of a loaded source. Externally tagged; the unit `Volatile` variant is the bare string \"volatile\".",
            "oneOf": [
                {"const": "volatile"},
                {"type": "object", "required": ["path"], "additionalProperties": false, "properties": {"path": {"type": "string"}}},
                {"type": "object", "required": ["dependency"], "additionalProperties": false, "properties": {"dependency": {"type": "string"}}}
            ]
        },
        "LiteralValue": {
            "description": "Parsed literal value. Externally tagged.",
            "oneOf": [
                {"type": "object", "required": ["number"], "additionalProperties": false, "properties": {"number": decimal_string()}},
                {
                    "type": "object",
                    "required": ["number_with_unit"],
                    "additionalProperties": false,
                    "properties": {
                        "number_with_unit": {
                            "type": "array",
                            "prefixItems": [decimal_string(), {"type": "string"}],
                            "items": false,
                            "minItems": 2,
                            "maxItems": 2
                        }
                    }
                },
                {"type": "object", "required": ["text"], "additionalProperties": false, "properties": {"text": {"type": "string"}}},
                {"type": "object", "required": ["date"], "additionalProperties": false, "properties": {"date": {"type": "string"}}},
                {"type": "object", "required": ["time"], "additionalProperties": false, "properties": {"time": {"type": "string"}}},
                {
                    "type": "object",
                    "required": ["boolean"],
                    "additionalProperties": false,
                    "properties": {"boolean": {"enum": ["true", "false", "yes", "no"]}}
                },
                {
                    "type": "object",
                    "required": ["range"],
                    "additionalProperties": false,
                    "properties": {
                        "range": {
                            "type": "array",
                            "prefixItems": [{"$ref": "#/$defs/LiteralValue"}, {"$ref": "#/$defs/LiteralValue"}],
                            "items": false,
                            "minItems": 2,
                            "maxItems": 2
                        }
                    }
                }
            ]
        },
        "Show": {
            "description": "Result of Engine::show: a Draft 2020-12 JSON Schema for the spec's evaluation-data inputs. properties holds inputs rules need (non-empty x-lemma-needed-by-rules), required the unbound ones, $defs instance-list row schemas. x-lemma-types is local data declarations; x-lemma-rules is this spec's rule graph (local plus reachable imports).",
            "allOf": [
                {"$ref": "https://json-schema.org/draft/2020-12/schema"},
                {
                    "type": "object",
                    "required": ["$schema", "title", "type", "additionalProperties", "properties", "x-lemma-start-line", "x-lemma-meta", "x-lemma-types", "x-lemma-rules"],
                    "additionalProperties": false,
                    "properties": {
                        "$schema": {"const": "https://json-schema.org/draft/2020-12/schema"},
                        "title": {"type": "string", "description": "Spec name."},
                        "description": {"type": "string", "description": "Spec commentary."},
                        "type": {"const": "object"},
                        "additionalProperties": {"const": true},
                        "properties": {"type": "object", "additionalProperties": {"$ref": "#/$defs/ShowProperty"}},
                        "required": {"type": "array", "items": {"type": "string"}},
                        "$defs": {"type": "object", "additionalProperties": {"$ref": "#/$defs/ShowRowSchema"}},
                        "x-lemma-repository": {"type": "string", "description": "Interned repository name. Omitted for the unnamed workspace."},
                        "x-lemma-effective-from": {"type": "string"},
                        "x-lemma-effective-to": {"type": "string"},
                        "x-lemma-start-line": {"type": "integer"},
                        "x-lemma-source-type": {"$ref": "#/$defs/SourceType"},
                        "x-lemma-meta": {"type": "object", "additionalProperties": {"$ref": "#/$defs/LiteralValue"}},
                        "x-lemma-types": {
                            "type": "object",
                            "additionalProperties": {"$ref": "#/$defs/LemmaType"},
                            "description": "Local data declarations (empty path), declaration order."
                        },
                        "x-lemma-rules": {"type": "object", "additionalProperties": {"$ref": "#/$defs/ShowRule"}}
                    }
                }
            ]
        },
        "RuleResult": {
            "type": "object",
            "required": ["vetoed", "rule_type"],
            "additionalProperties": false,
            "description": "Result of evaluating one rule. RuleResultValue fields are flattened directly onto this object (no `value` wrapper key) when the rule is not vetoed.",
            "properties": response_result_fields
        },
        "Response": {
            "type": "object",
            "required": ["spec", "effective", "results"],
            "additionalProperties": false,
            "description": "Result of Engine::evaluate.",
            "properties": {
                "spec": {"type": "string"},
                "effective": {"type": "string"},
                "spec_effective_from": {"type": "string"},
                "spec_effective_to": {"type": "string"},
                "results": {"type": "object", "additionalProperties": {"$ref": "#/$defs/RuleResult"}},
                "rows": {
                    "type": "array",
                    "items": {"$ref": "#/$defs/RowResult"},
                    "description": "Rows reached by results, in first-reached order, indexed by RowReference.index. Omitted when no result reaches a row."
                }
            }
        },
        "SpecDependency": {
            "type": "object",
            "required": ["name"],
            "additionalProperties": false,
            "description": "One uses target written on a spec version.",
            "properties": {
                "repository": {"type": "string"},
                "name": {"type": "string"},
                "effective": {"type": "string"}
            }
        },
        "SpecVersion": {
            "type": "object",
            "required": ["name"],
            "additionalProperties": false,
            "description": "One temporal spec version from Engine::specs.",
            "properties": {
                "name": {"type": "string"},
                "effective_from": {"type": "string"},
                "effective_to": {"type": "string"},
                "dependencies": {
                    "type": "array",
                    "items": {"$ref": "#/$defs/SpecDependency"},
                    "description": "uses targets in declaration order. Omitted when empty."
                }
            }
        },
        "Repository": {
            "type": "object",
            "required": ["specs"],
            "additionalProperties": false,
            "description": "One repository group from Engine::specs. `repository` is absent for the default unnamed repository.",
            "properties": {
                "repository": {"type": "string"},
                "specs": {"type": "array", "items": {"$ref": "#/$defs/SpecVersion"}}
            }
        },
        "EngineErrorSource": {
            "type": "object",
            "required": ["attribute", "line", "column", "length"],
            "additionalProperties": false,
            "description": "Source location attached to an EngineError. Line/column are 1-based; length is the UTF-8 byte length of the offending span.",
            "properties": {
                "attribute": {"type": "string"},
                "line": {"type": "integer"},
                "column": {"type": "integer"},
                "length": {"type": "integer"}
            }
        },
        "Recommendation": {
            "type": "object",
            "required": ["message", "spec", "effective_from", "repository", "source"],
            "additionalProperties": false,
            "description": "Structural quality recommendation from Engine::quality. Advisory only.",
            "properties": {
                "message": {"type": "string"},
                "spec": {"type": "string"},
                "effective_from": {
                    "type": ["string", "null"],
                    "description": "Declared temporal start of the analyzed slice, or null for Origin."
                },
                "repository": {"type": ["string", "null"]},
                "source": {"$ref": "#/$defs/EngineErrorSource"}
            }
        },
        "EngineError": {
            "type": "object",
            "required": [
                "kind", "message", "related_data", "spec", "related_spec", "source",
                "suggestion", "repository", "registry_kind", "request_kind",
                "limit_name", "limit_value", "actual_value"
            ],
            "additionalProperties": false,
            "description": "Structured error thrown by Engine::evaluate/show/load/fetch.",
            "properties": {
                "kind": {"enum": ["parsing", "validation", "inversion", "registry", "missing_repository", "request", "resource_limit"]},
                "message": {"type": "string"},
                "related_data": nullable_string(),
                "spec": nullable_string(),
                "related_spec": nullable_string(),
                "source": {"anyOf": [{"type": "null"}, {"$ref": "#/$defs/EngineErrorSource"}]},
                "suggestion": nullable_string(),
                "repository": nullable_string(),
                "registry_kind": {"anyOf": [{"type": "null"}, {"enum": ["not_found", "unauthorized", "network_error", "server_error", "other"]}]},
                "request_kind": {"anyOf": [{"type": "null"}, {"enum": ["spec_not_found", "rule_not_found", "invalid_request"]}]},
                "limit_name": nullable_string(),
                "limit_value": nullable_string(),
                "actual_value": nullable_string()
            }
        },
        "Cause": {
            "type": "object",
            "required": ["condition", "value"],
            "additionalProperties": false,
            "description": "One evaluated unless condition, stated as a fact. A falsified comparison is flipped to its complement, so the condition text describes what held.",
            "properties": {
                "condition": {"type": "string", "description": "True-form condition expression text from the source rule."},
                "value": {"type": "string", "description": "\"true\" for facts stated positively (including flipped conditions), \"false\" when the condition could not be flipped into a positive statement, or the veto text when evaluating the condition vetoed."},
                "children": {"type": "array", "items": {"$ref": "#/$defs/ExplanationNode"}}
            }
        },
        "ConversionStep": {
            "type": "object",
            "required": ["role", "text"],
            "additionalProperties": false,
            "properties": {
                "role": {"enum": ["outcome", "rule"]},
                "text": {"type": "string"}
            }
        },
        "ExplanationNode": {
            "oneOf": [
                {"$ref": "#/$defs/RuleRefNode"},
                {"$ref": "#/$defs/ComposeNode"},
                {"$ref": "#/$defs/DataNode"},
                {"$ref": "#/$defs/DataUnusedNode"},
                {"$ref": "#/$defs/ConversionNode"},
                {"$ref": "#/$defs/VetoNode"},
                {"$ref": "#/$defs/RowNode"},
                {"$ref": "#/$defs/FilterNode"}
            ]
        },
        "RuleNode": {
            "type": "object",
            "required": ["type", "name", "result", "body"],
            "additionalProperties": false,
            "description": "Root rule explanation under RuleResult.explanation. Nested rule uses are rule_ref into the rules table. RuleResultValue fields are flattened.",
            "properties": rule_node_properties
        },
        "ExplainedRule": {
            "type": "object",
            "required": ["id", "name", "result", "body"],
            "additionalProperties": false,
            "description": "One narrated rule body in a root explanation's rules table.",
            "properties": explained_rule_properties
        },
        "RuleRefNode": {
            "type": "object",
            "required": ["type", "id", "name", "result"],
            "additionalProperties": false,
            "description": "Reference to an ExplainedRule in the enclosing root explanation's rules table. RuleResultValue fields are flattened.",
            "properties": rule_ref_node_properties
        },
        "RowNode": {
            "type": "object",
            "required": ["type", "number", "fields"],
            "additionalProperties": false,
            "description": "One instance-list row a list command read. Label in ASCII is n. when unkeyed, or [key values] when field-keyed.",
            "properties": {
                "type": {"const": "row"},
                "number": {"type": "integer", "minimum": 1},
                "appended": {"type": "boolean", "description": "True when the row was appended by an expression -> with."},
                "key": {"type": "array", "items": {"$ref": "#/$defs/ExplanationNode"}, "description": "Key field Data nodes in -> key order; empty for numbered lists."},
                "fields": {"type": "array", "items": {"$ref": "#/$defs/ExplanationNode"}}
            }
        },
        "FilterNode": {
            "type": "object",
            "required": ["type", "condition", "matched", "total"],
            "additionalProperties": false,
            "description": "Summary of a where filter: matched of total rows satisfied the condition.",
            "properties": {
                "type": {"const": "filter"},
                "condition": {"type": "string"},
                "matched": {"type": "integer", "minimum": 0},
                "total": {"type": "integer", "minimum": 0},
                "children": {"type": "array", "items": {"$ref": "#/$defs/ExplanationNode"}}
            }
        },
        "ComposeNode": {
            "type": "object",
            "required": ["type", "expression", "operands"],
            "additionalProperties": false,
            "properties": {
                "type": {"const": "compose"},
                "expression": {"type": "string"},
                "operator": {
                    "type": "string",
                    "enum": ["add", "subtract", "multiply", "divide", "modulo", "power"]
                },
                "operands": {"type": "array", "items": {"$ref": "#/$defs/ExplanationNode"}}
            }
        },
        "DataNode": {
            "type": "object",
            "required": ["type", "name", "result"],
            "additionalProperties": false,
            "description": "Bound data narration. RuleResultValue fields are flattened (result plus typed measure/ratio/… maps).",
            "properties": data_node_properties
        },
        "DataUnusedNode": {
            "type": "object",
            "required": ["type", "name"],
            "additionalProperties": false,
            "description": "Data path present in a cause condition structure but never looked up (short-circuit or static and-false).",
            "properties": {
                "type": {"const": "data_unused"},
                "name": {"type": "string"}
            }
        },
        "ConversionNode": {
            "type": "object",
            "required": ["type", "expression", "steps", "operands"],
            "additionalProperties": false,
            "properties": {
                "type": {"const": "conversion"},
                "expression": {"type": "string"},
                "steps": {"type": "array", "items": {"$ref": "#/$defs/ConversionStep"}},
                "operands": {"type": "array", "items": {"$ref": "#/$defs/ExplanationNode"}}
            }
        },
        "VetoNode": {
            "type": "object",
            "required": ["type"],
            "additionalProperties": false,
            "properties": {
                "type": {"const": "veto"},
                "message": {"type": "string"}
            }
        },
        "RepositoryInstallResult": {
            "type": "object",
            "required": ["source", "id"],
            "additionalProperties": false,
            "description": "Result of Engine.install / Lemma.install: LemmaBase repository source text and normalized id. Not yet loaded.",
            "properties": {
                "source": {"type": "string"},
                "id": {"type": "string"}
            }
        }
    });

    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://github.com/lemma/lemma/blob/main/engine/schemas/api.v1.json",
        "title": "Lemma engine API",
        "description": "Schema for the JSON document shapes the engine produces at its consumer boundary: Show (Engine::show), Response (Engine::evaluate), specs (Engine::specs, array of Repository), errors (array of EngineError), and RepositoryInstallResult (SDK install). Explanation trees live under RuleResult.explanation (RuleNode / ExplanationNode).",
        "$defs": with_defs(defs, show_type_defs),
        "anyOf": [
            {"$ref": "#/$defs/Show"},
            {"$ref": "#/$defs/Response"},
            {"type": "array", "items": {"$ref": "#/$defs/Repository"}},
            {"type": "array", "items": {"$ref": "#/$defs/EngineError"}},
            {"$ref": "#/$defs/RepositoryInstallResult"}
        ]
    })
}

fn schema_path(root: &Path) -> PathBuf {
    root.join(SCHEMA_REL_PATH)
}

fn render() -> Result<String, String> {
    let mut text = serde_json::to_string_pretty(&api_v1_schema()).map_err(|e| e.to_string())?;
    text.push('\n');
    Ok(text)
}

/// Fails when the checked-in schema differs from the generated one. Never writes.
pub fn check(root: &Path) -> Result<(), String> {
    let text = render()?;
    let path = schema_path(root);
    let checked_in =
        std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if checked_in != text {
        return Err(format!(
            "{} is stale; run `cargo run -p xtask -- schema`",
            path.display()
        ));
    }
    Ok(())
}

pub fn run(root: &Path) -> Result<(), String> {
    let text = render()?;
    let path = schema_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    eprintln!("xtask: wrote {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_valid_json_and_self_consistent() {
        let schema = api_v1_schema();
        assert!(schema.get("$defs").is_some());
        assert!(schema.get("anyOf").is_some());
        assert_eq!(
            schema["$id"],
            "https://github.com/lemma/lemma/blob/main/engine/schemas/api.v1.json"
        );
    }
}
