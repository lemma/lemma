#![no_main]

use lemma::DateTimeValue;
use lemma::Engine;
use libfuzzer_sys::fuzz_target;
use std::collections::HashMap;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        let mut engine = Engine::new();
        let loaded = engine.load([(lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("fuzz_input"))), s.to_string())]);

        // Property: load Ok => every loaded spec must evaluate without panic.
        if loaded.is_ok() {
            let now = DateTimeValue::now();
            let spec_names: Vec<String> = engine
                .list()
                .iter()
                .find(|r| r.repository.is_none())
                .expect("BUG: default repository must exist in list()")
                .specs
                .iter()
                .map(|ls| ls.name.clone())
                .collect();
            for name in spec_names {
                let _ = engine.run(None, &name, Some(&now), HashMap::new(), None, false);
            }
        }
    }
});
