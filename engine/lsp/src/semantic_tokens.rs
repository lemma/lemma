use lemma::{can_be_label, Lexer, TokenKind};
use tower_lsp_server::ls_types::*;

/// Legend indices — must stay in sync with TOKEN_TYPES order and monaco.js SEMANTIC_TOKEN_TYPES.
const IDX_NAMESPACE: u32 = 0; // repo qualifier tokens
const IDX_CLASS: u32 = 1; // spec name tokens
const IDX_PROPERTY: u32 = 2; // data field path tokens (before colon)
const IDX_FUNCTION: u32 = 3; // rule name token (colon excluded)
const IDX_VALUE: u32 = 4; // every value: literals, booleans, duration units, identifiers in body
const IDX_COMMENT: u32 = 5;
const IDX_KEYWORD: u32 = 6; // type/constraint words (muted; business users don't need them)
const IDX_OPERATOR: u32 = 7;
const IDX_CONTROL: u32 = 8; // unless, then, not, and, in, veto, now, past, future, stray repo
const IDX_DATA_BODY: u32 = 9; // data block after the colon
const IDX_PUNCTUATION: u32 = 10; // colons after data field path and rule name
const IDX_REFERENCE: u32 = 11; // identifiers in rule/spec body (paths, aliases, …)
const IDX_DECLARATION: u32 = 12; // declaration keywords: spec, data, with, rule, repo, uses, meta

/// Custom LSP token types not in the standard set.
pub const CONTROL_KEYWORD: SemanticTokenType = SemanticTokenType::new("controlKeyword");
pub const DATA_BODY: SemanticTokenType = SemanticTokenType::new("dataBody");
pub const PUNCTUATION: SemanticTokenType = SemanticTokenType::new("punctuation");
pub const REFERENCE: SemanticTokenType = SemanticTokenType::new("reference");
pub const DECLARATION_KEYWORD: SemanticTokenType = SemanticTokenType::new("declarationKeyword");

/// Ordered legend. Index positions are the `IDX_*` constants above.
pub const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::NAMESPACE, // 0
    SemanticTokenType::CLASS,     // 1
    SemanticTokenType::PROPERTY,  // 2
    SemanticTokenType::FUNCTION,  // 3
    SemanticTokenType::STRING,    // 4 — the "value" bucket
    SemanticTokenType::COMMENT,   // 5
    SemanticTokenType::KEYWORD,   // 6
    SemanticTokenType::OPERATOR,  // 7
    CONTROL_KEYWORD,              // 8
    DATA_BODY,                    // 9
    PUNCTUATION,                  // 10
    REFERENCE,                    // 11
    DECLARATION_KEYWORD,          // 12
];

pub const TOKEN_MODIFIERS: &[SemanticTokenModifier] = &[];

/// Tracks whether the scan is currently consuming a declaration header.
#[derive(Clone, Copy, PartialEq)]
enum HeaderState {
    /// Not inside a declaration header — standard body context.
    None,
    /// After `repo`: consume one opaque qualifier [`TokenKind::LogicalName`].
    Repo,
    /// After `spec`: consume opaque spec name / uses targets.
    Spec,
    /// After `data`: consume field-path tokens until Colon.
    Data,
    /// After `data … :` — type annotation + constraint arrows.
    DataBody,
    /// After `->` inside data body — recognize constraint keywords.
    DataBodyAfterArrow,
    /// After `rule`: consume the single rule-name token then `RuleColon`.
    Rule,
    /// After rule name: expect Colon as IDX_PUNCTUATION, then body.
    RuleColon,
}

/// Returns true for type keyword tokens.
fn is_type_keyword(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::MeasureKw
            | TokenKind::NumberKw
            | TokenKind::TextKw
            | TokenKind::DateKw
            | TokenKind::TimeKw
            | TokenKind::BooleanKw
            | TokenKind::RatioKw
    )
}

/// Classify a token that appears in body (non-header) context.
fn type_in_body(kind: &TokenKind) -> Option<u32> {
    match kind {
        // Control / flow surface — subdued but distinct
        TokenKind::Unless
        | TokenKind::Then
        | TokenKind::Not
        | TokenKind::And
        | TokenKind::In
        | TokenKind::Veto
        | TokenKind::Now
        | TokenKind::Past
        | TokenKind::Future
        // Structural `repo` in body/stray position (not `repo` declaration or qualifier segment)
        | TokenKind::Repo => Some(IDX_CONTROL),

        // Type system / constraint keywords — muted; business users skip past these
        _ if is_type_keyword(kind) => Some(IDX_KEYWORD),

        // Math function names — muted; treat as type-system noise
        TokenKind::Sqrt
        | TokenKind::Sin
        | TokenKind::Cos
        | TokenKind::Tan
        | TokenKind::Asin
        | TokenKind::Acos
        | TokenKind::Atan
        | TokenKind::Log
        | TokenKind::Exp
        | TokenKind::Abs
        | TokenKind::Floor
        | TokenKind::Ceil
        | TokenKind::Round
        | TokenKind::List
        | TokenKind::Key
        | TokenKind::Find
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
        | TokenKind::Asc
        | TokenKind::Desc
        | TokenKind::Lower
        | TokenKind::Upper => Some(IDX_KEYWORD),

        // Operators
        TokenKind::Plus
        | TokenKind::Minus
        | TokenKind::Star
        | TokenKind::Slash
        | TokenKind::Percent
        | TokenKind::PercentPercent
        | TokenKind::Caret
        | TokenKind::Gt
        | TokenKind::Lt
        | TokenKind::Gte
        | TokenKind::Lte
        | TokenKind::Arrow
        | TokenKind::Ellipsis
        | TokenKind::As
        | TokenKind::Mod
        | TokenKind::Is => Some(IDX_OPERATOR),

        // Comment
        TokenKind::Commentary => Some(IDX_COMMENT),

        // Value-like: literals, booleans (identifiers
        // use [`expression_semantic_type`] → IDX_REFERENCE).
        TokenKind::StringLit
        | TokenKind::NumberLit
        | TokenKind::True
        | TokenKind::False
        | TokenKind::Yes
        | TokenKind::No
        | TokenKind::Permille => Some(IDX_VALUE),

        // Punctuation (Colon, Dot, Comma, LParen, RParen, …) — transparent
        _ => None,
    }
}

/// Semantic type in spec/rule/repo body: literals and operators from
/// [`type_in_body`], plus [`TokenKind::Identifier`] as [`IDX_REFERENCE`].
fn expression_semantic_type(kind: &TokenKind) -> Option<u32> {
    type_in_body(kind).or(match kind {
        TokenKind::Identifier | TokenKind::LogicalName => Some(IDX_REFERENCE),
        TokenKind::Colon => Some(IDX_PUNCTUATION),
        _ => None,
    })
}

/// Returns true for token kinds that can legally appear as a rule name.
/// Type keywords are also legal identifiers in that position in the grammar.
fn is_name_token(kind: &TokenKind) -> bool {
    matches!(kind, TokenKind::Identifier) || is_type_keyword(kind)
}

/// Produce LSP delta-encoded semantic tokens for `text` using a stateful
/// single-pass scan over the lexer token stream.
///
/// Declaration headers (`repo`, `spec`, `data`, `with`, `rule`, `uses`, `meta`) emit
/// declarationKeyword followed by names (spec/data/rule) or body tokens (uses/meta).
/// `dataBody` covers the data block after the colon, with type keywords and constraint
/// words highlighted separately. Rule bodies use reference, value, operator, keyword,
/// and control buckets.
pub fn tokenize(text: &str) -> Vec<SemanticToken> {
    let mut lexer = Lexer::new(text, &lemma::SourceType::Volatile);
    let mut tokens = Vec::new();
    let mut prev_line: u32 = 0;
    let mut prev_col: u32 = 0;
    let mut state = HeaderState::None;
    let mut prev_token_kind: Option<TokenKind> = None;
    let mut uses_import = false;
    let mut uses_alias = false;
    // Next token in Repo/Spec header is an opaque name — use Lexer::next_logical_name.
    let mut expect_opaque_name = false;

    loop {
        let tok = if expect_opaque_name {
            expect_opaque_name = false;
            match lexer.next_logical_name() {
                Ok(t) => t,
                Err(_) => break,
            }
        } else {
            match lexer.next_token() {
                Ok(t) => t,
                Err(_) => break,
            }
        };
        if tok.kind == TokenKind::Eof {
            break;
        }

        // Declaration keywords always (re-)start a header state from any context.
        // `repo` is handled inside [`HeaderState::None`] and [`HeaderState::Repo`] so
        // `spec repo` does not reinterpret the keyword as a new repository declaration.
        // Returns (type_index, modifier_bits).
        let token_info: Option<(u32, u32)> = match tok.kind {
            TokenKind::Spec => {
                state = HeaderState::Spec;
                uses_import = false;
                uses_alias = false;
                expect_opaque_name = true;
                Some((IDX_DECLARATION, 0))
            }
            TokenKind::Data => {
                state = HeaderState::Data;
                expect_opaque_name = false;
                Some((IDX_DECLARATION, 0))
            }
            TokenKind::With => {
                state = HeaderState::Data;
                expect_opaque_name = false;
                Some((IDX_DECLARATION, 0))
            }
            TokenKind::Rule => {
                state = HeaderState::Rule;
                expect_opaque_name = false;
                Some((IDX_DECLARATION, 0))
            }
            TokenKind::Uses => {
                state = HeaderState::Spec;
                uses_import = true;
                uses_alias = match lexer.peek() {
                    Ok(first) if can_be_label(&first.kind) => {
                        matches!(lexer.peek_second(), Ok(second) if second.kind == TokenKind::Colon)
                    }
                    _ => false,
                };
                // Alias form: first token is a simple label; otherwise opaque target/qualifier.
                expect_opaque_name = !uses_alias;
                Some((IDX_DECLARATION, 0))
            }
            TokenKind::Meta => {
                state = HeaderState::None;
                expect_opaque_name = false;
                Some((IDX_DECLARATION, 0))
            }

            _ => match state {
                HeaderState::Repo => match tok.kind {
                    TokenKind::LogicalName => Some((IDX_NAMESPACE, 0)),
                    _ => {
                        state = HeaderState::None;
                        expression_semantic_type(&tok.kind).map(|idx| (idx, 0))
                    }
                },

                HeaderState::Spec if uses_import => match tok.kind {
                    TokenKind::Colon => {
                        // After `alias:`, next token is the opaque qualifier/target.
                        expect_opaque_name = true;
                        Some((IDX_PUNCTUATION, 0))
                    }
                    _ if uses_alias && can_be_label(&tok.kind) => {
                        uses_alias = false;
                        Some((IDX_PROPERTY, 0))
                    }
                    TokenKind::LogicalName | TokenKind::NumberLit => Some((IDX_REFERENCE, 0)),
                    _ if can_be_label(&tok.kind) => Some((IDX_REFERENCE, 0)),
                    _ => {
                        uses_import = false;
                        uses_alias = false;
                        state = HeaderState::None;
                        expression_semantic_type(&tok.kind).map(|idx| (idx, 0))
                    }
                },

                HeaderState::Spec => match tok.kind {
                    TokenKind::LogicalName => Some((IDX_CLASS, 0)),
                    // NumberLit covers the effective-from year/date (e.g. `spec foo 2025`
                    // or `spec foo 2026-03-04`). Minus stays transparent between date parts.
                    TokenKind::NumberLit => Some((IDX_CLASS, 0)),
                    TokenKind::Minus => None,
                    _ => {
                        state = HeaderState::None;
                        expression_semantic_type(&tok.kind).map(|idx| (idx, 0))
                    }
                },

                HeaderState::Data => match tok.kind {
                    TokenKind::Identifier => Some((IDX_PROPERTY, 0)),
                    TokenKind::Dot => None,
                    TokenKind::Colon => {
                        state = HeaderState::DataBody;
                        Some((IDX_PUNCTUATION, 0))
                    }
                    _ => {
                        state = HeaderState::DataBody;
                        Some((IDX_DATA_BODY, 0))
                    }
                },

                HeaderState::DataBody => {
                    if tok.kind == TokenKind::Commentary {
                        Some((IDX_COMMENT, 0))
                    } else if is_type_keyword(&tok.kind) {
                        Some((IDX_KEYWORD, 0))
                    } else if tok.kind == TokenKind::Arrow {
                        state = HeaderState::DataBodyAfterArrow;
                        Some((IDX_OPERATOR, 0))
                    } else if type_in_body(&tok.kind) == Some(IDX_CONTROL)
                        || matches!(
                            tok.kind,
                            TokenKind::Spec
                                | TokenKind::Data
                                | TokenKind::With
                                | TokenKind::Rule
                                | TokenKind::Uses
                                | TokenKind::Meta
                        )
                    {
                        state = HeaderState::None;
                        if matches!(
                            tok.kind,
                            TokenKind::Spec | TokenKind::Data | TokenKind::With | TokenKind::Rule
                        ) {
                            match tok.kind {
                                TokenKind::Spec => {
                                    state = HeaderState::Spec;
                                    Some((IDX_DECLARATION, 0))
                                }
                                TokenKind::Data => {
                                    state = HeaderState::Data;
                                    Some((IDX_DECLARATION, 0))
                                }
                                TokenKind::With => {
                                    state = HeaderState::Data;
                                    Some((IDX_DECLARATION, 0))
                                }
                                TokenKind::Rule => {
                                    state = HeaderState::Rule;
                                    Some((IDX_DECLARATION, 0))
                                }
                                _ => unreachable!("BUG: matched declaration but not in match arm"),
                            }
                        } else if matches!(tok.kind, TokenKind::Uses | TokenKind::Meta) {
                            Some((IDX_DECLARATION, 0))
                        } else {
                            Some((IDX_CONTROL, 0))
                        }
                    } else {
                        Some((IDX_DATA_BODY, 0))
                    }
                }

                HeaderState::DataBodyAfterArrow => {
                    state = HeaderState::DataBody;
                    if tok.kind == TokenKind::Identifier {
                        if lemma::try_parse_type_constraint_command(&tok.text).is_some() {
                            Some((IDX_KEYWORD, 0))
                        } else {
                            Some((IDX_DATA_BODY, 0))
                        }
                    } else if matches!(tok.kind, TokenKind::Lower | TokenKind::Upper) {
                        // Lexer keywords, not Identifier — still TypeConstraintCommand spellings.
                        Some((IDX_KEYWORD, 0))
                    } else if tok.kind == TokenKind::Commentary {
                        Some((IDX_COMMENT, 0))
                    } else if is_type_keyword(&tok.kind) {
                        Some((IDX_KEYWORD, 0))
                    } else if tok.kind == TokenKind::Arrow {
                        state = HeaderState::DataBodyAfterArrow;
                        Some((IDX_OPERATOR, 0))
                    } else if type_in_body(&tok.kind) == Some(IDX_CONTROL)
                        || matches!(
                            tok.kind,
                            TokenKind::Spec
                                | TokenKind::Data
                                | TokenKind::With
                                | TokenKind::Rule
                                | TokenKind::Uses
                                | TokenKind::Meta
                        )
                    {
                        state = HeaderState::None;
                        if matches!(
                            tok.kind,
                            TokenKind::Spec | TokenKind::Data | TokenKind::With | TokenKind::Rule
                        ) {
                            match tok.kind {
                                TokenKind::Spec => {
                                    state = HeaderState::Spec;
                                    Some((IDX_DECLARATION, 0))
                                }
                                TokenKind::Data => {
                                    state = HeaderState::Data;
                                    Some((IDX_DECLARATION, 0))
                                }
                                TokenKind::With => {
                                    state = HeaderState::Data;
                                    Some((IDX_DECLARATION, 0))
                                }
                                TokenKind::Rule => {
                                    state = HeaderState::Rule;
                                    Some((IDX_DECLARATION, 0))
                                }
                                _ => unreachable!("BUG: matched declaration but not in match arm"),
                            }
                        } else if matches!(tok.kind, TokenKind::Uses | TokenKind::Meta) {
                            Some((IDX_DECLARATION, 0))
                        } else {
                            Some((IDX_CONTROL, 0))
                        }
                    } else {
                        Some((IDX_DATA_BODY, 0))
                    }
                }

                HeaderState::Rule => {
                    if is_name_token(&tok.kind) {
                        state = HeaderState::RuleColon;
                        Some((IDX_FUNCTION, 0))
                    } else {
                        state = HeaderState::None;
                        expression_semantic_type(&tok.kind).map(|idx| (idx, 0))
                    }
                }

                HeaderState::RuleColon => {
                    state = HeaderState::None;
                    if tok.kind == TokenKind::Colon {
                        Some((IDX_PUNCTUATION, 0))
                    } else {
                        expression_semantic_type(&tok.kind).map(|idx| (idx, 0))
                    }
                }

                HeaderState::None => {
                    if tok.kind == TokenKind::Repo {
                        state = HeaderState::Repo;
                        expect_opaque_name = true;
                        Some((IDX_DECLARATION, 0))
                    } else if tok.kind == TokenKind::Dot
                        && matches!(prev_token_kind, Some(TokenKind::Identifier))
                    {
                        Some((IDX_REFERENCE, 0))
                    } else {
                        expression_semantic_type(&tok.kind).map(|idx| (idx, 0))
                    }
                }
            },
        };

        let (type_idx, modifier_bits) = match token_info {
            Some(info) => info,
            None => {
                prev_token_kind = Some(tok.kind);
                continue;
            }
        };

        let start_line = (tok.span.line as u32).saturating_sub(1);
        let start_col = (tok.span.col as u32).saturating_sub(1);

        // Commentary and string-literal token text excludes delimiters but the
        // span covers them. Reconstruct the full visual text so lengths match
        // the highlighted range.
        let full_delimited;
        let display_text = match tok.kind {
            TokenKind::Commentary => {
                full_delimited = format!("\"\"\"{}\"\"\"", tok.text);
                &full_delimited
            }
            TokenKind::StringLit => {
                full_delimited = format!("\"{}\"", tok.text);
                &full_delimited
            }
            _ => &tok.text,
        };

        prev_token_kind = Some(tok.kind);

        // A single lexer token can span multiple lines (e.g. block comments).
        // Emit one SemanticToken per visual line segment.
        let lines: Vec<&str> = display_text.split('\n').collect();
        for (i, segment) in lines.iter().enumerate() {
            let seg_len = segment.chars().count() as u32;
            if seg_len == 0 {
                continue;
            }

            let line = start_line + i as u32;
            let col = if i == 0 { start_col } else { 0 };

            let delta_line = line - prev_line;
            let delta_start = if delta_line == 0 { col - prev_col } else { col };

            tokens.push(SemanticToken {
                delta_line,
                delta_start,
                length: seg_len,
                token_type: type_idx,
                token_modifiers_bitset: modifier_bits,
            });

            prev_line = line;
            prev_col = col;
        }
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_types(text: &str) -> Vec<u32> {
        tokenize(text).iter().map(|t| t.token_type).collect()
    }

    #[test]
    fn repo_keyword_and_qualifier_same_colour() {
        // repo → DECLARATION, @iso/countries → one NAMESPACE LogicalName
        assert_eq!(
            token_types("repo @iso/countries"),
            vec![IDX_DECLARATION, IDX_NAMESPACE]
        );
    }

    #[test]
    fn simple_repo_name_no_qualifier() {
        // repo → DECLARATION, local → NAMESPACE
        assert_eq!(
            token_types("repo local"),
            vec![IDX_DECLARATION, IDX_NAMESPACE]
        );
    }

    #[test]
    fn spec_keyword_and_name_same_colour() {
        // spec → DECLARATION, weather_clothing → CLASS
        assert_eq!(
            token_types("spec weather_clothing"),
            vec![IDX_DECLARATION, IDX_CLASS]
        );
    }

    /// `repo` after `spec` is invalid Lemma; opaque name scan yields CLASS, not a `repo` declaration.
    #[test]
    fn spec_followed_by_repo_spelling_is_class_not_namespace() {
        assert_eq!(token_types("spec repo"), vec![IDX_DECLARATION, IDX_CLASS]);
    }

    #[test]
    fn data_keyword_field_type_and_colon() {
        // data → DECLARATION, temperature → PROPERTY, : PUNCTUATION, number → KEYWORD
        assert_eq!(
            token_types("data temperature: number"),
            vec![IDX_DECLARATION, IDX_PROPERTY, IDX_PUNCTUATION, IDX_KEYWORD]
        );
    }

    #[test]
    fn data_body_granular_type_and_constraints() {
        let text = "data temperature: measure\n  -> unit celsius: 1.0\n  -> minimum -70 celsius";
        let types = token_types(text);
        assert_eq!(
            &types[..3],
            &[IDX_DECLARATION, IDX_PROPERTY, IDX_PUNCTUATION]
        );
        // measure → KEYWORD, -> OPERATOR, unit → KEYWORD, celsius : 1.0 → DATA_BODY...
        assert_eq!(types[3], IDX_KEYWORD); // measure
        assert_eq!(types[4], IDX_OPERATOR); // ->
        assert_eq!(types[5], IDX_KEYWORD); // unit
        assert_eq!(types[6], IDX_DATA_BODY); // celsius
        assert_eq!(types[7], IDX_DATA_BODY); // :
        assert_eq!(types[8], IDX_DATA_BODY); // 1.0
        assert_eq!(types[9], IDX_OPERATOR); // ->
        assert_eq!(types[10], IDX_KEYWORD); // minimum
        assert_eq!(types[11], IDX_DATA_BODY); // -70
        assert_eq!(types[12], IDX_DATA_BODY); // celsius
    }

    #[test]
    fn data_body_ends_at_next_declaration() {
        let types = token_types("data x: number\nrule y: 5");
        assert_eq!(
            types,
            vec![
                IDX_DECLARATION,
                IDX_PROPERTY,
                IDX_PUNCTUATION,
                IDX_KEYWORD,     // number
                IDX_DECLARATION, // rule
                IDX_FUNCTION,
                IDX_PUNCTUATION,
                IDX_VALUE,
            ]
        );
    }

    #[test]
    fn uses_block_with_binding_path_colon_punctuation() {
        // -> OPERATOR, with → DECLARATION, name → PROPERTY, : PUNCTUATION
        assert_eq!(
            token_types("  -> with name:"),
            vec![IDX_OPERATOR, IDX_DECLARATION, IDX_PROPERTY, IDX_PUNCTUATION]
        );
    }

    #[test]
    fn rule_keyword_name_and_colon() {
        // rule → DECLARATION, needs_umbrella → FUNCTION, : PUNCTUATION, 42 → VALUE
        assert_eq!(
            token_types("rule needs_umbrella: 42"),
            vec![IDX_DECLARATION, IDX_FUNCTION, IDX_PUNCTUATION, IDX_VALUE]
        );
    }

    #[test]
    fn unless_then_are_control() {
        // rule → DECLARATION, x → FUNCTION, : PUNCTUATION, yes → VALUE
        // unless → CONTROL, a → REFERENCE, then → CONTROL, no → VALUE
        assert_eq!(
            token_types("rule x: yes\n  unless a then no"),
            vec![
                IDX_DECLARATION,
                IDX_FUNCTION,
                IDX_PUNCTUATION,
                IDX_VALUE,
                IDX_CONTROL,
                IDX_REFERENCE,
                IDX_CONTROL,
                IDX_VALUE,
            ]
        );
    }

    #[test]
    fn uses_is_declaration() {
        // spec → DECLARATION, s → CLASS, uses → DECLARATION, alias → REFERENCE
        assert_eq!(
            token_types("spec s\nuses alias"),
            vec![IDX_DECLARATION, IDX_CLASS, IDX_DECLARATION, IDX_REFERENCE]
        );
    }

    #[test]
    fn meta_is_declaration() {
        // spec → DECLARATION, s → CLASS, meta → DECLARATION, author → REFERENCE, : PUNCTUATION, "x" → VALUE
        assert_eq!(
            token_types("spec s\nmeta author: \"x\""),
            vec![
                IDX_DECLARATION,
                IDX_CLASS,
                IDX_DECLARATION,
                IDX_REFERENCE,
                IDX_PUNCTUATION,
                IDX_VALUE
            ]
        );
    }

    #[test]
    fn rule_body_identifiers_are_reference() {
        assert_eq!(
            token_types("rule r: x"),
            vec![
                IDX_DECLARATION,
                IDX_FUNCTION,
                IDX_PUNCTUATION,
                IDX_REFERENCE,
            ]
        );
    }

    #[test]
    fn condition_references_and_literals() {
        // rule → DECLARATION, x → FUNCTION, : PUNCTUATION, 1 → VALUE
        // unless → CONTROL, temperature → REFERENCE, < → OPERATOR, 5 → VALUE, then → CONTROL, 2 → VALUE
        assert_eq!(
            token_types("rule x: 1\n  unless temperature < 5 then 2"),
            vec![
                IDX_DECLARATION,
                IDX_FUNCTION,
                IDX_PUNCTUATION,
                IDX_VALUE,
                IDX_CONTROL,
                IDX_REFERENCE,
                IDX_OPERATOR,
                IDX_VALUE,
                IDX_CONTROL,
                IDX_VALUE,
            ]
        );
    }

    #[test]
    fn string_and_number_and_bool_all_value() {
        assert_eq!(
            token_types("rule a: \"hello\"\nrule b: 42\nrule c: yes"),
            vec![
                IDX_DECLARATION,
                IDX_FUNCTION,
                IDX_PUNCTUATION,
                IDX_VALUE,
                IDX_DECLARATION,
                IDX_FUNCTION,
                IDX_PUNCTUATION,
                IDX_VALUE,
                IDX_DECLARATION,
                IDX_FUNCTION,
                IDX_PUNCTUATION,
                IDX_VALUE,
            ]
        );
    }

    #[test]
    fn string_literal_token_length_includes_quotes() {
        let tokens = tokenize("rule a: \"hi\"");
        let string_tok = tokens
            .iter()
            .find(|t| t.token_type == IDX_VALUE)
            .expect("string value token");
        assert_eq!(
            string_tok.length, 4,
            "\"hi\" must highlight 4 columns including both quotes"
        );
    }

    #[test]
    fn multiline_string_literal_token_lengths_include_quotes() {
        let tokens = tokenize("rule a: \"line1\nline2\"");
        let value_tokens: Vec<_> = tokens
            .iter()
            .filter(|t| t.token_type == IDX_VALUE)
            .collect();
        assert!(
            value_tokens.len() >= 2,
            "multiline string must emit one token per visual line"
        );
        assert_eq!(
            value_tokens[0].length, 6,
            "first line \"line1 must include opening quote: got {}",
            value_tokens[0].length
        );
        assert_eq!(
            value_tokens[1].length, 6,
            "second line line2\" must include closing quote: got {}",
            value_tokens[1].length
        );
    }

    #[test]
    fn spec_effective_year_same_colour_as_name() {
        // spec → DECLARATION, weather_clothing → CLASS, 2025 → CLASS
        assert_eq!(
            token_types("spec weather_clothing 2025"),
            vec![IDX_DECLARATION, IDX_CLASS, IDX_CLASS]
        );
    }

    #[test]
    fn spec_effective_full_date_same_colour_as_name() {
        // spec → DECLARATION, foo → CLASS, 2026 → CLASS, - transparent, 03 → CLASS, - transparent, 04 → CLASS
        assert_eq!(
            token_types("spec foo 2026-03-04"),
            vec![IDX_DECLARATION, IDX_CLASS, IDX_CLASS, IDX_CLASS, IDX_CLASS]
        );
    }

    #[test]
    fn commentary_delimiters_colored_as_comment() {
        let toks = tokenize("\"\"\"hello\"\"\"");
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].token_type, IDX_COMMENT);
        // length covers """hello""" = 11 chars
        assert_eq!(toks[0].length, 11);
    }

    #[test]
    fn multiline_commentary_delimiters_colored() {
        let toks = tokenize("\"\"\"\nHello\n\"\"\"");
        // Three segments: """ (line 0), Hello (line 1), """ (line 2)
        assert_eq!(toks.len(), 3);
        assert!(toks.iter().all(|t| t.token_type == IDX_COMMENT));
        assert_eq!(toks[0].length, 3); // opening """
        assert_eq!(toks[1].length, 5); // Hello
        assert_eq!(toks[2].length, 3); // closing """
    }

    #[test]
    fn declaration_keywords_restart_state_from_any_context() {
        // spec → DECLARATION, a → CLASS, data → DECLARATION (restarts from Spec state), x → PROPERTY, : PUNCTUATION
        assert_eq!(
            token_types("spec a\ndata x:"),
            vec![
                IDX_DECLARATION,
                IDX_CLASS,
                IDX_DECLARATION,
                IDX_PROPERTY,
                IDX_PUNCTUATION
            ]
        );
    }

    #[test]
    fn constraint_word_as_string_arg_stays_data_body() {
        // data → DECLARATION, x → PROPERTY, : PUNCT, text → KEYWORD, -> OP, option → KEYWORD, "active" → DATA_BODY
        assert_eq!(
            token_types("data x: text -> option \"active\""),
            vec![
                IDX_DECLARATION,
                IDX_PROPERTY,
                IDX_PUNCTUATION,
                IDX_KEYWORD,
                IDX_OPERATOR,
                IDX_KEYWORD,
                IDX_DATA_BODY
            ]
        );
    }

    #[test]
    fn dot_in_reference_path_colored() {
        // rule → DECLARATION, r → FUNCTION, : PUNCT, units → REFERENCE, . → REFERENCE, mass → REFERENCE
        assert_eq!(
            token_types("rule r: units.mass"),
            vec![
                IDX_DECLARATION,
                IDX_FUNCTION,
                IDX_PUNCTUATION,
                IDX_REFERENCE,
                IDX_REFERENCE,
                IDX_REFERENCE
            ]
        );
    }

    #[test]
    fn repo_declaration_one_logical_name() {
        assert_eq!(
            token_types("repo @org/repo"),
            vec![IDX_DECLARATION, IDX_NAMESPACE]
        );
    }

    #[test]
    fn uses_with_alias() {
        // @iso/countries is one LogicalName REFERENCE
        assert_eq!(
            token_types("spec s\nuses alias: @iso/countries"),
            vec![
                IDX_DECLARATION,
                IDX_CLASS,
                IDX_DECLARATION,
                IDX_PROPERTY,
                IDX_PUNCTUATION,
                IDX_REFERENCE,
            ]
        );
    }

    #[test]
    fn uses_alias_qualified_path_and_year_are_reference() {
        assert_eq!(
            token_types("spec s\nuses alias: @iso/countries alpha2 2025"),
            vec![
                IDX_DECLARATION,
                IDX_CLASS,
                IDX_DECLARATION,
                IDX_PROPERTY,
                IDX_PUNCTUATION,
                IDX_REFERENCE,
                IDX_REFERENCE,
                IDX_REFERENCE,
            ]
        );
    }

    #[test]
    fn uses_bare_qualified_path_and_year_are_reference() {
        assert_eq!(
            token_types("spec s\nuses @iso/countries alpha2 2026"),
            vec![
                IDX_DECLARATION,
                IDX_CLASS,
                IDX_DECLARATION,
                IDX_REFERENCE,
                IDX_REFERENCE,
                IDX_REFERENCE,
            ]
        );
    }

    #[test]
    fn data_body_terminated_by_uses() {
        // data → DECLARATION, x → PROPERTY, : PUNCT, number → KEYWORD, uses → DECLARATION, foo → REFERENCE
        assert_eq!(
            token_types("data x: number\nuses foo"),
            vec![
                IDX_DECLARATION,
                IDX_PROPERTY,
                IDX_PUNCTUATION,
                IDX_KEYWORD,
                IDX_DECLARATION,
                IDX_REFERENCE
            ]
        );
    }

    #[test]
    fn data_body_terminated_by_meta() {
        // data → DECLARATION, x → PROPERTY, : PUNCT, number → KEYWORD, meta → DECLARATION, author → REFERENCE
        assert_eq!(
            token_types("data x: number\nmeta author: \"x\""),
            vec![
                IDX_DECLARATION,
                IDX_PROPERTY,
                IDX_PUNCTUATION,
                IDX_KEYWORD,
                IDX_DECLARATION,
                IDX_REFERENCE,
                IDX_PUNCTUATION,
                IDX_VALUE
            ]
        );
    }

    #[test]
    fn as_and_mod_and_ellipsis_are_operators() {
        assert_eq!(
            token_types("rule r: x as number"),
            vec![
                IDX_DECLARATION,
                IDX_FUNCTION,
                IDX_PUNCTUATION,
                IDX_REFERENCE,
                IDX_OPERATOR,
                IDX_KEYWORD,
            ]
        );
        assert_eq!(
            token_types("rule r: 10 mod 3"),
            vec![
                IDX_DECLARATION,
                IDX_FUNCTION,
                IDX_PUNCTUATION,
                IDX_VALUE,
                IDX_OPERATOR,
                IDX_VALUE,
            ]
        );
        assert_eq!(
            token_types("rule r: 1...5"),
            vec![
                IDX_DECLARATION,
                IDX_FUNCTION,
                IDX_PUNCTUATION,
                IDX_VALUE,
                IDX_OPERATOR,
                IDX_VALUE,
            ]
        );
    }

    #[test]
    fn lower_and_upper_are_keywords() {
        assert_eq!(
            token_types("rule r: lower x"),
            vec![
                IDX_DECLARATION,
                IDX_FUNCTION,
                IDX_PUNCTUATION,
                IDX_KEYWORD,
                IDX_REFERENCE,
            ]
        );
        assert_eq!(
            token_types("rule r: upper x"),
            vec![
                IDX_DECLARATION,
                IDX_FUNCTION,
                IDX_PUNCTUATION,
                IDX_KEYWORD,
                IDX_REFERENCE,
            ]
        );
    }

    #[test]
    fn data_constraint_lower_upper_are_keywords() {
        assert_eq!(
            token_types("data x: number -> lower 0 -> upper 10"),
            vec![
                IDX_DECLARATION,
                IDX_PROPERTY,
                IDX_PUNCTUATION,
                IDX_KEYWORD,
                IDX_OPERATOR,
                IDX_KEYWORD,
                IDX_DATA_BODY,
                IDX_OPERATOR,
                IDX_KEYWORD,
                IDX_DATA_BODY,
            ]
        );
    }
}
