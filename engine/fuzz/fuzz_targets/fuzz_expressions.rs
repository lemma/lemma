#![no_main]

use lemma::DateTimeValue;
use lemma::Engine;
use lemma::IndexMap;
use libfuzzer_sys::fuzz_target;
use std::str::FromStr;

const MAX_DEPTH: usize = 4;

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

fn number_range(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 4 {
        0 => "1...10".to_string(),
        1 => "0...100".to_string(),
        2 => "-5...5".to_string(),
        _ => format!("{}...{}", number_leaf(cursor), number_leaf(cursor)),
    }
}

fn deep_nested_number(cursor: &mut Cursor<'_>) -> String {
    // Depth 1..=9 crosses the default max_expression_depth of 7.
    let depth = (cursor.next_u8() as usize % 9) + 1;
    let variant = cursor.next_u8() % 4;
    let mut expr = number_leaf(cursor);
    for i in 0..depth {
        expr = match variant {
            0 => format!("({expr} + 1)"),
            1 => format!("({expr} * 2)"),
            2 => format!("({expr} - {i})"),
            _ => format!("({expr})"),
        };
    }
    expr
}

fn unless_number(cursor: &mut Cursor<'_>, depth: usize) -> String {
    let default_body = build_expr(Kind::Number, depth.saturating_sub(1), cursor);
    let cond = build_expr(Kind::Boolean, depth.saturating_sub(1), cursor);
    let arm = build_expr(Kind::Number, depth.saturating_sub(1), cursor);
    if cursor.next_u8() % 2 == 0 {
        format!("{default_body}\n  unless {cond} then {arm}")
    } else {
        let cond2 = build_expr(Kind::Boolean, depth.saturating_sub(1), cursor);
        let arm2 = build_expr(Kind::Number, depth.saturating_sub(1), cursor);
        format!("{default_body}\n  unless {cond} then {arm}\n  unless {cond2} then {arm2}")
    }
}

fn list_number(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 6 {
        0 => "items -> count".to_string(),
        1 => "items -> sum qty".to_string(),
        2 => "items -> min qty".to_string(),
        3 => "items -> max qty".to_string(),
        4 => "items -> where qty > 0 -> count".to_string(),
        _ => "(items -> sort qty desc -> first).qty".to_string(),
    }
}

fn build_expr(kind: Kind, depth: usize, cursor: &mut Cursor<'_>) -> String {
    if depth == 0 {
        return leaf(kind, cursor);
    }

    match kind {
        Kind::Number => match cursor.next_u8() % 14 {
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
            10 => {
                let mode = rounding_mode(cursor);
                format!(
                    "({} -> {mode}(0.25))",
                    build_expr(Kind::Number, depth - 1, cursor)
                )
            }
            11 => unless_number(cursor, depth),
            12 => list_number(cursor),
            _ => deep_nested_number(cursor),
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
        Kind::Boolean => match cursor.next_u8() % 10 {
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
            7 => format!(
                "({} in {})",
                build_expr(Kind::Number, depth - 1, cursor),
                number_range(cursor)
            ),
            8 => format!("(items -> count > {})", number_leaf(cursor)),
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
spec item
data name: text
data qty: number

spec fuzz_test
uses lemma units
uses item
data x: 100
data y: 50
data flag: true
data label: "a"
data m: measure
  -> unit kilogram: 1
  -> unit gram: 0.001
  -> suggest 1 kilogram
data items: item list
  -> key name
  -> with name: "a", qty: 1
  -> with name: "b", qty: 2
rule test_expr: {expr}
"#
    );

    let mut engine = Engine::new();
    let loaded = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("fuzz_expr"))),
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
