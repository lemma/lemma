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

    // List keywords
    List,
    Key,
    Find,
    First,
    Last,
    Sum,
    Avg,
    Product,
    Count,
    Min,
    Max,
    Any,
    All,
    Where,
    Sort,
    Top,
    Asc,
    Desc,

    // Range endpoint operators
    Lower,
    Upper,

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
    Mod,
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
    LParen,
    RParen,

    // Literals
    NumberLit,
    StringLit,

    // Commentary (raw text between """ delimiters)
    Commentary,

    // Identifiers
    Identifier,

    /// Opaque spec / repo / registry name: optional `@` + `[A-Za-z0-9_./-]+`.
    LogicalName,

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
            TokenKind::List => write!(f, "'list'"),
            TokenKind::Key => write!(f, "'key'"),
            TokenKind::Find => write!(f, "'find'"),
            TokenKind::First => write!(f, "'first'"),
            TokenKind::Last => write!(f, "'last'"),
            TokenKind::Sum => write!(f, "'sum'"),
            TokenKind::Avg => write!(f, "'avg'"),
            TokenKind::Product => write!(f, "'product'"),
            TokenKind::Count => write!(f, "'count'"),
            TokenKind::Min => write!(f, "'min'"),
            TokenKind::Max => write!(f, "'max'"),
            TokenKind::Any => write!(f, "'any'"),
            TokenKind::All => write!(f, "'all'"),
            TokenKind::Where => write!(f, "'where'"),
            TokenKind::Sort => write!(f, "'sort'"),
            TokenKind::Top => write!(f, "'top'"),
            TokenKind::Asc => write!(f, "'asc'"),
            TokenKind::Desc => write!(f, "'desc'"),
            TokenKind::Lower => write!(f, "'lower'"),
            TokenKind::Upper => write!(f, "'upper'"),
            TokenKind::Permille => write!(f, "'permille'"),
            TokenKind::Is => write!(f, "'is'"),
            TokenKind::Plus => write!(f, "'+'"),
            TokenKind::Minus => write!(f, "'-'"),
            TokenKind::Star => write!(f, "'*'"),
            TokenKind::Slash => write!(f, "'/'"),
            TokenKind::Comma => write!(f, "','"),
            TokenKind::Percent => write!(f, "'%'"),
            TokenKind::PercentPercent => write!(f, "'%%'"),
            TokenKind::Mod => write!(f, "'mod'"),
            TokenKind::Caret => write!(f, "'^'"),
            TokenKind::Gt => write!(f, "'>'"),
            TokenKind::Lt => write!(f, "'<'"),
            TokenKind::Gte => write!(f, "'>='"),
            TokenKind::Lte => write!(f, "'<='"),
            TokenKind::Colon => write!(f, "':'"),
            TokenKind::Arrow => write!(f, "'->'"),
            TokenKind::Ellipsis => write!(f, "'...'"),
            TokenKind::Dot => write!(f, "'.'"),
            TokenKind::LParen => write!(f, "'('"),
            TokenKind::RParen => write!(f, "')'"),
            TokenKind::NumberLit => write!(f, "a number"),
            TokenKind::StringLit => write!(f, "a string"),
            TokenKind::Commentary => write!(f, "commentary block"),
            TokenKind::Identifier => write!(f, "an identifier"),
            TokenKind::LogicalName => write!(f, "a name"),
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

    /// Opaque spec / repo / registry name for name positions only.
    ///
    /// If a normal peek already materialized a prefix token, rewind and rescan
    /// so connectors (`/`, `.`, `-`) are absorbed into one [`TokenKind::LogicalName`].
    pub fn next_logical_name(&mut self) -> Result<Token, Error> {
        if let Some(tok) = &self.peeked {
            if tok.kind == TokenKind::LogicalName {
                return self.next_token();
            }
            let start_byte = tok.span.start;
            let start_line = tok.span.line;
            let start_col = tok.span.col;
            self.peeked = None;
            self.peeked2 = None;
            self.rewind_to(start_byte, start_line, start_col);
            return self.scan_integral_logical_name(start_byte, start_line, start_col);
        }
        self.skip_whitespace();
        let start_byte = self.byte_offset;
        let start_line = self.line;
        let start_col = self.col;
        self.scan_integral_logical_name(start_byte, start_line, start_col)
    }

    fn rewind_to(&mut self, byte_offset: usize, line: usize, col: usize) {
        let mut pos = 0;
        let mut bytes = 0;
        while bytes < byte_offset && pos < self.source.len() {
            bytes += self.source[pos].len_utf8();
            pos += 1;
        }
        self.pos = pos;
        self.byte_offset = byte_offset;
        self.line = line;
        self.col = col;
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

        // Identifier or keyword
        if ch.is_ascii_alphabetic() || ch == '_' {
            return Ok(self.scan_identifier(start_byte, start_line, start_col));
        }

        // Registry qualifier: `@` + opaque name characters
        if ch == '@' {
            return self.scan_integral_logical_name(start_byte, start_line, start_col);
        }

        // Unknown character
        self.advance();
        let span = self.make_span(start_byte, start_line, start_col);
        Err(self.make_error(format!("Unexpected character '{}'", ch), span))
    }

    fn is_logical_name_char(ch: char) -> bool {
        ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '/' | '-')
    }

    fn scan_integral_logical_name(
        &mut self,
        start_byte: usize,
        start_line: usize,
        start_col: usize,
    ) -> Result<Token, Error> {
        let mut text = String::new();
        let had_at = self.current_char() == Some('@');
        if had_at {
            text.push('@');
            self.advance();
        }
        let Some(first) = self.current_char() else {
            let span = self.make_span(start_byte, start_line, start_col);
            return Err(self.make_error(
                if had_at {
                    "Expected a name after '@'"
                } else {
                    "Expected a name"
                },
                span,
            ));
        };
        if !Self::is_logical_name_char(first) {
            let span = self.make_span(start_byte, start_line, start_col);
            return Err(self.make_error(
                if had_at {
                    format!("Expected a name after '@', found '{}'", first)
                } else {
                    format!("Expected a name, found '{}'", first)
                },
                span,
            ));
        }
        while let Some(ch) = self.current_char() {
            if Self::is_logical_name_char(ch) {
                text.push(ch);
                self.advance();
            } else {
                break;
            }
        }
        let span = self.make_span(start_byte, start_line, start_col);
        Ok(Token {
            kind: TokenKind::LogicalName,
            span,
            text,
        })
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

        // Integer part: digits with optional `_` separators
        while let Some(ch) = self.current_char() {
            if ch.is_ascii_digit() || ch == '_' {
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

macro_rules! lemma_keywords {
    ($(($spell:literal, $kind:ident)),+ $(,)?) => {
        /// Spellings shared by [`keyword_from_identifier`] and LSP completion.
        pub const COMPLETION_KEYWORD_LABELS: &[&str] = &[$($spell),+];

        fn keyword_from_identifier(text: &str) -> TokenKind {
            match text {
                $(s if s.eq_ignore_ascii_case($spell) => TokenKind::$kind,)+
                _ => TokenKind::Identifier,
            }
        }
    };
}

lemma_keywords! {
    ("spec", Spec),
    ("repo", Repo),
    ("data", Data),
    ("rule", Rule),
    ("unless", Unless),
    ("then", Then),
    ("not", Not),
    ("and", And),
    ("in", In),
    ("as", As),
    ("uses", Uses),
    ("with", With),
    ("meta", Meta),
    ("veto", Veto),
    ("now", Now),
    ("past", Past),
    ("future", Future),
    ("true", True),
    ("false", False),
    ("yes", Yes),
    ("no", No),
    ("measure", MeasureKw),
    ("number", NumberKw),
    ("text", TextKw),
    ("date", DateKw),
    ("time", TimeKw),
    ("boolean", BooleanKw),
    ("ratio", RatioKw),
    ("sqrt", Sqrt),
    ("sin", Sin),
    ("cos", Cos),
    ("tan", Tan),
    ("asin", Asin),
    ("acos", Acos),
    ("atan", Atan),
    ("log", Log),
    ("exp", Exp),
    ("abs", Abs),
    ("floor", Floor),
    ("ceil", Ceil),
    ("round", Round),
    ("list", List),
    ("key", Key),
    ("find", Find),
    ("first", First),
    ("last", Last),
    ("sum", Sum),
    ("avg", Avg),
    ("product", Product),
    ("count", Count),
    ("min", Min),
    ("max", Max),
    ("any", Any),
    ("all", All),
    ("where", Where),
    ("sort", Sort),
    ("top", Top),
    ("asc", Asc),
    ("desc", Desc),
    ("lower", Lower),
    ("upper", Upper),
    ("is", Is),
    ("permille", Permille),
    ("mod", Mod),
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
            | TokenKind::Is
            | TokenKind::Past
            | TokenKind::Future
            | TokenKind::Permille
            | TokenKind::Mod
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
            | TokenKind::Upper
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
/// (`data`, `rule`, `meta`, or `uses`).
pub fn is_spec_body_keyword(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Data | TokenKind::Rule | TokenKind::Meta | TokenKind::Uses
    )
}

/// Returns true if the token kind can be used as a label or reference segment
/// (identifier, or one of the keywords `past` / `future` / `permille` / `is` in label position).
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

/// Returns true if the token kind can name a unit (`5 eur`, `-> unit eur: 1`).
/// `is`, `past`, `future`, `permille` are reserved and never units: after a number they start a
/// comparison, a date window, or a literal suffix.
#[must_use]
pub fn can_be_unit_name(kind: &TokenKind) -> bool {
    matches!(kind, TokenKind::Identifier)
}

/// True when a bare (non-`@`) repository name spelling is a reserved keyword.
#[must_use]
pub fn is_reserved_repository_name(name: &str) -> bool {
    is_keyword(&keyword_from_identifier(name))
}

/// True when the opaque spelling is allowed as a `spec` name: a plain identifier, not a keyword.
#[must_use]
pub fn can_be_spec_name_spelling(name: &str) -> bool {
    matches!(keyword_from_identifier(name), TokenKind::Identifier)
}

/// `calendar` in `in calendar month` / `past calendar year` — not a type keyword.
#[must_use]
pub fn token_is_calendar_period_marker(tok: &Token) -> bool {
    tok.kind == TokenKind::Identifier && tok.text == "calendar"
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
    fn completion_keyword_labels_match_identifier_lex() {
        for label in COMPLETION_KEYWORD_LABELS {
            let kinds = lex_kinds(label).unwrap();
            assert_eq!(
                kinds.last(),
                Some(&TokenKind::Eof),
                "keyword {label} must be a single token"
            );
            assert_eq!(kinds.len(), 2, "keyword {label} lexed as {kinds:?}");
            assert_ne!(
                kinds[0],
                TokenKind::Identifier,
                "catalog entry {label} must be a keyword token"
            );
        }
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
        let tokens = lex_all("9_999_999_999_999_999_999_999_999_999").unwrap();
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
    fn lex_registry_logical_name() {
        let tokens = lex_all("@iso/countries").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::LogicalName);
        assert_eq!(tokens[0].text, "@iso/countries");
        assert_eq!(tokens[1].kind, TokenKind::Eof);
    }

    #[test]
    fn lex_bare_at_is_error() {
        assert!(lex_all("@").is_err());
    }

    #[test]
    fn next_logical_name_absorbs_slashes() {
        let mut lexer = Lexer::new(
            "contracts/employment/jack",
            &crate::parsing::source::SourceType::Volatile,
        );
        let tok = lexer.next_logical_name().unwrap();
        assert_eq!(tok.kind, TokenKind::LogicalName);
        assert_eq!(tok.text, "contracts/employment/jack");
    }

    #[test]
    fn next_logical_name_after_peeked_identifier_rewinds() {
        let mut lexer = Lexer::new(
            "finance/units",
            &crate::parsing::source::SourceType::Volatile,
        );
        assert_eq!(lexer.peek().unwrap().kind, TokenKind::Identifier);
        let tok = lexer.next_logical_name().unwrap();
        assert_eq!(tok.kind, TokenKind::LogicalName);
        assert_eq!(tok.text, "finance/units");
    }

    #[test]
    fn division_and_units_keep_slash_token() {
        let kinds = lex_kinds("a / b").unwrap();
        assert_eq!(
            &kinds[..3],
            &[
                TokenKind::Identifier,
                TokenKind::Slash,
                TokenKind::Identifier
            ]
        );
        let kinds = lex_kinds("meter/second").unwrap();
        assert_eq!(
            &kinds[..3],
            &[
                TokenKind::Identifier,
                TokenKind::Slash,
                TokenKind::Identifier
            ]
        );
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
    fn lex_spec_keyword_then_slashed_name_via_next_logical_name() {
        let mut lexer = Lexer::new(
            "spec contracts/employment/jack",
            &crate::parsing::source::SourceType::Volatile,
        );
        assert_eq!(lexer.next_token().unwrap().kind, TokenKind::Spec);
        let name = lexer.next_logical_name().unwrap();
        assert_eq!(name.kind, TokenKind::LogicalName);
        assert_eq!(name.text, "contracts/employment/jack");
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
    fn lex_range_bound_keywords() {
        let kinds = lex_kinds("lower upper LOWER").unwrap();
        assert_eq!(
            &kinds[..3],
            &[TokenKind::Lower, TokenKind::Upper, TokenKind::Lower,]
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
        let kinds = lex_kinds("1,000,000").unwrap();
        assert_eq!(
            kinds,
            vec![
                TokenKind::NumberLit,
                TokenKind::Comma,
                TokenKind::NumberLit,
                TokenKind::Comma,
                TokenKind::NumberLit,
                TokenKind::Eof,
            ]
        );
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
