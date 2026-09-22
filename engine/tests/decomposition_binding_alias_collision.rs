//! Binding aliases sharing a measure family must not block decomposition promotion.

use lemma::Engine;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

fn source() -> lemma::SourceType {
    lemma::SourceType::Path(Arc::new(PathBuf::from(
        "decomposition_binding_alias_collision.lemma",
    )))
}

fn assert_loads(code: &str) {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("spec must load and plan");
}

fn eval_display(code: &str, spec_name: &str, rule_name: &str) -> String {
    let mut engine = Engine::new();
    engine
        .load([(source(), code.to_string())])
        .expect("spec must load");
    let response = engine
        .run(None, spec_name, None, HashMap::new(), None, true)
        .expect("spec must evaluate");
    response
        .results
        .get(rule_name)
        .unwrap_or_else(|| panic!("rule '{rule_name}' missing"))
        .result()
        .expect("result")
        .to_string()
}

const PURCHASE_COST_SPEC: &str = r#"spec units
uses lemma units
data money: measure
  -> unit eur: 1
data mass: measure
  -> unit kg: 1
data price_per_weight: measure
  -> unit eur_per_kg: eur/kg

spec purchase_cost
uses u: units
data total_eur: 100 eur
data weight: 10 kg
data starch_levy_per_kg: u.price_per_weight
  -> suggest 0.1 eur_per_kg
data amortization_per_kg: u.price_per_weight
  -> suggest 0.2 eur_per_kg
rule base_cost_per_kg: total_eur / weight
"#;

#[test]
fn division_promotes_despite_binding_aliases() {
    assert_loads(PURCHASE_COST_SPEC);
    let display = eval_display(PURCHASE_COST_SPEC, "purchase_cost", "base_cost_per_kg");
    assert!(
        display.contains("10") && display.to_lowercase().contains("eur_per_kg"),
        "expected 10 eur_per_kg, got: {display}"
    );
}
