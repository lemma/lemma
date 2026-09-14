//! Requested-rule evaluation: a rule runs only when the NormalForm walk visits
//! its `rule_ref`. Requested-rule `missing_data` is for that walk only; a full
//! run (all local rules) may still report missing inputs on other requested rules.

use lemma::{DateTimeValue, Engine, SourceType};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;

#[test]
fn unless_gated_dep_flag_false_empty_missing_data() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Volatile,
            r#"
spec gated
data amount: number
data flag: boolean
rule dep: amount * 2
rule total: 0
  unless flag then dep
"#
            .to_string(),
        )])
        .expect("load");
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("flag".to_string(), "false".to_string());
    let response = engine
        .run(
            None,
            "gated",
            Some(&now),
            data,
            Some(&["total".to_string()]),
            false,
        )
        .expect("run");
    let total = response.results.get("total").expect("total");
    assert!(!total.vetoed);
    assert!(
        total.missing_data().is_empty(),
        "flag=false must not surface amount: {:?}",
        total.missing_data()
    );
    let number = Decimal::from_str(
        total
            .value
            .as_ref()
            .expect("value")
            .number
            .as_ref()
            .expect("number"),
    )
    .expect("decimal");
    assert_eq!(number, Decimal::ZERO);
}

#[test]
fn unless_gated_dep_flag_true_lists_amount() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Volatile,
            r#"
spec gated
data amount: number
data flag: boolean
rule dep: amount * 2
rule total: 0
  unless flag then dep
"#
            .to_string(),
        )])
        .expect("load");
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("flag".to_string(), "true".to_string());
    let response = engine
        .run(
            None,
            "gated",
            Some(&now),
            data,
            Some(&["total".to_string()]),
            false,
        )
        .expect("run");
    let total = response.results.get("total").expect("total");
    assert!(total.vetoed);
    assert!(
        total.awaits_missing_data(),
        "flag=true unbound amount must await: {:?}",
        total.veto_reason
    );
    assert_eq!(
        total.missing_data(),
        &["amount".to_string()][..],
        "taken unless arm must list amount: {:?}",
        total.missing_data()
    );
}

#[test]
fn cross_spec_requested_rule_run() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Volatile,
            r#"
spec a
rule x: 7

spec b
uses a: a
rule y: a.x + 1
"#
            .to_string(),
        )])
        .expect("load");
    let now = DateTimeValue::now();
    let response = engine
        .run(
            None,
            "b",
            Some(&now),
            HashMap::new(),
            Some(&["y".to_string()]),
            false,
        )
        .expect("run");
    assert_eq!(response.results.len(), 1);
    let y = response.results.get("y").expect("y");
    assert!(!y.vetoed);
    let number = Decimal::from_str(
        y.value
            .as_ref()
            .expect("value")
            .number
            .as_ref()
            .expect("number"),
    )
    .expect("decimal");
    assert_eq!(number, Decimal::from(8));
}

const AND_LEFT_RULE_REF: &str = r#"
spec gate
data n: number
data secret: number
rule gate_rule: n > 0
rule main: gate_rule and secret > 0
rule pw: 0
  unless gate_rule then secret
"#;

fn run_gate(data: &[(&str, &str)], rules: Option<&[String]>) -> lemma::Response {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, AND_LEFT_RULE_REF.to_string())])
        .expect("load");
    let now = DateTimeValue::now();
    let bindings: HashMap<String, String> = data
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    engine
        .run(None, "gate", Some(&now), bindings, rules, false)
        .expect("run")
}

#[test]
fn rule_ref_and_left_true_lists_unbound_right_tip_run() {
    let response = run_gate(&[("n", "5")], Some(&["main".to_string()]));
    let main = response.results.get("main").expect("main");
    assert!(main.awaits_missing_data(), "{:?}", main.veto_reason);
    assert_eq!(main.missing_data(), &["secret".to_string()][..]);
}

#[test]
fn rule_ref_and_left_true_lists_unbound_right_full_run() {
    let response = run_gate(&[("n", "5")], None);
    let main = response.results.get("main").expect("main");
    assert!(main.awaits_missing_data(), "{:?}", main.veto_reason);
    assert_eq!(main.missing_data(), &["secret".to_string()][..]);
    let gate_rule = response.results.get("gate_rule").expect("gate_rule");
    assert!(!gate_rule.vetoed);
    assert!(gate_rule.missing_data().is_empty());
}

#[test]
fn rule_ref_and_left_missing_data_lists_left_only() {
    let response = run_gate(&[], Some(&["main".to_string()]));
    let main = response.results.get("main").expect("main");
    assert!(main.awaits_missing_data(), "{:?}", main.veto_reason);
    assert_eq!(main.missing_data(), &["n".to_string()][..]);
}

#[test]
fn rule_ref_and_left_false_is_false_without_missing_data() {
    let response = run_gate(&[("n", "-5")], Some(&["main".to_string()]));
    let main = response.results.get("main").expect("main");
    assert!(!main.vetoed, "{:?}", main.veto_reason);
    assert!(main.missing_data().is_empty());
    assert_eq!(main.value.as_ref().expect("value").boolean, Some(false));
}

#[test]
fn rule_ref_piecewise_condition_true_lists_unbound_body() {
    let response = run_gate(&[("n", "5")], Some(&["pw".to_string()]));
    let pw = response.results.get("pw").expect("pw");
    assert!(pw.awaits_missing_data(), "{:?}", pw.veto_reason);
    assert_eq!(pw.missing_data(), &["secret".to_string()][..]);
}

#[test]
fn full_run_lists_amount_on_dep_when_requested_total_does_not() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Volatile,
            r#"
spec gated
data amount: number
data flag: boolean
rule dep: amount * 2
rule total: 0
  unless flag then dep
"#
            .to_string(),
        )])
        .expect("load");
    let now = DateTimeValue::now();
    let mut data = HashMap::new();
    data.insert("flag".to_string(), "false".to_string());
    let response = engine
        .run(None, "gated", Some(&now), data, None, false)
        .expect("run");
    let total = response.results.get("total").expect("total");
    assert!(!total.vetoed);
    assert!(
        total.missing_data().is_empty(),
        "requested-equivalent total must stay clean: {:?}",
        total.missing_data()
    );
    let dep = response.results.get("dep").expect("dep");
    assert!(
        dep.awaits_missing_data(),
        "full run still evaluates dep: {dep:?}"
    );
    assert_eq!(
        dep.missing_data(),
        &["amount".to_string()][..],
        "full-run dep lists amount while total does not"
    );
}
