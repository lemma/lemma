//! Row-template sharing: one Arc body per row SpecIdentity per planning pass. A plan
//! holds a template for every spec it imports with `uses` and every spec whose rows it
//! holds, and every plan reaching a spec shares that spec's one template.

use crate::fixture::LogisticsFixture;
use crate::sharing::{self, SharingLayout};
use lemma::{Engine, SourceType};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Arc of the row template `identity` in the plan of `spec`.
fn template_ptr(engine: &Engine, spec: &str, identity: &str) -> usize {
    let ptrs = engine.debug_row_template_arc_ptrs(None, spec);
    ptrs.iter()
        .find(|(holder, found, _)| holder.is_empty() && found == identity)
        .map(|(_, _, ptr)| *ptr)
        .unwrap_or_else(|| panic!("{spec}.row_templates[{identity}] missing in {ptrs:?}"))
}

/// Row-template identities a plan holds, under no holder.
fn template_identities(engine: &Engine, spec: &str) -> BTreeSet<String> {
    engine
        .debug_row_template_arc_ptrs(None, spec)
        .into_iter()
        .filter(|(holder, _, _)| holder.is_empty())
        .map(|(_, identity, _)| identity)
        .collect()
}

#[test]
fn two_consumers_share_one_row_template_arc() {
    let source = r#"
spec body
data n: number
rule doubled: n * 2

spec left
uses body
data items: body list
  -> with n: 1
rule out: items -> sum doubled

spec right
uses body
data items: body list
  -> with n: 2
rule out: items -> sum doubled

spec root
uses l: left
uses r: right
rule total: l.out + r.out
"#;
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, source.to_string())])
        .expect("shared row templates must plan");

    let left_body = template_ptr(&engine, "left", "body");
    let right_body = template_ptr(&engine, "right", "body");
    let root_body = template_ptr(&engine, "root", "body");
    assert_eq!(
        left_body, right_body,
        "left and right must share one body template Arc"
    );
    assert_eq!(
        root_body, left_body,
        "root reaches body rows through its imports and shares their template Arc"
    );
    for spec in ["left", "right"] {
        assert_eq!(
            template_identities(&engine, spec),
            BTreeSet::from(["body".to_string()]),
            "{spec} holds a template for its row spec only"
        );
    }
    assert_eq!(
        template_identities(&engine, "root"),
        BTreeSet::from(["body".to_string(), "left".to_string(), "right".to_string()]),
        "root holds a template for each import and for the row spec they reach"
    );

    let response = engine
        .evaluate(
            None,
            "root",
            None,
            lemma::IndexMap::new(),
            Some(&["total".to_string()]),
            false,
        )
        .expect("shared row templates must evaluate");
    assert_eq!(
        response
            .results
            .get("total")
            .expect("total")
            .result()
            .expect("value")
            .to_string(),
        "6"
    );
}

#[test]
fn uses_import_registers_its_template() {
    let source = r#"
spec knoop
data n: number
rule doubled: n * 2

spec consumer
uses k: knoop
  -> with n: 4
rule out: k.doubled
"#;
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, source.to_string())])
        .expect("consumer must plan");
    assert_eq!(
        template_identities(&engine, "consumer"),
        BTreeSet::from(["knoop".to_string()]),
        "a `uses` import is an instance of the imported spec's template"
    );
    let response = engine
        .evaluate(
            None,
            "consumer",
            None,
            lemma::IndexMap::new(),
            Some(&["out".to_string()]),
            false,
        )
        .expect("consumer must evaluate");
    assert_eq!(
        response
            .results
            .get("out")
            .expect("out")
            .result()
            .expect("value")
            .to_string(),
        "8"
    );
}

#[test]
fn rule_built_rows_register_their_template() {
    let source = r#"
spec knoop
data n: number
rule doubled: n * 2

spec consumer
uses knoop
rule rows: knoop list -> with n: 4 -> with n: 5
rule out: rows -> sum doubled
"#;
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, source.to_string())])
        .expect("consumer must plan");
    assert_eq!(
        template_identities(&engine, "consumer"),
        BTreeSet::from(["knoop".to_string()]),
        "rows a rule builds run in a row context of the row spec's template"
    );
    let response = engine
        .evaluate(
            None,
            "consumer",
            None,
            lemma::IndexMap::new(),
            Some(&["out".to_string()]),
            false,
        )
        .expect("consumer must evaluate");
    assert_eq!(
        response
            .results
            .get("out")
            .expect("out")
            .result()
            .expect("value")
            .to_string(),
        "18"
    );
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

/// Row specs whose rows the pipelines hold: rate cells, ZIP zones and fuel brackets.
const ROW_SPECS: [&str; 3] = ["rate_cell", "zip_zone", "fuel_bracket"];

fn assert_templates(engine: &Engine, layout: &SharingLayout) {
    let mut quotes = Vec::new();
    for carrier in &layout.carriers {
        for service in &layout.services {
            quotes.push(layout.quote_spec(carrier, service));
        }
    }
    let rate_shop = template_identities(engine, LogisticsFixture::SPEC_NAME);
    for quote in &quotes {
        assert!(
            rate_shop.contains(quote),
            "rate_shop imports {quote}, so it holds its template: {rate_shop:?}"
        );
    }
    let mut arcs_by_identity: BTreeMap<String, HashSet<usize>> = BTreeMap::new();
    for plan in std::iter::once(LogisticsFixture::SPEC_NAME.to_string()).chain(quotes) {
        let identities = template_identities(engine, &plan);
        for identity in ROW_SPECS {
            assert!(
                identities.contains(identity),
                "{plan} holds a template for row spec {identity}: {identities:?}"
            );
        }
        for identity in identities {
            let ptr = template_ptr(engine, &plan, &identity);
            arcs_by_identity.entry(identity).or_default().insert(ptr);
        }
    }
    for (identity, arcs) in &arcs_by_identity {
        assert_eq!(
            arcs.len(),
            1,
            "rate_shop and its {} quote pipelines must share one {identity} template Arc, got {arcs:?}",
            layout.pipelines()
        );
    }
}

#[test]
fn rate_shop_quotes_share_row_templates() {
    let (embedded, layout) = sharing::logistics_sharing();
    assert!(
        embedded.catalog_json.is_none(),
        "sharing catalog keeps rows in source"
    );

    let mut engine = Engine::with_limits(embedded.limits.clone());
    engine
        .load([(SourceType::Volatile, embedded.source.clone())])
        .expect("sharing catalog must load");

    assert_templates(&engine, &layout);
    cheapest(&engine);
}
