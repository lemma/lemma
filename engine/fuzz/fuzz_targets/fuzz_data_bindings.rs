#![no_main]

use lemma::DateTimeValue;
use lemma::Engine;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let mut engine = Engine::new();

        let code = r#"
spec fuzz_test
data x: number
rule doubled: x * 2
"#;

        engine
            .load([(
                lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "fuzz_binding",
                ))),
                code.to_string(),
            )])
            .expect("BUG: static fuzz spec must load");

        // Property: valid spec loaded => arbitrary data input must not panic.
        let data = lemma::evaluate_data_from_entries(vec![(
            "x".to_string(),
            lemma::EvaluateDataValue::Literal(s.to_string()),
        )])
        .expect("BUG: single fuzz key is unique");
        let now = DateTimeValue::now();
        if let Err(error) = engine.evaluate(None, "fuzz_test", Some(&now), data, None, false) {
            std::hint::black_box(error);
        }
    }
});
