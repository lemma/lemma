use indexmap::IndexMap;
use lemma::{DateGranularity, DateTimeValue, Engine, EvaluateDataValue, SourceType};
use std::path::PathBuf;
use std::sync::Arc;

const SPEC: &str = r#"
spec t
data s: text
  -> option "a"
  -> option "b"
  -> option "c"
data x: number

rule r: 0
  unless s is "a" and x > 1 and x < 100 then 10
  unless s is "a" and x > 5 and x < 200 then 20
  unless s is "b" and x > 2 and x < 50 then 30
  unless s is "b" and x > 7 then 40
"#;

fn effective() -> DateTimeValue {
    DateTimeValue {
        year: 2026,
        month: 10,
        day: 5,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: None,
        granularity: DateGranularity::Full,
    }
}

#[test]
fn explanation_conjunctive_nest_follows_last_matching_unless() {
    let mut engine = Engine::new();
    let source = SourceType::Path(Arc::new(PathBuf::from("conjunctive_nest.lemma")));
    engine.load(vec![(source, SPEC.to_string())]).expect("load");

    let cases = [
        ("a", "0", "0"),
        ("a", "3", "10"),
        ("a", "7", "20"),
        ("a", "150", "20"),
        ("b", "1", "0"),
        ("b", "3", "30"),
        ("b", "8", "40"),
        ("c", "7", "0"),
    ];
    for (s, x, expected) in cases {
        let mut data: IndexMap<String, EvaluateDataValue> = IndexMap::new();
        data.insert("s".to_string(), EvaluateDataValue::Literal(s.to_string()));
        data.insert("x".to_string(), EvaluateDataValue::Literal(x.to_string()));
        let response = engine
            .evaluate(None, "t", Some(&effective()), data, None, true)
            .expect("evaluate");
        let rule = response.results.get("r").expect("rule r");
        assert!(!rule.vetoed, "s={s} x={x} vetoed");
        let value = rule.result.as_ref().expect("value").to_string();
        assert_eq!(value, expected, "s={s} x={x}");
        assert!(
            rule.explanation.is_some(),
            "s={s} x={x} missing explanation"
        );
    }
}
