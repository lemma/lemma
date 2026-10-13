//! Fallible arbitrary-precision integers vendored from num-bigint 0.4.6 algorithms.
//!
//! See LICENSE in this directory for upstream attribution.

mod alloc;
mod biguint;
mod digit;
mod signed;

pub use alloc::AllocError;
#[cfg(test)]
pub(crate) use biguint::work_counters;
pub use biguint::BigUint;
pub use signed::BigInt;
