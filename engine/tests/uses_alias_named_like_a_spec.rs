//! Inside a spec only the local names of its `uses` lines exist. `uses a: b` names
//! spec `b` as its target, even when another line `uses b: c` gives the local name `b`
//! to spec `c`. Everywhere else in the spec (list elements, member reads) a name is a
//! local name.

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, Errors, SourceType};

fn load(code: &str) -> Result<Engine, Errors> {
    let mut engine = Engine::new();
    engine.load([(
        SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "local_names.lemma",
        ))),
        code.to_string(),
    )])?;
    Ok(engine)
}

fn rule_value(engine: &Engine, spec: &str, rule: &str) -> String {
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, spec, Some(&now), IndexMap::new(), None, false)
        .unwrap_or_else(|error| panic!("{spec} evaluates: {error}"));
    response
        .results
        .values()
        .find(|result| result.rule.name == rule)
        .unwrap_or_else(|| panic!("rule `{rule}` in response"))
        .result()
        .expect("rule has a value")
        .to_string()
}

#[test]
fn list_element_names_the_row_spec_through_its_local_name() {
    let engine = load(
        r#"
spec employee
data salary: number

spec team
uses e: employee
data staff: e list
  -> with salary: 10
  -> with salary: 20
rule headcount: staff -> count
"#,
    )
    .expect("`e` is the local name of `employee`");
    assert_eq!(rule_value(&engine, "team", "headcount"), "2");
}

#[test]
fn list_element_naming_a_spec_without_a_local_name_is_rejected() {
    load(
        r#"
spec employee
data salary: number

spec team
uses e: employee
data staff: employee list
  -> with salary: 10
"#,
    )
    .err()
    .expect("`employee` is no name inside `team`; only `e` is");
}

#[test]
fn list_element_naming_a_spec_that_is_not_imported_is_rejected() {
    load(
        r#"
spec employee
data salary: number

spec team
data staff: employee list
  -> with salary: 10
"#,
    )
    .err()
    .expect("`team` has no `uses` line for `employee`");
}

#[test]
fn uses_alias_named_like_an_imported_spec_resolves_its_own_target() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "alias_names.lemma",
            ))),
            r#"
spec c
rule r: 3

spec b
rule r: 2

spec app
uses a: b
uses b: c
rule from_a: a.r
rule from_b: b.r
"#
            .to_string(),
        )])
        .expect("`a` is spec `b`, `b` is spec `c`");

    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, "app", Some(&now), IndexMap::new(), None, false)
        .expect("app evaluates");
    let value = |name: &str| {
        response
            .results
            .values()
            .find(|result| result.rule.name == name)
            .unwrap_or_else(|| panic!("rule `{name}` in response"))
            .result()
            .expect("rule has a value")
            .to_string()
    };
    assert_eq!(value("from_a"), "2");
    assert_eq!(value("from_b"), "3");
}
