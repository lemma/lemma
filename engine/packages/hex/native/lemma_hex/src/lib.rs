#![recursion_limit = "256"]

mod error_encoding;

use error_encoding::encode_error;
use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, EvaluateDataValue, ResourceLimits, SourceType};
use rustler::types::atom;
use rustler::types::MapIterator;
use rustler::TermType;
use rustler::{Binary, Encoder, Env, NifResult, OwnedBinary, Resource, ResourceArc, Term};
use serde_json::json;
use std::sync::Mutex;

pub struct LemmaEngineResource(pub Mutex<Engine>);

fn poisoned_engine_lock() -> rustler::Error {
    rustler::Error::RaiseTerm(Box::new(
        "Engine lock poisoned by an earlier panic in the lemma engine; create a new engine. The panic message is on stderr".to_string(),
    ))
}

impl Resource for LemmaEngineResource {}

pub struct InstallResource(pub Mutex<lemma::Install<'static>>);

impl Resource for InstallResource {}

fn registries() -> &'static lemma::Registries {
    use std::sync::OnceLock;
    static REGISTRIES: OnceLock<lemma::Registries> = OnceLock::new();
    REGISTRIES.get_or_init(lemma::Registries::default)
}

fn load(env: Env, _: Term) -> bool {
    env.register::<LemmaEngineResource>().is_ok() && env.register::<InstallResource>().is_ok()
}

#[rustler::nif]
fn lemma_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[rustler::nif]
fn lemma_new<'a>(env: Env<'a>, limits_term: Option<Term<'a>>) -> NifResult<Term<'a>> {
    let engine = match limits_term {
        None => Engine::new(),
        Some(term) => {
            if term.as_c_arg() == atom::nil().as_c_arg() {
                Engine::new()
            } else {
                let limits = limits_from_term(term)
                    .map_err(|msg| rustler::Error::RaiseTerm(Box::new(msg)))?;
                Engine::with_limits(limits)
            }
        }
    };
    let resource = ResourceArc::new(LemmaEngineResource(Mutex::new(engine)));
    Ok((rustler::Atom::from_str(env, "ok")?, resource).encode(env))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn lemma_load<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
    sources_term: Term<'a>,
) -> NifResult<Term<'a>> {
    let batch = match sources_from_load_term(sources_term) {
        Ok(b) => b,
        Err(message) => {
            let err = lemma::Error::request(message, None::<String>);
            let list = error_encoding::encode_errors(env, &[err])?;
            return Ok((rustler::Atom::from_str(env, "error")?, list).encode(env));
        }
    };
    let mut engine = resource.0.lock().map_err(|_| poisoned_engine_lock())?;
    match engine.load(batch) {
        Ok(()) => Ok(rustler::Atom::from_str(env, "ok")?.encode(env)),
        Err(load_err) => {
            let list = error_encoding::encode_errors(env, &load_err.errors)?;
            Ok((rustler::Atom::from_str(env, "error")?, list).encode(env))
        }
    }
}

fn sources_from_load_term(term: Term) -> Result<Vec<(SourceType, String)>, String> {
    if let Ok(binary) = term.decode::<Binary>() {
        let code = std::str::from_utf8(&binary)
            .map_err(|_| "load: source text must be valid UTF-8".to_string())?;
        return Ok(vec![(SourceType::Volatile, code.to_string())]);
    }
    if let Some(iter) = MapIterator::new(term) {
        return sources_from_label_map(iter);
    }
    if let Ok(list) = term.decode::<Vec<(String, String)>>() {
        return sources_from_label_pairs(list);
    }
    Err("load: sources must be a binary, a map, or a list of {label, code} tuples".to_string())
}

fn sources_from_label_map(iter: MapIterator) -> Result<Vec<(SourceType, String)>, String> {
    // BEAM maps have no insertion-order contract. Lexicographic label order is the
    // documented map-load contract; list-of-tuples preserves caller order instead.
    let mut pairs = Vec::new();
    for (key, value) in iter {
        let label: String = key
            .decode()
            .map_err(|_| "load: map keys must be strings".to_string())?;
        let code: String = value
            .decode()
            .map_err(|_| "load: map values must be strings (Lemma source text)".to_string())?;
        pairs.push((label, code));
    }
    pairs.sort_by(|(a, _), (b, _)| a.cmp(b));
    pairs
        .into_iter()
        .map(|(label, code)| {
            SourceType::from_binding_label(&label)
                .map(|source_type| (source_type, code))
                .map_err(|e| format!("load: label '{label}': {e}"))
        })
        .collect()
}

fn sources_from_label_pairs(
    pairs: Vec<(String, String)>,
) -> Result<Vec<(SourceType, String)>, String> {
    pairs
        .into_iter()
        .enumerate()
        .map(|(i, (label, code))| {
            SourceType::from_binding_label(&label)
                .map(|source_type| (source_type, code))
                .map_err(|e| format!("load: entry {i}: {e}"))
        })
        .collect()
}

#[rustler::nif]
fn lemma_specs<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
) -> NifResult<Term<'a>> {
    let engine = resource.0.lock().map_err(|_| poisoned_engine_lock())?;

    let repos = engine.specs();
    let json = serde_json::to_vec(&repos).map_err(|e| {
        rustler::Error::RaiseTerm(Box::new(format!("Specs serialization failed: {e}")))
    })?;
    let mut owned = OwnedBinary::new(json.len()).ok_or_else(|| {
        rustler::Error::RaiseTerm(Box::new("Binary allocation failed".to_string()))
    })?;
    owned.as_mut_slice().copy_from_slice(&json);
    let binary = Binary::from_owned(owned, env);
    Ok((rustler::Atom::from_str(env, "ok")?, binary).encode(env))
}

#[rustler::nif]
fn lemma_show<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
    repository: Option<String>,
    spec: String,
    effective_opt: Option<String>,
) -> NifResult<Term<'a>> {
    let engine = resource.0.lock().map_err(|_| poisoned_engine_lock())?;
    let effective = match effective_opt {
        Some(s) => Some(s.parse::<DateTimeValue>().map_err(|e| {
            rustler::Error::RaiseTerm(Box::new(format!("Invalid effective date: {}", e)))
        })?),
        None => None,
    };
    let repo = repository
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match engine.show(repo, &spec, effective.as_ref()) {
        Ok(view) => {
            let json =
                serde_json::to_vec(&lemma::api::show_document(&engine, &view)).map_err(|e| {
                    rustler::Error::RaiseTerm(Box::new(format!("Show serialization failed: {}", e)))
                })?;
            let mut owned = OwnedBinary::new(json.len()).ok_or_else(|| {
                rustler::Error::RaiseTerm(Box::new("Binary allocation failed".to_string()))
            })?;
            owned.as_mut_slice().copy_from_slice(&json);
            let binary = rustler::Binary::from_owned(owned, env);
            Ok((rustler::Atom::from_str(env, "ok")?, binary).encode(env))
        }
        Err(err) => {
            let term = encode_error(env, &err)?;
            Ok((rustler::Atom::from_str(env, "error")?, term).encode(env))
        }
    }
}

#[rustler::nif]
fn lemma_source<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
    repository: Option<String>,
    spec: Option<String>,
    effective_opt: Option<String>,
) -> NifResult<Term<'a>> {
    let engine = resource.0.lock().map_err(|_| poisoned_engine_lock())?;
    let repo = repository
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let spec_name = spec.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let effective = match (spec_name, effective_opt) {
        (Some(_), Some(s)) => Some(s.parse::<DateTimeValue>().map_err(|e| {
            rustler::Error::RaiseTerm(Box::new(format!("Invalid effective date: {}", e)))
        })?),
        (Some(_), None) => None,
        _ => None,
    };
    match engine.source(repo, spec_name, effective.as_ref()) {
        Ok(text) => Ok((rustler::Atom::from_str(env, "ok")?, text).encode(env)),
        Err(err) => {
            let term = encode_error(env, &err)?;
            Ok((rustler::Atom::from_str(env, "error")?, term).encode(env))
        }
    }
}

#[rustler::nif(schedule = "DirtyCpu")]
fn lemma_run<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
    target: Term<'a>,
    options: Term<'a>,
) -> NifResult<Term<'a>> {
    let (repository, spec, effective_opt) = decode_evaluate_target(target)?;
    let EvaluateOptions {
        data: data_term,
        rules,
        explain,
    } = decode_evaluate_options(options)?;
    let data_values = match data_term.map(map_term_to_data_values).transpose() {
        Ok(values) => values.unwrap_or_default(),
        Err(err) => {
            let term = encode_error(env, &err)?;
            return Ok((rustler::Atom::from_str(env, "error")?, term).encode(env));
        }
    };
    let engine = resource.0.lock().map_err(|_| poisoned_engine_lock())?;
    let effective = match effective_opt {
        Some(s) => Some(&s.parse::<DateTimeValue>().map_err(|e| {
            rustler::Error::RaiseTerm(Box::new(format!("Invalid effective date: {}", e)))
        })?),
        None => None,
    };
    let repo = repository
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let rules = match rules {
        None => None,
        Some(names) if names.is_empty() => {
            return Err(rustler::Error::RaiseTerm(Box::new(
                "rules must not be empty".to_string(),
            )));
        }
        Some(names) => Some(names),
    };
    match engine.evaluate(
        repo,
        &spec,
        effective,
        data_values,
        rules.as_deref(),
        explain,
    ) {
        Ok(response) => {
            let json = serde_json::to_vec(&lemma::api::Response::from(&response)).map_err(|e| {
                rustler::Error::RaiseTerm(Box::new(format!("Response serialization failed: {}", e)))
            })?;
            let mut owned = OwnedBinary::new(json.len()).ok_or_else(|| {
                rustler::Error::RaiseTerm(Box::new("Binary allocation failed".to_string()))
            })?;
            owned.as_mut_slice().copy_from_slice(&json);
            let binary = rustler::Binary::from_owned(owned, env);
            Ok((rustler::Atom::from_str(env, "ok")?, binary).encode(env))
        }
        Err(err) => {
            let term = encode_error(env, &err)?;
            Ok((rustler::Atom::from_str(env, "error")?, term).encode(env))
        }
    }
}

#[rustler::nif]
fn lemma_remove<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
    repository: Option<String>,
    spec_name: String,
    effective: Option<String>,
) -> NifResult<Term<'a>> {
    let mut engine = resource.0.lock().map_err(|_| poisoned_engine_lock())?;
    let effective_dt = match effective {
        None => None,
        Some(s) => Some(s.parse::<DateTimeValue>().map_err(|e| {
            rustler::Error::RaiseTerm(Box::new(format!("Invalid effective date: {}", e)))
        })?),
    };
    let repo = repository
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match engine.remove(repo, &spec_name, effective_dt.as_ref()) {
        Ok(()) => Ok(rustler::Atom::from_str(env, "ok")?.encode(env)),
        Err(err) => {
            let term = encode_error(env, &err)?;
            Ok((rustler::Atom::from_str(env, "error")?, term).encode(env))
        }
    }
}

#[rustler::nif(schedule = "DirtyCpu")]
fn lemma_update<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
    repository: Option<String>,
    code: String,
    attribute: Option<String>,
) -> NifResult<Term<'a>> {
    let mut engine = resource.0.lock().map_err(|_| poisoned_engine_lock())?;
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
        Some(label) => SourceType::from_binding_label(label).map_err(|e| {
            rustler::Error::RaiseTerm(Box::new(format!("update: label '{label}': {e}")))
        })?,
    };
    match engine.update(repo, code, source_type) {
        Ok(()) => Ok(rustler::Atom::from_str(env, "ok")?.encode(env)),
        Err(load_err) => {
            let list = error_encoding::encode_errors(env, &load_err.errors)?;
            Ok((rustler::Atom::from_str(env, "error")?, list).encode(env))
        }
    }
}

#[rustler::nif]
fn lemma_limits<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
) -> NifResult<Term<'a>> {
    let engine = resource.0.lock().map_err(|_| poisoned_engine_lock())?;
    let json = serde_json::to_vec(engine.limits()).map_err(|e| {
        rustler::Error::RaiseTerm(Box::new(format!("BUG: limits serialization failed: {e}")))
    })?;
    let mut owned = OwnedBinary::new(json.len())
        .ok_or_else(|| rustler::Error::RaiseTerm(Box::new("out of memory".to_string())))?;
    owned.as_mut_slice().copy_from_slice(&json);
    let binary = rustler::Binary::from_owned(owned, env);
    Ok((rustler::Atom::from_str(env, "ok")?, binary).encode(env))
}

#[rustler::nif]
fn lemma_snapshot<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
) -> NifResult<Term<'a>> {
    let engine = resource.0.lock().map_err(|_| poisoned_engine_lock())?;
    match engine.snapshot() {
        Ok(bytes) => {
            let mut owned = OwnedBinary::new(bytes.len())
                .ok_or_else(|| rustler::Error::RaiseTerm(Box::new("out of memory".to_string())))?;
            owned.as_mut_slice().copy_from_slice(&bytes);
            let binary = rustler::Binary::from_owned(owned, env);
            Ok((rustler::Atom::from_str(env, "ok")?, binary).encode(env))
        }
        Err(err) => {
            let term = encode_error(env, &err)?;
            Ok((rustler::Atom::from_str(env, "error")?, term).encode(env))
        }
    }
}

#[rustler::nif]
fn lemma_from_snapshot<'a>(env: Env<'a>, bytes: Binary) -> NifResult<Term<'a>> {
    match Engine::from_snapshot(bytes.as_slice()) {
        Ok(engine) => {
            let resource = ResourceArc::new(LemmaEngineResource(Mutex::new(engine)));
            Ok((rustler::Atom::from_str(env, "ok")?, resource).encode(env))
        }
        Err(err) => {
            let term = encode_error(env, &err)?;
            Ok((rustler::Atom::from_str(env, "error")?, term).encode(env))
        }
    }
}

#[rustler::nif]
fn lemma_quality<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
) -> NifResult<Term<'a>> {
    let engine = resource.0.lock().map_err(|_| poisoned_engine_lock())?;
    let json = serde_json::to_vec(&engine.quality()).map_err(|e| {
        rustler::Error::RaiseTerm(Box::new(format!("BUG: quality serialization failed: {e}")))
    })?;
    let mut owned = OwnedBinary::new(json.len())
        .ok_or_else(|| rustler::Error::RaiseTerm(Box::new("out of memory".to_string())))?;
    owned.as_mut_slice().copy_from_slice(&json);
    let binary = rustler::Binary::from_owned(owned, env);
    Ok((rustler::Atom::from_str(env, "ok")?, binary).encode(env))
}

#[rustler::nif]
fn lemma_format<'a>(env: Env<'a>, code: String) -> NifResult<Term<'a>> {
    match lemma::format_source(&code, SourceType::Volatile) {
        Ok(formatted) => Ok((rustler::Atom::from_str(env, "ok")?, formatted).encode(env)),
        Err(err) => {
            let term = encode_error(env, &err)?;
            Ok((rustler::Atom::from_str(env, "error")?, term).encode(env))
        }
    }
}

#[rustler::nif]
fn lemma_generate_openapi<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
    explain: bool,
    effective_opt: Option<String>,
) -> NifResult<Term<'a>> {
    let engine = resource.0.lock().map_err(|_| poisoned_engine_lock())?;

    let effective = match effective_opt {
        None => DateTimeValue::now(),
        Some(s) => s.parse::<DateTimeValue>().map_err(|e| {
            rustler::Error::RaiseTerm(Box::new(format!("Invalid effective date: {}", e)))
        })?,
    };

    let spec = lemma_openapi::generate_openapi_effective(&engine, explain, &effective);
    let json = serde_json::to_vec(&spec).map_err(|e| {
        rustler::Error::RaiseTerm(Box::new(format!(
            "OpenAPI JSON serialization failed: {}",
            e
        )))
    })?;
    let mut owned = OwnedBinary::new(json.len()).ok_or_else(|| {
        rustler::Error::RaiseTerm(Box::new("Binary allocation failed".to_string()))
    })?;
    owned.as_mut_slice().copy_from_slice(&json);
    let binary = rustler::Binary::from_owned(owned, env);
    Ok((rustler::Atom::from_str(env, "ok")?, binary).encode(env))
}

/// Temporal version choices (title + slug) for API docs, aligned with `lemma_openapi::temporal_api_sources`.
#[rustler::nif]
fn lemma_temporal_api_sources<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
) -> NifResult<Term<'a>> {
    let engine = resource.0.lock().map_err(|_| poisoned_engine_lock())?;

    let sources = lemma_openapi::temporal_api_sources(&engine);
    let rows: Vec<_> = sources
        .into_iter()
        .map(|s| {
            json!({
                "title": s.title,
                "slug": s.slug,
            })
        })
        .collect();

    let json = serde_json::to_vec(&rows).map_err(|e| {
        rustler::Error::RaiseTerm(Box::new(format!("temporal API sources JSON failed: {}", e)))
    })?;
    let mut owned = OwnedBinary::new(json.len()).ok_or_else(|| {
        rustler::Error::RaiseTerm(Box::new("Binary allocation failed".to_string()))
    })?;
    owned.as_mut_slice().copy_from_slice(&json);
    let binary = rustler::Binary::from_owned(owned, env);
    Ok((rustler::Atom::from_str(env, "ok")?, binary).encode(env))
}

fn limits_from_term(term: Term) -> Result<ResourceLimits, String> {
    let iter = MapIterator::new(term).ok_or_else(|| "limits must be a map".to_string())?;
    let mut limits = ResourceLimits::default();
    for (key, value) in iter {
        let key_str: String = key
            .decode()
            .map_err(|_| "limits map keys must be strings".to_string())?;
        let value_int: i64 = value
            .decode()
            .map_err(|_| format!("limits value for '{}' must be an integer", key_str))?;
        if value_int < 0 {
            return Err(format!(
                "limits value for '{}' must be non-negative",
                key_str
            ));
        }
        let value_usize = value_int as usize;
        limits.apply(&key_str, value_usize)?;
    }
    Ok(limits)
}

fn map_key_string(term: Term) -> Result<String, rustler::Error> {
    if let Ok(s) = term.atom_to_string() {
        return Ok(s);
    }
    term.decode::<String>().map_err(|_| rustler::Error::BadArg)
}

fn data_map_key(term: Term) -> Result<String, lemma::Error> {
    if let Ok(s) = term.atom_to_string() {
        return Ok(s);
    }
    term.decode::<String>().map_err(|_| {
        lemma::Error::request("data map keys must be strings or atoms", None::<String>)
    })
}

/// Top-level `data` map: each value's error is attributed to its key.
fn data_map_entries(term: Term) -> Result<Vec<(String, EvaluateDataValue)>, lemma::Error> {
    let iter = MapIterator::new(term).expect("BUG: caller checked term is a map");
    let mut entries = Vec::new();
    for (key, value) in iter {
        let key = data_map_key(key)?;
        let parsed = term_to_evaluate_data(value)
            .map_err(|e| lemma::attribute_evaluate_data_error(e, &key))?;
        if let Some(parsed) = parsed {
            entries.push((key, parsed));
        }
    }
    Ok(entries)
}

fn map_term_to_data_values(
    term: Term,
) -> Result<IndexMap<String, EvaluateDataValue>, lemma::Error> {
    if term.as_c_arg() == atom::nil().as_c_arg() {
        return Ok(IndexMap::new());
    }
    if term.get_type() != TermType::Map {
        return Err(lemma::Error::request("data must be a map", None::<String>));
    }
    lemma::evaluate_data_from_entries(data_map_entries(term)?)
}

fn scalar_evaluate_data(term: Term) -> Result<Option<EvaluateDataValue>, lemma::Error> {
    match term.get_type() {
        TermType::Atom => {
            if term.as_c_arg() == atom::nil().as_c_arg() {
                return Ok(None);
            }
            match term.decode::<bool>() {
                Ok(b) => Ok(Some(EvaluateDataValue::Boolean(b))),
                Err(_) => Err(lemma::Error::request(
                    format!(
                        "atom :{} is not a data value",
                        term.atom_to_string()
                            .expect("BUG: atom term must convert to string")
                    ),
                    Some("pass a binary string"),
                )),
            }
        }
        TermType::Binary => term
            .decode::<String>()
            .map(|s| Some(EvaluateDataValue::Literal(s)))
            .map_err(|_| lemma::Error::request("data binary must be UTF-8", None::<String>)),
        TermType::Integer => {
            let value: rustler::BigInt = term
                .decode()
                .expect("BUG: integer term must decode to BigInt");
            Ok(Some(EvaluateDataValue::Literal(value.to_string())))
        }
        TermType::Float => {
            let value: f64 = term.decode().expect("BUG: float term must decode to f64");
            Err(lemma::Error::request(
                format!("pass as string: \"{value}\""),
                Some("Elixir floats have no exact native type; use a binary or integer"),
            ))
        }
        TermType::Map | TermType::List => {
            panic!("BUG: map and list terms are walked by term_to_evaluate_data")
        }
        _ => Err(lemma::Error::request(
            "data value must be a binary, boolean, integer, map, or list",
            None::<String>,
        )),
    }
}

/// Non-null Hex term to run data. `nil` means omit; a `nil` list item is a request error.
/// Frames replace recursion, so nesting depth never grows the native stack.
fn term_to_evaluate_data(term: Term) -> Result<Option<EvaluateDataValue>, lemma::Error> {
    let mut frames: Vec<HexFrame<'_>> = Vec::new();
    let mut step = HexStep::Descend(term);
    loop {
        step = match step {
            HexStep::Descend(term) => match term.get_type() {
                TermType::Map => {
                    frames.push(HexFrame::Map {
                        entries: Box::new(
                            MapIterator::new(term).expect("BUG: caller checked term is a map"),
                        ),
                        pending_key: None,
                        out: Vec::new(),
                    });
                    HexStep::NextChild
                }
                TermType::List => {
                    let items: Vec<Term> = term.decode().map_err(|_| {
                        lemma::Error::request("data list must be proper", None::<String>)
                    })?;
                    frames.push(HexFrame::List {
                        items: items.into_iter(),
                        delivered: 0,
                        out: Vec::new(),
                    });
                    HexStep::NextChild
                }
                _ => HexStep::Deliver(scalar_evaluate_data(term)?),
            },
            HexStep::NextChild => {
                let frame = frames
                    .last_mut()
                    .expect("BUG: hex frame stack empty while reading a child");
                match frame.next_child()? {
                    Some(child) => HexStep::Descend(child),
                    None => {
                        let frame = frames
                            .pop()
                            .expect("BUG: hex frame stack empty on completion");
                        HexStep::Deliver(frame.finish()?)
                    }
                }
            }
            HexStep::Deliver(item) => match frames.last_mut() {
                None => return Ok(item),
                Some(frame) => {
                    frame.push(item)?;
                    HexStep::NextChild
                }
            },
        };
    }
}

enum HexStep<'a> {
    Descend(Term<'a>),
    NextChild,
    Deliver(Option<EvaluateDataValue>),
}

enum HexFrame<'a> {
    Map {
        entries: Box<MapIterator<'a>>,
        pending_key: Option<String>,
        out: Vec<(String, EvaluateDataValue)>,
    },
    List {
        items: std::vec::IntoIter<Term<'a>>,
        delivered: usize,
        out: Vec<EvaluateDataValue>,
    },
}

impl<'a> HexFrame<'a> {
    fn next_child(&mut self) -> Result<Option<Term<'a>>, lemma::Error> {
        match self {
            HexFrame::Map {
                entries,
                pending_key,
                ..
            } => match entries.next() {
                None => Ok(None),
                Some((key, value)) => {
                    *pending_key = Some(data_map_key(key)?);
                    Ok(Some(value))
                }
            },
            HexFrame::List {
                items, delivered, ..
            } => {
                let child = items.next();
                if child.is_some() {
                    *delivered += 1;
                }
                Ok(child)
            }
        }
    }

    fn push(&mut self, item: Option<EvaluateDataValue>) -> Result<(), lemma::Error> {
        match self {
            HexFrame::Map {
                pending_key, out, ..
            } => {
                let key = pending_key
                    .take()
                    .expect("BUG: hex map child delivered without its key");
                if let Some(value) = item {
                    out.push((key, value));
                }
                Ok(())
            }
            HexFrame::List { delivered, out, .. } => match item {
                Some(value) => {
                    out.push(value);
                    Ok(())
                }
                None => Err(lemma::evaluate_data_null_list_item(*delivered)),
            },
        }
    }

    fn finish(self) -> Result<Option<EvaluateDataValue>, lemma::Error> {
        match self {
            HexFrame::Map { out, .. } => lemma::evaluate_data_map_from_entries(out).map(Some),
            HexFrame::List { out, .. } => Ok(Some(EvaluateDataValue::List(out))),
        }
    }
}

fn optional_string(term: Term) -> Result<Option<String>, rustler::Error> {
    if term.as_c_arg() == atom::nil().as_c_arg() {
        return Ok(None);
    }
    Ok(Some(
        term.decode::<String>()
            .map_err(|_| rustler::Error::BadArg)?,
    ))
}

fn decode_evaluate_target(
    term: Term,
) -> Result<(Option<String>, String, Option<String>), rustler::Error> {
    let iter = MapIterator::new(term).ok_or(rustler::Error::BadArg)?;
    let mut repository = None;
    let mut spec = None;
    let mut effective = None;
    for (key, value) in iter {
        let key_str = map_key_string(key)?;
        match key_str.as_str() {
            "repo" | "repository" => repository = optional_string(value)?,
            "spec" => {
                spec = Some(
                    value
                        .decode::<String>()
                        .map_err(|_| rustler::Error::BadArg)?,
                )
            }
            "effective" => effective = optional_string(value)?,
            other => {
                return Err(rustler::Error::RaiseTerm(Box::new(format!(
                    "unknown evaluate target key: '{other}'"
                ))));
            }
        }
    }
    let spec = spec.ok_or(rustler::Error::RaiseTerm(Box::new(
        "evaluate target must include :spec".to_string(),
    )))?;
    Ok((repository, spec, effective))
}

struct EvaluateOptions<'a> {
    data: Option<Term<'a>>,
    rules: Option<Vec<String>>,
    explain: bool,
}

fn decode_evaluate_options(term: Term) -> Result<EvaluateOptions, rustler::Error> {
    if term.as_c_arg() == atom::nil().as_c_arg() {
        return Ok(EvaluateOptions {
            data: None,
            rules: None,
            explain: false,
        });
    }
    let iter = MapIterator::new(term).ok_or(rustler::Error::BadArg)?;
    let mut data = None;
    let mut rules = None;
    let mut explain = false;
    for (key, value) in iter {
        let key_str = map_key_string(key)?;
        match key_str.as_str() {
            "data" => data = Some(value),
            "rules" => {
                if value.as_c_arg() == atom::nil().as_c_arg() {
                    rules = None;
                } else {
                    rules = Some(
                        value
                            .decode::<Vec<String>>()
                            .map_err(|_| rustler::Error::BadArg)?,
                    );
                }
            }
            "explain" => explain = value.decode::<bool>().map_err(|_| rustler::Error::BadArg)?,
            other => {
                return Err(rustler::Error::RaiseTerm(Box::new(format!(
                    "unknown evaluate options key: '{other}'"
                ))));
            }
        }
    }
    Ok(EvaluateOptions {
        data,
        rules,
        explain,
    })
}

fn mcp_args(json: &str) -> Result<serde_json::Value, rustler::Error> {
    serde_json::from_str(json).map_err(|error| {
        rustler::Error::RaiseTerm(Box::new(format!("mcp args must be JSON: {error}")))
    })
}

fn mcp_tool_result<'a>(
    env: Env<'a>,
    result: Result<String, lemma::mcp::ToolError>,
) -> NifResult<Term<'a>> {
    match result {
        Ok(text) => Ok((atom::ok(), text).encode(env)),
        Err(lemma::mcp::ToolError::InvalidArguments(message)) => Ok((
            atom::error(),
            rustler::Atom::from_str(env, "invalid_arguments")?,
            message,
        )
            .encode(env)),
        Err(lemma::mcp::ToolError::NotFound(message)) => Ok((
            atom::error(),
            rustler::Atom::from_str(env, "not_found")?,
            message,
        )
            .encode(env)),
        Err(lemma::mcp::ToolError::Diagnostics(message)) => Ok((
            atom::error(),
            rustler::Atom::from_str(env, "diagnostics")?,
            message,
        )
            .encode(env)),
    }
}

#[rustler::nif]
fn mcp_list_tools<'a>(env: Env<'a>) -> NifResult<Term<'a>> {
    let json = serde_json::to_string(&lemma::mcp::list_tools())
        .unwrap_or_else(|error| panic!("BUG: MCP tool catalog must serialize: {error}"));
    Ok((atom::ok(), json).encode(env))
}

#[rustler::nif]
fn mcp_list_resources<'a>(env: Env<'a>) -> NifResult<Term<'a>> {
    let json = serde_json::to_string(&lemma::mcp::list_resources())
        .unwrap_or_else(|error| panic!("BUG: MCP resource catalog must serialize: {error}"));
    Ok((atom::ok(), json).encode(env))
}

#[rustler::nif]
fn mcp_read_resource<'a>(env: Env<'a>, uri: String) -> NifResult<Term<'a>> {
    match lemma::mcp::read_resource(&uri) {
        Ok(text) => Ok((atom::ok(), text).encode(env)),
        Err(lemma::mcp::ResourceError::UnknownUri(message)) => Ok((
            atom::error(),
            rustler::Atom::from_str(env, "unknown_uri")?,
            message,
        )
            .encode(env)),
    }
}

fn mcp_run_impl<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
    args_json: String,
) -> NifResult<Term<'a>> {
    let args = mcp_args(&args_json)?;
    let engine = resource.0.lock().map_err(|_| poisoned_engine_lock())?;
    mcp_tool_result(env, lemma::mcp::run(&engine, &args))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn mcp_run<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
    args_json: String,
) -> NifResult<Term<'a>> {
    mcp_run_impl(env, resource, args_json)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn mcp_evaluate<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
    args_json: String,
) -> NifResult<Term<'a>> {
    mcp_run_impl(env, resource, args_json)
}

#[rustler::nif(schedule = "DirtyCpu")]
fn mcp_specs<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
    args_json: String,
) -> NifResult<Term<'a>> {
    let args = mcp_args(&args_json)?;
    let engine = resource.0.lock().map_err(|_| poisoned_engine_lock())?;
    mcp_tool_result(env, lemma::mcp::specs(&engine, &args))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn mcp_show<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
    args_json: String,
) -> NifResult<Term<'a>> {
    let args = mcp_args(&args_json)?;
    let engine = resource.0.lock().map_err(|_| poisoned_engine_lock())?;
    mcp_tool_result(env, lemma::mcp::show(&engine, &args))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn mcp_source<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
    args_json: String,
) -> NifResult<Term<'a>> {
    let args = mcp_args(&args_json)?;
    let engine = resource.0.lock().map_err(|_| poisoned_engine_lock())?;
    mcp_tool_result(env, lemma::mcp::source(&engine, &args))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn mcp_check<'a>(env: Env<'a>, args_json: String) -> NifResult<Term<'a>> {
    let args = mcp_args(&args_json)?;
    mcp_tool_result(env, lemma::mcp::check(&args))
}

#[rustler::nif]
fn mcp_guide<'a>(env: Env<'a>, args_json: String) -> NifResult<Term<'a>> {
    let args = mcp_args(&args_json)?;
    mcp_tool_result(env, lemma::mcp::guide(&args))
}

#[rustler::nif]
fn lemma_install_start<'a>(
    env: Env<'a>,
    resource: ResourceArc<LemmaEngineResource>,
    repository: String,
) -> NifResult<Term<'a>> {
    let limits = {
        let engine = resource.0.lock().map_err(|_| poisoned_engine_lock())?;
        engine.limits().clone()
    };
    let (install, step) = lemma::Install::start(registries(), &repository, limits);
    encode_install_step(env, step, InstallCarrier::New(install))
}

#[rustler::nif(schedule = "DirtyCpu")]
fn lemma_install_respond<'a>(
    env: Env<'a>,
    resource: ResourceArc<InstallResource>,
    response: Term<'a>,
) -> NifResult<Term<'a>> {
    let transport = decode_transport_result(response)
        .map_err(|msg| rustler::Error::RaiseTerm(Box::new(msg)))?;
    let step = {
        let mut install = resource.0.lock().map_err(|_| {
            rustler::Error::RaiseTerm(Box::new("Install lock poisoned".to_string()))
        })?;
        install.respond(transport)
    };
    encode_install_step(env, step, InstallCarrier::Existing(resource))
}

enum InstallCarrier {
    New(lemma::Install<'static>),
    Existing(ResourceArc<InstallResource>),
}

fn encode_install_step<'a>(
    env: Env<'a>,
    step: lemma::InstallStep,
    carrier: InstallCarrier,
) -> NifResult<Term<'a>> {
    match step {
        lemma::InstallStep::Fetch(fetch) => {
            let resource = match carrier {
                InstallCarrier::New(install) => {
                    ResourceArc::new(InstallResource(Mutex::new(install)))
                }
                InstallCarrier::Existing(resource) => resource,
            };
            let headers: Vec<(String, String)> = fetch
                .headers
                .into_iter()
                .map(|h| (h.name, h.value))
                .collect();
            let fetch_atom = rustler::Atom::from_str(env, "fetch")?;
            Ok((fetch_atom, fetch.url, headers, resource).encode(env))
        }
        lemma::InstallStep::Finished(Ok(result)) => {
            let json = serde_json::to_string(&result).map_err(|e| {
                rustler::Error::RaiseTerm(Box::new(format!(
                    "BUG: RepositoryInstallResult serialize failed: {e}"
                )))
            })?;
            let finished = rustler::Atom::from_str(env, "finished")?;
            Ok((finished, (atom::ok(), json)).encode(env))
        }
        lemma::InstallStep::Finished(Err(error)) => {
            let list = error_encoding::encode_errors(env, std::slice::from_ref(&error))?;
            let finished = rustler::Atom::from_str(env, "finished")?;
            Ok((finished, (atom::error(), list)).encode(env))
        }
    }
}

fn decode_transport_result(
    term: Term,
) -> Result<Result<lemma::HttpResponse, lemma::TransportFailure>, String> {
    if let Ok((tag, status, headers, body)) =
        term.decode::<(rustler::Atom, u16, Vec<(String, String)>, rustler::Binary)>()
    {
        if tag == atom::ok() {
            let body = match std::str::from_utf8(body.as_slice()) {
                Ok(s) => s.to_owned(),
                Err(_) => {
                    return Ok(Err(lemma::TransportFailure {
                        message: "response body is not valid UTF-8".to_string(),
                    }));
                }
            };
            let headers = headers
                .into_iter()
                .map(|(name, value)| lemma::Header { name, value })
                .collect();
            return Ok(Ok(lemma::HttpResponse {
                status,
                headers,
                body,
            }));
        }
    }
    if let Ok((tag, status, headers, body)) =
        term.decode::<(rustler::Atom, u16, Vec<(String, String)>, String)>()
    {
        if tag == atom::ok() {
            let headers = headers
                .into_iter()
                .map(|(name, value)| lemma::Header { name, value })
                .collect();
            return Ok(Ok(lemma::HttpResponse {
                status,
                headers,
                body,
            }));
        }
    }
    if let Ok((tag, message)) = term.decode::<(rustler::Atom, String)>() {
        if tag == atom::error() {
            return Ok(Err(lemma::TransportFailure { message }));
        }
    }
    Err("install respond: expected {:ok, status, headers, body} or {:error, message}".to_string())
}

rustler::init!("Elixir.Lemma.Native", load = load);

#[cfg(test)]
mod tests {
    use lemma::{Install, Registries};

    #[test]
    fn install_rejects_empty_id() {
        let transport = lemma::__test_support::FixtureTransport::bundled();
        let registries = Registries::default();
        let err = Install::run(
            &registries,
            "   ",
            &transport,
            lemma::ResourceLimits::default(),
        )
        .expect_err("empty id");
        assert_eq!(err.kind(), lemma::ErrorKind::Registry);
    }

    #[test]
    fn install_fixture_countries() {
        let transport = lemma::__test_support::FixtureTransport::bundled();
        let registries = Registries::default();
        let result = Install::run(
            &registries,
            "@iso/countries",
            &transport,
            lemma::ResourceLimits::default(),
        )
        .expect("install");
        assert_eq!(result.id, "@iso/countries");
        assert!(result.source.contains("spec alpha2"));
    }
}
