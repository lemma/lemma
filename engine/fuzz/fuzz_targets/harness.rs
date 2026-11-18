//! Production path shared by every fuzz target: format, load, show, source,
//! evaluate with explanations, determinism.

use lemma::DateTimeValue;
use lemma::EvaluateDataValue;
use lemma::IndexMap;
use lemma::SourceType;
use std::str::FromStr;
use std::sync::Arc;

fn fixed_now() -> DateTimeValue {
    DateTimeValue::from_str("2024-06-15T12:00:00Z").expect("BUG: fixed fuzz datetime")
}

fn source_type(label: &str) -> SourceType {
    SourceType::Path(Arc::new(std::path::PathBuf::from(label)))
}

pub fn check_format(label: &str, source: &str) {
    match lemma::format_source(source, source_type(label)) {
        Ok(formatted) => {
            let again = lemma::format_source(&formatted, source_type(label))
                .expect("BUG: canonical Lemma source must format again");
            assert_eq!(again, formatted, "BUG: format_source must be idempotent");
            std::hint::black_box(again);
        }
        Err(error) => {
            std::hint::black_box(error);
        }
    }
}

pub fn exercise_source(label: &str, source: &str) {
    check_format(label, source);
    let mut engine = lemma::Engine::new();
    let loaded = engine.load([(source_type(label), source.to_string())]);
    if let Err(error) = loaded {
        std::hint::black_box(error);
        return;
    }
    exercise_loaded(&engine, &IndexMap::new(), None);
}

pub fn exercise_loaded(
    engine: &lemma::Engine,
    data: &IndexMap<String, EvaluateDataValue>,
    rules: Option<&[String]>,
) {
    let fixed = fixed_now();
    for repo in engine.specs() {
        let repository = repo.repository.as_deref();
        for version in &repo.specs {
            let mut instants = vec![fixed.clone()];
            if let Some(from) = &version.effective_from {
                if from != &fixed {
                    instants.push(from.clone());
                }
            }
            for instant in &instants {
                exercise_instant(engine, repository, &version.name, instant, data, rules);
            }
        }
    }
}

fn exercise_instant(
    engine: &lemma::Engine,
    repository: Option<&str>,
    name: &str,
    instant: &DateTimeValue,
    data: &IndexMap<String, EvaluateDataValue>,
    rules: Option<&[String]>,
) {
    match engine.show(repository, name, Some(instant)) {
        Ok(show) => {
            let json = serde_json::to_string(&lemma::show_document(engine, &show))
                .expect("BUG: Show document must serialize");
            std::hint::black_box(json);
        }
        Err(error) => {
            std::hint::black_box(error);
        }
    }
    match engine.source(repository, Some(name), Some(instant)) {
        Ok(text) => {
            std::hint::black_box(text);
        }
        Err(error) => {
            std::hint::black_box(error);
        }
    }

    let first = engine.evaluate(repository, name, Some(instant), data.clone(), rules, true);
    let second = engine.evaluate(repository, name, Some(instant), data.clone(), rules, true);
    match (first, second) {
        (Ok(a), Ok(b)) => {
            let json_a = serde_json::to_string(&lemma::api::Response::from(&a))
                .expect("BUG: Response must serialize");
            let json_b = serde_json::to_string(&lemma::api::Response::from(&b))
                .expect("BUG: Response must serialize");
            assert_eq!(
                json_a, json_b,
                "BUG: evaluate must be deterministic for fixed effective datetime"
            );
            std::hint::black_box(json_a);
            for result in a.results.values() {
                if !result.vetoed {
                    let explanation = result
                        .explanation
                        .as_ref()
                        .expect("BUG: explain:true and non-vetoed rule must carry explanation");
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
            panic!("BUG: evaluate Ok/Err disagree across identical deterministic runs");
        }
    }
}
