//! Row `$defs` `x-lemma-needed-by-rules` = parent Show.rules that need each field.

use lemma::api::show_document;
use lemma::{DateTimeValue, Engine, SourceType};
use serde_json::Value as JsonValue;

fn row_needed(document: &JsonValue, def: &str, field: &str) -> Vec<String> {
    document["$defs"][def]["properties"][field]["x-lemma-needed-by-rules"]
        .as_array()
        .unwrap_or_else(|| panic!("missing needed_by_rules for {def}.{field}"))
        .iter()
        .map(|v| {
            v.as_str()
                .unwrap_or_else(|| panic!("needed_by_rules entry not a string for {def}.{field}"))
                .to_string()
        })
        .collect()
}

fn show_doc(code: &str, spec: &str) -> JsonValue {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, code.to_string())])
        .expect("load");
    let show = engine
        .show(None, spec, Some(&DateTimeValue::now()))
        .expect("show");
    show_document(&engine, &show)
}

#[test]
fn row_field_sum_marks_only_used_field() {
    let document = show_doc(
        r#"
spec xx
data z: number
data type: text

spec consumer
uses zz: xx list
rule x: zz -> sum z
"#,
        "consumer",
    );
    let def = "LemmaRow_xx";
    assert_eq!(row_needed(&document, def, "z"), vec!["x".to_string()]);
    assert_eq!(row_needed(&document, def, "type"), Vec::<String>::new());
}

#[test]
fn row_field_where_marks_predicate_and_reduce_fields() {
    let document = show_doc(
        r#"
spec xx
data z: number
data type: text

spec consumer
uses zz: xx list
rule y: zz -> where type is "a" -> sum z
"#,
        "consumer",
    );
    let def = "LemmaRow_xx";
    assert_eq!(row_needed(&document, def, "type"), vec!["y".to_string()]);
    assert_eq!(row_needed(&document, def, "z"), vec!["y".to_string()]);
}

#[test]
fn row_field_multi_rule_unions_alphabetically() {
    let document = show_doc(
        r#"
spec xx
data z: number
data type: text

spec consumer
uses zz: xx list
rule a_sum: zz -> sum z
rule b_where: zz -> where type is "a" -> sum z
"#,
        "consumer",
    );
    let def = "LemmaRow_xx";
    assert_eq!(
        row_needed(&document, def, "z"),
        vec!["a_sum".to_string(), "b_where".to_string()]
    );
    assert_eq!(
        row_needed(&document, def, "type"),
        vec!["b_where".to_string()]
    );
}

#[test]
fn row_field_find_marks_key_and_spec_member_marks_price() {
    let document = show_doc(
        r#"
spec menu_item
data drink: text
data price: number
data size: number

spec coffee
uses menu: menu_item list
  -> key drink
data ordered: text
rule selected: menu -> find ordered
rule selected_price: (menu -> find ordered).price
"#,
        "coffee",
    );
    let def = "LemmaRow_menu__item";
    assert_eq!(
        row_needed(&document, def, "drink"),
        vec!["selected".to_string(), "selected_price".to_string()]
    );
    assert_eq!(
        row_needed(&document, def, "price"),
        vec!["selected_price".to_string()]
    );
    assert_eq!(row_needed(&document, def, "size"), Vec::<String>::new());
}

#[test]
fn row_field_sum_row_rule_expands_to_parent_consumers() {
    let document = show_doc(
        r#"
spec article
data price: number

spec line
uses article: article
data qty: number
rule cost: qty * article.price

spec order
uses lines: line list
rule total: lines -> sum cost
"#,
        "order",
    );
    let def = "LemmaRow_line";
    assert_eq!(row_needed(&document, def, "qty"), vec!["total".to_string()]);
    assert_eq!(
        row_needed(&document, def, "article.price"),
        vec!["total".to_string()]
    );
}
