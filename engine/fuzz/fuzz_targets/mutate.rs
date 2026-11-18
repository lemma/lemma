//! Structure-preserving edits. One edit rewrites a token or a line and leaves
//! the rest of the document alone, which is how a person finds a panic.

use lemma::SourceType;
use lemma::TokenKind;

mod fixtures {
    include!(concat!(env!("OUT_DIR"), "/fixtures.rs"));
}

struct Rng(u64);

impl Rng {
    fn new(seed: u32) -> Self {
        Self((u64::from(seed) << 1) | 1)
    }

    fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        (z ^ (z >> 31)) as u32
    }
}

struct Tok {
    kind: TokenKind,
    start: usize,
    end: usize,
}

pub fn mutate(data: &mut [u8], size: usize, max_size: usize, seed: u32) -> usize {
    if max_size == 0 || data.is_empty() {
        return 0;
    }
    let size = size.min(data.len()).min(max_size);
    let mut rng = Rng::new(seed);
    if rng.next_u32() % 4 == 0 {
        return libfuzzer_sys::fuzzer_mutate(data, size, max_size);
    }
    let current = std::str::from_utf8(&data[..size]).ok();
    let keep = current.is_some_and(|text| text.contains("spec")) && rng.next_u32() % 5 != 0;
    let mut text = if keep {
        current
            .expect("BUG: spec-containing utf-8 checked above")
            .to_string()
    } else if let Some(fixture) = pick_fixture(&mut rng, max_size) {
        fixture.to_string()
    } else {
        return libfuzzer_sys::fuzzer_mutate(data, size, max_size);
    };
    let edits = 1 + (rng.next_u32() as usize % 3);
    for _ in 0..edits {
        text = apply_edit(&text, &mut rng);
    }
    write_back(data, max_size, &text)
}

fn pick_fixture(rng: &mut Rng, max_size: usize) -> Option<&'static str> {
    let fitting: Vec<&str> = fixtures::FIXTURES
        .iter()
        .copied()
        .filter(|source| source.len() <= max_size)
        .collect();
    if fitting.is_empty() {
        None
    } else {
        Some(fitting[rng.next_u32() as usize % fitting.len()])
    }
}

fn write_back(data: &mut [u8], max_size: usize, text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut n = bytes.len().min(max_size).min(data.len());
    while n > 0 && !text.is_char_boundary(n) {
        n -= 1;
    }
    data[..n].copy_from_slice(&bytes[..n]);
    n
}

type Edit = fn(&str, Option<&[Tok]>, &mut Rng) -> Option<String>;

const EDITS: &[Edit] = &[
    edit_literal,
    edit_operator,
    edit_wrap,
    edit_cast,
    edit_unless,
    edit_type,
    edit_unit,
    edit_list_command,
    edit_delete_arrow,
    edit_delete_with,
    edit_swap_idents,
    edit_spec_date,
    edit_date_window,
    edit_uses_units,
    edit_is_veto,
];

fn apply_edit(source: &str, rng: &mut Rng) -> String {
    let tokens = lex_tokens(source);
    let start = rng.next_u32() as usize % EDITS.len();
    for offset in 0..EDITS.len() {
        let edit = EDITS[(start + offset) % EDITS.len()];
        if let Some(edited) = edit(source, tokens.as_deref(), rng) {
            return edited;
        }
    }
    source.to_string()
}

fn lex_tokens(source: &str) -> Option<Vec<Tok>> {
    let mut lexer = lemma::Lexer::new(source, &SourceType::Volatile);
    let mut out = Vec::new();
    loop {
        match lexer.next_token() {
            Ok(token) => {
                if token.kind == TokenKind::Eof {
                    break;
                }
                if token.span.end < token.span.start || token.span.end > source.len() {
                    return None;
                }
                out.push(Tok {
                    kind: token.kind,
                    start: token.span.start,
                    end: token.span.end,
                });
            }
            Err(error) => {
                std::hint::black_box(error);
                return None;
            }
        }
    }
    Some(out)
}

fn choose<'a, T>(rng: &mut Rng, items: &'a [T]) -> Option<&'a T> {
    if items.is_empty() {
        None
    } else {
        Some(&items[rng.next_u32() as usize % items.len()])
    }
}

fn replace_range(source: &str, start: usize, end: usize, replacement: &str) -> String {
    let mut out = String::with_capacity(source.len() + replacement.len());
    out.push_str(&source[..start]);
    out.push_str(replacement);
    out.push_str(&source[end..]);
    out
}

fn edge_number(rng: &mut Rng) -> String {
    match rng.next_u32() % 6 {
        0 => "0".to_string(),
        1 => "-1".to_string(),
        2 => "1".to_string(),
        3 => "0.0".to_string(),
        4 => "veto".to_string(),
        _ => {
            let digits = 200 + (rng.next_u32() as usize % 201);
            let mut number = String::from("1");
            for _ in 1..digits {
                number.push(char::from(b'0' + (rng.next_u32() % 10) as u8));
            }
            number
        }
    }
}

fn edit_literal(source: &str, tokens: Option<&[Tok]>, rng: &mut Rng) -> Option<String> {
    let tokens = tokens?;
    let numbers: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| token.kind == TokenKind::NumberLit)
        .map(|(index, _)| index)
        .collect();
    let strings: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| token.kind == TokenKind::StringLit)
        .map(|(index, _)| index)
        .collect();
    if rng.next_u32() % 5 == 0 {
        let index = *choose(rng, &strings)?;
        let token = &tokens[index];
        let replacement = choose(rng, &["\"\"", "\"a\"", "\"é\""])?;
        return Some(replace_range(source, token.start, token.end, replacement));
    }
    let index = *choose(rng, &numbers)?;
    let token = &tokens[index];
    if tokens
        .get(index + 1)
        .is_some_and(|next| next.kind == TokenKind::Percent)
    {
        let end = tokens[index + 1].end;
        let replacement = if rng.next_u32() % 2 == 0 {
            "0%"
        } else {
            "100%"
        };
        return Some(replace_range(source, token.start, end, replacement));
    }
    if let Some(date_end) = date_span_end(source, tokens, index) {
        let replacement = choose(rng, &["0001-01-01", "9999-12-31", "2020-01-01"])?;
        return Some(replace_range(source, token.start, date_end, replacement));
    }
    Some(replace_range(
        source,
        token.start,
        token.end,
        &edge_number(rng),
    ))
}

fn date_span_end(source: &str, tokens: &[Tok], index: usize) -> Option<usize> {
    let first = tokens.get(index)?;
    let second = tokens.get(index + 1)?;
    let third = tokens.get(index + 2)?;
    let fourth = tokens.get(index + 3)?;
    let fifth = tokens.get(index + 4)?;
    if second.kind != TokenKind::Minus
        || third.kind != TokenKind::NumberLit
        || fourth.kind != TokenKind::Minus
        || fifth.kind != TokenKind::NumberLit
    {
        return None;
    }
    let year = &source[first.start..first.end];
    if year.len() == 4 && year.bytes().all(|byte| byte.is_ascii_digit()) {
        Some(fifth.end)
    } else {
        None
    }
}

fn edit_operator(source: &str, tokens: Option<&[Tok]>, rng: &mut Rng) -> Option<String> {
    let tokens = tokens?;
    let mut sites = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            TokenKind::Plus
            | TokenKind::Minus
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Mod
            | TokenKind::Caret
            | TokenKind::Gt
            | TokenKind::Lt
            | TokenKind::Gte
            | TokenKind::Lte
            | TokenKind::And
            | TokenKind::Or => {
                sites.push((index, false));
            }
            TokenKind::Is => sites.push((index, true)),
            _ => {}
        }
    }
    let (index, is_keyword) = *choose(rng, &sites)?;
    let token = &tokens[index];
    if is_keyword {
        let next_is_not = tokens
            .get(index + 1)
            .is_some_and(|next| next.kind == TokenKind::Not);
        if next_is_not {
            let end = tokens[index + 1].end;
            return Some(replace_range(source, token.start, end, "is"));
        }
        return Some(replace_range(source, token.start, token.end, "is not"));
    }
    let group: &[&str] = match token.kind {
        TokenKind::Plus
        | TokenKind::Minus
        | TokenKind::Star
        | TokenKind::Slash
        | TokenKind::Mod
        | TokenKind::Caret => &["+", "-", "*", "/", "mod", "^"],
        TokenKind::Gt | TokenKind::Lt | TokenKind::Gte | TokenKind::Lte => &[">", "<", ">=", "<="],
        TokenKind::And | TokenKind::Or => &["and", "or"],
        _ => return None,
    };
    let replacement = choose(rng, group)?;
    Some(replace_range(source, token.start, token.end, replacement))
}

fn edit_wrap(source: &str, tokens: Option<&[Tok]>, rng: &mut Rng) -> Option<String> {
    let tokens = tokens?;
    let mut spans = Vec::new();
    for token in tokens {
        if token.kind == TokenKind::NumberLit || token.kind == TokenKind::Identifier {
            spans.push((token.start, token.end));
        }
    }
    let mut depth = 0i32;
    let mut start = None;
    for token in tokens {
        if token.kind == TokenKind::LParen {
            if depth == 0 {
                start = Some(token.start);
            }
            depth += 1;
        } else if token.kind == TokenKind::RParen {
            depth -= 1;
            if depth == 0 {
                if let Some(open) = start.take() {
                    spans.push((open, token.end));
                }
            }
            if depth < 0 {
                depth = 0;
            }
        }
    }
    let (start, end) = *choose(rng, &spans)?;
    let prefix = choose(
        rng,
        &["abs", "sqrt", "log", "exp", "sin", "lower", "upper", "not"],
    )?;
    let inner = &source[start..end];
    let wrapped = format!("{prefix}({inner})");
    Some(replace_range(source, start, end, &wrapped))
}

fn edit_cast(source: &str, tokens: Option<&[Tok]>, rng: &mut Rng) -> Option<String> {
    let tokens = tokens?;
    let sites: Vec<&Tok> = tokens
        .iter()
        .filter(|token| {
            matches!(
                token.kind,
                TokenKind::NumberLit | TokenKind::Identifier | TokenKind::RParen
            )
        })
        .collect();
    let token = *choose(rng, &sites)?;
    let cast = choose(
        rng,
        &[" as number", " as gram", " as eur", " as day", " as time"],
    )?;
    Some(replace_range(source, token.end, token.end, cast))
}

fn edit_unless(source: &str, _tokens: Option<&[Tok]>, rng: &mut Rng) -> Option<String> {
    map_lines(source, |lines| {
        let rules: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| {
                let trimmed = line.trim_start();
                trimmed.starts_with("rule ") && trimmed.contains(':')
            })
            .map(|(index, _)| index)
            .collect();
        let rule_at = *choose(rng, &rules)?;
        let body = rule_body(lines, rule_at);
        let clause = match rng.next_u32() % 3 {
            0 => format!("  unless true then {body}"),
            1 => "  unless false then veto".to_string(),
            _ => format!("  unless {body} is veto then {body}"),
        };
        let header = lines[rule_at].trim_start();
        let header_has_body = header
            .split_once(':')
            .is_some_and(|(_, rest)| !rest.trim().is_empty());
        let insert_at = if header_has_body {
            rule_at + 1
        } else {
            (rule_at + 2).min(lines.len())
        };
        lines.insert(insert_at, clause);
        Some(())
    })
}

fn rule_body(lines: &[String], rule_at: usize) -> String {
    let header = lines[rule_at].trim_start();
    let inline = header
        .split_once(':')
        .map(|(_, rest)| rest.trim())
        .unwrap_or("");
    if !inline.is_empty() {
        return inline.to_string();
    }
    if let Some(next) = lines.get(rule_at + 1) {
        let trimmed = next.trim();
        if !trimmed.is_empty() && !starts_declaration(trimmed) {
            return trimmed.to_string();
        }
    }
    "0".to_string()
}

fn starts_declaration(trimmed: &str) -> bool {
    trimmed.starts_with("rule ")
        || trimmed.starts_with("data ")
        || trimmed.starts_with("spec ")
        || trimmed.starts_with("uses ")
        || trimmed.starts_with("meta ")
        || trimmed.starts_with("repo ")
}

fn edit_type(source: &str, tokens: Option<&[Tok]>, rng: &mut Rng) -> Option<String> {
    let tokens = tokens?;
    const TYPES: &[&str] = &[
        "number", "text", "boolean", "measure", "ratio", "time", "date",
    ];
    let sites: Vec<&Tok> = tokens
        .iter()
        .filter(|token| {
            matches!(
                token.kind,
                TokenKind::NumberKw
                    | TokenKind::TextKw
                    | TokenKind::BooleanKw
                    | TokenKind::MeasureKw
                    | TokenKind::RatioKw
                    | TokenKind::TimeKw
                    | TokenKind::DateKw
            )
        })
        .collect();
    let token = *choose(rng, &sites)?;
    let replacement = choose(rng, TYPES)?;
    Some(replace_range(source, token.start, token.end, replacement))
}

fn edit_unit(source: &str, tokens: Option<&[Tok]>, rng: &mut Rng) -> Option<String> {
    let tokens = tokens?;
    const UNITS: &[&str] = &[
        "kilogram", "gram", "eur", "meter", "second", "day", "hour", "minute", "percent",
    ];
    let sites: Vec<&Tok> = tokens
        .iter()
        .filter(|token| {
            token.kind == TokenKind::Identifier
                && UNITS
                    .iter()
                    .any(|unit| source[token.start..token.end].eq_ignore_ascii_case(unit))
        })
        .collect();
    let token = *choose(rng, &sites)?;
    let replacement = choose(rng, UNITS)?;
    Some(replace_range(source, token.start, token.end, replacement))
}

fn edit_list_command(source: &str, tokens: Option<&[Tok]>, rng: &mut Rng) -> Option<String> {
    let tokens = tokens?;
    const COMMANDS: &[&str] = &[
        "sum", "avg", "product", "min", "max", "count", "any", "all", "where", "sort", "top",
        "find", "first", "last",
    ];
    let sites: Vec<&Tok> = tokens
        .iter()
        .filter(|token| {
            matches!(
                token.kind,
                TokenKind::Sum
                    | TokenKind::Avg
                    | TokenKind::Product
                    | TokenKind::Min
                    | TokenKind::Max
                    | TokenKind::Count
                    | TokenKind::Any
                    | TokenKind::All
                    | TokenKind::Where
                    | TokenKind::Sort
                    | TokenKind::Top
                    | TokenKind::Find
                    | TokenKind::First
                    | TokenKind::Last
            )
        })
        .collect();
    let token = *choose(rng, &sites)?;
    let replacement = choose(rng, COMMANDS)?;
    Some(replace_range(source, token.start, token.end, replacement))
}

fn edit_delete_arrow(source: &str, _tokens: Option<&[Tok]>, rng: &mut Rng) -> Option<String> {
    map_lines(source, |lines| {
        let arrows: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.trim_start().starts_with("->"))
            .map(|(index, _)| index)
            .collect();
        let index = *choose(rng, &arrows)?;
        lines.remove(index);
        Some(())
    })
}

fn edit_delete_with(source: &str, _tokens: Option<&[Tok]>, _rng: &mut Rng) -> Option<String> {
    map_lines(source, |lines| {
        let before = lines.len();
        lines.retain(|line| !line.trim_start().starts_with("-> with"));
        if lines.len() == before {
            None
        } else {
            Some(())
        }
    })
}

fn edit_swap_idents(source: &str, tokens: Option<&[Tok]>, rng: &mut Rng) -> Option<String> {
    let tokens = tokens?;
    let idents: Vec<&Tok> = tokens
        .iter()
        .filter(|token| token.kind == TokenKind::Identifier && token.end > token.start)
        .collect();
    if idents.len() < 2 {
        return None;
    }
    let left = rng.next_u32() as usize % idents.len();
    let mut right = rng.next_u32() as usize % idents.len();
    if right == left {
        right = (right + 1) % idents.len();
    }
    let (first, second) = if idents[left].start <= idents[right].start {
        (idents[left], idents[right])
    } else {
        (idents[right], idents[left])
    };
    if first.end > second.start {
        return None;
    }
    let left_text = &source[first.start..first.end];
    let right_text = &source[second.start..second.end];
    if left_text.eq_ignore_ascii_case(right_text) {
        return None;
    }
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..first.start]);
    out.push_str(right_text);
    out.push_str(&source[first.end..second.start]);
    out.push_str(left_text);
    out.push_str(&source[second.end..]);
    Some(out)
}

fn edit_spec_date(source: &str, tokens: Option<&[Tok]>, rng: &mut Rng) -> Option<String> {
    let tokens = tokens?;
    let specs: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter(|(_, token)| token.kind == TokenKind::Spec)
        .map(|(index, _)| index)
        .collect();
    let index = *choose(rng, &specs)?;
    let name = tokens.get(index + 1)?;
    if !matches!(name.kind, TokenKind::Identifier | TokenKind::LogicalName) {
        return None;
    }
    let line_end = source[name.end..]
        .find('\n')
        .map(|offset| name.end + offset)
        .unwrap_or(source.len());
    let date = choose(rng, &["2020-01-01", "0001-01-01", "9999-12-31"])?;
    let replacement = format!(" {date}");
    Some(replace_range(source, name.end, line_end, &replacement))
}

fn edit_date_window(source: &str, _tokens: Option<&[Tok]>, rng: &mut Rng) -> Option<String> {
    let mut dates = Vec::new();
    let bytes = source.as_bytes();
    let mut index = 0;
    while index + 10 <= bytes.len() {
        if source.is_char_boundary(index)
            && source.is_char_boundary(index + 10)
            && is_iso_date(&bytes[index..index + 10])
        {
            dates.push(index);
            index += 10;
        } else {
            index += 1;
        }
    }
    let start = *choose(rng, &dates)?;
    match rng.next_u32() % 3 {
        0 => Some(replace_range(source, start + 10, start + 10, " in past")),
        1 => Some(replace_range(
            source,
            start + 10,
            start + 10,
            " in future calendar year",
        )),
        _ => Some(replace_range(source, start, start + 10, "past 7 day")),
    }
}

fn is_iso_date(bytes: &[u8]) -> bool {
    bytes.len() == 10
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

fn edit_uses_units(source: &str, _tokens: Option<&[Tok]>, _rng: &mut Rng) -> Option<String> {
    if source.to_ascii_lowercase().contains("uses lemma units") {
        return None;
    }
    map_lines(source, |lines| {
        let spec_at = lines
            .iter()
            .position(|line| line.trim_start().starts_with("spec "))?;
        let mut insert_at = spec_at + 1;
        if lines
            .get(insert_at)
            .is_some_and(|line| line.trim() == "\"\"\"")
        {
            insert_at += 1;
            while insert_at < lines.len() && lines[insert_at].trim() != "\"\"\"" {
                insert_at += 1;
            }
            if insert_at < lines.len() {
                insert_at += 1;
            }
        }
        lines.insert(insert_at, "uses lemma units".to_string());
        Some(())
    })
}

fn edit_is_veto(source: &str, tokens: Option<&[Tok]>, rng: &mut Rng) -> Option<String> {
    let tokens = tokens?;
    let sites: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter(|(index, token)| {
            token.kind == TokenKind::Identifier
                && !tokens.get(index.wrapping_sub(1)).is_some_and(|prev| {
                    matches!(
                        prev.kind,
                        TokenKind::Data
                            | TokenKind::Rule
                            | TokenKind::Spec
                            | TokenKind::Uses
                            | TokenKind::Repo
                            | TokenKind::Meta
                    )
                })
        })
        .map(|(index, _)| index)
        .collect();
    let index = *choose(rng, &sites)?;
    let token = &tokens[index];
    let suffix = if rng.next_u32() % 2 == 0 {
        " is veto"
    } else {
        " is not veto"
    };
    Some(replace_range(source, token.end, token.end, suffix))
}

fn map_lines(source: &str, edit: impl FnOnce(&mut Vec<String>) -> Option<()>) -> Option<String> {
    let mut lines: Vec<String> = source.lines().map(str::to_string).collect();
    edit(&mut lines)?;
    let mut out = lines.join("\n");
    if source.ends_with('\n') {
        out.push('\n');
    }
    Some(out)
}
