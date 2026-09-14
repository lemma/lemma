use lemma::EMBEDDED_STDLIB_REPOSITORY;
use lemma::{Engine, ErrorKind};

#[test]
fn list_includes_embedded_lemma_repository_with_units() {
    let engine = Engine::new();
    let repos = engine.list();
    let lemma = repos
        .iter()
        .find(|r| r.repository.as_deref() == Some(EMBEDDED_STDLIB_REPOSITORY))
        .expect("embedded lemma repository must appear in list");
    assert!(
        lemma.specs.iter().any(|ls| ls.name == "units"),
        "expected spec units in lemma repo"
    );
}

#[test]
fn source_lemma_repo_contains_units_duration() {
    let engine = Engine::new();
    let text = engine
        .source(Some(EMBEDDED_STDLIB_REPOSITORY), None, None)
        .expect("embedded lemma repo source");
    assert!(
        text.contains("repo lemma"),
        "expected repo header, got:\n{text}"
    );
    assert!(
        text.contains("spec units"),
        "expected spec units in formatted output, got:\n{text}"
    );
    assert!(
        text.contains("duration") && text.contains("trait duration"),
        "expected duration typedef in formatted output, got:\n{text}"
    );
}

#[test]
fn source_unknown_qualifier_errors() {
    let engine = Engine::new();
    let err = engine
        .source(Some("workspace"), None, None)
        .expect_err("'workspace' is not a repository qualifier");
    assert!(
        err.kind() == ErrorKind::Request,
        "expected request error, got: {err:?}"
    );
}

#[test]
fn user_workspace_cannot_claim_repo_lemma() {
    let mut engine = Engine::new();
    let err = engine
        .load([(
            lemma::SourceType::Volatile,
            "repo lemma\nspec x\ndata a: 1".to_string(),
        )])
        .expect_err("user repo lemma must be rejected");
    let msg = err
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        msg.contains(EMBEDDED_STDLIB_REPOSITORY) && msg.contains("reserved"),
        "expected reserved-repo error, got: {msg}"
    );
}
