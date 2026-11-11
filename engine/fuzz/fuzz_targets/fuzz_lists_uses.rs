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
}

fn qty_cmp(cursor: &mut Cursor<'_>) -> &'static str {
    match cursor.next_u8() % 4 {
        0 => ">",
        1 => "<",
        2 => "is",
        _ => ">=",
    }
}

fn qty_threshold(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 4 {
        0 => "0".to_string(),
        1 => "1".to_string(),
        2 => "2".to_string(),
        _ => (cursor.next_u8() % 5).to_string(),
    }
}

fn list_pipeline(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 10 {
        0 => "items -> count".to_string(),
        1 => "items -> sum qty".to_string(),
        2 => "items -> min qty".to_string(),
        3 => "items -> max qty".to_string(),
        4 => "items -> avg qty".to_string(),
        5 => {
            let op = qty_cmp(cursor);
            let n = qty_threshold(cursor);
            format!("items -> where qty {op} {n} -> count")
        }
        6 => "(items -> sort qty asc -> first).qty".to_string(),
        7 => "(items -> sort qty desc -> first).qty".to_string(),
        8 => "items -> find wanted".to_string(),
        _ => {
            let op = qty_cmp(cursor);
            let n = qty_threshold(cursor);
            format!("items -> where qty {op} {n} -> sum qty")
        }
    }
}

fn cross_spec_rule(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 4 {
        0 => "base.qty * 2".to_string(),
        1 => "base.qty + extra".to_string(),
        2 => {
            let pipeline = list_pipeline(cursor);
            format!("{pipeline} + base.qty")
        }
        _ => {
            format!(
                "base.qty\n  unless flag then base.qty + extra\n  unless items -> count > 0 then {}",
                list_pipeline(cursor)
            )
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let mut cursor = Cursor::new(data);
    let pipeline = list_pipeline(&mut cursor);
    let cross = cross_spec_rule(&mut cursor);

    let code = format!(
        r#"
spec item
data name: text
data qty: number

spec base
data qty: number
  -> suggest 1

spec fuzz_test
uses item
uses base
data flag: true
data extra: 3
data wanted: item.name
  -> with "a"
data items: item list
  -> key name
  -> with name: "a", qty: 1
  -> with name: "b", qty: 2
  -> with name: "c", qty: 0
rule listed: {pipeline}
rule composed: {cross}
"#
    );

    let mut engine = Engine::new();
    let loaded = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
            "fuzz_lists_uses",
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
