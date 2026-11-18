#![no_main]

use libfuzzer_sys::fuzz_mutator;
use libfuzzer_sys::fuzz_target;

fuzz_mutator!(|data: &mut [u8], size: usize, max_size: usize, seed: u32| {
    lemma_fuzz::mutate(data, size, max_size, seed)
});

fuzz_target!(|data: &[u8]| {
    if let Ok(source) = std::str::from_utf8(data) {
        lemma_fuzz::exercise_source("fuzz_input", source);
    }
});
