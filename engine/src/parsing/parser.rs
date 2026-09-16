use crate::error::Error;
use crate::limits::ResourceLimits;
use crate::parsing::ast::{try_parse_type_constraint_command, *};
use crate::parsing::lexer::{
    can_be_label, can_be_repository_qualifier_segment, is_boolean_keyword, is_keyword,
    is_math_function, is_spec_body_keyword, token_is_calendar_period_marker,
    token_kind_to_boolean_value, token_kind_to_primitive, Lexer, LexerCheckpoint, Token, TokenKind,
};
use crate::parsing::source::Source;
use indexmap::IndexMap;
use rust_decimal::Decimal;
use std::sync::Arc;

#[derive(Debug)]
struct DeprecatedStandaloneWith {
    path: Reference,
    rhs: WithRhs,
    source_location: Source,
}

fn merge_deprecated_standalone_with(
    data: &mut [LemmaData],
    pending: Vec<DeprecatedStandaloneWith>,
) -> Result<(), Error> {
    for item in pending {
        let alias = item.path.segments[0].clone();
        let relative = Reference {
            segments: item.path.segments[1..].to_vec(),
            name: item.path.name.clone(),
        };
        let import = data.iter_mut().find(|datum| {
            datum.reference.is_local()
                && datum.reference.name == alias
                && matches!(&datum.value, DataValue::Import { .. })
        });
        if import.is_none() {
            return Err(Error::parsing(
                format!("`uses {alias}: …` is required for standalone `with {alias}.…`"),
                item.source_location,
                Some(format!("Add `uses {alias}: <spec_name>` in this spec")),
            ));
        }
        let datum = import.expect("BUG: import row exists after is_none check");
        let DataValue::Import { bindings, .. } = &mut datum.value else {
            unreachable!("BUG: matched Import arm in find predicate");
        };
        bindings.push(UsesBinding {
            path: relative,
            rhs: item.rhs,
            source_location: item.source_location,
            deprecated_standalone_with: true,
        });
    }
    Ok(())
}

#[derive(Debug)]
pub struct ParseResult {
    pub repositories: IndexMap<Arc<LemmaRepository>, Vec<LemmaSpec>>,
    pub expression_count: usize,
}

impl ParseResult {
    /// Specs in parse order: repository groups follow declaration order; specs within each group follow source order.
    #[must_use]
    pub fn flatten_specs(&self) -> Vec<&LemmaSpec> {
        self.repositories
            .values()
            .flat_map(|specs| specs.iter())
            .collect()
    }

    #[must_use]
    pub fn into_flattened_specs(self) -> Vec<LemmaSpec> {
        self.repositories.into_values().flatten().collect()
    }
}

/// Parse a standalone repository qualifier string (trimmed). Requires EOF after
/// the qualifier so callers can reject URL/path injection in install ids.
pub fn parse_repository_qualifier_str(
    input: &str,
) -> Result<crate::parsing::ast::RepositoryQualifier, Error> {
    let limits = ResourceLimits::default();
    let mut parser = Parser::new(
        input.trim(),
        crate::parsing::source::SourceType::Volatile,
        &limits,
    );
    let (qualifier, _) = parser.parse_repository_qualifier()?;
    if !parser.at(&TokenKind::Eof)? {
        let tok = parser.peek()?.clone();
        return Err(parser.error_at_token(
            &tok,
            format!(
                "Unexpected {} after repository qualifier '{}'",
                tok.kind, qualifier.name
            ),
        ));
    }
    Ok(qualifier)
}

pub fn parse(
    content: &str,
    source_type: crate::parsing::source::SourceType,
    limits: &ResourceLimits,
) -> Result<ParseResult, Error> {
    if content.len() > limits.max_source_size_bytes {
        return Err(Error::resource_limit_exceeded(
            "max_source_size_bytes",
            format!(
                "{} bytes ({} MB)",
                limits.max_source_size_bytes,
                limits.max_source_size_bytes / (1024 * 1024)
            ),
            format!(
                "{} bytes ({:.2} MB)",
                content.len(),
                content.len() as f64 / (1024.0 * 1024.0)
            ),
            "Reduce source size or split into multiple specs",
            None,
            None,
            None,
        ));
    }

    let mut parser = Parser::new(content, source_type, limits);
    let repositories = parser.parse_file()?;
    let mut result = ParseResult {
        repositories,
        expression_count: parser.expression_count,
    };
    canonicalize_parse_result(&mut result);
    Ok(result)
}

fn canonicalize_parse_result(result: &mut ParseResult) {
    let old = std::mem::take(&mut result.repositories);
    let mut new_map: IndexMap<Arc<LemmaRepository>, Vec<LemmaSpec>> = IndexMap::new();
    for (repo, mut specs) in old {
        let mut canonical_repo = (*repo).clone();
        canonicalize_repository(&mut canonical_repo);
        for spec in &mut specs {
            canonicalize_lemma_spec(spec);
        }
        new_map
            .entry(Arc::new(canonical_repo))
            .or_default()
            .extend(specs);
    }
    result.repositories = new_map;
}

struct Parser {
    lexer: Lexer,
    source_type: crate::parsing::source::SourceType,
    depth_tracker: DepthTracker,
    expression_count: usize,
    max_expression_count: usize,
    max_spec_name_length: usize,
    max_data_name_length: usize,
    max_rule_name_length: usize,
    last_span: Span,
}

impl Parser {
    fn new(
        content: &str,
        source_type: crate::parsing::source::SourceType,
        limits: &ResourceLimits,
    ) -> Self {
        Parser {
            lexer: Lexer::new(content, &source_type),
            source_type,
            depth_tracker: DepthTracker::with_max_depth(limits.max_expression_depth),
            expression_count: 0,
            max_expression_count: limits.max_expression_count,
            max_spec_name_length: crate::limits::MAX_SPEC_NAME_LENGTH,
            max_data_name_length: crate::limits::MAX_DATA_NAME_LENGTH,
            max_rule_name_length: crate::limits::MAX_RULE_NAME_LENGTH,
            last_span: Span {
                start: 0,
                end: 0,
                line: 1,
                col: 0,
            },
        }
    }

    fn source_type(&self) -> crate::parsing::source::SourceType {
        self.source_type.clone()
    }

    fn peek(&mut self) -> Result<&Token, Error> {
        self.lexer.peek()
    }

    fn next(&mut self) -> Result<Token, Error> {
        let token = self.lexer.next_token()?;
        self.last_span = token.span.clone();
        Ok(token)
    }

    fn at(&mut self, kind: &TokenKind) -> Result<bool, Error> {
        Ok(&self.peek()?.kind == kind)
    }

    fn at_any(&mut self, kinds: &[TokenKind]) -> Result<bool, Error> {
        let current = &self.peek()?.kind;
        Ok(kinds.contains(current))
    }

    fn checkpoint(&self) -> (LexerCheckpoint, usize) {
        (self.lexer.checkpoint(), self.expression_count)
    }

    fn restore(&mut self, checkpoint: (LexerCheckpoint, usize)) {
        self.lexer.restore(checkpoint.0);
        self.expression_count = checkpoint.1;
    }

    fn expect(&mut self, kind: &TokenKind) -> Result<Token, Error> {
        let token = self.next()?;
        if &token.kind == kind {
            Ok(token)
        } else {
            Err(self.error_at_token(&token, format!("Expected {}, found {}", kind, token.kind)))
        }
    }

    fn at_calendar_period_marker(&mut self) -> Result<bool, Error> {
        Ok(token_is_calendar_period_marker(self.peek()?))
    }

    fn expect_calendar_period_marker(&mut self) -> Result<Token, Error> {
        let token = self.next()?;
        if token_is_calendar_period_marker(&token) {
            Ok(token)
        } else {
            Err(self.error_at_token(&token, "Expected 'calendar' (date-period predicate marker)"))
        }
    }

    fn next_calendar_period_marker(&mut self) -> Result<Token, Error> {
        self.expect_calendar_period_marker()
    }

    fn error_at_token(&self, token: &Token, message: impl Into<String>) -> Error {
        Error::parsing(
            message,
            Source::new(self.source_type(), token.span.clone()),
            None::<String>,
        )
    }

    fn error_at_token_with_suggestion(
        &self,
        token: &Token,
        message: impl Into<String>,
        suggestion: impl Into<String>,
    ) -> Error {
        Error::parsing(
            message,
            Source::new(self.source_type(), token.span.clone()),
            Some(suggestion),
        )
    }

    fn parse_spec_ref_trailing_effective(&mut self) -> Result<Option<DateTimeValue>, Error> {
        let mut effective = None;
        if self.at(&TokenKind::NumberLit)? {
            let peeked = self.peek()?;
            if peeked.text.len() == 4 && peeked.text.chars().all(|c| c.is_ascii_digit()) {
                effective = self.try_parse_effective_from()?;
            }
        }
        Ok(effective)
    }

    fn make_source(&self, span: Span) -> Source {
        Source::new(self.source_type(), span)
    }

    fn span_from(&self, start: &Span) -> Span {
        // Create a span from start to the current lexer position.
        // We peek to get the current position.
        Span {
            start: start.start,
            end: start.end.max(start.start),
            line: start.line,
            col: start.col,
        }
    }

    fn span_covering(&self, start: &Span, end: &Span) -> Span {
        Span {
            start: start.start,
            end: end.end,
            line: start.line,
            col: start.col,
        }
    }

    // ========================================================================
    // Top-level: file and spec
    // ========================================================================

    fn parse_file(&mut self) -> Result<IndexMap<Arc<LemmaRepository>, Vec<LemmaSpec>>, Error> {
        let mut map: IndexMap<Arc<LemmaRepository>, Vec<LemmaSpec>> = IndexMap::new();
        let mut current_repo = Arc::new(LemmaRepository::new(None));

        loop {
            if self.at(&TokenKind::Eof)? {
                break;
            }

            if self.at(&TokenKind::Repo)? {
                let repo_token = self.expect(&TokenKind::Repo)?;
                let start_line = repo_token.span.line;
                let (qualifier, _) = self.parse_repository_qualifier()?;
                crate::limits::check_max_length(
                    &qualifier.name,
                    self.max_spec_name_length,
                    "repository name",
                    Some(Source::new(self.source_type(), repo_token.span)),
                )?;
                current_repo = Arc::new(
                    LemmaRepository::new(Some(qualifier.name)).with_start_line(start_line),
                );
                map.entry(Arc::clone(&current_repo)).or_default();
                continue;
            }

            if self.at(&TokenKind::Spec)? {
                let spec = self.parse_spec()?;
                map.entry(Arc::clone(&current_repo)).or_default().push(spec);
                continue;
            }

            let token = self.next()?;
            return Err(self.error_at_token_with_suggestion(
                &token,
                format!(
                    "Expected a top-level `repo` or `spec` declaration, found {}",
                    token.kind
                ),
                "Each Lemma file is a sequence of optional `repo <name>` sections followed by `spec <name>` blocks",
            ));
        }

        Ok(map)
    }

    fn parse_spec(&mut self) -> Result<LemmaSpec, Error> {
        let spec_token = self.expect(&TokenKind::Spec)?;
        let start_line = spec_token.span.line;

        let (name, name_span) = self.parse_spec_name()?;
        crate::limits::check_max_length(
            &name,
            self.max_spec_name_length,
            "spec",
            Some(Source::new(self.source_type(), name_span)),
        )?;

        let effective_from = self.try_parse_effective_from()?;

        let commentary = self.try_parse_commentary()?;

        let mut spec = LemmaSpec::new(name.clone())
            .with_source_type(self.source_type())
            .with_start_line(start_line);
        spec.effective_from = crate::parsing::ast::EffectiveDate::from_option(effective_from);

        if let Some(commentary_text) = commentary {
            spec = spec.set_commentary(commentary_text);
        }

        // First pass: collect type definitions
        // We need to peek and handle type definitions first, but since we consume tokens
        // linearly, we'll collect all items in one pass.
        let mut data = Vec::new();
        let mut rules = Vec::new();
        let mut meta_fields = Vec::new();
        let mut pending_deprecated_with: Vec<DeprecatedStandaloneWith> = Vec::new();

        loop {
            let peek_kind = self.peek()?.kind.clone();
            match peek_kind {
                TokenKind::Data => {
                    let datum = self.parse_data()?;
                    data.push(datum);
                }
                TokenKind::With => {
                    pending_deprecated_with.push(self.parse_deprecated_standalone_with()?);
                }
                TokenKind::Rule => {
                    let rule = self.parse_rule()?;
                    rules.push(rule);
                }
                TokenKind::Meta => {
                    let meta = self.parse_meta()?;
                    meta_fields.push(meta);
                }
                TokenKind::Uses => {
                    let uses_data = self.parse_uses_statement()?;
                    data.push(uses_data);
                }
                TokenKind::Spec | TokenKind::Repo | TokenKind::Eof => break,
                _ => {
                    let token = self.next()?;
                    return Err(self.error_at_token_with_suggestion(
                        &token,
                        format!(
                            "Expected 'data', 'rule', 'meta', 'uses', or a new 'spec', found '{}'",
                            token.text
                        ),
                        "Check the spelling or add the appropriate keyword",
                    ));
                }
            }
        }

        merge_deprecated_standalone_with(&mut data, pending_deprecated_with)?;

        for data in data {
            spec = spec.add_data(data);
        }
        for rule in rules {
            spec = spec.add_rule(rule);
        }
        for meta in meta_fields {
            spec = spec.add_meta_field(meta);
        }

        Ok(spec)
    }

    fn parse_spec_name(&mut self) -> Result<(String, Span), Error> {
        if self.at(&TokenKind::At)? {
            let at_tok = self.next()?;
            return Err(Error::parsing(
                "'@' is not allowed in spec names; it is valid for repository names (`repo @org/name`) and qualifiers (`uses @org/name`)",
                self.make_source(at_tok.span),
                Some(
                    "Write `spec my_spec`, then reference registry specs as `uses alias: @org/repo spec_name` or `data x: alias.TypeName` after importing with `uses`.",
                ),
            ));
        }

        let first = self.next()?;
        if !can_be_label(&first.kind) {
            return Err(self.error_at_token(
                &first,
                format!("Expected a spec name, found {}", first.kind),
            ));
        }
        let mut name = first.text.clone();
        let start_span = first.span.clone();
        let mut end_span = first.span.clone();

        loop {
            if self.at(&TokenKind::Slash)? {
                self.next()?;
                let seg = self.next()?;
                if !can_be_label(&seg.kind) {
                    return Err(self.error_at_token(
                        &seg,
                        format!(
                            "Expected identifier after '/' in spec name, found {}",
                            seg.kind
                        ),
                    ));
                }
                name.push('/');
                name.push_str(&seg.text);
                end_span = seg.span.clone();
            } else if self.at(&TokenKind::Dot)? {
                self.next()?;
                let seg = self.next()?;
                if !can_be_label(&seg.kind) {
                    return Err(self.error_at_token(
                        &seg,
                        format!(
                            "Expected identifier after '.' in spec name, found {}",
                            seg.kind
                        ),
                    ));
                }
                name.push('.');
                name.push_str(&seg.text);
                end_span = seg.span.clone();
            } else if self.at(&TokenKind::Minus)? {
                let minus_span = self.peek()?.span.clone();
                self.next()?;
                let peeked = self.peek()?;
                if !can_be_label(&peeked.kind) {
                    let span = self.span_covering(&start_span, &minus_span);
                    return Err(Error::parsing(
                        "Trailing '-' after spec name",
                        self.make_source(span),
                        None::<String>,
                    ));
                }
                let seg = self.next()?;
                name.push('-');
                name.push_str(&seg.text);
                end_span = seg.span.clone();
            } else {
                break;
            }
        }

        let full_span = self.span_covering(&start_span, &end_span);
        Ok((name, full_span))
    }

    /// Parse a repository qualifier: `[@] identifier ((Slash | Dot | Minus) identifier)*`.
    ///
    /// The `@` prefix, when present, is included in the name string (e.g. `"@org/repo"`).
    /// Slashes, dots and minuses between segments are stitched into the name verbatim
    /// so the qualifier round-trips exactly.
    ///
    /// Used in `repo` declarations and registry qualifiers (`uses`).
    fn parse_repository_qualifier(&mut self) -> Result<(RepositoryQualifier, Span), Error> {
        let has_at = self.at(&TokenKind::At)?;
        let start_span = if has_at {
            let at_tok = self.next()?;
            at_tok.span.clone()
        } else {
            Span {
                start: 0,
                end: 0,
                line: 0,
                col: 0,
            }
        };

        let first = self.next()?;
        if !can_be_repository_qualifier_segment(&first.kind) {
            return Err(self.error_at_token(
                &first,
                format!(
                    "Expected a repository qualifier segment, found {}",
                    first.kind
                ),
            ));
        }
        if !has_at && is_keyword(&first.kind) {
            return Err(self.error_at_token(
                &first,
                format!(
                    "'{}' is a reserved keyword and cannot be used as a repository name",
                    first.text
                ),
            ));
        }
        let start_span = if has_at {
            start_span
        } else {
            first.span.clone()
        };
        let mut name = first.text.clone();

        loop {
            let next_kind = self.peek()?.kind.clone();
            match next_kind {
                TokenKind::Slash => {
                    self.next()?;
                    name.push('/');
                    let seg = self.next()?;
                    if !can_be_repository_qualifier_segment(&seg.kind) {
                        return Err(self.error_at_token(
                            &seg,
                            format!(
                                "Expected identifier after '/' in repository qualifier segment, found {}",
                                seg.kind
                            ),
                        ));
                    }
                    name.push_str(&seg.text);
                }
                TokenKind::Dot => {
                    self.next()?;
                    name.push('.');
                    let seg = self.next()?;
                    if !can_be_repository_qualifier_segment(&seg.kind) {
                        return Err(self.error_at_token(
                            &seg,
                            format!(
                                "Expected identifier after '.' in repository qualifier segment, found {}",
                                seg.kind
                            ),
                        ));
                    }
                    name.push_str(&seg.text);
                }
                TokenKind::Minus => {
                    let minus_text_peek = self.lexer.peek_second()?;
                    if !can_be_repository_qualifier_segment(&minus_text_peek.kind) {
                        break;
                    }
                    self.next()?;
                    name.push('-');
                    let seg = self.next()?;
                    name.push_str(&seg.text);
                }
                _ => break,
            }
        }

        if has_at {
            name.insert(0, '@');
        }

        let full_span = self.span_covering(&start_span, &self.last_span);
        Ok((RepositoryQualifier { name }, full_span))
    }

    /// Parses `[<repository_qualifier>] <spec> [<effective>]`
    pub fn parse_spec_ref_target(&mut self) -> Result<SpecRef, Error> {
        let mut repository = None;
        let mut repository_span = None;

        if self.at(&TokenKind::At)? {
            let (q, span) = self.parse_repository_qualifier()?;
            repository = Some(q);
            repository_span = Some(span);
        } else {
            // Speculative first stitch: if another label follows, the first stitch is a
            // repository qualifier (`uses accounting invoice`). Otherwise rewind and treat
            // the stitch as the spec name (`uses finance/units`).
            let saved_state = self.lexer.clone();
            if let Ok((potential_name, span)) = self.parse_spec_name() {
                if can_be_label(&self.peek()?.kind) {
                    repository = Some(RepositoryQualifier {
                        name: potential_name,
                    });
                    repository_span = Some(span);
                } else {
                    self.lexer = saved_state;
                }
            } else {
                self.lexer = saved_state;
            }
        }

        let (spec_name, spec_name_span) = self.parse_spec_name()?;
        let effective = self.parse_spec_ref_trailing_effective()?;
        let target_span = self.span_covering(&spec_name_span, &self.last_span);

        let has_repository = repository.is_some();
        Ok(SpecRef {
            name: spec_name,
            repository,
            effective,
            repository_span: if has_repository {
                repository_span
            } else {
                None
            },
            target_span: Some(target_span),
        })
    }

    fn try_parse_effective_from(&mut self) -> Result<Option<DateTimeValue>, Error> {
        // effective_from is a date/time token right after the spec name.
        // It's tricky because it looks like a number (e.g. 2026-03-04).
        // In the old grammar it was a special atomic rule.
        // We'll check if the next token is a NumberLit that looks like a year.
        if !self.at(&TokenKind::NumberLit)? {
            return Ok(None);
        }

        let peeked = self.peek()?;
        let peeked_text = peeked.text.clone();
        let peeked_span = peeked.span.clone();

        // Check if it could be a date: 4-digit number followed by -
        if peeked_text.len() == 4 && peeked_text.chars().all(|c| c.is_ascii_digit()) {
            // Collect the full datetime string by consuming tokens
            let mut dt_str = String::new();
            let num_tok = self.next()?; // consume the year number
            dt_str.push_str(&num_tok.text);

            // Try to consume -MM-DD and optional T... parts
            while self.at(&TokenKind::Minus)? {
                self.next()?; // consume -
                dt_str.push('-');
                let part = self.next()?;
                dt_str.push_str(&part.text);
            }

            // Check for T (time part)
            if self.at(&TokenKind::Identifier)? {
                let peeked = self.peek()?;
                if peeked.text.starts_with('T') || peeked.text.starts_with('t') {
                    let time_part = self.next()?;
                    dt_str.push_str(&time_part.text);
                    // Consume any : separated parts
                    while self.at(&TokenKind::Colon)? {
                        self.next()?;
                        dt_str.push(':');
                        let part = self.next()?;
                        dt_str.push_str(&part.text);
                    }
                    // Check for timezone (+ or Z)
                    if self.at(&TokenKind::Plus)? {
                        self.next()?;
                        dt_str.push('+');
                        let tz_part = self.next()?;
                        dt_str.push_str(&tz_part.text);
                        if self.at(&TokenKind::Colon)? {
                            self.next()?;
                            dt_str.push(':');
                            let tz_min = self.next()?;
                            dt_str.push_str(&tz_min.text);
                        }
                    }
                }
            }

            // Try to parse as datetime
            if let Ok(dtv) = dt_str.parse::<DateTimeValue>() {
                return Ok(Some(dtv));
            }

            return Err(Error::parsing(
                format!("Invalid date/time in spec declaration: '{}'", dt_str),
                self.make_source(peeked_span),
                None::<String>,
            ));
        }

        Ok(None)
    }

    fn try_parse_commentary(&mut self) -> Result<Option<String>, Error> {
        if !self.at(&TokenKind::Commentary)? {
            return Ok(None);
        }
        let token = self.next()?;
        let trimmed = token.text.trim().to_string();
        if trimmed.is_empty() {
            Ok(None)
        } else {
            Ok(Some(trimmed))
        }
    }

    // ========================================================================
    // Data parsing
    // ========================================================================

    fn parse_data(&mut self) -> Result<LemmaData, Error> {
        let data_token = self.expect(&TokenKind::Data)?;
        let start_span = data_token.span.clone();

        let reference = self.parse_reference()?;
        for segment in reference
            .segments
            .iter()
            .chain(std::iter::once(&reference.name))
        {
            crate::limits::check_max_length(
                segment,
                self.max_data_name_length,
                "data",
                Some(Source::new(self.source_type(), start_span.clone())),
            )?;
        }

        self.expect(&TokenKind::Colon)?;

        if !reference.segments.is_empty() {
            let tok = self.peek()?.clone();
            return Err(self.error_at_token_with_suggestion(
                &tok,
                "Dotted paths require `uses` with `-> with`; `data` declares types and values on local names only.",
                "Use `uses alias: spec` with `  -> with path.to.slot: <value or reference>`.",
            ));
        }

        let value = self.parse_data_value()?;

        let span = self.span_covering(&start_span, &self.last_span);
        let source = self.make_source(span);

        Ok(LemmaData::new(reference, value, source))
    }

    fn with_rhs_starts_as_literal(kind: &TokenKind) -> bool {
        matches!(
            kind,
            TokenKind::StringLit | TokenKind::NumberLit | TokenKind::Minus | TokenKind::Plus
        ) || is_boolean_keyword(kind)
    }

    fn parse_deprecated_standalone_with(&mut self) -> Result<DeprecatedStandaloneWith, Error> {
        let with_token = self.expect(&TokenKind::With)?;
        let start_span = with_token.span.clone();
        let path = self.parse_reference()?;
        if path.segments.is_empty() {
            return Err(self.error_at_token_with_suggestion(
                &with_token,
                "Standalone `with` must target an imported spec path (`with alias.field: …`).",
                "Use `data name: …` for local slots, or nest under `uses` with `  -> with path: …`.",
            ));
        }
        let (rhs, _) =
            self.parse_assignment_after_key(|parser| parser.parse_with_rhs(), "with", false)?;
        let span = self.span_covering(&start_span, &self.last_span);
        Ok(DeprecatedStandaloneWith {
            path,
            rhs,
            source_location: self.make_source(span),
        })
    }

    fn parse_with_rhs(&mut self) -> Result<WithRhs, Error> {
        let peek_kind = self.peek()?.kind.clone();

        if Self::with_rhs_starts_as_literal(&peek_kind) {
            let value = self.parse_literal_value()?;
            return Ok(WithRhs::Literal(value));
        }

        if can_be_label(&peek_kind) {
            let target = self.parse_reference()?;
            if self.at(&TokenKind::Arrow)? && self.lexer.peek_second()?.kind != TokenKind::With {
                let tok = self.peek()?.clone();
                return Err(self.error_at_token_with_suggestion(
                    &tok,
                    "Constraint chains (`-> ...`) are not allowed on a `uses` binding reference.",
                    "Use `data name: <type> -> ...` for type constraints on local slots.",
                ));
            }
            return Ok(WithRhs::Reference { target });
        }

        let tok = self.peek()?.clone();
        Err(self.error_at_token(
            &tok,
            format!(
                "Expected a reference or literal after `-> with ...:`, found {}",
                tok.kind
            ),
        ))
    }

    fn parse_reference(&mut self) -> Result<Reference, Error> {
        let mut segments = Vec::new();

        let first = self.next()?;
        // Keywords cannot be used as names
        if is_keyword(&first.kind) {
            return Err(self.error_at_token_with_suggestion(
                &first,
                format!(
                    "'{}' is a reserved keyword and cannot be used as a name",
                    first.text
                ),
                "Choose a different name that is not a reserved keyword",
            ));
        }

        if !can_be_label(&first.kind) {
            return Err(self.error_at_token(
                &first,
                format!("Expected an identifier, found {}", first.kind),
            ));
        }

        segments.push(first.text.clone());

        // Consume . separated segments
        while self.at(&TokenKind::Dot)? {
            self.next()?; // consume .
            let seg = self.next()?;
            if !can_be_label(&seg.kind) {
                return Err(self.error_at_token(
                    &seg,
                    format!("Expected an identifier after '.', found {}", seg.kind),
                ));
            }
            segments.push(seg.text.clone());
        }

        Ok(Reference::from_path(segments))
    }

    fn parse_data_value(&mut self) -> Result<DataValue, Error> {
        if self.at(&TokenKind::Spec)? {
            let token = self.next()?;
            return Err(self.error_at_token_with_suggestion(
                &token,
                "Cannot import a spec with `data`; use `uses`",
                "Use `uses <spec_name>` or `uses <alias>: <spec_name>`",
            ));
        }

        let peek_kind = self.peek()?.kind.clone();

        if token_kind_to_primitive(&peek_kind).is_some() || can_be_label(&peek_kind) {
            let (base, constraints) = self.parse_type_arrow_chain()?;
            return Ok(DataValue::Definition {
                base: Some(base),
                constraints,
                value: None,
            });
        }

        // Otherwise, it's a literal value
        let value = self.parse_literal_value()?;
        Ok(DataValue::Definition {
            base: None,
            constraints: None,
            value: Some(value),
        })
    }

    /// Parse a single `uses` item: `[alias ':']` then [`Self::parse_spec_ref_target`] (optional
    /// repository qualifier, spec name, optional effective date pin).
    fn parse_uses_item(&mut self, start_span: &Span) -> Result<LemmaData, Error> {
        let explicit_alias = if can_be_label(&self.peek()?.kind)
            && self.lexer.peek_second()?.kind == TokenKind::Colon
        {
            let alias_tok = self.next()?;
            self.expect(&TokenKind::Colon)?;
            Some(alias_tok)
        } else {
            None
        };

        let spec_ref = self.parse_spec_ref_target()?;

        let spec_name_source = spec_ref
            .target_span
            .as_ref()
            .map(|sp| Source::new(self.source_type(), sp.clone()));

        crate::limits::check_max_length(
            &spec_ref.name,
            self.max_spec_name_length,
            "spec",
            spec_name_source.clone(),
        )?;

        let alias = if let Some(ref alias_tok) = explicit_alias {
            crate::limits::check_max_length(
                &alias_tok.text,
                self.max_data_name_length,
                "data",
                Some(Source::new(self.source_type(), alias_tok.span.clone())),
            )?;
            alias_tok.text.clone()
        } else {
            let implicit = spec_ref.name.clone();
            crate::limits::check_max_length(
                &implicit,
                self.max_data_name_length,
                "data",
                spec_name_source,
            )?;
            implicit
        };

        let mut bindings = Vec::new();
        while self.at(&TokenKind::Arrow)? {
            if self.lexer.peek_second()?.kind != TokenKind::With {
                let tok = self.peek()?.clone();
                return Err(self.error_at_token_with_suggestion(
                    &tok,
                    "Expected `with` after `->` in a `uses` block.",
                    "Write `  -> with path.to.slot: <value or reference>` under the `uses` line.",
                ));
            }
            self.next()?; // ->
            let with_token = self.expect(&TokenKind::With)?;
            let binding_start_span = with_token.span.clone();

            let path = self.parse_reference()?;
            for segment in path.segments.iter().chain(std::iter::once(&path.name)) {
                crate::limits::check_max_length(
                    segment,
                    self.max_data_name_length,
                    "uses binding",
                    Some(Source::new(self.source_type(), binding_start_span.clone())),
                )?;
            }
            if path
                .segments
                .first()
                .is_some_and(|segment| segment == &alias)
            {
                return Err(self.error_at_token_with_suggestion(
                    &with_token,
                    format!(
                        "Binding path must be relative to the imported spec, not prefixed with import alias `{alias}`."
                    ),
                    format!(
                        "Use `-> with {}: …` without the `{alias}.` prefix.",
                        path.name
                    ),
                ));
            }

            let (rhs, _) =
                self.parse_assignment_after_key(|parser| parser.parse_with_rhs(), "with", false)?;

            let binding_span = self.span_covering(&binding_start_span, &self.last_span);
            bindings.push(crate::parsing::ast::UsesBinding {
                path,
                rhs,
                source_location: self.make_source(binding_span),
                deprecated_standalone_with: false,
            });
        }

        let span = self.span_covering(start_span, &self.last_span);
        Ok(LemmaData::new(
            Reference::local(alias),
            DataValue::Import { spec_ref, bindings },
            self.make_source(span),
        ))
    }

    fn parse_uses_statement(&mut self) -> Result<LemmaData, Error> {
        let uses_token = self.expect(&TokenKind::Uses)?;
        let start_span = uses_token.span.clone();
        self.parse_uses_item(&start_span)
    }

    // ========================================================================
    // Rule parsing
    // ========================================================================

    fn parse_rule(&mut self) -> Result<LemmaRule, Error> {
        let rule_token = self.expect(&TokenKind::Rule)?;
        let start_span = rule_token.span.clone();

        let name_tok = self.next()?;
        if is_keyword(&name_tok.kind) {
            return Err(self.error_at_token_with_suggestion(
                &name_tok,
                format!(
                    "'{}' is a reserved keyword and cannot be used as a rule name",
                    name_tok.text
                ),
                "Choose a different name that is not a reserved keyword",
            ));
        }
        if !can_be_label(&name_tok.kind) {
            return Err(self.error_at_token(
                &name_tok,
                format!("Expected a rule name, found {}", name_tok.kind),
            ));
        }
        let rule_name = name_tok.text.clone();
        crate::limits::check_max_length(
            &rule_name,
            self.max_rule_name_length,
            "rule",
            Some(Source::new(self.source_type(), name_tok.span.clone())),
        )?;

        self.expect(&TokenKind::Colon)?;

        // Parse the base expression or veto result (`veto "msg"`). `veto is …` is an expression.
        let expression = if self.at(&TokenKind::Veto)? && !self.at_bare_veto_followed_by_is()? {
            self.parse_veto_expression()?
        } else {
            self.parse_expression()?
        };

        // Parse unless clauses
        let mut unless_clauses = Vec::new();
        while self.at(&TokenKind::Unless)? {
            unless_clauses.push(self.parse_unless_clause()?);
        }

        let end_span = if let Some(last_unless) = unless_clauses.last() {
            last_unless.source_location.span.clone()
        } else if let Some(ref loc) = expression.source_location {
            loc.span.clone()
        } else {
            start_span.clone()
        };

        let span = self.span_covering(&start_span, &end_span);
        Ok(LemmaRule {
            name: rule_name,
            expression,
            unless_clauses,
            source_location: self.make_source(span),
        })
    }

    fn parse_veto_expression(&mut self) -> Result<Expression, Error> {
        let veto_tok = self.expect(&TokenKind::Veto)?;
        let start_span = veto_tok.span.clone();

        let message = if self.at(&TokenKind::StringLit)? {
            let str_tok = self.next()?;
            Some(str_tok.text)
        } else {
            None
        };

        let span = self.span_from(&start_span);
        self.new_expression(
            ExpressionKind::Veto(VetoExpression { message }),
            self.make_source(span),
        )
    }

    fn parse_unless_clause(&mut self) -> Result<UnlessClause, Error> {
        let unless_tok = self.expect(&TokenKind::Unless)?;
        let start_span = unless_tok.span.clone();

        let condition = self.parse_expression()?;

        self.expect(&TokenKind::Then)?;

        let result = if self.at(&TokenKind::Veto)? {
            self.parse_veto_expression()?
        } else {
            self.parse_expression()?
        };

        let end_span = result
            .source_location
            .as_ref()
            .map(|s| s.span.clone())
            .unwrap_or_else(|| start_span.clone());
        let span = self.span_covering(&start_span, &end_span);

        Ok(UnlessClause {
            condition,
            result,
            source_location: self.make_source(span),
        })
    }

    fn parse_leaf_parent_type(&mut self) -> Result<ParentType, Error> {
        let name_tok = self.next()?;
        self.parse_leaf_parent_type_from_first_token(name_tok)
    }

    fn parse_leaf_parent_type_from_first_token(
        &mut self,
        name_tok: Token,
    ) -> Result<ParentType, Error> {
        if let Some(kind) = token_kind_to_primitive(&name_tok.kind) {
            Ok(ParentType::Primitive { primitive: kind })
        } else if can_be_label(&name_tok.kind) {
            Ok(ParentType::Custom {
                name: name_tok.text.clone(),
            })
        } else {
            Err(self.error_at_token(
                &name_tok,
                format!("Expected a type name, found {}", name_tok.kind),
            ))
        }
    }

    /// Parse a type arrow chain: [`ParentType`] followed by `(-> command)*`.
    fn parse_type_arrow_chain(&mut self) -> Result<(ParentType, Option<Vec<Constraint>>), Error> {
        let peek_kind = self.peek()?.kind.clone();
        let base = if let Some(primitive) = token_kind_to_primitive(&peek_kind) {
            let _ = self.next()?;
            if self.at(&TokenKind::Dot)? {
                let dot_tok = self.peek()?.clone();
                return Err(self.error_at_token_with_suggestion(
                    &dot_tok,
                    "A primitive type cannot be the left segment of a qualified parent path",
                    "Use `data name: alias.typename` where `alias` is the `uses` import name and `typename` is the parent type.",
                ));
            }
            ParentType::Primitive { primitive }
        } else {
            let (name, _) = self.parse_spec_name()?;
            if self.at(&TokenKind::Dot)? {
                // Trailing `.` + primitive keyword (e.g. `finance.text`) — label stitch stopped.
                self.next()?;
                let inner = self.parse_leaf_parent_type()?;
                ParentType::Qualified {
                    spec_alias: name,
                    inner: Box::new(inner),
                }
            } else if let Some((alias, type_name)) = name.rsplit_once('.') {
                ParentType::Qualified {
                    spec_alias: alias.to_string(),
                    inner: Box::new(ParentType::Custom {
                        name: type_name.to_string(),
                    }),
                }
            } else {
                ParentType::Custom { name }
            }
        };

        let base = if self.at(&TokenKind::Identifier)? && self.peek()?.text == "range" {
            self.next()?;
            ParentType::Ranged {
                inner: Box::new(base),
            }
        } else {
            base
        };

        let constraints = self.parse_trailing_constraints()?;

        Ok((base, constraints))
    }

    fn parse_trailing_constraints(&mut self) -> Result<Option<Vec<Constraint>>, Error> {
        let mut commands = Vec::new();
        while self.at(&TokenKind::Arrow)? {
            let arrow_token = self.next()?;
            let constraint_start_span = arrow_token.span.clone();
            let (cmd, cmd_args, deprecated_without_colon) = self.parse_constraint_command()?;
            let constraint_span = self.span_covering(&constraint_start_span, &self.last_span);
            commands.push(Constraint {
                command: cmd,
                args: cmd_args,
                source_location: self.make_source(constraint_span),
                deprecated_without_colon,
            });
        }
        let constraints = if commands.is_empty() {
            None
        } else {
            Some(commands)
        };
        Ok(constraints)
    }

    fn parse_constraint_command(
        &mut self,
    ) -> Result<(TypeConstraintCommand, Vec<CommandArg>, bool), Error> {
        let name_tok = self.next()?;
        if !can_be_label(&name_tok.kind) {
            return Err(self.error_at_token(
                &name_tok,
                format!("Expected a command name, found {}", name_tok.kind),
            ));
        }
        let cmd = try_parse_type_constraint_command(&name_tok.text).ok_or_else(|| {
            self.error_at_token(
                &name_tok,
                format!(
                    "Unknown constraint command '{}'. Valid commands: help, suggest, fill, unit, trait, minimum, maximum, decimals, option, options, length",
                    name_tok.text
                ),
            )
        })?;

        match cmd.continuation_shape() {
            ContinuationShape::Assignment => {
                let (args, deprecated_without_colon) = self.parse_unit_as_assignment()?;
                Ok((cmd, args, deprecated_without_colon))
            }
            ContinuationShape::SpaceSeparated => {
                Ok((cmd, self.parse_generic_command_args()?, false))
            }
        }
    }

    /// Shared spine for assignment continuations (`with`, `unit`): `key: value` or deprecated space.
    fn parse_assignment_after_key<T>(
        &mut self,
        parse_value: impl FnOnce(&mut Self) -> Result<T, Error>,
        assignment_context: &str,
        allow_deprecated_without_colon: bool,
    ) -> Result<(T, bool), Error> {
        if self.at(&TokenKind::Colon)? {
            self.next()?;
            let value = parse_value(self)?;
            return Ok((value, false));
        }
        if allow_deprecated_without_colon {
            let value = parse_value(self)?;
            return Ok((value, true));
        }
        let tok = self.peek()?.clone();
        Err(self.error_at_token_with_suggestion(
            &tok,
            format!("Expected `:` after `{assignment_context}` key."),
            format!("Use `-> {assignment_context} <key>: <value>`."),
        ))
    }

    fn parse_unit_as_assignment(&mut self) -> Result<(Vec<CommandArg>, bool), Error> {
        if self.at_command_terminator()? {
            return Ok((Vec::new(), false));
        }

        let peek_kind = self.peek()?.kind.clone();
        if !can_be_label(&peek_kind) {
            return Ok((Vec::new(), false));
        }

        let unit_name_tok = self.next()?;
        let unit_name_arg = CommandArg::Label(unit_name_tok.text.clone());

        let (unit_arg, deprecated_without_colon) =
            self.parse_assignment_after_key(|parser| parser.parse_unit_payload(), "unit", true)?;

        Ok((
            vec![unit_name_arg, CommandArg::UnitExpr(unit_arg)],
            deprecated_without_colon,
        ))
    }

    /// Value side of `-> unit <name>: <payload>` (factor, compound, or prefix + compound).
    fn parse_unit_payload(&mut self) -> Result<UnitArg, Error> {
        if self.at_command_terminator()? {
            let tok = self.peek()?.clone();
            return Err(self.error_at_token_with_suggestion(
                &tok,
                "Expected a unit conversion factor or compound unit expression after `:`.",
                "Use `-> unit <name>: <factor>` or `-> unit <name>: <compound>` (e.g. `unit eur: 1.00`, `unit eur_per_hour: eur/hour`).",
            ));
        }

        let numeric_prefix: Option<Decimal> = if self.at(&TokenKind::NumberLit)? {
            let num_tok = self.next()?;
            Some(parse_decimal_string(&num_tok.text, &num_tok.span, self)?)
        } else {
            None
        };

        let peek_kind_after_prefix = self.peek()?.kind.clone();
        let has_compound_expr =
            can_be_label(&peek_kind_after_prefix) && !self.at_command_terminator()?;

        if has_compound_expr {
            let factors = self.parse_unit_factors()?;
            let prefix = numeric_prefix.unwrap_or(Decimal::ONE);
            Ok(UnitArg::Expr(prefix, factors))
        } else if let Some(factor) = numeric_prefix {
            Ok(UnitArg::Factor(factor))
        } else {
            let tok = self.peek()?.clone();
            Err(self.error_at_token_with_suggestion(
                &tok,
                "Expected a unit conversion factor or compound unit expression after `:`.",
                "Use `-> unit <name>: <factor>` or `-> unit <name>: <compound>` (e.g. `unit eur: 1.00`, `unit eur_per_hour: eur/hour`).",
            ))
        }
    }

    /// Parse arguments for a generic (non-unit) constraint command.
    fn parse_generic_command_args(&mut self) -> Result<Vec<CommandArg>, Error> {
        let mut args = Vec::new();
        loop {
            if self.at(&TokenKind::Arrow)?
                || self.at(&TokenKind::Eof)?
                || is_spec_body_keyword(&self.peek()?.kind)
                || self.at(&TokenKind::Spec)?
            {
                break;
            }

            let peek_kind = self.peek()?.kind.clone();
            match peek_kind {
                TokenKind::NumberLit
                | TokenKind::Minus
                | TokenKind::Plus
                | TokenKind::StringLit => {
                    let value = self.parse_literal_value()?;
                    args.push(CommandArg::Literal(value));
                }
                ref k if is_boolean_keyword(k) => {
                    let value = self.parse_literal_value()?;
                    args.push(CommandArg::Literal(value));
                }
                ref k if can_be_label(k) => {
                    let tok = self.next()?;
                    args.push(CommandArg::Label(tok.text));
                }
                _ => break,
            }
        }
        Ok(args)
    }

    fn parse_scalar_literal_value(&mut self) -> Result<Value, Error> {
        self.parse_scalar_literal_value_with_context("value (number, text, boolean, date, etc.)")
    }

    fn parse_scalar_literal_value_with_context(&mut self, expected: &str) -> Result<Value, Error> {
        let peeked = self.peek()?;
        match &peeked.kind {
            TokenKind::StringLit => {
                let tok = self.next()?;
                Ok(Value::Text(tok.text))
            }
            k if is_boolean_keyword(k) => {
                let tok = self.next()?;
                Ok(Value::Boolean(token_kind_to_boolean_value(&tok.kind)))
            }
            TokenKind::NumberLit => self.parse_number_literal(),
            TokenKind::Minus | TokenKind::Plus => self.parse_signed_number_literal(),
            _ => {
                let tok = self.next()?;
                Err(self
                    .error_at_token(&tok, format!("Expected a {expected}, found '{}'", tok.text)))
            }
        }
    }

    /// Returns true when the current token ends the current command argument list.
    fn at_command_terminator(&mut self) -> Result<bool, Error> {
        if self.at(&TokenKind::Arrow)? || self.at(&TokenKind::Eof)? || self.at(&TokenKind::Spec)? {
            return Ok(true);
        }
        Ok(is_spec_body_keyword(&self.peek()?.kind))
    }

    /// Parse a sequence of `<measure_ref>[^[-]<integer>]` terms joined by `*` or `/`.
    ///
    /// `*` is explicit multiplication and resets to numerator mode.
    /// `/` switches to denominator mode for all subsequent factors until the next `*`.
    /// Exponents in denominator mode are negated.
    /// An explicit `^<integer>` overrides the default (±1), still relative to the current mode.
    ///
    /// Space has no meaning in Lemma — `kg * m / s^2` and `kg*m/s^2` are identical.
    /// Space-adjacent labels without an intervening `*` or `/` end the expression;
    /// the bare label belongs to the next syntactic element.
    ///
    /// Examples:
    /// - `meter/second`            → `[{meter, +1}, {second, -1}]`
    /// - `meter/second^2`          → `[{meter, +1}, {second, -2}]`
    /// - `kg * meter / second^2`   → `[{kg, +1}, {meter, +1}, {second, -2}]`
    /// - `meter / second * kg`     → `[{meter, +1}, {second, -1}, {kg, +1}]`
    fn parse_unit_factors(&mut self) -> Result<Vec<UnitFactor>, Error> {
        let mut factors: Vec<UnitFactor> = Vec::new();
        let mut denominator_mode = false;
        // Tracks whether the loop is sitting immediately after an operator (* or /).
        // On the first iteration there is an implicit "we just started", so we allow
        // the first label without a preceding operator.
        let mut operator_just_consumed = true;

        loop {
            if self.at_command_terminator()? {
                if !operator_just_consumed {
                    break;
                }
                // An operator was consumed but no label followed — that is a parse error
                // only if we already emitted at least one factor (dangling operator).
                // If factors is empty the caller will handle the missing expression.
                break;
            }

            // `*` — explicit multiplication; reset to numerator mode.
            if self.at(&TokenKind::Star)? {
                if operator_just_consumed && !factors.is_empty() {
                    let bad_tok = self.next()?;
                    return Err(self.error_at_token(
                        &bad_tok,
                        "Unexpected '*' in unit expression: two consecutive operators".to_string(),
                    ));
                }
                self.next()?;
                denominator_mode = false;
                operator_just_consumed = true;
                continue;
            }

            // `/` — switch to denominator mode.
            if self.at(&TokenKind::Slash)? {
                if operator_just_consumed && !factors.is_empty() {
                    let bad_tok = self.next()?;
                    return Err(self.error_at_token(
                        &bad_tok,
                        "Unexpected '/' in unit expression: two consecutive operators".to_string(),
                    ));
                }
                self.next()?;
                denominator_mode = true;
                operator_just_consumed = true;
                continue;
            }

            // A label is a measure reference.
            let peek_kind = self.peek()?.kind.clone();
            if !can_be_label(&peek_kind) {
                break;
            }

            // A label without a preceding operator ends the expression.
            // The first factor is permitted without an operator (operator_just_consumed starts true).
            if !operator_just_consumed {
                break;
            }
            operator_just_consumed = false;

            let (measure_ref, _end_span) = self.parse_unit_path()?;

            // Optional exponent: `^` followed by an optional `-` and an integer.
            let explicit_exp: Option<i32> = if self.at(&TokenKind::Caret)? {
                self.next()?; // consume `^`

                let negative = if self.at(&TokenKind::Minus)? {
                    self.next()?; // consume `-`
                    true
                } else {
                    false
                };

                if !self.at(&TokenKind::NumberLit)? {
                    let bad_tok = self.next()?;
                    return Err(self.error_at_token(
                        &bad_tok,
                        format!(
                            "Expected an integer exponent after '^' in unit expression, found {}",
                            bad_tok.kind
                        ),
                    ));
                }

                let exp_tok = self.next()?;
                let raw: i32 = exp_tok.text.parse::<i32>().map_err(|_| {
                    self.error_at_token(
                        &exp_tok,
                        format!(
                            "Exponent '{}' is not a valid integer in unit expression",
                            exp_tok.text
                        ),
                    )
                })?;

                if raw == 0 {
                    return Err(self.error_at_token(
                        &exp_tok,
                        "Exponent cannot be zero in a unit expression".to_string(),
                    ));
                }

                Some(if negative { -raw } else { raw })
            } else {
                None
            };

            // Apply denominator mode: negate the exponent (or its default) when in denominator.
            let final_exp = match (explicit_exp, denominator_mode) {
                (Some(exponent), true) => -exponent,
                (Some(exponent), false) => exponent,
                (None, true) => -1,
                (None, false) => 1,
            };

            factors.push(UnitFactor {
                measure_ref,
                exp: final_exp,
            });
        }

        Ok(factors)
    }

    // ========================================================================
    // Meta parsing
    // ========================================================================

    fn parse_meta(&mut self) -> Result<MetaField, Error> {
        let meta_tok = self.expect(&TokenKind::Meta)?;
        let start_span = meta_tok.span.clone();

        let key_tok = self.next()?;
        let key = key_tok.text.clone();

        self.expect(&TokenKind::Colon)?;

        let value = self.parse_meta_literal_value()?;

        let span = self.span_covering(&start_span, &self.last_span);

        Ok(MetaField {
            key,
            value,
            source_location: self.make_source(span),
        })
    }

    // ========================================================================
    // Literal value parsing
    // ========================================================================

    fn parse_meta_literal_value(&mut self) -> Result<Value, Error> {
        if self.at(&TokenKind::Ellipsis)? {
            let tok = self.peek()?.clone();
            return Err(self.error_at_token(&tok, "Expected a meta literal, found a range"));
        }
        self.parse_scalar_literal_value_with_context("meta literal")
    }

    fn parse_literal_value(&mut self) -> Result<Value, Error> {
        let left = self.parse_scalar_literal_value()?;
        if self.at(&TokenKind::Ellipsis)? {
            self.next()?;
            let right = self.parse_scalar_literal_value()?;
            Ok(Value::Range(Box::new(left), Box::new(right)))
        } else {
            Ok(left)
        }
    }

    fn parse_signed_number_literal(&mut self) -> Result<Value, Error> {
        let sign_tok = self.next()?;
        let sign_span = sign_tok.span.clone();
        let is_negative = sign_tok.kind == TokenKind::Minus;

        if !self.at(&TokenKind::NumberLit)? {
            let tok = self.peek()?.clone();
            return Err(self.error_at_token(
                &tok,
                format!(
                    "Expected a number after '{}', found '{}'",
                    sign_tok.text, tok.text
                ),
            ));
        }

        let value = self.parse_number_literal()?;
        if !is_negative {
            return Ok(value);
        }
        match try_negate_numeric_literal(value) {
            Ok(negated) => Ok(negated),
            Err(other) => Err(Error::parsing(
                format!("Cannot negate this value: {}", other),
                self.make_source(sign_span),
                None::<String>,
            )),
        }
    }

    fn parse_number_literal(&mut self) -> Result<Value, Error> {
        let num_tok = self.next()?;
        let num_text = &num_tok.text;
        let num_span = num_tok.span.clone();

        // Check if followed by - which could make it a date (YYYY-MM-DD)
        if num_text.len() == 4
            && num_text.chars().all(|c| c.is_ascii_digit())
            && self.at(&TokenKind::Minus)?
        {
            return self.parse_date_literal(num_text.clone(), num_span);
        }

        // Check what follows the number
        let peeked = self.peek()?;

        // Number followed by : could be a time literal (HH:MM:SS)
        if num_text.len() == 2
            && num_text.chars().all(|c| c.is_ascii_digit())
            && peeked.kind == TokenKind::Colon
        {
            // Only if we're in a data value context... this is ambiguous.
            // Time literals look like: 14:30:00 or 14:30
            // But we might also have "rule x: expr" where : is assignment.
            // The grammar handles this at the grammar level. For us,
            // we need to check if the context is right.
            // Let's try to parse as time if the following pattern matches.
            return self.try_parse_time_literal(num_text.clone(), num_span);
        }

        // Check for %% (permille) - must be before %
        if peeked.kind == TokenKind::PercentPercent {
            let pp_tok = self.next()?;
            // Check it's not followed by a digit
            if let Ok(next_peek) = self.peek() {
                if next_peek.kind == TokenKind::NumberLit {
                    return Err(self.error_at_token(
                        &pp_tok,
                        "Permille literal cannot be followed by a digit",
                    ));
                }
            }
            let decimal = parse_decimal_string(num_text, &num_span, self)?;
            return Ok(Value::NumberWithUnit(decimal, "permille".to_string()));
        }

        // Check for % (percent)
        if peeked.kind == TokenKind::Percent {
            let pct_tok = self.next()?;
            // Check it's not followed by a digit or another %
            if let Ok(next_peek) = self.peek() {
                if next_peek.kind == TokenKind::NumberLit || next_peek.kind == TokenKind::Percent {
                    return Err(self.error_at_token(
                        &pct_tok,
                        "Percent literal cannot be followed by a digit",
                    ));
                }
            }
            let decimal = parse_decimal_string(num_text, &num_span, self)?;
            return Ok(Value::NumberWithUnit(decimal, "percent".to_string()));
        }

        // Check for "permille" keyword
        if peeked.kind == TokenKind::Permille {
            self.next()?; // consume "permille"
            let decimal = parse_decimal_string(num_text, &num_span, self)?;
            return Ok(Value::NumberWithUnit(decimal, "permille".to_string()));
        }

        if can_be_label(&peeked.kind) {
            let (unit_path, _end_span) = self.parse_unit_path()?;
            let decimal = parse_decimal_string(num_text, &num_span, self)?;
            return Ok(Value::NumberWithUnit(decimal, unit_path));
        }

        // Plain number
        let decimal = parse_decimal_string(num_text, &num_span, self)?;
        Ok(Value::Number(decimal))
    }

    /// One or more labels separated by `.` (bare unit or qualified unit path).
    fn parse_unit_path(&mut self) -> Result<(String, Span), Error> {
        let first = self.next()?;
        if !can_be_label(&first.kind) {
            return Err(self.error_at_token(
                &first,
                format!("Expected a unit name, found {}", first.kind),
            ));
        }
        let mut path = first.text.clone();
        let mut end_span = first.span.clone();
        while self.at(&TokenKind::Dot)? {
            self.next()?;
            let seg = self.next()?;
            if !can_be_label(&seg.kind) {
                return Err(self.error_at_token(
                    &seg,
                    format!("Expected a unit path segment after '.', found {}", seg.kind),
                ));
            }
            path.push('.');
            path.push_str(&seg.text);
            end_span = seg.span.clone();
        }
        Ok((path, end_span))
    }

    fn parse_date_literal(&mut self, year_text: String, start_span: Span) -> Result<Value, Error> {
        let mut dt_str = year_text;

        // Consume -MM
        self.expect(&TokenKind::Minus)?;
        dt_str.push('-');
        let month_tok = self.expect(&TokenKind::NumberLit)?;
        dt_str.push_str(&month_tok.text);

        // Consume -DD
        self.expect(&TokenKind::Minus)?;
        dt_str.push('-');
        let day_tok = self.expect(&TokenKind::NumberLit)?;
        dt_str.push_str(&day_tok.text);

        // Check for T (time component)
        if self.at(&TokenKind::Identifier)? {
            let peeked = self.peek()?;
            if peeked.text.len() >= 2
                && (peeked.text.starts_with('T') || peeked.text.starts_with('t'))
            {
                // The lexer may have tokenized T14 as a single identifier
                let t_tok = self.next()?;
                dt_str.push_str(&t_tok.text);

                // Consume :MM
                if self.at(&TokenKind::Colon)? {
                    self.next()?;
                    dt_str.push(':');
                    let min_tok = self.next()?;
                    dt_str.push_str(&min_tok.text);

                    // Consume :SS and optional fractional second
                    if self.at(&TokenKind::Colon)? {
                        self.next()?;
                        dt_str.push(':');
                        let sec_tok = self.next()?;
                        dt_str.push_str(&sec_tok.text);

                        // Check for fractional second .NNNNNN
                        if self.at(&TokenKind::Dot)? {
                            self.next()?;
                            dt_str.push('.');
                            let frac_tok = self.expect(&TokenKind::NumberLit)?;
                            dt_str.push_str(&frac_tok.text);
                        }
                    }
                }

                // Check for timezone
                self.try_consume_timezone(&mut dt_str)?;
            }
        }

        if let Ok(dtv) = dt_str.parse::<crate::literals::DateTimeValue>() {
            return Ok(Value::Date(dtv));
        }

        Err(Error::parsing(
            format!("Invalid date/time format: '{}'", dt_str),
            self.make_source(start_span),
            None::<String>,
        ))
    }

    fn try_consume_timezone(&mut self, dt_str: &mut String) -> Result<(), Error> {
        // Z timezone
        if self.at(&TokenKind::Identifier)? {
            let peeked = self.peek()?;
            if (peeked.text == "Z" || peeked.text == "z") && peeked.span.start == self.last_span.end
            {
                let z_tok = self.next()?;
                dt_str.push_str(&z_tok.text);
                return Ok(());
            }
        }

        // +HH:MM or -HH:MM, only when attached directly to the preceding token.
        if self.at(&TokenKind::Plus)? || self.at(&TokenKind::Minus)? {
            let mut lookahead = self.lexer.clone();
            let sign_tok = lookahead.next_token()?;
            let hour_tok = lookahead.next_token()?;
            let colon_tok = lookahead.next_token()?;
            let minute_tok = lookahead.next_token()?;

            let attached = sign_tok.span.start == self.last_span.end;
            let is_timezone_shape = hour_tok.kind == TokenKind::NumberLit
                && colon_tok.kind == TokenKind::Colon
                && minute_tok.kind == TokenKind::NumberLit;

            if attached && is_timezone_shape {
                let sign_tok = self.next()?;
                dt_str.push_str(&sign_tok.text);
                let hour_tok = self.expect(&TokenKind::NumberLit)?;
                dt_str.push_str(&hour_tok.text);
                self.expect(&TokenKind::Colon)?;
                dt_str.push(':');
                let min_tok = self.expect(&TokenKind::NumberLit)?;
                dt_str.push_str(&min_tok.text);
            }
        }

        Ok(())
    }

    fn try_parse_time_literal(
        &mut self,
        hour_text: String,
        start_span: Span,
    ) -> Result<Value, Error> {
        let mut time_str = hour_text;

        // Consume :MM
        self.expect(&TokenKind::Colon)?;
        time_str.push(':');
        let min_tok = self.expect(&TokenKind::NumberLit)?;
        time_str.push_str(&min_tok.text);

        // Optional :SS
        if self.at(&TokenKind::Colon)? {
            self.next()?;
            time_str.push(':');
            let sec_tok = self.expect(&TokenKind::NumberLit)?;
            time_str.push_str(&sec_tok.text);

            // Optional fractional second .NNNNNN
            if self.at(&TokenKind::Dot)? {
                self.next()?;
                time_str.push('.');
                let frac_tok = self.expect(&TokenKind::NumberLit)?;
                time_str.push_str(&frac_tok.text);
            }
        }

        // Try timezone
        self.try_consume_timezone(&mut time_str)?;

        if let Ok(t) = time_str.parse::<TimeValue>() {
            return Ok(Value::Time(TimeValue {
                hour: t.hour,
                minute: t.minute,
                second: t.second,
                microsecond: t.microsecond,
                timezone: t.timezone,
            }));
        }

        Err(Error::parsing(
            format!("Invalid time format: '{}'", time_str),
            self.make_source(start_span),
            None::<String>,
        ))
    }

    // ========================================================================
    // Expression parsing (Pratt parser / precedence climbing)
    // ========================================================================

    fn new_expression(
        &mut self,
        kind: ExpressionKind,
        source: Source,
    ) -> Result<Expression, Error> {
        self.expression_count += 1;
        if self.expression_count > self.max_expression_count {
            return Err(Error::resource_limit_exceeded(
                "max_expression_count",
                self.max_expression_count.to_string(),
                self.expression_count.to_string(),
                "Split logic into multiple rules to reduce expression count",
                Some(source),
                None,
                None,
            ));
        }
        Ok(Expression::new(kind, source))
    }

    fn check_depth(&mut self) -> Result<(), Error> {
        if let Err(actual) = self.depth_tracker.push_depth() {
            let span = self.peek()?.span.clone();
            self.depth_tracker.pop_depth();
            return Err(Error::resource_limit_exceeded(
                "max_expression_depth",
                self.depth_tracker.max_depth().to_string(),
                actual.to_string(),
                "Simplify nested expressions or break into separate rules",
                Some(self.make_source(span)),
                None,
                None,
            ));
        }
        Ok(())
    }

    fn parse_expression(&mut self) -> Result<Expression, Error> {
        self.check_depth()?;
        let result = self.parse_and_expression();
        self.depth_tracker.pop_depth();
        result
    }

    fn parse_and_expression(&mut self) -> Result<Expression, Error> {
        let start_span = self.peek()?.span.clone();
        let mut left = self.parse_and_operand()?;

        while self.at(&TokenKind::And)? {
            self.next()?; // consume 'and'
            let right = self.parse_and_operand()?;
            let span = self.span_covering(
                &start_span,
                &right
                    .source_location
                    .as_ref()
                    .map(|s| s.span.clone())
                    .unwrap_or_else(|| start_span.clone()),
            );
            left = self.new_expression(
                ExpressionKind::LogicalAnd(Arc::new(left), Arc::new(right)),
                self.make_source(span),
            )?;
        }

        Ok(left)
    }

    fn at_bare_veto_token(&mut self) -> Result<bool, Error> {
        if !self.at(&TokenKind::Veto)? {
            return Ok(false);
        }
        let checkpoint = self.checkpoint();
        self.next()?;
        let bare = !self.at(&TokenKind::StringLit)?;
        self.restore(checkpoint);
        Ok(bare)
    }

    fn at_bare_veto_followed_by_is(&mut self) -> Result<bool, Error> {
        if !self.at_bare_veto_token()? {
            return Ok(false);
        }
        let checkpoint = self.checkpoint();
        self.next()?;
        let followed = self.at(&TokenKind::Is)?;
        self.restore(checkpoint);
        Ok(followed)
    }

    fn at_not_bare_veto_followed_by_is(&mut self) -> Result<bool, Error> {
        if !self.at(&TokenKind::Not)? {
            return Ok(false);
        }
        let checkpoint = self.checkpoint();
        self.next()?;
        if !self.at(&TokenKind::Veto)? {
            self.restore(checkpoint);
            return Ok(false);
        }
        self.next()?;
        if self.at(&TokenKind::StringLit)? {
            self.restore(checkpoint);
            return Ok(false);
        }
        let followed = self.at(&TokenKind::Is)?;
        self.restore(checkpoint);
        Ok(followed)
    }

    fn wrap_result_is_veto_expression(
        &mut self,
        operand: Expression,
        operator_is_not: bool,
        keyword_was_negated: bool,
        start_span: Span,
    ) -> Result<Expression, Error> {
        let negate = operator_is_not ^ keyword_was_negated;
        let end_span = operand
            .source_location
            .as_ref()
            .map(|source| source.span.clone())
            .unwrap_or_else(|| start_span.clone());
        let span = self.span_covering(&start_span, &end_span);
        let core = self.new_expression(
            ExpressionKind::ResultIsVeto(Arc::new(operand)),
            self.make_source(span.clone()),
        )?;
        if negate {
            self.new_expression(
                ExpressionKind::LogicalNegation(Arc::new(core), NegationType::Not),
                self.make_source(span),
            )
        } else {
            Ok(core)
        }
    }

    fn parse_veto_status_lhs_is_comparison(&mut self) -> Result<Expression, Error> {
        let start_span = self.peek()?.span.clone();
        let keyword_was_negated = if self.at(&TokenKind::Not)? {
            self.next()?;
            true
        } else {
            false
        };
        self.expect(&TokenKind::Veto)?;
        if self.at(&TokenKind::StringLit)? {
            let tok = self.peek()?.clone();
            return Err(self.error_at_token(
                &tok,
                "veto with a message is only valid as a rule or unless result, not in `is veto` comparisons",
            ));
        }
        let operator = self.parse_comparison_operator()?;
        let operator_is_not = matches!(operator, ComparisonComputation::IsNot);
        if !matches!(
            operator,
            ComparisonComputation::Is | ComparisonComputation::IsNot
        ) {
            let tok = self.peek()?.clone();
            return Err(self.error_at_token(
                &tok,
                "Expected `is` or `is not` after `veto` in a veto-status comparison",
            ));
        }
        let operand = self.parse_range_expression()?;
        self.wrap_result_is_veto_expression(
            operand,
            operator_is_not,
            keyword_was_negated,
            start_span,
        )
    }

    fn parse_and_operand(&mut self) -> Result<Expression, Error> {
        if self.at_not_bare_veto_followed_by_is()? || self.at_bare_veto_followed_by_is()? {
            return self.parse_veto_status_lhs_is_comparison();
        }

        // not expression
        if self.at(&TokenKind::Not)? {
            return self.parse_not_expression();
        }

        // repository_with_suffix: repository_expression followed by optional suffix
        self.parse_repository_with_suffix()
    }

    fn parse_not_expression(&mut self) -> Result<Expression, Error> {
        let not_tok = self.expect(&TokenKind::Not)?;
        let start_span = not_tok.span.clone();

        self.check_depth()?;
        let operand = self.parse_and_operand()?;
        self.depth_tracker.pop_depth();

        let end_span = operand
            .source_location
            .as_ref()
            .map(|s| s.span.clone())
            .unwrap_or_else(|| start_span.clone());
        let span = self.span_covering(&start_span, &end_span);

        self.new_expression(
            ExpressionKind::LogicalNegation(Arc::new(operand), NegationType::Not),
            self.make_source(span),
        )
    }

    fn parse_repository_with_suffix(&mut self) -> Result<Expression, Error> {
        let start_span = self.peek()?.span.clone();
        let repository = self.parse_range_expression()?;
        self.continue_repository_operand(repository, start_span)
    }

    /// Postfix suffixes on a completed repository/range expression (`in`, calendar, comparison, `as`).
    fn continue_repository_operand(
        &mut self,
        mut expr: Expression,
        start_span: Span,
    ) -> Result<Expression, Error> {
        loop {
            let peeked = self.peek()?;

            if is_comparison_operator(&peeked.kind) {
                return self.parse_comparison_suffix(expr, start_span);
            }

            if peeked.kind == TokenKind::Not {
                expr = self.parse_not_in_calendar_suffix(expr, start_span.clone())?;
                continue;
            }

            if peeked.kind == TokenKind::In {
                expr = self.parse_in_suffix(expr, start_span.clone())?;
                continue;
            }

            if peeked.kind == TokenKind::As {
                expr = self.parse_as_chain(expr, start_span.clone())?;
                continue;
            }

            break;
        }

        if self.at_expression_suffix_end()? {
            return Ok(expr);
        }

        let tok = self.peek()?.clone();
        Err(self.error_at_token(
            &tok,
            format!("Unexpected token '{}' after expression", tok.text),
        ))
    }

    fn parse_comparison_suffix(
        &mut self,
        left: Expression,
        start_span: Span,
    ) -> Result<Expression, Error> {
        let operator = self.parse_comparison_operator()?;
        let operator_is_not = matches!(operator, ComparisonComputation::IsNot);

        if matches!(
            operator,
            ComparisonComputation::Is | ComparisonComputation::IsNot
        ) && self.at_bare_veto_token()?
        {
            self.expect(&TokenKind::Veto)?;
            if self.at(&TokenKind::StringLit)? {
                let tok = self.peek()?.clone();
                return Err(self.error_at_token(
                    &tok,
                    "veto with a message is only valid as a rule or unless result, not in `is veto` comparisons",
                ));
            }
            return self.wrap_result_is_veto_expression(left, operator_is_not, false, start_span);
        }

        // Right side can be: not_expr | range/repository expression (term-level `as` included)
        let right = if self.at(&TokenKind::Not)? {
            self.parse_not_expression()?
        } else {
            self.parse_range_expression()?
        };

        let end_span = right
            .source_location
            .as_ref()
            .map(|s| s.span.clone())
            .unwrap_or_else(|| start_span.clone());
        let span = self.span_covering(&start_span, &end_span);

        self.new_expression(
            ExpressionKind::Comparison(Arc::new(left), operator, Arc::new(right)),
            self.make_source(span),
        )
    }

    fn parse_comparison_operator(&mut self) -> Result<ComparisonComputation, Error> {
        let tok = self.next()?;
        match tok.kind {
            TokenKind::Gt => Ok(ComparisonComputation::GreaterThan),
            TokenKind::Lt => Ok(ComparisonComputation::LessThan),
            TokenKind::Gte => Ok(ComparisonComputation::GreaterThanOrEqual),
            TokenKind::Lte => Ok(ComparisonComputation::LessThanOrEqual),
            TokenKind::Is => {
                // Check for "is not"
                if self.at(&TokenKind::Not)? {
                    self.next()?; // consume 'not'
                    Ok(ComparisonComputation::IsNot)
                } else {
                    Ok(ComparisonComputation::Is)
                }
            }
            _ => Err(self.error_at_token(
                &tok,
                format!("Expected a comparison operator, found {}", tok.kind),
            )),
        }
    }

    fn parse_not_in_calendar_suffix(
        &mut self,
        repository: Expression,
        start_span: Span,
    ) -> Result<Expression, Error> {
        self.expect(&TokenKind::Not)?;
        self.expect(&TokenKind::In)?;
        self.expect_calendar_period_marker()?;
        let unit = self.parse_calendar_unit()?;
        let end = self.peek()?.span.clone();
        let span = self.span_covering(&start_span, &end);
        self.new_expression(
            ExpressionKind::DateCalendar(DateCalendarKind::NotIn, unit, Arc::new(repository)),
            self.make_source(span),
        )
    }

    fn parse_in_suffix(
        &mut self,
        repository: Expression,
        start_span: Span,
    ) -> Result<Expression, Error> {
        self.expect(&TokenKind::In)?;

        let peeked = self.peek()?;

        // "in past calendar <unit>" or "in future calendar <unit>"
        if peeked.kind == TokenKind::Past || peeked.kind == TokenKind::Future {
            let direction = self.next()?;
            let rel_kind = if direction.kind == TokenKind::Past {
                DateRelativeKind::InPast
            } else {
                DateRelativeKind::InFuture
            };

            // Check for "calendar" keyword
            if self.at_calendar_period_marker()? {
                self.next_calendar_period_marker()?;
                let cal_kind = if direction.kind == TokenKind::Past {
                    DateCalendarKind::Past
                } else {
                    DateCalendarKind::Future
                };
                let unit = self.parse_calendar_unit()?;
                let end = self.peek()?.span.clone();
                let span = self.span_covering(&start_span, &end);
                return self.new_expression(
                    ExpressionKind::DateCalendar(cal_kind, unit, Arc::new(repository)),
                    self.make_source(span),
                );
            }

            if self.at(&TokenKind::And)?
                || self.at(&TokenKind::Unless)?
                || self.at(&TokenKind::Then)?
                || self.at(&TokenKind::RParen)?
                || self.at(&TokenKind::Eof)?
                || is_comparison_operator(&self.peek()?.kind)
            {
                let end = self.peek()?.span.clone();
                let span = self.span_covering(&start_span, &end);
                return self.new_expression(
                    ExpressionKind::DateRelative(rel_kind, Arc::new(repository)),
                    self.make_source(span),
                );
            }

            let offset = self.parse_repository_expression()?;
            let offset_end_span = offset
                .source_location
                .as_ref()
                .map(|s| s.span.clone())
                .unwrap_or_else(|| start_span.clone());
            let range = self.new_expression(
                ExpressionKind::PastFutureRange(rel_kind, Arc::new(offset)),
                self.make_source(self.span_covering(&direction.span, &offset_end_span)),
            )?;
            let span = self.span_covering(&start_span, &offset_end_span);
            return self.new_expression(
                ExpressionKind::RangeContainment(Arc::new(repository), Arc::new(range)),
                self.make_source(span),
            );
        }

        // "in calendar <unit>"
        if token_is_calendar_period_marker(peeked) {
            self.next_calendar_period_marker()?;
            let unit = self.parse_calendar_unit()?;
            let end = self.peek()?.span.clone();
            let span = self.span_covering(&start_span, &end);
            return self.new_expression(
                ExpressionKind::DateCalendar(DateCalendarKind::Current, unit, Arc::new(repository)),
                self.make_source(span),
            );
        }

        let range = self.parse_range_expression()?;
        let end_span = range
            .source_location
            .as_ref()
            .map(|s| s.span.clone())
            .unwrap_or_else(|| start_span.clone());
        let span = self.span_covering(&start_span, &end_span);
        self.new_expression(
            ExpressionKind::RangeContainment(Arc::new(repository), Arc::new(range)),
            self.make_source(span),
        )
    }

    fn parse_as_chain(
        &mut self,
        mut expr: Expression,
        start_span: Span,
    ) -> Result<Expression, Error> {
        while self.at(&TokenKind::As)? {
            self.expect(&TokenKind::As)?;
            let target_tok = self.next()?;
            let target = if matches!(target_tok.kind, TokenKind::Permille) {
                ConversionTarget::Unit {
                    unit_name: "permille".to_string(),
                }
            } else if let Some(primitive) = token_kind_to_primitive(&target_tok.kind) {
                ConversionTarget::Type(primitive)
            } else if can_be_label(&target_tok.kind) {
                let mut unit_path = target_tok.text.clone();
                let mut end_span = target_tok.span.clone();
                while self.at(&TokenKind::Dot)? {
                    self.next()?;
                    let seg = self.next()?;
                    if !can_be_label(&seg.kind) {
                        return Err(self.error_at_token(
                            &seg,
                            format!("Expected a unit path segment after '.', found {}", seg.kind),
                        ));
                    }
                    unit_path.push('.');
                    unit_path.push_str(&seg.text);
                    end_span = seg.span.clone();
                }
                let target = ConversionTarget::Unit {
                    unit_name: unit_path,
                };
                expr = self.new_expression(
                    ExpressionKind::UnitConversion(Arc::new(expr), target),
                    self.make_source(self.span_covering(&start_span, &end_span)),
                )?;
                continue;
            } else {
                return Err(self.error_at_token(
                    &target_tok,
                    format!(
                        "Expected a type keyword or unit name after 'as', found {}",
                        target_tok.kind
                    ),
                ));
            };
            expr = self.new_expression(
                ExpressionKind::UnitConversion(Arc::new(expr), target),
                self.make_source(self.span_covering(&start_span, &target_tok.span)),
            )?;
        }
        Ok(expr)
    }

    fn is_plain_number_literal(expr: &Expression) -> bool {
        matches!(expr.kind, ExpressionKind::Literal(Value::Number(_)))
    }

    fn is_unit_conversion(expr: &Expression) -> bool {
        matches!(expr.kind, ExpressionKind::UnitConversion(..))
    }

    /// True when the next token can follow a completed suffix expression (no further operands).
    ///
    /// Must include every token that can start the next spec-body item, a new `spec`/`repo`,
    /// or end the file. See `parse_unit_conversion_before_expression_boundaries` in `parsing/mod.rs`.
    fn at_expression_suffix_end(&mut self) -> Result<bool, Error> {
        Ok(self.at(&TokenKind::And)?
            || self.at(&TokenKind::Unless)?
            || self.at(&TokenKind::Then)?
            || self.at(&TokenKind::RParen)?
            || self.at(&TokenKind::Eof)?
            || self.at(&TokenKind::Spec)?
            || self.at(&TokenKind::Repo)?
            || self.at(&TokenKind::Uses)?
            || is_spec_body_keyword(&self.peek()?.kind))
    }

    fn parse_calendar_unit(&mut self) -> Result<CalendarPeriodUnit, Error> {
        let tok = self.next()?;
        if let Some(unit) = CalendarPeriodUnit::from_keyword(&tok.text) {
            return Ok(unit);
        }
        Err(self.error_at_token(
            &tok,
            format!("Expected 'year', 'month', or 'week', found '{}'", tok.text),
        ))
    }

    // ========================================================================
    // Arithmetic expressions (precedence climbing)
    // ========================================================================

    fn parse_range_expression(&mut self) -> Result<Expression, Error> {
        self.parse_repository_expression()
    }

    /// Atom or range-typed value: `...` binds before `^`, `*`, `/`, `%` on the same operand.
    /// Both endpoints use [`Self::parse_range_ellipsis_bound`] (`+`/`-` only) so
    /// `now - 7 day...now` and `start...start + length` are valid, and
    /// `rate * period_start...period_end` keeps `*` outside the range.
    fn parse_range_operand(&mut self) -> Result<Expression, Error> {
        let start_span = self.peek()?.span.clone();
        let checkpoint = self.checkpoint();
        let left = self.parse_range_ellipsis_bound()?;
        if !self.at(&TokenKind::Ellipsis)? {
            self.restore(checkpoint);
            return self.parse_factor();
        }

        self.next()?;
        let right = self.parse_range_ellipsis_bound()?;
        let end_span = right
            .source_location
            .as_ref()
            .map(|s| s.span.clone())
            .unwrap_or_else(|| start_span.clone());
        let span = self.span_covering(&start_span, &end_span);
        self.new_expression(
            ExpressionKind::RangeLiteral(Arc::new(left), Arc::new(right)),
            self.make_source(span),
        )
    }

    /// One side of `...`: `+`/`-` between powers only (no `*`/`/`/`%` — those bind outside the range).
    fn parse_range_ellipsis_bound(&mut self) -> Result<Expression, Error> {
        let start_span = self.peek()?.span.clone();
        let mut left = self.parse_power_for_range_bound()?;

        while self.at_any(&[TokenKind::Plus, TokenKind::Minus])? {
            let op_tok = self.next()?;
            let operation = match op_tok.kind {
                TokenKind::Plus => ArithmeticComputation::Add,
                TokenKind::Minus => ArithmeticComputation::Subtract,
                _ => unreachable!("BUG: only + and - should reach here"),
            };

            let right = self.parse_power_for_range_bound()?;
            let end_span = right
                .source_location
                .as_ref()
                .map(|s| s.span.clone())
                .unwrap_or_else(|| start_span.clone());
            let span = self.span_covering(&start_span, &end_span);

            left = self.new_expression(
                ExpressionKind::Arithmetic(Arc::new(left), operation, Arc::new(right)),
                self.make_source(span),
            )?;
        }

        Ok(left)
    }

    fn parse_power_for_range_bound(&mut self) -> Result<Expression, Error> {
        let start_span = self.peek()?.span.clone();
        let left = self.parse_factor()?;

        if self.at(&TokenKind::Caret)? {
            self.next()?;
            self.check_depth()?;
            let right = self.parse_power_for_range_bound()?;
            self.depth_tracker.pop_depth();
            let end_span = right
                .source_location
                .as_ref()
                .map(|s| s.span.clone())
                .unwrap_or_else(|| start_span.clone());
            let span = self.span_covering(&start_span, &end_span);

            return self.new_expression(
                ExpressionKind::Arithmetic(
                    Arc::new(left),
                    ArithmeticComputation::Power,
                    Arc::new(right),
                ),
                self.make_source(span),
            );
        }

        Ok(left)
    }

    fn parse_repository_expression(&mut self) -> Result<Expression, Error> {
        let start_span = self.peek()?.span.clone();
        let mut left = self.parse_term()?;

        while self.at_any(&[TokenKind::Plus, TokenKind::Minus])? {
            // Check if this minus is really a binary operator or could be part of something else
            // In "X not in calendar year", we don't want to consume "not" as an operator
            let op_tok = self.next()?;
            let operation = match op_tok.kind {
                TokenKind::Plus => ArithmeticComputation::Add,
                TokenKind::Minus => ArithmeticComputation::Subtract,
                _ => unreachable!("BUG: only + and - should reach here"),
            };

            let right = self.parse_term()?;
            if Self::is_plain_number_literal(&left) && Self::is_unit_conversion(&right) {
                let source = right
                    .source_location
                    .clone()
                    .unwrap_or_else(|| self.make_source(start_span.clone()));
                return Err(Error::parsing(
                    "Cannot add a plain number to a converted value; convert each operand before \
                     '+' (e.g. '5 as usd + c as usd')",
                    source,
                    None::<String>,
                ));
            }

            let end_span = right
                .source_location
                .as_ref()
                .map(|s| s.span.clone())
                .unwrap_or_else(|| start_span.clone());
            let span = self.span_covering(&start_span, &end_span);

            left = self.new_expression(
                ExpressionKind::Arithmetic(Arc::new(left), operation, Arc::new(right)),
                self.make_source(span),
            )?;
        }

        Ok(left)
    }

    fn parse_term(&mut self) -> Result<Expression, Error> {
        self.parse_term_with_as(true)
    }

    fn parse_term_with_as(&mut self, allow_as: bool) -> Result<Expression, Error> {
        let start_span = self.peek()?.span.clone();
        let mut left = self.parse_power()?;
        if allow_as {
            left = self.parse_as_chain(left, start_span.clone())?;
        }

        while self.at_any(&[TokenKind::Star, TokenKind::Slash, TokenKind::Percent])? {
            // Be careful: % could be a percent literal suffix (e.g. 50%)
            // But here in term context, it's modulo since we already parsed the number
            let op_tok = self.next()?;
            let operation = match op_tok.kind {
                TokenKind::Star => ArithmeticComputation::Multiply,
                TokenKind::Slash => ArithmeticComputation::Divide,
                TokenKind::Percent => ArithmeticComputation::Modulo,
                _ => unreachable!("BUG: only *, /, % should reach here"),
            };

            let right_start_span = self.peek()?.span.clone();
            let mut right = self.parse_power()?;
            if allow_as {
                right = self.parse_as_chain(right, right_start_span)?;
            }
            let end_span = right
                .source_location
                .as_ref()
                .map(|s| s.span.clone())
                .unwrap_or_else(|| start_span.clone());
            let span = self.span_covering(&start_span, &end_span);

            left = self.new_expression(
                ExpressionKind::Arithmetic(Arc::new(left), operation, Arc::new(right)),
                self.make_source(span),
            )?;
        }

        Ok(left)
    }

    fn parse_power(&mut self) -> Result<Expression, Error> {
        let start_span = self.peek()?.span.clone();
        let left = self.parse_range_operand()?;

        if self.at(&TokenKind::Caret)? {
            self.next()?;
            self.check_depth()?;
            let right = self.parse_power()?;
            self.depth_tracker.pop_depth();
            let end_span = right
                .source_location
                .as_ref()
                .map(|s| s.span.clone())
                .unwrap_or_else(|| start_span.clone());
            let span = self.span_covering(&start_span, &end_span);

            return self.new_expression(
                ExpressionKind::Arithmetic(
                    Arc::new(left),
                    ArithmeticComputation::Power,
                    Arc::new(right),
                ),
                self.make_source(span),
            );
        }

        Ok(left)
    }

    fn parse_factor(&mut self) -> Result<Expression, Error> {
        let peeked = self.peek()?;
        let start_span = peeked.span.clone();

        if peeked.kind == TokenKind::Minus {
            self.next()?;
            let operand = self.parse_primary_or_math()?;
            let end_span = operand
                .source_location
                .as_ref()
                .map(|s| s.span.clone())
                .unwrap_or_else(|| start_span.clone());
            let span = self.span_covering(&start_span, &end_span);

            if let ExpressionKind::Literal(value) = &operand.kind {
                if let Ok(negated) = try_negate_numeric_literal(value.clone()) {
                    return self
                        .new_expression(ExpressionKind::Literal(negated), self.make_source(span));
                }
            }

            let zero = self.new_expression(
                ExpressionKind::Literal(Value::Number(Decimal::ZERO)),
                self.make_source(start_span),
            )?;
            return self.new_expression(
                ExpressionKind::Arithmetic(
                    Arc::new(zero),
                    ArithmeticComputation::Subtract,
                    Arc::new(operand),
                ),
                self.make_source(span),
            );
        }

        if peeked.kind == TokenKind::Plus {
            self.next()?;
            return self.parse_primary_or_math();
        }

        self.parse_primary_or_math()
    }

    fn parse_primary_or_math(&mut self) -> Result<Expression, Error> {
        let peeked = self.peek()?;

        // Math functions
        if is_math_function(&peeked.kind) {
            return self.parse_math_function();
        }

        self.parse_primary()
    }

    fn parse_math_function(&mut self) -> Result<Expression, Error> {
        let func_tok = self.next()?;
        let start_span = func_tok.span.clone();

        let operator = match func_tok.kind {
            TokenKind::Sqrt => MathematicalComputation::Sqrt,
            TokenKind::Sin => MathematicalComputation::Sin,
            TokenKind::Cos => MathematicalComputation::Cos,
            TokenKind::Tan => MathematicalComputation::Tan,
            TokenKind::Asin => MathematicalComputation::Asin,
            TokenKind::Acos => MathematicalComputation::Acos,
            TokenKind::Atan => MathematicalComputation::Atan,
            TokenKind::Log => MathematicalComputation::Log,
            TokenKind::Exp => MathematicalComputation::Exp,
            TokenKind::Abs => MathematicalComputation::Abs,
            TokenKind::Floor => MathematicalComputation::Floor,
            TokenKind::Ceil => MathematicalComputation::Ceil,
            TokenKind::Round => MathematicalComputation::Round,
            _ => unreachable!("BUG: only math functions should reach here"),
        };

        self.check_depth()?;
        let operand = self.parse_repository_expression()?;
        self.depth_tracker.pop_depth();

        let end_span = operand
            .source_location
            .as_ref()
            .map(|s| s.span.clone())
            .unwrap_or_else(|| start_span.clone());
        let span = self.span_covering(&start_span, &end_span);

        self.new_expression(
            ExpressionKind::MathematicalComputation(operator, Arc::new(operand)),
            self.make_source(span),
        )
    }

    fn parse_primary(&mut self) -> Result<Expression, Error> {
        let peeked = self.peek()?;
        let start_span = peeked.span.clone();

        match &peeked.kind {
            // Parenthesized expression
            TokenKind::LParen => {
                self.next()?; // consume (
                let inner = self.parse_expression()?;
                self.expect(&TokenKind::RParen)?;
                Ok(inner)
            }

            // Now keyword
            TokenKind::Now => {
                let tok = self.next()?;
                self.new_expression(ExpressionKind::Now, self.make_source(tok.span))
            }

            TokenKind::Past | TokenKind::Future => {
                let tok = self.next()?;
                let kind = if tok.kind == TokenKind::Past {
                    DateRelativeKind::InPast
                } else {
                    DateRelativeKind::InFuture
                };
                let offset = self.parse_repository_expression()?;
                let span = self.span_covering(
                    &start_span,
                    &offset
                        .source_location
                        .as_ref()
                        .map(|s| s.span.clone())
                        .unwrap_or(start_span.clone()),
                );
                self.new_expression(
                    ExpressionKind::PastFutureRange(kind, Arc::new(offset)),
                    self.make_source(span),
                )
            }

            // String literal
            TokenKind::StringLit => {
                let tok = self.next()?;
                self.new_expression(
                    ExpressionKind::Literal(Value::Text(tok.text)),
                    self.make_source(tok.span),
                )
            }

            // Boolean literals
            k if is_boolean_keyword(k) => {
                let tok = self.next()?;
                self.new_expression(
                    ExpressionKind::Literal(Value::Boolean(token_kind_to_boolean_value(&tok.kind))),
                    self.make_source(tok.span),
                )
            }

            // Number literal (could be: plain number, date, time, duration, percent, unit)
            TokenKind::NumberLit => self.parse_number_expression(),

            // Reference (identifier, type keyword)
            k if can_be_label(k) => {
                let reference = self.parse_expression_reference()?;
                let span = self.span_covering(&start_span, &self.last_span);
                self.new_expression(ExpressionKind::Reference(reference), self.make_source(span))
            }

            _ => {
                let tok = self.next()?;
                Err(self.error_at_token(
                    &tok,
                    format!("Expected an expression, found '{}'", tok.text),
                ))
            }
        }
    }

    fn parse_number_expression(&mut self) -> Result<Expression, Error> {
        let num_tok = self.next()?;
        let num_text = num_tok.text.clone();
        let start_span = num_tok.span.clone();

        // Check if this is a date literal (YYYY-MM-DD)
        if num_text.len() == 4
            && num_text.chars().all(|c| c.is_ascii_digit())
            && self.at(&TokenKind::Minus)?
        {
            // Peek further: if next-next is a number, this is likely a date
            // We need to be careful: "2024 - 5" is arithmetic, "2024-01-15" is a date
            // Date format requires: YYYY-MM-DD where MM and DD are 2 digits
            // This is ambiguous at the token level. Let's check if the pattern matches.
            // Since dates use -NN- pattern and arithmetic uses - N pattern (with spaces),
            // we can use the span positions to disambiguate.
            let minus_span = self.peek()?.span.clone();
            // If minus is immediately adjacent to the number (no space), it's a date
            if minus_span.start == start_span.end {
                let value = self.parse_date_literal(num_text, start_span.clone())?;
                return self
                    .new_expression(ExpressionKind::Literal(value), self.make_source(start_span));
            }
        }

        // Check for time literal (HH:MM:SS)
        if num_text.len() == 2
            && num_text.chars().all(|c| c.is_ascii_digit())
            && self.at(&TokenKind::Colon)?
        {
            let colon_span = self.peek()?.span.clone();
            if colon_span.start == start_span.end {
                let value = self.try_parse_time_literal(num_text, start_span.clone())?;
                return self
                    .new_expression(ExpressionKind::Literal(value), self.make_source(start_span));
            }
        }

        // Check for %% (permille)
        if self.at(&TokenKind::PercentPercent)? {
            let pp_tok = self.next()?;
            if let Ok(next_peek) = self.peek() {
                if next_peek.kind == TokenKind::NumberLit {
                    return Err(self.error_at_token(
                        &pp_tok,
                        "Permille literal cannot be followed by a digit",
                    ));
                }
            }
            let decimal = parse_decimal_string(&num_text, &start_span, self)?;
            return self.new_expression(
                ExpressionKind::Literal(Value::NumberWithUnit(decimal, "permille".to_string())),
                self.make_source(start_span),
            );
        }

        // Check for % (percent)
        if self.at(&TokenKind::Percent)? {
            let pct_span = self.peek()?.span.clone();
            // Only consume % if it's directly adjacent (no space) for the shorthand syntax
            // Or if it's "50 %" (space separated is also valid per the grammar)
            let pct_tok = self.next()?;
            if let Ok(next_peek) = self.peek() {
                if next_peek.kind == TokenKind::NumberLit || next_peek.kind == TokenKind::Percent {
                    return Err(self.error_at_token(
                        &pct_tok,
                        "Percent literal cannot be followed by a digit",
                    ));
                }
            }
            let decimal = parse_decimal_string(&num_text, &start_span, self)?;
            return self.new_expression(
                ExpressionKind::Literal(Value::NumberWithUnit(decimal, "percent".to_string())),
                self.make_source(self.span_covering(&start_span, &pct_span)),
            );
        }

        // Check for "permille" keyword
        if self.at(&TokenKind::Permille)? {
            self.next()?;
            let decimal = parse_decimal_string(&num_text, &start_span, self)?;
            return self.new_expression(
                ExpressionKind::Literal(Value::NumberWithUnit(decimal, "permille".to_string())),
                self.make_source(start_span),
            );
        }

        if can_be_label(&self.peek()?.kind) {
            let (unit_path, end_span) = self.parse_unit_path()?;
            let decimal = parse_decimal_string(&num_text, &start_span, self)?;
            return self.new_expression(
                ExpressionKind::Literal(Value::NumberWithUnit(decimal, unit_path)),
                self.make_source(self.span_covering(&start_span, &end_span)),
            );
        }

        // Plain number
        let decimal = parse_decimal_string(&num_text, &start_span, self)?;
        self.new_expression(
            ExpressionKind::Literal(Value::Number(decimal)),
            self.make_source(start_span),
        )
    }

    fn parse_expression_reference(&mut self) -> Result<Reference, Error> {
        let mut segments = Vec::new();

        let first = self.next()?;
        segments.push(first.text.clone());

        while self.at(&TokenKind::Dot)? {
            self.next()?; // consume .
            let seg = self.next()?;
            if !can_be_label(&seg.kind) {
                return Err(self.error_at_token(
                    &seg,
                    format!("Expected an identifier after '.', found {}", seg.kind),
                ));
            }
            segments.push(seg.text.clone());
        }

        Ok(Reference::from_path(segments))
    }
}

// ============================================================================
// Helper functions
// ============================================================================

fn parse_decimal_string(text: &str, span: &Span, parser: &Parser) -> Result<Decimal, Error> {
    text.parse::<crate::literals::NumberLiteral>()
        .map(|parsed| parsed.0)
        .map_err(|message| {
            Error::parsing(message, parser.make_source(span.clone()), None::<String>)
        })
}

/// Negate a numeric literal value. Returns `Err(value)` when the value is not a number.
fn try_negate_numeric_literal(value: Value) -> Result<Value, Value> {
    match value {
        Value::Number(d) => Ok(Value::Number(-d)),
        Value::NumberWithUnit(d, unit) => Ok(Value::NumberWithUnit(-d, unit)),
        other => Err(other),
    }
}

fn is_comparison_operator(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Gt | TokenKind::Lt | TokenKind::Gte | TokenKind::Lte | TokenKind::Is
    )
}
