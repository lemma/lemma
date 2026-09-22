use crate::error::EngineError;
use crate::{Engine, Error, ResourceLimits, SourceType};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[wasm_bindgen(js_name = Engine)]
pub struct WasmEngine {
    engine: Engine,
}

impl Default for WasmEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen(js_class = "Engine")]
impl WasmEngine {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        console_error_panic_hook::set_once();
        WasmEngine {
            engine: Engine::new(),
        }
    }

    /// Create an engine with custom [`crate::ResourceLimits`].
    ///
    /// Pass a plain object of limit keys (same names as Rust / Java). Unknown keys throw.
    #[wasm_bindgen(js_name = withLimits)]
    pub fn with_limits(limits: JsValue) -> Result<WasmEngine, JsValue> {
        console_error_panic_hook::set_once();
        let map: HashMap<String, f64> = serde_wasm_bindgen::from_value(limits)
            .map_err(|e| js_err(format!("invalid limits: {e}")))?;
        let mut resource_limits = crate::ResourceLimits::default();
        for (key, value) in map {
            let n = crate::limits::usize_limit_from_f64(&key, value).map_err(js_err)?;
            resource_limits.apply(&key, n).map_err(js_err)?;
        }
        Ok(WasmEngine {
            engine: Engine::with_limits(resource_limits),
        })
    }

    /// Restore an engine from [`Engine::snapshot`] bytes.
    #[wasm_bindgen(js_name = fromSnapshot)]
    pub fn from_snapshot(bytes: &[u8]) -> Result<WasmEngine, JsValue> {
        console_error_panic_hook::set_once();
        Ok(WasmEngine {
            engine: Engine::from_snapshot(bytes).map_err(|e| error_to_js(&e))?,
        })
    }

    /// Load Lemma source(s).
    ///
    /// - string → one volatile source in the default repository
    /// - plain object or `[label, code][]` → labeled sources in one planning pass
    ///
    /// Throws with an array of serialized errors on failure. `null` / `undefined` are rejected.
    #[wasm_bindgen(js_name = load)]
    pub fn load_wasm(&mut self, sources: JsValue) -> Result<(), JsValue> {
        let batch = parse_load_sources(sources)?;
        self.engine.load(batch).map_err(serialize_load_errors)
    }

    /// Evaluate spec. Returns [`crate::evaluation::Response`] as a JS object. Throws on planning/runtime error.
    ///
    /// Accepts an options object: `{ spec, repository?, effective?, data?, rules?, explain? }`.
    #[wasm_bindgen(js_name = run)]
    pub fn run(&self, options: JsValue) -> Result<JsValue, JsValue> {
        let data_js = js_sys::Reflect::get(&options, &JsValue::from_str("data"))
            .unwrap_or(JsValue::UNDEFINED);

        let opts: RunOptions = serde_wasm_bindgen::from_value(options)
            .map_err(|e| js_err(format!("invalid run options: {e}")))?;

        let effective_dt =
            crate::resolve_effective(opts.effective.as_deref()).map_err(|e| error_to_js(&e))?;

        let data = parse_run_data_js(&data_js).map_err(js_err)?;

        let repo = opts
            .repository
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let response_rules = crate::resolve_run_rules(&opts.rules).map_err(js_err)?;

        let response = self
            .engine
            .run(
                repo,
                &opts.spec,
                Some(&effective_dt),
                data,
                response_rules.as_deref(),
                opts.explain.unwrap_or(false),
            )
            .map_err(|e| error_to_js(&e))?;

        serialize_engine_json(&crate::api::Response::from(&response))
    }

    /// Catalog of loaded repositories and specs (metadata only, no source).
    #[wasm_bindgen(js_name = list)]
    pub fn list_wasm(&self) -> Result<JsValue, JsValue> {
        let repos = self.engine.list();
        serialize_engine_json(&repos)
    }

    /// Download Lemma source for a LemmaBase repository identifier via the host's
    /// global `fetch`. Returns `{ source, id }`. Does not load this engine.
    #[wasm_bindgen(js_name = install)]
    pub fn install_wasm(&self, name: &str) -> js_sys::Promise {
        let name = name.to_string();
        let limits = self.engine.limits().clone();
        wasm_bindgen_futures::future_to_promise(async move { wasm_install(name, limits).await })
    }

    /// Spec data catalog and temporal window at `effective`. Lemma text is [`Self::source`].
    #[wasm_bindgen(js_name = show)]
    pub fn show_wasm(
        &self,
        repository: Option<String>,
        spec: &str,
        effective: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let effective_dt =
            crate::resolve_effective(effective.as_deref()).map_err(|e| error_to_js(&e))?;
        let repo = repository
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let view = self
            .engine
            .show(repo, spec, Some(&effective_dt))
            .map_err(|e| error_to_js(&e))?;
        serialize_engine_json(&crate::api::Show::from(&view))
    }

    /// Formatted canonical Lemma source. Omit `spec` for whole-repository text.
    #[wasm_bindgen(js_name = source)]
    pub fn source_wasm(
        &self,
        repository: Option<String>,
        spec: Option<String>,
        effective: Option<String>,
    ) -> Result<String, JsValue> {
        let repo = repository
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let spec_name = spec.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let effective_dt = match (spec_name, effective) {
            (Some(_), eff) => {
                Some(crate::resolve_effective(eff.as_deref()).map_err(|e| error_to_js(&e))?)
            }
            _ => None,
        };
        self.engine
            .source(repo, spec_name, effective_dt.as_ref())
            .map_err(|e| error_to_js(&e))
    }

    /// Remove a temporal spec slice. `effective`: ISO datetime string or omit for now.
    #[wasm_bindgen(js_name = remove)]
    pub fn remove_wasm(
        &mut self,
        repository: Option<String>,
        spec: &str,
        effective: Option<String>,
    ) -> Result<(), JsValue> {
        let (repo, effective_dt) = parse_repo_and_effective(repository.as_ref(), effective)?;
        self.engine
            .remove(repo, spec, effective_dt.as_ref())
            .map_err(|e| error_to_js(&e))
    }

    /// Replace identities in `code` (atomic upsert; Path/Dependency prune siblings).
    ///
    /// `attribute` is the source label (path or `@owner/repo`). Omit for a volatile source.
    #[wasm_bindgen(js_name = update)]
    pub fn update_wasm(
        &mut self,
        repository: Option<String>,
        code: &str,
        attribute: Option<String>,
    ) -> Result<(), JsValue> {
        let repo = repository
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let source_type = match attribute
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            None => SourceType::Volatile,
            Some(label) => SourceType::from_binding_label(label).map_err(js_err)?,
        };
        self.engine
            .update(repo, code.to_string(), source_type)
            .map_err(serialize_load_errors)
    }

    /// Resource limits configured for this engine.
    #[wasm_bindgen(js_name = limits)]
    pub fn limits_wasm(&self) -> Result<JsValue, JsValue> {
        serialize_engine_json(self.engine.limits())
    }

    /// Persist parsed specs + plans + limits as opaque bytes (see [`Engine::snapshot`]).
    #[wasm_bindgen(js_name = snapshot)]
    pub fn snapshot_wasm(&self) -> Result<Vec<u8>, JsValue> {
        self.engine.snapshot().map_err(|e| error_to_js(&e))
    }

    /// Returns formatted source string on success; throws with error message on failure.
    #[wasm_bindgen(js_name = format)]
    pub fn format_wasm(&self, code: &str, attribute: Option<String>) -> Result<JsValue, JsValue> {
        let attr = attribute
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("inline source (no path)");
        match crate::format_source(
            code,
            crate::parsing::source::SourceType::Path(std::sync::Arc::new(
                std::path::PathBuf::from(attr),
            )),
        ) {
            Ok(formatted) => Ok(JsValue::from_str(&formatted)),
            Err(e) => Err(error_to_js(&e)),
        }
    }

    /// Structural quality recommendations across loaded specs. Advisory only.
    #[wasm_bindgen(js_name = quality)]
    pub fn quality_wasm(&self) -> Result<JsValue, JsValue> {
        serialize_engine_json(&self.engine.quality())
    }
}

fn parse_repo_and_effective(
    repository: Option<&String>,
    effective: Option<String>,
) -> Result<(Option<&str>, Option<crate::DateTimeValue>), JsValue> {
    let effective_dt = match effective {
        None => None,
        Some(s) => Some(crate::resolve_effective(Some(s.as_str())).map_err(|e| error_to_js(&e))?),
    };
    let repo = repository
        .map(|s| s.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    Ok((repo, effective_dt))
}

#[derive(Deserialize)]
struct RunOptions {
    spec: String,
    repository: Option<String>,
    effective: Option<String>,
    rules: Option<serde_json::Value>,
    explain: Option<bool>,
}

/// Convert npm `data` object to overlay strings for [`Engine::run`].
///
/// Reads the plain JS object directly so `bigint` and digit strings reach Rust
/// without `serde_json::Value` (which cannot hold BigInt above `u64`).
fn parse_run_data_js(data: &JsValue) -> Result<HashMap<String, String>, String> {
    if data.is_undefined() || data.is_null() {
        return Ok(HashMap::new());
    }
    if js_sys::Array::is_array(data) {
        return Err("data must be a plain object".to_string());
    }
    let obj = data
        .dyn_ref::<js_sys::Object>()
        .ok_or_else(|| "data must be a plain object".to_string())?;
    let entries = js_sys::Object::entries(obj);
    let mut out = HashMap::with_capacity(entries.length() as usize);
    for entry in entries.iter() {
        let pair = js_sys::Array::from(&entry);
        let key = pair
            .get(0)
            .as_string()
            .ok_or_else(|| "data keys must be strings".to_string())?;
        let value = pair.get(1);
        out.insert(key.clone(), run_data_overlay_string_from_js(&key, &value)?);
    }
    Ok(out)
}

fn js_magnitude_to_decimal_string(value: &JsValue) -> Result<String, String> {
    if let Some(s) = value.as_string() {
        return Ok(s);
    }
    if value.is_bigint() {
        let bigint = js_sys::BigInt::new(value).map_err(|e| js_value_message(&e))?;
        let digits = bigint
            .to_string(10)
            .map_err(|e| format!("bigint toString failed: {e:?}"))?;
        return Ok(String::from(digits));
    }
    if js_sys::Number::is_safe_integer(value) {
        let n = value.unchecked_into_f64() as i64;
        return Ok(n.to_string());
    }
    if value.as_f64().is_some() {
        return Err("decimal values must be passed as strings to preserve exactness".to_string());
    }
    Err("data magnitude must be a string, bigint, or safe integer".to_string())
}

fn run_data_overlay_string_from_js(key: &str, value: &JsValue) -> Result<String, String> {
    if let Some(s) = value.as_string() {
        return Ok(s);
    }
    if let Some(b) = value.as_bool() {
        return Ok(b.to_string());
    }
    if value.is_bigint() || js_sys::Number::is_safe_integer(value) {
        return js_magnitude_to_decimal_string(value);
    }
    if value.as_f64().is_some() {
        return Err("decimal values must be passed as strings to preserve exactness".to_string());
    }
    if value.is_null() || value.is_undefined() {
        return Err("data value must not be null".to_string());
    }
    if js_sys::Array::is_array(value) {
        return Err("data value must not be an array".to_string());
    }
    let obj = value.dyn_ref::<js_sys::Object>().ok_or_else(|| {
        "data value must be a string, boolean, bigint, safe integer, or unit map".to_string()
    })?;
    let entries = js_sys::Object::entries(obj);
    if entries.length() == 0 {
        return Err("data value object must not be empty".to_string());
    }
    let has_value = js_sys::Reflect::has(obj, &JsValue::from_str("value")).unwrap_or(false);
    let has_unit = js_sys::Reflect::has(obj, &JsValue::from_str("unit")).unwrap_or(false);
    if entries.length() == 2 && has_value && has_unit {
        return Err(
            "the {value, unit} object shape is not supported; use a unit map like {\"eur\": \"84\"}"
                .to_string(),
        );
    }
    if entries.length() != 1 {
        return Err(format!(
            "data value '{key}' must be a convenience string for run"
        ));
    }
    let pair = js_sys::Array::from(&entries.get(0));
    let unit = pair
        .get(0)
        .as_string()
        .ok_or_else(|| "data value object must be a unit map with string magnitudes".to_string())?;
    let mag_text = js_magnitude_to_decimal_string(&pair.get(1))?;
    Ok(format!("{mag_text} {unit}"))
}

/// Same JSON as CLI/HTTP.
/// `IndexMap` entries (e.g. `Response.results` → `{}`); `JSON.parse` matches browser semantics.
fn serialize_engine_json<T: Serialize>(v: &T) -> Result<JsValue, JsValue> {
    let s = serde_json::to_string(v)
        .map_err(|e| js_err(format!("BUG: serde_json::to_string failed: {}", e)))?;
    js_sys::JSON::parse(&s).map_err(|e| {
        let detail = e
            .as_string()
            .unwrap_or_else(|| "(non-string error from JSON.parse)".to_string());
        js_err(format!("BUG: JSON.parse failed: {}", detail))
    })
}

fn js_err(msg: impl Into<String>) -> JsValue {
    JsValue::from_str(&msg.into())
}

/// Serializer that emits `null` (not `undefined`) for missing optionals so the object
/// matches the published `EngineError` TypeScript type.
fn js_error_serializer() -> serde_wasm_bindgen::Serializer {
    serde_wasm_bindgen::Serializer::new().serialize_missing_as_null(true)
}

fn serialize_engine_errors(errors: &[EngineError]) -> JsValue {
    errors
        .serialize(&js_error_serializer())
        .expect("BUG: serialize EngineError array")
}

/// Convert an engine [`Error`] into a plain JS object thrown from WASM.
fn error_to_js(e: &Error) -> JsValue {
    let err = EngineError::from(e);
    err.serialize(&js_error_serializer())
        .expect("BUG: serialize EngineError")
}

fn serialize_load_errors(load_err: crate::Errors) -> JsValue {
    let errors: Vec<EngineError> = load_err.errors.iter().map(EngineError::from).collect();
    serialize_engine_errors(&errors)
}

fn request_error_js(message: impl Into<String>) -> JsValue {
    serialize_engine_errors(&[EngineError::from(&Error::request(message, None::<String>))])
}

fn errors_to_js(error: &Error) -> JsValue {
    serialize_engine_errors(&[EngineError::from(error)])
}

fn js_value_message(value: &JsValue) -> String {
    use wasm_bindgen::JsCast;
    if let Some(s) = value.as_string() {
        return s;
    }
    if let Some(obj) = value.dyn_ref::<js_sys::Object>() {
        if let Ok(msg) = js_sys::Reflect::get(obj, &JsValue::from_str("message")) {
            if let Some(s) = msg.as_string() {
                return s;
            }
        }
    }
    format!("{value:?}")
}

fn parse_load_sources(sources: JsValue) -> Result<Vec<(SourceType, String)>, JsValue> {
    if sources.is_undefined() || sources.is_null() {
        return Err(request_error_js(
            "load: sources must be a string, plain object, or array of [label, code] pairs"
                .to_string(),
        ));
    }
    if let Some(code) = sources.as_string() {
        return Ok(vec![(SourceType::Volatile, code)]);
    }
    if sources.is_array() {
        let arr = js_sys::Array::from(&sources);
        let mut batch = Vec::with_capacity(arr.length() as usize);
        for (i, item) in arr.iter().enumerate() {
            if !item.is_array() {
                return Err(request_error_js(format!(
                    "load: entry {i} must be a [label, code] pair"
                )));
            }
            let pair = js_sys::Array::from(&item);
            if pair.length() != 2 {
                return Err(request_error_js(format!(
                    "load: entry {i} must be a [label, code] pair"
                )));
            }
            let label = pair.get(0).as_string().ok_or_else(|| {
                request_error_js(format!("load: entry {i} label must be a string"))
            })?;
            let code = pair.get(1).as_string().ok_or_else(|| {
                request_error_js(format!("load: entry {i} code must be a string"))
            })?;
            let source_type = SourceType::from_binding_label(&label)
                .map_err(|e| request_error_js(format!("load: entry {i}: {e}")))?;
            batch.push((source_type, code));
        }
        return Ok(batch);
    }
    let map: IndexMap<String, String> = serde_wasm_bindgen::from_value(sources).map_err(|e| {
        request_error_js(format!(
            "load: sources must be a string, plain object, or array of [label, code] pairs: {e}"
        ))
    })?;
    map.into_iter()
        .map(|(label, code)| {
            SourceType::from_binding_label(&label)
                .map(|source_type| (source_type, code))
                .map_err(|e| request_error_js(format!("load: label '{label}': {e}")))
        })
        .collect()
}

fn registries() -> &'static crate::Registries {
    use std::sync::OnceLock;
    static REGISTRIES: OnceLock<crate::Registries> = OnceLock::new();
    REGISTRIES.get_or_init(crate::Registries::default)
}

async fn wasm_install(name: String, limits: ResourceLimits) -> Result<JsValue, JsValue> {
    use crate::registry::{Install, InstallStep};

    let (mut install, mut step) = Install::start(registries(), &name, limits);
    loop {
        match step {
            InstallStep::Finished(Ok(result)) => {
                return serialize_engine_json(&result);
            }
            InstallStep::Finished(Err(error)) => {
                return Err(errors_to_js(&error));
            }
            InstallStep::Fetch(fetch) => {
                let response = global_fetch(&fetch).await;
                step = install.respond(response);
            }
        }
    }
}

async fn global_fetch(
    fetch: &crate::registry::Fetch,
) -> Result<crate::registry::HttpResponse, crate::registry::TransportFailure> {
    use crate::registry::{Header, HttpResponse, TransportFailure};
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

    let global = js_sys::global();
    let fetch_fn = js_sys::Reflect::get(&global, &JsValue::from_str("fetch")).map_err(|e| {
        TransportFailure {
            message: format!("global fetch is not available: {}", js_value_message(&e)),
        }
    })?;
    let fetch_fn: js_sys::Function = fetch_fn.dyn_into().map_err(|_| TransportFailure {
        message: "global fetch is not a function".to_string(),
    })?;

    let init = web_sys::RequestInit::new();
    init.set_method("GET");

    let abort = web_sys::AbortController::new().map_err(|e| TransportFailure {
        message: format!("failed to create AbortController: {}", js_value_message(&e)),
    })?;
    init.set_signal(Some(&abort.signal()));

    let headers = web_sys::Headers::new().map_err(|e| TransportFailure {
        message: format!("failed to create request headers: {}", js_value_message(&e)),
    })?;
    for header in &fetch.headers {
        headers
            .set(&header.name, &header.value)
            .map_err(|e| TransportFailure {
                message: format!(
                    "failed to set request header {}: {}",
                    header.name,
                    js_value_message(&e)
                ),
            })?;
    }
    init.set_headers(&headers);

    let request = web_sys::Request::new_with_str_and_init(&fetch.url, &init).map_err(|e| {
        TransportFailure {
            message: format!(
                "invalid request URL {}: {}",
                fetch.url,
                js_value_message(&e)
            ),
        }
    })?;

    let set_timeout =
        js_sys::Reflect::get(&global, &JsValue::from_str("setTimeout")).map_err(|e| {
            TransportFailure {
                message: format!("setTimeout is not available: {}", js_value_message(&e)),
            }
        })?;
    let set_timeout: js_sys::Function = set_timeout.dyn_into().map_err(|_| TransportFailure {
        message: "setTimeout is not a function".to_string(),
    })?;
    let abort_for_timeout = abort.clone();
    let timeout_cb = Closure::once(move || {
        abort_for_timeout.abort();
    });
    set_timeout
        .call2(
            &global,
            timeout_cb.as_ref().unchecked_ref(),
            &JsValue::from_f64(30_000.0),
        )
        .map_err(|e| TransportFailure {
            message: format!("failed to schedule fetch timeout: {}", js_value_message(&e)),
        })?;
    // Kept alive until the timeout fires or the page tears down.
    timeout_cb.forget();

    let promise = fetch_fn
        .call1(&global, &request)
        .map_err(|e| TransportFailure {
            message: format!("fetch call failed: {}", js_value_message(&e)),
        })?;
    let response_value = wasm_bindgen_futures::JsFuture::from(js_sys::Promise::from(promise))
        .await
        .map_err(|e| TransportFailure {
            message: format!("fetch rejected: {}", js_value_message(&e)),
        })?;
    let response: web_sys::Response = response_value.dyn_into().map_err(|_| TransportFailure {
        message: "fetch did not return a Response".to_string(),
    })?;

    let status = response.status() as u16;
    let mut response_headers = Vec::new();
    let header_iter = js_sys::try_iter(&response.headers())
        .map_err(|e| TransportFailure {
            message: format!(
                "failed to iterate response headers: {}",
                js_value_message(&e)
            ),
        })?
        .ok_or_else(|| TransportFailure {
            message: "response headers are not iterable".to_string(),
        })?;
    for entry in header_iter {
        let entry = entry.map_err(|e| TransportFailure {
            message: format!(
                "failed to read response header entry: {}",
                js_value_message(&e)
            ),
        })?;
        let pair = js_sys::Array::from(&entry);
        if pair.length() < 2 {
            return Err(TransportFailure {
                message: "response header entry must be a [name, value] pair".to_string(),
            });
        }
        let name = pair.get(0).as_string().ok_or_else(|| TransportFailure {
            message: "response header name must be a string".to_string(),
        })?;
        let value = pair.get(1).as_string().ok_or_else(|| TransportFailure {
            message: format!("response header '{name}' value must be a string"),
        })?;
        response_headers.push(Header { name, value });
    }

    let text_promise = response.text().map_err(|e| TransportFailure {
        message: format!("failed to read response body: {}", js_value_message(&e)),
    })?;
    let text_value = wasm_bindgen_futures::JsFuture::from(text_promise)
        .await
        .map_err(|e| TransportFailure {
            message: format!("failed to await response body: {}", js_value_message(&e)),
        })?;
    let body = text_value.as_string().ok_or_else(|| TransportFailure {
        message: "response body was not a string".to_string(),
    })?;

    Ok(HttpResponse {
        status,
        headers: response_headers,
        body,
    })
}
