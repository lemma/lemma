//! An engine reached through a sequence of edits must expose the same spec versions,
//! temporal boundaries and run results as an engine loaded once with the final sources.

use lemma::{DateTimeValue, Engine, SourceType};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

fn path(name: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(name)))
}

fn at(date: &str) -> DateTimeValue {
    DateTimeValue::from_str(date).expect("date")
}

fn fresh(files: &BTreeMap<&'static str, String>) -> Engine {
    let mut engine = Engine::new();
    engine
        .load(files.iter().map(|(name, code)| (path(name), code.clone())))
        .expect("fresh load of final sources");
    engine
}

fn observe(engine: &Engine, specs: &[&str]) -> Vec<String> {
    let mut versions: Vec<String> = engine
        .specs()
        .iter()
        .flat_map(|repository| {
            repository
                .specs
                .iter()
                .map(|version| format!("{:?} {version:?}", repository.repository))
        })
        .collect();
    versions.sort();
    let mut lines = vec![format!("specs {versions:?}")];
    for spec in specs {
        for date in ["2025-03-01", "2025-09-01", "2026-02-01"] {
            let response = engine
                .evaluate(
                    None,
                    spec,
                    Some(&at(date)),
                    lemma::IndexMap::new(),
                    None,
                    false,
                )
                .expect("planned spec must evaluate");
            lines.push(format!("{spec} {date} {:?}", response.results));
        }
    }
    lines
}

fn assert_same_observations(incremental: &Engine, fresh: &Engine, specs: &[&str], step: &str) {
    let left = observe(incremental, specs);
    let right = observe(fresh, specs);
    assert_eq!(left.len(), right.len(), "{step}: observation count");
    for (index, (left_line, right_line)) in left.iter().zip(&right).enumerate() {
        assert_eq!(left_line, right_line, "{step}: observation {index} differs");
    }
}

#[test]
fn edit_sequence_matches_fresh_engine() {
    let mut files: BTreeMap<&'static str, String> = BTreeMap::new();
    files.insert("base_v1.lemma", "spec base\ndata v: 1\nrule r: v\n".into());
    files.insert(
        "base_v2.lemma",
        "spec base 2025-06-01\ndata v: 2\nrule r: v\n".into(),
    );
    files.insert(
        "mid.lemma",
        "spec mid\nuses b: base\nrule r: b.r + 1\n".into(),
    );
    files.insert(
        "root.lemma",
        "spec root\nuses m: mid\nrule out: m.r * 2\n".into(),
    );
    files.insert(
        "pinned.lemma",
        "spec pinned\nuses b: base 2025-06-01\nrule out: b.r\n".into(),
    );

    let mut engine = Engine::new();
    engine
        .load(files.iter().map(|(name, code)| (path(name), code.clone())))
        .expect("initial load");
    let mut names = vec!["base", "mid", "root", "pinned"];
    assert_same_observations(&engine, &fresh(&files), &names, "initial load");

    files.insert(
        "base_v2.lemma",
        "spec base 2025-06-01\ndata v: 20\nrule r: v\n".into(),
    );
    engine
        .update(None, files["base_v2.lemma"].clone(), path("base_v2.lemma"))
        .expect("edit second version body");
    assert_same_observations(&engine, &fresh(&files), &names, "edit second version body");

    files.insert(
        "base_v3.lemma",
        "spec base 2026-01-01\ndata v: 3\nrule r: v\n".into(),
    );
    engine
        .load([(path("base_v3.lemma"), files["base_v3.lemma"].clone())])
        .expect("add third version");
    assert_same_observations(&engine, &fresh(&files), &names, "add third version");

    files.insert(
        "mid.lemma",
        "spec mid\nuses b: base\nrule r: b.r + 10\n".into(),
    );
    engine
        .update(None, files["mid.lemma"].clone(), path("mid.lemma"))
        .expect("edit consumer body");
    assert_same_observations(&engine, &fresh(&files), &names, "edit consumer body");

    files.insert(
        "base_v1.lemma",
        "spec base\ndata v: 100\nrule r: v\n".into(),
    );
    engine
        .update(None, files["base_v1.lemma"].clone(), path("base_v1.lemma"))
        .expect("edit origin version body");
    assert_same_observations(&engine, &fresh(&files), &names, "edit origin version body");

    files.remove("pinned.lemma");
    engine
        .remove(None, "pinned", None)
        .expect("remove pinned consumer");
    names.retain(|name| *name != "pinned");
    assert_same_observations(&engine, &fresh(&files), &names, "remove pinned consumer");
}
