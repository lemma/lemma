//! One spec version imported at two instants is two templates: a pinned `uses` plans
//! the target at the pin, an unpinned one at the consumer's instant, and their own
//! unpinned imports resolve at those instants.

use indexmap::IndexMap;
use lemma::{DateTimeValue, EffectiveDate, Engine, SourceType, TypeSpecification};
use std::str::FromStr;

fn load(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "two_instants.lemma",
            ))),
            code.to_string(),
        )])
        .unwrap_or_else(|errors| panic!("loads: {errors:?}"));
    engine
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

const LEAF_AND_MID: &str = r#"
spec leaf 2025-01-01
rule v: 1

spec leaf 2026-01-01
rule v: 2

spec mid 2025-01-01
uses j: leaf
rule v: j.v
"#;

#[test]
fn pinned_and_unpinned_import_of_one_version_read_their_own_instants() {
    let engine = load(&format!(
        "{LEAF_AND_MID}
spec app 2026-06-01
uses old: mid 2025-03-01
uses current: mid
rule from_old: old.v
rule from_current: current.v
"
    ));
    assert_eq!(rule_value(&engine, "app", "from_old"), "1");
    assert_eq!(rule_value(&engine, "app", "from_current"), "2");
}

#[test]
fn row_spec_show_is_the_slice_the_list_rows_use() {
    let engine = load(&format!(
        "{LEAF_AND_MID}
spec app 2026-06-01
uses current: mid
data rows: current list
rule total: rows -> sum v
"
    ));
    let now = DateTimeValue::now();
    let show = engine.show(None, "app", Some(&now)).expect("app shows");
    let rows = &show
        .data
        .get("rows")
        .expect("`total` needs `rows`")
        .lemma_type;
    let TypeSpecification::List { element, .. } = &rows.specifications else {
        panic!("`rows` is a list, got {rows:?}");
    };
    let TypeSpecification::Spec { spec: identity } = &element.specifications else {
        panic!("`rows` holds spec rows, got {element:?}");
    };
    let leaf_2026 = EffectiveDate::DateTimeValue(DateTimeValue::from_str("2026-01-01").unwrap());
    assert_eq!(
        identity.slice, leaf_2026,
        "`mid` at 2026-06-01 is its slice from `leaf` 2026-01-01"
    );
    assert_eq!(engine.show_spec(identity).plan_effective, leaf_2026);
}

#[test]
fn unpinned_import_first_then_pinned_read_their_own_instants() {
    let engine = load(&format!(
        "{LEAF_AND_MID}
spec app 2026-06-01
uses current: mid
uses old: mid 2025-03-01
rule from_current: current.v
rule from_old: old.v
"
    ));
    assert_eq!(rule_value(&engine, "app", "from_current"), "2");
    assert_eq!(rule_value(&engine, "app", "from_old"), "1");
}
