use lemma::Error;
use lemma::ErrorKind;
use lemma::RegistryErrorKind;
use lemma::RequestErrorKind;
use lemma::Source;
use rustler::{Encoder, Env, NifResult, Term};

fn error_kind_string(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::Parsing => "parsing",
        ErrorKind::Validation => "validation",
        ErrorKind::Registry => "registry",
        ErrorKind::MissingRepository => "missing_repository",
        ErrorKind::Request => "request",
        ErrorKind::ResourceLimit => "resource_limit",
    }
}

fn registry_error_kind_string(kind: RegistryErrorKind) -> &'static str {
    match kind {
        RegistryErrorKind::NotFound => "not_found",
        RegistryErrorKind::Unauthorized => "unauthorized",
        RegistryErrorKind::NetworkError => "network_error",
        RegistryErrorKind::ServerError => "server_error",
        RegistryErrorKind::Other => "other",
    }
}

fn request_error_kind_string(kind: RequestErrorKind) -> &'static str {
    match kind {
        RequestErrorKind::SpecNotFound => "spec_not_found",
        RequestErrorKind::RuleNotFound => "rule_not_found",
        RequestErrorKind::InvalidRequest => "invalid_request",
    }
}

/// `EngineErrorSource` projection: same fields as the WASM/JS/Java/TS `source` shape
/// (`attribute`, `line`, `column`, `length`), keyed by atoms for Elixir map access.
fn encode_source<'a>(env: Env<'a>, source: &Source) -> NifResult<Term<'a>> {
    let mut map = rustler::types::map::map_new(env);
    map = map.map_put(
        rustler::Atom::from_str(env, "attribute")?.encode(env),
        source.source_type.to_string().encode(env),
    )?;
    map = map.map_put(
        rustler::Atom::from_str(env, "line")?.encode(env),
        source.span.line.encode(env),
    )?;
    map = map.map_put(
        rustler::Atom::from_str(env, "column")?.encode(env),
        source.span.col.encode(env),
    )?;
    map = map.map_put(
        rustler::Atom::from_str(env, "length")?.encode(env),
        source
            .span
            .end
            .saturating_sub(source.span.start)
            .encode(env),
    )?;
    Ok(map)
}

/// Mirrors the canonical `EngineError` wire shape (see `engine/src/lib.rs` `JsError`):
/// every field below is always present in the returned map, `nil` when absent —
/// never conditionally omitted. `kind`/`message`/`source` are handled inline above.
pub fn encode_error<'a>(env: Env<'a>, err: &Error) -> NifResult<Term<'a>> {
    let mut map = rustler::types::map::map_new(env);
    map = map.map_put(
        rustler::Atom::from_str(env, "kind")?.encode(env),
        error_kind_string(err.kind()).encode(env),
    )?;
    map = map.map_put(
        rustler::Atom::from_str(env, "message")?.encode(env),
        err.message().encode(env),
    )?;

    let source = match err.source_location() {
        Some(source) => Some(encode_source(env, source)?),
        None => None,
    };
    map = map.map_put(
        rustler::Atom::from_str(env, "source")?.encode(env),
        source.encode(env),
    )?;

    map = map.map_put(
        rustler::Atom::from_str(env, "suggestion")?.encode(env),
        err.suggestion().encode(env),
    )?;
    map = map.map_put(
        rustler::Atom::from_str(env, "repository")?.encode(env),
        err.repository().encode(env),
    )?;
    map = map.map_put(
        rustler::Atom::from_str(env, "related_data")?.encode(env),
        err.related_data().encode(env),
    )?;
    map = map.map_put(
        rustler::Atom::from_str(env, "spec")?.encode(env),
        err.spec_context_name().encode(env),
    )?;
    map = map.map_put(
        rustler::Atom::from_str(env, "related_spec")?.encode(env),
        err.related_spec().encode(env),
    )?;
    map = map.map_put(
        rustler::Atom::from_str(env, "registry_kind")?.encode(env),
        err.registry_kind()
            .map(registry_error_kind_string)
            .encode(env),
    )?;
    map = map.map_put(
        rustler::Atom::from_str(env, "request_kind")?.encode(env),
        err.request_kind()
            .map(request_error_kind_string)
            .encode(env),
    )?;
    map = map.map_put(
        rustler::Atom::from_str(env, "limit_name")?.encode(env),
        err.limit_name().encode(env),
    )?;
    map = map.map_put(
        rustler::Atom::from_str(env, "limit_value")?.encode(env),
        err.limit_value().encode(env),
    )?;
    map = map.map_put(
        rustler::Atom::from_str(env, "actual_value")?.encode(env),
        err.actual_value().encode(env),
    )?;

    Ok(map)
}

pub fn encode_errors<'a>(env: Env<'a>, errors: &[Error]) -> NifResult<Term<'a>> {
    let mut terms: Vec<Term<'a>> = Vec::with_capacity(errors.len());
    for e in errors {
        terms.push(encode_error(env, e)?);
    }
    Ok(terms.encode(env))
}
