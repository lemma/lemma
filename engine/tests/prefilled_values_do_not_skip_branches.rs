//! Spec prefilled values (`data is_member: false`) must not cause
//! `collect_needed_data_paths` to treat unless arms as dead. Only caller overlay
//! may skip branch inputs. Covers static show and per-rule `missing_data`.

use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;

const PRICING_SPEC: &str = r#"
spec pricing
data base_price: 100
data is_member: false
data quantity: number
rule discount: 0%
  unless quantity >= 10 then 10%
  unless quantity >= 50 then 15%
  unless is_member then 20%
rule discount_amount: base_price * discount
rule discounted_price: base_price - discount_amount
rule vat: discounted_price * 21%
rule total: discounted_price + vat
"#;

fn missing_data_union(response: &lemma::Response) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut names = Vec::new();
    for result in response.results.values() {
        for key in result.missing_data() {
            if seen.insert(key.clone()) {
                names.push(key.clone());
            }
        }
    }
    names
}

#[test]
fn show_includes_prefilled_value_in_live_unless() {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, PRICING_SPEC.to_string())])
        .expect("pricing spec must load");
    let now = DateTimeValue::now();
    let show = engine
        .show(None, "pricing", Some(&now))
        .expect("show must succeed");

    assert!(
        show.data.contains_key("is_member"),
        "is_member must appear even when spec prefills false: {:?}",
        show.data.keys().collect::<Vec<_>>()
    );
    assert!(
        show.data["is_member"].fill.is_some(),
        "is_member must carry fill from spec literal"
    );
}

#[test]
fn show_includes_overridable_prefilled_value_in_live_unless() {
    let code = r#"
spec t
data base_price: 100
data quantity: number
rule discount: 0%
  unless base_price < 100 then 10%
rule total: quantity
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("spec must load");
    let now = DateTimeValue::now();
    let show = engine
        .show(None, "t", Some(&now))
        .expect("show must succeed");

    assert!(
        show.data.contains_key("base_price"),
        "base_price must appear when unless arm can become live via override: {:?}",
        show.data.keys().collect::<Vec<_>>()
    );
    let entry = show
        .data
        .get("base_price")
        .expect("base_price entry must exist");
    assert!(
        entry.fill.is_some(),
        "base_price must show spec fill literal"
    );
}

#[test]
fn show_and_run_omit_flag_when_unless_and_is_statically_false() {
    // Catalog keeps declared data; statically dead unless arms leave needed_by_rules
    // empty and omit the name from missing_data (see show_only_rule_used_data).
    let code = r#"
spec t
data flag: boolean
rule discount: 0%
  unless flag and (1 > 2) then 20%
rule total: 1
"#;

    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("spec must load");
    let now = DateTimeValue::now();
    let show = engine
        .show(None, "t", Some(&now))
        .expect("show must succeed");

    let flag = show
        .data
        .get("flag")
        .expect("flag must remain in show catalog when declared");
    assert!(
        flag.needed_by_rules.is_empty(),
        "statically false unless must leave flag with empty needed_by_rules: {:?}",
        flag.needed_by_rules
    );

    let response = engine
        .run(
            None,
            "t",
            Some(&now),
            HashMap::new(),
            Some(&["discount".to_string()]),
            false,
        )
        .expect("run must succeed");
    let names = missing_data_union(&response);
    assert!(
        !names.contains(&"flag".to_string()),
        "flag must not appear in missing_data when unless is statically false: {names:?}"
    );
}

#[test]
fn prefilled_is_member_bound_not_in_missing_data() {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, PRICING_SPEC.to_string())])
        .expect("pricing spec must load");
    let now = DateTimeValue::now();
    let mut inputs = HashMap::new();
    inputs.insert("quantity".to_string(), "5".into());

    let show = engine
        .show(None, "pricing", Some(&now))
        .expect("show must succeed");
    assert!(
        show.data.contains_key("is_member"),
        "is_member must stay in show when unless arm may still apply"
    );
    assert!(
        show.data["is_member"].fill.is_some(),
        "is_member must carry fill in show"
    );

    let response = engine
        .run(
            None,
            "pricing",
            Some(&now),
            inputs,
            Some(&["discount".to_string()]),
            false,
        )
        .expect("evaluation must succeed");

    let names = missing_data_union(&response);
    assert!(
        !names.contains(&"is_member".to_string()),
        "filled is_member is bound and must not appear in missing_data: {names:?}"
    );
    assert!(
        !names.contains(&"quantity".to_string()),
        "supplied quantity must not appear in missing_data: {names:?}"
    );
}

#[test]
fn eval_honors_supplied_override_for_unless_arm() {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, PRICING_SPEC.to_string())])
        .expect("pricing spec must load");
    let now = DateTimeValue::now();

    let default_show = engine
        .show(None, "pricing", Some(&now))
        .expect("show must succeed");
    assert!(default_show.data.contains_key("is_member"));

    let mut inputs = HashMap::new();
    inputs.insert("quantity".to_string(), "5".into());
    inputs.insert("is_member".to_string(), "true".into());

    let response = engine
        .run(
            None,
            "pricing",
            Some(&now),
            inputs,
            Some(&["discount".to_string()]),
            false,
        )
        .expect("evaluation must succeed");

    let discount = response
        .results
        .get("discount")
        .expect("discount must be present");
    assert_eq!(
        discount.result(),
        Some("20%"),
        "supplied override is_member true must activate member unless arm"
    );
}
