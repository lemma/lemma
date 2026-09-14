//! # Lemma Engine
//!
//! **Rules for man and machine**
//!
//! Consumer API: **`load`** → **`list`** → **`show`** / **`source`** → **`run`**.

#[cfg(test)]
mod tests;

pub mod api;
pub(crate) mod computation;
pub mod documentation;
pub(crate) mod engine;
pub(crate) mod error;
pub(crate) mod evaluation;
pub(crate) mod formatting;
pub(crate) mod limits;
pub(crate) mod literals;
pub(crate) mod parsing;
pub(crate) mod planning;
pub mod quality;
pub(crate) mod registry;
pub mod result_value;
pub(crate) mod snapshot;
pub(crate) mod spec_set_id;
pub(crate) mod stdlib;
pub(crate) mod string_distance;

#[cfg(not(target_arch = "wasm32"))]
pub mod deps;

#[cfg(not(target_arch = "wasm32"))]
pub mod mcp;

#[cfg(target_arch = "wasm32")]
pub mod wasm;

pub use computation::{OperationResult, VetoType};
pub use engine::{
    resolve_effective, Engine, Errors, ListedSpec, ResolvedRepository, EMBEDDED_STDLIB_REPOSITORY,
};
pub use error::{
    EngineError, EngineErrorSource, Error, ErrorDetails, ErrorKind, RegistryErrorKind,
    RequestErrorKind,
};
pub use evaluation::explanations::{format_explanation, Cause, Explanation, ExplanationNode};
pub use evaluation::response::{Response, RuleResult};
pub use evaluation::run_data::{
    parse_run_data_object, resolve_run_rules, run_data_value_from_json_value, RunDataValue,
};
pub use formatting::{format_parse_result, format_source, format_specs};
pub use limits::{
    ResourceLimits, MAX_DATA_NAME_LENGTH, MAX_RULE_NAME_LENGTH, MAX_SPEC_NAME_LENGTH,
};
pub use literals::{DateGranularity, MeasureUnit, MeasureUnits, RatioUnit, RatioUnits};
pub use parsing::ast::DateTimeValue;
pub use parsing::source::SourceType;
pub use planning::execution_plan::type_detail_lines;
pub use planning::execution_plan::{
    Show, ShowBranch, ShowConversionTarget, ShowData, ShowExpression, ShowRule, ShowVersion,
};
pub use planning::explanation::{ConversionTraceRole, SerializedConversionTraceStep};
pub use planning::semantics::{DataPath, LemmaType, LiteralValue, TypeSpecification, ValueKind};
pub use result_value::{CalendarResult, RangeResult, RuleResultValue, RuleResultValueFailure};
pub use spec_set_id::parse_spec_set_id;
pub use stdlib::UNITS_LEMMA;

/// Exact rational helpers for in-tree integration tests. Not a supported consumer API.
#[doc(hidden)]
pub mod __test_support {
    pub use crate::computation::rational::{
        checked_div, checked_mul, decimal_to_rational, rational_new,
    };
    pub use crate::literals::TimeValue;
    pub use crate::planning::semantics::{
        SemanticDateTime, SemanticTime, SemanticTimezone, TypedLiteral,
    };

    /// Serializes an [`Error`](crate::Error) as [`EngineError`](crate::EngineError).
    pub fn current_binding_error_json(error: &crate::Error) -> serde_json::Value {
        serde_json::to_value(crate::EngineError::from(error))
            .expect("BUG: EngineError must serialize")
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub use fixture_transport::FixtureTransport;

    #[cfg(not(target_arch = "wasm32"))]
    mod fixture_transport {
        use crate::registry::{Fetch, Header, HttpResponse, HttpTransport, TransportFailure};
        use std::collections::HashMap;
        use std::path::{Path, PathBuf};

        /// Offline [`HttpTransport`] over bundled (or custom) LemmaBase fixture files.
        ///
        /// Panics if asked for a URL that is not a LemmaBase source URL.
        pub struct FixtureTransport {
            fixtures: HashMap<String, String>,
        }

        impl FixtureTransport {
            pub fn new(dir: impl AsRef<Path>) -> Self {
                let dir = dir.as_ref();
                let mut fixtures = HashMap::new();
                collect_fixture_files(dir, dir, &mut fixtures);
                Self { fixtures }
            }

            pub fn bundled() -> Self {
                Self::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/registry_fixtures"))
            }
        }

        impl HttpTransport for FixtureTransport {
            fn get(&self, fetch: &Fetch) -> Result<HttpResponse, TransportFailure> {
                match self.fixtures.get(&fetch.repository) {
                    Some(body) => Ok(HttpResponse {
                        status: 200,
                        headers: Vec::<Header>::new(),
                        body: body.clone(),
                    }),
                    None => Ok(HttpResponse {
                        status: 404,
                        headers: Vec::new(),
                        body: String::new(),
                    }),
                }
            }
        }

        fn collect_fixture_files(dir: &Path, base: &Path, fixtures: &mut HashMap<String, String>) {
            let entries = std::fs::read_dir(dir)
                .unwrap_or_else(|e| panic!("BUG: read fixture dir {}: {e}", dir.display()));
            for entry in entries {
                let entry = entry
                    .unwrap_or_else(|e| panic!("BUG: fixture dir entry in {}: {e}", dir.display()));
                let path = entry.path();
                if path.is_dir() {
                    collect_fixture_files(&path, base, fixtures);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "lemma") {
                    continue;
                }
                let relative = path.strip_prefix(base).unwrap_or_else(|_| {
                    panic!("BUG: fixture path not under base: {}", path.display())
                });
                let identifier = relative
                    .with_extension("")
                    .to_string_lossy()
                    .replace('\\', "/");
                let content = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("BUG: read fixture {}: {e}", path.display()));
                fixtures.insert(identifier, content);
            }
        }
    }
}

// Tier 1 — language surface (always)
pub use parsing::ast::{
    try_parse_type_constraint_command, DataValue, Span, SpecRef, TimezoneValue,
};
pub use parsing::lexer::{Lexer, TokenKind};
pub use parsing::source::Source;
pub use parsing::{parse, ParseResult};
pub use quality::Recommendation;

// Tier 2 — registry resolution (sans-IO; hosts supply HttpTransport)
pub use engine::Context;
pub use parsing::ast::{LemmaRepository, LemmaSpec, RepositoryQualifier};
pub use planning::LemmaSpecSet;
pub use registry::{
    Fetch, Header, HttpResponse, HttpTransport, Install, InstallStep, LemmaBase, Registries,
    Registry, RegistryBundle, RegistryError, RepositoryInstallResult, Resolve, ResolveStep,
    TransportFailure,
};
