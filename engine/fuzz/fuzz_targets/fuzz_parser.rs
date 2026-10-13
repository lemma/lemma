#![no_main]

use lemma::DateTimeValue;
use lemma::Engine;
use lemma::IndexMap;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let mut engine = Engine::new();
        let loaded = engine.load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("fuzz_input"))),
            s.to_string(),
        )]);

        // Property: load Ok => every loaded spec must evaluate without panic.
        if loaded.is_ok() {
            let now = DateTimeValue::now();
            let spec_names: Vec<String> = engine
                .specs()
                .iter()
                .find(|r| r.repository.is_none())
                .expect("BUG: default repository must exist in specs()")
                .specs
                .iter()
                .map(|version| version.name.clone())
                .collect();
            for name in spec_names {
                if let Err(error) = engine.evaluate(
                    None,
                    &name,
                    Some(&now),
                    IndexMap::new(),
                    None,
                    false,
                ) {
                    std::hint::black_box(error);
                }
            }
        }
    }
});
