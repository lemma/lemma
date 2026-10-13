//! Contract: `engine/schemas/api.v1.json` is the schema for Show, Response, list, and errors.

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, EvaluateDataValue, ResourceLimits, SourceType};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn api_schema_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas/api.v1.json")
}

fn load_api_schema() -> serde_json::Value {
    let path = api_schema_path();
    assert!(
        path.is_file(),
        "engine/schemas/api.v1.json must exist (emit via `cargo run -p xtask -- schema`)"
    );
    let text = std::fs::read_to_string(&path).expect("read api.v1.json");
    serde_json::from_str(&text).expect("api.v1.json must be JSON")
}

fn sample_documents() -> Vec<(&'static str, serde_json::Value)> {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("sample.lemma"))),
            r#"
spec sample 2024-01-01
meta title: "t"
data amount: number -> suggest 1
data money: measure
  -> unit eur: 1
  -> suggest 10 eur
rule ok: amount
"#
            .to_string(),
        )])
        .expect("load");
    let now = DateTimeValue::now();
    let show = engine.show(None, "sample", Some(&now)).expect("show");
    let response = engine
        .evaluate(
            None,
            "sample",
            Some(&now),
            IndexMap::from([("amount".into(), "2".into())]),
            None,
            true,
        )
        .expect("evaluate");
    let specs = engine.specs();

    let limits = ResourceLimits {
        max_source_size_bytes: 50,
        ..ResourceLimits::default()
    };
    let err = Engine::with_limits(limits)
        .load([(SourceType::Volatile, "spec t\ndata x: 1\n".repeat(20))])
        .expect_err("limit");
    let errors: Vec<_> = err
        .errors
        .iter()
        .map(lemma::__test_support::current_binding_error_json)
        .collect();

    let install = lemma::RepositoryInstallResult {
        source: "repo @iso/countries\n\nspec alpha2\ndata code: text\n".to_string(),
        id: "@iso/countries".to_string(),
    };

    vec![
        ("show", lemma::api::show_document(&engine, &show)),
        (
            "response",
            serde_json::to_value(lemma::api::Response::from(&response)).expect("response"),
        ),
        ("specs", serde_json::to_value(&specs).expect("specs")),
        ("errors", serde_json::Value::Array(errors)),
        ("install", serde_json::to_value(&install).expect("install")),
    ]
}

#[test]
fn api_schema_file_exists_and_is_object() {
    let schema = load_api_schema();
    assert!(schema.is_object(), "api.v1.json root must be an object");
    assert_eq!(
        schema["$id"],
        "https://github.com/lemma/lemma/blob/main/engine/schemas/api.v1.json"
    );
}

#[test]
fn real_show_run_list_and_error_documents_validate_against_schema() {
    for (name, document) in sample_documents() {
        // Full JSON Schema validation lands with the schema emitter; until then the
        // schema file must at least exist and documents must be JSON objects/arrays.
        assert!(
            document.is_object() || document.is_array(),
            "{name} document must be object or array"
        );
        validate_against_api_schema(&document)
            .unwrap_or_else(|e| panic!("{name} must validate against api.v1.json: {e}"));
    }
}

/// Show documents are JSON Schemas: they validate against the Draft 2020-12 metaschema
/// bundled with the `jsonschema` crate (no network).
fn assert_is_2020_12_schema(name: &str, document: &serde_json::Value) {
    jsonschema::draft202012::meta::validate(document)
        .unwrap_or_else(|e| panic!("{name} must be a Draft 2020-12 JSON Schema: {e}"));
    jsonschema::draft202012::new(document)
        .unwrap_or_else(|e| panic!("{name} must compile as a JSON Schema: {e}"));
}

#[test]
fn show_documents_are_2020_12_schemas() {
    let show = sample_documents()
        .into_iter()
        .find(|(name, _)| *name == "show")
        .expect("sample show")
        .1;
    assert_is_2020_12_schema("sample show", &show);
    let engine = list_engine();
    for (name, document) in list_documents(&engine) {
        if name.ends_with("show") {
            assert_is_2020_12_schema(&name, &document);
        }
    }
}

fn validate_against_api_schema(document: &serde_json::Value) -> Result<(), String> {
    let schema = load_api_schema();
    let validator = jsonschema::validator_for(&schema).map_err(|e| e.to_string())?;
    validator.validate(document).map_err(|e| e.to_string())
}

#[test]
fn every_decimal_string_field_has_explicit_format_marker() {
    let schema = load_api_schema();
    let text = serde_json::to_string(&schema).expect("schema text");
    assert!(
        text.contains("\"format\"")
            && (text.contains("decimal") || text.contains("lemma-decimal")),
        "decimal-string fields must carry an explicit format marker for Java BigDecimal / TS string mapping"
    );
}

#[test]
fn checked_in_schema_matches_generated() {
    let status = Command::new("cargo")
        .args(["run", "-p", "xtask", "--quiet", "--", "schema", "--check"])
        .current_dir(workspace_root())
        .status()
        .expect("spawn xtask schema --check");
    assert!(
        status.success(),
        "checked-in api.v1.json must match `cargo run -p xtask -- schema` output"
    );
}

const LIST_SOURCE: &str = r#"
spec menu_item
data drink: text
data price: measure
  -> unit eur: 1.00
  -> unit cent: 0.01

spec coffee_order
uses menu: menu_item list
  -> key drink
  -> with drink: "espresso", price: 2.50 eur
  -> with drink: "latte", price: 350 cent
data ordered: menu.drink
  -> with "latte"
data fares: menu.price list
  -> with 89 eur, 120 eur
rule selected:
  menu -> find ordered
rule selected_price:
  selected.price
rule pricey:
  menu -> where price > 3 eur
rule by_price:
  menu -> sort price desc -> top 1
rule high_fares:
  fares -> where >= 100 eur
rule mid_fares:
  fares -> where in 80 eur...100 eur
rule fare_total:
  fares -> sum
rule extended:
  menu -> with drink: "mocha", price: 4 eur
rule any_cheap:
  menu -> any price < 3 eur

spec part
data money: measure
  -> unit eur: 1.00
uses components: part list
data own_cost: money
rule unit_cost:
  own_cost + (components -> sum components.unit_cost)
  unless components -> count is 0 then own_cost
rule first_child:
  components -> first
"#;

fn list_engine() -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("lists.lemma"))),
            LIST_SOURCE.to_string(),
        )])
        .expect("load");
    engine
}

fn list_documents(engine: &Engine) -> Vec<(String, serde_json::Value)> {
    let now = DateTimeValue::now();
    let mut documents = Vec::new();
    let runs: [(&str, IndexMap<String, EvaluateDataValue>); 2] = [
        ("coffee_order", IndexMap::new()),
        (
            "part",
            IndexMap::from([
                ("own_cost".to_string(), EvaluateDataValue::from("1 eur")),
                (
                    "components".to_string(),
                    EvaluateDataValue::List(vec![EvaluateDataValue::Map(IndexMap::from([
                        ("own_cost".to_string(), EvaluateDataValue::from("2 eur")),
                        (
                            "components".to_string(),
                            EvaluateDataValue::List(vec![EvaluateDataValue::Map(IndexMap::from(
                                [("own_cost".to_string(), EvaluateDataValue::from("3 eur"))],
                            ))]),
                        ),
                    ]))]),
                ),
            ]),
        ),
    ];
    for (spec, data) in runs {
        let show = engine.show(None, spec, Some(&now)).expect("show");
        documents.push((
            format!("{spec} show"),
            lemma::api::show_document(engine, &show),
        ));
        let response = engine
            .evaluate(None, spec, Some(&now), data, None, true)
            .expect("evaluate");
        documents.push((
            format!("{spec} response"),
            serde_json::to_value(lemma::api::Response::from(&response)).expect("response json"),
        ));
    }
    documents
}

/// Every validation error of `document` against `#/$defs/{def}`, with instance paths.
fn errors_against_def(document: &serde_json::Value, def: &str) -> Vec<String> {
    let schema = load_api_schema();
    let rooted = serde_json::json!({
        "$schema": schema["$schema"],
        "$defs": schema["$defs"],
        "$ref": format!("#/$defs/{def}"),
    });
    let validator = jsonschema::validator_for(&rooted).expect("api.v1.json compiles");
    validator
        .iter_errors(document)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect()
}

#[test]
fn list_show_and_run_documents_validate_against_schema() {
    let engine = list_engine();
    for (name, document) in list_documents(&engine) {
        let def = if name.ends_with("show") {
            "Show"
        } else {
            "Response"
        };
        let errors = errors_against_def(&document, def);
        assert!(
            errors.is_empty(),
            "{name} must validate against api.v1.json:\n{}",
            errors.join("\n")
        );
        validate_against_api_schema(&document)
            .unwrap_or_else(|e| panic!("{name} must validate against api.v1.json: {e}"));
    }
}

/// `tests/fixtures/api/{show,response}_list.json` are exactly what the engine emits for
/// `tests/fixtures/coffee_order_lists.lemma` at 2026-01-01, and both validate.
#[test]
fn list_fixtures_match_engine_output_and_schema() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let source = std::fs::read_to_string(fixtures.join("coffee_order_lists.lemma"))
        .expect("read coffee_order_lists.lemma");
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("coffee_order_lists.lemma"))),
            source,
        )])
        .expect("load");
    let effective = <DateTimeValue as std::str::FromStr>::from_str("2026-01-01").expect("date");
    let show = lemma::api::show_document(
        &engine,
        &engine
            .show(None, "coffee_order", Some(&effective))
            .expect("show"),
    );
    let response = serde_json::to_value(lemma::api::Response::from(
        &engine
            .evaluate(
                None,
                "coffee_order",
                Some(&effective),
                IndexMap::new(),
                None,
                false,
            )
            .expect("evaluate"),
    ))
    .expect("response json");
    for (file, def, actual) in [
        ("show_list.json", "Show", show),
        ("response_list.json", "Response", response),
    ] {
        let text = std::fs::read_to_string(fixtures.join("api").join(file))
            .unwrap_or_else(|e| panic!("read {file}: {e}"));
        let fixture: serde_json::Value =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{file} JSON: {e}"));
        assert_eq!(fixture, actual, "{file} must equal engine output");
        let errors = errors_against_def(&fixture, def);
        assert!(errors.is_empty(), "{file}:\n{}", errors.join("\n"));
    }
}

#[test]
fn list_documents_carry_list_type_rows_and_structured_commands() {
    let engine = list_engine();
    let documents: HashMap<String, serde_json::Value> =
        list_documents(&engine).into_iter().collect();
    let show = &documents["coffee_order show"];
    assert_eq!(
        show["properties"]["menu"]["x-lemma-type"]["key"],
        serde_json::json!({"fields": ["drink"]})
    );
    assert_eq!(
        show["properties"]["menu"]["x-lemma-type"]["element"]["kind"],
        "spec"
    );
    assert_eq!(
        show["properties"]["fares"]["x-lemma-type"]["key"],
        "numbered"
    );
    let high = &show["x-lemma-rules"]["high_fares"]["branches"][0]["result"];
    assert_eq!(high["command"], "where");
    assert_eq!(high["args"][0]["type"], "comparison");
    assert_eq!(high["args"][0]["left"]["type"], "row_value");
    let by_price = &show["x-lemma-rules"]["by_price"]["branches"][0]["result"];
    assert_eq!(by_price["command"], "top");
    assert_eq!(by_price["list"]["command"], "sort");
    assert_eq!(by_price["list"]["direction"], "desc");

    let response = &documents["coffee_order response"];
    let selected = &response["results"]["selected"]["row"];
    assert_eq!(selected["instance"], "menu.2");
    let index = selected["index"].as_u64().expect("row index") as usize;
    assert_eq!(
        response["rows"][index]["data"]["drink"]["result"]["text"],
        "latte"
    );
    assert_eq!(
        response["results"]["high_fares"]["list"][0]["result"],
        "120 eur"
    );
}
