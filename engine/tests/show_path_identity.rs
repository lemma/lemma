//! Show.repository and ShowData/ShowRule.path carry resolved path identity.

use lemma::{DateTimeValue, Engine, PathSegment, SourceType};
use std::path::PathBuf;
use std::sync::Arc;

fn path_source(path: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(path)))
}

#[test]
fn show_omits_repository_for_unnamed_workspace_local_paths_empty() {
    let mut engine = Engine::new();
    engine
        .load([(
            path_source("local.lemma"),
            r#"
spec s
data n: number
rule r: n
"#
            .to_string(),
        )])
        .expect("load");
    let show = engine.show(None, "s", None).expect("show");
    assert!(show.repository.is_none());
    assert_eq!(
        show.data.get("n").expect("n").path,
        Vec::<PathSegment>::new()
    );
    assert_eq!(
        show.rules.get("r").expect("r").path,
        Vec::<PathSegment>::new()
    );
    let json = serde_json::to_value(lemma::api::Show::from(&show)).expect("json");
    assert!(json.get("repository").is_none());
}

#[test]
fn show_interns_named_repository() {
    let mut engine = Engine::new();
    engine
        .load([(
            path_source("alpha.lemma"),
            r#"repo alpha
spec duped
rule answer: 1
"#
            .to_string(),
        )])
        .expect("load");
    engine
        .load([(
            path_source("beta.lemma"),
            r#"repo beta
spec duped
rule answer: 99
"#
            .to_string(),
        )])
        .expect("load");
    let show = engine
        .show(Some("alpha"), "duped", Some(&DateTimeValue::now()))
        .expect("show alpha");
    assert_eq!(show.repository.as_deref(), Some("alpha"));
}

#[test]
fn show_data_path_one_level_same_repo_import() {
    let mut engine = Engine::new();
    engine
        .load([(
            path_source("compose.lemma"),
            r#"
spec pricing
data price: number

spec cashier
uses calc: pricing
rule total: calc.price
"#
            .to_string(),
        )])
        .expect("load");
    let show = engine.show(None, "cashier", None).expect("show");
    let entry = show.data.get("calc.price").expect("calc.price");
    assert_eq!(
        entry.path,
        vec![PathSegment {
            uses: "calc".to_string(),
            repository: None,
            spec: "pricing".to_string(),
        }]
    );
}

#[test]
fn show_data_path_two_level_unnamed_import() {
    let mut engine = Engine::new();
    engine
        .load([(
            path_source("nested.lemma"),
            r#"
spec leaf
data x: number

spec mid
uses b: leaf

spec outer
uses a: mid
rule r: a.b.x
"#
            .to_string(),
        )])
        .expect("load");
    let show = engine.show(None, "outer", None).expect("show");
    let entry = show.data.get("a.b.x").expect("a.b.x");
    assert_eq!(
        entry.path,
        vec![
            PathSegment {
                uses: "a".to_string(),
                repository: None,
                spec: "mid".to_string(),
            },
            PathSegment {
                uses: "b".to_string(),
                repository: None,
                spec: "leaf".to_string(),
            },
        ]
    );
}

#[test]
fn show_data_path_cross_repo_same_spec_basename() {
    let mut engine = Engine::new();
    engine
        .load([(
            path_source("alpha.lemma"),
            r#"repo alpha
spec duped
data n: number
"#
            .to_string(),
        )])
        .expect("alpha");
    engine
        .load([(
            path_source("beta.lemma"),
            r#"repo beta
spec duped
data n: number
"#
            .to_string(),
        )])
        .expect("beta");
    engine
        .load([(
            path_source("consumer.lemma"),
            r#"
spec consumer
uses a: alpha duped
uses b: beta duped
rule left: a.n
rule right: b.n
"#
            .to_string(),
        )])
        .expect("consumer");
    let show = engine.show(None, "consumer", None).expect("show");
    let a = show.data.get("a.n").expect("a.n");
    let b = show.data.get("b.n").expect("b.n");
    assert_eq!(
        a.path,
        vec![PathSegment {
            uses: "a".to_string(),
            repository: Some("alpha".to_string()),
            spec: "duped".to_string(),
        }]
    );
    assert_eq!(
        b.path,
        vec![PathSegment {
            uses: "b".to_string(),
            repository: Some("beta".to_string()),
            spec: "duped".to_string(),
        }]
    );
}

#[test]
fn show_lemma_units_segment_has_lemma_repository() {
    let mut engine = Engine::new();
    engine
        .load([(
            path_source("units_user.lemma"),
            r#"
spec s
uses u: lemma units
data elapsed: u.duration
rule r: elapsed
"#
            .to_string(),
        )])
        .expect("load");
    let show = engine.show(None, "s", None).expect("show");
    // Nested units typedef slots appear under the uses alias with repository lemma.
    let duration = show
        .data
        .get("u.duration")
        .or_else(|| show.data.get("elapsed"))
        .expect("duration-related slot");
    let segment = duration
        .path
        .iter()
        .find(|segment| segment.uses == "u" || segment.spec == "units")
        .or_else(|| duration.path.first());
    if let Some(segment) = segment {
        assert_eq!(segment.repository.as_deref(), Some("lemma"));
        assert_eq!(segment.spec, "units");
    } else {
        // Local `elapsed: u.duration` may be a type declaration with empty path;
        // the import alias still appears on nested promptable units data.
        let lemma_segments: Vec<_> = show
            .data
            .values()
            .flat_map(|entry| entry.path.iter())
            .filter(|segment| segment.repository.as_deref() == Some("lemma"))
            .collect();
        assert!(
            !lemma_segments.is_empty(),
            "expected PathSegment with repository lemma; keys: {:?}",
            show.data.keys().collect::<Vec<_>>()
        );
    }
}
