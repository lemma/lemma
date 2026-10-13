//! Same-engine edits of instance-list keys, row fields, and element data must
//! replan or reject through planning, never abort at planning or evaluation.

use lemma::{DateTimeValue, Engine, SourceType};

const BASE: &str = r#"spec xx
data z: number
data name: text
spec test
uses zz: xx list
  -> key z
  -> with z: 5, name: "b"
  -> with z: 7, name: "a"
rule x:
  zz -> avg z
"#;

fn evaluate(engine: &Engine, spec: &str, rule: &str) -> String {
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(
            None,
            spec,
            Some(&now),
            lemma::IndexMap::new(),
            Some(&[rule.to_string()]),
            false,
        )
        .expect("planned spec must evaluate");
    response
        .results
        .get(rule)
        .expect("rule result present")
        .result()
        .expect("rule value present")
        .to_string()
}

fn loaded_base() -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, BASE.to_string())])
        .expect("base must plan");
    assert_eq!(evaluate(&engine, "test", "x"), "6");
    engine
}

#[test]
fn renamed_key_and_row_fields_replan_without_panic() {
    let mut engine = loaded_base();
    let renamed = BASE
        .replace("data z: number", "data score: number")
        .replace("-> key z", "-> key score")
        .replace("-> with z:", "-> with score:")
        .replace("zz -> avg z", "zz -> avg score");
    engine
        .update(None, renamed, SourceType::Volatile)
        .expect("renamed key must replan");
    assert_eq!(evaluate(&engine, "test", "x"), "6");
}

#[test]
fn key_only_change_replans_without_panic() {
    let mut engine = loaded_base();
    let key_on_name = BASE.replace("-> key z", "-> key name");
    engine
        .update(None, key_on_name, SourceType::Volatile)
        .expect("key-only change must replan");
    assert_eq!(evaluate(&engine, "test", "x"), "6");
}

#[test]
fn row_value_edit_replans() {
    let mut engine = loaded_base();
    let edited = BASE.replace("-> with z: 7, name: \"a\"", "-> with z: 9, name: \"a\"");
    engine
        .update(None, edited, SourceType::Volatile)
        .expect("row edit must replan");
    assert_eq!(evaluate(&engine, "test", "x"), "7");
}

#[test]
fn renamed_element_field_still_used_by_rows_is_planning_error() {
    let mut engine = loaded_base();
    let element_renamed = BASE.replace("data name: text", "data label: text");
    let result = engine.update(None, element_renamed, SourceType::Volatile);
    assert!(
        result.is_err(),
        "rows still reading `name` must fail planning, not evaluation"
    );
    assert_eq!(evaluate(&engine, "test", "x"), "6");
}

#[test]
fn duplicate_key_introduced_is_planning_error() {
    let mut engine = loaded_base();
    let duplicated = BASE.replace(
        "  -> with z: 7, name: \"a\"",
        "  -> with z: 7, name: \"a\"\n  -> with z: 5, name: \"c\"",
    );
    let result = engine.update(None, duplicated, SourceType::Volatile);
    assert!(
        result.is_err(),
        "duplicate source key must be a planning error"
    );
    assert_eq!(evaluate(&engine, "test", "x"), "6");
}

#[test]
fn undefined_key_field_is_planning_error() {
    let mut engine = loaded_base();
    let undefined_key = BASE.replace("-> key z", "-> key nope");
    let result = engine.update(None, undefined_key, SourceType::Volatile);
    assert!(
        result.is_err(),
        "undefined key field must be a planning error"
    );
    assert_eq!(evaluate(&engine, "test", "x"), "6");
}

#[test]
fn element_key_type_change_is_planning_error() {
    let mut engine = loaded_base();
    let retyped = BASE.replace("data z: number", "data z: text");
    let result = engine.update(None, retyped, SourceType::Volatile);
    assert!(
        result.is_err(),
        "retyped element key must be a planning error"
    );
    assert_eq!(evaluate(&engine, "test", "x"), "6");
}

#[test]
fn dependency_type_change_under_element_is_planning_error() {
    let source = r#"spec dd 2025-01-01
data code: number

spec dd 2026-01-01
data code: text

spec xx
uses dd
data z: dd.code

spec test 2025-01-01
uses zz: xx list
  -> key z
  -> with z: 5
  -> with z: 7
rule x:
  zz -> count
"#;
    let mut engine = Engine::new();
    let result = engine.load([(SourceType::Volatile, source.to_string())]);
    assert!(
        result.is_err(),
        "a type change across dependency versions under a list element must fail planning"
    );
}

#[test]
fn edit_of_unrelated_consumer_keeps_list_rows_shared() {
    let source = format!("{BASE}spec report\nuses t: test\nrule y: t.x\n");
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, source.clone())])
        .expect("base with consumer must plan");
    let before = engine.debug_instance_list_row_counts(None, "test");
    let before_ptr = before
        .iter()
        .find(|(path, _, _)| path == "zz")
        .map(|(_, _, ptr)| *ptr)
        .expect("zz list present before edit");

    let consumer_edited = source.replace("rule y: t.x", "rule y: t.x + 1");
    engine
        .update(None, consumer_edited, SourceType::Volatile)
        .expect("consumer edit must replan");

    let after = engine.debug_instance_list_row_counts(None, "test");
    let after_ptr = after
        .iter()
        .find(|(path, _, _)| path == "zz")
        .map(|(_, _, ptr)| *ptr)
        .expect("zz list present after edit");
    assert_eq!(
        before_ptr, after_ptr,
        "a dependent-only edit must not rebuild the untouched list rows"
    );
}
