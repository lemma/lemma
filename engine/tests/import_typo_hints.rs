//! Misspelled keys under an import alias produce a "did you mean" hint in the imported body.

use indexmap::IndexMap;
use lemma::DateTimeValue;
use lemma::Engine;

fn load(engine: &mut Engine, code: &str) {
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("t.lemma"))),
            code.to_string(),
        )])
        .expect("load");
}

#[test]
fn misspelled_key_under_import_alias_suggests_in_body() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec inner
data amount: number
rule doubled: amount * 2

spec outer
uses p: inner
rule total: p.doubled
"#,
    );
    let now = DateTimeValue::now();
    let mut data = IndexMap::new();
    data.insert("p.amountt".to_string(), "5".into());
    let response = engine
        .evaluate(None, "outer", Some(&now), data, None, false)
        .expect("ignored typo key; run completes");
    let reason = response
        .results
        .get("total")
        .expect("total")
        .veto_reason
        .as_deref()
        .expect("veto reason");
    assert!(
        reason.contains("Required data") && reason.contains("Did you mean 'amountt'"),
        "got: {reason}"
    );
}
