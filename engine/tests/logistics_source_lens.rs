//! Logistics ladder: pin source lengths and smoke-load the ground profile.

use lemma_fixtures::logistics;

use lemma::{Engine, SourceType};
use logistics::{LogisticsFixture, LADDER};

#[test]
fn logistics_ladder_pins_and_ground_eval() {
    for rung in &LADDER {
        let fixture = logistics::logistics(rung.rate_cells);
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

    let fixture = logistics::logistics(LADDER[0].rate_cells);
    assert!(
        fixture.catalog_json.is_none(),
        "embedded fixture keeps catalog in source"
    );
    let embedded_source_len = fixture.source.len();
    let mut engine = Engine::with_limits(fixture.limits.clone());
    engine
        .load([(SourceType::Volatile, fixture.source)])
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

    let external = logistics::logistics_external_catalog(LADDER[0].rate_cells);
    assert!(external.catalog_json.is_some());
    assert!(external.source.len() < embedded_source_len);
}
