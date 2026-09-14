//! Emits `engine/schemas/api.v1.json`: the JSON Schema for Show, Response,
//! `list`, and error documents (engine `Engine::show`/`run`/`list`, thrown `EngineError`
//! arrays). Hand-authored, not derived by macro, because the wire shapes involve
//! hand-written `Serialize` impls (decimal strings, externally/internally tagged enums)
//! that a derive-based schema generator cannot see.
//!
//! `cargo run -p xtask -- schema` regenerates the file; regeneration must be a no-op
//! against the checked-in copy (enforced by `engine/tests/api_schema.rs`).

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

/// One `LemmaType` variant: the flattened `TypeSpecification` fields plus the
/// always-present `name`/`kind`/`extends` that `LemmaType` adds around them.
fn lemma_type_variant(kind: &str, spec_fields: Vec<(&str, Value)>) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<Value> = vec!["name".into(), "kind".into()];
    properties.insert("name".to_string(), nullable_string());
    properties.insert("kind".to_string(), json!({"const": kind}));
    for (name, schema) in spec_fields {
        required.push(name.into());
        properties.insert(name.to_string(), schema);
    }
    required.push("extends".into());
    properties.insert(
        "extends".to_string(),
        json!({"$ref": "#/$defs/TypeExtends"}),
    );
    json!({
        "type": "object",
        "required": required,
        "additionalProperties": false,
        "properties": properties,
    })
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

    let lemma_type_boolean =
        lemma_type_variant("boolean", vec![("help", json!({"type": "string"}))]);

    let lemma_type_measure = lemma_type_variant(
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
    );

    let lemma_type_number = lemma_type_variant(
        "number",
        vec![
            ("minimum", nullable_decimal_string()),
            ("maximum", nullable_decimal_string()),
            ("decimals", nullable_integer()),
            ("help", json!({"type": "string"})),
        ],
    );

    let lemma_type_number_range = lemma_type_variant(
        "numberrange",
        vec![
            ("lower", nullable_decimal_string()),
            ("upper", nullable_decimal_string()),
            ("minimum", nullable_decimal_string()),
            ("maximum", nullable_decimal_string()),
            ("help", json!({"type": "string"})),
        ],
    );

    let lemma_type_ratio = lemma_type_variant(
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
    );

    let lemma_type_ratio_range = lemma_type_variant(
        "ratiorange",
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
    );

    let lemma_type_text = lemma_type_variant(
        "text",
        vec![
            ("length", nullable_integer()),
            (
                "options",
                json!({"type": "array", "items": {"type": "string"}}),
            ),
            ("help", json!({"type": "string"})),
        ],
    );

    let lemma_type_date = lemma_type_variant(
        "date",
        vec![
            ("minimum", nullable_string()),
            ("maximum", nullable_string()),
            ("help", json!({"type": "string"})),
        ],
    );

    let lemma_type_date_range = lemma_type_variant(
        "daterange",
        vec![
            ("lower", nullable_string()),
            ("upper", nullable_string()),
            ("minimum", nullable_named_bound()),
            ("maximum", nullable_named_bound()),
            ("help", json!({"type": "string"})),
        ],
    );

    let lemma_type_time = lemma_type_variant(
        "time",
        vec![
            ("minimum", nullable_string()),
            ("maximum", nullable_string()),
            ("help", json!({"type": "string"})),
        ],
    );

    let lemma_type_time_range = lemma_type_variant(
        "timerange",
        vec![
            ("lower", nullable_string()),
            ("upper", nullable_string()),
            ("minimum", nullable_named_bound()),
            ("maximum", nullable_named_bound()),
            ("help", json!({"type": "string"})),
        ],
    );

    let lemma_type_measure_range = lemma_type_variant(
        "measurerange",
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
    );

    let rule_result_value_fields = json!({
        "display": {"type": "string"},
        "measure": {"type": "object", "additionalProperties": decimal_string()},
        "ratio": {"type": "object", "additionalProperties": decimal_string()},
        "number": decimal_string(),
        "boolean": {"type": "boolean"},
        "text": {"type": "string"},
        "date": {"type": "string"},
        "time": {"type": "string"},
        "calendar": {"$ref": "#/$defs/CalendarResult"}
    });
    // Endpoint properties are RuleResultValue's fields minus `range`: a range endpoint
    // must never itself be a range (enforced by `RuleResultValue::to_literal`).
    let rule_result_value_endpoint_properties =
        rule_result_value_fields.as_object().unwrap().clone();
    let mut rule_result_value_properties = rule_result_value_endpoint_properties.clone();
    rule_result_value_properties
        .insert("range".to_string(), json!({"$ref": "#/$defs/RangeResult"}));

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
                {"$ref": "#/$defs/LemmaTypeMeasureRange"}
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
        "RuleResultValue": {
            "type": "object",
            "additionalProperties": false,
            "description": "API value shared by RuleResult (flattened), ShowData.fill, and ShowData.suggestion. When present: always `display`, plus exactly one typed field for a non-range value; `range` is set instead for a range value.",
            "properties": rule_result_value_properties
        },
        "ShowData": {
            "type": "object",
            "required": ["type", "needed_by_rules"],
            "additionalProperties": false,
            "properties": {
                "type": {"$ref": "#/$defs/LemmaType"},
                "fill": {"$ref": "#/$defs/RuleResultValue"},
                "suggestion": {"$ref": "#/$defs/RuleResultValue"},
                "needed_by_rules": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Local rule names that transitively need this data after normalize. Empty = reuse catalog only (not an eval intake key for this spec)."
                }
            }
        },
        "ShowVersion": {
            "type": "object",
            "additionalProperties": false,
            "description": "Half-open [effective_from, effective_to) for one loaded temporal row.",
            "properties": {
                "effective_from": nullable_string(),
                "effective_to": nullable_string()
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
                    "properties": {
                        "type": {"const": "literal"},
                        "display": {"type": "string"},
                        "measure": {"type": "object", "additionalProperties": {"type": "string"}},
                        "ratio": {"type": "object", "additionalProperties": {"type": "string"}},
                        "number": {"type": "string"},
                        "boolean": {"type": "boolean"},
                        "text": {"type": "string"},
                        "date": {"type": "string"},
                        "time": {"type": "string"},
                        "calendar": {"$ref": "#/$defs/CalendarResult"},
                        "range": {"$ref": "#/$defs/RangeResult"}
                    }
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
                    "required": ["type", "left", "right"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "and"},
                        "left": {"$ref": "#/$defs/ShowExpression"},
                        "right": {"$ref": "#/$defs/ShowExpression"}
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
                    "required": ["type", "op", "left", "right"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "arithmetic"},
                        "op": {"type": "string", "enum": ["add", "subtract", "multiply", "divide", "modulo", "power"]},
                        "left": {"$ref": "#/$defs/ShowExpression"},
                        "right": {"$ref": "#/$defs/ShowExpression"}
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
                    "required": ["type", "operand", "target"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "unit_conversion"},
                        "operand": {"$ref": "#/$defs/ShowExpression"},
                        "target": {"$ref": "#/$defs/ShowConversionTarget"}
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
                    "required": ["type", "operand"],
                    "additionalProperties": false,
                    "properties": {
                        "type": {"const": "is_veto"},
                        "operand": {"$ref": "#/$defs/ShowExpression"}
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
            "required": ["type", "branches", "depends_on_rules"],
            "additionalProperties": false,
            "description": "Local rule on Show: result type, authored default/unless branches, stored planning depends_on_rules.",
            "properties": {
                "type": {"$ref": "#/$defs/LemmaType"},
                "branches": {"type": "array", "items": {"$ref": "#/$defs/ShowBranch"}},
                "depends_on_rules": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Local rule names this rule depends on (planning topo). Always present."
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
            "type": "object",
            "required": ["spec", "start_line", "data", "rules", "meta"],
            "additionalProperties": false,
            "description": "Result of Engine::show: declared promptable data catalog (needed_by_rules empty = reuse-only), local rule graph (ShowRule with type, branches, depends_on_rules), and resolved temporal window.",
            "properties": {
                "spec": {"type": "string"},
                "commentary": nullable_string(),
                "effective_from": nullable_string(),
                "effective_to": nullable_string(),
                "versions": {"type": "array", "items": {"$ref": "#/$defs/ShowVersion"}},
                "start_line": {"type": "integer"},
                "source_type": {"$ref": "#/$defs/SourceType"},
                "data": {"type": "object", "additionalProperties": {"$ref": "#/$defs/ShowData"}},
                "rules": {"type": "object", "additionalProperties": {"$ref": "#/$defs/ShowRule"}},
                "meta": {"type": "object", "additionalProperties": {"$ref": "#/$defs/LiteralValue"}}
            }
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
            "description": "Result of Engine::run.",
            "properties": {
                "spec": {"type": "string"},
                "effective": {"type": "string"},
                "spec_effective_from": {"type": "string"},
                "spec_effective_to": {"type": "string"},
                "results": {"type": "object", "additionalProperties": {"$ref": "#/$defs/RuleResult"}}
            }
        },
        "ListedSpec": {
            "type": "object",
            "required": ["name"],
            "additionalProperties": false,
            "properties": {
                "name": {"type": "string"},
                "effective_from": {"type": "string"},
                "effective_to": {"type": "string"}
            }
        },
        "ResolvedRepository": {
            "type": "object",
            "required": ["specs"],
            "additionalProperties": false,
            "description": "One repository group from Engine::list. `repository` is absent for the default unnamed repository.",
            "properties": {
                "repository": {"type": "string"},
                "specs": {"type": "array", "items": {"$ref": "#/$defs/ListedSpec"}}
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
            "description": "Structured error thrown by Engine::run/show/load/fetch.",
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
                "role": {"enum": ["outcome", "rule", "source"]},
                "text": {"type": "string"}
            }
        },
        "ExplanationNode": {
            "oneOf": [
                {"$ref": "#/$defs/RuleNode"},
                {"$ref": "#/$defs/ComposeNode"},
                {"$ref": "#/$defs/DataNode"},
                {"$ref": "#/$defs/DataUnusedNode"},
                {"$ref": "#/$defs/ConversionNode"},
                {"$ref": "#/$defs/VetoNode"}
            ]
        },
        "RuleNode": {
            "type": "object",
            "required": ["type", "name", "result", "body"],
            "additionalProperties": false,
            "properties": {
                "type": {"const": "rule"},
                "name": {"type": "string"},
                "result": {"type": "string", "description": "Display string for this rule's result."},
                "body": {"type": "string"},
                "causes": {"type": "array", "items": {"$ref": "#/$defs/Cause"}},
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
                "operands": {"type": "array", "items": {"$ref": "#/$defs/ExplanationNode"}}
            }
        },
        "DataNode": {
            "type": "object",
            "required": ["type", "name", "display"],
            "additionalProperties": false,
            "properties": {
                "type": {"const": "data"},
                "name": {"type": "string"},
                "display": {"type": "string"}
            }
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
        "$id": "https://lemma.dev/schemas/api.v1.json",
        "title": "Lemma engine API",
        "description": "Schema for the JSON document shapes the engine produces at its consumer boundary: Show (Engine::show), Response (Engine::run), list (Engine::list, array of ResolvedRepository), errors (array of EngineError), and RepositoryInstallResult (SDK install). Explanation trees live under RuleResult.explanation (RuleNode / ExplanationNode).",
        "$defs": defs,
        "anyOf": [
            {"$ref": "#/$defs/Show"},
            {"$ref": "#/$defs/Response"},
            {"type": "array", "items": {"$ref": "#/$defs/ResolvedRepository"}},
            {"type": "array", "items": {"$ref": "#/$defs/EngineError"}},
            {"$ref": "#/$defs/RepositoryInstallResult"}
        ]
    })
}

fn schema_path(root: &Path) -> PathBuf {
    root.join(SCHEMA_REL_PATH)
}

pub fn run(root: &Path) -> Result<(), String> {
    let schema = api_v1_schema();
    let mut text = serde_json::to_string_pretty(&schema).map_err(|e| e.to_string())?;
    text.push('\n');
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
    }
}
