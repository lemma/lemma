//! Decomposition promotion with cross-repo `units` basename collision (bugs.md §1–§3).

use lemma::Engine;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn source() -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from(
        "decomposition_promotion_collision.lemma",
    )))
}

fn assert_loads(code: &str) {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("spec must load and plan");
}

fn eval_display(code: &str, repository: Option<&str>, spec_name: &str, rule_name: &str) -> String {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("spec must load");
    let response = engine
        .run(repository, spec_name, None, HashMap::new(), None, true)
        .expect("spec must evaluate");
    response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("rule '{rule_name}' missing"))
        .result()
        .expect("result")
        .to_string()
}

const AREA_CALC_SPEC: &str = r#"repo alpha
spec units
data length: measure
  -> unit widget: 1
data area: measure
  -> unit sqwidget: widget*widget

spec worker
uses myunits: alpha units
data l: 3 widget
data w: 4 widget
rule area_calc: l * w
"#;

#[test]
fn area_calc_promotes_to_sqwidget() {
    assert_loads(AREA_CALC_SPEC);
    let display = eval_display(AREA_CALC_SPEC, Some("alpha"), "worker", "area_calc");
    assert!(
        display.contains("12") && display.to_lowercase().contains("sqwidget"),
        "expected 12 sqwidget, got: {display}"
    );
}
