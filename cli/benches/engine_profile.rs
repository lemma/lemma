// Criterion cases synced with xtask/src/benchmarks/cli.rs PROFILE_BENCH_CASES.
use criterion::{criterion_group, criterion_main, Criterion};
use lemma::*;
use std::collections::HashMap;

fn load_engine() -> Engine {
    let examples_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../engine/documentation/examples");

    let mut paths = Vec::new();
    let mut dirs = vec![examples_dir.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("read examples dir") {
            let entry = entry.expect("dir entry");
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("lemma") {
                paths.push(path);
            }
        }
    }

    let mut sources = Vec::with_capacity(paths.len());
    for path in &paths {
        let content = std::fs::read_to_string(path).expect("read example lemma file");
        sources.push((SourceType::Path(std::sync::Arc::new(path.clone())), content));
    }

    let mut engine = Engine::new();
    engine.load(sources).expect("specs must load");
    engine
}

fn salary_data() -> HashMap<String, String> {
    [
        ("gross_salary", "5000 eur"),
        ("pay_period", "month"),
        ("income_source", "employment"),
        ("pension_contribution", "150 eur"),
        ("payroll_tax_credit", "true"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect()
}

fn bench_dutch_salary_profile(c: &mut Criterion) {
    let engine = load_engine();
    let now = DateTimeValue::now();
    let spec = "net_salary";
    let data = salary_data();

    let mut group = c.benchmark_group("dutch_salary");

    group.bench_function("engine_evaluate", |b| {
        b.iter(|| {
            let resp = engine
                .run(None, spec, Some(&now), data.clone(), None, false)
                .expect("run");
            std::hint::black_box(resp);
        });
    });

    group.bench_function("single_rule", |b| {
        b.iter(|| {
            let resp = engine
                .run(
                    None,
                    spec,
                    Some(&now),
                    data.clone(),
                    Some(&[String::from("periods_per_year")]),
                    false,
                )
                .expect("run");
            std::hint::black_box(resp);
        });
    });

    group.bench_function("json_envelope", |b| {
        let response = engine
            .run(None, spec, Some(&now), data.clone(), None, false)
            .expect("run");
        b.iter(|| {
            let envelope = build_envelope(&response, spec, &now);
            let json = serde_json::to_vec(&envelope).expect("serialize");
            std::hint::black_box(json);
        });
    });

    group.bench_function("json_raw_response", |b| {
        let response = engine
            .run(None, spec, Some(&now), data.clone(), None, false)
            .expect("run");
        b.iter(|| {
            let json =
                serde_json::to_vec(&lemma::api::Response::from(&response)).expect("serialize");
            std::hint::black_box(json);
        });
    });

    group.finish();
}

fn build_envelope(
    response: &Response,
    spec_name: &str,
    effective: &DateTimeValue,
) -> serde_json::Value {
    let mut result = serde_json::Map::new();
    for (name, rule_result) in &response.results {
        let mut entry = serde_json::Map::new();
        entry.insert("vetoed".into(), serde_json::Value::Bool(rule_result.vetoed));
        if let Some(result) = rule_result.result() {
            entry.insert(
                "result".into(),
                serde_json::Value::String(result.to_string()),
            );
        }
        if let Some(veto_reason) = &rule_result.veto_reason {
            entry.insert(
                "veto_reason".into(),
                serde_json::Value::String(veto_reason.clone()),
            );
        }
        if let Some(explanation) = &rule_result.explanation {
            if let OperationResult::Value(v) = &explanation.result {
                let val = serde_json::to_value(lemma::api::ValueKind::from(&v.value))
                    .expect("ValueKind serializes");
                entry.insert("value".into(), val);
            }
        }
        entry.insert(
            "rule_type".into(),
            serde_json::Value::String(rule_result.rule_type.clone()),
        );
        result.insert(name.clone(), serde_json::Value::Object(entry));
    }
    serde_json::json!({
        "spec": spec_name,
        "effective": effective.to_string(),
        "result": result,
    })
}

criterion_group!(benches, bench_dutch_salary_profile);
criterion_main!(benches);
