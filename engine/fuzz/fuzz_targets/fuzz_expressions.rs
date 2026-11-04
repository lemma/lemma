#![no_main]

use lemma::DateTimeValue;
use lemma::Engine;
use lemma::IndexMap;
use libfuzzer_sys::fuzz_target;

const MAX_DEPTH: usize = 4;

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
}

fn number_leaf(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 6 {
        0 => "0".to_string(),
        1 => "1".to_string(),
        2 => "2".to_string(),
        3 => "-1".to_string(),
        4 => "x".to_string(),
        _ => "y".to_string(),
    }
}

fn measure_leaf(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 4 {
        0 => "1 kilogram".to_string(),
        1 => "2 gram".to_string(),
        2 => "m".to_string(),
        _ => "0 kilogram".to_string(),
    }
}

fn boolean_leaf(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 4 {
        0 => "true".to_string(),
        1 => "false".to_string(),
        2 => "flag".to_string(),
        _ => "x > 0".to_string(),
    }
}

fn text_leaf(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 3 {
        0 => "\"a\"".to_string(),
        1 => "\"b\"".to_string(),
        _ => "label".to_string(),
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Number,
    Measure,
    Boolean,
    Text,
}

fn pick_kind(cursor: &mut Cursor<'_>) -> Kind {
    match cursor.next_u8() % 4 {
        0 => Kind::Number,
        1 => Kind::Measure,
        2 => Kind::Boolean,
        _ => Kind::Text,
    }
}

fn leaf(kind: Kind, cursor: &mut Cursor<'_>) -> String {
    match kind {
        Kind::Number => number_leaf(cursor),
        Kind::Measure => measure_leaf(cursor),
        Kind::Boolean => boolean_leaf(cursor),
        Kind::Text => text_leaf(cursor),
    }
}

fn rounding_mode(cursor: &mut Cursor<'_>) -> &'static str {
    match cursor.next_u8() % 3 {
        0 => "round",
        1 => "floor",
        _ => "ceil",
    }
}

fn build_expr(kind: Kind, depth: usize, cursor: &mut Cursor<'_>) -> String {
    if depth == 0 {
        return leaf(kind, cursor);
    }

    match kind {
        Kind::Number => match cursor.next_u8() % 11 {
            0 => format!(
                "({} + {})",
                build_expr(Kind::Number, depth - 1, cursor),
                build_expr(Kind::Number, depth - 1, cursor)
            ),
            1 => format!(
                "({} - {})",
                build_expr(Kind::Number, depth - 1, cursor),
                build_expr(Kind::Number, depth - 1, cursor)
            ),
            2 => format!(
                "({} * {})",
                build_expr(Kind::Number, depth - 1, cursor),
                build_expr(Kind::Number, depth - 1, cursor)
            ),
            3 => format!(
                "({} / {})",
                build_expr(Kind::Number, depth - 1, cursor),
                build_expr(Kind::Number, depth - 1, cursor)
            ),
            4 => format!(
                "({} mod {})",
                build_expr(Kind::Number, depth - 1, cursor),
                build_expr(Kind::Number, depth - 1, cursor)
            ),
            5 => format!(
                "({} ^ {})",
                build_expr(Kind::Number, depth - 1, cursor),
                number_leaf(cursor)
            ),
            6 => format!("abs({})", build_expr(Kind::Number, depth - 1, cursor)),
            7 => format!(
                "({} -> floor)",
                build_expr(Kind::Number, depth - 1, cursor)
            ),
            8 => format!(
                "({} -> ceil)",
                build_expr(Kind::Number, depth - 1, cursor)
            ),
            9 => format!(
                "({} -> round)",
                build_expr(Kind::Number, depth - 1, cursor)
            ),
            _ => {
                let mode = rounding_mode(cursor);
                format!(
                    "({} -> {mode}(0.25))",
                    build_expr(Kind::Number, depth - 1, cursor)
                )
            }
        },
        Kind::Measure => match cursor.next_u8() % 6 {
            0 => format!(
                "({} + {})",
                build_expr(Kind::Measure, depth - 1, cursor),
                build_expr(Kind::Measure, depth - 1, cursor)
            ),
            1 => format!(
                "({} - {})",
                build_expr(Kind::Measure, depth - 1, cursor),
                build_expr(Kind::Measure, depth - 1, cursor)
            ),
            2 => format!(
                "({} * {})",
                build_expr(Kind::Measure, depth - 1, cursor),
                build_expr(Kind::Number, depth - 1, cursor)
            ),
            3 => format!(
                "({} / {})",
                build_expr(Kind::Measure, depth - 1, cursor),
                build_expr(Kind::Number, depth - 1, cursor)
            ),
            4 => format!("abs({})", build_expr(Kind::Measure, depth - 1, cursor)),
            _ => {
                let mode = rounding_mode(cursor);
                let step = if cursor.next_u8() % 2 == 0 {
                    "1 kilogram"
                } else {
                    "5 gram"
                };
                format!(
                    "({} -> {mode}({step}))",
                    build_expr(Kind::Measure, depth - 1, cursor)
                )
            }
        },
        Kind::Boolean => match cursor.next_u8() % 8 {
            0 => format!(
                "({} and {})",
                build_expr(Kind::Boolean, depth - 1, cursor),
                build_expr(Kind::Boolean, depth - 1, cursor)
            ),
            1 => format!(
                "({} or {})",
                build_expr(Kind::Boolean, depth - 1, cursor),
                build_expr(Kind::Boolean, depth - 1, cursor)
            ),
            2 => format!("(not {})", build_expr(Kind::Boolean, depth - 1, cursor)),
            3 => format!(
                "({} > {})",
                build_expr(Kind::Number, depth - 1, cursor),
                build_expr(Kind::Number, depth - 1, cursor)
            ),
            4 => format!(
                "({} < {})",
                build_expr(Kind::Number, depth - 1, cursor),
                build_expr(Kind::Number, depth - 1, cursor)
            ),
            5 => format!(
                "({} is {})",
                build_expr(Kind::Text, depth - 1, cursor),
                build_expr(Kind::Text, depth - 1, cursor)
            ),
            6 => format!(
                "({} is {})",
                build_expr(Kind::Number, depth - 1, cursor),
                build_expr(Kind::Number, depth - 1, cursor)
            ),
            _ => format!(
                "({} in 1...10)",
                build_expr(Kind::Number, depth - 1, cursor)
            ),
        },
        Kind::Text => leaf(Kind::Text, cursor),
    }
}

fuzz_target!(|data: &[u8]| {
    let mut cursor = Cursor::new(data);
    let depth = (cursor.next_u8() as usize % MAX_DEPTH) + 1;
    let kind = pick_kind(&mut cursor);
    let expr = build_expr(kind, depth, &mut cursor);

    let code = format!(
        r#"
spec fuzz_test
uses lemma units
data x: 100
data y: 50
data flag: true
data label: "a"
data m: measure
  -> unit kilogram: 1
  -> unit gram: 0.001
  -> suggest 1 kilogram
rule test_expr: {expr}
"#
    );

    let mut engine = Engine::new();
    let loaded = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("fuzz_expr"))),
        code,
    )]);

    // Property: load Ok => evaluation must not panic.
    if loaded.is_ok() {
        let now = DateTimeValue::now();
        if let Err(error) = engine.evaluate(
            None,
            "fuzz_test",
            Some(&now),
            IndexMap::new(),
            None,
            false,
        ) {
            std::hint::black_box(error);
        }
    }
});
