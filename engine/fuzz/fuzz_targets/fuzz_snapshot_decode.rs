#![no_main]

use lemma::Engine;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(engine) = Engine::from_snapshot(data) {
        engine
            .snapshot()
            .expect("BUG: a decoded snapshot must encode again");
    }
});
