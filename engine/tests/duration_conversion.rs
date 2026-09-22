use std::collections::HashMap;

use lemma::DateTimeValue;
use lemma::{Engine, ValueKind};

#[test]
fn test_duration_conversion_properties() {
    let mut engine = Engine::new();
    let code = r#"
spec test
uses lemma units
data duration: 60 minute
rule to_hours: duration as hour
"#;
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("test"))),
            code.to_string(),
        )])
        .unwrap();

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), HashMap::new(), None, true)
        .unwrap();
    let rule_result = response
        .results
        .values()
        .find(|r| r.rule.name == "to_hours")
        .unwrap();
    let val = rule_result
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value")
        .clone();

    if let ValueKind::Measure(_) = &val.value {
        assert_eq!(
            rule_result
                .result
                .as_ref()
                .and_then(|v| v.measure.as_ref())
                .and_then(|m| m.get("hour"))
                .copied(),
            Some(rust_decimal::Decimal::from(1)),
            "60 minute as hour"
        );
    } else {
        panic!(
            "to_hours should be a Measure after conversion, got {:?}",
            val
        );
    }
}
