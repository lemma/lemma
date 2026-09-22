//! Per-rule `missing_data` from `Engine::run` (structural − bound − released).
//! Types/suggestions stay on `Engine::show`.

use lemma::{DateTimeValue, Engine, VetoType};
use std::collections::HashMap;

fn missing(response: &lemma::Response, rule: &str) -> Vec<String> {
    response
        .results
        .get(rule)
        .unwrap_or_else(|| panic!("rule {rule}"))
        .missing_data()
        .to_vec()
}

const CHOOSER_LEMMA: &str = r#"
spec chooser

data mode: text -> options "simple" "complex"
data simple_input: number
data complex_input_a: number
data complex_input_b: number

rule result: veto "pick mode"
  unless mode is "simple" then simple_input
  unless mode is "complex" then complex_input_a + complex_input_b
"#;

#[test]
fn missing_data_only_for_requested_rules_live_inputs() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec demo
data expensive: 10
data threshold: number
data unrelated: 5
rule main: expensive * 2
  unless now > 1970-01-01 then threshold + 1
rule other: unrelated * 2
"#
            .to_string(),
        )])
        .expect("spec must load");

    let now = DateTimeValue::now();
    let mut inputs = HashMap::new();
    inputs.insert("threshold".to_string(), "4".into());
    let response = engine
        .run(
            None,
            "demo",
            Some(&now),
            inputs,
            Some(&["main".to_string()]),
            false,
        )
        .expect("evaluation must succeed");

    let md = missing(&response, "main");
    assert!(
        md.is_empty(),
        "unless win + threshold bound releases expensive: {md:?}"
    );
    assert!(
        !response.results.contains_key("other"),
        "unrequested rule must not appear"
    );
}

#[test]
fn missing_data_lists_unbound_reachable_inputs() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec demo
data supplied: number
data missing: number
rule main: supplied + missing
"#
            .to_string(),
        )])
        .expect("spec must load");

    let now = DateTimeValue::now();
    let mut inputs = HashMap::new();
    inputs.insert("supplied".to_string(), "4".into());
    let response = engine
        .run(None, "demo", Some(&now), inputs, None, false)
        .expect("evaluation must succeed");

    assert_eq!(missing(&response, "main"), vec!["missing".to_string()]);
}

#[test]
fn missing_data_reports_all_unbound_operands() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec demo
data a: number
data b: number
data c: number
rule main: a + b + c
"#
            .to_string(),
        )])
        .expect("spec must load");

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "demo", Some(&now), HashMap::new(), None, false)
        .expect("evaluation must succeed");

    assert_eq!(
        missing(&response, "main"),
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
    let main = response.results.get("main").expect("main rule result");
    assert!(main.vetoed, "main must veto when operands are missing");
    assert!(
        matches!(
            main.veto_detail.as_ref(),
            Some(VetoType::MissingData { data, .. }) if data.input_key() == "a"
        ),
        "rule execution stops at first missing operand: {:?}",
        main.veto_detail
    );
}

#[test]
fn missing_data_follows_rule_target_reference() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec inner
data slot: number

spec source_spec
data v: 5
rule computed: v * 2

spec outer
uses i: inner
  -> with slot: src.computed
uses src: source_spec
rule r: i.slot
"#
            .to_string(),
        )])
        .expect("spec must load");

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "outer", Some(&now), HashMap::new(), None, false)
        .expect("evaluation must succeed");

    let md = missing(&response, "r");
    assert!(
        md.is_empty(),
        "src.v prefilled via reference chain; got {md:?}"
    );
}

#[test]
fn suggest_does_not_bind_prefill_does() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec demo
data prefilled: 10
data suggested: number -> suggest 5
data required: number
rule main: prefilled + suggested + required
"#
            .to_string(),
        )])
        .expect("spec must load");

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "demo", Some(&now), HashMap::new(), None, false)
        .expect("evaluation must succeed");

    let md = missing(&response, "main");
    assert!(
        !md.contains(&"prefilled".to_string()),
        "prefill binds: {md:?}"
    );
    assert!(
        md.contains(&"suggested".to_string()),
        "suggest does not bind: {md:?}"
    );
    assert!(
        md.contains(&"required".to_string()),
        "required unbound: {md:?}"
    );

    let show = engine.show(None, "demo", Some(&now)).expect("show");
    let suggested = show.data.get("suggested").expect("suggested in show");
    assert!(suggested
        .suggestion
        .as_ref()
        .is_some_and(|s| s.number == Some(rust_decimal::Decimal::from(5))));
    assert!(suggested.fill.is_none());

    let main = response.results.get("main").expect("main rule");
    assert!(main.vetoed, "required operand missing");
}

#[test]
fn invalid_overlay_veto_binds_path_not_missing() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec demo
data amount: number -> minimum 0
rule main: amount * 2
"#
            .to_string(),
        )])
        .expect("spec must load");

    let now = DateTimeValue::now();
    let mut inputs = HashMap::new();
    inputs.insert("amount".to_string(), "-5".into());
    let response = engine
        .run(None, "demo", Some(&now), inputs, None, false)
        .expect("evaluation must succeed");

    assert!(
        missing(&response, "main").is_empty(),
        "veto-bound path is not missing: {:?}",
        missing(&response, "main")
    );
    let main = response.results.get("main").expect("main rule");
    assert!(main.vetoed);
    assert!(matches!(
        main.veto_detail.as_ref(),
        Some(VetoType::Computation { .. })
    ));
}

#[test]
fn missing_data_prunes_dead_branch_after_unless_discriminator() {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, CHOOSER_LEMMA.to_string())])
        .expect("spec must load");

    let now = DateTimeValue::now();
    let mut inputs = HashMap::new();
    inputs.insert("mode".to_string(), "simple".into());
    let response = engine
        .run(
            None,
            "chooser",
            Some(&now),
            inputs,
            Some(&["result".to_string()]),
            false,
        )
        .expect("evaluation must succeed");

    assert_eq!(
        missing(&response, "result"),
        vec!["simple_input".to_string()]
    );
}

#[test]
fn missing_data_prunes_simple_branch_when_mode_is_complex() {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, CHOOSER_LEMMA.to_string())])
        .expect("spec must load");

    let now = DateTimeValue::now();
    let mut inputs = HashMap::new();
    inputs.insert("mode".to_string(), "complex".into());
    let response = engine
        .run(
            None,
            "chooser",
            Some(&now),
            inputs,
            Some(&["result".to_string()]),
            false,
        )
        .expect("evaluation must succeed");

    assert_eq!(
        missing(&response, "result"),
        vec!["complex_input_a".to_string(), "complex_input_b".to_string(),]
    );
}

#[test]
fn missing_data_order_follows_evaluation_order() {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, CHOOSER_LEMMA.to_string())])
        .expect("spec must load");

    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "chooser",
            Some(&now),
            HashMap::new(),
            Some(&["result".to_string()]),
            false,
        )
        .expect("evaluation must succeed");

    assert_eq!(
        missing(&response, "result"),
        vec![
            "mode".to_string(),
            "complex_input_a".to_string(),
            "complex_input_b".to_string(),
            "simple_input".to_string(),
        ]
    );
}

#[test]
fn suggest_metadata_lives_on_show_not_run() {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Volatile,
            r#"
spec demo
data n: number -> suggest 42
rule main: n
"#
            .to_string(),
        )])
        .expect("spec must load");

    let now = DateTimeValue::now();
    let show = engine.show(None, "demo", Some(&now)).expect("show");
    let show_n = show.data.get("n").expect("n in show");
    assert!(show_n
        .suggestion
        .as_ref()
        .is_some_and(|s| s.number == Some(rust_decimal::Decimal::from(42))));

    let response = engine
        .run(None, "demo", Some(&now), HashMap::new(), None, false)
        .expect("run");
    assert_eq!(missing(&response, "main"), vec!["n".to_string()]);
}
