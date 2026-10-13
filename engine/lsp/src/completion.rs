use crate::diagnostics::{position_to_byte_offset, position_within_range, span_to_range};
use lemma::{
    can_be_label, CommandArg, DataValue, Engine, LemmaData, LemmaRepository, LemmaSpec, Lexer,
    ParentType, ParseResult, SourceType, TokenKind, TypeConstraintCommand, TypeSpecification,
    COMPLETION_KEYWORD_LABELS,
};
use std::collections::BTreeMap;
use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, CompletionTextEdit, Documentation, MarkupContent,
    MarkupKind, Position, Range, TextEdit,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArrowKind {
    Data,
    RuleExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionContext {
    FileTop,
    SpecBody,
    TypeAnnotation,
    ArrowCommand { kind: ArrowKind },
    Expression,
    UsesTarget,
    UnitLiteral { data_name: Option<String> },
    SpecMember { alias: String },
}

struct PrefixTokens {
    kinds: Vec<TokenKind>,
    texts: Vec<String>,
    last_end_at_cursor: bool,
    last_text: String,
    last_span_in_prefix: Option<(usize, usize)>,
}

struct Classified {
    context: CompletionContext,
    replace_range: Range,
    spec: Option<(Option<String>, String, Option<lemma::DateTimeValue>)>,
    partial: String,
}

/// Completions at `position` in `text`.
///
/// `parse` is the last successful parse of this buffer, if any. `engine` is the
/// workspace engine after validate (may lag the buffer). Neither is required.
pub fn complete(
    text: &str,
    position: Position,
    parse: Option<&ParseResult>,
    engine: Option<&Engine>,
) -> Vec<CompletionItem> {
    let classified = classify_full(text, position, parse);
    collect(&classified, parse, engine)
}

pub fn classify(text: &str, position: Position, parse: Option<&ParseResult>) -> CompletionContext {
    classify_full(text, position, parse).context
}

fn classify_full(text: &str, position: Position, parse: Option<&ParseResult>) -> Classified {
    let offset = position_to_byte_offset(text, position);
    let (line_start, prefix) = current_line_prefix(text, offset);
    let tokens = lex_prefix(prefix);
    let replace_range = replace_range_for(text, line_start, &tokens, position);
    let cursor_line = (position.line as usize).saturating_add(1);

    let enclosing = parse.and_then(|p| enclosing_spec(p, cursor_line));
    let statement = enclosing.and_then(|(_, spec)| enclosing_statement(spec, text, position));
    let prefix_context = context_from_prefix(&tokens, statement.as_ref());

    let context = if let Some(ctx) = prefix_context {
        ctx
    } else if let Some(stmt) = statement {
        match stmt {
            EnclosingStatement::Data(data) => match &data.value {
                DataValue::Import { .. } | DataValue::ImportList { .. } => {
                    CompletionContext::UsesTarget
                }
                DataValue::Definition { .. } => CompletionContext::TypeAnnotation,
            },
            EnclosingStatement::Rule => CompletionContext::Expression,
            EnclosingStatement::Meta => CompletionContext::SpecBody,
        }
    } else if enclosing.is_some() {
        CompletionContext::SpecBody
    } else {
        CompletionContext::FileTop
    };

    let spec = enclosing.map(|(repo, spec)| {
        (
            repo.name.clone(),
            spec.name.clone(),
            spec.effective_from().cloned(),
        )
    });

    let partial = if cursor_ends_in_word(&tokens) {
        tokens.last_text.clone()
    } else {
        String::new()
    };

    Classified {
        context,
        replace_range,
        spec,
        partial,
    }
}

fn collect(
    classified: &Classified,
    parse: Option<&ParseResult>,
    engine: Option<&Engine>,
) -> Vec<CompletionItem> {
    let mut items: BTreeMap<String, RankedItem> = BTreeMap::new();
    let partial = classified.partial.as_str();

    match &classified.context {
        CompletionContext::FileTop => {
            push_keywords(&mut items, &["repo", "spec"], Group::Keyword);
        }
        CompletionContext::SpecBody => {
            push_keywords(
                &mut items,
                &["data", "rule", "uses", "meta"],
                Group::Keyword,
            );
        }
        CompletionContext::TypeAnnotation => {
            push_keywords(
                &mut items,
                &[
                    "measure", "number", "text", "date", "time", "boolean", "ratio", "list",
                    "range",
                ],
                Group::Type,
            );
            if let Some((_, spec)) = parse.and_then(|p| {
                classified
                    .spec
                    .as_ref()
                    .and_then(|(_, name, _)| enclosing_spec_named(p, name))
            }) {
                for name in custom_type_names(spec) {
                    push_item(
                        &mut items,
                        name,
                        CompletionItemKind::CLASS,
                        Group::Type,
                        None,
                        None,
                    );
                }
                for data in &spec.data {
                    if matches!(
                        data.value,
                        DataValue::Import { .. } | DataValue::ImportList { .. }
                    ) {
                        push_item(
                            &mut items,
                            data.reference.name.clone(),
                            CompletionItemKind::MODULE,
                            Group::Local,
                            None,
                            None,
                        );
                    }
                }
            }
        }
        CompletionContext::ArrowCommand { kind } => match kind {
            ArrowKind::Data => {
                for command in TypeConstraintCommand::ALL {
                    if command.is_data_constraint() {
                        push_item(
                            &mut items,
                            command.to_string(),
                            CompletionItemKind::KEYWORD,
                            Group::Command,
                            None,
                            None,
                        );
                    }
                }
            }
            ArrowKind::RuleExpr => {
                push_keywords(
                    &mut items,
                    &[
                        "find", "first", "last", "sum", "avg", "product", "count", "min", "max",
                        "any", "all", "where", "sort", "top", "with",
                    ],
                    Group::Command,
                );
            }
        },
        CompletionContext::Expression => {
            push_keywords(
                &mut items,
                &[
                    "unless", "then", "not", "and", "in", "as", "veto", "now", "past", "future",
                    "true", "false", "yes", "no", "sqrt", "sin", "cos", "tan", "asin", "acos",
                    "atan", "log", "exp", "abs", "floor", "ceil", "round",
                ],
                Group::Keyword,
            );
            if let Some((_, spec)) = parse.and_then(|p| {
                classified
                    .spec
                    .as_ref()
                    .and_then(|(_, name, _)| enclosing_spec_named(p, name))
            }) {
                push_locals(&mut items, spec);
            }
            push_show(&mut items, classified, engine);
        }
        CompletionContext::UsesTarget => {
            if let Some(engine) = engine {
                for repo in engine.specs() {
                    if let Some(name) = repo.repository {
                        push_item(
                            &mut items,
                            name,
                            CompletionItemKind::MODULE,
                            Group::Spec,
                            None,
                            None,
                        );
                    }
                    for version in repo.specs {
                        push_item(
                            &mut items,
                            version.name,
                            CompletionItemKind::CLASS,
                            Group::Spec,
                            None,
                            None,
                        );
                    }
                }
            }
        }
        CompletionContext::UnitLiteral { data_name } => {
            push_units(&mut items, classified, parse, engine, data_name.as_deref());
        }
        CompletionContext::SpecMember { alias } => {
            push_spec_members(&mut items, classified, parse, engine, alias);
        }
    }

    let mut ranked: Vec<RankedItem> = items.into_values().collect();
    if !partial.is_empty() {
        let needle = partial.to_ascii_lowercase();
        ranked.retain(|item| item.item.label.to_ascii_lowercase().starts_with(&needle));
    }
    ranked.sort_by(|a, b| {
        a.group
            .cmp(&b.group)
            .then_with(|| a.item.label.cmp(&b.item.label))
    });
    ranked
        .into_iter()
        .map(|mut ranked| {
            ranked.item.text_edit = Some(CompletionTextEdit::Edit(TextEdit {
                range: classified.replace_range,
                new_text: ranked.item.label.clone(),
            }));
            ranked.item
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    Keyword,
    Command,
    Type,
    Local,
    Show,
    Spec,
    Unit,
}

struct RankedItem {
    group: Group,
    item: CompletionItem,
}

enum EnclosingStatement<'a> {
    Data(&'a LemmaData),
    Rule,
    Meta,
}

fn current_line_prefix(text: &str, offset: usize) -> (usize, &str) {
    let clamped = offset.min(text.len());
    let line_start = text[..clamped].rfind('\n').map(|i| i + 1).unwrap_or(0);
    (line_start, &text[line_start..clamped])
}

fn lex_prefix(prefix: &str) -> PrefixTokens {
    let mut lexer = Lexer::new(prefix, &SourceType::Volatile);
    let mut kinds = Vec::new();
    let mut texts = Vec::new();
    let mut last_end_at_cursor = false;
    let mut last_text = String::new();
    let mut last_span_in_prefix = None;
    while let Ok(tok) = lexer.next_token() {
        if tok.kind == TokenKind::Eof {
            break;
        }
        last_end_at_cursor = tok.span.end == prefix.len();
        last_text = tok.text.clone();
        last_span_in_prefix = Some((tok.span.start, tok.span.end));
        kinds.push(tok.kind);
        texts.push(tok.text.clone());
    }
    PrefixTokens {
        kinds,
        texts,
        last_end_at_cursor,
        last_text,
        last_span_in_prefix,
    }
}

fn replace_range_for(
    text: &str,
    line_start: usize,
    tokens: &PrefixTokens,
    cursor: Position,
) -> Range {
    if tokens.last_end_at_cursor {
        if let Some((start, end)) = tokens.last_span_in_prefix {
            return span_to_range(text, line_start + start, line_start + end);
        }
    }
    Range {
        start: cursor,
        end: cursor,
    }
}

fn context_from_prefix(
    tokens: &PrefixTokens,
    statement: Option<&EnclosingStatement<'_>>,
) -> Option<CompletionContext> {
    if tokens.kinds.is_empty() {
        return None;
    }

    let kinds = significant_kinds(tokens);
    if kinds.is_empty() {
        return None;
    }

    if kinds.contains(&TokenKind::Uses) {
        return Some(CompletionContext::UsesTarget);
    }

    if let Some(arrow_at) = kinds.iter().rposition(|k| *k == TokenKind::Arrow) {
        let after = &kinds[arrow_at + 1..];
        if unit_command_after_arrow(after, tokens) {
            let data_name = statement.and_then(|s| match s {
                EnclosingStatement::Data(d) => Some(d.reference.name.clone()),
                _ => None,
            });
            return Some(CompletionContext::UnitLiteral { data_name });
        }
        let kind = match statement {
            Some(EnclosingStatement::Rule) => ArrowKind::RuleExpr,
            Some(EnclosingStatement::Data(_)) => ArrowKind::Data,
            _ => {
                if kinds.contains(&TokenKind::Rule) {
                    ArrowKind::RuleExpr
                } else {
                    ArrowKind::Data
                }
            }
        };
        return Some(CompletionContext::ArrowCommand { kind });
    }

    if kinds.contains(&TokenKind::Data) && kinds.contains(&TokenKind::Colon) {
        return Some(CompletionContext::TypeAnnotation);
    }
    if kinds.contains(&TokenKind::Rule) && kinds.contains(&TokenKind::Colon) {
        if let Some(alias) = member_access_alias(tokens) {
            return Some(CompletionContext::SpecMember { alias });
        }
        return Some(CompletionContext::Expression);
    }
    if matches!(kinds[0], TokenKind::Spec | TokenKind::Repo) && kinds.len() <= 3 {
        return Some(CompletionContext::FileTop);
    }
    None
}

fn member_access_alias(tokens: &PrefixTokens) -> Option<String> {
    let mut end = tokens.kinds.len();
    if tokens.last_end_at_cursor && end > 0 && tokens.kinds[end - 1] == TokenKind::Identifier {
        end -= 1;
    }
    if end < 2 || tokens.kinds[end - 1] != TokenKind::Dot {
        return None;
    }
    if tokens.kinds[end - 2] != TokenKind::Identifier {
        return None;
    }
    Some(tokens.texts[end - 2].clone())
}

fn cursor_ends_in_word(tokens: &PrefixTokens) -> bool {
    if !tokens.last_end_at_cursor {
        return false;
    }
    let Some(last) = tokens.kinds.last() else {
        return false;
    };
    can_be_label(last)
        || matches!(last, TokenKind::Identifier | TokenKind::LogicalName)
        || lemma::try_parse_type_constraint_command(&tokens.last_text).is_some()
}

fn significant_kinds(tokens: &PrefixTokens) -> Vec<TokenKind> {
    if cursor_ends_in_word(tokens) {
        return tokens.kinds[..tokens.kinds.len() - 1].to_vec();
    }
    tokens.kinds.clone()
}

fn unit_command_after_arrow(after: &[TokenKind], tokens: &PrefixTokens) -> bool {
    if after.is_empty() {
        return false;
    }
    after.len() == 1
        && after[0] == TokenKind::Identifier
        && tokens.last_text.eq_ignore_ascii_case("unit")
        && !tokens.last_end_at_cursor
}

fn enclosing_spec<'a>(
    parse: &'a ParseResult,
    cursor_line: usize,
) -> Option<(&'a LemmaRepository, &'a LemmaSpec)> {
    let mut best: Option<(&'a LemmaRepository, &'a LemmaSpec)> = None;
    for (repo, specs) in &parse.repositories {
        for spec in specs {
            if spec.start_line <= cursor_line {
                match best {
                    None => best = Some((repo.as_ref(), spec)),
                    Some((_, prev)) if spec.start_line >= prev.start_line => {
                        best = Some((repo.as_ref(), spec));
                    }
                    _ => {}
                }
            }
        }
    }
    best
}

fn enclosing_spec_named<'a>(
    parse: &'a ParseResult,
    name: &str,
) -> Option<(&'a LemmaRepository, &'a LemmaSpec)> {
    for (repo, specs) in &parse.repositories {
        for spec in specs {
            if spec.name == name {
                return Some((repo.as_ref(), spec));
            }
        }
    }
    None
}

fn enclosing_statement<'a>(
    spec: &'a LemmaSpec,
    text: &str,
    position: Position,
) -> Option<EnclosingStatement<'a>> {
    for data in &spec.data {
        let range = span_to_range(
            text,
            data.source_location.span.start,
            data.source_location.span.end,
        );
        if position_within_range(position, &range) {
            return Some(EnclosingStatement::Data(data));
        }
    }
    for rule in &spec.rules {
        let range = span_to_range(
            text,
            rule.source_location.span.start,
            rule.source_location.span.end,
        );
        if position_within_range(position, &range) {
            return Some(EnclosingStatement::Rule);
        }
    }
    for meta in &spec.meta_fields {
        let range = span_to_range(
            text,
            meta.source_location.span.start,
            meta.source_location.span.end,
        );
        if position_within_range(position, &range) {
            return Some(EnclosingStatement::Meta);
        }
    }
    None
}

fn custom_type_names(spec: &LemmaSpec) -> Vec<String> {
    let mut names = Vec::new();
    for data in &spec.data {
        if let DataValue::Definition { base: Some(ty), .. } = &data.value {
            collect_parent_type_names(ty, &mut names);
        }
    }
    names
}

fn collect_parent_type_names(ty: &ParentType, names: &mut Vec<String>) {
    let mut current = ty;
    loop {
        match current {
            ParentType::Custom { name } => {
                names.push(name.clone());
                return;
            }
            ParentType::Qualified { spec_alias, inner } => {
                names.push(spec_alias.clone());
                current = inner.as_ref();
            }
            ParentType::Ranged { inner } | ParentType::List { inner } => current = inner.as_ref(),
            ParentType::Primitive { .. } => return,
        }
    }
}

fn push_keywords(items: &mut BTreeMap<String, RankedItem>, labels: &[&str], group: Group) {
    for label in labels {
        debug_assert!(
            COMPLETION_KEYWORD_LABELS.contains(label) || *label == "range",
            "completion label {label} missing from lexer catalog (range is a postfix type word)"
        );
        push_item(
            items,
            (*label).to_string(),
            CompletionItemKind::KEYWORD,
            group,
            None,
            None,
        );
    }
}

fn push_locals(items: &mut BTreeMap<String, RankedItem>, spec: &LemmaSpec) {
    for data in &spec.data {
        push_item(
            items,
            data.reference.name.clone(),
            CompletionItemKind::FIELD,
            Group::Local,
            None,
            None,
        );
    }
    for rule in &spec.rules {
        push_item(
            items,
            rule.name.clone(),
            CompletionItemKind::FUNCTION,
            Group::Local,
            None,
            None,
        );
    }
}

fn push_show(
    items: &mut BTreeMap<String, RankedItem>,
    classified: &Classified,
    engine: Option<&Engine>,
) {
    let Some(engine) = engine else {
        return;
    };
    let Some((repo, name, effective)) = &classified.spec else {
        return;
    };
    let Ok(show) = engine.show(repo.as_deref(), name, effective.as_ref()) else {
        return;
    };
    for (key, data) in &show.data {
        let detail = Some(data.lemma_type.specifications.to_string());
        let help = data.lemma_type.specifications.help();
        let docs = if help.is_empty() {
            None
        } else {
            Some(help.to_string())
        };
        push_item(
            items,
            key.clone(),
            CompletionItemKind::FIELD,
            Group::Show,
            detail,
            docs,
        );
    }
    for key in show.rules.keys() {
        push_item(
            items,
            key.clone(),
            CompletionItemKind::FUNCTION,
            Group::Show,
            None,
            None,
        );
    }
}

fn push_spec_members(
    items: &mut BTreeMap<String, RankedItem>,
    classified: &Classified,
    parse: Option<&ParseResult>,
    engine: Option<&Engine>,
    alias: &str,
) {
    let (Some(engine), Some(parse), Some((consumer_repo, consumer_spec, consumer_effective))) =
        (engine, parse, classified.spec.as_ref())
    else {
        return;
    };
    let Some((_, spec)) = enclosing_spec_named(parse, consumer_spec) else {
        return;
    };
    let Some(import) = spec.data.iter().find(|data| data.reference.name == alias) else {
        return;
    };
    let DataValue::Import { spec_ref, .. } = &import.value else {
        return;
    };
    let repository = spec_ref
        .repository
        .as_ref()
        .map(|qualifier| qualifier.name.clone())
        .or_else(|| consumer_repo.clone());
    let effective = spec_ref.effective.as_ref().or(consumer_effective.as_ref());
    let Ok(declaration) = engine.spec_declaration(repository.as_deref(), &spec_ref.name, effective)
    else {
        return;
    };
    for data in &declaration.data {
        if matches!(data.value, DataValue::Definition { .. }) {
            push_item(
                items,
                data.reference.name.clone(),
                CompletionItemKind::FIELD,
                Group::Local,
                None,
                None,
            );
        }
    }
    for rule in &declaration.rules {
        push_item(
            items,
            rule.name.clone(),
            CompletionItemKind::FUNCTION,
            Group::Local,
            None,
            None,
        );
    }
}

fn push_units(
    items: &mut BTreeMap<String, RankedItem>,
    classified: &Classified,
    parse: Option<&ParseResult>,
    engine: Option<&Engine>,
    data_name: Option<&str>,
) {
    if let (Some(engine), Some((repo, spec, effective)), Some(data_name)) =
        (engine, classified.spec.as_ref(), data_name)
    {
        if let Ok(show) = engine.show(repo.as_deref(), spec, effective.as_ref()) {
            if let Some(entry) = show.data.get(data_name) {
                for name in unit_names_from_type(&entry.lemma_type.specifications) {
                    push_item(
                        items,
                        name,
                        CompletionItemKind::UNIT,
                        Group::Unit,
                        None,
                        None,
                    );
                }
            }
        }
    }
    if let (Some(parse), Some(data_name)) = (parse, data_name) {
        if let Some((_, spec)) = classified
            .spec
            .as_ref()
            .and_then(|(_, name, _)| enclosing_spec_named(parse, name))
        {
            for data in &spec.data {
                if data.reference.name != data_name {
                    continue;
                }
                for name in ast_unit_names(data) {
                    push_item(
                        items,
                        name,
                        CompletionItemKind::UNIT,
                        Group::Unit,
                        None,
                        None,
                    );
                }
            }
        }
    }
}

fn unit_names_from_type(spec: &TypeSpecification) -> Vec<String> {
    match spec {
        TypeSpecification::Measure { units, .. }
        | TypeSpecification::MeasureRange { units, .. } => {
            units.0.iter().map(|u| u.name.clone()).collect()
        }
        TypeSpecification::Ratio { units, .. } | TypeSpecification::RatioRange { units, .. } => {
            units.0.iter().map(|u| u.name.clone()).collect()
        }
        _ => Vec::new(),
    }
}

fn ast_unit_names(data: &LemmaData) -> Vec<String> {
    let DataValue::Definition {
        constraints: Some(constraints),
        ..
    } = &data.value
    else {
        return Vec::new();
    };
    constraints
        .iter()
        .filter(|c| c.command == TypeConstraintCommand::Unit)
        .filter_map(|c| match c.args.first() {
            Some(CommandArg::Label(name)) => Some(name.clone()),
            _ => None,
        })
        .collect()
}

fn push_item(
    items: &mut BTreeMap<String, RankedItem>,
    label: String,
    kind: CompletionItemKind,
    group: Group,
    detail: Option<String>,
    documentation: Option<String>,
) {
    if label.is_empty() || items.contains_key(&label) {
        return;
    }
    let mut item = CompletionItem::new_simple(label.clone(), String::new());
    item.label = label.clone();
    item.kind = Some(kind);
    item.detail = detail;
    item.documentation = documentation.map(|value| {
        Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        })
    });
    items.insert(label, RankedItem { group, item });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::byte_offset_to_position;
    use lemma::{parse, Engine, ResourceLimits, SourceType};

    fn labels_at(src: &str, marker: &str) -> Vec<String> {
        let (text, position) = position_at_marker(src, marker);
        let parse = parse(&text, SourceType::Volatile, &ResourceLimits::default()).ok();
        complete(&text, position, parse.as_ref(), None)
            .into_iter()
            .map(|i| i.label)
            .collect()
    }

    fn labels_with_engine(src: &str, marker: &str) -> Vec<String> {
        let (text, position) = position_at_marker(src, marker);
        let parse = parse(&text, SourceType::Volatile, &ResourceLimits::default())
            .expect("BUG: completion test source must parse");
        let mut engine = Engine::new();
        engine
            .load([(SourceType::Volatile, text.as_str())])
            .expect("BUG: completion test source must load");
        complete(&text, position, Some(&parse), Some(&engine))
            .into_iter()
            .map(|i| i.label)
            .collect()
    }

    fn context_at(src: &str, marker: &str) -> CompletionContext {
        let (text, position) = position_at_marker(src, marker);
        let parse = parse(&text, SourceType::Volatile, &ResourceLimits::default()).ok();
        classify(&text, position, parse.as_ref())
    }

    fn position_at_marker(src: &str, marker: &str) -> (String, Position) {
        let offset = src
            .find(marker)
            .unwrap_or_else(|| panic!("marker {marker:?} missing"));
        let text = src.replace(marker, "");
        let position = byte_offset_to_position(&text, offset);
        (text, position)
    }

    #[test]
    fn file_top_offers_repo_and_spec() {
        let labels = labels_at("|", "|");
        assert!(labels.contains(&"spec".to_string()));
        assert!(labels.contains(&"repo".to_string()));
        assert!(!labels.contains(&"data".to_string()));
    }

    #[test]
    fn spec_body_offers_declarations() {
        let labels = labels_at("spec pricing\n|", "|");
        assert!(labels.contains(&"data".to_string()));
        assert!(labels.contains(&"rule".to_string()));
        assert!(labels.contains(&"uses".to_string()));
        assert!(labels.contains(&"meta".to_string()));
    }

    #[test]
    fn type_annotation_after_data_colon() {
        assert_eq!(
            context_at("spec pricing\ndata qty: |", "|"),
            CompletionContext::TypeAnnotation
        );
        let labels = labels_at("spec pricing\ndata qty: |", "|");
        assert!(labels.contains(&"number".to_string()));
        assert!(labels.contains(&"measure".to_string()));
        assert!(labels.contains(&"list".to_string()));
    }

    #[test]
    fn arrow_on_data_offers_data_commands_not_list_queries() {
        assert!(matches!(
            context_at("spec pricing\ndata qty: number\n  -> |", "|"),
            CompletionContext::ArrowCommand {
                kind: ArrowKind::Data
            }
        ));
        let labels = labels_at("spec pricing\ndata qty: number\n  -> |", "|");
        assert!(labels.contains(&"minimum".to_string()));
        assert!(labels.contains(&"unit".to_string()));
        assert!(!labels.contains(&"where".to_string()));
        assert!(!labels.contains(&"sum".to_string()));
    }

    #[test]
    fn arrow_on_rule_offers_list_queries() {
        assert!(matches!(
            context_at(
                "spec pricing\ndata rows: number list\nrule total: rows -> |",
                "|"
            ),
            CompletionContext::ArrowCommand {
                kind: ArrowKind::RuleExpr
            }
        ));
        let labels = labels_at(
            "spec pricing\ndata rows: number list\nrule total: rows -> |",
            "|",
        );
        assert!(labels.contains(&"sum".to_string()));
        assert!(labels.contains(&"where".to_string()));
        assert!(!labels.contains(&"minimum".to_string()));
    }

    #[test]
    fn expression_offers_local_data_and_rule_names() {
        let src = "spec pricing\ndata qty: number\nrule total: |qty\n";
        assert_eq!(context_at(src, "|"), CompletionContext::Expression);
        let labels = labels_at(src, "|");
        assert!(
            labels.contains(&"qty".to_string()),
            "locals missing: {labels:?}"
        );
        assert!(labels.contains(&"total".to_string()));
    }

    #[test]
    fn uses_target_context() {
        assert_eq!(
            context_at("spec pricing\nuses |", "|"),
            CompletionContext::UsesTarget
        );
    }

    #[test]
    fn parse_failed_still_offers_type_keywords() {
        let labels = labels_at("spec broken\ndata x: |", "|");
        assert!(labels.contains(&"number".to_string()));
    }

    #[test]
    fn keyword_catalog_covers_spec() {
        assert!(COMPLETION_KEYWORD_LABELS.contains(&"spec"));
        assert!(COMPLETION_KEYWORD_LABELS.contains(&"uses"));
        assert_eq!(
            COMPLETION_KEYWORD_LABELS.len(),
            COMPLETION_KEYWORD_LABELS
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        );
    }

    #[test]
    fn command_sets_partition_all_variants() {
        for command in TypeConstraintCommand::ALL {
            assert!(command.is_data_constraint());
            assert_eq!(
                lemma::try_parse_type_constraint_command(&command.to_string()),
                Some(*command)
            );
        }
        let listed = TypeConstraintCommand::ALL.len();
        assert_eq!(listed, 13, "expected every TypeConstraintCommand variant");
    }

    #[test]
    fn show_backed_symbols_include_imported_data() {
        let src = r#"spec units_src
data mass: measure
  -> unit kg: 1.0

spec consumer
uses units_src
rule out: units_src.|mass
"#;
        let labels = labels_with_engine(src, "|");
        assert!(
            labels.contains(&"mass".to_string()) || labels.contains(&"out".to_string()),
            "show/local symbols missing: {labels:?}"
        );
    }

    #[test]
    fn spec_member_context_after_dot_in_rule() {
        let src = "spec consumer\nuses units_src\nrule out: units_src.|";
        assert_eq!(
            context_at(src, "|"),
            CompletionContext::SpecMember {
                alias: "units_src".to_string()
            }
        );
    }

    #[test]
    fn spec_member_context_while_typing_member() {
        let src = "spec consumer\nuses units_src\nrule out: units_src.ma|";
        assert_eq!(
            context_at(src, "|"),
            CompletionContext::SpecMember {
                alias: "units_src".to_string()
            }
        );
    }

    #[test]
    fn qualified_type_after_dot_in_data_stays_type_annotation() {
        let src = "spec consumer\ndata x: units_src.|";
        assert_eq!(context_at(src, "|"), CompletionContext::TypeAnnotation);
    }

    #[test]
    fn unit_literal_after_unit_command() {
        let ctx = context_at("spec pricing\ndata qty: measure\n  -> unit |", "|");
        assert!(
            matches!(ctx, CompletionContext::UnitLiteral { .. }),
            "got {ctx:?}"
        );
    }
}
