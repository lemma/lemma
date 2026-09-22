//! Regression: `Engine::plans` key by `(repository, name)`.
//! Different repositories may reuse the same spec basename without clobbering plans.

use lemma::DateTimeValue;
use lemma::{Engine, SourceType};
use rust_decimal::Decimal;
use std::sync::Arc;

fn path_source(path: &str) -> SourceType {
    SourceType::Path(Arc::new(std::path::PathBuf::from(path)))
}

fn rule_answer_decimal(response: &lemma::Response) -> Decimal {
    let rr = response
        .results
        .get("answer")
        .expect("spec defines rule answer");
    let lit = rr
        .explanation
        .as_ref()
        .expect("explanation")
        .result
        .literal_value()
        .expect("value");
    match &lit.value {
        lemma::ValueKind::Number(n) => lemma::ValueKind::Number(n.clone())
            .as_decimal_magnitude()
            .unwrap(),
        other => panic!("expected number answer, got {:?}", other),
    }
}

#[test]
fn distinct_repositories_same_spec_basename_must_not_alias_execution_plan() {
    let mut engine = Engine::new();
    engine
        .load([(
            path_source("alpha.lemma"),
            r#"repo alpha
spec duped
rule answer: 1
"#
            .to_string(),
        )])
        .expect("alpha repository loads");

    engine
        .load([(
            path_source("beta.lemma"),
            r#"repo beta
spec duped
rule answer: 99
"#
            .to_string(),
        )])
        .expect("beta repository loads");

    let now = DateTimeValue::now();

    let run_alpha = engine
        .run(
            Some("alpha"),
            "duped",
            Some(&now),
            Default::default(),
            None,
            true,
        )
        .expect("run alpha");
    let run_beta = engine
        .run(
            Some("beta"),
            "duped",
            Some(&now),
            Default::default(),
            None,
            true,
        )
        .expect("run beta");

    assert_eq!(
        rule_answer_decimal(&run_alpha),
        Decimal::ONE,
        "alpha::duped rule answer must be 1"
    );
    assert_eq!(
        rule_answer_decimal(&run_beta),
        Decimal::from(99),
        "beta::duped rule answer must be 99"
    );
}
