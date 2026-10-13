use crate::common::{effective, source_path, Fixture};
use indexmap::IndexMap;
use lemma::{Engine, EvaluateDataValue, Response, SourceType};
use std::sync::Arc;

pub fn terminal_rule(spec_name: &str) -> &'static str {
    match spec_name {
        "bench_shipping" | "bench_pricing" => "total",
        "bench_order_pipeline" => "grand_total",
        other => panic!("BUG: no terminal rule for bench spec '{other}'"),
    }
}

pub fn load_engine(fixture: &Fixture) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(source_path(fixture.lemma_path))),
            fixture.source.to_string(),
        )])
        .expect("BUG: bench fixture spec must load");
    engine
}

pub fn evaluate_loaded(
    engine: &Engine,
    spec_name: &str,
    data: IndexMap<String, EvaluateDataValue>,
    rules: &[String],
    explain: bool,
) -> Response {
    let effective = effective();
    engine
        .evaluate(
            None,
            spec_name,
            Some(&effective),
            data,
            Some(rules),
            explain,
        )
        .expect("BUG: bench fixture must evaluate")
}
