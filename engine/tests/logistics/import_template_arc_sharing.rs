//! Named-`uses` template sharing: one Arc body per SpecIdentity per planning pass.

use crate::fixture::{self, LogisticsFixture, LADDER};
use lemma::{Engine, SourceType};
use std::collections::HashSet;

#[test]
fn two_consumers_share_one_import_template_arc() {
    let source = r#"
spec body
data n: number
rule doubled: n * 2

spec left
uses b: body
  -> with n: n
data n: number
rule out: b.doubled

spec right
uses b: body
  -> with n: n
data n: number
rule out: b.doubled

spec root
uses l: left
  -> with n: n
uses r: right
  -> with n: n
data n: number
rule total: l.out + r.out
"#;
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, source.to_string())])
        .expect("shared import templates must plan");

    let ptrs = engine.debug_row_template_arc_ptrs(None, "root");
    let left_body = ptrs
        .iter()
        .find(|(holder, identity, _)| holder == "left" && identity == "body")
        .map(|(_, _, ptr)| *ptr)
        .expect("left.row_templates[body]");
    let right_body = ptrs
        .iter()
        .find(|(holder, identity, _)| holder == "right" && identity == "body")
        .map(|(_, _, ptr)| *ptr)
        .expect("right.row_templates[body]");
    assert_eq!(
        left_body, right_body,
        "left and right must share one body template Arc"
    );

    let response = engine
        .evaluate(
            None,
            "root",
            None,
            lemma::IndexMap::from([("n".into(), "3".into())]),
            Some(&["total".to_string()]),
            false,
        )
        .expect("shared import templates must evaluate");
    assert_eq!(
        response
            .results
            .get("total")
            .expect("total")
            .result()
            .expect("value")
            .to_string(),
        "12"
    );
}

#[test]
fn rate_shop_quotes_share_accessorials_and_zone_templates() {
    let enterprise = &LADDER[3];
    let embedded = fixture::logistics(enterprise.rate_cells);
    assert_eq!(embedded.profile, "enterprise");
    assert_eq!(embedded.rate_cells, enterprise.rate_cells);
    assert!(
        embedded.catalog_json.is_none(),
        "embedded enterprise keeps catalog in source"
    );
    let external_ground = fixture::logistics_external_catalog(LADDER[0].rate_cells);
    assert!(external_ground.catalog_json.is_some());
    assert!(external_ground.source.len() < embedded.source.len());

    let mut engine = Engine::with_limits(embedded.limits.clone());
    engine
        .load([(SourceType::Volatile, embedded.source.clone())])
        .expect("hypothesis fixture must load");

    let ptrs = engine.debug_row_template_arc_ptrs(None, LogisticsFixture::SPEC_NAME);

    let accessorials: HashSet<usize> = ptrs
        .iter()
        .filter(|(holder, identity, _)| holder.starts_with("quote_") && identity == "accessorials")
        .map(|(_, _, ptr)| *ptr)
        .collect();
    assert_eq!(
        accessorials.len(),
        1,
        "all quote pipelines must share one accessorials template Arc, got {accessorials:?}"
    );

    let mut zones_by_carrier: std::collections::BTreeMap<String, HashSet<usize>> =
        std::collections::BTreeMap::new();
    for (holder, identity, ptr) in &ptrs {
        if !holder.starts_with("quote_") || !identity.starts_with("zones_") {
            continue;
        }
        zones_by_carrier
            .entry(identity.clone())
            .or_default()
            .insert(*ptr);
    }
    assert!(
        !zones_by_carrier.is_empty(),
        "expected nested zones_* templates under quotes"
    );
    for (zones_id, arcs) in &zones_by_carrier {
        assert_eq!(
            arcs.len(),
            1,
            "quotes of one carrier must share zones template {zones_id}, got {arcs:?}"
        );
    }

    let quote_templates: HashSet<usize> = ptrs
        .iter()
        .filter(|(holder, identity, _)| holder.is_empty() && identity.starts_with("quote_"))
        .map(|(_, _, ptr)| *ptr)
        .collect();
    assert_eq!(
        quote_templates.len(),
        120,
        "rate_shop holds one Arc per quote spec (120 carrier×service pipelines)"
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
}
