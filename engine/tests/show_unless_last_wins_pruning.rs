//! `run` missing-data discovery must mirror last-match-wins unless semantics
//! (`compile_piecewise_rule` evaluates unless arms in reverse source order).

use lemma::{DateGranularity, DateTimeValue, Engine, TimezoneValue};
use std::collections::HashMap;

const FILM_ACCESS: &str = r#"
spec premium_membership
uses lemma units
data start: date
data length: units.calendar
rule valid: now in start...start + length

spec film_access
uses premium_membership
data type: text
  -> option "rental"
  -> option "purchase"
data views_consumed: number
data premium_member: boolean
rule max_views: 3
  unless premium_membership.valid then 10
  unless premium_member then 5
rule can_view: no
  unless type is "rental" and views_consumed < max_views then yes
  unless type is "purchase" then yes
"#;

fn effective_2027() -> DateTimeValue {
    DateTimeValue {
        year: 2027,
        month: 2,
        day: 14,
        hour: 12,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: Some(TimezoneValue {
            offset_hours: 0,
            offset_minutes: 0,
        }),
        granularity: DateGranularity::DateTime,
    }
}

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

fn run_can_view(engine: &Engine, inputs: HashMap<String, String>) -> lemma::Response {
    let effective = effective_2027();
    engine
        .run(
            None,
            "film_access",
            Some(&effective),
            inputs,
            Some(&["can_view".to_string()]),
            false,
        )
        .expect("film_access run must succeed")
}

#[test]
fn run_omits_membership_dates_when_premium_member_true() {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, FILM_ACCESS.to_string())])
        .expect("film_access spec must load");

    let names = missing_data_union(&run_can_view(
        &engine,
        HashMap::from([
            ("type".to_string(), "rental".into()),
            ("views_consumed".to_string(), "6".into()),
            ("premium_member".to_string(), "true".into()),
        ]),
    ));

    assert!(
        !names.contains(&"premium_membership.start".to_string()),
        "start must not appear when last unless (premium_member) is true: {names:?}"
    );
    assert!(
        !names.contains(&"premium_membership.length".to_string()),
        "length must not appear when last unless (premium_member) is true: {names:?}"
    );
}

#[test]
fn run_includes_membership_dates_when_premium_member_false() {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, FILM_ACCESS.to_string())])
        .expect("film_access spec must load");

    let names = missing_data_union(&run_can_view(
        &engine,
        HashMap::from([
            ("type".to_string(), "rental".into()),
            ("views_consumed".to_string(), "6".into()),
            ("premium_member".to_string(), "false".into()),
        ]),
    ));

    assert!(
        names.contains(&"premium_membership.start".to_string()),
        "start must appear when premium_member is false and valid unless may win"
    );
    assert!(
        names.contains(&"premium_membership.length".to_string()),
        "length must appear when premium_member is false and valid unless may win"
    );
}

#[test]
fn run_includes_membership_dates_when_premium_member_unknown() {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, FILM_ACCESS.to_string())])
        .expect("film_access spec must load");

    let names = missing_data_union(&run_can_view(
        &engine,
        HashMap::from([
            ("type".to_string(), "rental".into()),
            ("views_consumed".to_string(), "6".into()),
        ]),
    ));

    assert!(
        names.contains(&"premium_member".to_string()),
        "premium_member must appear when last unless outcome is unknown"
    );
    assert!(
        names.contains(&"premium_membership.start".to_string()),
        "start must appear when premium_member is unknown"
    );
    assert!(
        names.contains(&"premium_membership.length".to_string()),
        "length must appear when premium_member is unknown"
    );
}
