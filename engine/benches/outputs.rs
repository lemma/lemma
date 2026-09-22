//! Emit Lemma's per-rule output values for each benchmark fixture.
//!
//! No timing, no warmup, no iteration loop. Runs each fixture once and
//! prints a JSON document the xtask report can diff against the Python
//! benchmark's output dump.

use lemma::{Engine, LemmaType, LiteralValue, OperationResult, SourceType, ValueKind, VetoType};
use serde::Serialize;
use serde_json::Value;
use std::sync::Arc;

mod common;

fn build_engine(fixture: &common::Fixture) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(common::source_path(fixture.lemma_path))),
            fixture.source.to_string(),
        )])
        .expect("BUG: bench fixture spec must load");
    engine
}

#[derive(Serialize)]
struct Document {
    fixtures: Vec<FixtureOutputs>,
}

#[derive(Serialize)]
struct FixtureOutputs {
    spec_name: &'static str,
    outputs: serde_json::Map<String, Value>,
}

#[derive(Serialize)]
struct Output {
    kind: &'static str,
    value: String,
    unit: Option<String>,
}

fn serialized_magnitude(value: &ValueKind) -> String {
    let raw = serde_json::to_value(lemma::api::ValueKind::from(value))
        .expect("BUG: ValueKind serialization is infallible here");
    let Value::Object(mut map) = raw else {
        panic!("BUG: ValueKind serialized to non-object: {raw:?}");
    };
    if map.len() != 1 {
        panic!(
            "BUG: ValueKind serialized with {} keys, expected 1",
            map.len()
        );
    }
    let (tag, payload) = map
        .iter_mut()
        .next()
        .map(|(k, v)| (k.clone(), v.take()))
        .expect("BUG: len-1 map has one entry");
    match tag.as_str() {
        "number" => payload
            .as_str()
            .expect("BUG: number payload must be a string")
            .to_string(),
        "ratio" | "measure" => payload
            .as_object()
            .expect("BUG: measure/ratio payload must be an object")
            .get("value")
            .and_then(Value::as_str)
            .expect("BUG: measure/ratio payload missing 'value' string")
            .to_string(),
        other => panic!("BUG: serialized_magnitude called on unexpected tag '{other}'"),
    }
}

fn measure_unit_name(lemma_type: &LemmaType) -> Option<String> {
    if let Some(binding) = lemma_type.measure_binding_unit.as_ref() {
        return Some(binding.clone());
    }
    lemma_type
        .measure_runtime_signature()
        .first()
        .map(|(name, _)| name.clone())
}

fn normalize_literal(literal: &LiteralValue, lemma_type: &LemmaType) -> Output {
    match &literal.value {
        ValueKind::Number(_) => Output {
            kind: "number",
            value: serialized_magnitude(&literal.value),
            unit: None,
        },
        ValueKind::Ratio(_) => Output {
            kind: "ratio",
            value: serialized_magnitude(&literal.value),
            unit: lemma_type.ratio_primary_unit().map(str::to_string),
        },
        ValueKind::Measure(_) if lemma_type.is_calendar_like() => Output {
            kind: "calendar",
            value: serialized_magnitude(&literal.value),
            unit: measure_unit_name(lemma_type),
        },
        ValueKind::Measure(_) => Output {
            kind: "measure",
            value: serialized_magnitude(&literal.value),
            unit: measure_unit_name(lemma_type),
        },
        ValueKind::Boolean(b) => Output {
            kind: "boolean",
            value: if *b {
                "true".to_string()
            } else {
                "false".to_string()
            },
            unit: None,
        },
        ValueKind::Text(s) => Output {
            kind: "text",
            value: s.clone(),
            unit: None,
        },
        ValueKind::Date(dt) => Output {
            kind: "date",
            value: serde_json::to_string(dt).expect("BUG: date is JSON-serializable"),
            unit: None,
        },
        ValueKind::Time(t) => Output {
            kind: "time",
            value: serde_json::to_string(t).expect("BUG: time is JSON-serializable"),
            unit: None,
        },
        ValueKind::Range(_, _) => {
            todo!(
                "range output not yet expected in benchmark fixtures; comparison semantics undefined"
            )
        }
    }
}

fn normalize_veto(veto: &VetoType) -> Output {
    Output {
        kind: "veto",
        value: veto.to_string(),
        unit: None,
    }
}

fn main() {
    let mut fixtures = Vec::new();
    for fixture in common::fixtures() {
        let engine = build_engine(&fixture);
        let data = fixture.inputs();
        let response = engine
            .run(
                None,
                fixture.spec_name,
                Some(&common::effective()),
                data,
                None,
                true,
            )
            .expect("BUG: outputs bench fixture must evaluate");

        let mut outputs = serde_json::Map::new();
        for (rule_name, result) in &response.results {
            let explanation = result
                .explanation
                .as_ref()
                .expect("BUG: outputs bench requires explain: true");
            let normalized = match &explanation.result {
                OperationResult::Value(bound) => {
                    normalize_literal(&bound.to_literal(), explanation.result_type.as_ref())
                }
                OperationResult::Veto(veto) => normalize_veto(veto),
            };
            outputs.insert(
                rule_name.clone(),
                serde_json::to_value(&normalized)
                    .expect("BUG: Output is trivially JSON-serializable"),
            );
        }
        fixtures.push(FixtureOutputs {
            spec_name: fixture.spec_name,
            outputs,
        });
    }

    let document = Document { fixtures };
    let rendered = serde_json::to_string(&document).expect("BUG: Document is JSON-serializable");
    println!("{rendered}");
}
