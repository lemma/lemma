//! Invalid runtime data overrides complete evaluation with Veto, not abort with Error.

use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn coffee_example_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/coffee_order.lemma")
}

fn recipe_example_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/recipe_scaling.lemma")
}

fn load_coffee(engine: &mut Engine) {
    let code = std::fs::read_to_string(coffee_example_path()).expect("read coffee example");
    engine
        .load([(
            lemma::SourceType::Path(Arc::new(PathBuf::from("01_coffee_order.lemma"))),
            &code.to_string(),
        )])
        .expect("load coffee_order");
}

fn load_recipe(engine: &mut Engine) {
    let code = std::fs::read_to_string(recipe_example_path()).expect("read recipe example");
    engine
        .load([(
            lemma::SourceType::Path(Arc::new(PathBuf::from("03_recipe_scaling.lemma"))),
            &code.to_string(),
        )])
        .expect("load recipe_scaling");
}

fn full_coffee_data(product: &str) -> HashMap<String, String> {
    HashMap::from([
        ("product".to_string(), product.to_string()),
        ("size".to_string(), "medium".into()),
        ("number_of_cups".to_string(), "1".into()),
        ("has_loyalty_card".to_string(), "false".into()),
        ("age".to_string(), "30".into()),
    ])
}

fn assert_run_completes_with_veto_not_validation_error(
    result: Result<lemma::Response, lemma::Error>,
    context: &str,
) -> lemma::Response {
    match result {
        Ok(response) => response,
        Err(err) => {
            panic!("{context}: run must complete with veto, not abort with Error — got: {err}")
        }
    }
}

#[test]
fn invalid_text_option_override_completes_with_veto_not_validation_error() {
    let mut engine = Engine::new();
    load_coffee(&mut engine);

    let now = DateTimeValue::now();
    let data = full_coffee_data("tea");

    let response = assert_run_completes_with_veto_not_validation_error(
        engine.run(None, "coffee_order", Some(&now), data, None, false),
        "product=tea (not in options)",
    );

    let base = response
        .results
        .get("base_price")
        .expect("base_price in results");
    assert!(
        base.vetoed,
        "invalid product override must veto base_price, not fail run"
    );
}

#[test]
fn below_minimum_number_override_completes_with_veto_not_validation_error() {
    let mut engine = Engine::new();
    load_coffee(&mut engine);

    let now = DateTimeValue::now();
    let mut data = full_coffee_data("latte");
    data.insert("age".to_string(), "-5".into());

    let response = assert_run_completes_with_veto_not_validation_error(
        engine.run(None, "coffee_order", Some(&now), data, None, false),
        "age=-5 (below minimum 0)",
    );

    let age_discount = response.results.get("age_discount").expect("age_discount");
    assert!(
        age_discount.vetoed,
        "below-minimum age must veto age_discount, got {:?}",
        age_discount.result()
    );
    let reason = age_discount.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.to_lowercase().contains("minimum") || reason.contains("at least"),
        "veto reason must mention minimum, got: {reason}"
    );
}

#[test]
fn unparsable_number_override_completes_with_veto_not_validation_error() {
    let code = r#"
spec s
data age: number
rule doubled: age * 2
"#;
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("plan");

    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("age".to_string(), "twenty".into());

    let response = assert_run_completes_with_veto_not_validation_error(
        engine.run(None, "s", Some(&now), data, None, false),
        "age=twenty (not a number)",
    );

    let doubled = response.results.get("doubled").expect("doubled");
    assert!(doubled.vetoed, "unparsable age must veto doubled");
    let reason = doubled.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.contains("Data age [number]:")
            && reason.contains("twenty")
            && reason.contains("Invalid number"),
        "veto reason must name field, type, and parse fact, got: {reason}"
    );
    assert!(
        !reason.contains("not a valid"),
        "veto reason must not use generic invalid template, got: {reason}"
    );
}

#[test]
fn empty_measure_override_names_field_type_and_unit() {
    let code = r#"
spec s
data mass: measure
  -> unit kilogram: 1
  -> unit gram: 0.001
data price: mass
rule total: price
"#;
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("plan");

    let now = DateTimeValue::now();
    for empty in ["", "   "] {
        let mut data = HashMap::new();
        data.insert("price".to_string(), empty.to_string());

        let response = assert_run_completes_with_veto_not_validation_error(
            engine.run(None, "s", Some(&now), data, None, false),
            &format!("price={empty:?} (empty measure)"),
        );
        let total = response.results.get("total").expect("total");
        assert!(total.vetoed, "empty price must veto total");
        let reason = total.veto_reason.as_deref().expect("veto reason");
        assert!(
            reason.contains("Data price [mass]:")
                && reason.to_lowercase().contains("cannot be empty"),
            "empty measure veto must name field and type, got: {reason}"
        );
        assert!(
            !reason.contains("eur"),
            "example unit must come from this type, got: {reason}"
        );
        assert!(
            !reason.contains("Measure value cannot be empty"),
            "veto reason must not dump FromStr grammar, got: {reason}"
        );
    }
}

#[test]
fn empty_number_override_names_field_without_unit_lecture() {
    let code = r#"
spec s
data age: number
rule doubled: age * 2
"#;
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("plan");

    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("age".to_string(), "   ".into());

    let response = assert_run_completes_with_veto_not_validation_error(
        engine.run(None, "s", Some(&now), data, None, false),
        "age=whitespace (empty number)",
    );
    let doubled = response.results.get("doubled").expect("doubled");
    assert!(doubled.vetoed, "empty age must veto doubled");
    let reason = doubled.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.contains("Data age [number]:") && reason.to_lowercase().contains("cannot be empty"),
        "empty number veto must name field and type, got: {reason}"
    );
    assert!(
        !reason.to_lowercase().contains("unit"),
        "number empty veto must not mention units, got: {reason}"
    );
}

#[test]
fn below_minimum_typedecl_override_completes_with_veto_not_validation_error() {
    let mut engine = Engine::new();
    load_recipe(&mut engine);

    let now = DateTimeValue::now();
    let data = HashMap::from([
        ("desired_servings".to_string(), "0".into()),
        ("original_servings".to_string(), "4".into()),
        ("recipe_name".to_string(), "chocolate_cake".into()),
    ]);

    let response = assert_run_completes_with_veto_not_validation_error(
        engine.run(None, "recipe_scaling", Some(&now), data, None, false),
        "desired_servings=0 (below minimum 1)",
    );

    let scaling_factor = response
        .results
        .get("scaling_factor")
        .expect("scaling_factor");
    assert!(
        scaling_factor.vetoed,
        "below-minimum desired_servings must veto scaling_factor, got {:?}",
        scaling_factor.result()
    );
    let reason = scaling_factor.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.to_lowercase().contains("minimum") || reason.contains("at least"),
        "veto reason must mention minimum, got: {reason}"
    );
}

#[test]
fn invalid_boolean_override_completes_with_veto_not_validation_error() {
    let code = r#"
spec s
data active: boolean
rule flag: active
"#;
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("plan");

    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("active".to_string(), "maybe".into());

    let response = assert_run_completes_with_veto_not_validation_error(
        engine.run(None, "s", Some(&now), data, None, false),
        "active=maybe (not boolean)",
    );

    let flag = response.results.get("flag").expect("flag");
    assert!(flag.vetoed, "invalid boolean override must veto flag");
}
