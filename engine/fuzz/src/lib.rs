//! Shared fuzz harness and the structure-preserving mutator.
//!
//! Public items are the surface every fuzz binary calls. A binary that only
//! needs part of it still links this crate, so unused-in-one-binary is not a
//! dead-code warning.

#[path = "../fuzz_targets/harness.rs"]
mod harness;
#[path = "../fuzz_targets/mutate.rs"]
mod mutate;

pub use harness::{check_format, exercise_loaded, exercise_source};
pub use mutate::mutate;
