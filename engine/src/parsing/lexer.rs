use crate::error::Error;
use crate::parsing::ast::{BooleanValue, PrimitiveKind, Span};
use crate::parsing::source::Source;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    // Keywords
    Spec,
    Repo,
    Data,
    Rule,
    Unless,
    Then,
    Not,
    And,
    In,
    As,
    Uses,
    With,
    Meta,
    Veto,
    Now,
    Past,
    Future,

    // Boolean keywords
    True,
    False,
    Yes,
    No,

    // Type keywords
    MeasureKw,
    NumberKw,
    TextKw,
    DateKw,
    TimeKw,
    BooleanKw,
    RatioKw,

    // Math function keywords
    Sqrt,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Log,
    Exp,
    Abs,
    Floor,
    Ceil,
    Round,

    Permille,

    // Comparison keyword operators
    Is,

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Comma,
    Percent,
    PercentPercent,
    Caret,
    Gt,
    Lt,
    Gte,
    Lte,

    // Punctuation
    Colon,
    Arrow,
    Ellipsis,
    Dot,
    At,
    LParen,
    RParen,

    // Literals
    NumberLit,
    StringLit,

    // Commentary (raw text between """ delimiters)
    Commentary,

    // Identifiers
    Identifier,

    // End of file
    Eof,
}

impl std::fmt::Display for TokenKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenKind::Spec => write!(f, "'spec'"),
            TokenKind::Repo => write!(f, "'repo'"),
            TokenKind::Data => write!(f, "'data'"),
            TokenKind::Rule => write!(f, "'rule'"),
            TokenKind::Unless => write!(f, "'unless'"),
            TokenKind::Then => write!(f, "'then'"),
            TokenKind::Not => write!(f, "'not'"),
            TokenKind::And => write!(f, "'and'"),
            TokenKind::In => write!(f, "'in'"),
            TokenKind::As => write!(f, "'as'"),
            TokenKind::Uses => write!(f, "'uses'"),
            TokenKind::With => write!(f, "'with'"),
            TokenKind::Meta => write!(f, "'meta'"),
            TokenKind::Veto => write!(f, "'veto'"),
            TokenKind::Now => write!(f, "'now'"),
            TokenKind::Past => write!(f, "'past'"),
            TokenKind::Future => write!(f, "'future'"),
            TokenKind::True => write!(f, "'true'"),
            TokenKind::False => write!(f, "'false'"),
            TokenKind::Yes => write!(f, "'yes'"),
            TokenKind::No => write!(f, "'no'"),
            TokenKind::MeasureKw => write!(f, "'measure'"),
            TokenKind::NumberKw => write!(f, "'number'"),
            TokenKind::TextKw => write!(f, "'text'"),
            TokenKind::DateKw => write!(f, "'date'"),
            TokenKind::TimeKw => write!(f, "'time'"),
            TokenKind::BooleanKw => write!(f, "'boolean'"),
            TokenKind::RatioKw => write!(f, "'ratio'"),
            TokenKind::Sqrt => write!(f, "'sqrt'"),
            TokenKind::Sin => write!(f, "'sin'"),
            TokenKind::Cos => write!(f, "'cos'"),
            TokenKind::Tan => write!(f, "'tan'"),
            TokenKind::Asin => write!(f, "'asin'"),
            TokenKind::Acos => write!(f, "'acos'"),
            TokenKind::Atan => write!(f, "'atan'"),
            TokenKind::Log => write!(f, "'log'"),
            TokenKind::Exp => write!(f, "'exp'"),
            TokenKind::Abs => write!(f, "'abs'"),
            TokenKind::Floor => write!(f, "'floor'"),
            TokenKind::Ceil => write!(f, "'ceil'"),
            TokenKind::Round => write!(f, "'round'"),
            TokenKind::Permille => write!(f, "'permille'"),
            TokenKind::Is => write!(f, "'is'"),
            TokenKind::Plus => write!(f, "'+'"),
            TokenKind::Minus => write!(f, "'-'"),
            TokenKind::Star => write!(f, "'*'"),
            TokenKind::Slash => write!(f, "'/'"),
            TokenKind::Comma => write!(f, "','"),
            TokenKind::Percent => write!(f, "'%'"),
            TokenKind::PercentPercent => write!(f, "'%%'"),
            TokenKind::Caret => write!(f, "'^'"),
            TokenKind::Gt => write!(f, "'>'"),
            TokenKind::Lt => write!(f, "'<'"),
            TokenKind::Gte => write!(f, "'>='"),
            TokenKind::Lte => write!(f, "'<='"),
            TokenKind::Colon => write!(f, "':'"),
            TokenKind::Arrow => write!(f, "'->'"),
            TokenKind::Ellipsis => write!(f, "'...'"),
            TokenKind::Dot => write!(f, "'.'"),
            TokenKind::At => write!(f, "'@'"),
            TokenKind::LParen => write!(f, "'('"),
            TokenKind::RParen => write!(f, "')'"),
            TokenKind::NumberLit => write!(f, "a number"),
            TokenKind::StringLit => write!(f, "a string"),
            TokenKind::Commentary => write!(f, "commentary block"),
            TokenKind::Identifier => write!(f, "an identifier"),
            TokenKind::Eof => write!(f, "end of file"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
    pub text: String,
}

impl Token {
    pub fn eof(offset: usize, line: usize, col: usize) -> Self {
        Token {
            kind: TokenKind::Eof,
            span: Span {
                start: offset,
                end: offset,
                line,
                col,
            },
            text: String::new(),
        }
    }
}

#[derive(Clone)]
pub struct LexerCheckpoint {
    pos: usize,
    line: usize,
    col: usize,
    byte_offset: usize,
    peeked: Option<Token>,
    peeked2: Option<Token>,
}

// todo: find out why derive Clone is necessary
#[derive(Clone)]
pub struct Lexer {
    source: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
    byte_offset: usize,
    source_type: crate::parsing::source::SourceType,
    source_text: Arc<str>,
    peeked: Option<Token>,
    peeked2: Option<Token>,
}

impl Lexer {
    pub fn new(input: &str, source_type: &crate::parsing::source::SourceType) -> Self {
        let source_text: Arc<str> = Arc::from(input);
        Lexer {
            source: input.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
            byte_offset: 0,
            source_type: source_type.clone(),
            source_text,
            peeked: None,
            peeked2: None,
        }
    }

    pub fn peek(&mut self) -> Result<&Token, Error> {
        if self.peeked.is_none() {
            let token = self.lex_token()?;
            self.peeked = Some(token);
        }
        Ok(self.peeked.as_ref().expect("just assigned"))
    }

    pub fn peek_second(&mut self) -> Result<&Token, Error> {
        self.peek()?;
        if self.peeked2.is_none() {
            let token = self.lex_token()?;
            self.peeked2 = Some(token);
        }
        Ok(self.peeked2.as_ref().expect("just assigned"))
    }

    pub fn next_token(&mut self) -> Result<Token, Error> {
        if let Some(token) = self.peeked.take() {
            self.peeked = self.peeked2.take();
            return Ok(token);
        }
        self.lex_token()
    }

    /// Saved lexer position for speculative parsing.
    pub fn checkpoint(&self) -> LexerCheckpoint {
        LexerCheckpoint {
            pos: self.pos,
            line: self.line,
            col: self.col,
            byte_offset: self.byte_offset,
            peeked: self.peeked.clone(),
            peeked2: self.peeked2.clone(),
        }
    }

    pub fn restore(&mut self, checkpoint: LexerCheckpoint) {
        self.pos = checkpoint.pos;
        self.line = checkpoint.line;
        self.col = checkpoint.col;
        self.byte_offset = checkpoint.byte_offset;
        self.peeked = checkpoint.peeked;
        self.peeked2 = checkpoint.peeked2;
    }

    fn current_char(&self) -> Option<char> {
        self.source.get(self.pos).copied()
    }

    fn peek_char(&self) -> Option<char> {
        self.source.get(self.pos + 1).copied()
    }

    fn peek_char_at(&self, offset: usize) -> Option<char> {
        self.source.get(self.pos + offset).copied()
    }

    fn advance(&mut self) {
        if let Some(ch) = self.current_char() {
            self.byte_offset += ch.len_utf8();
            if ch == '\n' {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
            self.pos += 1;
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.current_char() {
            if ch.is_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn make_span(&self, start_byte: usize, start_line: usize, start_col: usize) -> Span {
        Span {
            start: start_byte,
            end: self.byte_offset,
            line: start_line,
            col: start_col,
        }
    }

    fn make_error(&self, message: impl Into<String>, span: Span) -> Error {
        Error::parsing(
            message,
            Source::new(self.source_type.clone(), span),
            None::<String>,
        )
    }

    fn lex_token(&mut self) -> Result<Token, Error> {
        self.skip_whitespace();

        let start_byte = self.byte_offset;
        let start_line = self.line;
        let start_col = self.col;

        let Some(ch) = self.current_char() else {
            return Ok(Token::eof(start_byte, start_line, start_col));
        };

        // Triple-quote commentary
        if ch == '"' && self.peek_char() == Some('"') && self.peek_char_at(2) == Some('"') {
            return self.scan_triple_quote(start_byte, start_line, start_col);
        }

        // String literal
        if ch == '"' {
            return self.scan_string(start_byte, start_line, start_col);
        }

        // Number literal (sign handled by parser, not lexer)
        if ch.is_ascii_digit() {
            return self.scan_number(start_byte, start_line, start_col);
        }

        // Two-character operators (check before single-char)
        if let Some(token) = self.try_two_char_operator(start_byte, start_line, start_col) {
            return Ok(token);
        }

        // Three-character ellipsis
        if ch == '.' && self.peek_char() == Some('.') && self.peek_char_at(2) == Some('.') {
            self.advance();
            self.advance();
            self.advance();
            let span = self.make_span(start_byte, start_line, start_col);
            return Ok(Token {
                kind: TokenKind::Ellipsis,
                span,
                text: "...".to_string(),
            });
        }

        // Single-character operators/punctuation
        if let Some(kind) = self.single_char_token(ch) {
            self.advance();
            let span = self.make_span(start_byte, start_line, start_col);
            let text = ch.to_string();
            return Ok(Token { kind, span, text });
        }

        // Identifier or keyword (starts with letter or @)
        if ch.is_ascii_alphabetic() || ch == '_' {
            return Ok(self.scan_identifier(start_byte, start_line, start_col));
        }

        // @ prefix for registry references
        if ch == '@' {
            self.advance();
            let span = self.make_span(start_byte, start_line, start_col);
            return Ok(Token {
                kind: TokenKind::At,
                span,
                text: "@".to_string(),
            });
        }

        // Unknown character
        self.advance();
        let span = self.make_span(start_byte, start_line, start_col);
        Err(self.make_error(format!("Unexpected character '{}'", ch), span))
    }

    fn scan_triple_quote(
        &mut self,
        start_byte: usize,
        start_line: usize,
        start_col: usize,
    ) -> Result<Token, Error> {
        self.advance(); // "
        self.advance(); // "
        self.advance(); // "

        let content_start = self.byte_offset;
        loop {
            match self.current_char() {
                None => {
                    let span = self.make_span(start_byte, start_line, start_col);
                    return Err(self.make_error(
                        "Unterminated commentary block: expected closing \"\"\"",
                        span,
                    ));
                }
                Some('"')
                    if self.source.get(self.pos + 1) == Some(&'"')
                        && self.source.get(self.pos + 2) == Some(&'"') =>
                {
                    let content_end = self.byte_offset;
                    self.advance(); // "
                    self.advance(); // "
                    self.advance(); // "
                    let raw: String = self.source_text[content_start..content_end].to_string();
                    let span = self.make_span(start_byte, start_line, start_col);
                    return Ok(Token {
                        kind: TokenKind::Commentary,
                        span,
                        text: raw,
                    });
                }
                Some(_) => {
                    self.advance();
                }
            }
        }
    }

    fn scan_string(
        &mut self,
        start_byte: usize,
        start_line: usize,
        start_col: usize,
    ) -> Result<Token, Error> {
        self.advance(); // consume opening "
        let mut content = String::new();
        loop {
            match self.current_char() {
                None => {
                    let span = self.make_span(start_byte, start_line, start_col);
                    return Err(self.make_error("String starting here was never closed", span));
                }
                Some('"') => {
                    self.advance(); // consume closing "
                    break;
                }
                Some(ch) => {
                    content.push(ch);
                    self.advance();
                }
            }
        }
        let span = self.make_span(start_byte, start_line, start_col);
        if content.len() > crate::limits::MAX_TEXT_VALUE_LENGTH {
            return Err(self.make_error(
                format!(
                    "Text literal exceeds maximum length of {} characters (found {})",
                    crate::limits::MAX_TEXT_VALUE_LENGTH,
                    content.len()
                ),
                span,
            ));
        }
        // Token text is the unquoted content. The span still covers the quotes so
        // diagnostics point at the full literal; the parser must not strip again.
        Ok(Token {
            kind: TokenKind::StringLit,
            span,
            text: content,
        })
    }

    fn scan_number(
        &mut self,
        start_byte: usize,
        start_line: usize,
        start_col: usize,
    ) -> Result<Token, Error> {
        let mut text = String::new();

        // Integer part: digits with optional _ or , separators
        while let Some(ch) = self.current_char() {
            if ch.is_ascii_digit() || ch == '_' || ch == ',' {
                text.push(ch);
                self.advance();
            } else {
                break;
            }
        }

        // Decimal part
        if self.current_char() == Some('.') {
            // Check if next char after dot is a digit (not a method call or dotted reference)
            if let Some(next) = self.peek_char() {
                if next.is_ascii_digit() {
                    text.push('.');
                    self.advance(); // consume .
                    while let Some(ch) = self.current_char() {
                        if ch.is_ascii_digit() {
                            text.push(ch);
                            self.advance();
                        } else {
                            break;
                        }
                    }
                }
            }
        }

        // Scientific notation: e or E followed by optional +/- and digits
        if let Some(ch) = self.current_char() {
            if ch == 'e' || ch == 'E' {
                let mut sci_text = String::new();
                sci_text.push(ch);
                let save_pos = self.pos;
                let save_byte = self.byte_offset;
                let save_line = self.line;
                let save_col = self.col;
                self.advance(); // consume e/E

                if let Some(sign) = self.current_char() {
                    if sign == '+' || sign == '-' {
                        sci_text.push(sign);
                        self.advance();
                    }
                }

                if let Some(d) = self.current_char() {
                    if d.is_ascii_digit() {
                        while let Some(ch) = self.current_char() {
                            if ch.is_ascii_digit() {
                                sci_text.push(ch);
                                self.advance();
                            } else {
                                break;
                            }
                        }
                        text.push_str(&sci_text);
                    } else {
                        // Not actually scientific notation, backtrack
                        self.pos = save_pos;
                        self.byte_offset = save_byte;
                        self.line = save_line;
                        self.col = save_col;
                    }
                } else {
                    self.pos = save_pos;
                    self.byte_offset = save_byte;
                    self.line = save_line;
                    self.col = save_col;
                }
            }
        }

        let span = self.make_span(start_byte, start_line, start_col);
        Ok(Token {
            kind: TokenKind::NumberLit,
            span,
            text,
        })
    }

    fn try_two_char_operator(
        &mut self,
        start_byte: usize,
        start_line: usize,
        start_col: usize,
    ) -> Option<Token> {
        let ch = self.current_char()?;
        let next = self.peek_char();

        let kind = match (ch, next) {
            ('-', Some('>')) => TokenKind::Arrow,
            ('>', Some('=')) => TokenKind::Gte,
            ('<', Some('=')) => TokenKind::Lte,
            ('%', Some('%')) => {
                // Check that it's not followed by a digit (invalid permille like 10%%5)
                TokenKind::PercentPercent
            }
            _ => return None,
        };

        self.advance();
        self.advance();
        let span = self.make_span(start_byte, start_line, start_col);
        let text: String = self.source_text[span.start..span.end].to_string();
        Some(Token { kind, span, text })
    }

    fn single_char_token(&self, ch: char) -> Option<TokenKind> {
        match ch {
            '+' => Some(TokenKind::Plus),
            '*' => Some(TokenKind::Star),
            '/' => Some(TokenKind::Slash),
            ',' => Some(TokenKind::Comma),
            '^' => Some(TokenKind::Caret),
            ':' => Some(TokenKind::Colon),
            '.' => Some(TokenKind::Dot),
            '(' => Some(TokenKind::LParen),
            ')' => Some(TokenKind::RParen),
            '>' => Some(TokenKind::Gt),
            '<' => Some(TokenKind::Lt),
            '%' => Some(TokenKind::Percent),
            '-' => Some(TokenKind::Minus),
            _ => None,
        }
    }

    fn scan_identifier(&mut self, start_byte: usize, start_line: usize, start_col: usize) -> Token {
        let mut text = String::new();
        while let Some(ch) = self.current_char() {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                text.push(ch);
                self.advance();
            } else {
                break;
            }
        }

        let kind = keyword_from_identifier(&text);
        let span = self.make_span(start_byte, start_line, start_col);
        Token { kind, span, text }
    }
}

fn keyword_from_identifier(text: &str) -> TokenKind {
    // ASCII fold without allocating a lowercased String on every identifier.
    match text {
        s if s.eq_ignore_ascii_case("spec") => TokenKind::Spec,
        s if s.eq_ignore_ascii_case("repo") => TokenKind::Repo,
        s if s.eq_ignore_ascii_case("data") => TokenKind::Data,
        s if s.eq_ignore_ascii_case("rule") => TokenKind::Rule,
        s if s.eq_ignore_ascii_case("unless") => TokenKind::Unless,
        s if s.eq_ignore_ascii_case("then") => TokenKind::Then,
        s if s.eq_ignore_ascii_case("not") => TokenKind::Not,
        s if s.eq_ignore_ascii_case("and") => TokenKind::And,
        s if s.eq_ignore_ascii_case("in") => TokenKind::In,
        s if s.eq_ignore_ascii_case("as") => TokenKind::As,
        s if s.eq_ignore_ascii_case("uses") => TokenKind::Uses,
        s if s.eq_ignore_ascii_case("with") => TokenKind::With,
        s if s.eq_ignore_ascii_case("meta") => TokenKind::Meta,
        s if s.eq_ignore_ascii_case("veto") => TokenKind::Veto,
        s if s.eq_ignore_ascii_case("now") => TokenKind::Now,
        s if s.eq_ignore_ascii_case("past") => TokenKind::Past,
        s if s.eq_ignore_ascii_case("future") => TokenKind::Future,
        s if s.eq_ignore_ascii_case("true") => TokenKind::True,
        s if s.eq_ignore_ascii_case("false") => TokenKind::False,
        s if s.eq_ignore_ascii_case("yes") => TokenKind::Yes,
        s if s.eq_ignore_ascii_case("no") => TokenKind::No,
        s if s.eq_ignore_ascii_case("measure") => TokenKind::MeasureKw,
        s if s.eq_ignore_ascii_case("number") => TokenKind::NumberKw,
        s if s.eq_ignore_ascii_case("text") => TokenKind::TextKw,
        s if s.eq_ignore_ascii_case("date") => TokenKind::DateKw,
        s if s.eq_ignore_ascii_case("time") => TokenKind::TimeKw,
        s if s.eq_ignore_ascii_case("boolean") => TokenKind::BooleanKw,
        s if s.eq_ignore_ascii_case("ratio") => TokenKind::RatioKw,
        s if s.eq_ignore_ascii_case("sqrt") => TokenKind::Sqrt,
        s if s.eq_ignore_ascii_case("sin") => TokenKind::Sin,
        s if s.eq_ignore_ascii_case("cos") => TokenKind::Cos,
        s if s.eq_ignore_ascii_case("tan") => TokenKind::Tan,
        s if s.eq_ignore_ascii_case("asin") => TokenKind::Asin,
        s if s.eq_ignore_ascii_case("acos") => TokenKind::Acos,
        s if s.eq_ignore_ascii_case("atan") => TokenKind::Atan,
        s if s.eq_ignore_ascii_case("log") => TokenKind::Log,
        s if s.eq_ignore_ascii_case("exp") => TokenKind::Exp,
        s if s.eq_ignore_ascii_case("abs") => TokenKind::Abs,
        s if s.eq_ignore_ascii_case("floor") => TokenKind::Floor,
        s if s.eq_ignore_ascii_case("ceil") => TokenKind::Ceil,
        s if s.eq_ignore_ascii_case("round") => TokenKind::Round,
        s if s.eq_ignore_ascii_case("is") => TokenKind::Is,
        s if s.eq_ignore_ascii_case("permille") => TokenKind::Permille,
        _ => TokenKind::Identifier,
    }
}

/// Structural keywords can never be used as identifiers (data/rule names).
/// Type keywords (measure, number, text, date, time, boolean, ratio)
/// are reserved and cannot be used as names.
pub fn is_keyword(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Spec
            | TokenKind::Repo
            | TokenKind::Data
            | TokenKind::Rule
            | TokenKind::Unless
            | TokenKind::Then
            | TokenKind::Not
            | TokenKind::And
            | TokenKind::In
            | TokenKind::As
            | TokenKind::Uses
            | TokenKind::With
            | TokenKind::Meta
            | TokenKind::Veto
            | TokenKind::Now
            | TokenKind::Sqrt
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
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Yes
            | TokenKind::No
            | TokenKind::MeasureKw
            | TokenKind::NumberKw
            | TokenKind::TextKw
            | TokenKind::DateKw
            | TokenKind::TimeKw
            | TokenKind::BooleanKw
            | TokenKind::RatioKw
    )
}

/// Map type keyword token to PrimitiveKind. Single source of truth for type keywords.
#[must_use]
pub fn token_kind_to_primitive(kind: &TokenKind) -> Option<PrimitiveKind> {
    match kind {
        TokenKind::BooleanKw => Some(PrimitiveKind::Boolean),
        TokenKind::MeasureKw => Some(PrimitiveKind::Measure),
        TokenKind::NumberKw => Some(PrimitiveKind::Number),
        TokenKind::RatioKw => Some(PrimitiveKind::Ratio),
        TokenKind::TextKw => Some(PrimitiveKind::Text),
        TokenKind::DateKw => Some(PrimitiveKind::Date),
        TokenKind::TimeKw => Some(PrimitiveKind::Time),
        _ => None,
    }
}

/// Returns true if the token kind represents a boolean literal keyword.
pub fn is_boolean_keyword(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::True | TokenKind::False | TokenKind::Yes | TokenKind::No
    )
}

/// Maps a boolean-keyword token kind to BooleanValue. Call only when `is_boolean_keyword(kind)`.
#[must_use]
pub fn token_kind_to_boolean_value(kind: &TokenKind) -> BooleanValue {
    match kind {
        TokenKind::True => BooleanValue::True,
        TokenKind::False => BooleanValue::False,
        TokenKind::Yes => BooleanValue::Yes,
        TokenKind::No => BooleanValue::No,
        _ => unreachable!(
            "BUG: token_kind_to_boolean_value called with non-boolean token {:?}",
            kind
        ),
    }
}

/// Returns true if the token kind represents a math function keyword.
pub fn is_math_function(kind: &TokenKind) -> bool {
    matches!(
        kind,
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
    )
}

/// Returns true if the token kind can start the body of a spec
/// (`data`, `with`, `rule`, `meta`, or `uses`).
pub fn is_spec_body_keyword(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Data
            | TokenKind::With
            | TokenKind::Rule
            | TokenKind::Meta
            | TokenKind::Uses
    )
}

/// Returns true if the token kind can be used as a label or reference segment
/// (identifier, or non-reserved contextual keyword such as `past` / `future` /
/// `permille` / `is`).
pub fn can_be_label(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Identifier
            | TokenKind::Past
            | TokenKind::Future
            | TokenKind::Permille
            | TokenKind::Is
    )
}

/// `calendar` in `in calendar month` / `past calendar year` — not a type keyword.
#[must_use]
pub fn token_is_calendar_period_marker(tok: &Token) -> bool {
    tok.kind == TokenKind::Identifier && tok.text == "calendar"
}

/// Slash-/dot-separated registry path segments (`@org/repo/...`). Keywords that are
/// reserved at the structural level (`spec`, `rule`, etc.) are allowed inside
/// multi-segment paths (e.g. `@org/repo`) but callers must reject them when they
/// appear as the entire stand-alone name (e.g. `repo spec`).
#[must_use]
pub fn can_be_repository_qualifier_segment(kind: &TokenKind) -> bool {
    matches!(kind, TokenKind::Identifier)
        || is_keyword(kind)
        || can_be_label(kind)
        || is_boolean_keyword(kind)
        || is_math_function(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lex_all(input: &str) -> Result<Vec<Token>, Error> {
        let mut lexer = Lexer::new(input, &crate::parsing::source::SourceType::Volatile);
        let mut tokens = Vec::new();
        loop {
            let token = lexer.next_token()?;
            if token.kind == TokenKind::Eof {
                tokens.push(token);
                break;
            }
            tokens.push(token);
        }
        Ok(tokens)
    }

    fn lex_kinds(input: &str) -> Result<Vec<TokenKind>, Error> {
        Ok(lex_all(input)?.into_iter().map(|t| t.kind).collect())
    }

    #[test]
    fn lex_empty_input() {
        let tokens = lex_all("").unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Eof);
    }

    #[test]
    fn string_literal_at_max_length_is_accepted() {
        let content = "a".repeat(crate::limits::MAX_TEXT_VALUE_LENGTH);
        let tokens = lex_all(&format!("\"{content}\"")).unwrap();
        assert_eq!(tokens[0].kind, TokenKind::StringLit);
    }

    #[test]
    fn string_literal_over_max_length_is_parse_error() {
        let content = "a".repeat(crate::limits::MAX_TEXT_VALUE_LENGTH + 1);
        let err = lex_all(&format!("\"{content}\"")).unwrap_err();
        assert!(
            err.message().contains("maximum length"),
            "expected length error, got: {err}"
        );
        assert!(err.location().is_some(), "parse error must carry a source");
    }

    #[test]
    fn number_literal_with_separators_lexes() {
        let tokens = lex_all("9,999,999,999,999,999,999,999,999,999").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::NumberLit);
    }

    #[test]
    fn lex_spec_declaration() {
        let kinds = lex_kinds("spec person").unwrap();
        assert_eq!(
            kinds,
            vec![TokenKind::Spec, TokenKind::Identifier, TokenKind::Eof]
        );
    }

    #[test]
    fn lex_data_definition() {
        let kinds = lex_kinds("data age: 25").unwrap();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Data,
                TokenKind::Identifier,
                TokenKind::Colon,
                TokenKind::NumberLit,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn lex_rule_with_comparison() {
        let kinds = lex_kinds("rule is_adult: age >= 18").unwrap();
        assert_eq!(
            kinds,
            vec![
                TokenKind::Rule,
                TokenKind::Identifier,
                TokenKind::Colon,
                TokenKind::Identifier,
                TokenKind::Gte,
                TokenKind::NumberLit,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn lex_string_literal() {
        let tokens = lex_all(r#""hello world""#).unwrap();
        assert_eq!(tokens[0].kind, TokenKind::StringLit);
        assert_eq!(tokens[0].text, "hello world");
    }

    #[test]
    fn lex_unterminated_string() {
        let result = lex_all(r#""hello"#);
        assert!(result.is_err());
    }

    #[test]
    fn lex_number_with_decimal() {
        let tokens = lex_all("3.14").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::NumberLit);
        assert_eq!(tokens[0].text, "3.14");
    }

    #[test]
    fn lex_number_with_underscores() {
        let tokens = lex_all("1_000_000").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::NumberLit);
        assert_eq!(tokens[0].text, "1_000_000");
    }

    #[test]
    fn lex_scientific_notation() {
        let tokens = lex_all("1.5e+10").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::NumberLit);
        assert_eq!(tokens[0].text, "1.5e+10");
    }

    #[test]
    fn lex_all_operators() {
        let kinds = lex_kinds("+ - * / % ^ > < >= <= -> %%").unwrap();
        assert_eq!(
            &kinds[..12],
            &[
                TokenKind::Plus,
                TokenKind::Minus,
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::Percent,
                TokenKind::Caret,
                TokenKind::Gt,
                TokenKind::Lt,
                TokenKind::Gte,
                TokenKind::Lte,
                TokenKind::Arrow,
                TokenKind::PercentPercent,
            ]
        );
    }

    #[test]
    fn lex_keywords() {
        let kinds =
            lex_kinds("spec data rule unless then not and in as uses meta veto now").unwrap();
        assert_eq!(
            &kinds[..13],
            &[
                TokenKind::Spec,
                TokenKind::Data,
                TokenKind::Rule,
                TokenKind::Unless,
                TokenKind::Then,
                TokenKind::Not,
                TokenKind::And,
                TokenKind::In,
                TokenKind::As,
                TokenKind::Uses,
                TokenKind::Meta,
                TokenKind::Veto,
                TokenKind::Now,
            ]
        );
    }

    #[test]
    fn lex_boolean_keywords() {
        let kinds = lex_kinds("true false yes no").unwrap();
        assert_eq!(
            &kinds[..4],
            &[
                TokenKind::True,
                TokenKind::False,
                TokenKind::Yes,
                TokenKind::No,
            ]
        );
    }

    #[test]
    fn lex_duration_keywords() {
        let kinds = lex_kinds("year month week day hour minute second").unwrap();
        assert_eq!(
            &kinds[..7],
            &[
                TokenKind::Identifier,
                TokenKind::Identifier,
                TokenKind::Identifier,
                TokenKind::Identifier,
                TokenKind::Identifier,
                TokenKind::Identifier,
                TokenKind::Identifier,
            ]
        );
    }

    #[test]
    fn lex_commentary() {
        let tokens = lex_all(r#""""hello world""""#).unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Commentary);
        assert_eq!(tokens[0].text, "hello world");
    }

    #[test]
    fn lex_at_sign() {
        let kinds = lex_kinds("@user").unwrap();
        assert_eq!(kinds[0], TokenKind::At);
        assert_eq!(kinds[1], TokenKind::Identifier);
    }

    #[test]
    fn lex_parentheses() {
        let kinds = lex_kinds("(x + 1)").unwrap();
        assert_eq!(
            &kinds[..5],
            &[
                TokenKind::LParen,
                TokenKind::Identifier,
                TokenKind::Plus,
                TokenKind::NumberLit,
                TokenKind::RParen,
            ]
        );
    }

    #[test]
    fn lex_dot_for_references() {
        let kinds = lex_kinds("employee.salary").unwrap();
        assert_eq!(
            &kinds[..3],
            &[TokenKind::Identifier, TokenKind::Dot, TokenKind::Identifier]
        );
    }

    #[test]
    fn lex_spec_name_with_slashes() {
        let tokens = lex_all("spec contracts/employment/jack").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Spec);
        // The lexer will see "contracts" as identifier, then "/" as Slash
        // The parser will handle assembling the spec name.
        assert_eq!(tokens[1].kind, TokenKind::Identifier);
    }

    #[test]
    fn lex_number_not_followed_by_e_identifier() {
        // "42 eur" should be number then identifier, not scientific notation
        let tokens = lex_all("42 eur").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::NumberLit);
        assert_eq!(tokens[0].text, "42");
        assert_eq!(tokens[1].kind, TokenKind::Identifier);
        assert_eq!(tokens[1].text, "eur");
    }

    #[test]
    fn lex_unknown_character() {
        let result = lex_all("§");
        assert!(result.is_err());
    }

    #[test]
    fn lex_peek_does_not_consume() {
        let mut lexer = Lexer::new("spec test", &crate::parsing::source::SourceType::Volatile);
        let peeked_kind = lexer.peek().unwrap().kind.clone();
        assert_eq!(peeked_kind, TokenKind::Spec);
        let next = lexer.next_token().unwrap();
        assert_eq!(next.kind, TokenKind::Spec);
    }

    #[test]
    fn lex_span_byte_offsets() {
        let tokens = lex_all("spec test").unwrap();
        assert_eq!(tokens[0].span.start, 0);
        assert_eq!(tokens[0].span.end, 4);
        assert_eq!(tokens[0].span.line, 1);
        assert_eq!(tokens[0].span.col, 1);

        assert_eq!(tokens[1].span.start, 5);
        assert_eq!(tokens[1].span.end, 9);
        assert_eq!(tokens[1].span.line, 1);
        assert_eq!(tokens[1].span.col, 6);
    }

    #[test]
    fn lex_multiline_span_tracking() {
        let tokens = lex_all("spec test\ndata x: 1").unwrap();
        // "data" should be on line 2
        let data_token = &tokens[2]; // spec, test, data
        assert_eq!(data_token.kind, TokenKind::Data);
        assert_eq!(data_token.span.line, 2);
        assert_eq!(data_token.span.col, 1);
    }

    #[test]
    fn lex_case_insensitive_keywords() {
        // Lemma keywords are case-insensitive
        let kinds = lex_kinds("SPEC Data RULE").unwrap();
        assert_eq!(kinds[0], TokenKind::Spec);
        assert_eq!(kinds[1], TokenKind::Data);
        assert_eq!(kinds[2], TokenKind::Rule);
    }

    #[test]
    fn lex_math_function_keywords() {
        let kinds =
            lex_kinds("sqrt sin cos tan asin acos atan log exp abs floor ceil round").unwrap();
        assert_eq!(
            &kinds[..13],
            &[
                TokenKind::Sqrt,
                TokenKind::Sin,
                TokenKind::Cos,
                TokenKind::Tan,
                TokenKind::Asin,
                TokenKind::Acos,
                TokenKind::Atan,
                TokenKind::Log,
                TokenKind::Exp,
                TokenKind::Abs,
                TokenKind::Floor,
                TokenKind::Ceil,
                TokenKind::Round,
            ]
        );
    }

    #[test]
    fn lex_is_keyword() {
        let kinds = lex_kinds("status is \"active\"").unwrap();
        assert_eq!(kinds[0], TokenKind::Identifier);
        assert_eq!(kinds[1], TokenKind::Is);
        assert_eq!(kinds[2], TokenKind::StringLit);
    }

    #[test]
    fn lex_percent_not_followed_by_digit() {
        // "50%" should be number then percent
        let kinds = lex_kinds("50%").unwrap();
        assert_eq!(kinds[0], TokenKind::NumberLit);
        assert_eq!(kinds[1], TokenKind::Percent);
    }

    #[test]
    fn lex_number_with_commas() {
        let tokens = lex_all("1,000,000").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::NumberLit);
        assert_eq!(tokens[0].text, "1,000,000");
    }

    #[test]
    fn lex_arrow_chain() {
        let kinds = lex_kinds("-> unit eur: 1.00 -> decimals 2").unwrap();
        assert_eq!(kinds[0], TokenKind::Arrow);
        assert_eq!(kinds[1], TokenKind::Identifier);
        assert_eq!(kinds[2], TokenKind::Identifier);
        assert_eq!(kinds[3], TokenKind::Colon);
        assert_eq!(kinds[4], TokenKind::NumberLit);
        assert_eq!(kinds[5], TokenKind::Arrow);
    }
}
