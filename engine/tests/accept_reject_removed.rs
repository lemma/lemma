use lemma::{DateTimeValue, Engine, ErrorKind};
use std::collections::HashMap;

#[test]
fn accept_and_reject_are_not_boolean_data_literals() {
    for keyword in ["accept", "reject"] {
        let code = format!("spec test\ndata flag: {keyword}");
        let mut engine = Engine::new();
        let err = engine
            .load([(lemma::SourceType::Volatile, code)])
            .expect_err("must not load");
        assert_eq!(err.errors.len(), 1);
        let e = &err.errors[0];
        assert_eq!(e.kind(), ErrorKind::Validation);
        let msg = e.to_string();
        assert!(
            msg.contains(keyword) && msg.contains("Unknown parent"),
            "'{keyword}' must fail as unknown type parent, not boolean literal; got: {msg}"
        );
    }
}

#[test]
fn accept_and_reject_are_not_boolean_rule_literals() {
    for keyword in ["accept", "reject"] {
        let code = format!("spec test\nrule r: {keyword}");
        let mut engine = Engine::new();
        let err = engine
            .load([(lemma::SourceType::Volatile, code)])
            .expect_err("must not load");
        assert_eq!(err.errors.len(), 1);
        let e = &err.errors[0];
        assert_eq!(e.kind(), ErrorKind::Validation);
        let msg = e.to_string();
        assert!(
            msg.contains(keyword) && msg.contains("not found"),
            "'{keyword}' must fail as missing reference, not boolean literal; got: {msg}"
        );
    }
}

#[test]
fn accept_and_reject_are_ordinary_identifiers() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec test
data accept: number
rule reject: accept
"#
            .to_string(),
        )])
        .expect("accept/reject as identifiers must load");

    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("accept".to_string(), "1".into());
    let response = engine
        .run(None, "test", Some(&now), data, None, false)
        .expect("run");
    let reject = response.results.get("reject").expect("reject rule");
    assert!(!reject.vetoed);
    assert_eq!(reject.result(), Some("1"));
}
