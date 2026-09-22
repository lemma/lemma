//! Adversarial: qualified parent types under `uses` must resolve the dependency
//! spec at the same instant as the rest of that parent's body (not only the root slice).

use lemma::{DateGranularity, DateTimeValue, Engine, SourceType};
use std::collections::HashMap;

fn date(y: i32, m: u32, d: u32) -> DateTimeValue {
    DateTimeValue {
        year: y,
        month: m,
        day: d,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: None,
        granularity: DateGranularity::Full,
    }
}

fn assert_rule_value(response: &lemma::Response, rule: &str, expected: &str) {
    let result = response.results.get(rule).expect("rule in results");
    let val = result.result().expect("result").to_string();
    assert_eq!(val.to_string(), expected, "rule {rule}");
}

/// `child` gains `usd` only from 2025-07. `dep` uses `1.00 usd` which requires that unit.
/// Consumer pins `dep` at 2025-07-01; `data money: child.money` must use child@2025-07 (usd),
/// not child@2025-01 (eur-only) at the consumer's slice instant.
#[test]
fn qualified_parent_data_import_resolves_child_at_qualifier_not_root_slice() {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("t.lemma"))),
            r#"
spec child 2025-01-01
data money: measure
 -> unit eur: 1.00
 -> decimals 2

spec child 2025-07-01
data money: measure
 -> unit eur: 1.00
 -> unit usd: 0.91
 -> decimals 2

spec dep 2025-07-01
uses child
data money: child.money
data price: money
rule val: 1.00 usd

spec app 2025-01-01
uses d: dep 2025-07-01
rule out: d.val
"#
            .to_string(),
        )])
        .expect("planning must resolve money type with usd when dep is pinned to 2025-07");

    let r = engine
        .run(
            None,
            "app",
            Some(&date(2025, 3, 1)),
            HashMap::new(),
            None,
            false,
        )
        .expect("run");
    assert_rule_value(&r, "out", "1.00 usd");
}
