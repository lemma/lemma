//! Logistics ladder: pin source lengths and smoke-load the ground profile.

use crate::fixture::{self, LogisticsFixture, LADDER};
use lemma::{Engine, SourceType};

#[test]
fn logistics_ladder_pins_and_ground_eval() {
    for rung in &LADDER {
        let fixture = fixture::logistics(rung.rate_cells);
        assert_eq!(
            fixture.rate_cells, rung.rate_cells,
            "{} rate_cells",
            fixture.profile
        );
        assert_eq!(
            fixture.source.len(),
            rung.source_bytes,
            "{} source length",
            fixture.profile
        );
    }

    let embedded = fixture::logistics(LADDER[0].rate_cells);
    assert!(
        embedded.catalog_json.is_none(),
        "embedded fixture keeps catalog in source"
    );
    let embedded_source_len = embedded.source.len();
    let mut engine = Engine::with_limits(embedded.limits.clone());
    engine
        .load([(SourceType::Volatile, embedded.source)])
        .expect("ground logistics source must plan");
    let response = engine
        .evaluate(
            None,
            LogisticsFixture::SPEC_NAME,
            None,
            LogisticsFixture::inputs(),
            Some(&[LogisticsFixture::TERMINAL_RULE.to_string()]),
            false,
        )
        .expect("ground rate_shop.cheapest must evaluate");
    response
        .results
        .get(LogisticsFixture::TERMINAL_RULE)
        .expect("cheapest result")
        .result()
        .expect("cheapest must produce a value");

    let external = fixture::logistics_external_catalog(LADDER[0].rate_cells);
    assert!(external.catalog_json.is_some());
    assert!(external.source.len() < embedded_source_len);
}
