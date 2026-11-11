#![no_main]

use lemma::DateTimeValue;
use lemma::Engine;
use lemma::IndexMap;
use libfuzzer_sys::fuzz_target;
use std::str::FromStr;

fn fixed_now() -> DateTimeValue {
    DateTimeValue::from_str("2024-06-15T12:00:00Z").expect("BUG: fixed fuzz datetime")
}

struct Cursor<'a> {
    bytes: &'a [u8],
    index: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, index: 0 }
    }

    fn next_u8(&mut self) -> u8 {
        let byte = self.bytes.get(self.index).copied().unwrap_or(0);
        self.index = self.index.saturating_add(1);
        byte
    }

    fn take(&mut self, len: usize) -> &'a [u8] {
        let start = self.index;
        let end = (start + len).min(self.bytes.len());
        self.index = end;
        &self.bytes[start..end]
    }
}

fn number_literal(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 8 {
        0 => "0".to_string(),
        1 => "1".to_string(),
        2 => "-1".to_string(),
        3 => "42".to_string(),
        4 => "0.5".to_string(),
        5 => "1.25".to_string(),
        6 => {
            let n = cursor.next_u8() as i16;
            n.to_string()
        }
        _ => {
            let whole = cursor.next_u8();
            let frac = cursor.next_u8() % 100;
            format!("{whole}.{frac:02}")
        }
    }
}

fn escape_lemma_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('"');
    for ch in raw.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            // Keep the literal on one line and inside the string: drop controls.
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn text_literal(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 5 {
        0 => "\"\"".to_string(),
        1 => "\"a\"".to_string(),
        2 => "\"hello\"".to_string(),
        3 => "\"é\"".to_string(),
        _ => {
            let len = (cursor.next_u8() as usize) % 17;
            let raw = String::from_utf8_lossy(cursor.take(len));
            escape_lemma_text(&raw)
        }
    }
}

fn measure_literal(cursor: &mut Cursor<'_>) -> String {
    let magnitude = number_literal(cursor);
    let unit = match cursor.next_u8() % 6 {
        0 => "kilogram",
        1 => "gram",
        2 => "eur",
        3 => "meter",
        4 => "second",
        _ => "day",
    };
    format!("{magnitude} {unit}")
}

fn ratio_literal(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 4 {
        0 => "50%".to_string(),
        1 => "0%".to_string(),
        2 => "12.5%".to_string(),
        _ => {
            let n = cursor.next_u8() % 101;
            format!("{n}%")
        }
    }
}

fn date_literal(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 5 {
        0 => "2024-01-01".to_string(),
        1 => "2024-06-15T12:00:00Z".to_string(),
        2 => "2024-W01".to_string(),
        3 => "2024".to_string(),
        _ => {
            let year = 2000 + (cursor.next_u8() as u16 % 50);
            let month = (cursor.next_u8() % 12) + 1;
            let day = (cursor.next_u8() % 28) + 1;
            format!("{year:04}-{month:02}-{day:02}")
        }
    }
}

fn time_literal(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 4 {
        0 => "00:00:00".to_string(),
        1 => "12:30:45".to_string(),
        2 => "23:59:59".to_string(),
        _ => {
            let hour = cursor.next_u8() % 24;
            let minute = cursor.next_u8() % 60;
            let second = cursor.next_u8() % 60;
            format!("{hour:02}:{minute:02}:{second:02}")
        }
    }
}

fn boolean_literal(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 4 {
        0 => "true".to_string(),
        1 => "false".to_string(),
        2 => "yes".to_string(),
        _ => "no".to_string(),
    }
}

fn range_literal(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 4 {
        0 => "1...10".to_string(),
        1 => "0...100".to_string(),
        2 => format!("{}...{}", number_literal(cursor), number_literal(cursor)),
        _ => format!(
            "{}...{}",
            measure_literal(cursor),
            measure_literal(cursor)
        ),
    }
}

fn build_literal(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 8 {
        0 => number_literal(cursor),
        1 => text_literal(cursor),
        2 => measure_literal(cursor),
        3 => ratio_literal(cursor),
        4 => date_literal(cursor),
        5 => time_literal(cursor),
        6 => boolean_literal(cursor),
        _ => range_literal(cursor),
    }
}

fuzz_target!(|data: &[u8]| {
    let mut cursor = Cursor::new(data);
    let literal = build_literal(&mut cursor);
    // Keep the value on one line so newlines in the generator cannot escape the
    // data declaration into a second full-document parser.
    let code = format!("spec fuzz_test\nuses lemma units\ndata test_value: {literal}\n");

    let mut engine = Engine::new();
    let loaded = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "fuzz_literal",
        ))),
        code,
    )]);

    // Property: load Ok => evaluation must not panic; serialize must succeed;
    // two evaluates with the same effective datetime must agree.
    if loaded.is_ok() {
        let now = fixed_now();
        let first = engine.evaluate(
            None,
            "fuzz_test",
            Some(&now),
            IndexMap::new(),
            None,
            false,
        );
        let second = engine.evaluate(
            None,
            "fuzz_test",
            Some(&now),
            IndexMap::new(),
            None,
            false,
        );
        match (first, second) {
            (Ok(a), Ok(b)) => {
                let json_a = serde_json::to_string(&lemma::api::Response::from(&a))
                    .expect("BUG: Response must serialize");
                let json_b = serde_json::to_string(&lemma::api::Response::from(&b))
                    .expect("BUG: Response must serialize");
                assert_eq!(
                    json_a, json_b,
                    "BUG: evaluate must be deterministic for fixed effective datetime"
                );
                std::hint::black_box(json_a);
            }
            (Err(error), Err(_)) => {
                std::hint::black_box(error);
            }
            (Ok(_), Err(_)) | (Err(_), Ok(_)) => {
                panic!("BUG: evaluate Ok/Err disagree across identical deterministic runs");
            }
        }
    }
});
