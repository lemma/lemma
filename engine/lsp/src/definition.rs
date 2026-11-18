//! `textDocument/definition` for Lemma references.
//!
//! A reference under the cursor opens the data, rule, or spec it names.
//! A type path does the same: `bedragen` opens the `uses` alias and `money`
//! opens that type's data declaration. Same-file data and rule names use the
//! open buffer's parse. Imported specs use [`Engine::spec_declaration`], the
//! same lookup completion uses.

use crate::completion::enclosing_spec;
use crate::diagnostics::position_to_byte_offset;
use lemma::{
    can_be_label, DataValue, DateTimeValue, Engine, Expression, ExpressionKind, InstanceListBody,
    InstanceListRow, LemmaSpec, Lexer, ParentType, ParseResult, Source, SourceType, SpecRef,
    TokenKind, UsesBindingRhs, WithRhs,
};
use tower_lsp_server::ls_types::Position;

/// Where a reference is declared, as byte offsets into the file for `source_type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionSite {
    pub source_type: SourceType,
    pub start: usize,
    pub end: usize,
}

/// Definitions for the reference at `position` in `text`.
///
/// `text_of` returns the file text a declaration's [`SourceType`] was parsed
/// from. The open buffer is the text for this file's path. Embedded `lemma`
/// is [`lemma::UNITS_LEMMA`].
pub fn definitions_at<'a, F>(
    text: &str,
    position: Position,
    parse: &'a ParseResult,
    engine: &'a Engine,
    text_of: F,
) -> Vec<DefinitionSite>
where
    F: Fn(&SourceType) -> Option<&'a str>,
{
    let offset = position_to_byte_offset(text, position);
    let cursor_line = (position.line as usize).saturating_add(1);
    let Some((repository, spec)) = enclosing_spec(parse, cursor_line) else {
        return Vec::new();
    };
    let repo_name = repository.name.clone();
    let effective = spec.effective_from().cloned();
    let tokens = lex_tokens(text);
    let mut hits = Vec::new();
    collect_hits(spec, repo_name.as_deref(), text, offset, &tokens, &mut hits);
    hits.sort_by(|left, right| {
        (left.end - left.start)
            .cmp(&(right.end - right.start))
            .then(right.start.cmp(&left.start))
    });
    let Some(hit) = hits.into_iter().next() else {
        return Vec::new();
    };
    match hit.kind {
        HitKind::Spec {
            name,
            repository,
            effective,
            qualifier,
        } => engine
            .spec_declaration(repository.as_deref(), &name, effective.as_ref())
            .ok()
            .and_then(|declared| spec_site(declared, qualifier, &text_of))
            .into_iter()
            .collect(),
        HitKind::LocalPath { names } => resolve_path(
            spec,
            repo_name.as_deref(),
            effective.as_ref(),
            &names,
            engine,
            &text_of,
        ),
        HitKind::ElementPath { element, names } => resolve_element_path(
            spec,
            repo_name.as_deref(),
            effective.as_ref(),
            &element,
            &names,
            engine,
            &text_of,
        ),
    }
}

struct Hit {
    start: usize,
    end: usize,
    kind: HitKind,
}

enum HitKind {
    Spec {
        name: String,
        repository: Option<String>,
        effective: Option<DateTimeValue>,
        qualifier: bool,
    },
    /// Path from the enclosing spec through the clicked segment.
    LocalPath { names: Vec<String> },
    /// Field path on the list element named `element` (`data items: element list`).
    ElementPath { element: String, names: Vec<String> },
}

struct Tok {
    kind: TokenKind,
    start: usize,
    end: usize,
    text: String,
}

struct Label {
    text: String,
    start: usize,
    end: usize,
}

fn collect_hits(
    spec: &LemmaSpec,
    consumer_repo: Option<&str>,
    text: &str,
    offset: usize,
    tokens: &[Tok],
    hits: &mut Vec<Hit>,
) {
    for rule in &spec.rules {
        walk_expression(&rule.expression, text, offset, hits);
        for clause in &rule.unless_clauses {
            walk_expression(&clause.condition, text, offset, hits);
            walk_expression(&clause.result, text, offset, hits);
        }
    }
    for data in &spec.data {
        match &data.value {
            DataValue::Import {
                spec_ref,
                bindings,
                effective_ref,
            } => {
                hit_spec_ref(spec, spec_ref, consumer_repo, offset, hits);
                let alias = data.reference.name.clone();
                for binding in bindings {
                    let mut groups =
                        vec![(PathRoot::Alias(alias.clone()), binding.path.full_path())];
                    if let UsesBindingRhs::References { targets } = &binding.rhs {
                        for target in targets {
                            groups.push((PathRoot::Local, target.full_path()));
                        }
                    }
                    hit_groups(
                        binding.source_location.span.start,
                        binding.source_location.span.end,
                        &groups,
                        tokens,
                        offset,
                        hits,
                    );
                }
                if let Some(pin) = effective_ref {
                    hit_after_keyword(
                        data.source_location.span.start,
                        data.source_location.span.end,
                        TokenKind::Effective,
                        &pin.full_path(),
                        tokens,
                        offset,
                        hits,
                    );
                }
            }
            DataValue::ImportList { spec_ref, list } => {
                hit_spec_ref(spec, spec_ref, consumer_repo, offset, hits);
                walk_list(
                    spec,
                    list,
                    Some(data.reference.name.as_str()),
                    tokens,
                    offset,
                    hits,
                );
            }
            DataValue::Definition { base, list, .. } => {
                if let Some(ty) = base {
                    hit_parent_type(
                        data.source_location.span.start,
                        data.source_location.span.end,
                        ty,
                        tokens,
                        offset,
                        hits,
                    );
                }
                if let Some(list) = list {
                    let element = list_element_name(base.as_ref());
                    walk_list(spec, list, element.as_deref(), tokens, offset, hits);
                }
            }
        }
    }
}

fn walk_expression(expr: &Expression, text: &str, offset: usize, hits: &mut Vec<Hit>) {
    for node in expr.table.nodes() {
        let ExpressionKind::Reference(reference) = &node.kind else {
            continue;
        };
        let Some(source) = node.source_location.as_ref() else {
            panic!("BUG: expression reference missing source span");
        };
        if source.span.end > text.len() || offset < source.span.start || offset >= source.span.end {
            continue;
        }
        let segments = segment_spans(text, source.span.start, source.span.end);
        let path = reference.full_path();
        if segments.len() != path.len() {
            panic!(
                "BUG: reference `{reference}` covers bytes {}..{} as {} segments, path has {}",
                source.span.start,
                source.span.end,
                segments.len(),
                path.len()
            );
        }
        let Some(index) = segments
            .iter()
            .position(|(start, end)| *start <= offset && offset < *end)
        else {
            continue;
        };
        let (start, end) = segments[index];
        hits.push(Hit {
            start,
            end,
            kind: HitKind::LocalPath {
                names: path[..=index].to_vec(),
            },
        });
    }
}

fn hit_spec_ref(
    spec: &LemmaSpec,
    spec_ref: &SpecRef,
    consumer_repo: Option<&str>,
    offset: usize,
    hits: &mut Vec<Hit>,
) {
    let repository = spec_ref
        .repository
        .as_ref()
        .map(|qualifier| qualifier.name.clone())
        .or_else(|| consumer_repo.map(str::to_string));
    let effective = spec_ref
        .effective
        .clone()
        .or_else(|| spec.effective_from().cloned());
    if let Some(span) = spec_ref.repository_span.as_ref() {
        if span.start <= offset && offset < span.end {
            hits.push(Hit {
                start: span.start,
                end: span.end,
                kind: HitKind::Spec {
                    name: spec_ref.name.clone(),
                    repository: repository.clone(),
                    effective: effective.clone(),
                    qualifier: true,
                },
            });
        }
    }
    if let Some(span) = spec_ref.target_span.as_ref() {
        if span.start <= offset && offset < span.end {
            hits.push(Hit {
                start: span.start,
                end: span.end,
                kind: HitKind::Spec {
                    name: spec_ref.name.clone(),
                    repository,
                    effective,
                    qualifier: false,
                },
            });
        }
    }
}

#[derive(Clone)]
enum PathRoot {
    Local,
    /// Prepend this `uses` alias, then resolve from the enclosing spec.
    Alias(String),
    /// `names` are fields of the spec called `element` (not a `uses` alias).
    Element(String),
}

fn walk_list(
    spec: &LemmaSpec,
    list: &InstanceListBody,
    element: Option<&str>,
    tokens: &[Tok],
    offset: usize,
    hits: &mut Vec<Hit>,
) {
    let root = element.map(|name| field_root(spec, name));
    if let (Some(root), Some(key)) = (root.as_ref(), list.key.as_ref()) {
        let groups: Vec<(PathRoot, Vec<String>)> = key
            .fields
            .iter()
            .map(|field| (root.clone(), field.full_path()))
            .collect();
        hit_groups(
            key.source_location.span.start,
            key.source_location.span.end,
            &groups,
            tokens,
            offset,
            hits,
        );
    }
    for row in list.rows.iter().chain(list.option_rows.iter()) {
        match row {
            InstanceListRow::Fields { bindings, .. } => {
                for binding in bindings {
                    let mut groups = Vec::new();
                    if let Some(root) = root.as_ref() {
                        groups.push((root.clone(), binding.path.full_path()));
                    }
                    if let WithRhs::Reference { target } = &binding.rhs {
                        groups.push((PathRoot::Local, target.full_path()));
                    }
                    hit_groups(
                        binding.source_location.span.start,
                        binding.source_location.span.end,
                        &groups,
                        tokens,
                        offset,
                        hits,
                    );
                }
            }
            InstanceListRow::Values {
                values,
                source_location,
            } => {
                let groups: Vec<(PathRoot, Vec<String>)> = values
                    .iter()
                    .filter_map(|value| match value {
                        WithRhs::Reference { target } => {
                            Some((PathRoot::Local, target.full_path()))
                        }
                        WithRhs::Literal(_) => None,
                    })
                    .collect();
                hit_groups(
                    source_location.span.start,
                    source_location.span.end,
                    &groups,
                    tokens,
                    offset,
                    hits,
                );
            }
        }
    }
}

fn field_root(spec: &LemmaSpec, element: &str) -> PathRoot {
    if let Some(alias) = import_alias(spec, element) {
        PathRoot::Alias(alias.to_string())
    } else {
        PathRoot::Element(element.to_string())
    }
}

/// Names in a type annotation, without `list` / `range` and without a primitive.
///
/// `bedragen.money` is `[bedragen, money]`. `number` and `money list` contribute
/// only the custom names. The primitive has no declaration to open.
fn type_path(ty: &ParentType) -> Vec<String> {
    let mut current = ty;
    loop {
        match current {
            ParentType::List { inner } | ParentType::Ranged { inner } => current = inner,
            ParentType::Primitive { .. } => return Vec::new(),
            ParentType::Custom { name } => return vec![name.clone()],
            ParentType::Qualified { spec_alias, inner } => {
                let mut names = vec![spec_alias.clone()];
                if let ParentType::Custom { name } = inner.as_ref() {
                    names.push(name.clone());
                }
                return names;
            }
        }
    }
}

fn hit_parent_type(
    start: usize,
    end: usize,
    ty: &ParentType,
    tokens: &[Tok],
    offset: usize,
    hits: &mut Vec<Hit>,
) {
    let names = type_path(ty);
    if names.is_empty() {
        return;
    }
    let Some(colon_end) = tokens.iter().find_map(|token| {
        (token.kind == TokenKind::Colon && token.start >= start && token.end <= end)
            .then_some(token.end)
    }) else {
        panic!("BUG: parent type `{ty}` has no `:` in bytes {start}..{end}");
    };
    let labels = labels_in(tokens, colon_end, end);
    let Some((_, matched)) = match_names(&labels, 0, &names) else {
        panic!("BUG: parent type `{ty}` has no name tokens in bytes {start}..{end}");
    };
    for (segment, label_index) in matched.iter().enumerate() {
        let label = &labels[*label_index];
        if label.start <= offset && offset < label.end {
            hits.push(Hit {
                start: label.start,
                end: label.end,
                kind: HitKind::LocalPath {
                    names: names[..=segment].to_vec(),
                },
            });
        }
    }
}

fn list_element_name(base: Option<&ParentType>) -> Option<String> {
    let mut current = base?;
    loop {
        match current {
            ParentType::List { inner } | ParentType::Ranged { inner } => current = inner,
            ParentType::Custom { name } => return Some(name.clone()),
            ParentType::Qualified { .. } | ParentType::Primitive { .. } => return None,
        }
    }
}

fn hit_groups(
    start: usize,
    end: usize,
    groups: &[(PathRoot, Vec<String>)],
    tokens: &[Tok],
    offset: usize,
    hits: &mut Vec<Hit>,
) {
    let labels = labels_in(tokens, start, end);
    let mut idx = 0;
    for (root, path) in groups {
        let Some((next, matched)) = match_names(&labels, idx, path) else {
            break;
        };
        idx = next;
        for (segment, label_index) in matched.iter().enumerate() {
            let label = &labels[*label_index];
            if label.start <= offset && offset < label.end {
                hits.push(Hit {
                    start: label.start,
                    end: label.end,
                    kind: hit_kind(root, &path[..=segment]),
                });
            }
        }
    }
}

fn hit_kind(root: &PathRoot, prefix: &[String]) -> HitKind {
    match root {
        PathRoot::Local => HitKind::LocalPath {
            names: prefix.to_vec(),
        },
        PathRoot::Alias(alias) => {
            let mut names = Vec::with_capacity(prefix.len() + 1);
            names.push(alias.clone());
            names.extend(prefix.iter().cloned());
            HitKind::LocalPath { names }
        }
        PathRoot::Element(element) => HitKind::ElementPath {
            element: element.clone(),
            names: prefix.to_vec(),
        },
    }
}

fn hit_after_keyword(
    start: usize,
    end: usize,
    keyword: TokenKind,
    path: &[String],
    tokens: &[Tok],
    offset: usize,
    hits: &mut Vec<Hit>,
) {
    let Some(keyword_end) = tokens.iter().find_map(|token| {
        (token.kind == keyword && token.start >= start && token.end <= end).then_some(token.end)
    }) else {
        return;
    };
    let labels = labels_in(tokens, keyword_end, end);
    let Some((_, matched)) = match_names(&labels, 0, path) else {
        return;
    };
    for (segment, label_index) in matched.iter().enumerate() {
        let label = &labels[*label_index];
        if label.start <= offset && offset < label.end {
            hits.push(Hit {
                start: label.start,
                end: label.end,
                kind: HitKind::LocalPath {
                    names: path[..=segment].to_vec(),
                },
            });
        }
    }
}

/// `ElementPath` when `element` is a spec name rather than a `uses` alias.
/// A `uses` alias is a [`HitKind::LocalPath`] that starts with that alias.
fn resolve_element_path<'a, F>(
    spec: &'a LemmaSpec,
    repository: Option<&str>,
    effective: Option<&DateTimeValue>,
    element: &str,
    names: &[String],
    engine: &'a Engine,
    text_of: &F,
) -> Vec<DefinitionSite>
where
    F: Fn(&SourceType) -> Option<&'a str>,
{
    if import_alias(spec, element).is_some() {
        let mut full = vec![element.to_string()];
        full.extend(names.iter().cloned());
        return resolve_path(spec, repository, effective, &full, engine, text_of);
    }
    let Ok(next) = engine.spec_declaration(repository, element, effective) else {
        return Vec::new();
    };
    let deeper = next.effective_from().cloned();
    let next_repo = repository.map(str::to_string);
    resolve_path(
        next,
        next_repo.as_deref(),
        deeper.as_ref(),
        names,
        engine,
        text_of,
    )
}

fn resolve_path<'a, F>(
    spec: &'a LemmaSpec,
    repository: Option<&str>,
    effective: Option<&DateTimeValue>,
    names: &[String],
    engine: &'a Engine,
    text_of: &F,
) -> Vec<DefinitionSite>
where
    F: Fn(&SourceType) -> Option<&'a str>,
{
    struct Frame<'a> {
        spec: &'a LemmaSpec,
        repository: Option<String>,
        effective: Option<DateTimeValue>,
        names: &'a [String],
    }

    let mut sites = Vec::new();
    let mut stack = vec![Frame {
        spec,
        repository: repository.map(str::to_string),
        effective: effective.cloned(),
        names,
    }];
    while let Some(frame) = stack.pop() {
        let Some((segment, rest)) = frame.names.split_first() else {
            continue;
        };
        if rest.is_empty() {
            sites.extend(sites_named(frame.spec, segment, text_of));
            continue;
        }
        // Push last-to-first so the first import is resolved before the next,
        // matching a depth-first walk of the path.
        let imports = imports_named(frame.spec, segment);
        for spec_ref in imports.into_iter().rev() {
            let next_repo = spec_ref
                .repository
                .as_ref()
                .map(|qualifier| qualifier.name.clone())
                .or_else(|| frame.repository.clone());
            let next_effective = spec_ref
                .effective
                .clone()
                .or_else(|| frame.effective.clone());
            let Ok(next) = engine.spec_declaration(
                next_repo.as_deref(),
                &spec_ref.name,
                next_effective.as_ref(),
            ) else {
                continue;
            };
            stack.push(Frame {
                spec: next,
                repository: next_repo,
                effective: next.effective_from().cloned(),
                names: rest,
            });
        }
    }
    sites
}

fn imports_named<'a>(spec: &'a LemmaSpec, name: &str) -> Vec<&'a SpecRef> {
    spec.data
        .iter()
        .filter_map(|data| {
            if !data.reference.name.eq_ignore_ascii_case(name) {
                return None;
            }
            match &data.value {
                DataValue::Import { spec_ref, .. } | DataValue::ImportList { spec_ref, .. } => {
                    Some(spec_ref)
                }
                DataValue::Definition { .. } => None,
            }
        })
        .collect()
}

fn import_alias<'a>(spec: &'a LemmaSpec, name: &str) -> Option<&'a str> {
    spec.data.iter().find_map(|data| {
        if !data.reference.name.eq_ignore_ascii_case(name) {
            return None;
        }
        match &data.value {
            DataValue::Import { .. } | DataValue::ImportList { .. } => {
                Some(data.reference.name.as_str())
            }
            DataValue::Definition { .. } => None,
        }
    })
}

fn sites_named<'a, F>(spec: &LemmaSpec, name: &str, text_of: &F) -> Vec<DefinitionSite>
where
    F: Fn(&SourceType) -> Option<&'a str>,
{
    let mut found = Vec::new();
    for data in &spec.data {
        if data.reference.name.eq_ignore_ascii_case(name) {
            if let Some(site) = statement_site(&data.source_location, text_of) {
                found.push((data.source_location.span.start, site));
            }
        }
    }
    for rule in &spec.rules {
        if rule.name.eq_ignore_ascii_case(name) {
            if let Some(site) = statement_site(&rule.source_location, text_of) {
                found.push((rule.source_location.span.start, site));
            }
        }
    }
    found.sort_by_key(|(start, _)| *start);
    found.into_iter().map(|(_, site)| site).collect()
}

fn statement_site<'a, F>(source: &Source, text_of: &F) -> Option<DefinitionSite>
where
    F: Fn(&SourceType) -> Option<&'a str>,
{
    let text = text_of(&source.source_type)?;
    let (start, end) = name_after_keyword(
        text,
        source.span.start,
        source.span.end,
        &[TokenKind::Data, TokenKind::Rule, TokenKind::Uses],
    )?;
    Some(DefinitionSite {
        source_type: source.source_type.clone(),
        start,
        end,
    })
}

fn spec_site<'a, F>(spec: &LemmaSpec, qualifier: bool, text_of: &F) -> Option<DefinitionSite>
where
    F: Fn(&SourceType) -> Option<&'a str>,
{
    let source_type = spec.source_type.clone()?;
    let text = text_of(&source_type)?;
    let (start, end) = if qualifier {
        repo_name_span(text).or_else(|| spec_name_span(text, spec.start_line))?
    } else {
        spec_name_span(text, spec.start_line)?
    };
    Some(DefinitionSite {
        source_type,
        start,
        end,
    })
}

fn name_after_keyword(
    text: &str,
    start: usize,
    end: usize,
    keywords: &[TokenKind],
) -> Option<(usize, usize)> {
    let mut lexer = Lexer::new(text, &SourceType::Volatile);
    loop {
        let tok = lexer.next_token().ok()?;
        if tok.kind == TokenKind::Eof || tok.span.start >= end {
            return None;
        }
        if tok.span.start < start || !keywords.contains(&tok.kind) {
            continue;
        }
        let name = lexer.next_logical_name().ok()?;
        if name.span.start < start || name.span.end > end {
            return None;
        }
        return Some((name.span.start, name.span.end));
    }
}

fn spec_name_span(text: &str, start_line: usize) -> Option<(usize, usize)> {
    let mut lexer = Lexer::new(text, &SourceType::Volatile);
    loop {
        let tok = lexer.next_token().ok()?;
        if tok.kind == TokenKind::Eof {
            return None;
        }
        if tok.kind == TokenKind::Spec && tok.span.line == start_line {
            let name = lexer.next_logical_name().ok()?;
            if name.span.line != start_line {
                return None;
            }
            return Some((name.span.start, name.span.end));
        }
    }
}

fn repo_name_span(text: &str) -> Option<(usize, usize)> {
    let mut lexer = Lexer::new(text, &SourceType::Volatile);
    loop {
        let tok = lexer.next_token().ok()?;
        if tok.kind == TokenKind::Eof {
            return None;
        }
        if tok.kind == TokenKind::Repo {
            let name = lexer.next_logical_name().ok()?;
            return Some((name.span.start, name.span.end));
        }
    }
}

fn segment_spans(text: &str, start: usize, end: usize) -> Vec<(usize, usize)> {
    let slice = &text[start..end];
    let mut spans = Vec::new();
    let mut ident_start: Option<usize> = None;
    for (index, ch) in slice.char_indices() {
        let abs = start + index;
        if ch == '.' || ch.is_whitespace() {
            if let Some(ident) = ident_start.take() {
                spans.push((ident, abs));
            }
        } else if ident_start.is_none() {
            ident_start = Some(abs);
        }
    }
    if let Some(ident) = ident_start {
        spans.push((ident, end));
    }
    spans
}

fn lex_tokens(text: &str) -> Vec<Tok> {
    let mut lexer = Lexer::new(text, &SourceType::Volatile);
    let mut tokens = Vec::new();
    loop {
        let tok = lexer.next_token().unwrap_or_else(|error| {
            panic!("BUG: lex of a parsed buffer failed: {error}");
        });
        if tok.kind == TokenKind::Eof {
            break;
        }
        tokens.push(Tok {
            kind: tok.kind,
            start: tok.span.start,
            end: tok.span.end,
            text: tok.text.of(text).to_string(),
        });
    }
    tokens
}

fn labels_in(tokens: &[Tok], start: usize, end: usize) -> Vec<Label> {
    tokens
        .iter()
        .filter(|token| token.start >= start && token.end <= end && can_be_label(&token.kind))
        .map(|token| Label {
            text: token.text.clone(),
            start: token.start,
            end: token.end,
        })
        .collect()
}

/// Match `names` in order against `labels`, skipping labels that are not the next name.
fn match_names(labels: &[Label], from: usize, names: &[String]) -> Option<(usize, Vec<usize>)> {
    let mut idx = from;
    let mut matched = Vec::new();
    for name in names {
        let mut found = None;
        for (offset, label) in labels[idx..].iter().enumerate() {
            if label.text.eq_ignore_ascii_case(name) {
                found = Some(idx + offset);
                break;
            }
        }
        let at = found?;
        matched.push(at);
        idx = at + 1;
    }
    Some((idx, matched))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::byte_offset_to_position;
    use lemma::{parse, Engine, ResourceLimits};
    use std::path::PathBuf;
    use std::sync::Arc;

    fn path_source(path: &str) -> SourceType {
        SourceType::Path(Arc::new(PathBuf::from(path)))
    }

    fn cursor_on(text: &str, needle: &str, nth: usize) -> Position {
        byte_offset_to_position(text, find_nth(text, needle, nth))
    }

    fn find_nth(text: &str, needle: &str, nth: usize) -> usize {
        let mut from = 0;
        for index in 0..=nth {
            let rel = text[from..]
                .find(needle)
                .unwrap_or_else(|| panic!("missing {needle} #{nth} in {text}"));
            let at = from + rel;
            if index == nth {
                return at;
            }
            from = at + needle.len();
        }
        unreachable!("loop returns on nth")
    }

    fn load_pair<'a>(
        employee_text: &'a str,
        payroll_text: &'a str,
    ) -> (Engine, ParseResult, SourceType, SourceType) {
        let employee_source = path_source("/employee.lemma");
        let payroll_source = path_source("/payroll.lemma");
        let mut engine = Engine::new();
        engine
            .load([
                (employee_source.clone(), employee_text),
                (payroll_source.clone(), payroll_text),
            ])
            .expect("load employee and payroll");
        let parse = parse(
            payroll_text,
            payroll_source.clone(),
            &ResourceLimits::default(),
        )
        .expect("parse payroll");
        (engine, parse, employee_source, payroll_source)
    }

    #[test]
    fn dotted_path_picks_alias_or_field() {
        let employee_text = "spec employee\ndata salary: number\n";
        let payroll_text = "spec payroll\nuses employee\nrule pay: employee.salary\n";
        let (engine, parse, employee_source, payroll_source) =
            load_pair(employee_text, payroll_text);
        let text_of = |source: &SourceType| {
            if source == &employee_source {
                Some(employee_text)
            } else if source == &payroll_source {
                Some(payroll_text)
            } else {
                None
            }
        };
        let dotted = find_nth(payroll_text, "employee.salary", 0);

        let alias = definitions_at(
            payroll_text,
            byte_offset_to_position(payroll_text, dotted),
            &parse,
            &engine,
            text_of,
        );
        assert_eq!(alias.len(), 1, "alias sites: {alias:?}");
        assert_eq!(alias[0].source_type, payroll_source);
        assert_eq!(&payroll_text[alias[0].start..alias[0].end], "employee");

        let field = definitions_at(
            payroll_text,
            byte_offset_to_position(payroll_text, dotted + "employee.".len()),
            &parse,
            &engine,
            text_of,
        );
        assert_eq!(field.len(), 1, "field sites: {field:?}");
        assert_eq!(field[0].source_type, employee_source);
        assert_eq!(&employee_text[field[0].start..field[0].end], "salary");

        let dot = definitions_at(
            payroll_text,
            byte_offset_to_position(payroll_text, dotted + "employee".len()),
            &parse,
            &engine,
            text_of,
        );
        assert!(dot.is_empty(), "dot is not a name: {dot:?}");
    }

    #[test]
    fn uses_binding_jumps_to_field_and_local() {
        let employee_text = "spec employee\ndata salary: number\n";
        let payroll_text =
            "spec payroll\ndata base: number\nuses employee\n  -> with salary: base\n";
        let (engine, parse, employee_source, payroll_source) =
            load_pair(employee_text, payroll_text);
        let text_of = |source: &SourceType| {
            if source == &employee_source {
                Some(employee_text)
            } else if source == &payroll_source {
                Some(payroll_text)
            } else {
                None
            }
        };

        let field = definitions_at(
            payroll_text,
            cursor_on(payroll_text, "salary", 0),
            &parse,
            &engine,
            text_of,
        );
        assert_eq!(field.len(), 1, "binding field: {field:?}");
        assert_eq!(field[0].source_type, employee_source);
        assert_eq!(&employee_text[field[0].start..field[0].end], "salary");

        let local = definitions_at(
            payroll_text,
            cursor_on(payroll_text, "base", 1),
            &parse,
            &engine,
            text_of,
        );
        assert_eq!(local.len(), 1, "binding rhs: {local:?}");
        assert_eq!(local[0].source_type, payroll_source);
        assert_eq!(&payroll_text[local[0].start..local[0].end], "base");
    }

    #[test]
    fn uses_spec_name_jumps_to_spec_header() {
        let employee_text = "spec employee\ndata salary: number\n";
        let payroll_text = "spec payroll\nuses employee\n";
        let (engine, parse, employee_source, payroll_source) =
            load_pair(employee_text, payroll_text);
        let text_of = |source: &SourceType| {
            if source == &employee_source {
                Some(employee_text)
            } else if source == &payroll_source {
                Some(payroll_text)
            } else {
                None
            }
        };
        let sites = definitions_at(
            payroll_text,
            cursor_on(payroll_text, "employee", 0),
            &parse,
            &engine,
            text_of,
        );
        assert_eq!(sites.len(), 1, "spec sites: {sites:?}");
        assert_eq!(sites[0].source_type, employee_source);
        assert_eq!(&employee_text[sites[0].start..sites[0].end], "employee");
    }

    #[test]
    fn type_path_jumps_to_alias_and_type() {
        let bedragen_text = "spec bedragen\ndata money: measure\n  -> unit eur: 1\n";
        let payroll_text =
            "spec payroll\nuses bedragen\ndata tarief: bedragen.money\n  -> suggest 0 eur\n";
        let (engine, parse, bedragen_source, payroll_source) =
            load_pair(bedragen_text, payroll_text);
        let text_of = |source: &SourceType| {
            if source == &bedragen_source {
                Some(bedragen_text)
            } else if source == &payroll_source {
                Some(payroll_text)
            } else {
                None
            }
        };
        let dotted = find_nth(payroll_text, "bedragen.money", 0);

        let alias = definitions_at(
            payroll_text,
            byte_offset_to_position(payroll_text, dotted),
            &parse,
            &engine,
            text_of,
        );
        assert_eq!(alias.len(), 1, "type alias: {alias:?}");
        assert_eq!(alias[0].source_type, payroll_source);
        assert_eq!(&payroll_text[alias[0].start..alias[0].end], "bedragen");
        assert_eq!(alias[0].start, find_nth(payroll_text, "bedragen", 0));

        let money = definitions_at(
            payroll_text,
            byte_offset_to_position(payroll_text, dotted + "bedragen.".len()),
            &parse,
            &engine,
            text_of,
        );
        assert_eq!(money.len(), 1, "type name: {money:?}");
        assert_eq!(money[0].source_type, bedragen_source);
        assert_eq!(&bedragen_text[money[0].start..money[0].end], "money");
    }

    #[test]
    fn local_type_jumps_to_data() {
        let text = "spec pricing\ndata money: number\ndata price: money\n";
        let source = path_source("/pricing.lemma");
        let mut engine = Engine::new();
        engine.load([(source.clone(), text)]).expect("load pricing");
        let parse = parse(text, source.clone(), &ResourceLimits::default()).expect("parse pricing");
        let text_of = |candidate: &SourceType| (candidate == &source).then_some(text);
        let sites = definitions_at(text, cursor_on(text, "money", 1), &parse, &engine, text_of);
        assert_eq!(sites.len(), 1, "local type: {sites:?}");
        assert_eq!(sites[0].start, find_nth(text, "money", 0));
        assert_eq!(&text[sites[0].start..sites[0].end], "money");

        let primitive =
            definitions_at(text, cursor_on(text, "number", 0), &parse, &engine, text_of);
        assert!(primitive.is_empty(), "primitive type: {primitive:?}");
    }
}
