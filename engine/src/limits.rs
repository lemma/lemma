use crate::error::Error;
use crate::parsing::source::Source;

pub const MAX_SPEC_NAME_LENGTH: usize = 128;
pub const MAX_DATA_NAME_LENGTH: usize = 256;
pub const MAX_RULE_NAME_LENGTH: usize = 256;

/// Maximum character length for a text value (data/runtime input).
pub const MAX_TEXT_VALUE_LENGTH: usize = 1024;

/// Validate that a name does not exceed the given character limit.
/// `kind` is a human-readable noun like "spec", "data", "rule", or "type".
pub fn check_max_length(
    name: &str,
    limit: usize,
    kind: &str,
    source: Option<Source>,
) -> Result<(), Error> {
    if name.len() > limit {
        return Err(Error::resource_limit_exceeded(
            format!("max_{kind}_name_length"),
            format!("{limit} characters"),
            format!("{} characters", name.len()),
            format!("Shorten the {kind} name to at most {limit} characters"),
            source,
            None,
            None,
        ));
    }
    Ok(())
}

/// Limits to prevent abuse and enable predictable resource usage
///
/// These limits protect against malicious inputs while being generous enough
/// for all legitimate use cases.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResourceLimits {
    /// Maximum size of one loaded source text in bytes.
    /// Real usage: ~5KB, Limit: 5MB (1000x)
    pub max_source_size_bytes: usize,

    /// Maximum expression nesting depth
    /// Real usage: ~3 levels, Limit: 7. Deeper logic via rule composition.
    pub max_expression_depth: usize,

    /// Maximum expression nodes per source (parser-level)
    /// Quick-reject for pathological single sources.
    pub max_expression_count: usize,

    /// Maximum size of a single data value in bytes
    /// Real usage: ~100 bytes, Limit: 1KB (10x)
    /// Enables server pre-allocation for zero-allocation evaluation
    pub max_data_value_bytes: usize,

    /// Maximum total bytes to load in one batch (and/or in-memory size of loaded specs)
    pub max_loaded_bytes: usize,

    /// Maximum number of sources in one load batch (e.g. after expanding paths on disk)
    pub max_sources: usize,

    /// Maximum unique normal-form cells reachable from one rule root in the
    /// shared graph after normalize. Rule references count as one cell: rule references are
    /// evaluation boundaries, so this bounds only intra-rule IR size. Bounds
    /// planning work and shipped table size. Default: 30,000.
    pub max_normalized_expression_nodes: usize,

    /// Maximum depth of the spec dependency chain (`uses` imports) from the
    /// root spec. Bounds recursion in dependency discovery and graph building.
    /// Real usage: ~3 levels, Limit: 32 (10x).
    pub max_spec_dependency_depth: usize,

    /// Maximum number of specs in one dependency DAG (the root spec plus all
    /// transitive dependencies). Bounds per-plan memory and planning work.
    pub max_dag_specs: usize,

    /// Maximum nesting depth of a rule's normalized NormalForm DAG. Leaves and
    /// rule references count as depth 1: rule references are evaluation boundaries, so this
    /// bounds only intra-rule Kind nesting. The evaluator walks recursively
    /// within one rule; planning must guarantee no rule root overflows the
    /// stack. Lemma's runtime does not return errors — this limit is the
    /// guarantee.
    pub max_normal_form_depth: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_source_size_bytes: 5 * 1024 * 1024, // 5 MB
            max_expression_depth: 7,
            max_expression_count: 65_536,
            max_data_value_bytes: 1024,         // 1 KB
            max_loaded_bytes: 50 * 1024 * 1024, // 50 MB
            max_sources: 4096,
            max_normalized_expression_nodes: 30_000,
            max_spec_dependency_depth: 32,
            max_dag_specs: 4096,
            // Bounds recursive eval stack depth within one rule (rule references = leaves).
            max_normal_form_depth: 4096,
        }
    }
}

impl ResourceLimits {
    /// Apply one named limit override. Unknown keys return `Err`.
    pub fn apply(&mut self, key: &str, value: usize) -> Result<(), String> {
        match key {
            "max_source_size_bytes" => self.max_source_size_bytes = value,
            "max_expression_depth" => self.max_expression_depth = value,
            "max_expression_count" => self.max_expression_count = value,
            "max_data_value_bytes" => self.max_data_value_bytes = value,
            "max_loaded_bytes" => self.max_loaded_bytes = value,
            "max_sources" => self.max_sources = value,
            "max_normalized_expression_nodes" => self.max_normalized_expression_nodes = value,
            "max_spec_dependency_depth" => self.max_spec_dependency_depth = value,
            "max_dag_specs" => self.max_dag_specs = value,
            "max_normal_form_depth" => self.max_normal_form_depth = value,
            other => return Err(format!("unknown limits key: '{other}'")),
        }
        Ok(())
    }
}

/// Convert a JS/JSON number to a [`usize`] limit. Rejects non-integers, negatives,
/// values outside the f64 safe-integer range, and values that do not fit in `usize`
/// (e.g. large safe integers on wasm32).
#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) fn usize_limit_from_f64(key: &str, value: f64) -> Result<usize, String> {
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 {
        return Err(format!(
            "limits value for '{key}' must be a non-negative integer"
        ));
    }
    let as_u64 = value as u64;
    if value >= 2f64.powi(53) || as_u64 as f64 != value {
        return Err(format!(
            "limits value for '{key}' must be a non-negative integer within f64 safe range"
        ));
    }
    if as_u64 > usize::MAX as u64 {
        return Err(format!(
            "limits value for '{key}' exceeds platform usize maximum ({})",
            usize::MAX
        ));
    }
    Ok(as_u64 as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_sets_known_key() {
        let mut limits = ResourceLimits::default();
        limits.apply("max_sources", 7).expect("known key");
        assert_eq!(limits.max_sources, 7);
    }

    #[test]
    fn apply_sets_max_normal_form_depth() {
        let mut limits = ResourceLimits::default();
        limits
            .apply("max_normal_form_depth", 99)
            .expect("known key");
        assert_eq!(limits.max_normal_form_depth, 99);
    }

    #[test]
    fn apply_rejects_unknown_key() {
        let mut limits = ResourceLimits::default();
        let err = limits.apply("not_a_limit", 1).expect_err("unknown");
        assert!(err.contains("unknown limits key"));
    }

    #[test]
    fn usize_limit_from_f64_accepts_integer() {
        assert_eq!(usize_limit_from_f64("max_sources", 7.0).unwrap(), 7);
    }

    #[test]
    fn usize_limit_from_f64_rejects_fraction() {
        let err = usize_limit_from_f64("max_sources", 1.5).expect_err("fraction");
        assert!(err.contains("non-negative integer"));
    }

    #[test]
    fn usize_limit_from_f64_rejects_above_safe_integer() {
        let err = usize_limit_from_f64("max_sources", 2f64.powi(53)).expect_err("unsafe");
        assert!(err.contains("f64 safe range"));
    }

    #[test]
    fn usize_limit_from_f64_rejects_above_usize_max() {
        // On 64-bit hosts usize::MAX is outside f64 safe integers, so the safe-range
        // check fires first. On 32-bit, a safe integer above u32::MAX must error here.
        if (usize::MAX as u64) < (1u64 << 53) {
            let too_big = (usize::MAX as u64).saturating_add(1) as f64;
            let err = usize_limit_from_f64("max_loaded_bytes", too_big).expect_err("overflow");
            assert!(err.contains("exceeds platform usize maximum"), "got: {err}");
        }
    }
}
