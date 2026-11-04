use crate::error::Error;
use crate::limits::ResourceLimits;
use crate::parsing::ast::{try_parse_type_constraint_command, *};
use crate::parsing::lexer::{
    can_be_label, can_be_spec_name_spelling, can_be_unit_name, is_boolean_keyword, is_keyword,
    is_math_function, is_prefix_rounding, is_reserved_repository_name, is_spec_body_keyword,
    token_is_calendar_period_marker, token_kind_to_boolean_value,
    token_kind_to_conversion_type_kind, token_kind_to_primitive, Lexer, LexerCheckpoint, Token,
    TokenKind,
};
use crate::parsing::source::Source;
use indexmap::IndexMap;
use rust_decimal::Decimal;
use std::sync::Arc;

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
        let tok = *parser.peek()?;
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

/// Parser position to rewind to after a speculative parse.
struct ParserCheckpoint {
    lexer: LexerCheckpoint,
    expression_count: usize,
    table_len: usize,
}

struct Parser {
    lexer: Lexer,
    source: Arc<str>,
    source_type: crate::parsing::source::SourceType,
    depth_tracker: DepthTracker,
    expression_count: usize,
    max_expression_count: usize,
    max_spec_name_length: usize,
    max_data_name_length: usize,
    max_rule_name_length: usize,
    last_span: Span,
    /// Nesting depth of comma-separated call arguments (`find(a, b)`).
    call_argument_depth: usize,
    /// Operand starts of the bound chain that last completed without `...`.
    bound_chain_starts: Vec<usize>,
    /// Operand starts whose bound chain is known to end without `...`, under the flags
    /// they were parsed with. Such an operand is a suffix of an already-scanned chain.
    no_range_at: std::collections::HashMap<usize, ExprFlags>,
    table: ExprTable<ExpressionKind>,
    /// Continuation stack of the expression machine, kept for its capacity. Empty between
    /// runs; a run nested in another starts from a fresh stack.
    expr_frames: Vec<ExprCont>,
}

impl Parser {
    fn new(
        content: &str,
        source_type: crate::parsing::source::SourceType,
        limits: &ResourceLimits,
    ) -> Self {
        let source: Arc<str> = Arc::from(content);
        Parser {
            lexer: Lexer::new(Arc::clone(&source), &source_type),
            source,
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
            call_argument_depth: 0,
            bound_chain_starts: Vec::new(),
            no_range_at: std::collections::HashMap::new(),
            expr_frames: Vec::new(),
            table: ExprTable::default(),
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
        self.last_span = token.span;
        Ok(token)
    }

    fn at(&mut self, kind: &TokenKind) -> Result<bool, Error> {
        Ok(&self.peek()?.kind == kind)
    }

    fn at_any(&mut self, kinds: &[TokenKind]) -> Result<bool, Error> {
        let current = &self.peek()?.kind;
        Ok(kinds.contains(current))
    }

    fn checkpoint(&self) -> ParserCheckpoint {
        ParserCheckpoint {
            lexer: self.lexer.checkpoint(),
            expression_count: self.expression_count,
            table_len: self.table.len(),
        }
    }

    /// Rewinds to `checkpoint`, dropping every expression node parsed since.
    fn restore(&mut self, checkpoint: ParserCheckpoint) {
        self.lexer.restore(checkpoint.lexer);
        self.expression_count = checkpoint.expression_count;
        self.table.truncate(checkpoint.table_len);
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
        let peeked = *self.peek()?;
        Ok(token_is_calendar_period_marker(&self.source, &peeked))
    }

    fn expect_calendar_period_marker(&mut self) -> Result<Token, Error> {
        let token = self.next()?;
        if token_is_calendar_period_marker(&self.source, &token) {
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
            Source::new(self.source_type(), token.span),
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
            Source::new(self.source_type(), token.span),
            Some(suggestion),
        )
    }

    fn parse_spec_ref_trailing_effective(&mut self) -> Result<Option<DateTimeValue>, Error> {
        let mut effective = None;
        if self.at(&TokenKind::NumberLit)? {
            let peeked = *self.peek()?;
            let peeked_text = peeked.text.of(&self.source);
            if peeked_text.len() == 4 && peeked_text.chars().all(|c| c.is_ascii_digit()) {
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

        loop {
            let peek_kind = self.peek()?.kind;
            match peek_kind {
                TokenKind::Data => {
                    let datum = self.parse_data()?;
                    data.push(datum);
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
                TokenKind::With => {
                    let token = self.next()?;
                    return Err(self.error_at_token_with_suggestion(
                        &token,
                        "Standalone `with alias.field: value` is not allowed".to_string(),
                        "Use `uses alias: spec -> with field: value`",
                    ));
                }
                _ => {
                    let token = self.next()?;
                    return Err(self.error_at_token_with_suggestion(
                        &token,
                        format!(
                            "Expected 'data', 'rule', 'meta', 'uses', or a new 'spec', found '{}'",
                            token.text.of(&self.source)
                        ),
                        "Check the spelling or add the appropriate keyword",
                    ));
                }
            }
        }

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

    fn next_logical_name(&mut self) -> Result<Token, Error> {
        let token = self.lexer.next_logical_name()?;
        self.last_span = token.span;
        Ok(token)
    }

    fn parse_spec_name(&mut self) -> Result<(String, Span), Error> {
        let tok = self.next_logical_name()?;
        if tok.text.of(&self.source).starts_with('@') {
            return Err(Error::parsing(
                "'@' is not allowed in spec names; it is valid for repository names (`repo @org/name`) and qualifiers (`uses @org/name`)",
                self.make_source(tok.span),
                Some(
                    "Write `spec my_spec`, then reference registry specs as `uses alias: @org/repo spec_name` or `data x: alias.TypeName` after importing with `uses`.",
                ),
            ));
        }
        if !can_be_spec_name_spelling(tok.text.of(&self.source)) {
            return Err(self.error_at_token(
                &tok,
                format!(
                    "Expected a spec name, found '{}'",
                    tok.text.of(&self.source)
                ),
            ));
        }
        Ok((tok.text.of(&self.source).to_string(), tok.span))
    }

    /// Parse a repository qualifier as one opaque [`TokenKind::LogicalName`].
    fn parse_repository_qualifier(&mut self) -> Result<(RepositoryQualifier, Span), Error> {
        let tok = self.next_logical_name()?;
        if !tok.text.of(&self.source).starts_with('@')
            && is_reserved_repository_name(tok.text.of(&self.source))
        {
            return Err(self.error_at_token(
                &tok,
                format!(
                    "'{}' is a reserved keyword and cannot be used as a repository name",
                    tok.text.of(&self.source).to_ascii_lowercase()
                ),
            ));
        }
        Ok((
            RepositoryQualifier {
                name: tok.text.of(&self.source).to_string(),
            },
            tok.span,
        ))
    }

    /// Parses `[<repository_qualifier>] <spec> [<effective>]`
    pub fn parse_spec_ref_target(&mut self) -> Result<SpecRef, Error> {
        let first = self.next_logical_name()?;
        let mut repository = None;
        let mut repository_span = None;
        let (spec_name, spec_name_span) = if first.text.of(&self.source).starts_with('@') {
            repository = Some(RepositoryQualifier {
                name: first.text.of(&self.source).to_string(),
            });
            repository_span = Some(first.span);
            self.parse_spec_name()?
        } else if can_be_label(&self.peek()?.kind) {
            if is_reserved_repository_name(first.text.of(&self.source)) {
                return Err(self.error_at_token(
                    &first,
                    format!(
                        "'{}' is a reserved keyword and cannot be used as a repository name",
                        first.text.of(&self.source).to_ascii_lowercase()
                    ),
                ));
            }
            repository = Some(RepositoryQualifier {
                name: first.text.of(&self.source).to_string(),
            });
            repository_span = Some(first.span);
            self.parse_spec_name()?
        } else {
            (first.text.of(&self.source).to_string(), first.span)
        };
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

        let peeked = *self.peek()?;
        let peeked_text = peeked.text.of(&self.source).to_string();
        let peeked_span = peeked.span;

        // Check if it could be a date: 4-digit number followed by -
        if peeked_text.len() == 4 && peeked_text.chars().all(|c| c.is_ascii_digit()) {
            // Collect the full datetime string by consuming tokens
            let mut dt_str = String::new();
            let num_tok = self.next()?; // consume the year number
            dt_str.push_str(num_tok.text.of(&self.source));

            // Try to consume -MM-DD and optional T... parts
            while self.at(&TokenKind::Minus)? {
                self.next()?; // consume -
                dt_str.push('-');
                let part = self.next()?;
                dt_str.push_str(part.text.of(&self.source));
            }

            // Check for T (time part)
            if self.at(&TokenKind::Identifier)? {
                let peeked = *self.peek()?;
                let time_marker = peeked.text.of(&self.source).starts_with(['T', 't']);
                if time_marker {
                    let time_part = self.next()?;
                    dt_str.push_str(time_part.text.of(&self.source));
                    // Consume any : separated parts
                    while self.at(&TokenKind::Colon)? {
                        self.next()?;
                        dt_str.push(':');
                        let part = self.next()?;
                        dt_str.push_str(part.text.of(&self.source));
                    }
                    // Check for timezone (+ or Z)
                    if self.at(&TokenKind::Plus)? {
                        self.next()?;
                        dt_str.push('+');
                        let tz_part = self.next()?;
                        dt_str.push_str(tz_part.text.of(&self.source));
                        if self.at(&TokenKind::Colon)? {
                            self.next()?;
                            dt_str.push(':');
                            let tz_min = self.next()?;
                            dt_str.push_str(tz_min.text.of(&self.source));
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
        let trimmed = token.text.of(&self.source).trim().to_string();
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
        let start_span = data_token.span;

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
                Some(Source::new(self.source_type(), start_span)),
            )?;
        }

        self.expect(&TokenKind::Colon)?;

        if !reference.segments.is_empty() {
            let tok = *self.peek()?;
            return Err(self.error_at_token_with_suggestion(
                &tok,
                "Dotted paths require `uses` with `-> with`; `data` declares types and values on local names only.",
                "Use `uses alias: spec` with `  -> with path.to.slot: <value or reference>`.",
            ));
        }

        let value = self.parse_data_value(&reference.name)?;

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

    fn parse_with_rhs(&mut self) -> Result<WithRhs, Error> {
        let peek_kind = self.peek()?.kind;

        if Self::with_rhs_starts_as_literal(&peek_kind) {
            let value = self.parse_literal_value()?;
            return Ok(WithRhs::Literal(value));
        }

        if can_be_label(&peek_kind) {
            let target = self.parse_reference()?;
            if self.at(&TokenKind::Arrow)? && self.lexer.peek_second()?.kind != TokenKind::With {
                let tok = *self.peek()?;
                return Err(self.error_at_token_with_suggestion(
                    &tok,
                    "Constraint chains (`-> ...`) are not allowed on a `uses` binding reference.",
                    "Use `data name: <type> -> ...` for type constraints on local slots.",
                ));
            }
            return Ok(WithRhs::Reference { target });
        }

        let tok = *self.peek()?;
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
                    first.text.of(&self.source).to_ascii_lowercase()
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

        segments.push(first.text.of(&self.source).to_string());

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
            segments.push(seg.text.of(&self.source).to_string());
        }

        Ok(Reference::from_path(segments))
    }

    fn parse_data_value(&mut self, data_name: &str) -> Result<DataValue, Error> {
        if self.at(&TokenKind::Spec)? {
            let token = self.next()?;
            return Err(self.error_at_token_with_suggestion(
                &token,
                "Cannot import a spec with `data`; use `uses`",
                "Use `uses <spec_name>` or `uses <alias>: <spec_name>`",
            ));
        }

        let peek = *self.peek()?;
        let starts_parent_type = token_kind_to_primitive(&peek.kind).is_some()
            || can_be_label(&peek.kind)
            || (peek.kind == TokenKind::LogicalName && peek.text.of(&self.source).starts_with('@'));

        if starts_parent_type {
            let base = self.parse_parent_type()?;
            if matches!(base, ParentType::List { .. }) {
                let (constraints, list) = self.parse_list_suffix(data_name)?;
                return Ok(DataValue::Definition {
                    base: Some(base),
                    constraints,
                    value: None,
                    list,
                });
            }
            let constraints = self.parse_trailing_constraints()?;
            return Ok(DataValue::Definition {
                base: Some(base),
                constraints,
                value: None,
                list: None,
            });
        }

        // Otherwise, it's a literal value
        let value = self.parse_literal_value()?;
        Ok(DataValue::Definition {
            base: None,
            constraints: None,
            value: Some(value),
            list: None,
        })
    }

    /// True when the next arrows are instance-list `-> key` or `-> with` field/named-instance rows.
    fn data_instance_list_body_ahead(&mut self) -> Result<bool, Error> {
        if !self.at(&TokenKind::Arrow)? {
            return Ok(false);
        }
        match self.lexer.peek_second()?.kind {
            TokenKind::Key => Ok(true),
            TokenKind::With => {
                let checkpoint = self.checkpoint();
                self.next()?; // ->
                self.next()?; // with
                let instance = if self.at_uses_field_pair()? {
                    true
                } else {
                    let kind = self.peek()?.kind;
                    can_be_label(&kind) && !Self::with_rhs_starts_as_literal(&kind)
                };
                self.restore(checkpoint);
                Ok(instance)
            }
            _ => Ok(false),
        }
    }

    fn list_query_ahead(&mut self) -> Result<bool, Error> {
        if !self.at(&TokenKind::Arrow)? {
            return Ok(false);
        }
        Ok(matches!(
            self.lexer.peek_second()?.kind,
            TokenKind::Find
                | TokenKind::First
                | TokenKind::Last
                | TokenKind::Sum
                | TokenKind::Avg
                | TokenKind::Product
                | TokenKind::Count
                | TokenKind::Min
                | TokenKind::Max
                | TokenKind::Any
                | TokenKind::All
                | TokenKind::Where
                | TokenKind::Sort
                | TokenKind::Top
        ))
    }

    fn option_field_row_ahead(&mut self) -> Result<bool, Error> {
        if !self.at(&TokenKind::Arrow)? {
            return Ok(false);
        }
        let second = *self.lexer.peek_second()?;
        if !second.text.of(&self.source).eq_ignore_ascii_case("option") {
            return Ok(false);
        }
        let checkpoint = self.checkpoint();
        self.next()?;
        self.next()?;
        let field = self.at_uses_field_pair()?;
        self.restore(checkpoint);
        Ok(field)
    }

    fn parse_option_field_row(
        &mut self,
        alias: &str,
        list: &mut InstanceListBody,
    ) -> Result<(), Error> {
        let arrow = self.next()?;
        let name_tok = self.next()?;
        let bindings = self.parse_row_field_pairs(alias, &name_tok)?;
        let span = self.span_covering(&arrow.span, &self.last_span);
        list.option_rows.push(InstanceListRow::Fields {
            bindings,
            source_location: self.make_source(span),
        });
        Ok(())
    }

    /// Parse a single `uses` item: `[alias ':']` then [`Self::parse_spec_ref_target`] (optional
    /// repository qualifier, spec name, optional effective date pin).
    fn parse_uses_item(&mut self, start_span: &Span) -> Result<LemmaData, Error> {
        let explicit_alias = if self.lexer.peek_second()?.kind == TokenKind::Colon {
            let peek = *self.peek()?;
            if is_keyword(&peek.kind) {
                return Err(self.error_at_token_with_suggestion(
                    &peek,
                    format!(
                        "'{}' is a reserved keyword and cannot be used as a name",
                        peek.text.of(&self.source).to_ascii_lowercase()
                    ),
                    "Choose a different name that is not a reserved keyword",
                ));
            }
            if can_be_label(&peek.kind) {
                let alias_tok = self.next()?;
                self.expect(&TokenKind::Colon)?;
                Some(alias_tok)
            } else {
                None
            }
        } else {
            None
        };

        let spec_ref = self.parse_spec_ref_target()?;

        let spec_name_source = spec_ref
            .target_span
            .as_ref()
            .map(|sp| Source::new(self.source_type(), *sp));

        crate::limits::check_max_length(
            &spec_ref.name,
            self.max_spec_name_length,
            "spec",
            spec_name_source.clone(),
        )?;

        let alias = if let Some(ref alias_tok) = explicit_alias {
            crate::limits::check_max_length(
                alias_tok.text.of(&self.source),
                self.max_data_name_length,
                "data",
                Some(Source::new(self.source_type(), alias_tok.span)),
            )?;
            alias_tok.text.of(&self.source).to_string()
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

        if self.at(&TokenKind::List)? {
            let list_tok = *self.peek()?;
            return Err(self.error_at_token_with_suggestion(
                &list_tok,
                format!(
                    "`uses {alias}: {spec_ref} list` is not valid; `uses` imports one sealed item"
                ),
                format!(
                    "Write `uses {alias}: {spec_ref}` for one item, or `data {alias}: {spec_ref} list` for a caller-owned list, or a list-valued rule for a sealed catalog"
                ),
            ));
        }

        let mut bindings = Vec::new();
        let mut effective_ref = None;
        while self.at(&TokenKind::Arrow)? {
            let command = self.lexer.peek_second()?.kind;
            match command {
                TokenKind::With => {
                    self.next()?; // ->
                    let with_token = self.expect(&TokenKind::With)?;
                    bindings.extend(self.parse_uses_field_pairs(&alias, &with_token)?);
                }
                TokenKind::Effective => {
                    self.next()?; // ->
                    let effective_token = self.expect(&TokenKind::Effective)?;
                    if effective_ref.is_some() {
                        return Err(self.error_at_token_with_suggestion(
                            &effective_token,
                            "A `uses` block may have at most one `-> effective`.",
                            "Remove the extra `-> effective` line.",
                        ));
                    }
                    if spec_ref.effective.is_some() {
                        return Err(self.error_at_token_with_suggestion(
                            &effective_token,
                            "Cannot combine a literal effective pin on the `uses` target with `-> effective`.",
                            "Remove the date after the spec name, or remove `-> effective`.",
                        ));
                    }
                    effective_ref = Some(self.parse_reference()?);
                }
                _ => {
                    let tok = *self.peek()?;
                    return Err(self.error_at_token_with_suggestion(
                        &tok,
                        "Expected `with` or `effective` after `->` in a `uses` block.",
                        "Write `  -> with path.to.slot: <value or reference>` or `  -> effective <date>` under the `uses` line.",
                    ));
                }
            }
        }

        let span = self.span_covering(start_span, &self.last_span);
        Ok(LemmaData::new(
            Reference::local(alias),
            DataValue::Import {
                spec_ref,
                bindings,
                effective_ref,
            },
            self.make_source(span),
        ))
    }

    /// Constraints and instance-list rows under `data name: T list`.
    ///
    /// `-> key` and instance `-> with` stay on the list body. `-> options`, `-> help`,
    /// and a value-list `-> with` stay constraints. A list can carry both.
    fn parse_list_suffix(
        &mut self,
        alias: &str,
    ) -> Result<(Option<Vec<Constraint>>, Option<InstanceListBody>), Error> {
        let mut commands = Vec::new();
        let mut list = InstanceListBody {
            key: None,
            rows: Vec::new(),
            option_rows: Vec::new(),
        };
        let mut saw_instance = false;
        while self.at(&TokenKind::Arrow)? {
            if self.option_field_row_ahead()? {
                saw_instance = true;
                self.parse_option_field_row(alias, &mut list)?;
            } else if self.data_instance_list_body_ahead()? {
                saw_instance = true;
                self.parse_instance_list_command(alias, &mut list)?;
            } else if self.list_query_ahead()? {
                let arrow = self.next()?;
                let name_tok = self.next()?;
                let command = self.parse_list_command_body(name_tok)?;
                let command = display_list_command(&self.table, &command);
                return Err(self.error_at_token(
                    &arrow,
                    format!(
                        "data supplies values; `-> {command}` is a query on `{alias}`, write it in a rule"
                    ),
                ));
            } else {
                commands.push(self.parse_one_constraint()?);
            }
        }
        let constraints = if commands.is_empty() {
            None
        } else {
            Some(commands)
        };
        let list = if saw_instance { Some(list) } else { None };
        Ok((constraints, list))
    }

    /// `-> key`, `-> with` rows under `data name: Spec list`.
    fn parse_instance_list_command(
        &mut self,
        alias: &str,
        list: &mut InstanceListBody,
    ) -> Result<(), Error> {
        let arrow = self.next()?;
        let name_tok = self.next()?;
        match name_tok.kind {
            TokenKind::Key => {
                if list.key.is_some() {
                    return Err(self.error_at_token(
                            &name_tok,
                            format!("`{alias}` declares `-> key` twice; list every key field in one `-> key`"),
                        ));
                }
                if !list.rows.is_empty() {
                    return Err(self.error_at_token(
                        &name_tok,
                        format!("`-> key` on `{alias}` must come before the first `-> with`"),
                    ));
                }
                let mut fields = vec![self.parse_reference()?];
                while self.at(&TokenKind::Comma)? {
                    self.next()?;
                    fields.push(self.parse_reference()?);
                }
                let span = self.span_covering(&arrow.span, &self.last_span);
                list.key = Some(InstanceListKey {
                    fields,
                    source_location: self.make_source(span),
                });
            }
            TokenKind::With => {
                let row = if self.at_uses_field_pair()? {
                    let bindings = self.parse_row_field_pairs(alias, &name_tok)?;
                    let span = self.span_covering(&arrow.span, &self.last_span);
                    InstanceListRow::Fields {
                        bindings,
                        source_location: self.make_source(span),
                    }
                } else {
                    let values = self.parse_instance_list_values()?;
                    let span = self.span_covering(&arrow.span, &self.last_span);
                    InstanceListRow::Values {
                        values,
                        source_location: self.make_source(span),
                    }
                };
                list.rows.push(row);
            }
            other => {
                unreachable!("BUG: instance-list command ahead was not key or with, got {other:?}")
            }
        }
        Ok(())
    }

    /// `-> with ada, grace` under `data name: Spec list`: references to instances (literals are
    /// rejected at planning with the row spec's name).
    fn parse_instance_list_values(&mut self) -> Result<Vec<WithRhs>, Error> {
        let mut values = Vec::new();
        loop {
            values.push(self.parse_with_rhs()?);
            if !self.at(&TokenKind::Comma)? {
                break;
            }
            self.next()?;
            if self.at_command_terminator()? || self.at(&TokenKind::Comma)? {
                let tok = *self.peek()?;
                return Err(self.error_at_token_with_suggestion(
                    &tok,
                    "Trailing or empty comma in list".to_string(),
                    "Remove the extra comma, or add the missing row",
                ));
            }
        }
        if self.at_uses_field_pair()? {
            let tok = *self.peek()?;
            return Err(self.error_at_token_with_suggestion(
                &tok,
                "A `-> with` line holds either references or field pairs, not both".to_string(),
                "Put the field pairs on their own `-> with` line",
            ));
        }
        Ok(values)
    }

    fn parse_row_field_pairs(
        &mut self,
        alias: &str,
        with_token: &Token,
    ) -> Result<Vec<RowBinding>, Error> {
        let mut bindings = Vec::new();
        let mut binding_start_span = with_token.span;
        loop {
            let path = self.parse_reference()?;
            for segment in path.segments.iter().chain(std::iter::once(&path.name)) {
                crate::limits::check_max_length(
                    segment,
                    self.max_data_name_length,
                    "row binding",
                    Some(Source::new(self.source_type(), binding_start_span)),
                )?;
            }
            if path
                .segments
                .first()
                .is_some_and(|segment| segment == alias)
            {
                return Err(self.error_at_token_with_suggestion(
                    with_token,
                    format!("Row path must be relative, not prefixed with `{alias}`."),
                    format!(
                        "Use `-> with {}: …` without the `{alias}.` prefix.",
                        path.name
                    ),
                ));
            }
            let (rhs, _) =
                self.parse_assignment_after_key(|parser| parser.parse_with_rhs(), "with", false)?;
            let binding_span = self.span_covering(&binding_start_span, &self.last_span);
            bindings.push(RowBinding {
                path,
                rhs,
                source_location: self.make_source(binding_span),
            });
            if self.at(&TokenKind::Comma)? {
                self.next()?;
                if self.at_command_terminator()? || self.at(&TokenKind::Comma)? {
                    let tok = *self.peek()?;
                    return Err(self.error_at_token_with_suggestion(
                        &tok,
                        "Trailing or empty comma in list".to_string(),
                        "Remove the extra comma, or add the missing field pair",
                    ));
                }
                if !self.at_uses_field_pair()? {
                    let tok = *self.peek()?;
                    return Err(self.error_at_token_with_suggestion(
                        &tok,
                        "Expected a field pair after comma".to_string(),
                        "Remove the extra comma, or add `path: value`",
                    ));
                }
                binding_start_span = self.peek()?.span;
                continue;
            }
            if self.at_uses_field_pair()? {
                let tok = *self.peek()?;
                return Err(self.error_at_token_with_suggestion(
                    &tok,
                    "Expected a comma between field pairs".to_string(),
                    "Separate field pairs with commas",
                ));
            }
            return Ok(bindings);
        }
    }

    /// Comma-separated `path: value` pairs after `-> with` in a `uses` block.
    fn parse_uses_field_pairs(
        &mut self,
        alias: &str,
        with_token: &Token,
    ) -> Result<Vec<UsesBinding>, Error> {
        let mut bindings = Vec::new();
        let mut binding_start_span = with_token.span;
        loop {
            let path = self.parse_reference()?;
            for segment in path.segments.iter().chain(std::iter::once(&path.name)) {
                crate::limits::check_max_length(
                    segment,
                    self.max_data_name_length,
                    "uses binding",
                    Some(Source::new(self.source_type(), binding_start_span)),
                )?;
            }
            if path
                .segments
                .first()
                .is_some_and(|segment| segment == alias)
            {
                return Err(self.error_at_token_with_suggestion(
                    with_token,
                    format!(
                        "Binding path must be relative to the imported spec, not prefixed with import alias `{alias}`."
                    ),
                    format!(
                        "Use `-> with {}: …` without the `{alias}.` prefix.",
                        path.name
                    ),
                ));
            }

            let (first_rhs, _) =
                self.parse_assignment_after_key(|parser| parser.parse_with_rhs(), "with", false)?;
            let mut next_field_pair = false;
            let rhs = match first_rhs {
                WithRhs::Literal(value) => UsesBindingRhs::Literal(value),
                WithRhs::Reference { target } => {
                    let mut targets = vec![target];
                    while self.at(&TokenKind::Comma)? {
                        self.next()?;
                        if self.at_command_terminator()? || self.at(&TokenKind::Comma)? {
                            let tok = *self.peek()?;
                            return Err(self.error_at_token_with_suggestion(
                                &tok,
                                "Trailing or empty comma in list".to_string(),
                                "Remove the extra comma, or add the missing reference",
                            ));
                        }
                        if self.at_uses_field_pair()? {
                            next_field_pair = true;
                            break;
                        }
                        let rhs_token = *self.peek()?;
                        match self.parse_with_rhs()? {
                            WithRhs::Reference { target } => targets.push(target),
                            WithRhs::Literal(_) => {
                                return Err(self.error_at_token_with_suggestion(
                                    &rhs_token,
                                    "A `uses -> with` reference sequence cannot contain literals"
                                        .to_string(),
                                    "Use only `uses` aliases in the sequence",
                                ));
                            }
                        }
                    }
                    UsesBindingRhs::References { targets }
                }
            };

            let binding_span = self.span_covering(&binding_start_span, &self.last_span);
            bindings.push(UsesBinding {
                path,
                rhs,
                source_location: self.make_source(binding_span),
            });

            if next_field_pair {
                binding_start_span = self.peek()?.span;
                continue;
            }

            if self.at(&TokenKind::Comma)? {
                self.next()?;
                if self.at_command_terminator()? || self.at(&TokenKind::Comma)? {
                    let tok = *self.peek()?;
                    return Err(self.error_at_token_with_suggestion(
                        &tok,
                        "Trailing or empty comma in list".to_string(),
                        "Remove the extra comma, or add the missing field pair",
                    ));
                }
                if !self.at_uses_field_pair()? {
                    let tok = *self.peek()?;
                    return Err(self.error_at_token_with_suggestion(
                        &tok,
                        "Expected a field pair after comma".to_string(),
                        "Remove the extra comma, or add `path: value`",
                    ));
                }
                binding_start_span = self.peek()?.span;
                continue;
            }

            if self.at_uses_field_pair()? {
                let tok = *self.peek()?;
                return Err(self.error_at_token_with_suggestion(
                    &tok,
                    "Expected a comma between field pairs".to_string(),
                    "Separate field pairs with commas, for example `-> with qty: 3, sku: \"latte\"`",
                ));
            }
            return Ok(bindings);
        }
    }

    fn at_uses_field_pair(&mut self) -> Result<bool, Error> {
        if self.at_command_terminator()? {
            return Ok(false);
        }
        self.at_path_then_colon()
    }

    /// True when the next tokens are `label (. label)* :`, the start of a field pair.
    /// Does not consume input.
    fn at_path_then_colon(&mut self) -> Result<bool, Error> {
        if !can_be_label(&self.peek()?.kind) {
            return Ok(false);
        }
        let checkpoint = self.checkpoint();
        let result = self.scan_path_then_colon();
        self.restore(checkpoint);
        result
    }

    fn scan_path_then_colon(&mut self) -> Result<bool, Error> {
        self.next()?;
        loop {
            match self.peek()?.kind {
                TokenKind::Colon => return Ok(true),
                TokenKind::Dot => {
                    self.next()?;
                    if !can_be_label(&self.peek()?.kind) {
                        return Ok(false);
                    }
                    self.next()?;
                }
                _ => return Ok(false),
            }
        }
    }

    fn parse_uses_statement(&mut self) -> Result<LemmaData, Error> {
        let uses_token = self.expect(&TokenKind::Uses)?;
        let start_span = uses_token.span;
        self.parse_uses_item(&start_span)
    }

    // ========================================================================
    // Rule parsing
    // ========================================================================

    fn parse_rule(&mut self) -> Result<LemmaRule, Error> {
        let rule_token = self.expect(&TokenKind::Rule)?;
        let start_span = rule_token.span;

        let name_tok = self.next()?;
        if is_keyword(&name_tok.kind) {
            return Err(self.error_at_token_with_suggestion(
                &name_tok,
                format!(
                    "'{}' is a reserved keyword and cannot be used as a rule name",
                    name_tok.text.of(&self.source).to_ascii_lowercase()
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
        let rule_name = name_tok.text.of(&self.source).to_string();
        crate::limits::check_max_length(
            &rule_name,
            self.max_rule_name_length,
            "rule",
            Some(Source::new(self.source_type(), name_tok.span)),
        )?;

        self.expect(&TokenKind::Colon)?;

        // Parse the base expression or veto result (`veto "msg"`). `veto is …` is an expression.
        let root = if self.at(&TokenKind::Veto)? && !self.at_bare_veto_followed_by_is()? {
            self.parse_veto_expression()?
        } else {
            self.parse_expression()?
        };

        let expression = self.finish_expression(root);

        // Parse unless clauses
        let mut unless_clauses = Vec::new();
        while self.at(&TokenKind::Unless)? {
            unless_clauses.push(self.parse_unless_clause()?);
        }

        let end_span = if let Some(last_unless) = unless_clauses.last() {
            last_unless.source_location.span
        } else if let Some(loc) = expression.root_source() {
            loc.span
        } else {
            start_span
        };

        let span = self.span_covering(&start_span, &end_span);
        Ok(LemmaRule {
            name: rule_name,
            expression,
            unless_clauses,
            source_location: self.make_source(span),
        })
    }

    fn parse_veto_expression(&mut self) -> Result<ExprId, Error> {
        let veto_tok = self.expect(&TokenKind::Veto)?;
        let start_span = veto_tok.span;

        let message = if self.at(&TokenKind::StringLit)? {
            let str_tok = self.next()?;
            Some(str_tok.text.of(&self.source).to_string())
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
        let start_span = unless_tok.span;

        let condition_root = self.parse_expression()?;
        let condition = self.finish_expression(condition_root);

        self.expect(&TokenKind::Then)?;

        let result_root = if self.at(&TokenKind::Veto)? {
            self.parse_veto_expression()?
        } else {
            self.parse_expression()?
        };

        let end_span = self.expr_span(result_root).unwrap_or(start_span);
        let span = self.span_covering(&start_span, &end_span);
        let result = self.finish_expression(result_root);

        Ok(UnlessClause {
            condition,
            result,
            source_location: self.make_source(span),
        })
    }

    /// Parse a parent type, including optional `range` / `list` suffixes.
    ///
    /// List and type parents are bare names. Pin and `@repo` belong on `uses`.
    fn parse_parent_type(&mut self) -> Result<ParentType, Error> {
        let peek = *self.peek()?;
        let base = if let Some(primitive) = token_kind_to_primitive(&peek.kind) {
            self.next()?;
            if self.at(&TokenKind::Dot)? {
                let dot_tok = *self.peek()?;
                return Err(self.error_at_token_with_suggestion(
                    &dot_tok,
                    "A primitive type cannot be the left segment of a qualified parent path",
                    "Use `data name: alias.typename` where `alias` is the `uses` import name and `typename` is the parent type.",
                ));
            }
            ParentType::Primitive { primitive }
        } else {
            self.parse_custom_or_spec_element_parent()?
        };

        let base = if self.at(&TokenKind::Identifier)? && {
            let peeked = *self.peek()?;
            peeked.text.of(&self.source) == "range"
        } {
            self.next()?;
            ParentType::Ranged {
                inner: Box::new(base),
            }
        } else {
            base
        };

        if self.at(&TokenKind::List)? {
            self.next()?;
            Ok(ParentType::List {
                inner: Box::new(base),
            })
        } else {
            Ok(base)
        }
    }

    /// Custom or qualified type name. `@repo` and a trailing pin are parse Errors.
    fn parse_custom_or_spec_element_parent(&mut self) -> Result<ParentType, Error> {
        let peek = *self.peek()?;
        if peek.text.of(&self.source).starts_with('@') {
            return Err(self.error_at_token_with_suggestion(
                &peek,
                "A repository qualifier is not valid on a type or list element",
                "Write `uses @repo Spec [pin]`, then `data name: Spec list` or `rule name: Spec list`.",
            ));
        }
        let (name, _) = self.parse_spec_name()?;
        if let Some((alias, type_name)) = name.rsplit_once('.') {
            return Ok(ParentType::Qualified {
                spec_alias: alias.to_string(),
                inner: Box::new(ParentType::custom(type_name.to_string())),
            });
        }
        if self.at(&TokenKind::NumberLit)? {
            let year = *self.peek()?;
            if year.text.of(&self.source).len() == 4
                && year
                    .text
                    .of(&self.source)
                    .chars()
                    .all(|c| c.is_ascii_digit())
            {
                return Err(self.error_at_token_with_suggestion(
                    &year,
                    "An effective pin is not valid on a type or list element",
                    "Write `uses Spec <pin>`, then `data name: Spec list` or `rule name: Spec list`.",
                ));
            }
        }
        Ok(ParentType::custom(name))
    }

    fn parse_trailing_constraints(&mut self) -> Result<Option<Vec<Constraint>>, Error> {
        let mut commands = Vec::new();
        while self.at(&TokenKind::Arrow)? {
            commands.push(self.parse_one_constraint()?);
        }
        let constraints = if commands.is_empty() {
            None
        } else {
            Some(commands)
        };
        Ok(constraints)
    }

    fn parse_one_constraint(&mut self) -> Result<Constraint, Error> {
        let arrow_token = self.next()?;
        let constraint_start_span = arrow_token.span;
        let (cmd, cmd_args, deprecated_without_colon) = self.parse_constraint_command()?;
        let constraint_span = self.span_covering(&constraint_start_span, &self.last_span);
        Ok(Constraint {
            command: cmd,
            args: cmd_args,
            source_location: self.make_source(constraint_span),
            deprecated_without_colon,
        })
    }

    fn parse_constraint_command(
        &mut self,
    ) -> Result<(TypeConstraintCommand, Vec<CommandArg>, bool), Error> {
        let name_tok = self.next()?;
        if matches!(name_tok.kind, TokenKind::Key) {
            return Err(self.error_at_token(
                &name_tok,
                "value lists are numbered; `-> key` is for instance lists".to_string(),
            ));
        }
        if matches!(
            name_tok.kind,
            TokenKind::Find
                | TokenKind::First
                | TokenKind::Last
                | TokenKind::Sum
                | TokenKind::Avg
                | TokenKind::Product
                | TokenKind::Count
                | TokenKind::Min
                | TokenKind::Max
                | TokenKind::Any
                | TokenKind::All
                | TokenKind::Where
                | TokenKind::Sort
                | TokenKind::Top
        ) {
            return Err(self.error_at_token(
                &name_tok,
                "data supplies values; query commands belong on rules, not data declarations"
                    .to_string(),
            ));
        }
        if !can_be_label(&name_tok.kind)
            && !matches!(
                name_tok.kind,
                TokenKind::Lower | TokenKind::Upper | TokenKind::With
            )
        {
            return Err(self.error_at_token(
                &name_tok,
                format!("Expected a command name, found {}", name_tok.kind),
            ));
        }
        let cmd = try_parse_type_constraint_command(name_tok.text.of(&self.source)).ok_or_else(|| {
            self.error_at_token(
                &name_tok,
                format!(
                    "Unknown constraint command '{}'. Valid commands: help, suggest, with, unit, trait, minimum, maximum, lower, upper, decimals, option, options, length",
                    name_tok.text.of(&self.source)
                ),
            )
        })?;

        match cmd.continuation_shape() {
            ContinuationShape::Assignment => {
                let (args, deprecated_without_colon) = self.parse_unit_as_assignment()?;
                Ok((cmd, args, deprecated_without_colon))
            }
            ContinuationShape::Single => Ok((cmd, self.parse_single_command_arg(cmd)?, false)),
            ContinuationShape::CommaSeparated => {
                Ok((cmd, self.parse_comma_separated_command_args()?, false))
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
        let tok = *self.peek()?;
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

        let peek_kind = self.peek()?.kind;
        if is_keyword(&peek_kind) {
            let tok = *self.peek()?;
            return Err(self.error_at_token_with_suggestion(
                &tok,
                format!(
                    "'{}' is a reserved keyword and cannot be used as a unit name",
                    tok.text.of(&self.source).to_ascii_lowercase()
                ),
                "Choose a different name that is not a reserved keyword",
            ));
        }
        if !can_be_label(&peek_kind) {
            return Ok((Vec::new(), false));
        }

        let unit_name_tok = self.next()?;
        let unit_name_arg = CommandArg::Label(unit_name_tok.text.of(&self.source).to_string());

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
            let tok = *self.peek()?;
            return Err(self.error_at_token_with_suggestion(
                &tok,
                "Expected a unit conversion factor or compound unit expression after `:`.",
                "Use `-> unit <name>: <factor>` or `-> unit <name>: <compound>` (e.g. `unit eur: 1.00`, `unit eur_per_hour: eur/hour`).",
            ));
        }

        let numeric_prefix: Option<Decimal> = if self.at(&TokenKind::NumberLit)? {
            let num_tok = self.next()?;
            Some(parse_decimal_string(
                num_tok.text.of(&self.source),
                &num_tok.span,
                self,
            )?)
        } else {
            None
        };

        let peek_kind_after_prefix = self.peek()?.kind;
        let has_compound_expr =
            can_be_unit_name(&peek_kind_after_prefix) && !self.at_command_terminator()?;

        if has_compound_expr {
            // When no numeric prefix leads, a bare or dotted identifier is a dynamic
            // scale reference (`-> unit gbp: rate`, `-> unit gbp: fx.rate`) unless a
            // compound operator (`*`, `/`, `^`) follows, in which case it is the
            // first factor of a compound unit expression (`-> unit mps: meter/second`).
            match numeric_prefix {
                None => {
                    let checkpoint = self.checkpoint();
                    let reference = self.parse_reference()?;
                    let next_kind = self.peek()?.kind;
                    if matches!(
                        next_kind,
                        TokenKind::Star | TokenKind::Slash | TokenKind::Caret
                    ) {
                        self.restore(checkpoint);
                        let factors = self.parse_unit_factors()?;
                        let prefix = Decimal::ONE;
                        Ok(UnitArg::Expr(prefix, factors))
                    } else {
                        Ok(UnitArg::Reference(reference))
                    }
                }
                Some(prefix) => {
                    let factors = self.parse_unit_factors()?;
                    Ok(UnitArg::Expr(prefix, factors))
                }
            }
        } else if let Some(factor) = numeric_prefix {
            Ok(UnitArg::Factor(factor))
        } else {
            let tok = *self.peek()?;
            Err(self.error_at_token_with_suggestion(
                &tok,
                "Expected a unit conversion factor or compound unit expression after `:`.",
                "Use `-> unit <name>: <factor>` or `-> unit <name>: <compound>` (e.g. `unit eur: 1.00`, `unit eur_per_hour: eur/hour`).",
            ))
        }
    }

    fn parse_single_command_arg(
        &mut self,
        cmd: TypeConstraintCommand,
    ) -> Result<Vec<CommandArg>, Error> {
        let Some(arg) = self.try_parse_one_command_arg()? else {
            return Ok(Vec::new());
        };
        if self.at(&TokenKind::Comma)? || self.command_arg_starts_without_comma()? {
            let tok = *self.peek()?;
            return Err(self.error_at_token_with_suggestion(
                &tok,
                format!("`-> {cmd}` takes one value"),
                format!("Write `-> {cmd}` with a single argument"),
            ));
        }
        Ok(vec![arg])
    }

    fn try_parse_one_command_arg(&mut self) -> Result<Option<CommandArg>, Error> {
        if self.at(&TokenKind::Arrow)?
            || self.at(&TokenKind::Eof)?
            || is_spec_body_keyword(&self.peek()?.kind)
            || self.at(&TokenKind::Spec)?
            || self.at(&TokenKind::Comma)?
        {
            return Ok(None);
        }

        let peek_kind = self.peek()?.kind;
        match peek_kind {
            TokenKind::NumberLit | TokenKind::Minus | TokenKind::Plus | TokenKind::StringLit => {
                let value = self.parse_literal_value()?;
                Ok(Some(CommandArg::Literal(value)))
            }
            ref k if is_boolean_keyword(k) => {
                let value = self.parse_literal_value()?;
                Ok(Some(CommandArg::Literal(value)))
            }
            ref k if can_be_label(k) => {
                let tok = self.next()?;
                Ok(Some(CommandArg::Label(
                    tok.text.of(&self.source).to_string(),
                )))
            }
            _ => Ok(None),
        }
    }

    fn parse_comma_separated_command_args(&mut self) -> Result<Vec<CommandArg>, Error> {
        let mut args = Vec::new();
        if self.at_command_terminator()? {
            return Ok(args);
        }
        if self.at(&TokenKind::Comma)? {
            let tok = self.next()?;
            return Err(self.error_at_token_with_suggestion(
                &tok,
                "Unexpected comma".to_string(),
                "Remove the extra comma, or put a value before it",
            ));
        }

        loop {
            let Some(arg) = self.try_parse_one_command_arg()? else {
                let tok = *self.peek()?;
                return Err(self.error_at_token_with_suggestion(
                    &tok,
                    "Expected a value after comma".to_string(),
                    "Remove the extra comma, or add the missing value",
                ));
            };
            args.push(arg);

            if self.at(&TokenKind::Comma)? {
                self.next()?;
                if self.at_command_terminator()? || self.at(&TokenKind::Comma)? {
                    let tok = *self.peek()?;
                    return Err(self.error_at_token_with_suggestion(
                        &tok,
                        "Trailing or empty comma in list".to_string(),
                        "Remove the extra comma, or add the missing value",
                    ));
                }
                continue;
            }

            if self.command_arg_starts_without_comma()? {
                let tok = *self.peek()?;
                return Err(self.error_at_token_with_suggestion(
                    &tok,
                    "Expected a comma between items".to_string(),
                    "Separate items with commas, for example `-> options \"a\", \"b\"`",
                ));
            }
            break;
        }
        Ok(args)
    }

    fn command_arg_starts_without_comma(&mut self) -> Result<bool, Error> {
        if self.at_command_terminator()? || self.at(&TokenKind::Comma)? {
            return Ok(false);
        }
        let peek_kind = self.peek()?.kind;
        Ok(matches!(
            peek_kind,
            TokenKind::NumberLit | TokenKind::Minus | TokenKind::Plus | TokenKind::StringLit
        ) || is_boolean_keyword(&peek_kind)
            || can_be_label(&peek_kind))
    }

    fn parse_scalar_literal_value(&mut self) -> Result<Value, Error> {
        self.parse_scalar_literal_value_with_context("value (number, text, boolean, date, etc.)")
    }

    fn parse_scalar_literal_value_with_context(&mut self, expected: &str) -> Result<Value, Error> {
        let peeked = self.peek()?;
        match &peeked.kind {
            TokenKind::StringLit => {
                let tok = self.next()?;
                Ok(Value::Text(tok.text.of(&self.source).to_string()))
            }
            k if is_boolean_keyword(k) => {
                let tok = self.next()?;
                Ok(Value::Boolean(token_kind_to_boolean_value(&tok.kind)))
            }
            TokenKind::NumberLit => self.parse_number_literal(),
            TokenKind::Minus | TokenKind::Plus => self.parse_signed_number_literal(),
            _ => {
                let tok = self.next()?;
                Err(self.error_at_token(
                    &tok,
                    format!(
                        "Expected a {expected}, found '{}'",
                        tok.text.of(&self.source)
                    ),
                ))
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
        let mut summed_exponents: std::collections::BTreeMap<String, i32> =
            std::collections::BTreeMap::new();
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
            let peek_kind = self.peek()?.kind;
            if !can_be_unit_name(&peek_kind) {
                break;
            }

            // A label without a preceding operator ends the expression.
            // The first factor is permitted without an operator (operator_just_consumed starts true).
            if !operator_just_consumed {
                break;
            }
            operator_just_consumed = false;

            let (measure_ref, measure_span) = self.parse_unit_path()?;

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
                let raw: i32 = exp_tok.text.of(&self.source).parse::<i32>().map_err(|_| {
                    self.error_at_token(
                        &exp_tok,
                        format!(
                            "Exponent '{}' is not a valid integer in unit expression",
                            exp_tok.text.of(&self.source)
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

            let summed = summed_exponents
                .entry(crate::parsing::ast::canonical_logical_name(&measure_ref).into_owned())
                .or_insert(0i32);
            *summed = summed
                .checked_add(final_exp)
                .filter(|sum| sum.unsigned_abs() <= crate::limits::MAX_UNIT_EXPONENT.unsigned_abs())
                .ok_or_else(|| {
                    Error::parsing(
                        format!(
                            "Unit '{}' exceeds the maximum unit exponent of {} in this unit expression",
                            measure_ref,
                            crate::limits::MAX_UNIT_EXPONENT
                        ),
                        Source::new(self.source_type(), measure_span),
                        None::<String>,
                    )
                })?;

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
        let start_span = meta_tok.span;

        let key_tok = self.next()?;
        let key = key_tok.text.of(&self.source).to_string();

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
            let tok = *self.peek()?;
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
        let sign_span = sign_tok.span;
        let is_negative = sign_tok.kind == TokenKind::Minus;

        if !self.at(&TokenKind::NumberLit)? {
            let tok = *self.peek()?;
            return Err(self.error_at_token(
                &tok,
                format!(
                    "Expected a number after '{}', found '{}'",
                    sign_tok.text.of(&self.source),
                    tok.text.of(&self.source)
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
        let source = Arc::clone(&self.source);
        let num_text = num_tok.text.of(&source);
        let num_span = num_tok.span;

        if self.at(&TokenKind::Comma)? {
            let comma = *self.peek()?;
            let next = *self.lexer.peek_second()?;
            let digits_grouped = next.kind == TokenKind::NumberLit
                && comma.span.start == num_span.end
                && next.span.start == comma.span.end;
            if digits_grouped {
                let grouped = format!("{}_{}", num_text, next.text.of(&self.source));
                return Err(self.error_at_token_with_suggestion(
                    &comma,
                    format!(
                        "`,` is a list separator, not a digit separator. Write `{grouped}` instead of `{num_text},{}`",
                        next.text.of(&self.source)
                    ),
                    format!("Use `{grouped}`"),
                ));
            }
        }

        // Check if followed by - which could make it a date (YYYY-MM-DD)
        if num_text.len() == 4
            && num_text.chars().all(|c| c.is_ascii_digit())
            && self.at(&TokenKind::Minus)?
        {
            return self.parse_date_literal(num_text, num_span);
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
            return self.try_parse_time_literal(num_text, num_span);
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

        let starts_unit = can_be_unit_name(&peeked.kind);
        if starts_unit && !self.at_path_then_colon()? {
            let (unit_path, _) = self.parse_unit_path()?;
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
        if !can_be_unit_name(&first.kind) {
            return Err(self.error_at_token(
                &first,
                format!("Expected a unit name, found {}", first.kind),
            ));
        }
        let mut path = first.text.of(&self.source).to_string();
        let mut end_span = first.span;
        while self.at(&TokenKind::Dot)? {
            self.next()?;
            let seg = self.next()?;
            if !can_be_unit_name(&seg.kind) {
                return Err(self.error_at_token(
                    &seg,
                    format!("Expected a unit path segment after '.', found {}", seg.kind),
                ));
            }
            path.push('.');
            path.push_str(seg.text.of(&self.source));
            end_span = seg.span;
        }
        Ok((path, end_span))
    }

    fn parse_date_literal(&mut self, year_text: &str, start_span: Span) -> Result<Value, Error> {
        let mut dt_str = year_text.to_string();

        // Consume -MM
        self.expect(&TokenKind::Minus)?;
        dt_str.push('-');
        let month_tok = self.expect(&TokenKind::NumberLit)?;
        dt_str.push_str(month_tok.text.of(&self.source));

        // Consume -DD
        self.expect(&TokenKind::Minus)?;
        dt_str.push('-');
        let day_tok = self.expect(&TokenKind::NumberLit)?;
        dt_str.push_str(day_tok.text.of(&self.source));

        // Check for T (time component)
        if self.at(&TokenKind::Identifier)? {
            let peeked = *self.peek()?;
            if peeked.text.of(&self.source).len() >= 2
                && (peeked.text.of(&self.source).starts_with('T')
                    || peeked.text.of(&self.source).starts_with('t'))
            {
                // The lexer may have tokenized T14 as a single identifier
                let t_tok = self.next()?;
                dt_str.push_str(t_tok.text.of(&self.source));

                // Consume :MM
                if self.at(&TokenKind::Colon)? {
                    self.next()?;
                    dt_str.push(':');
                    let min_tok = self.next()?;
                    dt_str.push_str(min_tok.text.of(&self.source));

                    // Consume :SS and optional fractional second
                    if self.at(&TokenKind::Colon)? {
                        self.next()?;
                        dt_str.push(':');
                        let sec_tok = self.next()?;
                        dt_str.push_str(sec_tok.text.of(&self.source));

                        // Check for fractional second .NNNNNN
                        if self.at(&TokenKind::Dot)? {
                            self.next()?;
                            dt_str.push('.');
                            let frac_tok = self.expect(&TokenKind::NumberLit)?;
                            dt_str.push_str(frac_tok.text.of(&self.source));
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
            let peeked = *self.peek()?;
            if (peeked.text.of(&self.source) == "Z" || peeked.text.of(&self.source) == "z")
                && peeked.span.start == self.last_span.end
            {
                let z_tok = self.next()?;
                dt_str.push_str(z_tok.text.of(&self.source));
                return Ok(());
            }
        }

        // +HH:MM or -HH:MM, only when attached directly to the preceding token.
        if self.at(&TokenKind::Plus)? || self.at(&TokenKind::Minus)? {
            let checkpoint = self.lexer.checkpoint();
            let sign_tok = self.lexer.next_token()?;
            let hour_tok = self.lexer.next_token()?;
            let colon_tok = self.lexer.next_token()?;
            let minute_tok = self.lexer.next_token()?;

            let attached = sign_tok.span.start == self.last_span.end;
            let is_timezone_shape = hour_tok.kind == TokenKind::NumberLit
                && colon_tok.kind == TokenKind::Colon
                && minute_tok.kind == TokenKind::NumberLit;

            self.lexer.restore(checkpoint);

            if attached && is_timezone_shape {
                let sign_tok = self.next()?;
                dt_str.push_str(sign_tok.text.of(&self.source));
                let hour_tok = self.expect(&TokenKind::NumberLit)?;
                dt_str.push_str(hour_tok.text.of(&self.source));
                self.expect(&TokenKind::Colon)?;
                dt_str.push(':');
                let min_tok = self.expect(&TokenKind::NumberLit)?;
                dt_str.push_str(min_tok.text.of(&self.source));
            }
        }

        Ok(())
    }

    fn try_parse_time_literal(
        &mut self,
        hour_text: &str,
        start_span: Span,
    ) -> Result<Value, Error> {
        let mut time_str = hour_text.to_string();

        // Consume :MM
        self.expect(&TokenKind::Colon)?;
        time_str.push(':');
        let min_tok = self.expect(&TokenKind::NumberLit)?;
        time_str.push_str(min_tok.text.of(&self.source));

        // Optional :SS
        if self.at(&TokenKind::Colon)? {
            self.next()?;
            time_str.push(':');
            let sec_tok = self.expect(&TokenKind::NumberLit)?;
            time_str.push_str(sec_tok.text.of(&self.source));

            // Optional fractional second .NNNNNN
            if self.at(&TokenKind::Dot)? {
                self.next()?;
                time_str.push('.');
                let frac_tok = self.expect(&TokenKind::NumberLit)?;
                time_str.push_str(frac_tok.text.of(&self.source));
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
    // Expression grammar
    // ========================================================================

    fn expr_span(&self, id: ExprId) -> Option<Span> {
        self.table.source(id).map(|source| source.span)
    }

    fn expr_source(&self, id: ExprId) -> Option<Source> {
        self.table.source(id).cloned()
    }

    /// Ends one top-level expression: its nodes move into a fresh table owned by the result.
    fn finish_expression(&mut self, root: ExprId) -> Expression {
        ExprTree::new(self.table.take_compact(), root)
    }

    fn new_expression(&mut self, kind: ExpressionKind, source: Source) -> Result<ExprId, Error> {
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
        Ok(self.table.push(kind, Some(source)))
    }

    fn check_depth(&mut self) -> Result<(), Error> {
        if let Err(actual) = self.depth_tracker.push_depth() {
            let span = self.peek()?.span;
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
        operand: ExprId,
        operator_is_not: bool,
        keyword_was_negated: bool,
        start_span: Span,
    ) -> Result<ExprId, Error> {
        let negate = operator_is_not ^ keyword_was_negated;
        let end_span = self.expr_span(operand).unwrap_or(start_span);
        let span = self.span_covering(&start_span, &end_span);
        let core = self.new_expression(
            ExpressionKind::ResultIsVeto(operand),
            self.make_source(span),
        )?;
        if negate {
            self.new_expression(
                ExpressionKind::LogicalNegation(core, NegationType::Not),
                self.make_source(span),
            )
        } else {
            Ok(core)
        }
    }

    fn next_is_list_type_suffix(&mut self) -> Result<bool, Error> {
        let checkpoint = self.checkpoint();
        self.next()?;
        let list_after_range = if self.at(&TokenKind::Identifier)? && {
            let peeked = *self.peek()?;
            peeked.text.of(&self.source) == "range"
        } {
            self.next()?;
            self.at(&TokenKind::List)?
        } else {
            self.at(&TokenKind::List)?
        };
        self.restore(checkpoint);
        Ok(list_after_range)
    }

    fn wrap_list_type_suffix(&mut self, expr: ExprId, start_span: Span) -> Result<ExprId, Error> {
        if self.pin_before_list_ahead()? {
            let tok = *self.peek()?;
            return Err(self.error_at_token_with_suggestion(
                &tok,
                "An effective pin is not valid on a type or list element",
                "Write `uses Spec <pin>`, then `data name: Spec list` or `rule name: Spec list`.",
            ));
        }
        if !self.at_list_type_suffix()? {
            return Ok(expr);
        }
        let ExpressionKind::Reference(reference) = self.table.kind(expr).clone() else {
            return Ok(expr);
        };
        let mut parent = parent_type_from_reference(&reference);
        if self.at(&TokenKind::Identifier)? && {
            let peeked = *self.peek()?;
            peeked.text.of(&self.source) == "range"
        } {
            self.next()?;
            parent = ParentType::Ranged {
                inner: Box::new(parent),
            };
        }
        self.expect(&TokenKind::List)?;
        parent = ParentType::List {
            inner: Box::new(parent),
        };
        let span = self.span_covering(&start_span, &self.last_span);
        *self.table.kind_mut(expr) = ExpressionKind::ListType { element: parent };
        self.table.set_source(expr, self.make_source(span));
        Ok(expr)
    }

    fn pin_before_list_ahead(&mut self) -> Result<bool, Error> {
        if !self.at(&TokenKind::NumberLit)? {
            return Ok(false);
        }
        let year = *self.peek()?;
        if year.text.of(&self.source).len() != 4
            || !year
                .text
                .of(&self.source)
                .chars()
                .all(|c| c.is_ascii_digit())
        {
            return Ok(false);
        }
        let checkpoint = self.checkpoint();
        self.next()?;
        let list = self.at(&TokenKind::List)?;
        self.restore(checkpoint);
        Ok(list)
    }

    fn at_list_type_suffix(&mut self) -> Result<bool, Error> {
        if self.at(&TokenKind::List)? {
            return Ok(true);
        }
        if self.at(&TokenKind::Identifier)? && {
            let peeked = *self.peek()?;
            peeked.text.of(&self.source) == "range"
        } {
            let checkpoint = self.checkpoint();
            self.next()?;
            let list = self.at(&TokenKind::List)?;
            self.restore(checkpoint);
            return Ok(list);
        }
        Ok(false)
    }

    fn parse_expression_parent_type(&mut self) -> Result<ParentType, Error> {
        let peek = *self.peek()?;
        let base = if let Some(primitive) = token_kind_to_primitive(&peek.kind) {
            self.next()?;
            ParentType::Primitive { primitive }
        } else {
            self.parse_custom_or_spec_element_parent()?
        };
        let base = if self.at(&TokenKind::Identifier)? && {
            let peeked = *self.peek()?;
            peeked.text.of(&self.source) == "range"
        } {
            self.next()?;
            ParentType::Ranged {
                inner: Box::new(base),
            }
        } else {
            base
        };
        self.expect(&TokenKind::List)?;
        Ok(ParentType::List {
            inner: Box::new(base),
        })
    }

    /// A token that starts a bare (unparenthesised) command argument. Never a sign:
    /// after a command, `+` / `-` is the binary operator. Never `is`: `items -> count is 0`.
    fn at_bare_operand_start(&mut self) -> Result<bool, Error> {
        let kind = self.peek()?.kind;
        if kind == TokenKind::Is {
            return Ok(false);
        }
        Ok(matches!(
            kind,
            TokenKind::NumberLit
                | TokenKind::StringLit
                | TokenKind::Now
                | TokenKind::Past
                | TokenKind::Future
                | TokenKind::Lower
                | TokenKind::Upper
        ) || is_boolean_keyword(&kind)
            || is_math_function(&kind)
            || can_be_label(&kind)
            || token_kind_to_primitive(&kind).is_some())
    }

    fn at_command_argument_start(&mut self) -> Result<bool, Error> {
        Ok(self.at(&TokenKind::LParen)? || self.at_bare_operand_start()?)
    }

    /// Tokens that may follow a bare argument. Anything else (an operator) is ambiguous.
    fn at_bare_argument_end(&mut self) -> Result<bool, Error> {
        Ok(self.at_any(&[
            TokenKind::Arrow,
            TokenKind::Then,
            TokenKind::Unless,
            TokenKind::RParen,
            TokenKind::Comma,
            TokenKind::Eof,
            TokenKind::Spec,
            TokenKind::Repo,
            TokenKind::Uses,
        ])? || is_spec_body_keyword(&self.peek()?.kind))
    }

    fn bare_argument_error(&mut self, command: &str) -> Error {
        let tok = match self.peek() {
            Ok(tok) => *tok,
            Err(error) => return error,
        };
        self.error_at_token(
            &tok,
            format!(
                "`{command}` without parentheses takes one operand, found '{}' after it; \
                 write `{command}(...)` around the whole argument, or `(... -> {command} ...)` \
                 to use the result",
                tok.text.of(&self.source)
            ),
        )
    }

    fn reject_comma_after_call(&mut self, command: &str) -> Result<(), Error> {
        if self.call_argument_depth == 0 && self.at(&TokenKind::Comma)? {
            let tok = *self.peek()?;
            return Err(self.error_at_token(
                &tok,
                format!(
                    "`,` after `{command}(...)`: put all arguments inside the parentheses, \
                     `{command}(a, b)`"
                ),
            ));
        }
        Ok(())
    }

    fn reject_command_argument(&mut self, command: &str) -> Result<(), Error> {
        if self.at_command_argument_start()? {
            let tok = *self.peek()?;
            return Err(self.error_at_token(&tok, format!("`{command}` takes no argument")));
        }
        Ok(())
    }

    fn at_row_condition_operator(&mut self) -> Result<bool, Error> {
        Ok(is_comparison_operator(&self.peek()?.kind) || self.at(&TokenKind::In)?)
    }

    fn parse_row_condition_operator(&mut self) -> Result<RowCondition, Error> {
        if self.at(&TokenKind::In)? {
            self.next()?;
            return Ok(RowCondition::In);
        }
        Ok(RowCondition::Compare(self.parse_comparison_operator()?))
    }

    fn condition_error(&mut self, command: &str) -> Error {
        let tok = match self.peek() {
            Ok(tok) => *tok,
            Err(error) => return error,
        };
        self.error_at_token(
            &tok,
            format!(
                "`{command}` condition without parentheses is one comparison between single \
                 operands, found '{}'; write `{command}(...)` around the whole condition, \
                 for example `{command}(> 5 or is 3)`",
                tok.text.of(&self.source)
            ),
        )
    }

    /// `asc` when omitted.
    fn parse_sort_direction(&mut self) -> Result<bool, Error> {
        if self.at(&TokenKind::Desc)? {
            self.next()?;
            return Ok(true);
        }
        if self.at(&TokenKind::Asc)? {
            self.next()?;
        }
        Ok(false)
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
        repository: ExprId,
        start_span: Span,
    ) -> Result<ExprId, Error> {
        self.expect(&TokenKind::Not)?;
        self.expect(&TokenKind::In)?;
        self.expect_calendar_period_marker()?;
        let unit = self.parse_calendar_unit()?;
        let end = self.peek()?.span;
        let span = self.span_covering(&start_span, &end);
        self.new_expression(
            ExpressionKind::DateCalendar(DateCalendarKind::NotIn, unit, repository),
            self.make_source(span),
        )
    }

    fn parse_as_chain(&mut self, mut expr: ExprId, start_span: Span) -> Result<ExprId, Error> {
        while self.at(&TokenKind::As)? {
            self.expect(&TokenKind::As)?;
            let target_tok = self.next()?;
            let target = if matches!(target_tok.kind, TokenKind::Permille) {
                ConversionTarget::Unit {
                    unit_name: "permille".to_string(),
                }
            } else if matches!(target_tok.kind, TokenKind::TextKw) {
                return Err(self.error_at_token_with_suggestion(
                    &target_tok,
                    "'text' is not a valid conversion target.".to_string(),
                    "Use `as number`, `as boolean`, `as date`, `as time`, `as measure`, `as ratio`, or a unit name (e.g. `as eur`).".to_string(),
                ));
            } else if let Some(kind) = token_kind_to_conversion_type_kind(&target_tok.kind) {
                ConversionTarget::Type(kind)
            } else if can_be_label(&target_tok.kind) {
                let mut unit_path = target_tok.text.of(&self.source).to_string();
                let mut end_span = target_tok.span;
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
                    unit_path.push_str(seg.text.of(&self.source));
                    end_span = seg.span;
                }
                let target = ConversionTarget::Unit {
                    unit_name: unit_path,
                };
                expr = self.new_expression(
                    ExpressionKind::UnitConversion(expr, target),
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
                ExpressionKind::UnitConversion(expr, target),
                self.make_source(self.span_covering(&start_span, &target_tok.span)),
            )?;
        }
        Ok(expr)
    }

    fn is_plain_number_literal(&self, id: ExprId) -> bool {
        matches!(
            self.table.kind(id),
            ExpressionKind::Literal(Value::Number(_))
        )
    }

    fn is_unit_conversion(&self, id: ExprId) -> bool {
        matches!(self.table.kind(id), ExpressionKind::UnitConversion(..))
    }

    /// True when the next token can follow a completed suffix expression (no further operands).
    ///
    /// Must include every token that can start the next spec-body item, a new `spec`/`repo`,
    /// or end the file. See `parse_unit_conversion_before_expression_boundaries` in `parsing/mod.rs`.
    fn at_expression_suffix_end(&mut self) -> Result<bool, Error> {
        Ok(self.at_any(&[TokenKind::And, TokenKind::Or])?
            || self.at(&TokenKind::Unless)?
            || self.at(&TokenKind::Then)?
            || self.at(&TokenKind::RParen)?
            || self.at(&TokenKind::Eof)?
            || self.at(&TokenKind::Arrow)?
            || self.at(&TokenKind::Comma)?
            || self.at(&TokenKind::Spec)?
            || self.at(&TokenKind::Repo)?
            || self.at(&TokenKind::Uses)?
            || is_spec_body_keyword(&self.peek()?.kind))
    }

    fn parse_calendar_unit(&mut self) -> Result<CalendarPeriodUnit, Error> {
        let tok = self.next()?;
        if let Some(unit) = CalendarPeriodUnit::from_keyword(tok.text.of(&self.source)) {
            return Ok(unit);
        }
        Err(self.error_at_token(
            &tok,
            format!(
                "Expected 'year', 'month', or 'week', found '{}'",
                tok.text.of(&self.source)
            ),
        ))
    }

    // ========================================================================
    // Arithmetic and range operands
    // ========================================================================

    /// One left fold over `steps`. Without steps the operand itself is the result.
    fn arithmetic_chain(
        &mut self,
        first: ExprId,
        steps: Vec<ChainStep<ArithmeticComputation>>,
    ) -> Result<ExprId, Error> {
        let source = match steps.last() {
            None => return Ok(first),
            Some(last) => last.source_location.clone(),
        };
        self.new_expression(ExpressionKind::Arithmetic { first, steps }, source)
    }

    fn parse_number_expression(&mut self) -> Result<ExprId, Error> {
        let num_tok = self.next()?;
        let source = Arc::clone(&self.source);
        let num_text = num_tok.text.of(&source);
        let start_span = num_tok.span;

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
            let minus_span = self.peek()?.span;
            // If minus is immediately adjacent to the number (no space), it's a date
            if minus_span.start == start_span.end {
                let value = self.parse_date_literal(num_text, start_span)?;
                return self
                    .new_expression(ExpressionKind::Literal(value), self.make_source(start_span));
            }
        }

        // Check for time literal (HH:MM:SS)
        if num_text.len() == 2
            && num_text.chars().all(|c| c.is_ascii_digit())
            && self.at(&TokenKind::Colon)?
        {
            let colon_span = self.peek()?.span;
            if colon_span.start == start_span.end {
                let value = self.try_parse_time_literal(num_text, start_span)?;
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
            let decimal = parse_decimal_string(num_text, &start_span, self)?;
            return self.new_expression(
                ExpressionKind::Literal(Value::NumberWithUnit(decimal, "permille".to_string())),
                self.make_source(start_span),
            );
        }

        // Check for % (percent)
        if self.at(&TokenKind::Percent)? {
            let pct_span = self.peek()?.span;
            // Only consume % if it's directly adjacent (no space) for the shorthand syntax
            // Or if it's "50 %" (space separated is also valid per the grammar)
            let pct_tok = self.next()?;
            if let Ok(next_peek) = self.peek() {
                if next_peek.kind == TokenKind::NumberLit || next_peek.kind == TokenKind::Percent {
                    return Err(self.error_at_token(
                        &pct_tok,
                        "Percent literal cannot be followed by a digit (use 'mod' for remainder)",
                    ));
                }
            }
            let decimal = parse_decimal_string(num_text, &start_span, self)?;
            return self.new_expression(
                ExpressionKind::Literal(Value::NumberWithUnit(decimal, "percent".to_string())),
                self.make_source(self.span_covering(&start_span, &pct_span)),
            );
        }

        // Check for "permille" keyword
        if self.at(&TokenKind::Permille)? {
            self.next()?;
            let decimal = parse_decimal_string(num_text, &start_span, self)?;
            return self.new_expression(
                ExpressionKind::Literal(Value::NumberWithUnit(decimal, "permille".to_string())),
                self.make_source(start_span),
            );
        }

        if can_be_unit_name(&self.peek()?.kind) {
            let (unit_path, end_span) = self.parse_unit_path()?;
            let decimal = parse_decimal_string(num_text, &start_span, self)?;
            return self.new_expression(
                ExpressionKind::Literal(Value::NumberWithUnit(decimal, unit_path)),
                self.make_source(self.span_covering(&start_span, &end_span)),
            );
        }

        // Plain number
        let decimal = parse_decimal_string(num_text, &start_span, self)?;
        self.new_expression(
            ExpressionKind::Literal(Value::Number(decimal)),
            self.make_source(start_span),
        )
    }

    fn parse_expression_reference(&mut self) -> Result<Reference, Error> {
        let mut segments = Vec::new();

        let first = self.next()?;
        segments.push(first.text.of(&self.source).to_string());

        while self.at(&TokenKind::Dot)? {
            self.next()?; // consume .
            let seg = self.next()?;
            if !can_be_label(&seg.kind) {
                return Err(self.error_at_token(
                    &seg,
                    format!("Expected an identifier after '.', found {}", seg.kind),
                ));
            }
            segments.push(seg.text.of(&self.source).to_string());
        }

        Ok(Reference::from_path(segments))
    }
    fn parse_expression(&mut self) -> Result<ExprId, Error> {
        Ok(expr_of(self.run_expression_machine(ExprTask::Expression)?))
    }

    fn parse_list_command_body(&mut self, name_tok: Token) -> Result<ListCommand, Error> {
        Ok(command_of(self.run_expression_machine(
            ExprTask::ListCommandBody { name: name_tok },
        )?))
    }

    fn save_flags(&self) -> ExprFlags {
        ExprFlags {
            call_argument_depth: self.call_argument_depth,
        }
    }

    fn run_expression_machine(&mut self, root: ExprTask) -> Result<ExprVal, Error> {
        let mut frames = std::mem::take(&mut self.expr_frames);
        let result = self.drive_expression_machine(root, &mut frames);
        self.expr_frames = frames;
        result
    }

    fn drive_expression_machine(
        &mut self,
        root: ExprTask,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprVal, Error> {
        let mut step = ExprStep::Enter(root);
        loop {
            let next = match step {
                ExprStep::Enter(task) => self.enter_expression(task, frames),
                ExprStep::Yield(value) => match frames.pop() {
                    None => return Ok(value),
                    Some(cont) => self.resume_expression(cont, value, frames),
                },
            };
            match next {
                Ok(next_step) => step = next_step,
                Err(error) => {
                    self.unwind_expression(frames);
                    return Err(error);
                }
            }
        }
    }

    fn unwind_expression(&mut self, frames: &mut Vec<ExprCont>) {
        while let Some(cont) = frames.pop() {
            match cont {
                ExprCont::Restore(flags) => {
                    self.call_argument_depth = flags.call_argument_depth;
                }
                ExprCont::PopDepth => self.depth_tracker.pop_depth(),
                _ => {}
            }
        }
    }

    fn enter_expression(
        &mut self,
        task: ExprTask,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        match task {
            ExprTask::Expression => {
                self.check_depth()?;
                frames.push(ExprCont::PopDepth);
                Ok(ExprStep::Enter(ExprTask::LogicalExpression {
                    connective: LogicalConnective::Or,
                }))
            }
            ExprTask::LogicalExpression { connective } => {
                let start_span = self.peek()?.span;
                let kind = LogicalChainKind::Expression;
                frames.push(ExprCont::LogicalFirst {
                    connective,
                    start_span,
                    kind,
                });
                Ok(ExprStep::Enter(logical_operand_task(connective, kind)))
            }
            ExprTask::LogicalOperand => {
                if self.at_not_bare_veto_followed_by_is()? || self.at_bare_veto_followed_by_is()? {
                    Ok(ExprStep::Enter(ExprTask::VetoStatus))
                } else if self.at(&TokenKind::Not)? {
                    Ok(ExprStep::Enter(ExprTask::NotExpression))
                } else {
                    Ok(ExprStep::Enter(ExprTask::RepositoryWithSuffix))
                }
            }
            ExprTask::VetoStatus => {
                let start_span = self.peek()?.span;
                let keyword_was_negated = if self.at(&TokenKind::Not)? {
                    self.next()?;
                    true
                } else {
                    false
                };
                self.expect(&TokenKind::Veto)?;
                if self.at(&TokenKind::StringLit)? {
                    let tok = *self.peek()?;
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
                    let tok = *self.peek()?;
                    return Err(self.error_at_token(
                        &tok,
                        "Expected `is` or `is not` after `veto` in a veto-status comparison",
                    ));
                }
                frames.push(ExprCont::VetoStatus {
                    operator_is_not,
                    keyword_was_negated,
                    start_span,
                });
                Ok(ExprStep::Enter(ExprTask::Pipe))
            }
            ExprTask::NotExpression => {
                let not_tok = self.expect(&TokenKind::Not)?;
                frames.push(ExprCont::Negation {
                    start_span: not_tok.span,
                });
                Ok(ExprStep::Enter(ExprTask::PrefixArgument {
                    keyword: "not".to_string(),
                }))
            }
            ExprTask::RepositoryWithSuffix => {
                let start_span = self.peek()?.span;
                frames.push(ExprCont::RepositorySuffix { start_span });
                Ok(ExprStep::Enter(ExprTask::Pipe))
            }
            ExprTask::Pipe => {
                let start_span = self.peek()?.span;
                frames.push(ExprCont::PipeFirst { start_span });
                Ok(ExprStep::Enter(ExprTask::RepositoryExpression))
            }
            ExprTask::ComparisonSuffix { left, start_span } => {
                self.enter_comparison_suffix(left, start_span, frames)
            }
            ExprTask::InSuffix {
                repository,
                start_span,
            } => self.enter_in_suffix(repository, start_span, frames),
            ExprTask::RepositoryExpression => {
                let start_span = self.peek()?.span;
                frames.push(ExprCont::RepositoryFirst { start_span });
                Ok(ExprStep::Enter(ExprTask::Term { allow_as: true }))
            }
            ExprTask::Term { allow_as } => {
                let start_span = self.peek()?.span;
                frames.push(ExprCont::TermFirst {
                    allow_as,
                    start_span,
                });
                Ok(ExprStep::Enter(ExprTask::Power))
            }
            ExprTask::Power => {
                let start_span = self.peek()?.span;
                frames.push(ExprCont::PowerLeft {
                    start_span,
                    range_bound: false,
                });
                Ok(ExprStep::Enter(ExprTask::RangeOperand))
            }
            ExprTask::RangeOperand => {
                let start_span = self.peek()?.span;
                let flags = self.save_flags();
                if self.no_range_at.get(&start_span.start) == Some(&flags) {
                    return Ok(ExprStep::Enter(ExprTask::Factor));
                }
                frames.push(ExprCont::RangeFactor { start_span, flags });
                Ok(ExprStep::Enter(ExprTask::Factor))
            }
            ExprTask::RangeEllipsisBound => {
                let start_span = self.peek()?.span;
                frames.push(ExprCont::EllipsisFirst { start_span });
                Ok(ExprStep::Enter(ExprTask::PowerForRangeBound))
            }
            ExprTask::PowerForRangeBound => {
                let start_span = self.peek()?.span;
                frames.push(ExprCont::PowerLeft {
                    start_span,
                    range_bound: true,
                });
                Ok(ExprStep::Enter(ExprTask::Factor))
            }
            ExprTask::Factor => {
                let peeked = self.peek()?;
                let start_span = peeked.span;
                let kind = peeked.kind;
                if kind == TokenKind::Minus {
                    self.next()?;
                    frames.push(ExprCont::FactorNegate { start_span });
                    return Ok(ExprStep::Enter(ExprTask::PrimaryOrMath));
                }
                if kind == TokenKind::Plus {
                    self.next()?;
                }
                Ok(ExprStep::Enter(ExprTask::PrimaryOrMath))
            }
            ExprTask::PrimaryOrMath => {
                let kind = self.peek()?.kind;
                if is_prefix_rounding(&kind) {
                    return Ok(ExprStep::Enter(ExprTask::PrefixRoundingRejected));
                }
                if is_math_function(&kind) {
                    return Ok(ExprStep::Enter(ExprTask::MathFunction));
                }
                if matches!(kind, TokenKind::Lower | TokenKind::Upper) {
                    return Ok(ExprStep::Enter(ExprTask::LowerUpper));
                }
                Ok(ExprStep::Enter(ExprTask::Primary))
            }
            ExprTask::PrefixRoundingRejected => {
                let tok = self.next()?;
                let keyword = tok.text.of(&self.source).to_string();
                frames.push(ExprCont::PrefixRoundingRejected {
                    keyword: keyword.clone(),
                    keyword_span: tok.span,
                });
                Ok(ExprStep::Enter(ExprTask::PrefixArgument { keyword }))
            }
            ExprTask::MathFunction => {
                let func_tok = self.next()?;
                let start_span = func_tok.span;
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
                    _ => unreachable!("BUG: only math functions should reach here"),
                };
                frames.push(ExprCont::Math {
                    operator,
                    start_span,
                });
                Ok(ExprStep::Enter(ExprTask::PrefixArgument {
                    keyword: func_tok.text.of(&self.source).to_string(),
                }))
            }
            ExprTask::LowerUpper => {
                let bound_tok = self.next()?;
                let start_span = bound_tok.span;
                let bound = match bound_tok.kind {
                    TokenKind::Lower => RangeBound::Lower,
                    TokenKind::Upper => RangeBound::Upper,
                    _ => unreachable!("BUG: only lower and upper should reach parse_range_bound"),
                };
                frames.push(ExprCont::LowerUpperEnd { bound, start_span });
                Ok(ExprStep::Enter(ExprTask::PrefixArgument {
                    keyword: bound_tok.text.of(&self.source).to_string(),
                }))
            }
            ExprTask::PrefixArgument { keyword } => self.enter_prefix_argument(keyword, frames),
            ExprTask::Primary => self.enter_primary(frames),
            ExprTask::ListCommandSuffix { list, start_span } => {
                self.expect(&TokenKind::Arrow)?;
                let name = self.next()?;
                frames.push(ExprCont::ListCommandDone { list, start_span });
                Ok(ExprStep::Enter(ExprTask::ListCommandBody { name }))
            }
            ExprTask::ListCommandBody { name } => self.enter_list_command_body(name, frames),
            ExprTask::FindKeys => {
                if self.at(&TokenKind::LParen)? {
                    return Ok(ExprStep::Enter(ExprTask::CallArguments { command: "find" }));
                }
                if !self.at_bare_operand_start()? {
                    let tok = *self.peek()?;
                    return Err(self.error_at_token(&tok, "`find` needs a key"));
                }
                frames.push(ExprCont::FindKey { keys: Vec::new() });
                Ok(ExprStep::Enter(ExprTask::BareArgument { command: "find" }))
            }
            ExprTask::CallArguments { command } => {
                self.expect(&TokenKind::LParen)?;
                let saved = self.save_flags();
                self.call_argument_depth += 1;
                frames.push(ExprCont::CallArgumentsEnd { command });
                frames.push(ExprCont::Restore(saved));
                frames.push(ExprCont::CallArgumentItems {
                    arguments: Vec::new(),
                });
                Ok(ExprStep::Enter(ExprTask::Expression))
            }
            ExprTask::SingleCallArgument { command } => {
                frames.push(ExprCont::SingleCallArgumentCheck { command });
                Ok(ExprStep::Enter(ExprTask::CallArguments { command }))
            }
            ExprTask::OptionalArgument { command } => {
                if self.at(&TokenKind::LParen)? {
                    frames.push(ExprCont::WrapSome);
                    return Ok(ExprStep::Enter(ExprTask::SingleCallArgument { command }));
                }
                if self.at_bare_operand_start()? {
                    frames.push(ExprCont::WrapSome);
                    return Ok(ExprStep::Enter(ExprTask::BareArgument { command }));
                }
                Ok(ExprStep::Yield(ExprVal::Optional(None)))
            }
            ExprTask::RequiredArgument { command } => {
                if self.at(&TokenKind::LParen)? {
                    return Ok(ExprStep::Enter(ExprTask::SingleCallArgument { command }));
                }
                if self.at_bare_operand_start()? {
                    return Ok(ExprStep::Enter(ExprTask::BareArgument { command }));
                }
                let tok = *self.peek()?;
                Err(self.error_at_token(&tok, format!("`{command}` needs an argument")))
            }
            ExprTask::BareArgument { command } => {
                frames.push(ExprCont::BareArgumentEnd { command });
                Ok(ExprStep::Enter(ExprTask::BareOperand))
            }
            ExprTask::BareOperand => Ok(ExprStep::Enter(ExprTask::RangeOperand)),
            ExprTask::ListPredicate { command } => self.enter_list_predicate(command, frames),
            ExprTask::CallListPredicate { command } => {
                let checkpoint = self.checkpoint();
                self.expect(&TokenKind::LParen)?;
                if !self.starts_value_predicate()? {
                    self.restore(checkpoint);
                    frames.push(ExprCont::WrapInstance);
                    return Ok(ExprStep::Enter(ExprTask::SingleCallArgument { command }));
                }
                frames.push(ExprCont::ValuePredicateCallEnd { command });
                Ok(ExprStep::Enter(ExprTask::ValuePredicate {
                    command,
                    connective: LogicalConnective::Or,
                }))
            }
            ExprTask::ValuePredicate {
                command,
                connective,
            } => {
                let start_span = self.peek()?.span;
                let kind = LogicalChainKind::ValuePredicate { command };
                frames.push(ExprCont::LogicalFirst {
                    connective,
                    start_span,
                    kind,
                });
                Ok(ExprStep::Enter(logical_operand_task(connective, kind)))
            }
            ExprTask::ValuePredicateFactor { command } => {
                self.enter_value_predicate_factor(command, frames)
            }
            ExprTask::SortCommand => {
                if self.at(&TokenKind::LParen)? {
                    frames.push(ExprCont::SortArgument { check_bare: false });
                    return Ok(ExprStep::Enter(ExprTask::SingleCallArgument {
                        command: "sort",
                    }));
                }
                if self.at_bare_operand_start()? {
                    frames.push(ExprCont::SortArgument { check_bare: true });
                    return Ok(ExprStep::Enter(ExprTask::BareOperand));
                }
                let descending = self.parse_sort_direction()?;
                Ok(ExprStep::Yield(ExprVal::Command(ListCommand::Sort {
                    descending,
                    argument: None,
                })))
            }
            ExprTask::ListWithCommand => {
                if !self.at(&TokenKind::LParen)? {
                    return Ok(ExprStep::Enter(ExprTask::ListWithRows));
                }
                self.next()?;
                let saved = self.save_flags();
                self.call_argument_depth += 1;
                frames.push(ExprCont::ListWithEnd);
                frames.push(ExprCont::Restore(saved));
                Ok(ExprStep::Enter(ExprTask::ListWithRows))
            }
            ExprTask::ListWithRows => {
                if self.at_path_then_colon()? {
                    return self.enter_pair_field(Vec::new(), frames);
                }
                frames.push(ExprCont::ValueItem { values: Vec::new() });
                Ok(ExprStep::Enter(ExprTask::BareArgument { command: "with" }))
            }
        }
    }

    fn resume_expression(
        &mut self,
        cont: ExprCont,
        value: ExprVal,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        match cont {
            ExprCont::Restore(flags) => {
                self.call_argument_depth = flags.call_argument_depth;
                Ok(ExprStep::Yield(value))
            }
            ExprCont::PopDepth => {
                self.depth_tracker.pop_depth();
                Ok(ExprStep::Yield(value))
            }
            ExprCont::LogicalFirst {
                connective,
                start_span,
                kind,
            } => self.logical_chain(
                connective,
                expr_of(value),
                Vec::new(),
                start_span,
                kind,
                frames,
            ),
            ExprCont::LogicalOperand {
                connective,
                first,
                mut steps,
                start_span,
                kind,
            } => {
                let operand = expr_of(value);
                let end_span = self.expr_span(operand).unwrap_or(start_span);
                let span = self.span_covering(&start_span, &end_span);
                steps.push(ChainStep {
                    op: (),
                    operand,
                    source_location: self.make_source(span),
                });
                self.logical_chain(connective, first, steps, start_span, kind, frames)
            }
            ExprCont::VetoStatus {
                operator_is_not,
                keyword_was_negated,
                start_span,
            } => expr_step(self.wrap_result_is_veto_expression(
                expr_of(value),
                operator_is_not,
                keyword_was_negated,
                start_span,
            )),
            ExprCont::Negation { start_span } => {
                let operand = expr_of(value);
                if self.at(&TokenKind::Arrow)? {
                    let tok = *self.peek()?;
                    let operand_text = display_expression(&self.table, operand);
                    return Err(self.error_at_token(
                        &tok,
                        format!(
                            "`not` without parentheses takes one operand, found '->' after it; \
                             write `not ({operand_text} -> ...)` to negate the command's result"
                        ),
                    ));
                }
                let end_span = self.expr_span(operand).unwrap_or(start_span);
                let span = self.span_covering(&start_span, &end_span);
                expr_step(self.new_expression(
                    ExpressionKind::LogicalNegation(operand, NegationType::Not),
                    self.make_source(span),
                ))
            }
            ExprCont::RepositorySuffix { start_span } => {
                self.continue_repository(expr_of(value), start_span, frames)
            }
            ExprCont::ContinueRepository { start_span } => {
                self.continue_repository(expr_of(value), start_span, frames)
            }
            ExprCont::ComparisonRight {
                left,
                operator,
                start_span,
            } => {
                let right = expr_of(value);
                let end_span = self.expr_span(right).unwrap_or(start_span);
                let span = self.span_covering(&start_span, &end_span);
                expr_step(self.new_expression(
                    ExpressionKind::Comparison(left, operator, right),
                    self.make_source(span),
                ))
            }
            ExprCont::InOffset {
                direction,
                rel_kind,
                repository,
                start_span,
            } => {
                let offset = expr_of(value);
                let offset_end_span = self.expr_span(offset).unwrap_or(start_span);
                let range = self.new_expression(
                    ExpressionKind::PastFutureRange(rel_kind, offset),
                    self.make_source(self.span_covering(&direction, &offset_end_span)),
                )?;
                let span = self.span_covering(&start_span, &offset_end_span);
                expr_step(self.new_expression(
                    ExpressionKind::RangeContainment(repository, range),
                    self.make_source(span),
                ))
            }
            ExprCont::InRange {
                repository,
                start_span,
            } => {
                let range = expr_of(value);
                let end_span = self.expr_span(range).unwrap_or(start_span);
                let span = self.span_covering(&start_span, &end_span);
                expr_step(self.new_expression(
                    ExpressionKind::RangeContainment(repository, range),
                    self.make_source(span),
                ))
            }
            ExprCont::RepositoryFirst { start_span } => {
                self.repository_chain(expr_of(value), Vec::new(), start_span, frames)
            }
            ExprCont::RepositoryOperand {
                first,
                mut steps,
                start_span,
                operation,
            } => {
                let operand = expr_of(value);
                if steps.is_empty()
                    && self.is_plain_number_literal(first)
                    && self.is_unit_conversion(operand)
                {
                    let source = self
                        .expr_source(operand)
                        .unwrap_or_else(|| self.make_source(start_span));
                    return Err(Error::parsing(
                        "Cannot add a plain number to a converted value; convert each operand before \
                         '+' (e.g. '5 as usd + c as usd')",
                        source,
                        None::<String>,
                    ));
                }
                let end_span = self.expr_span(operand).unwrap_or(start_span);
                let span = self.span_covering(&start_span, &end_span);
                steps.push(ChainStep {
                    op: operation,
                    operand,
                    source_location: self.make_source(span),
                });
                self.repository_chain(first, steps, start_span, frames)
            }
            ExprCont::TermFirst {
                allow_as,
                start_span,
            } => {
                let mut first = expr_of(value);
                if allow_as {
                    first = self.parse_as_chain(first, start_span)?;
                }
                self.term_chain(first, Vec::new(), allow_as, start_span, frames)
            }
            ExprCont::TermOperand {
                first,
                mut steps,
                allow_as,
                start_span,
                operation,
                right_start_span,
            } => {
                let mut operand = expr_of(value);
                if allow_as {
                    operand = self.parse_as_chain(operand, right_start_span)?;
                }
                let end_span = self.expr_span(operand).unwrap_or(start_span);
                let span = self.span_covering(&start_span, &end_span);
                steps.push(ChainStep {
                    op: operation,
                    operand,
                    source_location: self.make_source(span),
                });
                self.term_chain(first, steps, allow_as, start_span, frames)
            }
            ExprCont::PowerLeft {
                start_span,
                range_bound,
            } => self.power_tail(expr_of(value), start_span, range_bound, frames),
            ExprCont::PowerRight { left, start_span } => {
                let right = expr_of(value);
                let end_span = self.expr_span(right).unwrap_or(start_span);
                let span = self.span_covering(&start_span, &end_span);
                let step = ChainStep {
                    op: ArithmeticComputation::Power,
                    operand: right,
                    source_location: self.make_source(span),
                };
                expr_step(self.arithmetic_chain(left, vec![step]))
            }
            ExprCont::RangeFactor { start_span, flags } => {
                let factor = expr_of(value);
                if self.at(&TokenKind::Ellipsis)? {
                    self.next()?;
                    frames.push(ExprCont::RangeRight {
                        left: factor,
                        start_span,
                    });
                    return Ok(ExprStep::Enter(ExprTask::RangeEllipsisBound));
                }
                if !self.at_any(&[TokenKind::Caret, TokenKind::Plus, TokenKind::Minus])? {
                    return Ok(ExprStep::Yield(ExprVal::Expr(factor)));
                }
                frames.push(ExprCont::RangeLeft {
                    factor,
                    start_span,
                    checkpoint: Box::new(self.checkpoint()),
                    flags,
                });
                frames.push(ExprCont::EllipsisFirst { start_span });
                self.power_tail(factor, start_span, true, frames)
            }
            ExprCont::RangeLeft {
                factor,
                start_span,
                checkpoint,
                flags,
            } => {
                let left = expr_of(value);
                let starts = std::mem::take(&mut self.bound_chain_starts);
                if !self.at(&TokenKind::Ellipsis)? {
                    for start in starts {
                        self.no_range_at.insert(start, flags);
                    }
                    self.restore(*checkpoint);
                    return Ok(ExprStep::Yield(ExprVal::Expr(factor)));
                }
                self.next()?;
                frames.push(ExprCont::RangeRight { left, start_span });
                Ok(ExprStep::Enter(ExprTask::RangeEllipsisBound))
            }
            ExprCont::RangeRight { left, start_span } => {
                let right = expr_of(value);
                let end_span = self.expr_span(right).unwrap_or(start_span);
                let span = self.span_covering(&start_span, &end_span);
                expr_step(self.new_expression(
                    ExpressionKind::RangeLiteral(left, right),
                    self.make_source(span),
                ))
            }
            ExprCont::EllipsisFirst { start_span } => {
                let starts = vec![start_span.start];
                self.ellipsis_chain(expr_of(value), Vec::new(), start_span, starts, frames)
            }
            ExprCont::EllipsisOperand {
                first,
                mut steps,
                start_span,
                operation,
                starts,
            } => {
                let operand = expr_of(value);
                let end_span = self.expr_span(operand).unwrap_or(start_span);
                let span = self.span_covering(&start_span, &end_span);
                steps.push(ChainStep {
                    op: operation,
                    operand,
                    source_location: self.make_source(span),
                });
                self.ellipsis_chain(first, steps, start_span, starts, frames)
            }
            ExprCont::FactorNegate { start_span } => {
                let operand = expr_of(value);
                let end_span = self.expr_span(operand).unwrap_or(start_span);
                let span = self.span_covering(&start_span, &end_span);
                if let ExpressionKind::Literal(literal) = self.table.kind(operand) {
                    if let Ok(negated) = try_negate_numeric_literal(literal.clone()) {
                        return expr_step(self.new_expression(
                            ExpressionKind::Literal(negated),
                            self.make_source(span),
                        ));
                    }
                }
                let zero = self.new_expression(
                    ExpressionKind::Literal(Value::Number(Decimal::ZERO)),
                    self.make_source(start_span),
                )?;
                let step = ChainStep {
                    op: ArithmeticComputation::Subtract,
                    operand,
                    source_location: self.make_source(span),
                };
                expr_step(self.arithmetic_chain(zero, vec![step]))
            }
            ExprCont::PipeFirst { start_span } => {
                self.pipe_chain(expr_of(value), start_span, false, frames)
            }
            ExprCont::PipeCommand { start_span } => {
                self.pipe_chain(expr_of(value), start_span, true, frames)
            }
            ExprCont::StepRoundingDone {
                operand,
                mode,
                start_span,
            } => {
                let step = optional_of(value);
                let span = self.span_covering(&start_span, &self.last_span);
                let rounded = self.new_expression(
                    ExpressionKind::StepRounding {
                        operand,
                        mode,
                        step,
                    },
                    self.make_source(span),
                )?;
                self.pipe_chain(rounded, start_span, true, frames)
            }
            ExprCont::PrefixRoundingRejected {
                keyword,
                keyword_span,
            } => {
                let operand = expr_of(value);
                let operand_text = display_expression(&self.table, operand);
                Err(Error::parsing(
                    format!(
                        "`{keyword}` is written after `->`; write `{operand_text} -> {keyword}`"
                    ),
                    Source::new(self.source_type(), keyword_span),
                    None::<String>,
                ))
            }
            ExprCont::ListCommandDone { list, start_span } => {
                let command = command_of(value);
                let span = self.span_covering(&start_span, &self.last_span);
                expr_step(self.new_expression(
                    ExpressionKind::ListCommand { list, command },
                    self.make_source(span),
                ))
            }
            ExprCont::Command(builder) => Ok(ExprStep::Yield(ExprVal::Command(build_command(
                builder, value,
            )))),
            ExprCont::FindKey { mut keys } => {
                keys.push(expr_of(value));
                self.find_key_next(keys, frames)
            }
            ExprCont::CallArgumentsEnd { command } => {
                self.expect(&TokenKind::RParen)?;
                self.reject_comma_after_call(command)?;
                Ok(ExprStep::Yield(value))
            }
            ExprCont::CallArgumentItems { mut arguments } => {
                arguments.push(expr_of(value));
                self.call_argument_items(arguments, frames)
            }
            ExprCont::SingleCallArgumentCheck { command } => {
                let mut arguments = exprs_of(value);
                if arguments.len() != 1 {
                    let tok = *self.peek()?;
                    return Err(self.error_at_token(
                        &tok,
                        format!("`{command}` takes one argument, got {}", arguments.len()),
                    ));
                }
                Ok(ExprStep::Yield(ExprVal::Expr(arguments.remove(0))))
            }
            ExprCont::WrapSome => Ok(ExprStep::Yield(ExprVal::Optional(Some(expr_of(value))))),
            ExprCont::BareArgumentEnd { command } => {
                let argument = expr_of(value);
                if !self.at_bare_argument_end()? {
                    return Err(self.bare_argument_error(command));
                }
                Ok(ExprStep::Yield(ExprVal::Expr(argument)))
            }
            ExprCont::ParenEnd { start_span } => {
                let mut inner = expr_of(value);
                self.expect(&TokenKind::RParen)?;
                if self.at(&TokenKind::Dot)? {
                    self.next()?;
                    let path = self.parse_reference()?;
                    let span = self.span_covering(&start_span, &self.last_span);
                    inner = self.new_expression(
                        ExpressionKind::Member { base: inner, path },
                        self.make_source(span),
                    )?;
                }
                Ok(ExprStep::Yield(ExprVal::Expr(inner)))
            }
            ExprCont::PastFuture { kind, start_span } => {
                let offset = expr_of(value);
                let offset_span = self.expr_span(offset).unwrap_or(start_span);
                let span = self.span_covering(&start_span, &offset_span);
                expr_step(self.new_expression(
                    ExpressionKind::PastFutureRange(kind, offset),
                    self.make_source(span),
                ))
            }
            ExprCont::Math {
                operator,
                start_span,
            } => {
                let operand = expr_of(value);
                let end_span = self.expr_span(operand).unwrap_or(start_span);
                let span = self.span_covering(&start_span, &end_span);
                expr_step(self.new_expression(
                    ExpressionKind::MathematicalComputation(operator, operand),
                    self.make_source(span),
                ))
            }
            ExprCont::LowerUpperEnd { bound, start_span } => {
                let operand = expr_of(value);
                let end_span = self.expr_span(operand).unwrap_or(start_span);
                let span = self.span_covering(&start_span, &end_span);
                expr_step(self.new_expression(
                    ExpressionKind::RangeBound(bound, operand),
                    self.make_source(span),
                ))
            }
            ExprCont::PrefixArgumentEnd { keyword } => {
                let argument = expr_of(value);
                if !self.at_bare_argument_end()? {
                    let tok = *self.peek()?;
                    return Err(self.error_at_token(
                        &tok,
                        format!(
                            "`{keyword}` without parentheses takes one operand, found '{op}' after it; \
                             write `{keyword}(a {op} b)` or `{keyword}(a) {op} b`",
                            op = tok.text.of(&self.source)
                        ),
                    ));
                }
                Ok(ExprStep::Yield(ExprVal::Expr(argument)))
            }
            ExprCont::PredicateValueRight { op, command } => {
                let right = expr_of(value);
                if !self.at_bare_argument_end()? {
                    return Err(self.condition_error(command));
                }
                let left = self.new_expression(
                    ExpressionKind::ImplicitRowValue,
                    self.table
                        .source(right)
                        .cloned()
                        .expect("BUG: parsed predicate operand has source"),
                )?;
                let kind = match op {
                    RowCondition::Compare(op) => ExpressionKind::Comparison(left, op, right),
                    RowCondition::In => ExpressionKind::RangeContainment(left, right),
                };
                let expr = self.new_expression(
                    kind,
                    self.table
                        .source(right)
                        .cloned()
                        .expect("BUG: parsed predicate operand has source"),
                )?;
                Ok(ExprStep::Yield(ExprVal::Predicate(ListPredicate::Value {
                    expr,
                })))
            }
            ExprCont::PredicateLeft {
                start_span,
                command,
            } => {
                let left = expr_of(value);
                if self.at_row_condition_operator()? {
                    let op = self.parse_row_condition_operator()?;
                    frames.push(ExprCont::PredicateRight {
                        left,
                        op,
                        start_span,
                        command,
                    });
                    return Ok(ExprStep::Enter(ExprTask::BareOperand));
                }
                self.predicate_instance_end(left, command)
            }
            ExprCont::PredicateRight {
                left,
                op,
                start_span,
                command,
            } => {
                let right = expr_of(value);
                let span = self.span_covering(&start_span, &self.last_span);
                let kind = match op {
                    RowCondition::Compare(op) => ExpressionKind::Comparison(left, op, right),
                    RowCondition::In => ExpressionKind::RangeContainment(left, right),
                };
                let expr = self.new_expression(kind, self.make_source(span))?;
                self.predicate_instance_end(expr, command)
            }
            ExprCont::WrapInstance => Ok(ExprStep::Yield(ExprVal::Predicate(
                ListPredicate::Instance {
                    expr: expr_of(value),
                },
            ))),
            ExprCont::SortArgument { check_bare } => {
                let argument = expr_of(value);
                if check_bare
                    && !self.at_any(&[TokenKind::Asc, TokenKind::Desc])?
                    && !self.at_bare_argument_end()?
                {
                    return Err(self.bare_argument_error("sort"));
                }
                let descending = self.parse_sort_direction()?;
                Ok(ExprStep::Yield(ExprVal::Command(ListCommand::Sort {
                    descending,
                    argument: Some(argument),
                })))
            }
            ExprCont::ListWithEnd => {
                let rows = rows_of(value);
                self.expect(&TokenKind::RParen)?;
                self.reject_comma_after_call("with")?;
                Ok(ExprStep::Yield(ExprVal::Rows(rows)))
            }
            ExprCont::PairValue { path, mut pairs } => {
                pairs.push((path, expr_of(value)));
                self.pair_value_next(pairs, frames)
            }
            ExprCont::ValueItem { mut values } => {
                values.push(expr_of(value));
                self.value_item_next(values, frames)
            }
            ExprCont::ValuePredicateCallEnd { command } => {
                self.expect(&TokenKind::RParen)?;
                self.reject_comma_after_call(command)?;
                Ok(ExprStep::Yield(ExprVal::Predicate(ListPredicate::Value {
                    expr: expr_of(value),
                })))
            }
            ExprCont::ValuePredicateGroupEnd => {
                self.expect(&TokenKind::RParen)?;
                Ok(ExprStep::Yield(value))
            }
            ExprCont::ValuePredicateLeaf { op, start } => {
                let right = expr_of(value);
                let left =
                    self.new_expression(ExpressionKind::ImplicitRowValue, self.make_source(start))?;
                let kind = match op {
                    RowCondition::Compare(op) => ExpressionKind::Comparison(left, op, right),
                    RowCondition::In => ExpressionKind::RangeContainment(left, right),
                };
                expr_step(self.new_expression(
                    kind,
                    self.make_source(self.span_covering(&start, &self.last_span.clone())),
                ))
            }
        }
    }

    fn logical_chain(
        &mut self,
        connective: LogicalConnective,
        first: ExprId,
        steps: Vec<ChainStep<()>>,
        start_span: Span,
        kind: LogicalChainKind,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        let token = match connective {
            LogicalConnective::And => TokenKind::And,
            LogicalConnective::Or => TokenKind::Or,
        };
        if !self.at(&token)? {
            return match steps.last() {
                None => Ok(ExprStep::Yield(ExprVal::Expr(first))),
                Some(last) => {
                    let source = last.source_location.clone();
                    expr_step(self.new_expression(
                        ExpressionKind::Logical {
                            connective,
                            first,
                            steps,
                        },
                        source,
                    ))
                }
            };
        }
        self.next()?;
        frames.push(ExprCont::LogicalOperand {
            connective,
            first,
            steps,
            start_span,
            kind,
        });
        Ok(ExprStep::Enter(logical_operand_task(connective, kind)))
    }

    /// `operand -> command -> ...`. Each `->` applies to everything on its left up to the
    /// nearest comparison or logic operator. `piped` is true once a command was applied:
    /// an arithmetic operator may not follow it.
    fn pipe_chain(
        &mut self,
        operand: ExprId,
        start_span: Span,
        piped: bool,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        if self.at(&TokenKind::Arrow)? {
            let mode = match self.lexer.peek_second()?.kind {
                TokenKind::Round => Some(RoundingMode::Round),
                TokenKind::Floor => Some(RoundingMode::Floor),
                TokenKind::Ceil => Some(RoundingMode::Ceil),
                _ => None,
            };
            let Some(mode) = mode else {
                frames.push(ExprCont::PipeCommand { start_span });
                return Ok(ExprStep::Enter(ExprTask::ListCommandSuffix {
                    list: operand,
                    start_span,
                }));
            };
            self.next()?;
            self.next()?;
            frames.push(ExprCont::StepRoundingDone {
                operand,
                mode,
                start_span,
            });
            return Ok(ExprStep::Enter(ExprTask::OptionalArgument {
                command: mode.keyword(),
            }));
        }
        if piped
            && self.at_any(&[
                TokenKind::Plus,
                TokenKind::Minus,
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::Mod,
                TokenKind::Caret,
                TokenKind::As,
            ])?
        {
            let tok = *self.peek()?;
            let op = tok.text.of(&self.source);
            let command = display_pipe_command(&self.table, operand);
            return Err(self.error_at_token(
                &tok,
                format!(
                    "found '{op}' after `-> {command}`; `->` applies to everything on its left, \
                     so put parentheses around the command to use its result: \
                     `(... -> {command}) {op} ...`"
                ),
            ));
        }
        expr_step(Ok(operand))
    }

    fn continue_repository(
        &mut self,
        mut expr: ExprId,
        start_span: Span,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        loop {
            let kind = self.peek()?.kind;
            if is_comparison_operator(&kind) {
                return Ok(ExprStep::Enter(ExprTask::ComparisonSuffix {
                    left: expr,
                    start_span,
                }));
            }
            if kind == TokenKind::Not {
                expr = self.parse_not_in_calendar_suffix(expr, start_span)?;
                continue;
            }
            if kind == TokenKind::In {
                frames.push(ExprCont::ContinueRepository { start_span });
                return Ok(ExprStep::Enter(ExprTask::InSuffix {
                    repository: expr,
                    start_span,
                }));
            }
            if kind == TokenKind::As {
                expr = self.parse_as_chain(expr, start_span)?;
                continue;
            }
            break;
        }

        if self.at(&TokenKind::Arrow)? {
            let tok = *self.peek()?;
            let message = match self.table.kind(expr) {
                ExpressionKind::RangeContainment(value, range)
                    if !matches!(self.table.kind(*range), ExpressionKind::PastFutureRange(..)) =>
                {
                    let value_text = display_expression(&self.table, *value);
                    let range_text = display_expression(&self.table, *range);
                    format!(
                        "found '->' after `{value_text} in {range_text}`; `in` takes one range operand, \
                         so put parentheses around the command: `{value_text} in ({range_text} -> ...)`"
                    )
                }
                _ => {
                    let expr_text = display_expression(&self.table, expr);
                    format!(
                        "found '->' after `{expr_text}`; put parentheses around what `->` applies to: \
                         `({expr_text}) -> ...`"
                    )
                }
            };
            return Err(self.error_at_token(&tok, message));
        }

        if self.at_expression_suffix_end()? {
            return expr_step(Ok(expr));
        }

        let tok = *self.peek()?;
        Err(self.error_at_token(
            &tok,
            format!(
                "Unexpected token '{}' after expression",
                tok.text.of(&self.source)
            ),
        ))
    }

    fn enter_comparison_suffix(
        &mut self,
        left: ExprId,
        start_span: Span,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        let operator = self.parse_comparison_operator()?;
        let operator_is_not = matches!(operator, ComparisonComputation::IsNot);

        if matches!(
            operator,
            ComparisonComputation::Is | ComparisonComputation::IsNot
        ) && self.at_bare_veto_token()?
        {
            self.expect(&TokenKind::Veto)?;
            if self.at(&TokenKind::StringLit)? {
                let tok = *self.peek()?;
                return Err(self.error_at_token(
                    &tok,
                    "veto with a message is only valid as a rule or unless result, not in `is veto` comparisons",
                ));
            }
            return expr_step(self.wrap_result_is_veto_expression(
                left,
                operator_is_not,
                false,
                start_span,
            ));
        }

        let right = if self.at(&TokenKind::Not)? {
            ExprTask::NotExpression
        } else {
            ExprTask::Pipe
        };
        frames.push(ExprCont::ComparisonRight {
            left,
            operator,
            start_span,
        });
        Ok(ExprStep::Enter(right))
    }

    fn enter_in_suffix(
        &mut self,
        repository: ExprId,
        start_span: Span,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        self.expect(&TokenKind::In)?;

        let peeked = *self.peek()?;
        if peeked.kind == TokenKind::Past || peeked.kind == TokenKind::Future {
            let direction = self.next()?;
            let rel_kind = if direction.kind == TokenKind::Past {
                DateRelativeKind::InPast
            } else {
                DateRelativeKind::InFuture
            };

            if self.at_calendar_period_marker()? {
                self.next_calendar_period_marker()?;
                let cal_kind = if direction.kind == TokenKind::Past {
                    DateCalendarKind::Past
                } else {
                    DateCalendarKind::Future
                };
                let unit = self.parse_calendar_unit()?;
                let end = self.peek()?.span;
                let span = self.span_covering(&start_span, &end);
                return expr_step(self.new_expression(
                    ExpressionKind::DateCalendar(cal_kind, unit, repository),
                    self.make_source(span),
                ));
            }

            if self.at_any(&[TokenKind::And, TokenKind::Or])?
                || self.at(&TokenKind::Unless)?
                || self.at(&TokenKind::Then)?
                || self.at(&TokenKind::RParen)?
                || self.at(&TokenKind::Eof)?
                || self.at(&TokenKind::Spec)?
                || is_spec_body_keyword(&self.peek()?.kind)
                || is_comparison_operator(&self.peek()?.kind)
            {
                let end = self.peek()?.span;
                let span = self.span_covering(&start_span, &end);
                return expr_step(self.new_expression(
                    ExpressionKind::DateRelative(rel_kind, repository),
                    self.make_source(span),
                ));
            }

            frames.push(ExprCont::InOffset {
                direction: direction.span,
                rel_kind,
                repository,
                start_span,
            });
            return Ok(ExprStep::Enter(ExprTask::RepositoryExpression));
        }

        if token_is_calendar_period_marker(&self.source, &peeked) {
            self.next_calendar_period_marker()?;
            let unit = self.parse_calendar_unit()?;
            let end = self.peek()?.span;
            let span = self.span_covering(&start_span, &end);
            return expr_step(self.new_expression(
                ExpressionKind::DateCalendar(DateCalendarKind::Current, unit, repository),
                self.make_source(span),
            ));
        }

        frames.push(ExprCont::InRange {
            repository,
            start_span,
        });
        Ok(ExprStep::Enter(ExprTask::RepositoryExpression))
    }

    fn enter_prefix_argument(
        &mut self,
        keyword: String,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        if self.at(&TokenKind::LParen)? {
            return Ok(ExprStep::Enter(ExprTask::Primary));
        }
        if !self.at_bare_operand_start()? {
            let tok = *self.peek()?;
            return Err(self.error_at_token(
                &tok,
                format!(
                    "`{keyword}` takes one operand or `{keyword}(...)`, found '{}'",
                    tok.text.of(&self.source)
                ),
            ));
        }
        self.check_depth()?;
        frames.push(ExprCont::PrefixArgumentEnd { keyword });
        frames.push(ExprCont::PopDepth);
        Ok(ExprStep::Enter(ExprTask::RangeOperand))
    }

    fn enter_primary(&mut self, frames: &mut Vec<ExprCont>) -> Result<ExprStep, Error> {
        let start_span = self.peek()?.span;
        let kind = self.peek()?.kind;
        match &kind {
            TokenKind::LParen => {
                self.next()?;
                let saved = self.save_flags();
                self.call_argument_depth = 0;
                frames.push(ExprCont::ParenEnd { start_span });
                frames.push(ExprCont::Restore(saved));
                Ok(ExprStep::Enter(ExprTask::Expression))
            }
            TokenKind::Now => {
                let tok = self.next()?;
                expr_step(self.new_expression(ExpressionKind::Now, self.make_source(tok.span)))
            }
            TokenKind::Past | TokenKind::Future => {
                let tok = self.next()?;
                let rel_kind = if tok.kind == TokenKind::Past {
                    DateRelativeKind::InPast
                } else {
                    DateRelativeKind::InFuture
                };
                frames.push(ExprCont::PastFuture {
                    kind: rel_kind,
                    start_span,
                });
                Ok(ExprStep::Enter(ExprTask::RepositoryExpression))
            }
            TokenKind::StringLit => {
                let tok = self.next()?;
                expr_step(self.new_expression(
                    ExpressionKind::Literal(Value::Text(tok.text.of(&self.source).to_string())),
                    self.make_source(tok.span),
                ))
            }
            k if is_boolean_keyword(k) => {
                let tok = self.next()?;
                expr_step(self.new_expression(
                    ExpressionKind::Literal(Value::Boolean(token_kind_to_boolean_value(&tok.kind))),
                    self.make_source(tok.span),
                ))
            }
            TokenKind::NumberLit => expr_step(self.parse_number_expression()),
            k if can_be_label(k) => {
                let reference = self.parse_expression_reference()?;
                let span = self.span_covering(&start_span, &self.last_span);
                let expr = self
                    .new_expression(ExpressionKind::Reference(reference), self.make_source(span))?;
                expr_step(self.wrap_list_type_suffix(expr, start_span))
            }
            k if token_kind_to_primitive(k).is_some() => {
                if !self.next_is_list_type_suffix()? {
                    let tok = self.next()?;
                    return Err(self.error_at_token(
                        &tok,
                        format!(
                            "Expected an expression, found '{}'",
                            tok.text.of(&self.source)
                        ),
                    ));
                }
                let parent = self.parse_expression_parent_type()?;
                let span = self.span_covering(&start_span, &self.last_span);
                expr_step(self.new_expression(
                    ExpressionKind::ListType { element: parent },
                    self.make_source(span),
                ))
            }
            _ => {
                let tok = *self.peek()?;
                if tok.text.of(&self.source).starts_with('@') {
                    return Err(self.error_at_token_with_suggestion(
                        &tok,
                        "A repository qualifier is not valid on a type or list element",
                        "Write `uses @repo Spec [pin]`, then `data name: Spec list` or `rule name: Spec list`.",
                    ));
                }
                let tok = self.next()?;
                Err(self.error_at_token(
                    &tok,
                    format!(
                        "Expected an expression, found '{}'",
                        tok.text.of(&self.source)
                    ),
                ))
            }
        }
    }

    fn enter_list_command_body(
        &mut self,
        name: Token,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        match name.kind {
            TokenKind::Find => {
                frames.push(ExprCont::Command(CommandBuild::Find));
                Ok(ExprStep::Enter(ExprTask::FindKeys))
            }
            TokenKind::First => {
                self.reject_command_argument("first")?;
                Ok(ExprStep::Yield(ExprVal::Command(ListCommand::First)))
            }
            TokenKind::Last => {
                self.reject_command_argument("last")?;
                Ok(ExprStep::Yield(ExprVal::Command(ListCommand::Last)))
            }
            TokenKind::Count => {
                self.reject_command_argument("count")?;
                Ok(ExprStep::Yield(ExprVal::Command(ListCommand::Count)))
            }
            TokenKind::Sum => {
                frames.push(ExprCont::Command(CommandBuild::Sum));
                Ok(ExprStep::Enter(ExprTask::OptionalArgument {
                    command: "sum",
                }))
            }
            TokenKind::Avg => {
                frames.push(ExprCont::Command(CommandBuild::Avg));
                Ok(ExprStep::Enter(ExprTask::OptionalArgument {
                    command: "avg",
                }))
            }
            TokenKind::Product => {
                frames.push(ExprCont::Command(CommandBuild::Product));
                Ok(ExprStep::Enter(ExprTask::OptionalArgument {
                    command: "product",
                }))
            }
            TokenKind::Min => {
                frames.push(ExprCont::Command(CommandBuild::Min));
                Ok(ExprStep::Enter(ExprTask::OptionalArgument {
                    command: "min",
                }))
            }
            TokenKind::Max => {
                frames.push(ExprCont::Command(CommandBuild::Max));
                Ok(ExprStep::Enter(ExprTask::OptionalArgument {
                    command: "max",
                }))
            }
            TokenKind::Any => {
                frames.push(ExprCont::Command(CommandBuild::Any));
                Ok(ExprStep::Enter(ExprTask::ListPredicate { command: "any" }))
            }
            TokenKind::All => {
                frames.push(ExprCont::Command(CommandBuild::All));
                Ok(ExprStep::Enter(ExprTask::ListPredicate { command: "all" }))
            }
            TokenKind::Where => {
                frames.push(ExprCont::Command(CommandBuild::Where));
                Ok(ExprStep::Enter(ExprTask::ListPredicate {
                    command: "where",
                }))
            }
            TokenKind::Sort => Ok(ExprStep::Enter(ExprTask::SortCommand)),
            TokenKind::Top => {
                frames.push(ExprCont::Command(CommandBuild::Top));
                Ok(ExprStep::Enter(ExprTask::RequiredArgument {
                    command: "top",
                }))
            }
            TokenKind::With => {
                frames.push(ExprCont::Command(CommandBuild::With));
                Ok(ExprStep::Enter(ExprTask::ListWithCommand))
            }
            TokenKind::Key => {
                let mut fields = vec![self.parse_reference()?];
                while self.at(&TokenKind::Comma)? {
                    self.next()?;
                    fields.push(self.parse_reference()?);
                }
                Ok(ExprStep::Yield(ExprVal::Command(ListCommand::Key {
                    fields,
                })))
            }
            _ => Err(self.error_at_token(
                &name,
                format!("Unknown list command '{}'", name.text.of(&self.source)),
            )),
        }
    }

    fn enter_list_predicate(
        &mut self,
        command: &'static str,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        if self.at(&TokenKind::LParen)? {
            return Ok(ExprStep::Enter(ExprTask::CallListPredicate { command }));
        }
        if self.at_row_condition_operator()? {
            let op = self.parse_row_condition_operator()?;
            frames.push(ExprCont::PredicateValueRight { op, command });
            return Ok(ExprStep::Enter(ExprTask::BareOperand));
        }
        if !self.at_bare_operand_start()? {
            let tok = *self.peek()?;
            return Err(self.error_at_token(
                &tok,
                format!("`{command}` needs a condition; write `{command}(...)`"),
            ));
        }
        let start_span = self.peek()?.span;
        frames.push(ExprCont::PredicateLeft {
            start_span,
            command,
        });
        Ok(ExprStep::Enter(ExprTask::BareOperand))
    }

    fn starts_value_predicate(&mut self) -> Result<bool, Error> {
        let checkpoint = self.checkpoint();
        while self.at(&TokenKind::LParen)? {
            self.next()?;
        }
        let result = self.at_row_condition_operator();
        self.restore(checkpoint);
        result
    }

    fn enter_value_predicate_factor(
        &mut self,
        command: &'static str,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        if self.at(&TokenKind::LParen)? {
            self.next()?;
            self.check_depth()?;
            frames.push(ExprCont::ValuePredicateGroupEnd);
            frames.push(ExprCont::PopDepth);
            return Ok(ExprStep::Enter(ExprTask::ValuePredicate {
                command,
                connective: LogicalConnective::Or,
            }));
        }
        if !self.at_row_condition_operator()? {
            let token = *self.peek()?;
            return Err(self.error_at_token(
                &token,
                format!(
                    "value-list `{command}` predicates require a headless comparison, found '{}'",
                    token.text.of(&self.source)
                ),
            ));
        }
        let start = self.peek()?.span;
        let op = self.parse_row_condition_operator()?;
        frames.push(ExprCont::ValuePredicateLeaf { op, start });
        Ok(ExprStep::Enter(ExprTask::RepositoryExpression))
    }

    fn predicate_instance_end(
        &mut self,
        expr: ExprId,
        command: &'static str,
    ) -> Result<ExprStep, Error> {
        if !self.at_bare_argument_end()? {
            return Err(self.condition_error(command));
        }
        Ok(ExprStep::Yield(ExprVal::Predicate(
            ListPredicate::Instance { expr },
        )))
    }

    fn enter_pair_field(
        &mut self,
        pairs: Vec<(Reference, ExprId)>,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        let path = self.parse_reference()?;
        self.expect(&TokenKind::Colon)?;
        frames.push(ExprCont::PairValue { path, pairs });
        Ok(ExprStep::Enter(ExprTask::BareArgument { command: "with" }))
    }

    fn pair_value_next(
        &mut self,
        pairs: Vec<(Reference, ExprId)>,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        if !self.at(&TokenKind::Comma)? {
            return Ok(ExprStep::Yield(ExprVal::Rows(ListWithRows::FieldPairs(
                pairs,
            ))));
        }
        self.next()?;
        if !self.at_path_then_colon()? {
            let tok = *self.peek()?;
            return Err(self.error_at_token_with_suggestion(
                &tok,
                "Expected a field pair after comma".to_string(),
                "Remove the extra comma, or add `field: value`",
            ));
        }
        self.enter_pair_field(pairs, frames)
    }

    fn value_item_next(
        &mut self,
        values: Vec<ExprId>,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        if !self.at(&TokenKind::Comma)? {
            return Ok(ExprStep::Yield(ExprVal::Rows(ListWithRows::Values(values))));
        }
        self.next()?;
        if self.at_expression_suffix_end()? || self.at(&TokenKind::Comma)? {
            let tok = *self.peek()?;
            return Err(self.error_at_token_with_suggestion(
                &tok,
                "Trailing or empty comma in list".to_string(),
                "Remove the extra comma, or add the missing value",
            ));
        }
        frames.push(ExprCont::ValueItem { values });
        Ok(ExprStep::Enter(ExprTask::BareArgument { command: "with" }))
    }

    fn find_key_next(
        &mut self,
        keys: Vec<ExprId>,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        if self.at(&TokenKind::Comma)? && self.call_argument_depth == 0 {
            self.next()?;
            frames.push(ExprCont::FindKey { keys });
            return Ok(ExprStep::Enter(ExprTask::BareArgument { command: "find" }));
        }
        Ok(ExprStep::Yield(ExprVal::Exprs(keys)))
    }

    fn call_argument_items(
        &mut self,
        arguments: Vec<ExprId>,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        if self.at(&TokenKind::Comma)? {
            self.next()?;
            frames.push(ExprCont::CallArgumentItems { arguments });
            return Ok(ExprStep::Enter(ExprTask::Expression));
        }
        Ok(ExprStep::Yield(ExprVal::Exprs(arguments)))
    }

    fn repository_chain(
        &mut self,
        first: ExprId,
        steps: Vec<ChainStep<ArithmeticComputation>>,
        start_span: Span,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        if !self.at_any(&[TokenKind::Plus, TokenKind::Minus])? {
            return expr_step(self.arithmetic_chain(first, steps));
        }
        let op_tok = self.next()?;
        let operation = match op_tok.kind {
            TokenKind::Plus => ArithmeticComputation::Add,
            TokenKind::Minus => ArithmeticComputation::Subtract,
            _ => unreachable!("BUG: only + and - should reach here"),
        };
        frames.push(ExprCont::RepositoryOperand {
            first,
            steps,
            start_span,
            operation,
        });
        Ok(ExprStep::Enter(ExprTask::Term { allow_as: true }))
    }

    fn term_chain(
        &mut self,
        first: ExprId,
        steps: Vec<ChainStep<ArithmeticComputation>>,
        allow_as: bool,
        start_span: Span,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        if self.at(&TokenKind::Percent)? {
            let pct_tok = self.next()?;
            return Err(self.error_at_token(
                &pct_tok,
                "'%' is percent only; use 'mod' for remainder (e.g. 'value mod 7')",
            ));
        }
        if !self.at_any(&[TokenKind::Star, TokenKind::Slash, TokenKind::Mod])? {
            return expr_step(self.arithmetic_chain(first, steps));
        }
        let op_tok = self.next()?;
        let operation = match op_tok.kind {
            TokenKind::Star => ArithmeticComputation::Multiply,
            TokenKind::Slash => ArithmeticComputation::Divide,
            TokenKind::Mod => ArithmeticComputation::Modulo,
            _ => unreachable!("BUG: only *, /, mod should reach here"),
        };
        let right_start_span = self.peek()?.span;
        frames.push(ExprCont::TermOperand {
            first,
            steps,
            allow_as,
            start_span,
            operation,
            right_start_span,
        });
        Ok(ExprStep::Enter(ExprTask::Power))
    }

    fn power_tail(
        &mut self,
        left: ExprId,
        start_span: Span,
        range_bound: bool,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        if !self.at(&TokenKind::Caret)? {
            return Ok(ExprStep::Yield(ExprVal::Expr(left)));
        }
        self.next()?;
        self.check_depth()?;
        frames.push(ExprCont::PowerRight { left, start_span });
        frames.push(ExprCont::PopDepth);
        Ok(ExprStep::Enter(if range_bound {
            ExprTask::PowerForRangeBound
        } else {
            ExprTask::Power
        }))
    }

    fn ellipsis_chain(
        &mut self,
        first: ExprId,
        steps: Vec<ChainStep<ArithmeticComputation>>,
        start_span: Span,
        mut starts: Vec<usize>,
        frames: &mut Vec<ExprCont>,
    ) -> Result<ExprStep, Error> {
        if !self.at_any(&[TokenKind::Plus, TokenKind::Minus])? {
            self.bound_chain_starts = starts;
            return expr_step(self.arithmetic_chain(first, steps));
        }
        let op_tok = self.next()?;
        let operation = match op_tok.kind {
            TokenKind::Plus => ArithmeticComputation::Add,
            TokenKind::Minus => ArithmeticComputation::Subtract,
            _ => unreachable!("BUG: only + and - should reach here"),
        };
        starts.push(self.peek()?.span.start);
        frames.push(ExprCont::EllipsisOperand {
            first,
            steps,
            start_span,
            operation,
            starts,
        });
        Ok(ExprStep::Enter(ExprTask::PowerForRangeBound))
    }
}

/// Expression grammar state saved and restored around nested parses.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ExprFlags {
    call_argument_depth: usize,
}

/// Which operand grammar a logical chain continues with.
#[derive(Clone, Copy)]
enum LogicalChainKind {
    Expression,
    ValuePredicate { command: &'static str },
}

fn logical_operand_task(connective: LogicalConnective, kind: LogicalChainKind) -> ExprTask {
    match kind {
        LogicalChainKind::Expression => match connective {
            LogicalConnective::And => ExprTask::LogicalOperand,
            LogicalConnective::Or => ExprTask::LogicalExpression {
                connective: LogicalConnective::And,
            },
        },
        LogicalChainKind::ValuePredicate { command } => match connective {
            LogicalConnective::And => ExprTask::ValuePredicateFactor { command },
            LogicalConnective::Or => ExprTask::ValuePredicate {
                command,
                connective: LogicalConnective::And,
            },
        },
    }
}

/// A grammar nonterminal the expression machine can enter.
enum ExprTask {
    Expression,
    LogicalExpression {
        connective: LogicalConnective,
    },
    LogicalOperand,
    VetoStatus,
    NotExpression,
    RepositoryWithSuffix,
    ComparisonSuffix {
        left: ExprId,
        start_span: Span,
    },
    InSuffix {
        repository: ExprId,
        start_span: Span,
    },
    /// Comparison operand: an arithmetic expression followed by any `->` commands.
    Pipe,
    RepositoryExpression,
    Term {
        allow_as: bool,
    },
    Power,
    RangeOperand,
    RangeEllipsisBound,
    PowerForRangeBound,
    Factor,
    PrimaryOrMath,
    /// Prefix `round` / `floor` / `ceil`: parse the operand, then reject with a rewrite.
    PrefixRoundingRejected,
    MathFunction,
    LowerUpper,
    PrefixArgument {
        keyword: String,
    },
    Primary,
    ListCommandSuffix {
        list: ExprId,
        start_span: Span,
    },
    ListCommandBody {
        name: Token,
    },
    FindKeys,
    CallArguments {
        command: &'static str,
    },
    SingleCallArgument {
        command: &'static str,
    },
    OptionalArgument {
        command: &'static str,
    },
    RequiredArgument {
        command: &'static str,
    },
    BareArgument {
        command: &'static str,
    },
    BareOperand,
    ListPredicate {
        command: &'static str,
    },
    CallListPredicate {
        command: &'static str,
    },
    ValuePredicate {
        command: &'static str,
        connective: LogicalConnective,
    },
    ValuePredicateFactor {
        command: &'static str,
    },
    SortCommand,
    ListWithCommand,
    ListWithRows,
}

/// Command whose argument values are collected before the `ListCommand` is built.
#[derive(Clone, Copy)]
enum CommandBuild {
    Find,
    Sum,
    Avg,
    Product,
    Min,
    Max,
    Any,
    All,
    Where,
    Top,
    With,
}

/// Continuation: what to do with the value a nested nonterminal yields.
enum ExprCont {
    Restore(ExprFlags),
    PopDepth,
    LogicalFirst {
        connective: LogicalConnective,
        start_span: Span,
        kind: LogicalChainKind,
    },
    LogicalOperand {
        connective: LogicalConnective,
        first: ExprId,
        steps: Vec<ChainStep<()>>,
        start_span: Span,
        kind: LogicalChainKind,
    },
    VetoStatus {
        operator_is_not: bool,
        keyword_was_negated: bool,
        start_span: Span,
    },
    Negation {
        start_span: Span,
    },
    RepositorySuffix {
        start_span: Span,
    },
    ContinueRepository {
        start_span: Span,
    },
    ComparisonRight {
        left: ExprId,
        operator: ComparisonComputation,
        start_span: Span,
    },
    InOffset {
        direction: Span,
        rel_kind: DateRelativeKind,
        repository: ExprId,
        start_span: Span,
    },
    InRange {
        repository: ExprId,
        start_span: Span,
    },
    RepositoryFirst {
        start_span: Span,
    },
    RepositoryOperand {
        first: ExprId,
        steps: Vec<ChainStep<ArithmeticComputation>>,
        start_span: Span,
        operation: ArithmeticComputation,
    },
    TermFirst {
        allow_as: bool,
        start_span: Span,
    },
    TermOperand {
        first: ExprId,
        steps: Vec<ChainStep<ArithmeticComputation>>,
        allow_as: bool,
        start_span: Span,
        operation: ArithmeticComputation,
        right_start_span: Span,
    },
    PowerLeft {
        start_span: Span,
        range_bound: bool,
    },
    PowerRight {
        left: ExprId,
        start_span: Span,
    },
    /// Factor of a range operand is parsed; a range bound may continue it.
    RangeFactor {
        start_span: Span,
        flags: ExprFlags,
    },
    /// Bound continuing `factor` is parsed speculatively from `checkpoint`, taken right after
    /// `factor`. Without a following `...` the parser rewinds there and yields `factor`.
    RangeLeft {
        factor: ExprId,
        start_span: Span,
        checkpoint: Box<ParserCheckpoint>,
        flags: ExprFlags,
    },
    RangeRight {
        left: ExprId,
        start_span: Span,
    },
    EllipsisFirst {
        start_span: Span,
    },
    EllipsisOperand {
        first: ExprId,
        steps: Vec<ChainStep<ArithmeticComputation>>,
        start_span: Span,
        operation: ArithmeticComputation,
        starts: Vec<usize>,
    },
    FactorNegate {
        start_span: Span,
    },
    /// Arithmetic operand of a `->` chain is parsed; commands may follow.
    PipeFirst {
        start_span: Span,
    },
    /// A list command was applied; more commands may follow.
    PipeCommand {
        start_span: Span,
    },
    StepRoundingDone {
        operand: ExprId,
        mode: RoundingMode,
        start_span: Span,
    },
    ListCommandDone {
        list: ExprId,
        start_span: Span,
    },
    Command(CommandBuild),
    FindKey {
        keys: Vec<ExprId>,
    },
    CallArgumentsEnd {
        command: &'static str,
    },
    CallArgumentItems {
        arguments: Vec<ExprId>,
    },
    SingleCallArgumentCheck {
        command: &'static str,
    },
    WrapSome,
    BareArgumentEnd {
        command: &'static str,
    },
    ParenEnd {
        start_span: Span,
    },
    PastFuture {
        kind: DateRelativeKind,
        start_span: Span,
    },
    Math {
        operator: MathematicalComputation,
        start_span: Span,
    },
    PrefixRoundingRejected {
        keyword: String,
        keyword_span: Span,
    },
    LowerUpperEnd {
        bound: RangeBound,
        start_span: Span,
    },
    PrefixArgumentEnd {
        keyword: String,
    },
    PredicateValueRight {
        op: RowCondition,
        command: &'static str,
    },
    PredicateLeft {
        start_span: Span,
        command: &'static str,
    },
    PredicateRight {
        left: ExprId,
        op: RowCondition,
        start_span: Span,
        command: &'static str,
    },
    ValuePredicateCallEnd {
        command: &'static str,
    },
    ValuePredicateGroupEnd,
    ValuePredicateLeaf {
        op: RowCondition,
        start: Span,
    },
    WrapInstance,
    SortArgument {
        check_bare: bool,
    },
    ListWithEnd,
    PairValue {
        path: Reference,
        pairs: Vec<(Reference, ExprId)>,
    },
    ValueItem {
        values: Vec<ExprId>,
    },
}

/// Value a finished nonterminal yields to its continuation.
enum ExprVal {
    Expr(ExprId),
    Optional(Option<ExprId>),
    Exprs(Vec<ExprId>),
    Command(ListCommand),
    Predicate(ListPredicate),
    Rows(ListWithRows),
}

enum ExprStep {
    Enter(ExprTask),
    Yield(ExprVal),
}

fn expr_of(value: ExprVal) -> ExprId {
    match value {
        ExprVal::Expr(expr) => expr,
        _ => panic!("BUG: expression machine expected an expression value"),
    }
}

fn optional_of(value: ExprVal) -> Option<ExprId> {
    match value {
        ExprVal::Optional(argument) => argument,
        _ => panic!("BUG: expression machine expected an optional argument"),
    }
}

fn exprs_of(value: ExprVal) -> Vec<ExprId> {
    match value {
        ExprVal::Exprs(exprs) => exprs,
        _ => panic!("BUG: expression machine expected an argument list"),
    }
}

fn predicate_of(value: ExprVal) -> ListPredicate {
    match value {
        ExprVal::Predicate(predicate) => predicate,
        _ => panic!("BUG: expression machine expected a list predicate"),
    }
}

fn rows_of(value: ExprVal) -> ListWithRows {
    match value {
        ExprVal::Rows(rows) => rows,
        _ => panic!("BUG: expression machine expected list rows"),
    }
}

fn command_of(value: ExprVal) -> ListCommand {
    match value {
        ExprVal::Command(command) => command,
        _ => panic!("BUG: expression machine expected a list command"),
    }
}

fn expr_step(result: Result<ExprId, Error>) -> Result<ExprStep, Error> {
    result.map(|expr| ExprStep::Yield(ExprVal::Expr(expr)))
}

fn build_command(builder: CommandBuild, value: ExprVal) -> ListCommand {
    match builder {
        CommandBuild::Find => ListCommand::Find {
            keys: exprs_of(value),
        },
        CommandBuild::Sum => ListCommand::Sum {
            argument: optional_of(value),
        },
        CommandBuild::Avg => ListCommand::Avg {
            argument: optional_of(value),
        },
        CommandBuild::Product => ListCommand::Product {
            argument: optional_of(value),
        },
        CommandBuild::Min => ListCommand::Min {
            argument: optional_of(value),
        },
        CommandBuild::Max => ListCommand::Max {
            argument: optional_of(value),
        },
        CommandBuild::Any => ListCommand::Any {
            form: predicate_of(value),
        },
        CommandBuild::All => ListCommand::All {
            form: predicate_of(value),
        },
        CommandBuild::Where => ListCommand::Where {
            form: predicate_of(value),
        },
        CommandBuild::Top => ListCommand::Top { n: expr_of(value) },
        CommandBuild::With => ListCommand::With {
            rows: rows_of(value),
        },
    }
}

fn parent_type_from_reference(reference: &Reference) -> ParentType {
    if reference.segments.is_empty() {
        return ParentType::custom(reference.name.clone());
    }
    if reference.segments.len() == 1 {
        return ParentType::Qualified {
            spec_alias: reference.segments[0].clone(),
            inner: Box::new(ParentType::custom(reference.name.clone())),
        };
    }
    ParentType::Qualified {
        spec_alias: reference.segments[0].clone(),
        inner: Box::new(ParentType::custom(reference.name.clone())),
    }
}

// ============================================================================
// Helper functions
// ============================================================================

fn parse_decimal_string(text: &str, span: &Span, parser: &Parser) -> Result<Decimal, Error> {
    text.parse::<crate::literals::NumberLiteral>()
        .map(|parsed| parsed.0)
        .map_err(|message| Error::parsing(message, parser.make_source(*span), None::<String>))
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
