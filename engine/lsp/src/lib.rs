//! Lemma LSP: library for native (stdio) and WASM (browser streams) builds.

pub mod completion;
pub mod diagnostics;
pub mod semantic_tokens;
pub mod server;
pub mod workspace;

#[cfg(test)]
mod textmate_drift;

#[cfg(not(target_arch = "wasm32"))]
pub mod workspace_files;

#[cfg(target_arch = "wasm32")]
pub mod browser;

#[cfg(not(target_arch = "wasm32"))]
pub mod stdio;
