//! Adversarial: `effective_dates` unions every `effective_from` in the context; a consumer
//! whose range contains many foreign boundaries must still plan and evaluate consistently.

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

#[test]
fn many_global_boundaries_inside_consumer_range_plan_at_each_still_consistent() {
    let mut src = String::from(
        r#"spec stable
data v: 1

spec consumer 2025-01-01
uses s: stable
rule out: s.v

"#,
    );
    for m in 2..=11 {
        src.push_str(&format!("spec noise{m} 2025-{m:02}-15\ndata x: {m}\n\n"));
    }

    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("big.lemma"))),
            &src.to_string(),
        )])
        .expect("many specs with boundaries inside consumer range");

    for month in 1..=12 {
        let d = date(2025, month, 5);
        let r = engine.run(None, "consumer", Some(&d), HashMap::new(), None, false);
        assert!(r.is_ok(), "month {month}: {:?}", r.err());
        let resp = r.unwrap();
        let rule = resp.results.get("out").expect("out");
        let v = rule.result().expect("result").to_string();
        assert_eq!(v.to_string(), "1", "month {month}");
    }
}
