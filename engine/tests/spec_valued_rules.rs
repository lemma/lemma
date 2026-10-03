//! A rule may return a spec instance (`uses` alias), and dot reads that instance.

use lemma::result_value::{RuleResultValue, SpecResult};
use lemma::{DateTimeValue, Engine, Error, SourceType};
use rust_decimal::Decimal;
use std::collections::HashMap;

const BRACKETS: &str = r#"
spec tax_bracket

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data income: money
data band:   money range
data rate:   ratio
data extra:  number


rule taxable:
  band as eur
  unless income < upper band then income - lower band
  unless income <= lower band then 0 eur

rule tax:
  taxable * rate

rule surcharge:
  extra * 1 eur


spec income_tax

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data income: money

uses basic: tax_bracket
  -> with band: 0 eur...38000 eur
  -> with rate: 36%
  -> with income: income
  -> with extra: 0

uses higher: tax_bracket
  -> with band: 38000 eur...76000 eur
  -> with rate: 37%
  -> with income: income


rule top_bracket:
  basic
  unless income >= 38000 eur then higher

rule marginal_rate:
  top_bracket.rate

rule top_bracket_tax:
  top_bracket.tax

rule top_bracket_surcharge:
  top_bracket.surcharge

rule b2:
  top_bracket

rule b2_tax:
  b2.tax
"#;

fn now() -> DateTimeValue {
    DateTimeValue {
        year: 2026,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: None,
        granularity: lemma::DateGranularity::Full,
    }
}

fn engine(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, code.to_string())])
        .expect("load");
    engine
}

fn run(
    engine: &Engine,
    spec: &str,
    rule: &str,
    data: HashMap<String, String>,
) -> lemma::RuleResult {
    run_owned(engine, spec, rule, data)
}

fn run_owned(
    engine: &Engine,
    spec: &str,
    rule: &str,
    data: HashMap<String, String>,
) -> lemma::RuleResult {
    let response = engine
        .run(
            None,
            spec,
            Some(&now()),
            data,
            Some(&[rule.to_string()]),
            false,
        )
        .unwrap_or_else(|error| panic!("run {spec}.{rule}: {error}"));
    response.get(rule).expect(rule).clone()
}

fn load_error(code: &str) -> String {
    let mut engine = Engine::new();
    let errors = engine
        .load([(SourceType::Volatile, code.to_string())])
        .expect_err("expected planning error");
    errors
        .iter()
        .map(|error| match error {
            Error::Validation(details) => details.message.clone(),
            other => other.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn income(amount: &str) -> HashMap<String, String> {
    HashMap::from([("income".to_string(), amount.to_string())])
}

fn flag(value: bool) -> HashMap<String, String> {
    HashMap::from([("pick_high".to_string(), value.to_string())])
}

fn rule_text<'a>(spec: &'a SpecResult, rule: &str) -> Option<&'a str> {
    spec.rules
        .get(rule)
        .and_then(|rule| rule.result.as_ref())
        .and_then(|value| value.result.as_deref())
}

fn spec_of(result: &lemma::RuleResult) -> &SpecResult {
    result
        .result
        .as_ref()
        .and_then(|value| value.spec.as_deref())
        .unwrap_or_else(|| panic!("expected a spec result, vetoed={}", result.vetoed))
}

#[test]
fn bare_alias_returns_the_basic_instance() {
    let engine = engine(BRACKETS);
    let result = run(&engine, "income_tax", "top_bracket", income("30000 eur"));
    assert!(
        !result.vetoed,
        "{}",
        result.veto_reason.as_deref().unwrap_or("")
    );
    let spec = spec_of(&result);
    assert_eq!(spec.spec, "tax_bracket");
    assert_eq!(spec.instance, "basic");
    let income_entry = spec.data.get("income").expect("income data entry");
    assert!(!income_entry.vetoed);
    let income_value = income_entry.result.as_ref().expect("income value");
    let income_shown = income_value.result.as_deref().unwrap_or("");
    assert!(
        income_shown.contains("30000") && income_shown.contains("eur"),
        "income value, got {income_shown:?}"
    );
    let rate_entry = spec.data.get("rate").expect("rate data entry");
    assert!(!rate_entry.vetoed);
    let rate_shown = rate_entry
        .result
        .as_ref()
        .and_then(|value| value.result.as_deref())
        .unwrap_or("");
    assert!(rate_shown.contains("36"), "rate value, got {rate_shown:?}");
    let tax = spec.rules.get("tax").expect("tax rule");
    assert!(!tax.vetoed);
    let shown = tax
        .result
        .as_ref()
        .and_then(|value| value.result.as_deref())
        .unwrap_or("");
    assert!(
        shown.contains("10800"),
        "basic tax of 30000 eur at 36% with extra 0, got {shown}"
    );
}

#[test]
fn dot_reads_the_chosen_instances_rule_and_data() {
    let engine = engine(BRACKETS);
    let low = run(
        &engine,
        "income_tax",
        "top_bracket_tax",
        income("30000 eur"),
    );
    let high = run(
        &engine,
        "income_tax",
        "top_bracket_tax",
        income("50000 eur"),
    );
    let low_shown = low.result().unwrap_or("");
    let high_shown = high.result().unwrap_or("");
    assert!(low_shown.contains("10800"), "basic tax, got {low_shown}");
    assert!(
        high_shown.contains("4440"),
        "higher tax 12000 * 37%, got {high_shown}"
    );
    let rate = run(&engine, "income_tax", "marginal_rate", income("50000 eur"));
    let rate_shown = rate.result().unwrap_or("");
    assert!(
        rate_shown.contains("37") || rate_shown.contains("0.37"),
        "higher rate, got {rate_shown}"
    );
}

#[test]
fn chained_spec_rule_reads_the_same_field() {
    let engine = engine(BRACKETS);
    let direct = run(
        &engine,
        "income_tax",
        "top_bracket_tax",
        income("30000 eur"),
    );
    let chained = run(&engine, "income_tax", "b2_tax", income("30000 eur"));
    assert_eq!(direct.result(), chained.result());
}

#[test]
fn dot_reads_a_spec_returned_by_a_rule_field() {
    let code = r#"
spec priced

data label: text

rule name:
  label


spec shelf

data pick_high: boolean

uses low: priced
  -> with label: "latte"

uses high: priced
  -> with label: "mocha"

rule item:
  low
  unless pick_high then high


spec order

data pick_high: boolean

uses rack: shelf
  -> with pick_high: pick_high

rule shelf_rule:
  rack

rule chosen:
  shelf_rule.item

rule label:
  chosen.name
"#;
    let engine = engine(code);
    let low = run(&engine, "order", "label", flag(false));
    assert_eq!(low.result(), Some("latte"));
    let low_chosen = run(&engine, "order", "chosen", flag(false));
    let low_spec = spec_of(&low_chosen);
    assert_eq!(low_spec.spec, "priced");
    assert_eq!(low_spec.instance, "rack.low");
    assert_eq!(rule_text(low_spec, "name"), Some("latte"));

    let high = run(&engine, "order", "label", flag(true));
    assert_eq!(high.result(), Some("mocha"));
    let high_chosen = run(&engine, "order", "chosen", flag(true));
    let high_spec = spec_of(&high_chosen);
    assert_eq!(high_spec.spec, "priced");
    assert_eq!(high_spec.instance, "rack.high");
    assert_eq!(rule_text(high_spec, "name"), Some("mocha"));

    let missing = run(&engine, "order", "label", HashMap::new());
    assert!(missing.vetoed);
    assert!(
        missing
            .missing_data()
            .iter()
            .any(|key| key.contains("pick_high")),
        "missing_data {:?}, expected pick_high",
        missing.missing_data()
    );
}

#[test]
fn dot_walks_the_instances_own_uses_alias() {
    let code = r#"
spec priced

data label: text

rule name:
  label


spec wrap

uses item: priced
  -> with label: "latte"


spec order

uses box: wrap

rule picked:
  box

rule label:
  picked.item.name
"#;
    let engine = engine(code);
    let result = run(&engine, "order", "label", HashMap::new());
    assert_eq!(result.result(), Some("latte"));
}

#[test]
fn missing_income_vetoes_the_spec_and_the_field() {
    let engine = engine(BRACKETS);
    let response = engine
        .run(
            None,
            "income_tax",
            Some(&now()),
            HashMap::new(),
            Some(&["top_bracket".to_string(), "top_bracket_tax".to_string()]),
            false,
        )
        .expect("run");
    for rule in ["top_bracket", "top_bracket_tax"] {
        let result = response.get(rule).expect(rule);
        assert!(result.vetoed, "{rule} should veto");
        assert!(
            result.missing_data().iter().any(|key| key == "income"),
            "{rule} missing_data {:?}, expected income",
            result.missing_data()
        );
    }
}

#[test]
fn unchosen_instance_missing_data_is_not_reported() {
    let engine = engine(BRACKETS);
    let chosen_basic = engine
        .run(
            None,
            "income_tax",
            Some(&now()),
            income("30000 eur"),
            Some(&["top_bracket_surcharge".to_string()]),
            false,
        )
        .expect("run");
    let basic = chosen_basic.get("top_bracket_surcharge").expect("rule");
    assert!(
        !basic.vetoed,
        "basic fills extra, higher.extra must not veto: {}",
        basic.veto_reason.as_deref().unwrap_or("")
    );
    assert!(
        basic.missing_data().is_empty(),
        "unchosen higher.extra leaked: {:?}",
        basic.missing_data()
    );

    let chosen_higher = engine
        .run(
            None,
            "income_tax",
            Some(&now()),
            income("50000 eur"),
            Some(&["top_bracket_surcharge".to_string()]),
            false,
        )
        .expect("run");
    let higher = chosen_higher.get("top_bracket_surcharge").expect("rule");
    assert!(higher.vetoed);
    assert!(
        higher
            .missing_data()
            .iter()
            .any(|key| key.contains("higher") && key.contains("extra")),
        "expected higher.extra, got {:?}",
        higher.missing_data()
    );
}

#[test]
fn spec_value_is_veto_when_the_rule_vetoes() {
    let loaded = engine(BRACKETS);
    let response = loaded
        .run(
            None,
            "income_tax",
            Some(&now()),
            HashMap::new(),
            Some(&["top_bracket".to_string()]),
            false,
        )
        .expect("run");
    let vetoed = response.get("top_bracket").expect("rule");
    assert!(vetoed.vetoed);

    let loaded = engine(&format!(
        "{BRACKETS}\nrule bracket_vetoed:\n  top_bracket is veto\n"
    ));
    let result = run(&loaded, "income_tax", "bracket_vetoed", HashMap::new());
    assert_eq!(result.result(), Some("true"));
    let present = run(&loaded, "income_tax", "bracket_vetoed", income("30000 eur"));
    assert_eq!(present.result(), Some("false"));
}

#[test]
fn round_trip_runs_the_spec_with_emitted_data() {
    let engine = engine(BRACKETS);
    let show = engine
        .show(None, "tax_bracket", Some(&now()))
        .expect("show tax_bracket");
    for amount in ["30000 eur", "50000 eur"] {
        let result = run(&engine, "income_tax", "top_bracket", income(amount));
        let spec = spec_of(&result);
        let mut replay = HashMap::new();
        for (name, entry) in &spec.data {
            assert!(
                !entry.vetoed,
                "{name} data entry vetoed on {amount}: {:?}",
                entry.veto_reason
            );
            let value = entry.result.as_ref().expect("data entry value");
            let show_type = &show
                .data
                .get(name)
                .unwrap_or_else(|| panic!("show lacks {name}"))
                .lemma_type;
            let first = value.to_literal(show_type);
            let again = value.to_literal(show_type);
            assert_eq!(first, again, "{name} to_literal unstable on {amount}");
            replay.insert(name.clone(), data_entry_input(value));
        }
        let again = engine
            .run(None, "tax_bracket", Some(&now()), replay, None, false)
            .unwrap_or_else(|error| panic!("re-run tax_bracket: {error}"));
        for (name, original) in &spec.rules {
            let replayed = again.get(name).unwrap_or_else(|_| panic!("missing {name}"));
            let original_shown = original
                .result
                .as_ref()
                .and_then(|value| value.result.clone());
            assert_eq!(
                replayed.vetoed, original.vetoed,
                "{name} veto mismatch on {amount}"
            );
            if !original.vetoed {
                assert_eq!(
                    replayed.result().map(str::to_string),
                    original_shown,
                    "{name} value mismatch on {amount}"
                );
            }
        }
    }
}

fn data_entry_input(value: &RuleResultValue) -> String {
    if let Some(range) = &value.range {
        return format!(
            "{}...{}",
            data_entry_input(&range.from),
            data_entry_input(&range.to)
        );
    }
    if let Some(measure) = &value.measure {
        let unit = value
            .unit
            .as_deref()
            .expect("measure data entry needs written unit");
        let magnitude = measure.get(unit).expect("written unit in measure map");
        return format!("{magnitude} {unit}");
    }
    if let Some(ratio) = &value.ratio {
        let unit = value
            .unit
            .as_deref()
            .expect("ratio data entry needs written unit");
        let magnitude = ratio.get(unit).expect("written unit in ratio map");
        return format!("{magnitude} {unit}");
    }
    if let Some(number) = &value.number {
        return number.to_string();
    }
    if let Some(boolean) = &value.boolean {
        return boolean.to_string();
    }
    if let Some(text) = &value.text {
        return text.clone();
    }
    if let Some(display) = &value.result {
        return display.clone();
    }
    panic!("data entry has no replayable value");
}

#[test]
fn vetoed_data_appears_as_a_veto_entry() {
    let code = r#"
spec item

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data price: money -> minimum 1 eur

rule cost:
  price * 2


spec cart

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data budget: money -> minimum 1 eur

uses deal: item
  -> with price: budget

rule chosen:
  deal

rule total:
  deal.cost
"#;
    let engine = engine(code);
    let result = run(
        &engine,
        "cart",
        "chosen",
        HashMap::from([("budget".to_string(), "0 eur".to_string())]),
    );
    let spec = spec_of(&result);
    let price = spec.data.get("price").expect("price data entry");
    assert!(price.vetoed, "price entry should veto on 0 eur");
    let reason = price.veto_reason.as_deref().unwrap_or("");
    assert!(
        reason.contains("0") && reason.contains("below minimum 1.00 eur"),
        "constraint reason, got {reason:?}"
    );
    let cost = spec.rules.get("cost").expect("cost rule");
    assert!(cost.vetoed);
    assert_eq!(cost.veto_reason.as_deref(), price.veto_reason.as_deref());
}

#[test]
fn branches_of_different_spec_versions_are_a_planning_error() {
    let message = load_error(
        r#"
spec item
data n: number

spec item 2025-06-01
data n: text

spec pick
data flag: boolean
uses early: item
uses late: item 2025-06-01
rule chosen:
  early
  unless flag then late
"#,
    );
    assert!(
        message.contains("item") && (message.contains("2025") || message.contains("text")),
        "{message}"
    );
}

#[test]
fn branches_of_different_specs_are_a_planning_error() {
    let message = load_error(
        r#"
spec left
data n: number

spec right
data n: number

spec pick
data flag: boolean
uses a: left
uses b: right
rule chosen:
  a
  unless flag then b
"#,
    );
    assert!(
        message.contains("same") || message.contains("spec") || message.contains("type"),
        "{message}"
    );
    assert!(
        message.contains("left") && message.contains("right"),
        "{message}"
    );
}

#[test]
fn arithmetic_on_a_spec_is_a_planning_error() {
    let message = load_error(
        r#"
spec item
data n: number
spec holder
uses item
rule bad:
  item + 1
"#,
    );
    assert!(
        message.contains("spec instance") || message.contains("not a value"),
        "{message}"
    );
}

#[test]
fn comparing_specs_is_a_planning_error() {
    let message = load_error(
        r#"
spec item
data n: number
spec holder
uses a: item
uses b: item
rule bad:
  a is b
"#,
    );
    assert!(
        message.contains("spec instance") || message.contains("not a value"),
        "{message}"
    );
}

#[test]
fn spec_value_as_unless_condition_is_a_planning_error() {
    let message = load_error(
        r#"
spec item
data n: number
spec holder
uses item
data flag: boolean
rule bad:
  flag
  unless item then false
"#,
    );
    assert!(message.contains("boolean"), "{message}");
}

#[test]
fn unknown_member_is_a_planning_error() {
    let message = load_error(&format!(
        "{BRACKETS}\nrule missing_field:\n  top_bracket.nope\n"
    ));
    assert!(message.contains("nope"), "{message}");
}

#[test]
fn spec_instance_as_with_value_is_a_planning_error() {
    let message = load_error(
        r#"
spec item
data n: number
spec holder
uses item
uses other: item
  -> with n: item
"#,
    );
    assert!(
        message.contains("spec instance") || message.contains("cannot supply"),
        "{message}"
    );
}

#[test]
fn explanation_names_the_chosen_instance_and_its_rule() {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, BRACKETS.to_string())])
        .expect("load");
    let response = engine
        .run(
            None,
            "income_tax",
            Some(&now()),
            income("50000 eur"),
            Some(&["top_bracket_tax".to_string()]),
            true,
        )
        .expect("run");
    let result = response.get("top_bracket_tax").expect("rule");
    let explanation = format!("{:?}", result.explanation.as_ref().expect("explanation"));
    assert!(
        explanation.contains("higher"),
        "chosen instance missing: {explanation}"
    );
    assert!(
        explanation.contains("tax"),
        "field rule missing: {explanation}"
    );
}

#[test]
fn decimal_tax_matches_exact_ratio_product() {
    let engine = engine(BRACKETS);
    let low = run(
        &engine,
        "income_tax",
        "top_bracket_tax",
        income("30000 eur"),
    );
    let literal = low.to_literal();
    let magnitude = literal
        .value
        .as_decimal_magnitude()
        .expect("money magnitude");
    assert_eq!(magnitude, Decimal::from(10800));
}

#[test]
fn dropped_rule_does_not_report_nonexistent_reference() {
    let errors = load_error(
        r#"
spec dep
data v: 1
rule r: v + nope

spec consumer
uses d: dep
rule out: d.r
"#,
    );
    assert!(
        errors.contains("nope") && errors.contains("not found"),
        "{errors}"
    );
    assert!(!errors.contains("non-existent rule"), "{errors}");
}

#[test]
fn dropped_spec_valued_base_has_no_follow_on_field_errors() {
    let errors = load_error(
        r#"
spec inner
data n: number
rule r: n

spec outer
data n: number
uses i: inner
rule chosen: i + nope
rule out: chosen.r
"#,
    );
    assert!(
        errors.contains("nope") && errors.contains("not found"),
        "{errors}"
    );
    assert!(!errors.contains("does not return a spec"), "{errors}");
    assert!(!errors.contains("no data or rule"), "{errors}");
}

#[test]
fn rule_target_data_reference_to_dropped_rule_is_error() {
    let errors = load_error(
        r#"
spec dep
data v: 1
rule r: v + nope

spec consumer
uses d: dep
data x: d.r
rule out: x
"#,
    );
    assert!(
        errors.contains("nope") && errors.contains("not found"),
        "{errors}"
    );
}

#[test]
fn dropped_rule_and_type_error_are_reported_together() {
    let errors = load_error(
        r#"
spec s
data n: number
rule a: n + nope
rule b: n + true
"#,
    );
    assert!(
        errors.contains("nope") && errors.contains("not found"),
        "{errors}"
    );
    assert!(errors.contains("Cannot apply '+'"), "{errors}");
}
