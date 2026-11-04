//! A `-> with` path through a pinned import steps through the pinned spec's own imports
//! at the pin, the instant its template is planned at, not at the consumer's instant.

use lemma::{Engine, Errors, SourceType};

fn load(code: &str) -> Result<(), Errors> {
    let mut engine = Engine::new();
    engine.load([(
        SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "pinned_walk.lemma",
        ))),
        code.to_string(),
    )])
}

#[test]
fn binding_through_a_pinned_import_resolves_nested_imports_at_the_pin() {
    load(
        r#"
spec leaf 2025-01-01
data x: number

spec leaf 2026-01-01
data y: number

spec mid 2025-01-01
uses j: leaf

spec app 2026-06-01
uses i: mid 2025-03-01 -> with j.x: 5
"#,
    )
    .expect("`i` is `mid` at 2025-03-01, whose `j` is `leaf` 2025-01-01, which has `x`");
}

#[test]
fn reference_through_a_pinned_import_resolves_nested_imports_at_the_pin() {
    load(
        r#"
spec leaf 2025-01-01
data x: number

spec leaf 2026-01-01
data y: number

spec mid 2025-01-01
uses j: leaf

spec app 2026-06-01
uses i: mid 2025-03-01
rule r: i.j.x
"#,
    )
    .expect("`i.j` is `leaf` 2025-01-01, which has `x`");
}

#[test]
fn reference_to_a_field_only_the_consumer_instant_has_is_rejected_through_the_pin() {
    let err = load(
        r#"
spec leaf 2025-01-01
data y: number

spec leaf 2026-01-01
data x: number

spec mid 2025-01-01
uses j: leaf

spec app 2026-06-01
uses i: mid 2025-03-01
rule r: i.j.x
"#,
    )
    .expect_err("`i.j` is `leaf` 2025-01-01, which has no `x`");

    let messages: Vec<String> = err.errors.iter().map(|e| e.to_string()).collect();
    assert!(
        messages.iter().any(|message| message.contains("'x'")),
        "expected `x` not found on `leaf` 2025-01-01, got: {messages:?}"
    );
}

#[test]
fn binding_to_a_field_only_the_consumer_instant_has_is_rejected_through_the_pin() {
    let err = load(
        r#"
spec leaf 2025-01-01
data y: number

spec leaf 2026-01-01
data x: number

spec mid 2025-01-01
uses j: leaf

spec app 2026-06-01
uses i: mid 2025-03-01 -> with j.x: 5
"#,
    )
    .expect_err("`j` in `i` is `leaf` 2025-01-01, which has no `x`");

    let messages: Vec<String> = err.errors.iter().map(|e| e.to_string()).collect();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("`leaf` has no data field `x`")),
        "expected the binding to miss `x` on `leaf` 2025-01-01, got: {messages:?}"
    );
}
