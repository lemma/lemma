#![no_main]

use lemma::DateTimeValue;
use lemma::Engine;
use lemma::IndexMap;
use libfuzzer_sys::fuzz_target;
use std::str::FromStr;

fn fixed_now() -> DateTimeValue {
    DateTimeValue::from_str("2024-06-15T12:00:00Z").expect("BUG: fixed fuzz datetime")
}

fn response_json(response: &lemma::Response) -> String {
    serde_json::to_string(&lemma::api::Response::from(response))
        .expect("BUG: Response must serialize")
}

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let mut engine = Engine::new();
        let loaded = engine.load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("fuzz_input"))),
            s.to_string(),
        )]);

        // Property: load Ok => every loaded spec must evaluate and format
        // explanations without panic; serialize must succeed; two evaluates
        // with the same effective datetime must agree.
        if loaded.is_ok() {
            let now = fixed_now();
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
                let first = engine.evaluate(
                    None,
                    &name,
                    Some(&now),
                    IndexMap::new(),
                    None,
                    true,
                );
                let second = engine.evaluate(
                    None,
                    &name,
                    Some(&now),
                    IndexMap::new(),
                    None,
                    true,
                );
                match (first, second) {
                    (Ok(a), Ok(b)) => {
                        let json_a = response_json(&a);
                        let json_b = response_json(&b);
                        assert_eq!(
                            json_a, json_b,
                            "BUG: evaluate must be deterministic for fixed effective datetime"
                        );
                        std::hint::black_box(json_a);
                        for result in a.results.values() {
                            if !result.vetoed {
                                let explanation = result.explanation.as_ref().expect(
                                    "BUG: explain:true and non-vetoed rule must carry explanation",
                                );
                                std::hint::black_box(lemma::format_explanation(explanation));
                                std::hint::black_box(format!("{explanation:?}"));
                            } else if let Some(explanation) = &result.explanation {
                                std::hint::black_box(lemma::format_explanation(explanation));
                                std::hint::black_box(format!("{explanation:?}"));
                            }
                        }
                    }
                    (Err(error), Err(_)) => {
                        std::hint::black_box(error);
                    }
                    (Ok(_), Err(_)) | (Err(_), Ok(_)) => {
                        panic!(
                            "BUG: evaluate Ok/Err disagree across identical deterministic runs"
                        );
                    }
                }
            }
        }
    }
});
