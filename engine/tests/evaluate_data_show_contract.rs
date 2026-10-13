use indexmap::IndexMap;
use lemma::{
    evaluate_data_value_from_json_value, show_document, DateTimeValue, Engine, EvaluateDataValue,
    SourceType,
};
use serde_json::{json, Value};

const SPEC: &str = r#"
spec test
data money: measure
  -> unit eur: 1.00
  -> unit gbp: 1.20
data qty: number
data flag: boolean
data label: text
data share: ratio
data fares: money list
rule echo_qty: qty
rule echo_flag: flag
rule echo_label: label
rule echo_money: money
rule echo_share: share
rule echo_fares: fares
"#;

fn engine() -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, SPEC.to_string())])
        .expect("load");
    engine
}

fn show_property(engine: &Engine, key: &str) -> Value {
    let now = DateTimeValue::now();
    let show = engine.show(None, "test", Some(&now)).expect("show");
    show_document(engine, &show)["properties"][key].clone()
}

/// Whether run data binds `sample` for `key`: no request error and no veto on the echo rule.
fn engine_binds(key: &str, sample: &Value) -> bool {
    let Ok(Some(value)) = evaluate_data_value_from_json_value(sample.clone()) else {
        return false;
    };
    let engine = engine();
    let now = DateTimeValue::now();
    let data: IndexMap<String, EvaluateDataValue> = IndexMap::from([(key.to_string(), value)]);
    let rule = format!("echo_{key}");
    match engine.evaluate(
        None,
        "test",
        Some(&now),
        data,
        Some(std::slice::from_ref(&rule)),
        false,
    ) {
        Ok(response) => response.get(&rule).is_ok_and(|result| !result.vetoed),
        Err(_) => false,
    }
}

fn samples() -> Vec<(&'static str, Vec<Value>)> {
    vec![
        (
            "qty",
            vec![
                json!("3"),
                json!("+3"),
                json!("-3"),
                json!(".5"),
                json!("5."),
                json!("1_000"),
                json!("_1"),
                json!("1__0"),
                json!("1e3"),
                json!("1E+3"),
                json!("1e-3"),
                json!(" 3 "),
                json!("1 000"),
                json!(""),
                json!("abc"),
                json!("1234567890123456789012345678"),
                json!("12345678901234567890123456789"),
                json!("0.1234567890123456789012345678"),
                json!("0.12345678901234567890123456789"),
                json!(3),
            ],
        ),
        (
            "flag",
            vec![
                json!(true),
                json!("TRUE"),
                json!(" yes "),
                json!("maybe"),
                json!("1"),
                json!(false),
            ],
        ),
        ("label", vec![json!("hello"), json!("")]),
        (
            "money",
            vec![
                json!("10 eur"),
                json!("10eur"),
                json!("10  eur"),
                json!("10 usd"),
                json!({"eur": "84"}),
                json!({"eur": "1", "gbp": "2"}),
                json!({"usd": "5"}),
            ],
        ),
        (
            "share",
            vec![
                json!("50%"),
                json!("50 %"),
                json!("50%%"),
                json!("50 percent"),
                json!("50 permille"),
                json!("0.5"),
                json!({"percent": "50"}),
            ],
        ),
        (
            "fares",
            vec![
                json!(["10 eur"]),
                json!([{"eur": "10"}]),
                json!(["10 eur", "bogus"]),
            ],
        ),
    ]
}

#[test]
fn show_accepts_only_what_evaluate_data_binds() {
    let engine = engine();
    for (key, samples) in samples() {
        let property = show_property(&engine, key);
        for sample in samples {
            if jsonschema::is_valid(&property, &sample) {
                assert!(
                    engine_binds(key, &sample),
                    "{key}: Show accepts {sample} but run data does not bind it"
                );
            }
        }
    }
}

#[test]
fn unknown_top_level_key_is_ignored() {
    let engine = engine();
    let now = DateTimeValue::now();
    let data: IndexMap<String, EvaluateDataValue> = IndexMap::from([
        (
            "qty".to_string(),
            EvaluateDataValue::Literal("3".to_string()),
        ),
        (
            "nope".to_string(),
            EvaluateDataValue::Literal("1".to_string()),
        ),
    ]);
    assert!(engine
        .evaluate(
            None,
            "test",
            Some(&now),
            data,
            Some(&["echo_qty".to_string()]),
            false
        )
        .is_ok());
}
