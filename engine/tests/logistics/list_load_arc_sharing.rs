//! Nested `uses` list loads: a plan holds the rows of the lists it declares and reads
//! every other list through the template of the spec declaring it, one per spec across
//! every plan. The terminal result is right.

use crate::fixture::LogisticsFixture;
use crate::sharing::{self, SharingLayout};
use lemma::{Engine, SourceType};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Row count per instance-list path of `spec`'s plan.
fn row_counts(engine: &Engine, spec: &str) -> BTreeMap<String, usize> {
    engine
        .debug_instance_list_row_counts(None, spec)
        .into_iter()
        .map(|(path, rows, _)| (path, rows))
        .collect()
}

/// Row store Arc pointer per instance-list path of `spec`'s plan.
fn store_ptrs(engine: &Engine, spec: &str) -> BTreeMap<String, usize> {
    engine
        .debug_instance_list_row_counts(None, spec)
        .into_iter()
        .map(|(path, _, ptr)| (path, ptr))
        .collect()
}

/// Arc of the template of `identity` in the plan of `spec`.
fn template_ptr(engine: &Engine, spec: &str, identity: &str) -> usize {
    let ptrs = engine.debug_row_template_arc_ptrs(None, spec);
    ptrs.iter()
        .find(|(holder, found, _)| holder.is_empty() && found == identity)
        .map(|(_, _, ptr)| *ptr)
        .unwrap_or_else(|| panic!("{spec}.row_templates[{identity}] missing in {ptrs:?}"))
}

fn total_rows(counts: &BTreeMap<String, usize>) -> usize {
    counts.values().sum()
}

fn rows_at(counts: &BTreeMap<String, usize>, path: &str) -> usize {
    *counts
        .get(path)
        .unwrap_or_else(|| panic!("missing list path {path} in {counts:?}"))
}

fn cheapest(engine: &Engine) -> String {
    let response = engine
        .evaluate(
            None,
            LogisticsFixture::SPEC_NAME,
            None,
            LogisticsFixture::inputs(),
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

/// Row counts of the plans the sharing catalog builds, keyed by spec.
fn catalog_row_counts(
    engine: &Engine,
    layout: &SharingLayout,
) -> BTreeMap<String, BTreeMap<String, usize>> {
    let ups = &layout.carriers[0];
    let fedex = &layout.carriers[1];
    let ground = layout.services[0];
    [
        layout.rates_spec(ups, ground),
        layout.rates_spec(fedex, ground),
        layout.quote_spec(ups, ground),
        layout.quote_spec(fedex, ground),
        format!("zones_{ups}"),
        "fuel_ground".to_string(),
        LogisticsFixture::SPEC_NAME.to_string(),
    ]
    .into_iter()
    .map(|spec| {
        let counts = row_counts(engine, &spec);
        (spec, counts)
    })
    .collect()
}

fn assert_list_rows(counts: &BTreeMap<String, BTreeMap<String, usize>>, layout: &SharingLayout) {
    let ups = &layout.carriers[0];
    let fedex = &layout.carriers[1];
    let ground = layout.services[0];
    let rates_ups = &counts[&layout.rates_spec(ups, ground)];
    let rates_fedex = &counts[&layout.rates_spec(fedex, ground)];

    assert_eq!(total_rows(rates_ups), layout.cells_per_card());
    assert_eq!(total_rows(rates_fedex), layout.cells_per_card());
    assert_eq!(rows_at(rates_ups, "cells"), layout.cells_per_card());
    let zones_ups = &counts[&format!("zones_{ups}")];
    assert_eq!(
        total_rows(zones_ups),
        layout.services.len() * layout.zip_count,
        "a carrier's chart holds one zone list per service"
    );
    assert_eq!(
        rows_at(&counts["fuel_ground"], "brackets"),
        layout.fuel_rows
    );
    for spec in [
        layout.quote_spec(ups, ground),
        layout.quote_spec(fedex, ground),
        LogisticsFixture::SPEC_NAME.to_string(),
    ] {
        assert!(
            counts[&spec].is_empty(),
            "{spec} declares no list; it reads its imports' lists through their templates: {:?}",
            counts[&spec]
        );
    }
}

/// Every plan reading the lists of `uses` imports shares one template per spec.
fn assert_list_templates(engine: &Engine, layout: &SharingLayout) {
    let ups = &layout.carriers[0];
    let fedex = &layout.carriers[1];
    let ground = layout.services[0];
    let shop = LogisticsFixture::SPEC_NAME;
    let quote_ups = layout.quote_spec(ups, ground);
    let quote_fedex = layout.quote_spec(fedex, ground);
    let rates_ups = layout.rates_spec(ups, ground);
    assert_eq!(
        template_ptr(engine, &quote_ups, &rates_ups),
        template_ptr(engine, shop, &rates_ups),
        "the quote and rate_shop read one {rates_ups} template"
    );
    assert_eq!(
        template_ptr(engine, &quote_ups, "fuel_ground"),
        template_ptr(engine, &quote_fedex, "fuel_ground"),
        "ground quotes read the one fuel_ground template"
    );
    assert_eq!(
        template_ptr(engine, &quote_ups, "fuel_ground"),
        template_ptr(engine, shop, "fuel_ground")
    );
}

#[test]
fn nested_uses_share_list_bodies() {
    let (embedded, layout) = sharing::logistics_sharing();
    assert!(
        embedded.catalog_json.is_none(),
        "sharing catalog keeps rows in source"
    );

    let mut engine = Engine::with_limits(embedded.limits.clone());
    engine
        .load([(SourceType::Volatile, embedded.source.clone())])
        .expect("sharing catalog must load");

    let before_counts = catalog_row_counts(&engine, &layout);
    assert_list_rows(&before_counts, &layout);
    assert_list_templates(&engine, &layout);
    cheapest(&engine);
}

/// One declared list body is one store allocation: it lives in the plan of the spec
/// declaring it, every consumer reads it through that spec's one template, and
/// content-identical bodies on distinct declarations never merge.
#[test]
fn nested_uses_share_one_store_allocation() {
    let (embedded, layout) = sharing::logistics_sharing();
    let mut engine = Engine::with_limits(embedded.limits.clone());
    engine
        .load([(SourceType::Volatile, embedded.source.clone())])
        .expect("sharing catalog must load");

    let assert_topology = |engine: &Engine| {
        let ups = &layout.carriers[0];
        let fedex = &layout.carriers[1];
        let ground = layout.services[0];
        let rates_ups = store_ptrs(engine, &layout.rates_spec(ups, ground));
        let rates_fedex = store_ptrs(engine, &layout.rates_spec(fedex, ground));
        let zones_ups = store_ptrs(engine, &format!("zones_{ups}"));
        let zones_fedex = store_ptrs(engine, &format!("zones_{fedex}"));

        // ups and fedex ground bodies are identical by construction; their
        // declarations are distinct, so their stores never merge.
        assert_ne!(rates_ups["cells"], rates_fedex["cells"]);
        assert_ne!(zones_ups["ground_zones"], zones_fedex["ground_zones"]);

        let mut declaring: Vec<String> = Vec::new();
        for carrier in &layout.carriers {
            for service in &layout.services {
                declaring.push(layout.rates_spec(carrier, service));
            }
            declaring.push(format!("zones_{carrier}"));
        }
        declaring.push("fuel_ground".to_string());
        declaring.push("fuel_air".to_string());
        // 4 rate cards + 4 zone lists + 2 fuel tables, once each.
        let unique: BTreeSet<usize> = declaring
            .iter()
            .flat_map(|spec| store_ptrs(engine, spec).into_values())
            .collect();
        assert_eq!(
            unique.len(),
            10,
            "each declared catalog body must be one store"
        );

        let mut consumers = vec![LogisticsFixture::SPEC_NAME.to_string()];
        for carrier in &layout.carriers {
            for service in &layout.services {
                consumers.push(layout.quote_spec(carrier, service));
            }
        }
        let mut arcs_by_identity: BTreeMap<String, HashSet<usize>> = BTreeMap::new();
        for consumer in &consumers {
            assert!(
                store_ptrs(engine, consumer).is_empty(),
                "{consumer} declares no list of its own"
            );
            for (holder, identity, ptr) in engine.debug_row_template_arc_ptrs(None, consumer) {
                if holder.is_empty() {
                    arcs_by_identity.entry(identity).or_default().insert(ptr);
                }
            }
        }
        for spec in &declaring[..declaring.len() - 1] {
            let arcs = arcs_by_identity
                .get(spec)
                .unwrap_or_else(|| panic!("no consumer reads {spec}"));
            assert_eq!(
                arcs.len(),
                1,
                "every consumer reads {spec}'s lists through one template, got {arcs:?}"
            );
        }
    };

    assert_topology(&engine);
}

#[test]
fn nested_list_reference_fields_still_eval() {
    let source = r#"
spec item
data name: text
data qty: number

spec catalog
uses item
data items: item list
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

    let catalog = row_counts(&engine, "catalog");
    assert_eq!(rows_at(&catalog, "items"), 1);
    assert!(
        row_counts(&engine, "wrapper").is_empty(),
        "the wrapper reads the imported list through the catalog template"
    );
    let templates: BTreeSet<String> = engine
        .debug_row_template_arc_ptrs(None, "wrapper")
        .into_iter()
        .filter(|(holder, _, _)| holder.is_empty())
        .map(|(_, identity, _)| identity)
        .collect();
    assert_eq!(
        templates,
        BTreeSet::from(["catalog".to_string(), "item".to_string()]),
        "the wrapper holds the catalog template and the template of its rows"
    );
}
