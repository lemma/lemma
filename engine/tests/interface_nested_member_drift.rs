//! A consumer reading a member through its dependency's own import (`m.s.r`) is held
//! to the interface of that member across every version of the inner dependency that
//! is active in the consumer's range, exactly as a direct read (`m.r`) is.

use lemma::{Engine, SourceType};

#[test]
fn nested_member_type_drift_across_unpinned_versions_is_rejected() {
    let mut engine = Engine::new();
    let err = engine
        .load([(
            SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "nested_drift.lemma",
            ))),
            r#"
spec sub 2025-01-01
rule r: 1

spec sub 2025-07-01
rule r: "one"

spec mid 2025-01-01
uses s: sub

spec app 2025-01-01
uses m: mid
rule out: m.s.r
"#
            .to_string(),
        )])
        .expect_err("rule `r` of `sub` changes type inside the range `app` reads it in");

    let messages: Vec<String> = err.errors.iter().map(|e| e.to_string()).collect();
    assert!(
        messages.iter().any(|message| message
            .contains("rule 's.r' has type number in mid 2025-01-01 and text in mid 2025-07-01")),
        "expected the drift of `s.r` through `mid`, got: {messages:?}"
    );
}
