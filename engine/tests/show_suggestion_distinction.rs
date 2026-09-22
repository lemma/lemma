//! Spec-declared suggestions stay on [`DataDefinition::TypeDeclaration`] in the plan;
//! show/response expose them as suggestions only — the evaluator never commits them.

use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;

#[test]
fn run_plan_does_not_commit_typedecl_suggestion() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("raw.lemma"))),
            r#"
        spec s
        data n: number -> suggest 42
        rule r: n
    "#
            .to_string(),
        )])
        .expect("load");

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "s", Some(&now), HashMap::new(), None, false)
        .expect("response");

    assert!(
        response
            .results
            .get("r")
            .expect("rule r")
            .missing_data()
            .iter()
            .any(|p| p == "n"),
        "suggestion does not commit; n remains missing until supplied: {:?}",
        response.results.get("r").map(|r| r.missing_data())
    );

    let rule = response.results.get("r").expect("rule r");
    assert!(
        rule.vetoed,
        "suggest must not commit; unbound n must veto MissingData, got: {:?}",
        rule.veto_reason
    );

    let mut supplied = HashMap::new();
    supplied.insert("n".to_string(), "42".into());
    let response = engine
        .run(None, "s", Some(&now), supplied, None, false)
        .expect("response with n supplied");
    assert!(
        response
            .results
            .get("r")
            .expect("rule r")
            .missing_data()
            .is_empty(),
        "n must not be missing once supplied in run data"
    );
    let rule = response.results.get("r").expect("rule r");
    assert!(!rule.vetoed, "rule r must succeed once n is supplied");
    assert_eq!(rule.result(), Some("42"));
}

#[test]
fn show_shows_suggestion_not_prefilled_without_overlay() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("s.lemma"))),
            r#"
        spec s
        data n: number -> suggest 42
        rule r: n
    "#
            .to_string(),
        )])
        .expect("load");

    let now = DateTimeValue::now();
    let entry = engine
        .show(None, "s", Some(&now))
        .expect("show")
        .data
        .get("n")
        .expect("n in show")
        .clone();

    assert!(
        entry.fill.is_none(),
        "suggestion is not fill until caller supplies"
    );
    let suggestion = entry.suggestion.expect("show must expose suggestion");
    assert_eq!(
        suggestion.number,
        Some(rust_decimal::Decimal::from(42)),
        "suggestion magnitude must be the declared 42"
    );
    assert_eq!(
        suggestion.result.as_deref(),
        Some("42"),
        "suggestion must carry engine-rendered display from LiteralValue::display_value"
    );
}

#[test]
fn template_literal_with_prefills_nested_slot_not_suggestion() {
    let code = r#"
spec a
data x: number -> minimum 0 -> suggest 1
rule r: x

spec a/template
uses a
  -> with x: 2
rule r: a.r
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("load");

    let now = DateTimeValue::now();
    let base = engine.show(None, "a", Some(&now)).expect("base show");
    let base_x = base.data.get("x").expect("x in base show");
    assert!(
        base_x.suggestion.is_some(),
        "base spec exposes suggestion on x"
    );
    assert!(base_x.fill.is_none(), "base spec has no fill x");

    let template = engine
        .show(None, "a/template", Some(&now))
        .expect("template show");
    let template_x = template.data.get("a.x").expect("a.x in template show");
    assert!(
        template_x.fill.is_some(),
        "literal with must surface as fill"
    );
    assert_eq!(
        template_x.fill.as_ref().and_then(|v| v.number),
        Some(rust_decimal::Decimal::from(2)),
        "literal with fills magnitude 2"
    );
    assert!(
        template_x.suggestion.is_none(),
        "template must not inherit typedef suggestion once slot is filled"
    );
}
