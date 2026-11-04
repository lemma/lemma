//! Document `x-lemma-row-field-needed-by-rules` = parent Show.rules that need each
//! `$defs` row field. `$defs` properties keep empty `x-lemma-needed-by-rules` so
//! `LemmaRow_*` schemas are identical across Show documents.

use lemma::api::show_document;
use lemma::{DateTimeValue, Engine, SourceType};
use serde_json::Value as JsonValue;

fn row_needed(document: &JsonValue, def: &str, field: &str) -> Vec<String> {
    match document
        .get("x-lemma-row-field-needed-by-rules")
        .and_then(|m| m.get(def))
        .and_then(|m| m.get(field))
    {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .unwrap_or_else(|| panic!("needed_by_rules for {def}.{field} must be an array"))
            .iter()
            .map(|v| {
                v.as_str()
                    .unwrap_or_else(|| {
                        panic!("needed_by_rules entry not a string for {def}.{field}")
                    })
                    .to_string()
            })
            .collect(),
    }
}

fn assert_defs_field_needed_empty(document: &JsonValue, def: &str, field: &str) {
    let needed = document["$defs"][def]["properties"][field]["x-lemma-needed-by-rules"]
        .as_array()
        .unwrap_or_else(|| panic!("$defs {def}.{field} missing x-lemma-needed-by-rules"));
    assert!(
        needed.is_empty(),
        "$defs {def}.{field} must keep empty x-lemma-needed-by-rules, got {needed:?}"
    );
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
uses xx
data zz: xx list
rule x: zz -> sum z
"#,
        "consumer",
    );
    let def = "LemmaRow_xx";
    assert_eq!(row_needed(&document, def, "z"), vec!["x".to_string()]);
    assert_eq!(row_needed(&document, def, "type"), Vec::<String>::new());
    assert_defs_field_needed_empty(&document, def, "z");
    assert_defs_field_needed_empty(&document, def, "type");
}

#[test]
fn row_field_where_marks_predicate_and_reduce_fields() {
    let document = show_doc(
        r#"
spec xx
data z: number
data type: text

spec consumer
uses xx
data zz: xx list
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
uses xx
data zz: xx list
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
uses menu_item
data menu: menu_item list
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
uses line
data lines: line list
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

#[test]
fn shared_row_defs_are_identical_across_parent_shows() {
    let code = r#"
spec article
data price: number

spec line
uses article: article
data qty: number
rule cost: qty * article.price

spec order
uses line
data lines: line list
rule total: lines -> sum cost

spec cart
uses line
data lines: line list
rule subtotal: lines -> sum qty
"#;
    let order = show_doc(code, "order");
    let cart = show_doc(code, "cart");
    assert_eq!(
        order["$defs"]["LemmaRow_line"], cart["$defs"]["LemmaRow_line"],
        "LemmaRow_line must be identical across Show documents"
    );
    assert_eq!(
        row_needed(&order, "LemmaRow_line", "qty"),
        vec!["total".to_string()]
    );
    assert_eq!(
        row_needed(&cart, "LemmaRow_line", "qty"),
        vec!["subtotal".to_string()]
    );
    assert_eq!(
        row_needed(&order, "LemmaRow_line", "article.price"),
        vec!["total".to_string()]
    );
    assert_eq!(
        row_needed(&cart, "LemmaRow_line", "article.price"),
        Vec::<String>::new()
    );
}
