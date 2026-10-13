//! External-catalog logistics: empty keyed lists + JSON run data match embedded.

use crate::fixture::{self, LogisticsFixture, LADDER};
use lemma::{parse_evaluate_data_object, Engine, SourceType};

fn cheapest_display(
    engine: &Engine,
    data: indexmap::IndexMap<String, lemma::EvaluateDataValue>,
) -> String {
    let response = engine
        .evaluate(
            None,
            LogisticsFixture::SPEC_NAME,
            None,
            data,
            Some(&[LogisticsFixture::TERMINAL_RULE.to_string()]),
            false,
        )
        .expect("rate_shop.cheapest must evaluate");
    response
        .results
        .get(LogisticsFixture::TERMINAL_RULE)
        .expect("cheapest result")
        .result()
        .expect("cheapest must produce a value")
        .to_string()
}

#[test]
fn external_ground_matches_embedded_cheapest() {
    let ground = LADDER[0].rate_cells;
    let embedded = fixture::logistics(ground);
    let external = fixture::logistics_external_catalog(ground);
    assert_eq!(external.profile, "ground");
    assert_eq!(external.rate_cells, ground);
    assert_eq!(embedded.rate_cells, ground);
    assert!(
        external.source.len() < embedded.source.len() / 4,
        "external source {} must be much smaller than embedded {}",
        external.source.len(),
        embedded.source.len()
    );
    assert!(external.source.len() < LADDER[0].source_bytes);

    let mut embedded_engine = Engine::with_limits(embedded.limits.clone());
    embedded_engine
        .load([(SourceType::Volatile, embedded.source)])
        .expect("embedded ground must load");
    let expected = cheapest_display(&embedded_engine, LogisticsFixture::inputs());

    let catalog = external
        .catalog_json
        .clone()
        .expect("external fixture must carry catalog_json");
    let data = parse_evaluate_data_object(&Some(catalog)).expect("catalog_json must parse");

    let mut external_engine = Engine::with_limits(external.limits.clone());
    external_engine
        .load([(SourceType::Volatile, external.source)])
        .expect("external ground must load");
    let actual = cheapest_display(&external_engine, data);
    assert_eq!(actual, expected);
}

#[test]
fn external_carrier_matches_embedded_cheapest() {
    let rate_cells = LADDER[1].rate_cells;
    let embedded = fixture::logistics(rate_cells);
    let external = fixture::logistics_external_catalog(rate_cells);

    let mut embedded_engine = Engine::with_limits(embedded.limits.clone());
    embedded_engine
        .load([(SourceType::Volatile, embedded.source)])
        .expect("embedded carrier must load");
    let expected = cheapest_display(&embedded_engine, LogisticsFixture::inputs());

    let data = parse_evaluate_data_object(&external.catalog_json).expect("catalog_json must parse");
    let mut external_engine = Engine::with_limits(external.limits.clone());
    external_engine
        .load([(SourceType::Volatile, external.source)])
        .expect("external carrier must load");
    let actual = cheapest_display(&external_engine, data);
    assert_eq!(actual, expected);
}

#[test]
fn external_cells_reject_default_max_list_rows() {
    let mut skeleton = fixture::logistics_external_catalog(LADDER[0].rate_cells);
    skeleton.limits.max_list_rows = 1024;
    skeleton.limits.max_list_nodes = 4096;

    let mut engine = Engine::with_limits(skeleton.limits.clone());
    engine
        .load([(SourceType::Volatile, skeleton.source)])
        .expect("skeleton must load under default list limits");

    let err = engine
        .evaluate(
            None,
            LogisticsFixture::SPEC_NAME,
            None,
            parse_evaluate_data_object(&skeleton.catalog_json).expect("catalog_json must parse"),
            Some(&[LogisticsFixture::TERMINAL_RULE.to_string()]),
            false,
        )
        .expect_err("1050-row cells must exceed max_list_rows 1024");
    let message = err.to_string();
    assert!(
        message.contains("max_list_rows"),
        "expected max_list_rows in error, got {message}"
    );
}
