#![no_main]

use libfuzzer_sys::fuzz_target;

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

fn value_pipeline(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 12 {
        0 => "fares -> sum".to_string(),
        1 => "fares -> product".to_string(),
        2 => "fares -> avg".to_string(),
        3 => "fares -> min".to_string(),
        4 => "fares -> max".to_string(),
        5 => "fares -> where >= 1 -> count".to_string(),
        6 => "fares -> where in 0...10 -> sum".to_string(),
        7 => "fares -> any > 0".to_string(),
        8 => "fares -> all >= 0".to_string(),
        9 => "fares -> where >= 1 -> sort desc -> top 1 -> first".to_string(),
        10 => "fares -> last".to_string(),
        _ => "empty -> count".to_string(),
    }
}

fn empty_pipeline(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 6 {
        0 => "empty -> count".to_string(),
        1 => "empty -> sum".to_string(),
        2 => "empty -> avg".to_string(),
        3 => "empty -> product".to_string(),
        4 => "empty -> first".to_string(),
        _ => "empty -> last".to_string(),
    }
}

fn list_pipeline(cursor: &mut Cursor<'_>) -> String {
    match cursor.next_u8() % 14 {
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
        9 => "items -> last".to_string(),
        10 => "items -> product qty".to_string(),
        11 => "items -> any(qty > 0)".to_string(),
        12 => "items -> all(qty >= 0)".to_string(),
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
    let valued = value_pipeline(&mut cursor);
    let emptied = empty_pipeline(&mut cursor);
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
data fares: number list
  -> with 1, 2, 10
data empty: number list
rule listed: {pipeline}
rule valued: {valued}
rule emptied: {emptied}
rule composed: {cross}
"#
    );

    lemma_fuzz::exercise_source("fuzz_lists_uses", &code);
});
