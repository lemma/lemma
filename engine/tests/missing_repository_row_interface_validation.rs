//! A MissingRepository error on ONE temporal row of a consumer must not
//! suppress dependency interface validation for the consumer's OTHER
//! (healthy) temporal rows. The skip exists so a row that cannot resolve its
//! repository does not cascade noise; it must be scoped per row, not per
//! spec name.

use lemma::{Engine, ErrorKind, SourceType};

#[test]
fn missing_repository_row_does_not_suppress_sibling_row_interface_error() {
    let mut engine = Engine::new();

    // Dependency whose interface changes between temporal slices:
    // y is number before 2025-06-01, text after.
    engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "unstable.lemma",
            ))),
            r#"
spec unstable_dep
data y: 5

spec unstable_dep 2025-06-01
data y: "five"
"#
            .to_string(),
        )])
        .expect("dep alone must load; interface change only matters to consumers");

    // Consumer with two temporal rows:
    // - 2024 row references an absent registry repository (MissingRepository),
    // - 2025 row uses unstable_dep unpinned across its interface change.
    let result = engine.load([(
        SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "consumer.lemma",
        ))),
        r#"
spec consumer 2024-01-01
uses ext: @org/absent helper
rule v: ext.value

spec consumer 2025-01-01
uses b: unstable_dep
rule sy: b.y
"#
        .to_string(),
    )]);

    let errs = result.expect_err("both consumer rows carry errors");
    let joined = errs
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" | ");

    assert!(
        errs.iter()
            .any(|e| e.kind() == ErrorKind::MissingRepository),
        "2024 row must report the missing '@org/absent' repository. Got: {joined}"
    );

    assert!(
        joined.contains("without pinning an effective date"),
        "2025 row must still report unstable_dep's interface change; a \
         MissingRepository error on the 2024 row must not disable interface \
         validation for the whole spec name. Got: {joined}"
    );
}
