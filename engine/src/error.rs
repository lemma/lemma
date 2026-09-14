use crate::parsing::ast::{EffectiveDate, LemmaSpec};
use crate::parsing::source::Source;
use std::fmt;

/// The kind of failure that occurred during a Registry operation.
///
/// Registry implementations classify their errors into these kinds so that
/// the engine (and ultimately the user) can distinguish between a missing
/// spec, an authorization failure, a network outage, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistryErrorKind {
    /// The requested spec or type was not found (e.g. HTTP 404).
    NotFound,
    /// The request was unauthorized or forbidden (e.g. HTTP 401, 403).
    Unauthorized,
    /// A network or transport error occurred (DNS failure, timeout, connection refused).
    NetworkError,
    /// The registry server returned an internal error (e.g. HTTP 5xx).
    ServerError,
    /// An error that does not fit the other categories.
    Other,
}

impl fmt::Display for RegistryErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "not found"),
            Self::Unauthorized => write!(f, "unauthorized"),
            Self::NetworkError => write!(f, "network error"),
            Self::ServerError => write!(f, "server error"),
            Self::Other => write!(f, "error"),
        }
    }
}

/// Detailed error information with optional source location.
#[derive(Debug, Clone)]
pub struct ErrorDetails {
    pub message: String,
    pub source: Option<Source>,
    pub suggestion: Option<String>,
    /// Spec we were planning when this error occurred. Used for display grouping ("In spec 'X':").
    pub spec_context_name: Option<String>,
    pub spec_context_effective_from: Option<EffectiveDate>,
    /// When the cause involves a referenced spec, that temporal version. Displayed as "See spec 'X' (active from Y)."
    pub related_spec_name: Option<String>,
    pub related_spec_effective_from: Option<EffectiveDate>,
    /// Data name this error is about. Populated by the data-binding site so consumers can attribute
    /// the error to a specific input field without string parsing. Displayed as "Failed to parse data 'X':".
    pub related_data: Option<String>,
}

fn attribution_fields(spec: Option<&LemmaSpec>) -> (Option<String>, Option<EffectiveDate>) {
    match spec {
        Some(s) => (Some(s.name.clone()), Some(s.effective_from.clone())),
        None => (None, None),
    }
}

/// Classification of an [`Error`]. Serialized as the `kind` field on the flat object returned to JavaScript from WASM (`engine/src/wasm.rs`, `JsError`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Parsing,
    Validation,
    Inversion,
    Registry,
    MissingRepository,
    Request,
    ResourceLimit,
}

/// Error types for the Lemma system with source location tracking
#[derive(Debug, Clone)]
pub enum Error {
    /// Parse error with source location
    Parsing(Box<ErrorDetails>),

    /// Inversion error (valid Lemma, but unsupported by inversion) with source location
    Inversion(Box<ErrorDetails>),

    /// Validation error (semantic/planning, including circular dependency) with source location
    Validation(Box<ErrorDetails>),

    /// Registry resolution error with source location and structured error kind.
    ///
    /// Produced when an `@...` reference cannot be resolved by the configured Registry
    /// (e.g. the spec was not found, the request was unauthorized, or the network
    /// is unreachable).
    Registry {
        details: Box<ErrorDetails>,
        /// The `@...` identifier that failed to resolve (includes the leading `@`).
        identifier: String,
        /// The category of failure.
        kind: RegistryErrorKind,
    },

    /// A referenced repository is not present in the context (not loaded / not fetched).
    ///
    /// Produced during planning when a `uses @repository ...` reference names a repository
    /// qualifier that has not been loaded.
    MissingRepository {
        details: Box<ErrorDetails>,
        /// Full repository qualifier as written (e.g. `"@iso/countries"`).
        repository: String,
    },

    /// Resource limit exceeded
    ResourceLimitExceeded {
        details: Box<ErrorDetails>,
        limit_name: String,
        limit_value: String,
        actual_value: String,
    },

    /// Request error: invalid or unsatisfiable API request (e.g. spec not found, invalid parameters).
    /// Not a parse/planning failure; the request itself is invalid. Such errors occur *before* any evaluation and *never during* evaluation.
    Request {
        details: Box<ErrorDetails>,
        kind: RequestErrorKind,
    },
}

/// Distinguishes HTTP 404 (not found) from 400 (bad request) for request errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestErrorKind {
    /// Spec not found or no temporal version for effective — map to 404.
    SpecNotFound,
    /// Rule not found
    RuleNotFound,
    /// Invalid spec id, etc. — map to 400.
    InvalidRequest,
}

impl Error {
    /// Create a parse error. Source is required: parsing errors always originate from source code.
    pub fn parsing(
        message: impl Into<String>,
        source: Source,
        suggestion: Option<impl Into<String>>,
    ) -> Self {
        Self::parsing_with_context(message, source, suggestion, None, None)
    }

    /// Parse error with optional spec context (for display).
    pub fn parsing_with_context(
        message: impl Into<String>,
        source: Source,
        suggestion: Option<impl Into<String>>,
        spec_context: Option<&LemmaSpec>,
        related_spec: Option<&LemmaSpec>,
    ) -> Self {
        let (spec_context_name, spec_context_effective_from) = attribution_fields(spec_context);
        let (related_spec_name, related_spec_effective_from) = attribution_fields(related_spec);
        Self::Parsing(Box::new(ErrorDetails {
            message: message.into(),
            source: Some(source),
            suggestion: suggestion.map(Into::into),
            spec_context_name,
            spec_context_effective_from,
            related_spec_name,
            related_spec_effective_from,
            related_data: None,
        }))
    }

    /// Create a parse error with suggestion. Source is required.
    pub fn parsing_with_suggestion(
        message: impl Into<String>,
        source: Source,
        suggestion: impl Into<String>,
    ) -> Self {
        Self::parsing_with_context(message, source, Some(suggestion), None, None)
    }

    /// Create an inversion error with source information.
    pub fn inversion(
        message: impl Into<String>,
        source: Option<Source>,
        suggestion: Option<impl Into<String>>,
    ) -> Self {
        Self::inversion_with_context(message, source, suggestion, None, None)
    }

    /// Inversion error with optional spec context (for display).
    pub fn inversion_with_context(
        message: impl Into<String>,
        source: Option<Source>,
        suggestion: Option<impl Into<String>>,
        spec_context: Option<&LemmaSpec>,
        related_spec: Option<&LemmaSpec>,
    ) -> Self {
        let (spec_context_name, spec_context_effective_from) = attribution_fields(spec_context);
        let (related_spec_name, related_spec_effective_from) = attribution_fields(related_spec);
        Self::Inversion(Box::new(ErrorDetails {
            message: message.into(),
            source,
            suggestion: suggestion.map(Into::into),
            spec_context_name,
            spec_context_effective_from,
            related_spec_name,
            related_spec_effective_from,
            related_data: None,
        }))
    }

    /// Create an inversion error with suggestion
    pub fn inversion_with_suggestion(
        message: impl Into<String>,
        source: Option<Source>,
        suggestion: impl Into<String>,
        spec_context: Option<&LemmaSpec>,
        related_spec: Option<&LemmaSpec>,
    ) -> Self {
        Self::inversion_with_context(
            message,
            source,
            Some(suggestion),
            spec_context,
            related_spec,
        )
    }

    /// Create a validation error with source information (semantic/planning, including circular dependency).
    pub fn validation(
        message: impl Into<String>,
        source: Option<Source>,
        suggestion: Option<impl Into<String>>,
    ) -> Self {
        Self::validation_with_context(message, source, suggestion, None, None)
    }

    /// Validation error with optional spec context and related spec (for display).
    pub fn validation_with_context(
        message: impl Into<String>,
        source: Option<Source>,
        suggestion: Option<impl Into<String>>,
        spec_context: Option<&LemmaSpec>,
        related_spec: Option<&LemmaSpec>,
    ) -> Self {
        let (spec_context_name, spec_context_effective_from) = attribution_fields(spec_context);
        let (related_spec_name, related_spec_effective_from) = attribution_fields(related_spec);
        Self::Validation(Box::new(ErrorDetails {
            message: message.into(),
            source,
            suggestion: suggestion.map(Into::into),
            spec_context_name,
            spec_context_effective_from,
            related_spec_name,
            related_spec_effective_from,
            related_data: None,
        }))
    }

    /// Create a request error (invalid API request, e.g. bad spec id).
    /// Request errors never have source locations — they are API-level.
    pub fn request(message: impl Into<String>, suggestion: Option<impl Into<String>>) -> Self {
        Self::request_with_kind(message, suggestion, RequestErrorKind::InvalidRequest)
    }

    /// Create a "spec not found" request error — map to HTTP 404.
    pub fn request_not_found(
        message: impl Into<String>,
        suggestion: Option<impl Into<String>>,
    ) -> Self {
        Self::request_with_kind(message, suggestion, RequestErrorKind::SpecNotFound)
    }

    /// Create a rule not found error
    pub fn rule_not_found(rule_name: &str, suggestion: Option<impl Into<String>>) -> Self {
        Self::request_with_kind(
            format!("Rule '{}' not found", rule_name),
            suggestion,
            RequestErrorKind::RuleNotFound,
        )
    }

    fn request_with_kind(
        message: impl Into<String>,
        suggestion: Option<impl Into<String>>,
        kind: RequestErrorKind,
    ) -> Self {
        Self::Request {
            details: Box::new(ErrorDetails {
                message: message.into(),
                source: None,
                suggestion: suggestion.map(Into::into),
                spec_context_name: None,
                spec_context_effective_from: None,
                related_spec_name: None,
                related_spec_effective_from: None,
                related_data: None,
            }),
            kind,
        }
    }

    /// Create a resource-limit-exceeded error with optional source location and spec context.
    pub fn resource_limit_exceeded(
        limit_name: impl Into<String>,
        limit_value: impl Into<String>,
        actual_value: impl Into<String>,
        suggestion: impl Into<String>,
        source: Option<Source>,
        spec_context: Option<&LemmaSpec>,
        related_spec: Option<&LemmaSpec>,
    ) -> Self {
        let limit_name = limit_name.into();
        let limit_value = limit_value.into();
        let actual_value = actual_value.into();
        let message = format!("{limit_name} (limit: {limit_value}, actual: {actual_value})");
        let (spec_context_name, spec_context_effective_from) = attribution_fields(spec_context);
        let (related_spec_name, related_spec_effective_from) = attribution_fields(related_spec);
        Self::ResourceLimitExceeded {
            details: Box::new(ErrorDetails {
                message,
                source,
                suggestion: Some(suggestion.into()),
                spec_context_name,
                spec_context_effective_from,
                related_spec_name,
                related_spec_effective_from,
                related_data: None,
            }),
            limit_name,
            limit_value,
            actual_value,
        }
    }

    /// Create a registry error. Source is required: registry errors point to `@ref` in source.
    pub fn registry(
        message: impl Into<String>,
        source: Source,
        identifier: impl Into<String>,
        kind: RegistryErrorKind,
        suggestion: Option<impl Into<String>>,
        spec_context: Option<&LemmaSpec>,
        related_spec: Option<&LemmaSpec>,
    ) -> Self {
        let (spec_context_name, spec_context_effective_from) = attribution_fields(spec_context);
        let (related_spec_name, related_spec_effective_from) = attribution_fields(related_spec);
        Self::Registry {
            details: Box::new(ErrorDetails {
                message: message.into(),
                source: Some(source),
                suggestion: suggestion.map(Into::into),
                spec_context_name,
                spec_context_effective_from,
                related_spec_name,
                related_spec_effective_from,
                related_data: None,
            }),
            identifier: identifier.into(),
            kind,
        }
    }

    /// Repository referenced in source is not loaded in the context.
    pub fn missing_repository(
        message: impl Into<String>,
        source: Option<Source>,
        repository: impl Into<String>,
        suggestion: Option<impl Into<String>>,
        spec_context: Option<&LemmaSpec>,
    ) -> Self {
        let (spec_context_name, spec_context_effective_from) = attribution_fields(spec_context);
        Self::MissingRepository {
            details: Box::new(ErrorDetails {
                message: message.into(),
                source,
                suggestion: suggestion.map(Into::into),
                spec_context_name,
                spec_context_effective_from,
                related_spec_name: None,
                related_spec_effective_from: None,
                related_data: None,
            }),
            repository: repository.into(),
        }
    }

    /// Attach spec context for display grouping. Returns a new Error with context set.
    pub fn with_spec_context(self, spec: &LemmaSpec) -> Self {
        self.map_details(|d| {
            d.spec_context_name = Some(spec.name.clone());
            d.spec_context_effective_from = Some(spec.effective_from.clone());
        })
    }

    /// Attach a data-binding attribution. Returns a new Error carrying the data name.
    /// Consumers (WASM `JsError`, LSP, HTTP) can read this via [`Error::related_data`] to attribute
    /// the failure to a specific input field without parsing strings.
    pub fn with_related_data(self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.map_details(|d| d.related_data = Some(name))
    }

    /// Apply a mutator to the inner [`ErrorDetails`] regardless of variant.
    fn map_details(self, f: impl FnOnce(&mut ErrorDetails)) -> Self {
        match self {
            Error::Parsing(details) => {
                let mut d = *details;
                f(&mut d);
                Error::Parsing(Box::new(d))
            }
            Error::Inversion(details) => {
                let mut d = *details;
                f(&mut d);
                Error::Inversion(Box::new(d))
            }
            Error::Validation(details) => {
                let mut d = *details;
                f(&mut d);
                Error::Validation(Box::new(d))
            }
            Error::Registry {
                details,
                identifier,
                kind,
            } => {
                let mut d = *details;
                f(&mut d);
                Error::Registry {
                    details: Box::new(d),
                    identifier,
                    kind,
                }
            }
            Error::MissingRepository {
                details,
                repository,
            } => {
                let mut d = *details;
                f(&mut d);
                Error::MissingRepository {
                    details: Box::new(d),
                    repository,
                }
            }
            Error::ResourceLimitExceeded {
                details,
                limit_name,
                limit_value,
                actual_value,
            } => {
                let mut d = *details;
                f(&mut d);
                Error::ResourceLimitExceeded {
                    details: Box::new(d),
                    limit_name,
                    limit_value,
                    actual_value,
                }
            }
            Error::Request { details, kind } => {
                let mut d = *details;
                f(&mut d);
                Error::Request {
                    details: Box::new(d),
                    kind,
                }
            }
        }
    }
}

fn format_related_spec(name: &str, effective_from: &EffectiveDate) -> String {
    let effective_from_str = effective_from
        .as_ref()
        .map(|d| d.to_string())
        .unwrap_or_else(|| "beginning".to_string());
    format!(
        "See spec '{}' (effective from {}).",
        name, effective_from_str
    )
}

fn write_source_location(f: &mut fmt::Formatter<'_>, source: &Option<Source>) -> fmt::Result {
    if let Some(src) = source {
        write!(
            f,
            " at {}:{}:{}",
            src.source_type, src.span.line, src.span.col
        )
    } else {
        Ok(())
    }
}

fn write_related_spec(f: &mut fmt::Formatter<'_>, details: &ErrorDetails) -> fmt::Result {
    if let Some(ref name) = details.related_spec_name {
        let effective = details
            .related_spec_effective_from
            .as_ref()
            .expect("BUG: related_spec_name set without related_spec_effective_from");
        write!(f, " {}", format_related_spec(name, effective))?;
    }
    Ok(())
}

fn write_spec_context(f: &mut fmt::Formatter<'_>, name: &str) -> fmt::Result {
    write!(f, "In spec '{}': ", name)
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parsing(details) => {
                if let Some(ref name) = details.spec_context_name {
                    write_spec_context(f, name)?;
                }
                write!(f, "Parse error: {}", details.message)?;
                if let Some(suggestion) = &details.suggestion {
                    write!(f, " (suggestion: {suggestion})")?;
                }
                write_related_spec(f, details)?;
                write_source_location(f, &details.source)
            }
            Error::Inversion(details) => {
                if let Some(ref name) = details.spec_context_name {
                    write_spec_context(f, name)?;
                }
                write!(f, "Inversion error: {}", details.message)?;
                if let Some(suggestion) = &details.suggestion {
                    write!(f, " (suggestion: {suggestion})")?;
                }
                write_related_spec(f, details)?;
                write_source_location(f, &details.source)
            }
            Error::Validation(details) => {
                if let Some(ref name) = details.spec_context_name {
                    write_spec_context(f, name)?;
                }
                write!(f, "Validation error: ")?;
                if let Some(ref name) = details.related_data {
                    write!(f, "Failed to parse data '{}': ", name)?;
                }
                write!(f, "{}", details.message)?;
                if let Some(suggestion) = &details.suggestion {
                    write!(f, " (suggestion: {suggestion})")?;
                }
                write_related_spec(f, details)?;
                write_source_location(f, &details.source)
            }
            Error::Registry {
                details,
                identifier,
                kind,
            } => {
                if let Some(ref name) = details.spec_context_name {
                    write_spec_context(f, name)?;
                }
                write!(
                    f,
                    "Registry error ({}): {}: {}",
                    kind, identifier, details.message
                )?;
                if let Some(suggestion) = &details.suggestion {
                    write!(f, " (suggestion: {suggestion})")?;
                }
                write_related_spec(f, details)?;
                write_source_location(f, &details.source)
            }
            Error::MissingRepository {
                details,
                repository,
            } => {
                if let Some(ref name) = details.spec_context_name {
                    write_spec_context(f, name)?;
                }
                write!(f, "Missing repository: {}: {}", repository, details.message)?;
                if let Some(suggestion) = &details.suggestion {
                    write!(f, " (suggestion: {suggestion})")?;
                }
                write_related_spec(f, details)?;
                write_source_location(f, &details.source)
            }
            Error::ResourceLimitExceeded {
                details,
                limit_name,
                limit_value,
                actual_value,
            } => {
                if let Some(ref name) = details.spec_context_name {
                    write_spec_context(f, name)?;
                }
                write!(
                    f,
                    "Resource limit exceeded: {limit_name} (limit: {limit_value}, actual: {actual_value})"
                )?;
                if let Some(suggestion) = &details.suggestion {
                    write!(f, ". {suggestion}")?;
                }
                write_source_location(f, &details.source)
            }
            Error::Request { details, .. } => {
                if let Some(ref name) = details.spec_context_name {
                    write_spec_context(f, name)?;
                }
                write!(f, "Request error: {}", details.message)?;
                if let Some(suggestion) = &details.suggestion {
                    write!(f, " (suggestion: {suggestion})")?;
                }
                write_related_spec(f, details)?;
                write_source_location(f, &details.source)
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<std::fmt::Error> for Error {
    fn from(err: std::fmt::Error) -> Self {
        Error::validation(format!("Format error: {err}"), None, None::<String>)
    }
}

impl Error {
    /// Classify this error. Used by FFI/WASM consumers that need to branch on error category
    /// without depending on internal variant shapes.
    pub fn kind(&self) -> ErrorKind {
        match self {
            Error::Parsing(_) => ErrorKind::Parsing,
            Error::Validation(_) => ErrorKind::Validation,
            Error::Inversion(_) => ErrorKind::Inversion,
            Error::Registry { .. } => ErrorKind::Registry,
            Error::MissingRepository { .. } => ErrorKind::MissingRepository,
            Error::Request { .. } => ErrorKind::Request,
            Error::ResourceLimitExceeded { .. } => ErrorKind::ResourceLimit,
        }
    }

    /// Shared access to the inner [`ErrorDetails`] regardless of variant.
    pub(crate) fn details(&self) -> &ErrorDetails {
        match self {
            Error::Parsing(d) | Error::Inversion(d) | Error::Validation(d) => d,
            Error::Registry { details, .. }
            | Error::MissingRepository { details, .. }
            | Error::ResourceLimitExceeded { details, .. }
            | Error::Request { details, .. } => details,
        }
    }

    /// Repository identifier when the error is about a missing repository or a registry fetch target.
    ///
    /// Populated for [`Error::MissingRepository`] and [`Error::Registry`] (`identifier`).
    #[must_use]
    pub fn repository(&self) -> Option<&str> {
        match self {
            Error::MissingRepository { repository, .. } => Some(repository.as_str()),
            Error::Registry { identifier, .. } => Some(identifier.as_str()),
            _ => None,
        }
    }

    /// Get the error message.
    pub fn message(&self) -> &str {
        &self.details().message
    }

    /// Get the source location if available.
    pub fn location(&self) -> Option<&Source> {
        self.details().source.as_ref()
    }

    /// Alias for [`Error::location`]. Preferred name when building the WASM/JS error payload.
    pub fn source_location(&self) -> Option<&Source> {
        self.location()
    }

    /// Resolve source text from the sources map (for display). Source no longer stores text.
    pub fn source_text(
        &self,
        sources: &std::collections::HashMap<crate::parsing::source::SourceType, String>,
    ) -> Option<String> {
        self.location()
            .and_then(|s| s.text_from(sources).map(|c| c.into_owned()))
    }

    /// Get the suggestion if available.
    pub fn suggestion(&self) -> Option<&str> {
        self.details().suggestion.as_deref()
    }

    /// Data name this error is attributed to (set at the data-binding call site).
    pub fn related_data(&self) -> Option<&str> {
        self.details().related_data.as_deref()
    }

    /// Spec name when the error is attributed to a planning/eval context.
    pub fn spec_context_name(&self) -> Option<&str> {
        self.details().spec_context_name.as_deref()
    }

    /// Name of a related spec referenced by this error (e.g. a transitive dependency).
    pub fn related_spec(&self) -> Option<&str> {
        self.details().related_spec_name.as_deref()
    }

    /// Registry failure sub-kind, populated only for [`Error::Registry`].
    #[must_use]
    pub fn registry_kind(&self) -> Option<RegistryErrorKind> {
        match self {
            Error::Registry { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    /// Request failure sub-kind, populated only for [`Error::Request`].
    #[must_use]
    pub fn request_kind(&self) -> Option<RequestErrorKind> {
        match self {
            Error::Request { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    /// Name of the exceeded resource limit, populated only for [`Error::ResourceLimitExceeded`].
    #[must_use]
    pub fn limit_name(&self) -> Option<&str> {
        match self {
            Error::ResourceLimitExceeded { limit_name, .. } => Some(limit_name.as_str()),
            _ => None,
        }
    }

    /// Configured value of the exceeded resource limit, populated only for
    /// [`Error::ResourceLimitExceeded`].
    #[must_use]
    pub fn limit_value(&self) -> Option<&str> {
        match self {
            Error::ResourceLimitExceeded { limit_value, .. } => Some(limit_value.as_str()),
            _ => None,
        }
    }

    /// Actual value that exceeded the resource limit, populated only for
    /// [`Error::ResourceLimitExceeded`].
    #[must_use]
    pub fn actual_value(&self) -> Option<&str> {
        match self {
            Error::ResourceLimitExceeded { actual_value, .. } => Some(actual_value.as_str()),
            _ => None,
        }
    }
}

/// Source location attached to an [`EngineError`]. Line and column are 1-based;
/// `length` is the UTF-8 byte length of the offending span.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EngineErrorSource {
    pub attribute: String,
    pub line: usize,
    pub column: usize,
    pub length: usize,
}

impl From<&Source> for EngineErrorSource {
    fn from(source: &Source) -> Self {
        Self {
            attribute: source.source_type.to_string(),
            line: source.span.line,
            column: source.span.col,
            length: source.span.end.saturating_sub(source.span.start),
        }
    }
}

/// Flat wire view of [`Error`] matching `EngineError` in `engine/schemas/api.v1.json`
/// and `engine/packages/npm/lemma.d.ts`. Missing optionals serialize as JSON `null`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EngineError {
    pub kind: ErrorKind,
    pub message: String,
    pub related_data: Option<String>,
    pub spec: Option<String>,
    pub related_spec: Option<String>,
    pub source: Option<EngineErrorSource>,
    pub suggestion: Option<String>,
    /// Set for [`Error::MissingRepository`] and [`Error::Registry`] (registry `@` id).
    pub repository: Option<String>,
    /// Set only for [`Error::Registry`].
    pub registry_kind: Option<RegistryErrorKind>,
    /// Set only for [`Error::Request`].
    pub request_kind: Option<RequestErrorKind>,
    /// Set only for [`Error::ResourceLimitExceeded`].
    pub limit_name: Option<String>,
    pub limit_value: Option<String>,
    pub actual_value: Option<String>,
}

impl From<&Error> for EngineError {
    fn from(error: &Error) -> Self {
        Self {
            kind: error.kind(),
            message: error.message().to_string(),
            related_data: error.related_data().map(str::to_string),
            spec: error.spec_context_name().map(str::to_string),
            related_spec: error.related_spec().map(str::to_string),
            source: error.source_location().map(EngineErrorSource::from),
            suggestion: error.suggestion().map(str::to_string),
            repository: error.repository().map(str::to_string),
            registry_kind: error.registry_kind(),
            request_kind: error.request_kind(),
            limit_name: error.limit_name().map(str::to_string),
            limit_value: error.limit_value().map(str::to_string),
            actual_value: error.actual_value().map(str::to_string),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::ast::Span;

    fn test_source() -> Source {
        Source::new(
            crate::parsing::source::SourceType::Path(std::sync::Arc::new(
                std::path::PathBuf::from("test.lemma"),
            )),
            Span {
                start: 14,
                end: 21,
                line: 1,
                col: 15,
            },
        )
    }

    #[test]
    fn test_error_creation_and_display() {
        let parse_error = Error::parsing("Invalid currency", test_source(), None::<String>);
        let parse_error_display = format!("{parse_error}");
        assert!(parse_error_display.contains("Parse error: Invalid currency"));
        assert!(parse_error_display.contains("test.lemma:1:15"));

        let suggestion_source = Source::new(
            crate::parsing::source::SourceType::Volatile,
            Span {
                start: 5,
                end: 10,
                line: 2,
                col: 3,
            },
        );
        let suggestion_error =
            Error::parsing_with_suggestion("typo", suggestion_source, "did you mean X?");
        assert!(format!("{suggestion_error}").contains("suggestion: did you mean X?"));
    }

    #[test]
    fn test_request_error_accessors() {
        let err = Error::request("bad id", Some("use a valid id"));
        assert_eq!(err.kind(), ErrorKind::Request);
        assert_eq!(err.message(), "bad id");
        assert!(err.location().is_none());
        assert_eq!(err.suggestion(), Some("use a valid id"));
        assert!(err.spec_context_name().is_none());
        assert!(err.related_spec().is_none());
    }

    #[test]
    fn test_missing_repository_display() {
        let err = Error::missing_repository(
            "not loaded",
            None,
            "@iso/countries",
            Some("load the dependency first"),
            None,
        );
        let display = format!("{err}");
        assert!(display.contains("Missing repository"));
        assert!(display.contains("@iso/countries"));
        assert!(display.contains("not loaded"));
    }

    #[test]
    fn test_with_spec_context_copies_name() {
        let spec = LemmaSpec::new("pricing".to_string());
        let err = Error::validation("bad", None, None::<String>).with_spec_context(&spec);
        assert_eq!(err.spec_context_name(), Some("pricing"));
        let display = format!("{err}");
        assert!(display.contains("In spec 'pricing':"));
    }
}
