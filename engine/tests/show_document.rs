//! The Show document is a Draft 2020-12 JSON Schema for the spec's run-data inputs.

use lemma::{DateTimeValue, Engine, ResourceLimits, SourceType};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

const SOURCE: &str = r#"
spec article
data price: number

spec line
data qty: number -> suggest 1
uses article: article
rule cost: qty * article.price

spec part
data own_cost: number
uses components: part list
rule unit_cost:
  own_cost + (components -> sum components.unit_cost)
  unless components -> count is 0 then own_cost

spec item
data name: text
data price: number

spec catalog
uses items: item list
  -> key name
  -> with name: "a", price: 1
rule total: items -> sum price

spec order
data money: measure -> unit eur: 1 -> unit cent: 0.01
data shipping: money
data express: boolean
data discounts: number list
  -> with 5, 10
uses lines: line list
  -> with qty: 2, article.price: 5
uses parts: part list
uses supplier: article
rule total: lines -> sum cost
rule fee: shipping
rule fast: express
rule discount_total: discounts -> sum
rule supplier_price: supplier.price
rule part_cost: parts -> sum unit_cost
"#;

fn load(engine: &mut Engine) {
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("show_document.lemma"))),
            SOURCE.to_string(),
        )])
        .expect("load");
}

fn engine() -> Engine {
    let mut engine = Engine::new();
    load(&mut engine);
    engine
}

fn document(engine: &Engine, spec: &str) -> Value {
    let now = DateTimeValue::now();
    let show = engine.show(None, spec, Some(&now)).expect("show");
    lemma::api::show_document(engine, &show)
}

fn payload() -> Value {
    json!({
        "shipping": "4.50 eur",
        "express": true,
        "discounts": ["1", "2"],
        "lines": [{ "qty": "3", "article.price": "2" }],
        "parts": [{ "own_cost": "1", "components": [{ "own_cost": "2" }] }],
        "supplier.price": "7"
    })
}

fn schema_errors(schema: &Value, instance: &Value) -> Vec<String> {
    let validator = jsonschema::validator_for(schema).expect("Show document compiles");
    validator
        .iter_errors(instance)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect()
}

fn api_show_def() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schemas/api.v1.json");
    let schema: Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("read api.v1.json"))
            .expect("api.v1.json JSON");
    json!({
        "$schema": schema["$schema"],
        "$defs": schema["$defs"],
        "$ref": "#/$defs/Show",
    })
}

#[test]
fn real_payload_validates_and_run_accepts_it() {
    let engine = engine();
    let document = document(&engine, "order");
    let payload = payload();
    let errors = schema_errors(&document, &payload);
    assert!(errors.is_empty(), "{}", errors.join("\n"));

    let data = lemma::api::parse_evaluate_data_object(&Some(payload)).expect("evaluation data");
    let response = engine
        .evaluate(None, "order", None, data, None, false)
        .expect("evaluate accepts the payload");
    for (name, result) in &response.results {
        assert!(!result.vetoed, "{name} vetoed: {result:?}");
    }
}

#[test]
fn invalid_payloads_are_rejected_by_the_document() {
    let engine = engine();
    let document = document(&engine, "order");
    for (label, payload) in [
        ("unknown key", json!({ "shiping": "4 eur" })),
        ("unit glued", json!({ "shipping": "4eur" })),
        ("unknown unit", json!({ "shipping": "4 usd" })),
        ("grouped number", json!({ "discounts": ["1,000"] })),
        (
            "unknown row field",
            json!({ "lines": [{ "qty": "1", "price": "2" }] }),
        ),
        ("json number", json!({ "supplier.price": 7 })),
    ] {
        assert!(
            !schema_errors(&document, &payload).is_empty(),
            "{label} must be rejected: {payload}"
        );
    }
}

#[test]
fn uses_alias_is_not_a_property_but_its_fields_are() {
    let engine = engine();
    let document = document(&engine, "order");
    let properties = document["properties"].as_object().expect("properties");
    assert!(!properties.contains_key("supplier"));
    assert!(!properties.contains_key("lines.qty"));
    assert_eq!(properties["supplier.price"]["type"], "string");
    assert_eq!(
        properties["supplier.price"]["x-lemma-path"],
        json!([{ "uses": "supplier", "spec": "article" }])
    );
    assert!(document["required"]
        .as_array()
        .expect("required")
        .contains(&json!("supplier.price")));
}

#[test]
fn value_list_is_array_of_element_literals() {
    let engine = engine();
    let discounts = &document(&engine, "order")["properties"]["discounts"];
    assert_eq!(discounts["type"], "array");
    assert_eq!(
        discounts["items"],
        json!({ "type": "string", "pattern": lemma::api::number_pattern() })
    );
    assert_eq!(discounts["default"], json!(["5", "10"]));
}

#[test]
fn instance_list_rows_are_defs_objects_and_lists_are_never_required() {
    let engine = engine();
    let document = document(&engine, "order");
    let lines = &document["properties"]["lines"];
    assert_eq!(lines["type"], "array");
    assert_eq!(lines["items"], json!({ "$ref": "#/$defs/LemmaRow_line" }));
    assert_eq!(
        lines["default"],
        json!([{ "qty": "2", "article.price": "5" }])
    );
    let required = document["required"].as_array().expect("required");
    assert!(!required.contains(&json!("lines")));
    assert!(!required.contains(&json!("discounts")));

    let row = &document["$defs"]["LemmaRow_line"];
    assert_eq!(row["title"], "line row");
    assert_eq!(row["type"], "object");
    assert_eq!(row["additionalProperties"], true);
    assert_eq!(row["properties"]["qty"]["examples"], json!(["1"]));
    assert_eq!(row["properties"]["article.price"]["type"], "string");
    assert_eq!(row["required"], json!(["article.price", "qty"]));
    let defs: Vec<&String> = document["$defs"]
        .as_object()
        .expect("$defs")
        .keys()
        .collect();
    assert_eq!(defs, ["LemmaRow_line", "LemmaRow_part"]);
}

#[test]
fn instance_list_without_source_rows_has_no_default_and_is_not_required() {
    let engine = engine();
    let document = document(&engine, "order");
    let parts = &document["properties"]["parts"];
    assert!(parts.get("default").is_none(), "{parts}");
    assert!(parts.get("x-lemma-value").is_none(), "{parts}");
    assert!(!document["required"]
        .as_array()
        .expect("required")
        .contains(&json!("parts")));
}

#[test]
fn self_list_row_schema_refers_to_itself() {
    let engine = engine();
    let document = document(&engine, "part");
    assert_eq!(
        document["properties"]["components"]["items"],
        json!({ "$ref": "#/$defs/LemmaRow_part" })
    );
    assert_eq!(
        document["$defs"]["LemmaRow_part"]["properties"]["components"]["items"],
        json!({ "$ref": "#/$defs/LemmaRow_part" })
    );
}

#[test]
fn keyed_instance_list_keeps_key_in_x_lemma_type() {
    let engine = engine();
    let document = document(&engine, "catalog");
    let items = &document["properties"]["items"];
    assert_eq!(items["x-lemma-type"]["key"], json!({ "fields": ["name"] }));
    assert_eq!(items["default"], json!([{ "name": "a", "price": "1" }]));
    let row = &document["$defs"]["LemmaRow_item"];
    assert_eq!(row["properties"]["name"]["type"], "string");
    assert_eq!(row["properties"]["price"]["type"], "string");
}

#[test]
fn max_items_is_max_list_rows() {
    let mut engine = Engine::with_limits(ResourceLimits {
        max_list_rows: 7,
        ..ResourceLimits::default()
    });
    load(&mut engine);
    let document = document(&engine, "order");
    assert_eq!(document["properties"]["discounts"]["maxItems"], 7);
    assert_eq!(document["properties"]["lines"]["maxItems"], 7);
    assert_eq!(
        document["$defs"]["LemmaRow_part"]["properties"]["components"]["maxItems"],
        7
    );
}

#[test]
fn output_is_byte_identical_across_engines() {
    let first = serde_json::to_string(&document(&engine(), "order")).expect("serialize");
    for _ in 0..3 {
        let again = serde_json::to_string(&document(&engine(), "order")).expect("serialize");
        assert_eq!(first, again);
    }
}

#[test]
fn documents_validate_against_api_show_def_and_metaschema() {
    let engine = engine();
    let show_def = api_show_def();
    for spec in ["order", "part", "catalog", "line"] {
        let document = document(&engine, spec);
        let errors = schema_errors(&show_def, &document);
        assert!(errors.is_empty(), "{spec}:\n{}", errors.join("\n"));
        jsonschema::draft202012::meta::validate(&document)
            .unwrap_or_else(|e| panic!("{spec} must be a 2020-12 schema: {e}"));
    }
}

#[test]
fn measure_decomposition_is_keyed_by_declaring_spec() {
    let engine = engine();
    let document = document(&engine, "order");
    let shipping = &document["properties"]["shipping"];
    assert_eq!(
        shipping["x-lemma-type"]["decomposition"],
        json!({ "order.money": 1 })
    );
    assert_eq!(
        shipping["x-lemma-type"]["units"][0]["decomposition"],
        json!({ "order.money": 1 })
    );
}

#[test]
fn compound_measure_and_instance_list_serialize() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("compound.lemma"))),
            r#"
spec line
uses lemma units
data money: measure -> unit eur: 1
data weight: units.mass
data price: measure -> unit eur_per_kg: eur/kilogram
rule cost: price * weight

spec order
uses lemma units
uses items: line list
  -> with weight: 2 kilogram, price: 3 eur_per_kg
rule total: items -> sum cost
"#
            .to_string(),
        )])
        .expect("load");
    let document = document(&engine, "order");
    let items = &document["properties"]["items"];
    assert_eq!(items["type"], "array");
    let row_price = &document["$defs"]["LemmaRow_line"]["properties"]["price"]["x-lemma-type"];
    assert_eq!(
        row_price["decomposition"],
        json!({ "line.money": 1, "lemma units.mass": -1 })
    );
}

#[test]
fn same_named_families_from_two_specs_keep_separate_decomposition_keys() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("two_money.lemma"))),
            r#"
spec a
data money: measure -> unit eur: 1

spec b
data money: measure -> unit usd: 1

spec t
uses a
uses b
data fx: measure -> unit eur_per_usd: eur/usd
data rate: fx
rule r: rate
"#
            .to_string(),
        )])
        .expect("load");
    let document = document(&engine, "t");
    let rate = &document["properties"]["rate"]["x-lemma-type"];
    assert_eq!(
        rate["decomposition"],
        json!({ "a.money": 1, "b.money": -1 })
    );
    assert_eq!(
        rate["units"][0]["decomposition"],
        json!({ "a.money": 1, "b.money": -1 })
    );
}

#[test]
fn local_and_imported_money_keep_separate_decomposition_keys() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("local_imported_money.lemma"))),
            r#"
spec pricing
data money: measure -> unit usd: 1

spec shop
uses pricing
data money: measure -> unit eur: 1
data rate: measure -> unit usd_per_eur: pricing.money.usd / eur
rule r: rate
"#
            .to_string(),
        )])
        .expect("load");
    let document = document(&engine, "shop");
    assert_eq!(
        document["properties"]["rate"]["x-lemma-type"]["decomposition"],
        json!({ "pricing.money": 1, "shop.money": -1 })
    );
}

#[test]
fn dependency_row_spec_is_titled_and_keyed_with_repository_then_spec() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Dependency("@acme/catalog".to_string()),
            r#"
spec item
data money: measure -> unit eur: 1
data price: money
"#
            .to_string(),
        )])
        .expect("dependency loads");
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("basket.lemma"))),
            r#"
spec basket
uses items: @acme/catalog item list
rule total: items -> sum price
"#
            .to_string(),
        )])
        .expect("workspace loads");
    let document = document(&engine, "basket");
    let row_ref = document["properties"]["items"]["items"]["$ref"]
        .as_str()
        .expect("items row $ref");
    let row_name = row_ref
        .strip_prefix("#/$defs/")
        .expect("row $ref points into $defs");
    let row = &document["$defs"][row_name];
    assert_eq!(row["title"], "@acme/catalog item row");
    assert_eq!(
        row["properties"]["price"]["x-lemma-type"]["decomposition"],
        json!({ "@acme/catalog item.money": 1 })
    );
}

#[test]
fn api_show_def_rejects_malformed_documents() {
    let engine = engine();
    let show_def = api_show_def();
    let document = document(&engine, "order");

    let mut extra_property_keyword = document.clone();
    extra_property_keyword["properties"]["shipping"]["x-lemma-unknown"] = json!(true);
    let mut missing_rules = document.clone();
    missing_rules
        .as_object_mut()
        .expect("document")
        .remove("x-lemma-rules");
    let mut help_in_type = document.clone();
    help_in_type["properties"]["shipping"]["x-lemma-type"]["help"] = json!("dup");
    let mut not_a_schema = document;
    not_a_schema["properties"]["shipping"]["type"] = json!("money");

    for (label, malformed) in [
        ("extra property keyword", extra_property_keyword),
        ("missing x-lemma-rules", missing_rules),
        ("help in x-lemma-type", help_in_type),
        ("invalid type keyword", not_a_schema),
    ] {
        assert!(
            !schema_errors(&show_def, &malformed).is_empty(),
            "{label} must be rejected"
        );
    }
}
