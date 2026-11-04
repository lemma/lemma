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

        // Property: load Ok => every loaded spec must evaluate and format
        // explanations without panic.
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
                match engine.evaluate(
                    None,
                    &name,
                    Some(&now),
                    IndexMap::new(),
                    None,
                    true,
                ) {
                    Ok(response) => {
                        if let Ok(json) = serde_json::to_string(&lemma::api::Response::from(
                            &response,
                        )) {
                            std::hint::black_box(json);
                        }
                        for result in response.results.values() {
                            if let Some(explanation) = &result.explanation {
                                std::hint::black_box(lemma::format_explanation(explanation));
                                std::hint::black_box(format!("{explanation:?}"));
                            }
                        }
                    }
                    Err(error) => {
                        std::hint::black_box(error);
                    }
                }
            }
        }
    }
});
