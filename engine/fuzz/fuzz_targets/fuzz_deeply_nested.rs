#![no_main]

use lemma::DateTimeValue;
use lemma::Engine;
use lemma::IndexMap;
use libfuzzer_sys::fuzz_target;

// Depth range 1..=9 crosses the default max_expression_depth of 7, so both
// the accept path and the depth-limit rejection path are exercised.
fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }
    let depth = (data[0] as usize % 9) + 1;
    let variant = data[1] % 4;

    let mut expr = String::from("1");
    for i in 0..depth {
        expr = match variant {
            0 => format!("({} + 1)", expr),
            1 => format!("({} * 2)", expr),
            2 => format!("({} - {})", expr, i),
            _ => format!("({})", expr),
        };
    }

    let code = format!(
        "spec fuzz_nested\ndata x: 1\nrule deeply_nested: {}\n",
        expr
    );

    let mut engine = Engine::new();
    let loaded = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("fuzz_nested"))),
        &code.to_string(),
    )]);

    // Property: load Ok => evaluation must not panic.
    if loaded.is_ok() {
        let now = DateTimeValue::now();
        if let Err(error) = engine.evaluate(
            None,
            "fuzz_nested",
            Some(&now),
            IndexMap::new(),
            None,
            false,
        ) {
            std::hint::black_box(error);
        }
    }
});
