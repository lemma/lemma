//! Load-time split: pure parse vs full `Engine::load` (parse + plan) on the
//! embedded logistics ladder. The plan phase must dominate; if parse ever
//! dominates, the regression is in the parser, not planning.

use crate::fixture::{self, LADDER};
use lemma::{parse, Engine, SourceType};
use std::time::Instant;

#[test]
fn logistics_load_is_plan_dominated() {
    for rung in &LADDER[..2] {
        let fixture = fixture::logistics(rung.rate_cells);

        let started = Instant::now();
        parse(&fixture.source, SourceType::Volatile, &fixture.limits)
            .expect("logistics source must parse");
        let parse_ms = started.elapsed().as_secs_f64() * 1_000.0;

        let started = Instant::now();
        let mut engine = Engine::with_limits(fixture.limits.clone());
        engine
            .load([(SourceType::Volatile, fixture.source.clone())])
            .expect("logistics source must load");
        let load_ms = started.elapsed().as_secs_f64() * 1_000.0;

        eprintln!(
            "load_split {}: parse={parse_ms:.1}ms load={load_ms:.1}ms plan={:.1}ms",
            fixture.profile,
            load_ms - parse_ms
        );
        assert!(
            parse_ms < load_ms,
            "{}: parse {parse_ms:.1}ms must not exceed load {load_ms:.1}ms",
            fixture.profile
        );
    }
}
