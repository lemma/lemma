use indexmap::IndexMap;
use lemma::{DateGranularity, DateTimeValue, Engine, EvaluateDataValue, TimezoneValue};

fn effective_utc(y: i32, m: u32, d: u32) -> DateTimeValue {
    DateTimeValue {
        year: y,
        month: m,
        day: d,
        hour: 12,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: Some(TimezoneValue { offset_minutes: 0 }),
        granularity: DateGranularity::DateTime,
    }
}

const SPEC: &str = r#"
spec determinism_test
uses lemma units

data alpha: number
data bravo: number
data charlie: text
data delta: number
data echo: boolean

rule added: alpha + bravo + delta
rule over: added > 100
rule active: echo AND over
rule label: charlie
"#;

#[test]
fn repeated_load_produces_identical_json_responses() {
    let eff = effective_utc(2026, 6, 15);
    let data: IndexMap<String, EvaluateDataValue> = [
        ("alpha", "30"),
        ("bravo", "40"),
        ("charlie", "hello"),
        ("delta", "50"),
        ("echo", "true"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.into()))
    .collect();

    let mut jsons = Vec::new();
    for _ in 0..5 {
        let mut engine = Engine::new();
        engine
            .load([(lemma::SourceType::Volatile, SPEC.to_string())])
            .unwrap();
        let response = engine
            .evaluate(
                None,
                "determinism_test",
                Some(&eff),
                data.clone(),
                None,
                true,
            )
            .unwrap();
        let json = serde_json::to_string_pretty(&lemma::api::Response::from(&response)).unwrap();
        jsons.push(json);
    }

    for (i, json) in jsons.iter().enumerate().skip(1) {
        assert_eq!(
            &jsons[0], json,
            "Run 0 vs run {i} produced different JSON output"
        );
    }
}

#[test]
fn repeated_show_is_identical() {
    let eff = effective_utc(2026, 6, 15);

    let mut shows = Vec::new();
    for _ in 0..5 {
        let mut engine = Engine::new();
        engine
            .load([(lemma::SourceType::Volatile, SPEC.to_string())])
            .unwrap();
        let show = engine.show(None, "determinism_test", Some(&eff)).unwrap();
        let json =
            serde_json::to_string_pretty(&lemma::api::show_document(&engine, &show)).unwrap();
        shows.push(json);
    }

    for (i, show_json) in shows.iter().enumerate().skip(1) {
        assert_eq!(
            &shows[0], show_json,
            "Show run 0 vs run {i} produced different JSON"
        );
    }
}

#[test]
fn repeated_load_produces_identical_snapshot_bytes() {
    let mut snapshots = Vec::new();
    for _ in 0..5 {
        let mut engine = Engine::new();
        engine
            .load([(lemma::SourceType::Volatile, SPEC.to_string())])
            .unwrap();
        snapshots.push(engine.snapshot().expect("snapshot"));
    }
    for (i, bytes) in snapshots.iter().enumerate().skip(1) {
        assert_eq!(
            &snapshots[0], bytes,
            "snapshot run 0 vs run {i} produced different bytes"
        );
    }
}
