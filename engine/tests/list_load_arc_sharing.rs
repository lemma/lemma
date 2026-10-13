//! Measure list Load sharing: nested `uses` must share Arc list bodies.

use lemma_fixtures::logistics;

use lemma::{Engine, SourceType};
use logistics::{LogisticsFixture, GROUND_CELLS, LADDER};
use std::collections::HashSet;

fn total_rows(counts: &[(String, usize, usize)]) -> usize {
    counts.iter().map(|(_, n, _)| n).sum()
}

fn ptr_of(counts: &[(String, usize, usize)], needle: &str) -> usize {
    counts
        .iter()
        .find(|(path, _, _)| path == needle)
        .map(|(_, _, ptr)| *ptr)
        .unwrap_or_else(|| panic!("BUG: missing list path {needle}"))
}

#[test]
fn nested_uses_share_list_bodies() {
    let enterprise = LADDER
        .iter()
        .find(|rung| rung.rate_cells == GROUND_CELLS * 6 * 20)
        .expect("BUG: enterprise rung missing");
    let fixture = logistics::logistics(enterprise.rate_cells);
    assert_eq!(fixture.profile, "enterprise");
    assert_eq!(fixture.rate_cells, enterprise.rate_cells);
    assert!(
        fixture.catalog_json.is_none(),
        "embedded enterprise keeps catalog in source"
    );
    let external_ground = logistics::logistics_external_catalog(GROUND_CELLS);
    assert!(external_ground.catalog_json.is_some());
    assert!(external_ground.source.len() < fixture.source.len());

    let mut engine = Engine::with_limits(fixture.limits.clone());
    engine
        .load([(SourceType::Volatile, fixture.source.clone())])
        .expect("enterprise source must load");

    let rates_paths = engine.debug_instance_list_row_counts(None, "rates_contract_01_ground");
    let quote_paths = engine.debug_instance_list_row_counts(None, "quote_contract_01_ground");
    let shop_paths = engine.debug_instance_list_row_counts(None, "rate_shop");
    let rates_rows = total_rows(&rates_paths);
    let quote_rows = total_rows(&quote_paths);
    let shop_rows = total_rows(&shop_paths);

    let rates_cells_ptr = ptr_of(&rates_paths, "cells");
    let quote_cells_ptr = ptr_of(&quote_paths, "r.cells");
    let shop_cells_ptr = ptr_of(&shop_paths, "q0.r.cells");
    let unique_shop_arcs: HashSet<usize> = shop_paths.iter().map(|(_, _, ptr)| *ptr).collect();

    assert_eq!(rates_rows, 1050);
    assert_eq!(
        quote_rows, 6475,
        "logical presence of nested lists unchanged"
    );
    assert_eq!(shop_rows, quote_rows * 120);
    assert_eq!(
        rates_cells_ptr, quote_cells_ptr,
        "quote must share rates cells Arc"
    );
    assert_eq!(
        rates_cells_ptr, shop_cells_ptr,
        "rate_shop must share rates cells Arc"
    );
    // Distinct catalog bodies: 120 rate cards + 20 carriers × 6 zone lists + 2 fuel lists.
    let expected_unique = 120 + 20 * 6 + 2;
    assert_eq!(
        unique_shop_arcs.len(),
        expected_unique,
        "rate_shop should hold one Arc per distinct catalog list, not per path"
    );

    engine
        .evaluate(
            None,
            LogisticsFixture::SPEC_NAME,
            None,
            LogisticsFixture::inputs(),
            Some(&[LogisticsFixture::TERMINAL_RULE.to_string()]),
            false,
        )
        .expect("rate_shop.cheapest must evaluate");

    let snapshot = engine.snapshot().expect("snapshot");
    let restored = Engine::from_snapshot(&snapshot).expect("restore");
    restored
        .evaluate(
            None,
            LogisticsFixture::SPEC_NAME,
            None,
            LogisticsFixture::inputs(),
            Some(&[LogisticsFixture::TERMINAL_RULE.to_string()]),
            false,
        )
        .expect("restored rate_shop.cheapest must evaluate");
}

#[test]
fn nested_list_reference_fields_still_eval() {
    let source = r#"
spec item
data name: text
data qty: number

spec catalog
uses items: item list
  -> key name
  -> with name: "a", qty: qty

data qty: number
rule first_qty: (items -> find "a").qty

spec wrapper
uses c: catalog
  -> with qty: qty

data qty: number
rule out: c.first_qty
"#;
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, source.to_string())])
        .expect("nested reference list must plan");
    let response = engine
        .evaluate(
            None,
            "wrapper",
            None,
            lemma::IndexMap::from([("qty".into(), "7".into())]),
            Some(&["out".to_string()]),
            false,
        )
        .expect("nested reference list must evaluate");
    let value = response
        .results
        .get("out")
        .expect("out")
        .result()
        .expect("value")
        .to_string();
    assert_eq!(value, "7");

    let catalog = engine.debug_instance_list_row_counts(None, "catalog");
    let wrapper = engine.debug_instance_list_row_counts(None, "wrapper");
    assert_eq!(
        ptr_of(&catalog, "items"),
        ptr_of(&wrapper, "c.items"),
        "nested uses must share list Arc with reference fields"
    );
}
