//! OpenAPI 3.1 specification generator for the Lemma HTTP surface.
//!
//! Takes a Lemma `Engine` and produces a complete OpenAPI specification as JSON.
//! Used by both `lemma server` (CLI) and LemmaBase.com for consistent API docs.
//!
//! ## Temporal versioning
//!
//! Specs can have multiple temporal versions (e.g. `spec pricing 2024-01-01`
//! and `spec pricing 2025-01-01`) with potentially different interfaces (data, rules,
//! types). The OpenAPI document reflects the interface active at a specific point in
//! time. Use [`generate_openapi_effective`] with an explicit `DateTimeValue` to get the
//! document for a given instant. [`generate_openapi`] is a convenience wrapper that uses
//! the current time.
//!
//! For Scalar multi-source rendering, [`temporal_api_sources`] returns the list of
//! temporal version boundaries so the Scalar UI can offer a source selector.

use lemma::{DateTimeValue, Engine, SpecVersion};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// Query slug for the default temporal view (request-time instant). OpenAPI URLs use no `?effective=`.
pub const NOW_SLUG: &str = "now";

/// The Show document definition, hoisted from `api.v1.json` `$defs/Show`.
pub const SHOW_SCHEMA_REF: &str = "#/components/schemas/Show";

/// A single Scalar API reference source entry.
///
/// Each temporal version boundary gets its own source so Scalar renders a
/// version switcher in the UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ApiSource {
    pub title: String,
    pub slug: String,
    pub url: String,
}

/// Compute the list of Scalar multi-source entries for temporal versioning.
///
/// Returns one [`ApiSource`] per distinct temporal version boundary across all
/// loaded specs, plus one **now** source (slug [`NOW_SLUG`]) that uses no `effective`
/// query (evaluation instant = request time). That entry is first (Scalar default),
/// then boundaries in descending chronological order (newest first).
///
/// If there are no temporal version boundaries (all specs are unversioned),
/// returns a single **now** entry.
pub fn temporal_api_sources(engine: &Engine) -> Vec<ApiSource> {
    let mut all_boundaries: std::collections::BTreeSet<DateTimeValue> =
        std::collections::BTreeSet::new();

    for repo in engine.specs() {
        for version in &repo.specs {
            if let Some(af) = &version.effective_from {
                all_boundaries.insert(af.clone());
            }
        }
    }

    if all_boundaries.is_empty() {
        return vec![ApiSource {
            title: "Now".to_string(),
            slug: NOW_SLUG.to_string(),
            url: "/openapi.json".to_string(),
        }];
    }

    let mut sources: Vec<ApiSource> = Vec::with_capacity(all_boundaries.len() + 1);

    sources.push(ApiSource {
        title: "Now".to_string(),
        slug: NOW_SLUG.to_string(),
        url: "/openapi.json".to_string(),
    });

    for boundary in all_boundaries.iter().rev() {
        let label = boundary.to_string();
        sources.push(ApiSource {
            title: format!("Effective {}", label),
            slug: label.clone(),
            url: format!("/openapi.json?effective={}", label),
        });
    }

    sources
}

/// Generate a complete OpenAPI 3.1 specification using the current time.
///
/// Convenience wrapper around [`generate_openapi_effective`]. The document reflects
/// only the specs and interfaces active at `DateTimeValue::now()`.
pub fn generate_openapi(engine: &Engine, explain: bool) -> Value {
    generate_openapi_effective(engine, explain, &DateTimeValue::now())
}

/// Generate a complete OpenAPI 3.1 specification for a specific point in time.
///
/// The specification includes:
/// - `GET /` — loaded specs (`Engine::specs`)
/// - `/{spec}` GET (show: `Show` from `api.v1.json`) and POST (evaluate: `Response` /
///   `RuleResult` from `api.v1.json`) with optional `Accept-Datetime` header
/// - `?rules=` on POST only, to limit evaluated rules
/// - `x-effective-from` / `x-effective-to` vendor extensions on each PathItem
///   exposing the half-open `[effective_from, effective_to)` range of the version
///   resolved at the document's effective instant (both `null` when unbounded)
///
/// CLI `lemma server` also exposes shell routes (`/openapi.json`, `/health`, `/docs`) that are
/// intentionally omitted from the generated document.
///
/// When `explain` is true, the document adds the `x-explain` header parameter
/// to evaluation operations and describes the optional `explanation` field on rule results.
pub fn generate_openapi_effective(
    engine: &Engine,
    explain: bool,
    effective: &DateTimeValue,
) -> Value {
    let mut paths = Map::new();
    let mut components_schemas = Map::new();

    components_schemas.extend(hoisted_show_component_schemas().clone());
    let mut row_schemas: std::collections::BTreeMap<String, Value> =
        std::collections::BTreeMap::new();

    let repositories = engine.specs();
    let workspace = repositories
        .iter()
        .find(|r| r.repository.is_none())
        .map(|r| r.specs.as_slice())
        .expect("BUG: default repository must exist in specs()");

    let is_active = |version: &SpecVersion| -> bool {
        let after_start = match &version.effective_from {
            None => true,
            Some(from) => effective >= from,
        };
        let before_end = match &version.effective_to {
            None => true,
            Some(to) => effective < to,
        };
        after_start && before_end
    };

    let active_specs: Vec<&SpecVersion> = workspace
        .iter()
        .filter(|version| is_active(version))
        .collect();

    let unique_spec_names: std::collections::BTreeSet<&str> = active_specs
        .iter()
        .map(|version| version.name.as_str())
        .collect();
    let unique_spec_names: Vec<String> = unique_spec_names.into_iter().map(String::from).collect();

    paths.insert("/".to_string(), index_path_item(engine));

    for version in &active_specs {
        let spec_name = &version.name;
        if let Ok(show) = engine.show(None, spec_name, Some(effective)) {
            let artifacts = build_spec_openapi_artifacts(
                spec_name,
                &show,
                &lemma::api::show_document(engine, &show),
                (
                    version.effective_from.as_ref(),
                    version.effective_to.as_ref(),
                ),
                explain,
            );
            paths.insert(format!("/{spec_name}"), artifacts.path_item);
            for (name, schema_value) in artifacts.component_schemas {
                components_schemas.insert(name, schema_value);
            }
            for (name, schema_value) in artifacts.row_schemas {
                match row_schemas.get(&name) {
                    Some(existing) => assert_eq!(
                        existing, &schema_value,
                        "BUG: row schema '{name}' names one row spec, so every Show document renders it identically"
                    ),
                    None => {
                        row_schemas.insert(name, schema_value);
                    }
                }
            }
        }
    }
    components_schemas.extend(row_schemas);

    let mut tags = vec![json!({
        "name": "Specs",
        "description": "Loaded specs"
    })];
    for spec_name in &unique_spec_names {
        let safe_tag = spec_name.replace('.', "_");
        tags.push(json!({
            "name": safe_tag,
            "x-displayName": spec_name,
            "description": format!("Show and evaluate `{spec_name}`")
        }));
    }

    let spec_tags: Vec<Value> = unique_spec_names
        .iter()
        .map(|n| Value::String(n.replace('.', "_")))
        .collect();

    let tag_groups = vec![
        json!({ "name": "Overview", "tags": ["Specs"] }),
        json!({ "name": "Specs", "tags": spec_tags }),
    ];

    let version_label = format!("{} (effective {})", env!("CARGO_PKG_VERSION"), effective);

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Lemma API",
            "description": "Lemma is a declarative language for business logic: pricing, tax, eligibility, contracts, and policies. This API returns, shows, and evaluates loaded specs. Learn more at [LemmaBase.com](https://lemmabase.com).",
            "version": version_label
        },
        "tags": tags,
        "x-tagGroups": tag_groups,
        "paths": Value::Object(paths),
        "components": {
            "schemas": Value::Object(components_schemas)
        }
    })
}

// ---------------------------------------------------------------------------
// Index path (loaded specs)
// ---------------------------------------------------------------------------

fn index_path_item(engine: &Engine) -> Value {
    let repositories = engine.specs();
    let example = serde_json::to_value(&repositories).expect("BUG: Engine::specs must serialize");

    json!({
        "get": {
            "operationId": "specs",
            "summary": "Loaded specs",
            "tags": ["Specs"],
            "responses": {
                "200": {
                    "description": "Loaded repositories and their specs",
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "repository": {
                                            "type": ["string", "null"],
                                            "description": "Repository name. Null for the default repository."
                                        },
                                        "specs": {
                                            "type": "array",
                                            "description": "Specs in this repository",
                                            "items": {
                                                "type": "object",
                                                "properties": {
                                                    "name": {
                                                        "type": "string",
                                                        "description": "Spec name"
                                                    },
                                                    "effective_from": {
                                                        "type": ["string", "null"],
                                                        "description": "When this version starts. Null if unbounded."
                                                    },
                                                    "effective_to": {
                                                        "type": ["string", "null"],
                                                        "description": "When this version ends (exclusive). Null if this is the latest."
                                                    }
                                                },
                                                "required": ["name"]
                                            }
                                        }
                                    },
                                    "required": ["specs"]
                                }
                            },
                            "example": example
                        }
                    }
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Shared response schemas
// ---------------------------------------------------------------------------

fn error_response_schema() -> Value {
    json!({
        "description": "Bad request",
        "content": {
            "application/json": {
                "schema": {
                    "type": "object",
                    "properties": {
                        "error": { "type": "string" }
                    },
                    "required": ["error"]
                }
            }
        }
    })
}

fn not_found_response_schema() -> Value {
    json!({
        "description": "Spec not found",
        "content": {
            "application/json": {
                "schema": {
                    "type": "object",
                    "properties": {
                        "error": { "type": "string" }
                    },
                    "required": ["error"]
                }
            }
        }
    })
}

fn memento_spec_response_headers() -> Value {
    json!({
        "Memento-Datetime": {
            "description": "`effective_from` of the resolved version, when versioned",
            "schema": { "type": "string" }
        },
        "Vary": {
            "description": "Varies with Accept-Datetime",
            "schema": { "type": "string", "example": "Accept-Datetime" }
        }
    })
}

const API_V1_JSON: &str = include_str!("../../engine/schemas/api.v1.json");
const JSON_SCHEMA_METASCHEMA: &str = "https://json-schema.org/draft/2020-12/schema";
const DEFS_REF_PREFIX: &str = "#/$defs/";

/// `$defs/Show` and evaluate `RuleResult` / `RuleResultValue` / row-spec defs, plus
/// each local `$ref` closure, rewritten to `#/components/schemas/...`.
/// Drops the JSON Schema metaschema `allOf` `$ref` so OpenAPI UIs do not fetch it.
fn hoisted_show_component_schemas() -> &'static Map<String, Value> {
    static SCHEMAS: OnceLock<Map<String, Value>> = OnceLock::new();
    SCHEMAS.get_or_init(build_hoisted_show_component_schemas)
}

const EVALUATE_DEF_SEEDS: &[&str] = &[
    "RuleResult",
    "RuleResultValue",
    "SpecResult",
    "SpecRuleResult",
    "RowReference",
    "RowResult",
    "LemmaType",
];

fn build_hoisted_show_component_schemas() -> Map<String, Value> {
    let document: Value =
        serde_json::from_str(API_V1_JSON).expect("BUG: engine/schemas/api.v1.json must be JSON");
    let defs = document["$defs"]
        .as_object()
        .expect("BUG: api.v1.json must have $defs");
    let show = show_object_without_metaschema(defs);
    let mut needed: BTreeSet<String> = BTreeSet::new();
    collect_defs_ref_names(&show, &mut needed);
    for seed in EVALUATE_DEF_SEEDS {
        needed.insert((*seed).to_string());
    }
    let mut pending: Vec<String> = needed.iter().cloned().collect();
    while let Some(name) = pending.pop() {
        let def = defs
            .get(&name)
            .unwrap_or_else(|| panic!("BUG: api.v1.json $defs missing {name}"));
        let mut more = BTreeSet::new();
        collect_defs_ref_names(def, &mut more);
        for extra in more {
            if needed.insert(extra.clone()) {
                pending.push(extra);
            }
        }
    }
    let mut schemas = Map::new();
    let mut show = show;
    rewrite_defs_refs(&mut show);
    schemas.insert("Show".to_string(), show);
    for name in needed {
        let mut schema = defs
            .get(&name)
            .unwrap_or_else(|| panic!("BUG: api.v1.json $defs missing {name}"))
            .clone();
        rewrite_defs_refs(&mut schema);
        schemas.insert(name, schema);
    }
    schemas
}

fn show_object_without_metaschema(defs: &Map<String, Value>) -> Value {
    let show = defs
        .get("Show")
        .expect("BUG: api.v1.json $defs must define Show");
    let all_of = show["allOf"]
        .as_array()
        .expect("BUG: $defs/Show must be allOf[metaschema, object]");
    assert_eq!(
        all_of.len(),
        2,
        "BUG: $defs/Show allOf must have metaschema $ref then object"
    );
    assert_eq!(
        all_of[0]["$ref"].as_str(),
        Some(JSON_SCHEMA_METASCHEMA),
        "BUG: $defs/Show allOf[0] must $ref the JSON Schema 2020-12 metaschema"
    );
    let mut object = all_of[1].clone();
    if let Some(description) = show.get("description") {
        object["description"] = description.clone();
    }
    object
}

fn collect_defs_ref_names(value: &Value, names: &mut BTreeSet<String>) {
    let mut pending: Vec<&Value> = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(map) => {
                if let Some(Value::String(target)) = map.get("$ref") {
                    if let Some(name) = target.strip_prefix(DEFS_REF_PREFIX) {
                        assert!(
                            !name.contains('/'),
                            "BUG: nested JSON pointer $ref {target} is not a $defs name"
                        );
                        names.insert(name.to_string());
                    } else {
                        assert_eq!(
                            target, JSON_SCHEMA_METASCHEMA,
                            "BUG: unexpected $ref {target} in api.v1.json Show closure"
                        );
                    }
                }
                pending.extend(map.values());
            }
            Value::Array(items) => pending.extend(items.iter()),
            _ => {}
        }
    }
}

/// GET `/{spec}` body: the Show document, a JSON Schema for the spec's evaluation-data inputs.
fn build_get_show_response() -> Value {
    json!({
        "$ref": SHOW_SCHEMA_REF,
        "description": "Draft 2020-12 JSON Schema for this spec's evaluation-data inputs (properties, required, $defs) with the rule graph under x-lemma-rules."
    })
}

/// POST evaluate body: matches engine [`lemma::Response`] JSON shape.
fn build_evaluate_response_schema(show: &lemma::Show, rule_names: &[String]) -> Value {
    let mut result_props = Map::new();
    for rule_name in rule_names {
        if show.rules.contains_key(rule_name) {
            result_props.insert(
                rule_name.clone(),
                json!({
                    "$ref": "#/components/schemas/RuleResult"
                }),
            );
        }
    }

    json!({
        "type": "object",
        "required": ["spec", "effective", "results"],
        "properties": {
            "spec": {
                "type": "string",
                "description": "Spec that was evaluated"
            },
            "effective": {
                "type": "string",
                "description": "Datetime used to pick the version and evaluate"
            },
            "spec_effective_from": {
                "type": "string",
                "description": "When the resolved version starts"
            },
            "spec_effective_to": {
                "type": "string",
                "description": "When the resolved version ends (exclusive). Absent if unbounded."
            },
            "results": {
                "type": "object",
                "description": "Results by rule name. Honors `?rules=`.",
                "properties": Value::Object(result_props)
            },
            "rows": {
                "type": "array",
                "description": "List rows the results reach, in first-reached order. Omitted when none.",
                "items": { "$ref": "#/components/schemas/RowResult" }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Spec path items
// ---------------------------------------------------------------------------

struct SpecOpenApiArtifacts {
    path_item: Value,
    component_schemas: Map<String, Value>,
    /// The Show document's `$defs`, refs rewritten to `#/components/schemas/`.
    row_schemas: Map<String, Value>,
}

fn spec_component_show_names(spec_name: &str) -> (String, String, String, String) {
    let safe_name = spec_name.replace('.', "_");
    (
        format!("{safe_name}_get_show"),
        format!("{safe_name}_evaluate_response"),
        format!("{safe_name}_request"),
        format!("{safe_name}_form_request"),
    )
}

/// Build the PathItem and per-spec component schemas for `/{spec_name}`.
///
/// `effective_range` is the half-open `[effective_from, effective_to)`
/// validity range of the temporal version resolved at the OpenAPI document's
/// effective instant. Both bounds are emitted as the `x-effective-from` /
/// `x-effective-to` vendor extensions on the PathItem so tooling can render
/// the active version's window. `None` in either position (unbounded start for the first row,
/// unbounded end for the latest row) is serialised as JSON `null`.
fn build_spec_openapi_artifacts(
    spec_name: &str,
    show: &lemma::Show,
    document: &Value,
    effective_range: (Option<&DateTimeValue>, Option<&DateTimeValue>),
    explain: bool,
) -> SpecOpenApiArtifacts {
    let rule_names: Vec<String> = show
        .rules
        .iter()
        .filter(|(_, rule)| rule.path.is_empty())
        .map(|(name, _)| name.clone())
        .collect();
    let (
        get_show_component_name,
        evaluate_response_schema_name,
        post_body_schema_name,
        post_form_body_schema_name,
    ) = spec_component_show_names(spec_name);

    let mut component_schemas = Map::new();
    component_schemas.insert(get_show_component_name.clone(), build_get_show_response());
    component_schemas.insert(
        evaluate_response_schema_name.clone(),
        build_evaluate_response_schema(show, &rule_names),
    );
    component_schemas.insert(
        post_body_schema_name.clone(),
        build_post_request_schema(document),
    );
    component_schemas.insert(
        post_form_body_schema_name.clone(),
        build_post_form_request_schema(document),
    );
    let mut row_schemas = Map::new();
    if let Some(defs) = document.get("$defs") {
        for (name, schema) in defs.as_object().expect("BUG: Show $defs is an object") {
            let mut schema = schema.clone();
            rewrite_defs_refs(&mut schema);
            row_schemas.insert(name.clone(), schema);
        }
    }

    let path_item = build_spec_path_item_with_show_refs(
        spec_name,
        (
            &get_show_component_name,
            &evaluate_response_schema_name,
            &post_body_schema_name,
            &post_form_body_schema_name,
        ),
        &rule_names,
        explain,
        effective_range,
    );

    SpecOpenApiArtifacts {
        path_item,
        component_schemas,
        row_schemas,
    }
}

fn x_explain_header_parameter() -> Value {
    json!({
        "name": "x-explain",
        "in": "header",
        "required": false,
        "description": "Include explanations. Server needs `--explain`.",
        "schema": { "type": "string", "default": "true" }
    })
}

fn accept_datetime_header_parameter() -> Value {
    json!({
        "name": "Accept-Datetime",
        "in": "header",
        "required": false,
        "description": "Evaluate at this datetime. Omit for now.",
        "schema": { "type": "string", "format": "date-time" },
        "example": "Sat, 01 Jan 2025 00:00:00 GMT"
    })
}

/// Build the PathItem for `/{spec_name}` (GET show + POST evaluate).
fn build_spec_path_item_with_show_refs(
    spec_name: &str,
    component_names: (&str, &str, &str, &str),
    rule_names: &[String],
    explain: bool,
    effective_range: (Option<&DateTimeValue>, Option<&DateTimeValue>),
) -> Value {
    let (
        get_show_component_name,
        evaluate_response_schema_name,
        post_body_schema_name,
        post_form_body_schema_name,
    ) = component_names;
    let (effective_from, effective_to) = effective_range;

    let get_show_ref = json!({
        "$ref": format!("#/components/schemas/{}", get_show_component_name)
    });
    let evaluate_schema_ref = json!({
        "$ref": format!("#/components/schemas/{}", evaluate_response_schema_name)
    });
    let body_ref = json!({
        "$ref": format!("#/components/schemas/{}", post_body_schema_name)
    });
    let form_body_ref = json!({
        "$ref": format!("#/components/schemas/{}", post_form_body_schema_name)
    });

    let tag = spec_name.replace('.', "_");

    let rules_example = if rule_names.is_empty() {
        String::new()
    } else {
        rule_names.join(",")
    };

    let rules_param = json!({
        "name": "rules",
        "in": "query",
        "required": false,
        "description": "Rules to evaluate, comma-separated. Omit for all.",
        "schema": { "type": "string" },
        "example": rules_example
    });

    let mut get_parameters: Vec<Value> = Vec::new();
    get_parameters.push(accept_datetime_header_parameter());
    if explain {
        get_parameters.push(x_explain_header_parameter());
    }

    let get_operation_id = format!("get_{}", spec_name);
    let post_operation_id = format!("post_{}", spec_name);

    let mut post_parameters: Vec<Value> = vec![rules_param];
    post_parameters.push(accept_datetime_header_parameter());
    if explain {
        post_parameters.push(x_explain_header_parameter());
    }

    let datetime_or_null = |dt: Option<&DateTimeValue>| -> Value {
        match dt {
            Some(d) => Value::String(d.to_string()),
            None => Value::Null,
        }
    };

    json!({
        "x-effective-from": datetime_or_null(effective_from),
        "x-effective-to": datetime_or_null(effective_to),
        "get": {
            "operationId": get_operation_id,
            "summary": "Show spec",
            "tags": [tag],
            "parameters": get_parameters,
            "responses": {
                "200": {
                    "description": "Spec interface",
                    "headers": memento_spec_response_headers(),
                    "content": {
                        "application/json": {
                            "schema": get_show_ref
                        }
                    }
                },
                "400": error_response_schema(),
                "404": not_found_response_schema()
            }
        },
        "post": {
            "operationId": post_operation_id,
            "summary": "Evaluate",
            "tags": [tag],
            "parameters": post_parameters,
            "requestBody": {
                "required": true,
                "content": {
                    // First media type is Scalar's default body encoding.
                    "application/x-www-form-urlencoded": {
                        "schema": form_body_ref
                    },
                    "application/json": {
                        "schema": body_ref
                    }
                }
            },
            "responses": {
                "200": {
                    "description": "Evaluation results",
                    "headers": memento_spec_response_headers(),
                    "content": {
                        "application/json": {
                            "schema": evaluate_schema_ref
                        }
                    }
                },
                "400": error_response_schema(),
                "404": not_found_response_schema()
            }
        }
    })
}

// ---------------------------------------------------------------------------
// POST request bodies, derived from the Show document
// ---------------------------------------------------------------------------

/// JSON body: the Show document's `properties` and `required`, without the spec-level
/// annotations. Row `$ref`s point at the hoisted `LemmaRow_*` components.
fn build_post_request_schema(document: &Value) -> Value {
    let mut schema = json!({
        "type": "object",
        "additionalProperties": true,
        "properties": document["properties"].clone()
    });
    if let Some(required) = document.get("required") {
        schema["required"] = required.clone();
    }
    rewrite_defs_refs(&mut schema);
    schema
}

fn rewrite_defs_refs(value: &mut Value) {
    let mut pending: Vec<&mut Value> = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Object(map) => {
                if let Some(Value::String(target)) = map.get_mut("$ref") {
                    if let Some(name) = target.strip_prefix("#/$defs/") {
                        *target = format!("#/components/schemas/{name}");
                    }
                }
                pending.extend(map.values_mut());
            }
            Value::Array(items) => pending.extend(items.iter_mut()),
            _ => {}
        }
    }
}

/// Form keywords copied from a Show property: a form field is one literal string.
const FORM_KEYWORDS: [&str; 5] = ["pattern", "enum", "minLength", "maxLength", "description"];

/// Lists are JSON-only: a form field is one literal, and a boolean is the string
/// `true`/`false`.
fn build_post_form_request_schema(document: &Value) -> Value {
    let mut properties = Map::new();
    for (name, property) in document["properties"]
        .as_object()
        .expect("BUG: Show properties is an object")
    {
        let mut field = Map::new();
        field.insert("type".to_string(), json!("string"));
        let scalar = property
            .get("oneOf")
            .map_or(property, |alternatives| &alternatives[0]);
        match scalar["type"].as_str() {
            Some("array") => continue,
            Some("boolean") => {
                field.insert("enum".to_string(), json!(["true", "false"]));
            }
            Some("string") => {}
            other => unreachable!("BUG: Show property '{name}' has type {other:?}"),
        }
        for keyword in FORM_KEYWORDS {
            if let Some(value) = scalar.get(keyword).or_else(|| property.get(keyword)) {
                field.insert(keyword.to_string(), value.clone());
            }
        }
        if let Some(default) = property.get("default") {
            field.insert("default".to_string(), form_literal(default));
        }
        if let Some(examples) = property.get("examples") {
            let examples = examples
                .as_array()
                .expect("BUG: Show examples is an array")
                .iter()
                .map(form_literal)
                .collect();
            field.insert("examples".to_string(), Value::Array(examples));
        }
        properties.insert(name.clone(), Value::Object(field));
    }
    let mut schema = json!({
        "type": "object",
        "properties": Value::Object(properties)
    });
    if let Some(required) = document.get("required") {
        schema["required"] = required.clone();
    }
    schema
}

fn form_literal(value: &Value) -> Value {
    match value {
        Value::String(_) => value.clone(),
        Value::Bool(boolean) => Value::String(boolean.to_string()),
        other => unreachable!("BUG: scalar Show literal is a string or boolean, got {other}"),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use lemma::{DateGranularity, DateTimeValue, SourceType};

    fn create_engine_with_code(code: &str) -> Engine {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("test.lemma"))),
                code.to_string(),
            )])
            .expect("failed to parse lemma code");
        engine
    }

    fn create_engine_with_files(files: Vec<(&str, &str)>) -> Engine {
        let mut engine = Engine::new();
        for (name, code) in files {
            engine
                .load([(
                    SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(name))),
                    code.to_string(),
                )])
                .expect("failed to parse lemma code");
        }
        engine
    }

    fn date(year: i32, month: u32, day: u32) -> DateTimeValue {
        DateTimeValue {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,
            granularity: DateGranularity::Full,
        }
    }

    fn has_param(params: &Value, name: &str) -> bool {
        params
            .as_array()
            .map(|a| a.iter().any(|p| p["name"] == name))
            .unwrap_or(false)
    }

    // =======================================================================
    // Basic spec structure (pre-existing, adapted)
    // =======================================================================

    #[test]
    fn test_generate_openapi_x_tag_groups() {
        let engine = create_engine_with_code(
            "spec pricing
            data quantity: 10
            rule total: quantity * 2",
        );
        let spec = generate_openapi(&engine, false);

        let groups = spec["x-tagGroups"]
            .as_array()
            .expect("x-tagGroups should be array");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0]["name"], "Overview");
        assert_eq!(groups[0]["tags"], json!(["Specs"]));
        assert_eq!(groups[1]["name"], "Specs");
        assert_eq!(groups[1]["tags"], json!(["pricing"]));
    }

    #[test]
    fn test_spec_path_has_get_and_post() {
        let engine = create_engine_with_code(
            "spec pricing
            data quantity: 10
            rule total: quantity * 2",
        );
        let spec = generate_openapi(&engine, false);

        assert!(
            spec["paths"]["/pricing"].is_object(),
            "single spec path /pricing"
        );
        assert!(spec["paths"]["/pricing"]["get"].is_object());
        assert!(spec["paths"]["/pricing"]["post"].is_object());

        assert_eq!(
            spec["paths"]["/pricing"]["get"]["operationId"],
            "get_pricing"
        );
        assert_eq!(
            spec["paths"]["/pricing"]["post"]["operationId"],
            "post_pricing"
        );
        assert_eq!(spec["paths"]["/pricing"]["get"]["tags"][0], "pricing");

        let get_params = spec["paths"]["/pricing"]["get"]["parameters"]
            .as_array()
            .expect("parameters array");
        let param_names: Vec<&str> = get_params
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert!(
            !param_names.contains(&"rules"),
            "GET must not have rules query param (show is full interface)"
        );
        let post_params = spec["paths"]["/pricing"]["post"]["parameters"]
            .as_array()
            .expect("post parameters array");
        let post_param_names: Vec<&str> = post_params
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert!(
            post_param_names.contains(&"rules"),
            "POST must have rules query param"
        );
        assert!(
            param_names.contains(&"Accept-Datetime"),
            "GET must have Accept-Datetime header"
        );

        let get_ref = spec["paths"]["/pricing"]["get"]["responses"]["200"]["content"]
            ["application/json"]["schema"]["$ref"]
            .as_str()
            .unwrap();
        let post_ref = spec["paths"]["/pricing"]["post"]["responses"]["200"]["content"]
            ["application/json"]["schema"]["$ref"]
            .as_str()
            .unwrap();
        assert_eq!(get_ref, "#/components/schemas/pricing_get_show");
        assert_eq!(post_ref, "#/components/schemas/pricing_evaluate_response");
        assert_ne!(get_ref, post_ref);

        let get_show = &spec["components"]["schemas"]["pricing_get_show"];
        assert_eq!(get_show["$ref"], SHOW_SCHEMA_REF);
        assert_eq!(SHOW_SCHEMA_REF, "#/components/schemas/Show");
        let show = &spec["components"]["schemas"]["Show"];
        assert_eq!(show["type"], "object");
        let mut stack = vec![show];
        while let Some(node) = stack.pop() {
            match node {
                Value::Object(map) => {
                    if let Some(Value::String(target)) = map.get("$ref") {
                        assert!(
                            !target.starts_with("http"),
                            "Show schema must not $ref {target}"
                        );
                    }
                    stack.extend(map.values());
                }
                Value::Array(items) => stack.extend(items),
                _ => {}
            }
        }

        let h200 = &spec["paths"]["/pricing"]["get"]["responses"]["200"];
        assert!(h200["headers"]["Memento-Datetime"].is_object());
        assert!(h200["headers"]["Vary"].is_object());
    }

    #[test]
    fn suggestion_only_field_is_required_with_json_schema_default() {
        let engine = create_engine_with_code(
            r#"
spec age_check
data age: number -> suggest 18
rule adult: age >= 18
"#,
        );
        let spec = generate_openapi(&engine, false);
        let body = &spec["components"]["schemas"]["age_check_request"];
        let required = body["required"]
            .as_array()
            .expect("required array")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(
            required.contains(&"age"),
            "suggest-only fields must stay required, got {required:?}"
        );
        assert!(body["properties"]["age"].get("default").is_none());
        assert_eq!(body["properties"]["age"]["examples"], json!(["18"]));
    }

    /// The generated OpenAPI document describes the public spec surface only.
    /// Server shell routes (`/openapi.json`, `/health`, `/docs`) are
    /// intentionally omitted; consumers must not rely on them for code
    /// generation or contract inspection.
    #[test]
    fn test_openapi_omits_shell_and_unlisted_schema_routes() {
        let engine = create_engine_with_code(
            "spec pricing
            data quantity: 10
            rule total: quantity * 2",
        );
        let spec = generate_openapi(&engine, false);

        let paths = spec["paths"].as_object().expect("paths object");
        assert!(paths.contains_key("/"));
        assert_eq!(paths["/"]["get"]["operationId"], "specs");
        assert!(!paths.contains_key("/openapi.json"));
        assert!(!paths.contains_key("/health"));
        assert!(!paths.contains_key("/docs"));
        assert!(!paths.contains_key("/schema/pricing"));
        assert!(!paths.contains_key("/schema/pricing/{rules}"));
        assert!(!paths.keys().any(|key| key.starts_with("/schema/")));
    }

    #[test]
    fn test_generate_openapi_explain_adds_x_explain_and_explanation_schema() {
        let engine = create_engine_with_code(
            "spec pricing
            data quantity: 10
            rule total: quantity * 2",
        );
        let spec = generate_openapi(&engine, true);

        let get_params = &spec["paths"]["/pricing"]["get"]["parameters"];
        assert!(has_param(get_params, "x-explain"));

        let rule_result = &spec["components"]["schemas"]["RuleResult"];
        assert_eq!(
            rule_result["properties"]["explanation"]["$ref"],
            "#/components/schemas/RuleNode"
        );
        assert!(rule_result["properties"]["vetoed"]["type"] == "boolean");
        assert!(rule_result["properties"]["rule_type"]["type"] == "string");

        let evaluate = &spec["components"]["schemas"]["pricing_evaluate_response"];
        assert!(evaluate["required"]
            .as_array()
            .unwrap()
            .contains(&json!("spec")));
        assert!(evaluate["required"]
            .as_array()
            .unwrap()
            .contains(&json!("effective")));
        assert!(evaluate["required"]
            .as_array()
            .unwrap()
            .contains(&json!("results")));
        let total_ref = evaluate["properties"]["results"]["properties"]["total"]["$ref"]
            .as_str()
            .unwrap();
        assert_eq!(total_ref, "#/components/schemas/RuleResult");
    }

    #[test]
    fn test_generate_openapi_multiple_specs() {
        let engine = create_engine_with_files(vec![
            (
                "pricing.lemma",
                "spec pricing
                data quantity: 10
                rule total: quantity * 2",
            ),
            (
                "shipping.lemma",
                "spec shipping
                data weight: 5
                rule cost: weight * 3",
            ),
        ]);
        let spec = generate_openapi(&engine, false);

        assert!(spec["paths"]["/pricing"].is_object());
        assert!(spec["paths"]["/shipping"].is_object());
    }

    #[test]
    fn test_nested_spec_path_schema_refs_are_valid() {
        let engine = create_engine_with_code(
            "spec bc
        data x: number
        rule result: x",
        );
        let spec = generate_openapi(&engine, false);

        assert!(spec["paths"]["/bc"]["post"].is_object());
        let post_content = &spec["paths"]["/bc"]["post"]["requestBody"]["content"];
        let content_keys: Vec<&str> = post_content
            .as_object()
            .expect("requestBody.content object")
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(
            content_keys.first().copied(),
            Some("application/x-www-form-urlencoded"),
            "form-urlencoded must be first so Scalar docs default to Form URL Encoded"
        );
        let body_ref = post_content["application/json"]["schema"]["$ref"]
            .as_str()
            .unwrap();
        let form_body_ref = post_content["application/x-www-form-urlencoded"]["schema"]["$ref"]
            .as_str()
            .unwrap();
        assert_eq!(body_ref, "#/components/schemas/bc_request");
        assert_eq!(form_body_ref, "#/components/schemas/bc_form_request");
        assert!(spec["components"]["schemas"]["bc_request"].is_object());
        assert!(spec["components"]["schemas"]["bc_form_request"].is_object());
        assert!(spec["components"]["schemas"]["bc_request"]["properties"]["x"].is_object());
        assert!(spec["components"]["schemas"]["bc_form_request"]["properties"]["x"].is_object());
    }

    // =======================================================================
    // generate_openapi_effective with explicit timestamp
    // =======================================================================

    #[test]
    fn test_generate_openapi_effective_reflects_specific_time() {
        let engine = create_engine_with_code(
            "spec pricing
            data quantity: 10
            rule total: quantity * 2",
        );
        let effective = date(2025, 6, 15);
        let spec = generate_openapi_effective(&engine, false, &effective);

        assert_eq!(spec["openapi"], "3.1.0");
        let version = spec["info"]["version"].as_str().unwrap();
        assert!(
            version.contains("2025-06-15"),
            "version string should contain the effective date, got: {}",
            version
        );
    }

    #[test]
    fn test_effective_shows_correct_temporal_version_interface() {
        let engine = create_engine_with_files(vec![(
            "policy.lemma",
            r#"
spec policy
data base: 100
rule discount: 10

spec policy 2025-06-01
data base: 200
data premium: boolean
rule discount: 20
rule surcharge:
  5
  unless premium then 10
"#,
        )]);

        let before = date(2025, 3, 1);
        let spec_v1 = generate_openapi_effective(&engine, false, &before);

        assert!(spec_v1["paths"]["/policy"].is_object());
        let v1_evaluate = &spec_v1["components"]["schemas"]["policy_evaluate_response"];
        let v1_result = &v1_evaluate["properties"]["results"]["properties"];
        assert_eq!(
            v1_result["discount"]["$ref"].as_str(),
            Some("#/components/schemas/RuleResult"),
            "v1 should have discount rule"
        );
        assert!(
            v1_result["surcharge"].is_null(),
            "v1 must NOT have surcharge rule"
        );
        let v1_request = &spec_v1["components"]["schemas"]["policy_request"];
        assert!(
            v1_request["properties"]["premium"].is_null(),
            "v1 must NOT have premium data"
        );

        let after = date(2025, 8, 1);
        let spec_v2 = generate_openapi_effective(&engine, false, &after);

        let v2_evaluate = &spec_v2["components"]["schemas"]["policy_evaluate_response"];
        let v2_result = &v2_evaluate["properties"]["results"]["properties"];
        assert!(
            v2_result["discount"]["$ref"].is_string(),
            "v2 should have discount rule"
        );
        assert!(
            v2_result["surcharge"]["$ref"].is_string(),
            "v2 should have surcharge rule"
        );
        let v2_request = &spec_v2["components"]["schemas"]["policy_request"];
        assert!(
            v2_request["properties"]["premium"].is_object(),
            "v2 should have premium data"
        );
    }

    /// Each spec PathItem carries `x-effective-from` and `x-effective-to`
    /// describing the half-open `[effective_from, effective_to)` validity
    /// range of the version resolved at the document's effective instant.
    ///
    /// - Earlier row: `x-effective-to` = next row's `effective_from`.
    /// - Latest row: `x-effective-to` = `null` (no successor).
    /// - Unversioned spec (no declared `effective_from`): both extensions are
    ///   `null`.
    #[test]
    fn test_spec_path_item_exposes_half_open_effective_range_as_vendor_extensions() {
        let engine = create_engine_with_files(vec![(
            "policy.lemma",
            r#"
spec policy 2025-01-01
data base: 10
rule total: base

spec policy 2026-01-01
data base: 99
rule total: base
"#,
        )]);

        let at_earlier = date(2025, 6, 1);
        let earlier_doc = generate_openapi_effective(&engine, false, &at_earlier);
        let earlier_path = &earlier_doc["paths"]["/policy"];
        assert_eq!(
            earlier_path["x-effective-from"].as_str(),
            Some("2025-01-01"),
            "earlier version effective_from on PathItem"
        );
        assert_eq!(
            earlier_path["x-effective-to"].as_str(),
            Some("2026-01-01"),
            "earlier version effective_to equals next version's effective_from"
        );

        let at_latest = date(2026, 6, 1);
        let latest_doc = generate_openapi_effective(&engine, false, &at_latest);
        let latest_path = &latest_doc["paths"]["/policy"];
        assert_eq!(
            latest_path["x-effective-from"].as_str(),
            Some("2026-01-01"),
            "latest version effective_from on PathItem"
        );
        assert!(
            latest_path["x-effective-to"].is_null(),
            "latest version has no successor; x-effective-to must be null: {latest_path}"
        );
    }

    /// Unversioned specs (no declared `effective_from`) have both extensions
    /// serialised as JSON `null`, not omitted.
    #[test]
    fn test_spec_path_item_effective_extensions_null_for_unversioned_spec() {
        let engine = create_engine_with_code(
            "spec pricing
            data quantity: 10
            rule total: quantity * 2",
        );
        let document = generate_openapi(&engine, false);
        let path_item = &document["paths"]["/pricing"];
        assert!(
            path_item["x-effective-from"].is_null(),
            "unversioned spec: x-effective-from must be null: {path_item}"
        );
        assert!(
            path_item["x-effective-to"].is_null(),
            "unversioned spec: x-effective-to must be null: {path_item}"
        );
    }

    // =======================================================================
    // temporal_api_sources
    // =======================================================================

    #[test]
    fn test_temporal_sources_versioned_returns_boundaries_plus_now() {
        let engine = create_engine_with_files(vec![(
            "policy.lemma",
            r#"
spec policy
data base: 100
rule discount: 10

spec policy 2025-06-01
data base: 200
rule discount: 20
"#,
        )]);

        let sources = temporal_api_sources(&engine);

        assert_eq!(sources.len(), 2, "should have 1 now + 1 boundary");

        assert_eq!(sources[0].title, "Now");
        assert_eq!(sources[0].slug, NOW_SLUG);
        assert_eq!(sources[0].url, "/openapi.json");

        assert_eq!(sources[1].title, "Effective 2025-06-01");
        assert_eq!(sources[1].slug, "2025-06-01");
        assert_eq!(sources[1].url, "/openapi.json?effective=2025-06-01");
    }

    #[test]
    fn test_temporal_sources_multiple_specs_merged_boundaries() {
        let engine = create_engine_with_files(vec![
            (
                "policy.lemma",
                r#"
spec policy
data base: 100
rule discount: 10

spec policy 2025-06-01
data base: 200
rule discount: 20
"#,
            ),
            (
                "rates.lemma",
                r#"
spec rates
data rate: 5
rule total: rate * 2

spec rates 2025-03-01
data rate: 7
rule total: rate * 2

spec rates 2025-06-01
data rate: 9
rule total: rate * 2
"#,
            ),
        ]);

        let sources = temporal_api_sources(&engine);

        let slugs: Vec<&str> = sources.iter().map(|s| s.slug.as_str()).collect();
        assert!(
            slugs.contains(&"2025-03-01"),
            "should contain rates boundary"
        );
        assert!(
            slugs.contains(&"2025-06-01"),
            "should contain shared boundary"
        );
        assert!(slugs.contains(&NOW_SLUG), "should contain now");
        assert_eq!(slugs.len(), 3, "2 unique boundaries + now");
    }

    #[test]
    fn test_temporal_sources_ordered_chronologically() {
        let engine = create_engine_with_files(vec![(
            "policy.lemma",
            r#"
spec policy
data base: 100
rule discount: 10

spec policy 2024-01-01
data base: 50
rule discount: 5

spec policy 2025-06-01
data base: 200
rule discount: 20
"#,
        )]);

        let sources = temporal_api_sources(&engine);
        let slugs: Vec<&str> = sources.iter().map(|s| s.slug.as_str()).collect();
        assert_eq!(slugs, vec![NOW_SLUG, "2025-06-01", "2024-01-01"]);
    }

    // =======================================================================
    // Type-specific parameter tests
    // =======================================================================

    #[test]
    fn test_post_schema_text_with_options_has_enum() {
        let engine = create_engine_with_code(
            "spec test
            data sku: text -> option \"A\" -> option \"B\"
            rule result: sku",
        );
        let spec = generate_openapi(&engine, false);

        let sku_prop = &spec["components"]["schemas"]["test_request"]["properties"]["sku"];
        assert!(sku_prop["enum"].is_array());
        let enums = sku_prop["enum"].as_array().unwrap();
        assert_eq!(enums.len(), 2);
        assert_eq!(enums[0], "A");
        assert_eq!(enums[1], "B");
    }

    #[test]
    fn test_post_schema_boolean_is_json_boolean() {
        let engine = create_engine_with_code(
            "spec test
            data is_active: boolean
            rule result: is_active",
        );
        let spec = generate_openapi(&engine, false);

        let schema = &spec["components"]["schemas"]["test_request"];
        let is_active = &schema["properties"]["is_active"];
        assert_eq!(is_active["oneOf"][0]["type"], "boolean");

        let form_schema = &spec["components"]["schemas"]["test_form_request"];
        let form_is_active = &form_schema["properties"]["is_active"];
        assert_eq!(form_is_active["type"], "string");
        assert_eq!(form_is_active["enum"], json!(["true", "false"]));
    }

    #[test]
    fn test_post_schema_number_is_string() {
        let engine = create_engine_with_code(
            "spec test
            data quantity: number
            rule result: quantity",
        );
        let spec = generate_openapi(&engine, false);

        let schema = &spec["components"]["schemas"]["test_request"];
        assert_eq!(schema["properties"]["quantity"]["type"], "string");
    }

    #[test]
    fn test_data_with_default_is_not_required() {
        let engine = create_engine_with_code(
            "spec test
            data quantity: 10
            data name: text
            rule result: quantity
            rule label: name",
        );
        let spec = generate_openapi(&engine, false);

        let schema = &spec["components"]["schemas"]["test_request"];
        let required = schema["required"]
            .as_array()
            .expect("required should be array");

        assert!(required.contains(&Value::String("name".to_string())));
        assert!(!required.contains(&Value::String("quantity".to_string())));
    }

    #[test]
    fn test_help_and_default_in_openapi() {
        let engine = create_engine_with_code(
            r#"spec test
data quantity: number -> help "Number of items to order" -> suggest 10
data active: boolean -> help "Whether the feature is enabled" -> suggest true
rule result:
  quantity
  unless active then 0
"#,
        );
        let spec = generate_openapi(&engine, false);

        let req_schema = &spec["components"]["schemas"]["test_request"];
        assert!(req_schema["properties"]["quantity"]["description"]
            .as_str()
            .unwrap()
            .contains("Number of items to order"));
        assert_eq!(
            req_schema["properties"]["quantity"]["examples"],
            json!(["10"])
        );
        assert!(req_schema["properties"]["active"]["description"]
            .as_str()
            .unwrap()
            .contains("Whether the feature is enabled"));
        assert_eq!(
            req_schema["properties"]["active"]["examples"],
            json!([true])
        );
        assert_eq!(
            spec["components"]["schemas"]["test_form_request"]["properties"]["active"]["examples"],
            json!(["true"])
        );
        let required = req_schema["required"]
            .as_array()
            .expect("required array")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(required.contains(&"quantity"));
        assert!(required.contains(&"active"));
    }

    const LIST_CODE: &str = r#"spec line
data price: number
data qty: number -> suggest 1
rule cost: price * qty

spec order
uses lines: line list
data discounts: number list
  -> with 5, 10
rule total: lines -> sum cost
rule biggest: lines -> sort cost desc -> first
rule discount_total: discounts -> sum

spec part
data own_cost: number
uses components: part list
rule unit_cost:
  own_cost + (components -> sum components.unit_cost)
  unless components -> count is 0 then own_cost
"#;

    fn row_ref(schema: &Value) -> &str {
        schema["items"]["$ref"]
            .as_str()
            .unwrap_or_else(|| panic!("array of row refs expected: {schema}"))
    }

    #[test]
    fn instance_list_is_array_of_row_objects_and_never_required() {
        let engine = create_engine_with_code(LIST_CODE);
        let document = generate_openapi(&engine, false);
        let schemas = &document["components"]["schemas"];
        let request = &schemas["order_request"];
        let lines = &request["properties"]["lines"];
        assert_eq!(lines["type"], "array");
        assert_eq!(row_ref(lines), "#/components/schemas/LemmaRow_line");
        assert!(
            request.get("required").is_none(),
            "lists are empty when absent, never required: {request}"
        );

        let row = &schemas["LemmaRow_line"];
        assert_eq!(row["type"], "object");
        assert_eq!(row["additionalProperties"], true);
        assert_eq!(row["title"], "line row");
        assert_eq!(row["properties"]["price"]["type"], "string");
        assert_eq!(row["properties"]["qty"]["examples"], json!(["1"]));
        assert_eq!(row["required"], json!(["price", "qty"]));
    }

    #[test]
    fn value_list_is_array_of_literals_with_array_default() {
        let engine = create_engine_with_code(LIST_CODE);
        let document = generate_openapi(&engine, false);
        let discounts =
            &document["components"]["schemas"]["order_request"]["properties"]["discounts"];
        assert_eq!(discounts["type"], "array");
        assert_eq!(
            discounts["items"],
            json!({ "type": "string", "pattern": lemma::api::number_pattern() })
        );
        assert_eq!(discounts["default"], json!(["5", "10"]));
    }

    const SOURCE_ROWS_CODE: &str = r#"spec article
data price: number

spec line
data qty: number
uses article: article
rule cost: qty * article.price

spec order
uses lines: line list
  -> with qty: 2, article.price: 5
  -> with qty: 1
rule total: lines -> sum cost

spec bracket
data threshold: number
data income: number
rule applies: income > threshold

spec tax
data income: number
uses brackets: bracket list
  -> with threshold: 0, income: income
rule applicable: brackets -> where applies -> count
"#;

    #[test]
    fn instance_list_default_is_source_rows_as_objects() {
        let engine = create_engine_with_code(SOURCE_ROWS_CODE);
        let document = generate_openapi(&engine, false);
        let lines = &document["components"]["schemas"]["order_request"]["properties"]["lines"];
        assert_eq!(
            lines["default"],
            json!([{ "qty": "2", "article.price": "5" }, { "qty": "1" }])
        );
    }

    #[test]
    fn instance_list_with_referenced_fields_has_no_default() {
        let engine = create_engine_with_code(SOURCE_ROWS_CODE);
        let document = generate_openapi(&engine, false);
        let brackets = &document["components"]["schemas"]["tax_request"]["properties"]["brackets"];
        assert_eq!(brackets["type"], "array");
        assert!(brackets.get("default").is_none(), "{brackets}");
        assert!(
            brackets["x-lemma-value"]["list"][0]["fields"]["income"]["reference"] == "income",
            "{brackets}"
        );
    }

    #[test]
    fn row_schema_accepts_fields_of_row_spec_uses() {
        let engine = create_engine_with_code(SOURCE_ROWS_CODE);
        let document = generate_openapi(&engine, false);
        let row = &document["components"]["schemas"]["LemmaRow_line"];
        assert_eq!(row["additionalProperties"], true);
        assert_eq!(row["properties"]["article.price"]["type"], "string");
    }

    #[test]
    fn dotted_top_level_keys_are_request_properties() {
        let engine = create_engine_with_code(SOURCE_ROWS_CODE);
        let document = generate_openapi(&engine, false);
        let schemas = &document["components"]["schemas"];
        let request = &schemas["line_request"];
        assert_eq!(request["properties"]["article.price"]["type"], "string");
        assert_eq!(request["required"], json!(["article.price", "qty"]));
        assert_eq!(
            schemas["line_form_request"]["properties"]["article.price"]["type"],
            "string"
        );
    }

    #[test]
    fn self_list_row_schema_refers_to_itself() {
        let engine = create_engine_with_code(LIST_CODE);
        let document = generate_openapi(&engine, false);
        let schemas = &document["components"]["schemas"];
        assert_eq!(
            row_ref(&schemas["part_request"]["properties"]["components"]),
            "#/components/schemas/LemmaRow_part"
        );
        assert_eq!(
            row_ref(&schemas["LemmaRow_part"]["properties"]["components"]),
            "#/components/schemas/LemmaRow_part"
        );
    }

    #[test]
    fn lists_are_not_form_fields() {
        let engine = create_engine_with_code(LIST_CODE);
        let document = generate_openapi(&engine, false);
        let form = &document["components"]["schemas"]["order_form_request"]["properties"];
        assert_eq!(form, &json!({}));
    }

    #[test]
    fn evaluate_response_describes_rows_and_list_values() {
        let engine = create_engine_with_code(LIST_CODE);
        let document = generate_openapi(&engine, false);
        let schemas = &document["components"]["schemas"];
        assert_eq!(
            schemas["order_evaluate_response"]["properties"]["rows"]["items"]["$ref"],
            "#/components/schemas/RowResult"
        );
        let rule_result = &schemas["RuleResult"]["properties"];
        assert_eq!(
            rule_result["row"]["$ref"],
            "#/components/schemas/RowReference"
        );
        assert_eq!(
            rule_result["list"]["items"]["$ref"],
            "#/components/schemas/RuleResultValue"
        );
        assert_eq!(
            rule_result["spec"]["$ref"],
            "#/components/schemas/SpecResult"
        );
        assert_eq!(
            schemas["RuleResultValue"]["properties"]["list"]["items"]["$ref"],
            "#/components/schemas/RuleResultValue"
        );
    }

    #[test]
    fn every_schema_ref_resolves() {
        let engine = create_engine_with_code(LIST_CODE);
        let document = generate_openapi(&engine, false);
        let schemas = document["components"]["schemas"]
            .as_object()
            .expect("components.schemas");
        let mut stack = vec![&document];
        let mut refs = 0;
        while let Some(node) = stack.pop() {
            match node {
                Value::Object(map) => {
                    if let Some(Value::String(target)) = map.get("$ref") {
                        let name = target
                            .strip_prefix("#/components/schemas/")
                            .unwrap_or_else(|| panic!("unexpected $ref {target}"));
                        assert!(schemas.contains_key(name), "dangling $ref {target}");
                        refs += 1;
                    }
                    stack.extend(map.values());
                }
                Value::Array(items) => stack.extend(items),
                _ => {}
            }
        }
        assert!(refs > 0);
    }
}
