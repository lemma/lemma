#![no_main]

use lemma::DateTimeValue;
use lemma::Engine;
use lemma::EvaluateDataValue;
use lemma::IndexMap;
use libfuzzer_sys::fuzz_target;

const SPEC: &str = r#"
spec item
data name: text
data price: measure
  -> unit eur: 1
data qty: number

spec fuzz_test
uses item
data n: number
data t: text
  -> option "a"
  -> option "é"
data b: boolean
data d: 2024-01-01
data tm: time
data mass: measure
  -> unit kilogram: 1
  -> unit gram: 0.001
data weight: mass
data pct: ratio
data window: 1...10
data items: item list
  -> key name
  -> with name: "a", price: 1 eur, qty: 1
  -> with name: "b", price: 2 eur, qty: 2
data wanted: item.name
  -> with "a"
rule rn: n
rule rt: t
rule rb: b
rule rd: d
rule rtm: tm
rule rw: weight
rule rpct: pct
rule rwin: window
rule rn2: n * 2
rule rsum: weight + 1 kilogram
rule sel: items -> find wanted
rule total: items -> sum items.price
"#;

const KEYS: &[&str] = &[
    "n",
    "t",
    "b",
    "d",
    "tm",
    "weight",
    "pct",
    "window",
    "mass",
    "items",
    "wanted",
    "nosuch",
    "N",
    "Weight",
    "WEIGHT",
    "n.x",
    "weight.kilogram",
    "items.a.price",
    "items.b.qty",
    "items.1.price",
    "items.2.qty",
    "items.0.price",
    "items.99.price",
    "Items.A.Price",
    "items.a",
    "items.name",
    "foo.bar.baz",
    "",
    "é",
];

const RULE_SETS: &[&[&str]] = &[
    &[],
    &["rn"],
    &["nosuch"],
    &["rn", "rn"],
    &[""],
    &["rn", "nosuch"],
    &["n"],
    &["RN"],
    &["total", "sel"],
];

const MAX_ENTRIES: usize = 8;
const MAX_DEPTH: usize = 4;
const MAX_MAP_WIDTH: usize = 4;
const MAX_LIST_LEN: usize = 8;
const MAX_LITERAL_BYTES: usize = 64;

struct Cursor<'a> {
    bytes: &'a [u8],
    index: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, index: 0 }
    }

    fn next_u8(&mut self) -> Option<u8> {
        let byte = *self.bytes.get(self.index)?;
        self.index += 1;
        Some(byte)
    }

    fn take(&mut self, len: usize) -> &'a [u8] {
        let start = self.index;
        let end = (start + len).min(self.bytes.len());
        self.index = end;
        &self.bytes[start..end]
    }
}

fn decode_key(cursor: &mut Cursor<'_>) -> String {
    let tag = cursor.next_u8().unwrap_or(0);
    if tag == 255 {
        let len = (cursor.next_u8().unwrap_or(0) as usize) % (MAX_LITERAL_BYTES + 1);
        String::from_utf8_lossy(cursor.take(len)).into_owned()
    } else {
        KEYS[tag as usize % KEYS.len()].to_string()
    }
}

fn decode_value(cursor: &mut Cursor<'_>, depth: usize) -> EvaluateDataValue {
    if depth >= MAX_DEPTH {
        return decode_literal(cursor);
    }
    match cursor.next_u8().unwrap_or(0) % 4 {
        0 => decode_literal(cursor),
        1 => EvaluateDataValue::Boolean(cursor.next_u8().unwrap_or(0) % 2 == 0),
        2 => {
            let width = (cursor.next_u8().unwrap_or(0) as usize) % (MAX_MAP_WIDTH + 1);
            let mut entries = Vec::with_capacity(width);
            for i in 0..width {
                let key = match cursor.next_u8().unwrap_or(0) % 4 {
                    0 => "kilogram".to_string(),
                    1 => "gram".to_string(),
                    2 => "eur".to_string(),
                    _ => format!("k{i}"),
                };
                entries.push((key, decode_value(cursor, depth + 1)));
            }
            match lemma::evaluate_data_map_from_entries(entries) {
                Ok(value) => value,
                Err(_) => EvaluateDataValue::Map(IndexMap::new()),
            }
        }
        _ => {
            let len = (cursor.next_u8().unwrap_or(0) as usize) % (MAX_LIST_LEN + 1);
            let mut items = Vec::with_capacity(len);
            for _ in 0..len {
                items.push(decode_value(cursor, depth + 1));
            }
            EvaluateDataValue::List(items)
        }
    }
}

fn decode_literal(cursor: &mut Cursor<'_>) -> EvaluateDataValue {
    let len = (cursor.next_u8().unwrap_or(0) as usize) % (MAX_LITERAL_BYTES + 1);
    let text = String::from_utf8_lossy(cursor.take(len)).into_owned();
    EvaluateDataValue::Literal(text)
}

fn decode_rules(cursor: &mut Cursor<'_>) -> Option<Vec<String>> {
    let set = RULE_SETS[cursor.next_u8().unwrap_or(0) as usize % RULE_SETS.len()];
    if set.is_empty() {
        None
    } else {
        Some(set.iter().map(|name| (*name).to_string()).collect())
    }
}

fuzz_target!(|data: &[u8]| {
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                "fuzz_binding",
            ))),
            SPEC.to_string(),
        )])
        .expect("BUG: static fuzz spec must load");

    let mut cursor = Cursor::new(data);
    let entry_count = (cursor.next_u8().unwrap_or(0) as usize) % (MAX_ENTRIES + 1);
    let mut entries = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        let key = decode_key(&mut cursor);
        let value = decode_value(&mut cursor, 0);
        entries.push((key, value));
    }

    let bindings = match lemma::evaluate_data_from_entries(entries) {
        Ok(map) => map,
        Err(error) => {
            std::hint::black_box(error);
            return;
        }
    };

    let rules = decode_rules(&mut cursor);
    let now = DateTimeValue::now();
    if let Err(error) = engine.evaluate(
        None,
        "fuzz_test",
        Some(&now),
        bindings,
        rules.as_deref(),
        false,
    ) {
        std::hint::black_box(error);
    }
});
