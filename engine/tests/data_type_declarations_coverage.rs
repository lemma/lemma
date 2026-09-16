//! QA coverage for parse-time `DataValue::Definition` (constraints / literals).
//!
//! Matrix: every primitive keyword x applicable-vs-incompatible constraint.
//! Named-typedef references: happy + unknown + name-collision with rule.
//! Qualified parent types (`uses` plus `data x: alias.TypeName`): happy + unknown + errors.

use lemma::DateTimeValue;
use lemma::Engine;
use std::collections::HashMap;

fn load_ok(engine: &mut Engine, code: &str) {
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("types.lemma"))),
            code.to_string(),
        )])
        .unwrap_or_else(|errs| {
            let joined = errs
                .iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            panic!("expected load to succeed, got: {joined}");
        });
}

fn load_err_joined(engine: &mut Engine, code: &str) -> String {
    let err = engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("types.lemma"))),
            code.to_string(),
        )])
        .expect_err("expected load to fail");
    err.iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn rule_value(result: &lemma::Response, name: &str) -> String {
    let rr = result
        .results
        .get(name)
        .unwrap_or_else(|| panic!("rule '{}' not found", name));
    if rr.vetoed {
        format!("VETO({})", rr.veto_reason.as_deref().unwrap_or("Vetoed"))
    } else {
        rr.display().expect("display").to_string()
    }
}

fn run(
    engine: &Engine,
    spec: &str,
    data: HashMap<String, String>,
) -> Result<lemma::Response, lemma::Error> {
    let now = DateTimeValue::now();
    engine.run(None, spec, Some(&now), data, None, false)
}

fn assert_awaits_missing(rr: &lemma::RuleResult, key: &str) {
    assert!(rr.vetoed, "unbound {key} must veto, got {:?}", rr.display());
    assert!(
        rr.awaits_missing_data(),
        "unbound {key} must be MissingData, got {:?}",
        rr.veto_reason
    );
    assert_eq!(
        rr.missing_data(),
        [key.to_string()].as_slice(),
        "missing_data must list only {key}"
    );
    let expected = format!("Missing data: {key}");
    assert_eq!(
        rr.veto_reason.as_deref(),
        Some(expected.as_str()),
        "MissingData reason must name {key}"
    );
}

fn assert_duplicate_constraint_error(joined: &str, command: &str) {
    assert!(
        joined.contains(&format!("Duplicate '{command}' constraint"))
            && joined.contains("at most once"),
        "duplicate -> {command} must say Duplicate/at most once, got: {joined}"
    );
}

fn assert_inverted_bounds_error(joined: &str, type_name: &str, min: &str, max: &str) {
    let expected = format!(
        "Type '{type_name}' has invalid range: minimum {min} is greater than maximum {max}"
    );
    assert!(
        joined.contains(&expected),
        "inverted min/max must report {expected}, got: {joined}"
    );
}

fn fraction_ones(n: usize) -> String {
    "1".repeat(n)
}

// ─── Type-only data + missing at runtime → MissingData veto ──────────

#[test]
fn primitive_number_type_only_missing_vetoes() {
    let code = r#"
spec s
data x: number
rule r: x
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let resp = run(&engine, "s", HashMap::new()).expect("evaluates");
    assert_awaits_missing(resp.results.get("r").expect("r"), "x");
}

#[test]
fn primitive_text_type_only_missing_vetoes() {
    let code = r#"
spec s
data x: text
rule r: x
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let resp = run(&engine, "s", HashMap::new()).expect("evaluates");
    assert_awaits_missing(resp.results.get("r").expect("r"), "x");
}

#[test]
fn primitive_boolean_type_only_missing_vetoes() {
    let code = r#"
spec s
data b: boolean
rule r: b
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let resp = run(&engine, "s", HashMap::new()).expect("evaluates");
    assert_awaits_missing(resp.results.get("r").expect("r"), "b");
}

#[test]
fn primitive_date_type_only_missing_vetoes() {
    let code = r#"
spec s
data d: date
rule r: d
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let resp = run(&engine, "s", HashMap::new()).expect("evaluates");
    assert_awaits_missing(resp.results.get("r").expect("r"), "d");
}

#[test]
fn lemma_typedef_duration_missing_data_vetoes() {
    let code = r#"
spec s
uses lemma units
data d: units.duration
rule r: d
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let resp = run(&engine, "s", HashMap::new()).expect("evaluates");
    assert_awaits_missing(resp.results.get("r").expect("r"), "d");
}

#[test]
fn primitive_percent_type_only_missing_vetoes() {
    let code = r#"
spec s
data p: ratio
rule r: p
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let resp = run(&engine, "s", HashMap::new()).expect("evaluates");
    assert_awaits_missing(resp.results.get("r").expect("r"), "p");
}

// ─── Constraint × primitive compatibility matrix ─────────────────────

// `minimum` / `maximum` on number: valid, enforced
#[test]
fn number_minimum_enforces_on_user_value() {
    let code = r#"
spec s
data n: number -> minimum 10
rule r: n
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let mut data = HashMap::new();
    data.insert("n".to_string(), "5".to_string());
    let resp = run(&engine, "s", data).expect("5 < 10 must complete with veto");
    let rr = resp.results.get("r").expect("rule r");
    assert!(rr.vetoed, "5 < 10 must veto rule r");
    assert_eq!(
        rr.veto_reason.as_deref(),
        Some("Data n [number]: 5 is below minimum 10"),
        "got: {:?}",
        rr.veto_reason
    );
}

#[test]
fn number_maximum_enforces_on_user_value() {
    let code = r#"
spec s
data n: number -> maximum 5
rule r: n
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let mut data = HashMap::new();
    data.insert("n".to_string(), "10".to_string());
    let resp = run(&engine, "s", data).expect("10 > 5 must complete with veto");
    let rr = resp.results.get("r").expect("rule r");
    assert!(rr.vetoed, "10 > 5 must veto rule r");
    assert_eq!(
        rr.veto_reason.as_deref(),
        Some("Data n [number]: 10 is above maximum 5"),
        "got: {:?}",
        rr.veto_reason
    );
}

// `minimum` on text: INCOMPATIBLE — must be rejected at plan time
#[test]
fn text_minimum_constraint_is_rejected() {
    let code = r#"
spec s
data x: text -> minimum 5
rule r: x
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        !joined.is_empty() && (joined.contains("minimum") || joined.contains("text")),
        "text does not support `minimum`; must be rejected, got: {joined}"
    );
}

// `decimals` on boolean: INCOMPATIBLE
#[test]
fn boolean_decimals_constraint_is_rejected() {
    let code = r#"
spec s
data b: boolean -> decimals 2
rule r: b
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        !joined.is_empty() && (joined.contains("decimals") || joined.contains("boolean")),
        "boolean does not support `decimals`; must be rejected, got: {joined}"
    );
}

// `decimals` on text: INCOMPATIBLE
#[test]
fn text_decimals_constraint_is_rejected() {
    let code = r#"
spec s
data x: text -> decimals 2
rule r: x
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        !joined.is_empty() && (joined.contains("decimals") || joined.contains("text")),
        "text does not support `decimals`; must be rejected, got: {joined}"
    );
}

// `unit` on date: INCOMPATIBLE
#[test]
fn date_unit_constraint_is_rejected() {
    let code = r#"
spec s
data d: date -> unit meter: 1
rule r: d
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        !joined.is_empty() && (joined.contains("unit") || joined.contains("date")),
        "date does not support `unit`; must be rejected, got: {joined}"
    );
}

// `length` on text: VALID
#[test]
fn text_length_constraint_enforces_on_user_value() {
    let code = r#"
spec s
data msg: text -> length 5
rule r: msg
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let mut data = HashMap::new();
    data.insert("msg".to_string(), "way too long".to_string());
    let resp = run(&engine, "s", data).expect("length 5 must complete with veto");
    let rr = resp.results.get("r").expect("rule r");
    assert!(rr.vetoed, "length 5 must reject longer text");
    let reason = rr.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.contains("length"),
        "expected length constraint veto, got: {reason}"
    );
}

// `length` on number: INCOMPATIBLE
#[test]
fn number_length_constraint_is_rejected() {
    let code = r#"
spec s
data n: number -> length 5
rule r: n
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        !joined.is_empty() && (joined.contains("length") || joined.contains("number")),
        "number does not support `length`; must be rejected, got: {joined}"
    );
}

// ─── Suggest constraint ──────────────────────────────────────────────

#[test]
fn suggest_constraint_does_not_commit_when_missing() {
    let code = r#"
spec s
data n: number -> suggest 42
rule r: n
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let resp = run(&engine, "s", HashMap::new()).expect("evaluates");
    let rr = resp.results.get("r").unwrap();
    assert!(
        rr.vetoed,
        "suggestion must not commit; missing n must veto, got: {:?}",
        rr.veto_reason
    );
}

#[test]
fn suggest_is_overridden_by_user_value() {
    let code = r#"
spec s
data n: number -> suggest 42
rule r: n
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let mut data = HashMap::new();
    data.insert("n".to_string(), "99".to_string());
    let resp = run(&engine, "s", data).expect("evaluates");
    assert_eq!(rule_value(&resp, "r"), "99");
}

#[test]
fn suggest_that_violates_sibling_constraint_is_rejected() {
    // Suggest 3 violates `minimum 5` on the same chain.
    let code = r#"
spec s
data n: number -> suggest 3 -> minimum 5
rule r: n
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        joined.contains("suggestion") && (joined.contains("minimum") || joined.contains("3")),
        "suggestion violating minimum on same chain must be rejected, got: {joined}"
    );
}

#[test]
fn suggest_of_wrong_primitive_is_rejected() {
    let code = r#"
spec s
data n: number -> suggest "not a number"
rule r: n
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        joined.contains("suggest") && joined.contains("number"),
        "wrong-primitive suggestion must name suggest + number, got: {joined}"
    );
}

// ─── Chained constraints ─────────────────────────────────────────────

#[test]
fn chained_duplicate_minimum_is_planning_error() {
    let code = r#"
spec s
data n: number -> minimum 5 -> minimum 10
rule r: n
"#;
    let mut engine = Engine::new();
    assert_duplicate_constraint_error(&load_err_joined(&mut engine, code), "minimum");
}

#[test]
fn chained_duplicate_maximum_is_planning_error() {
    let code = r#"
spec s
data n: number -> maximum 10 -> maximum 5
"#;
    let mut engine = Engine::new();
    assert_duplicate_constraint_error(&load_err_joined(&mut engine, code), "maximum");
}

#[test]
fn chained_duplicate_decimals_is_planning_error() {
    let code = r#"
spec s
data n: number -> decimals 2 -> decimals 4
"#;
    let mut engine = Engine::new();
    assert_duplicate_constraint_error(&load_err_joined(&mut engine, code), "decimals");
}

#[test]
fn chained_duplicate_suggest_is_planning_error() {
    let code = r#"
spec s
data n: number -> suggest 1 -> suggest 2
"#;
    let mut engine = Engine::new();
    assert_duplicate_constraint_error(&load_err_joined(&mut engine, code), "suggest");
}

#[test]
fn child_overrides_inherited_minimum_parent_maximum_still_applies() {
    let code = r#"
spec s
data y: number -> minimum 5 -> maximum 10
data z: y -> minimum 3
rule r: z
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);

    let mut above_child = HashMap::new();
    above_child.insert("z".to_string(), "4".to_string());
    let resp = run(&engine, "s", above_child).expect("4 >= child min 3");
    assert_eq!(rule_value(&resp, "r"), "4");

    let mut below_child = HashMap::new();
    below_child.insert("z".to_string(), "2".to_string());
    let resp = run(&engine, "s", below_child).expect("2 < child min 3 must veto");
    let rr = resp.results.get("r").expect("rule r");
    assert!(rr.vetoed, "2 < child min 3 must veto");
    assert_eq!(
        rr.veto_reason.as_deref(),
        Some("Data z [y]: 2 is below minimum 3"),
        "child min veto must name 2 and 3, got: {:?}",
        rr.veto_reason
    );

    let mut above_parent_max = HashMap::new();
    above_parent_max.insert("z".to_string(), "11".to_string());
    let resp = run(&engine, "s", above_parent_max).expect("11 > inherited max 10 must veto");
    let rr = resp.results.get("r").expect("rule r");
    assert!(rr.vetoed, "11 > inherited max 10 must veto");
    assert_eq!(
        rr.veto_reason.as_deref(),
        Some("Data z [y]: 11 is above maximum 10"),
        "inherited max veto must name 11 and 10, got: {:?}",
        rr.veto_reason
    );
}

#[test]
fn child_overrides_inherited_maximum() {
    let code = r#"
spec s
data big_number: number -> minimum 0 -> maximum 1000
data small_number: big_number -> maximum 100
rule r: small_number
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let mut data = HashMap::new();
    data.insert("small_number".to_string(), "200".to_string());
    let resp = run(&engine, "s", data).expect("overridden max 100 must complete with veto");
    let rr = resp.results.get("r").expect("rule r");
    assert!(rr.vetoed, "overridden max 100 must reject 200");
    assert_eq!(
        rr.veto_reason.as_deref(),
        Some("Data small_number [big_number]: 200 is above maximum 100"),
        "child max veto must name 200 and 100, got: {:?}",
        rr.veto_reason
    );
}

#[test]
fn same_declaration_minimum_greater_than_maximum_is_planning_error() {
    let code = r#"
spec s
data n: number -> minimum 5 -> maximum 4
"#;
    let mut engine = Engine::new();
    assert_inverted_bounds_error(&load_err_joined(&mut engine, code), "n", "5", "4");
}

#[test]
fn child_inherit_minimum_override_maximum_inverted_is_planning_error() {
    let code = r#"
spec s
data y: number -> minimum 5
data z: y -> maximum 4
"#;
    let mut engine = Engine::new();
    assert_inverted_bounds_error(&load_err_joined(&mut engine, code), "z", "5", "4");
}

#[test]
fn child_inherit_maximum_override_minimum_inverted_is_planning_error() {
    let code = r#"
spec s
data y: number -> maximum 10
data z: y -> minimum 11
"#;
    let mut engine = Engine::new();
    assert_inverted_bounds_error(&load_err_joined(&mut engine, code), "z", "11", "10");
}

#[test]
fn child_overrides_both_bounds_to_inverted_pair_is_planning_error() {
    let code = r#"
spec s
data y: number -> minimum 0 -> maximum 100
data z: y -> minimum 8 -> maximum 7
"#;
    let mut engine = Engine::new();
    assert_inverted_bounds_error(&load_err_joined(&mut engine, code), "z", "8", "7");
}

// ─── Named typedef reference ─────────────────────────────────────────

#[test]
fn typedef_reference_resolves() {
    let code = r#"
spec s
data age: number -> minimum 0 -> maximum 150
data person_age: age
rule r: person_age
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let mut data = HashMap::new();
    data.insert("person_age".to_string(), "30".to_string());
    let resp = run(&engine, "s", data).expect("evaluates");
    assert_eq!(rule_value(&resp, "r"), "30");
}

#[test]
fn typedef_reference_inherits_constraints() {
    let code = r#"
spec s
data age: number -> minimum 0 -> maximum 150
data person_age: age
rule r: person_age
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let mut data = HashMap::new();
    data.insert("person_age".to_string(), "200".to_string());
    let resp = run(&engine, "s", data).expect("200 > 150 must complete with veto");
    let rr = resp.results.get("r").expect("rule r");
    assert!(rr.vetoed, "200 > 150 must veto via inherited max");
    assert_eq!(
        rr.veto_reason.as_deref(),
        Some("Data person_age [age]: 200 is above maximum 150"),
        "inherited max veto must name 200 and 150, got: {:?}",
        rr.veto_reason
    );
}

#[test]
fn typedef_reference_to_unknown_name_is_rejected() {
    let code = r#"
spec s
data x: nonexistent_type
rule r: x
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        joined.contains("Unknown parent") && joined.contains("nonexistent_type"),
        "unknown typedef must be reported with exact name, got: {joined}"
    );
}

/// UX LANDMINE: `data x: answer` where `answer` is a local rule currently
/// surfaces as "Unknown parent … for data definition". Users likely meant a value-copy reference.
#[test]
fn data_referencing_local_rule_name_suggests_reference_syntax() {
    let code = r#"
spec s
rule answer: 42
data x: answer
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        joined.contains("answer") && joined.contains("local rule"),
        "error must name parent 'answer' as a local rule, got: {joined}"
    );
}

#[test]
fn number_literal_at_max_fractional_digits_plans() {
    let scale = rust_decimal::Decimal::MAX_SCALE as usize;
    let frac = fraction_ones(scale);
    let literal = format!("0.{frac}");
    let code = format!("spec s\ndata n: {literal}\nrule r: n\n");
    let mut engine = Engine::new();
    load_ok(&mut engine, &code);
    let resp = run(&engine, "s", HashMap::new()).expect("evaluates");
    assert_eq!(rule_value(&resp, "r"), literal);
}

#[test]
fn number_literal_over_max_fractional_digits_is_planning_error() {
    let scale = rust_decimal::Decimal::MAX_SCALE as usize;
    let frac = fraction_ones(scale + 1);
    let literal = format!("0.{frac}");
    let code = format!("spec s\ndata n: {literal}\nrule r: n\n");
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, &code);
    assert!(
        joined.contains("too many fractional digits") && joined.contains(&format!("max {scale}")),
        "spec literal over scale must be planning Error, got: {joined}"
    );
}

#[test]
fn cross_spec_value_copy_reference_resolves() {
    let code = r#"
spec lib
data money: measure -> unit eur: 1 -> unit usd: 0.84

spec app
uses lib
data price: lib.money
rule r: price
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let mut data = HashMap::new();
    data.insert("price".to_string(), "100 eur".to_string());
    let resp = run(&engine, "app", data).expect("evaluates");
    let out = rule_value(&resp, "r");
    assert!(out.contains("100") && out.contains("eur"), "got: {out}");
}

#[test]
fn cross_spec_value_copy_unknown_data_is_rejected() {
    let code = r#"
spec lib
data money: measure -> unit eur: 1

spec app
uses lib
  -> with money: lib.nonexistent
rule r: lib.money
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        !joined.is_empty() && (joined.contains("nonexistent") || joined.contains("not found")),
        "unknown data in cross-spec value-copy reference must be rejected, got: {joined}"
    );
}

#[test]
fn cross_spec_value_copy_to_unknown_spec_is_rejected() {
    let code = r#"
spec app
uses dep: nonexistent_spec
  -> with money: 1
rule r: dep.money
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        joined.contains("nonexistent_spec"),
        "unknown spec in `uses` must name missing parent, got: {joined}"
    );
}

#[test]
fn qualified_type_with_slashed_opaque_spec_name_resolves() {
    let code = r#"
spec logistics/terms
data incoterm_type: text -> options "FOB" "CIF"

spec logistics/shipping
uses logistics/terms
data incoterm: logistics/terms.incoterm_type
rule label: incoterm
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let result = run(
        &engine,
        "logistics/shipping",
        HashMap::from([("incoterm".into(), "FOB".into())]),
    )
    .expect("run");
    assert_eq!(rule_value(&result, "label"), "FOB");
}

#[test]
fn uses_bare_terms_does_not_resolve_to_logistics_terms() {
    let code = r#"
spec logistics/terms
data incoterm_type: text

spec logistics/shipping
uses terms
data incoterm: terms.incoterm_type
rule label: incoterm
"#;
    let mut engine = Engine::new();
    let joined = load_err_joined(&mut engine, code);
    assert!(
        joined.contains("terms"),
        "bare `uses terms` must look for spec `terms`, not `logistics/terms`, got: {joined}"
    );
}

#[test]
fn show_exposes_default_help_for_each_primitive() {
    let code = r#"
spec help_defaults
uses lemma units
data flag: boolean
data n: number
data n_band: number range
data label: text
data amount: measure -> unit eur: 1
data band: measure range -> unit eur: 1
data rate: ratio
data rate_band: ratio range
data when: date
data period: date range
data clock: time
data shift: time range
rule use_flag: flag
rule use_n: n
rule use_n_band: n_band
rule use_label: label
rule use_amount: amount
rule use_band: band
rule use_rate: rate
rule use_rate_band: rate_band
rule use_when: when
rule use_period: period
rule use_clock: clock
rule use_shift: shift
"#;
    let mut engine = Engine::new();
    load_ok(&mut engine, code);
    let now = DateTimeValue::now();
    let show = engine
        .show(None, "help_defaults", Some(&now))
        .expect("show");

    let expected = [
        ("flag", "Whether this holds (true or false)."),
        ("n", "A dimensionless number."),
        ("n_band", "The lower and upper bound of the number range."),
        ("label", "A text value."),
        ("amount", "A numeric amount in one of this type's units."),
        (
            "band",
            "The lower and upper bound of the measure range in the same unit.",
        ),
        (
            "rate",
            "A ratio in one of this type's units (e.g. percent).",
        ),
        ("rate_band", "The lower and upper bound of the ratio range."),
        ("when", "A date, or a date and time with optional timezone."),
        ("period", "The start date and end date of the date range."),
        ("clock", "A time of day, with optional timezone."),
        ("shift", "The start time and end time of the time range."),
    ];
    for (name, help) in expected {
        let entry = show
            .data
            .get(name)
            .unwrap_or_else(|| panic!("missing data {name}"));
        assert_eq!(
            entry.lemma_type.specifications.help(),
            help,
            "default help for {name}"
        );
    }
}
