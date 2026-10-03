//! Safe UniFFI bindings for the .NET NuGet package.
//!
//! Methods take and return `String` / `Vec<u8>` / object handles. JSON uses
//! `serde_json` on `lemma::api` types, `EngineError`, `ResourceLimits`, and
//! install step documents. UniFFI owns the FFI; this crate has no `unsafe`.

uniffi::setup_scaffolding!();

use lemma::{DateTimeValue, Engine, EngineError, ResourceLimits, SourceType};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Planning / request failure carrying an `EngineError` JSON array.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum LemmaFfiError {
    #[error("{message}")]
    Engine {
        message: String,
        errors_json: String,
    },
    #[error("{message}")]
    Bug { message: String },
}

fn registries() -> &'static lemma::Registries {
    static REGISTRIES: OnceLock<lemma::Registries> = OnceLock::new();
    REGISTRIES.get_or_init(lemma::Registries::default)
}

fn engine_errors_json(errors: &[lemma::Error]) -> String {
    let rows: Vec<EngineError> = errors.iter().map(EngineError::from).collect();
    serde_json::to_string(&rows).expect("BUG: EngineError array JSON serialization failed")
}

fn single_error(err: lemma::Error) -> LemmaFfiError {
    let message = err.message().to_string();
    LemmaFfiError::Engine {
        message,
        errors_json: engine_errors_json(std::slice::from_ref(&err)),
    }
}

fn load_errors(load_err: lemma::Errors) -> LemmaFfiError {
    LemmaFfiError::Engine {
        message: "load failed".to_string(),
        errors_json: engine_errors_json(&load_err.errors),
    }
}

fn update_errors(load_err: lemma::Errors) -> LemmaFfiError {
    LemmaFfiError::Engine {
        message: "update failed".to_string(),
        errors_json: engine_errors_json(&load_err.errors),
    }
}

fn bug(message: impl Into<String>) -> LemmaFfiError {
    LemmaFfiError::Bug {
        message: message.into(),
    }
}

fn lock_engine(engine: &Mutex<Engine>) -> Result<std::sync::MutexGuard<'_, Engine>, LemmaFfiError> {
    engine.lock().map_err(|_| bug("BUG: Engine lock poisoned"))
}

fn lock_install<'a>(
    install: &'a Mutex<lemma::Install<'static>>,
) -> Result<std::sync::MutexGuard<'a, lemma::Install<'static>>, LemmaFfiError> {
    match install.lock() {
        Ok(guard) => Ok(guard),
        Err(_) => Err(bug("BUG: Install lock poisoned")),
    }
}

fn parse_effective(effective: Option<String>) -> Result<Option<DateTimeValue>, LemmaFfiError> {
    match effective {
        None => Ok(None),
        Some(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                trimmed
                    .parse::<DateTimeValue>()
                    .map(Some)
                    .map_err(|e| bug(format!("Invalid effective date: {e}")))
            }
        }
    }
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn limits_from_json(raw: &str) -> Result<ResourceLimits, LemmaFfiError> {
    let value: Value = serde_json::from_str(raw).map_err(|e| {
        single_error(lemma::Error::request(
            format!("limits JSON: {e}"),
            None::<String>,
        ))
    })?;
    let obj = value.as_object().ok_or_else(|| {
        single_error(lemma::Error::request(
            "limits must be a JSON object".to_string(),
            None::<String>,
        ))
    })?;
    let mut limits = ResourceLimits::default();
    for (key, value) in obj {
        let number = value.as_u64().ok_or_else(|| {
            single_error(lemma::Error::request(
                format!("limits value for '{key}' must be a non-negative integer"),
                None::<String>,
            ))
        })?;
        let as_usize = usize::try_from(number).map_err(|_| {
            single_error(lemma::Error::request(
                format!("limits value for '{key}' exceeds platform usize maximum"),
                None::<String>,
            ))
        })?;
        limits
            .apply(key, as_usize)
            .map_err(|message| single_error(lemma::Error::request(message, None::<String>)))?;
    }
    Ok(limits)
}

fn sources_from_labels(
    labels: Vec<String>,
    codes: Vec<String>,
) -> Result<Vec<(SourceType, String)>, LemmaFfiError> {
    if labels.len() != codes.len() {
        return Err(single_error(lemma::Error::request(
            "load: labels and codes length mismatch".to_string(),
            None::<String>,
        )));
    }
    labels
        .into_iter()
        .zip(codes)
        .enumerate()
        .map(|(i, (label, code))| {
            SourceType::from_binding_label(&label)
                .map(|source_type| (source_type, code))
                .map_err(|e| {
                    single_error(lemma::Error::request(
                        format!("load: entry {i}: {e}"),
                        None::<String>,
                    ))
                })
        })
        .collect()
}

fn serialize_install_step(step: &lemma::InstallStep) -> String {
    let value = match step {
        lemma::InstallStep::Fetch(fetch) => json!({ "fetch": fetch }),
        lemma::InstallStep::Finished(Ok(result)) => {
            let ok = serde_json::to_value(result)
                .unwrap_or_else(|e| panic!("BUG: RepositoryInstallResult serialize failed: {e}"));
            json!({ "finished": { "ok": ok } })
        }
        lemma::InstallStep::Finished(Err(error)) => {
            let err = EngineError::from(error);
            json!({ "finished": { "err": [err] } })
        }
    };
    serde_json::to_string(&value).expect("BUG: install step serialization failed")
}

fn headers_from_json(raw: &str) -> Result<Vec<lemma::Header>, LemmaFfiError> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| bug(format!("BUG: headers JSON parse failed: {e}")))?;
    let arr = value
        .as_array()
        .ok_or_else(|| bug("BUG: headers JSON must be an array"))?;
    let mut headers = Vec::with_capacity(arr.len());
    for item in arr {
        let obj = item
            .as_object()
            .ok_or_else(|| bug("BUG: header entry must be an object"))?;
        let name = obj
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| bug("BUG: header missing name"))?;
        let value = obj
            .get("value")
            .and_then(|v| v.as_str())
            .ok_or_else(|| bug("BUG: header missing value"))?;
        headers.push(lemma::Header {
            name: name.to_string(),
            value: value.to_string(),
        });
    }
    Ok(headers)
}

/// UniFFI engine handle. Handwritten C# `Engine` wraps this.
#[derive(uniffi::Object)]
pub struct NativeEngine {
    inner: Mutex<Engine>,
}

/// Sans-IO install session. Host HTTP lives in C#.
#[derive(uniffi::Object)]
pub struct NativeInstall {
    inner: Mutex<lemma::Install<'static>>,
}

/// First step plus session from [`NativeEngine::install_start`].
#[derive(uniffi::Record)]
pub struct InstallStart {
    pub session: Arc<NativeInstall>,
    pub step_json: String,
}

#[uniffi::export]
impl NativeEngine {
    #[uniffi::constructor]
    pub fn create() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Engine::new()),
        })
    }

    #[uniffi::constructor]
    pub fn create_with_limits(limits_json: String) -> Result<Arc<Self>, LemmaFfiError> {
        let limits = limits_from_json(&limits_json)?;
        Ok(Arc::new(Self {
            inner: Mutex::new(Engine::with_limits(limits)),
        }))
    }

    #[uniffi::constructor]
    pub fn from_snapshot(bytes: Vec<u8>) -> Result<Arc<Self>, LemmaFfiError> {
        let engine = Engine::from_snapshot(&bytes).map_err(single_error)?;
        Ok(Arc::new(Self {
            inner: Mutex::new(engine),
        }))
    }

    pub fn load(&self, code: String) -> Result<(), LemmaFfiError> {
        let mut engine = lock_engine(&self.inner)?;
        engine
            .load(vec![(SourceType::Volatile, code)])
            .map_err(load_errors)
    }

    pub fn load_labeled(
        &self,
        labels: Vec<String>,
        codes: Vec<String>,
    ) -> Result<(), LemmaFfiError> {
        let batch = sources_from_labels(labels, codes)?;
        let mut engine = lock_engine(&self.inner)?;
        engine.load(batch).map_err(load_errors)
    }

    pub fn list(&self) -> Result<String, LemmaFfiError> {
        let engine = lock_engine(&self.inner)?;
        serde_json::to_string(&engine.list())
            .map_err(|e| bug(format!("BUG: list serialization failed: {e}")))
    }

    pub fn show(
        &self,
        repository: Option<String>,
        spec: String,
        effective: Option<String>,
    ) -> Result<String, LemmaFfiError> {
        let engine = lock_engine(&self.inner)?;
        let effective = parse_effective(effective)?;
        let repo = trim_optional(repository);
        let view = engine
            .show(repo.as_deref(), &spec, effective.as_ref())
            .map_err(single_error)?;
        serde_json::to_string(&lemma::api::Show::from(&view))
            .map_err(|e| bug(format!("BUG: Show serialization failed: {e}")))
    }

    pub fn source(
        &self,
        repository: Option<String>,
        spec: Option<String>,
        effective: Option<String>,
    ) -> Result<String, LemmaFfiError> {
        let engine = lock_engine(&self.inner)?;
        let repo = trim_optional(repository);
        let spec_name = trim_optional(spec);
        let effective = match (&spec_name, effective) {
            (Some(_), Some(s)) => parse_effective(Some(s))?,
            (Some(_), None) => None,
            _ => None,
        };
        engine
            .source(repo.as_deref(), spec_name.as_deref(), effective.as_ref())
            .map_err(single_error)
    }

    pub fn run(
        &self,
        repository: Option<String>,
        spec: String,
        effective: Option<String>,
        data: HashMap<String, String>,
        rules: Option<Vec<String>>,
        explain: bool,
    ) -> Result<String, LemmaFfiError> {
        let engine = lock_engine(&self.inner)?;
        let effective = parse_effective(effective)?;
        let repo = trim_optional(repository);
        let rules = match rules {
            None => None,
            Some(names) if names.is_empty() => {
                return Err(single_error(lemma::Error::request(
                    "rules must not be empty".to_string(),
                    None::<String>,
                )));
            }
            Some(names) => Some(names),
        };
        let response = engine
            .run(
                repo.as_deref(),
                &spec,
                effective.as_ref(),
                data,
                rules.as_deref(),
                explain,
            )
            .map_err(single_error)?;
        serde_json::to_string(&lemma::api::Response::from(&response))
            .map_err(|e| bug(format!("BUG: Response serialization failed: {e}")))
    }

    pub fn remove(
        &self,
        repository: Option<String>,
        spec: String,
        effective: Option<String>,
    ) -> Result<(), LemmaFfiError> {
        let mut engine = lock_engine(&self.inner)?;
        let effective = parse_effective(effective)?;
        let repo = trim_optional(repository);
        engine
            .remove(repo.as_deref(), &spec, effective.as_ref())
            .map_err(single_error)
    }

    pub fn update(
        &self,
        repository: Option<String>,
        code: String,
        attribute: Option<String>,
    ) -> Result<(), LemmaFfiError> {
        let mut engine = lock_engine(&self.inner)?;
        let repo = trim_optional(repository);
        let source_type = match trim_optional(attribute) {
            None => SourceType::Volatile,
            Some(label) => SourceType::from_binding_label(&label).map_err(|e| {
                single_error(lemma::Error::request(
                    format!("update: label '{label}': {e}"),
                    None::<String>,
                ))
            })?,
        };
        engine
            .update(repo.as_deref(), code, source_type)
            .map_err(update_errors)
    }

    pub fn limits(&self) -> Result<String, LemmaFfiError> {
        let engine = lock_engine(&self.inner)?;
        serde_json::to_string(engine.limits())
            .map_err(|e| bug(format!("BUG: limits serialization failed: {e}")))
    }

    pub fn snapshot(&self) -> Result<Vec<u8>, LemmaFfiError> {
        let engine = lock_engine(&self.inner)?;
        engine.snapshot().map_err(single_error)
    }

    pub fn quality(&self) -> Result<String, LemmaFfiError> {
        let engine = lock_engine(&self.inner)?;
        serde_json::to_string(&engine.quality())
            .map_err(|e| bug(format!("BUG: quality serialization failed: {e}")))
    }

    pub fn install_start(&self, repository: String) -> Result<InstallStart, LemmaFfiError> {
        let limits = {
            let engine = lock_engine(&self.inner)?;
            engine.limits().clone()
        };
        let (install, step) = lemma::Install::start(registries(), &repository, limits);
        Ok(InstallStart {
            session: Arc::new(NativeInstall {
                inner: Mutex::new(install),
            }),
            step_json: serialize_install_step(&step),
        })
    }
}

#[uniffi::export]
impl NativeInstall {
    pub fn respond(
        &self,
        status: u16,
        headers_json: String,
        body: String,
    ) -> Result<String, LemmaFfiError> {
        let headers = headers_from_json(&headers_json)?;
        let response = Ok(lemma::HttpResponse {
            status,
            headers,
            body,
        });
        let mut install = lock_install(&self.inner)?;
        let step = install.respond(response);
        Ok(serialize_install_step(&step))
    }

    pub fn fail(&self, message: String) -> Result<String, LemmaFfiError> {
        let response = Err(lemma::TransportFailure { message });
        let mut install = lock_install(&self.inner)?;
        let step = install.respond(response);
        Ok(serialize_install_step(&step))
    }
}

#[uniffi::export]
pub fn format_source(code: String) -> Result<String, LemmaFfiError> {
    lemma::format_source(&code, SourceType::Volatile).map_err(single_error)
}

/// Sparse limits JSON: apply overrides onto defaults (for hosts that only send set keys).
#[uniffi::export]
pub fn apply_limits_json(overrides_json: String) -> Result<String, LemmaFfiError> {
    let limits = limits_from_json(&overrides_json)?;
    serde_json::to_string(&limits)
        .map_err(|e| bug(format!("BUG: limits serialization failed: {e}")))
}

/// Build a request-shaped EngineError JSON array for host-side failures.
#[uniffi::export]
pub fn request_error_json(message: String, related_data: Option<String>) -> String {
    let err = lemma::Error::request(message, related_data);
    engine_errors_json(std::slice::from_ref(&err))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_rejects_empty_id() {
        let engine = NativeEngine::create();
        let start = engine
            .install_start("   ".to_string())
            .expect("install_start returns finished error step");
        assert!(start.step_json.contains("\"finished\""));
        assert!(start.step_json.contains("\"err\""));
    }

    #[test]
    fn limits_sparse_apply() {
        let json = apply_limits_json(r#"{"max_sources":12}"#.to_string()).expect("apply");
        let value: Value = serde_json::from_str(&json).expect("json");
        assert_eq!(value["max_sources"], 12);
    }
}
