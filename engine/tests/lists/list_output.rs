//! Output paths, list/row result arms, Show list type and `value`.

use crate::lists::{
    assert_contains, boolean, data, evaluate, evaluate_explain, list, lit, load_ok, map,
    missing_data,
};
use lemma::format_explanation;
use lemma::result_value::RuleResultValue;
use lemma::{ExplanationNode, ListKey, TypeSpecification};

#[test]
fn missing_data_uses_row_numbers() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
data price: measure
  -> unit eur: 1.00
spec coffee_order
uses menu: menu_item list
  -> key drink
  -> with drink: "espresso"
  -> with drink: "latte"
rule cheapest:
  menu -> min price
"#,
    );
    let paths = missing_data(&evaluate(&engine, "coffee_order", data(&[])), "cheapest");
    assert!(paths.iter().any(|path| path == "menu.1.price"), "{paths:?}");
    assert!(paths.iter().any(|path| path == "menu.2.price"), "{paths:?}");
}

#[test]
fn explanations_name_rows_by_number_and_show_keys() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
data price: measure
  -> unit eur: 1.00
spec coffee_order
uses menu: menu_item list
  -> key drink
  -> with drink: "espresso", price: 2.50 eur
  -> with drink: "latte", price: 3.50 eur
rule cheapest:
  menu -> min price
"#,
    );
    let response = evaluate_explain(&engine, "coffee_order", data(&[]));
    let explanation = response
        .results
        .get("cheapest")
        .expect("cheapest")
        .explanation
        .as_ref()
        .expect("explanation");
    let text = format_explanation(explanation);
    assert_contains(&text, "[\"latte\"] price: 3.5 eur");
    assert_contains(&text, "[\"espresso\"] price: 2.5 eur");
    let json = serde_json::to_value(explanation).expect("json");
    let row = json["children"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(
            json["children"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|n| n.get("operands"))
                .filter_map(|o| o.as_array())
                .flatten(),
        )
        .find(|n| n["type"] == "row")
        .expect("row node");
    assert!(row["number"].as_u64().is_some(), "{row}");
    assert!(!row["key"].as_array().unwrap().is_empty(), "{row}");
}

#[test]
fn explanation_writes_sort_as_source_and_rows_once() {
    let engine = load_ok(
        r#"
spec employee
data name: text
data salary: measure
  -> unit eur: 1.00
spec payroll
uses staff: employee list
  -> with name: "Ada", salary: 9000 eur
  -> with name: "Grace", salary: 9500 eur
rule top_earner:
  staff -> sort salary desc -> first
"#,
    );
    let response = evaluate_explain(&engine, "payroll", data(&[]));
    let explanation = response
        .results
        .get("top_earner")
        .expect("top_earner")
        .explanation
        .as_ref()
        .expect("explanation");
    let text = format_explanation(explanation);
    assert_contains(&text, "staff -> sort salary desc -> first");
    assert!(!text.contains("desc("), "{text}");
    assert_eq!(text.matches("1. salary:").count(), 1, "{text}");
    assert_eq!(text.matches("2. salary:").count(), 1, "{text}");
}

#[test]
fn explanation_keeps_row_reads_of_per_row_arguments() {
    let engine = load_ok(
        r#"
spec employee
data salary: measure
  -> unit eur: 1.00
rule bonus:
  salary * 10%
spec payroll
uses staff: employee list
  -> with salary: 9000 eur
  -> with salary: 9500 eur
rule high_bonus:
  staff -> where salary > 9200 eur -> sum bonus
"#,
    );
    let response = evaluate_explain(&engine, "payroll", data(&[]));
    let explanation = response
        .results
        .get("high_bonus")
        .expect("high_bonus")
        .explanation
        .as_ref()
        .expect("explanation");
    let text = format_explanation(explanation);
    assert_contains(&text, "1 of 2 are salary > 9200 eur");
    assert_contains(&text, "2. bonus:");
    assert!(!text.contains("1. bonus"), "{text}");
    assert!(
        !text.contains("1. salary:") && !text.contains("2. salary:"),
        "where predicate row reads must be hidden:\n{text}"
    );
}

#[test]
fn list_valued_rule_result_arm() {
    let engine = load_ok(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
  -> with 89 eur, 120 eur
rule high:
  fares -> where >= 100 eur
"#,
    );
    let response = evaluate(&engine, "fares_card", data(&[]));
    let result = response
        .results
        .get("high")
        .expect("high")
        .result
        .as_ref()
        .expect("list result");
    let rows = result.list.as_ref().expect("list arm");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].result.as_deref(), Some("120 eur"));
}

#[test]
fn row_valued_rule_result_arm() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
data price: measure
  -> unit eur: 1.00
spec coffee_order
uses menu: menu_item list
  -> key drink
  -> with drink: "espresso", price: 2.50 eur
data ordered: text
  -> with "espresso"
rule selected_drink:
  menu -> find ordered
"#,
    );
    let response = evaluate(&engine, "coffee_order", data(&[]));
    let result = response
        .results
        .get("selected_drink")
        .expect("selected_drink")
        .result
        .as_ref()
        .expect("row result");
    assert!(result.spec.is_none(), "a row is not a fixed instance");
    let reference = result.row.as_ref().expect("row arm");
    assert_eq!(reference.spec, "menu_item");
    assert_eq!(reference.instance, "menu.1");
    assert_eq!(response.rows.len(), 1);
    let row = &response.rows[reference.index];
    assert_eq!(
        row.data
            .get("drink")
            .and_then(|slot| slot.result.as_ref())
            .and_then(|value| value.text.as_deref()),
        Some("espresso")
    );
}

#[test]
fn list_of_rows_result_references_every_row() {
    let engine = load_ok(
        r#"
spec line
data qty: number
spec order
uses lines: line list
  -> with qty: 1
  -> with qty: 0
  -> with qty: 3
rule positive:
  lines -> where qty > 0
"#,
    );
    let response = evaluate(&engine, "order", data(&[]));
    let result = response
        .results
        .get("positive")
        .and_then(|rule| rule.result.as_ref())
        .expect("positive");
    let rows = result.list.as_ref().expect("list arm");
    let instances: Vec<&str> = rows
        .iter()
        .map(|row| row.row.as_ref().expect("row arm").instance.as_str())
        .collect();
    assert_eq!(instances, ["lines.1", "lines.3"]);
    let quantities: Vec<Option<&str>> = rows
        .iter()
        .map(|row| {
            response.rows[row.row.as_ref().expect("row arm").index]
                .data
                .get("qty")
                .and_then(|slot| slot.result.as_ref())
                .and_then(|value| value.result.as_deref())
        })
        .collect();
    assert_eq!(quantities, [Some("1"), Some("3")]);
}

#[test]
fn row_reached_by_several_rules_is_filled_once() {
    let engine = load_ok(
        r#"
spec part
uses components: part list
data own_cost: number
rule first_child:
  components -> first
rule last_child:
  components -> last
"#,
    );
    let response = evaluate(
        &engine,
        "part",
        data(&[
            ("own_cost", crate::lists::lit("1")),
            (
                "components",
                crate::lists::list(vec![crate::lists::map(&[
                    ("own_cost", crate::lists::lit("2")),
                    (
                        "components",
                        crate::lists::list(vec![crate::lists::map(&[(
                            "own_cost",
                            crate::lists::lit("3"),
                        )])]),
                    ),
                ])]),
            ),
        ]),
    );
    let index_of = |rule: &str| {
        response
            .results
            .get(rule)
            .and_then(|result| result.result.as_ref())
            .and_then(|value| value.row.as_ref())
            .unwrap_or_else(|| panic!("{rule} row arm"))
            .index
    };
    assert_eq!(index_of("first_child"), index_of("last_child"));
    assert_eq!(
        response.rows.len(),
        2,
        "the child and the grandchild, each once"
    );
    let child = &response.rows[index_of("first_child")];
    let grandchild_index = child
        .rules
        .get("first_child")
        .and_then(|rule| rule.result.as_ref())
        .and_then(|value| value.row.as_ref())
        .expect("grandchild row arm")
        .index;
    assert_eq!(
        child
            .rules
            .get("last_child")
            .and_then(|rule| rule.result.as_ref())
            .and_then(|value| value.row.as_ref())
            .expect("grandchild row arm")
            .index,
        grandchild_index
    );
}

#[test]
fn value_list_result_rebuilds_literal() {
    let engine = load_ok(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
  -> unit cent: 100
data fares: money list
  -> with 89 eur, 12000 cent
rule all_fares:
  fares
"#,
    );
    let response = evaluate(&engine, "fares_card", data(&[]));
    let rule = response.results.get("all_fares").expect("all_fares");
    let literal = rule.to_literal();
    let rows = match &literal.value {
        lemma::ValueKind::List(rows) => rows,
        other => panic!("expected list literal, got {other:?}"),
    };
    let shown: Vec<String> = rows
        .iter()
        .map(|row| row.to_literal().display_value_with_type(&row.lemma_type))
        .collect();
    assert_eq!(shown, ["89 eur", "12000 cent"]);
}

#[test]
fn show_exposes_list_type_and_value() {
    let engine = load_ok(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
  -> with 89 eur, 120 eur
rule total:
  fares -> sum
"#,
    );
    let show = engine
        .show(None, "fares_card", Some(&crate::lists::now()))
        .expect("show");
    let fares = show.data.get("fares").expect("fares");
    match &fares.lemma_type.specifications {
        TypeSpecification::List { key, .. } => {
            assert_eq!(*key, ListKey::Numbered);
        }
        other => panic!("expected List type, got {other:?}"),
    }
    let committed = fares.value.as_ref().expect("value");
    let rows: &Vec<RuleResultValue> = committed.list.as_ref().expect("list value");
    assert_eq!(rows.len(), 2);
}

fn show_list_value(engine: &lemma::Engine, spec: &str, list: &str) -> Option<RuleResultValue> {
    engine
        .show(None, spec, Some(&crate::lists::now()))
        .expect("show")
        .data
        .get(list)
        .unwrap_or_else(|| panic!("{list} missing from Show.data"))
        .value
        .clone()
}

fn field_results(row: &RuleResultValue) -> Vec<(String, String)> {
    row.fields
        .as_ref()
        .expect("row fields")
        .iter()
        .map(|(name, value)| (name.clone(), value.result.clone().expect("field result")))
        .collect()
}

#[test]
fn show_instance_list_value_lists_source_row_fields() {
    let engine = load_ok(
        r#"
spec menu_item
data drink: text
  -> options "espresso", "latte"
data price: measure
  -> unit eur: 1.00
  -> decimals 2
spec coffee_order
uses menu: menu_item list
  -> key drink
  -> with drink: "espresso", price: 2.50 eur
  -> with drink: "latte"
rule cheapest:
  menu -> min price
"#,
    );
    let value = show_list_value(&engine, "coffee_order", "menu").expect("menu value");
    assert_eq!(value.result, None);
    let rows = value.list.as_ref().expect("list");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.result.is_none()), "{rows:?}");
    assert_eq!(
        field_results(&rows[0]),
        [
            ("drink".to_string(), "espresso".to_string()),
            ("price".to_string(), "2.50 eur".to_string()),
        ]
    );
    assert_eq!(
        field_results(&rows[1]),
        [("drink".to_string(), "latte".to_string())]
    );
    assert!(rows[0].fields.as_ref().expect("fields")["price"]
        .measure
        .is_some());
    let text = engine
        .show(None, "coffee_order", Some(&crate::lists::now()))
        .expect("show")
        .to_string();
    assert_contains(
        &text,
        "value:\n      1: drink espresso, price 2.50 eur\n      2: drink latte",
    );
}

#[test]
fn show_instance_list_value_names_dotted_row_fields() {
    let engine = load_ok(
        r#"
spec article
data price: number
spec line
data quantity: number
uses article: article
rule total: quantity * article.price
spec order
uses lines: line list
  -> with quantity: 2, article.price: 5
rule all_totals: lines -> sum total
"#,
    );
    let value = show_list_value(&engine, "order", "lines").expect("lines value");
    let rows = value.list.as_ref().expect("list");
    assert_eq!(
        field_results(&rows[0]),
        [
            ("quantity".to_string(), "2".to_string()),
            ("article.price".to_string(), "5".to_string()),
        ]
    );
}

#[test]
fn show_instance_list_value_names_referenced_row_fields() {
    let engine = load_ok(
        r#"
spec bracket
data threshold: number
data rate: number
data income: number
rule applies: income > threshold
spec tax
data income: number
uses brackets: bracket list
  -> with threshold: 0, rate: 0.36, income: income
  -> with threshold: 38000, rate: 0.5, income: income
rule applicable: brackets -> where applies -> count
"#,
    );
    let value = show_list_value(&engine, "tax", "brackets").expect("brackets value");
    let rows = value.list.as_ref().expect("list");
    assert_eq!(rows.len(), 2);
    let first = rows[0].fields.as_ref().expect("fields");
    assert_eq!(first["threshold"].result.as_deref(), Some("0"));
    assert_eq!(first["rate"].result.as_deref(), Some("0.36"));
    assert_eq!(first["income"].reference.as_deref(), Some("income"));
    assert_eq!(
        rows[1].fields.as_ref().expect("fields")["income"]
            .reference
            .as_deref(),
        Some("income")
    );
}

#[test]
fn show_instance_list_value_names_named_instance_row_references() {
    let engine = load_ok(
        r#"
spec employee
data name: text
data salary: measure
  -> unit eur: 1.00
spec payroll
uses ada: employee
  -> with name: "Ada", salary: 9000 eur
uses grace: employee
  -> with name: "Grace", salary: 9500 eur
uses staff: employee list
  -> with ada, grace
rule total: staff -> sum salary
"#,
    );
    let value = show_list_value(&engine, "payroll", "staff").expect("staff value");
    let rows = value.list.as_ref().expect("list");
    assert_eq!(rows.len(), 2);
    let first = rows[0].fields.as_ref().expect("fields");
    assert_eq!(first["name"].reference.as_deref(), Some("ada.name"));
    assert_eq!(first["salary"].reference.as_deref(), Some("ada.salary"));
    let second = rows[1].fields.as_ref().expect("fields");
    assert_eq!(second["name"].reference.as_deref(), Some("grace.name"));
    assert_eq!(second["salary"].reference.as_deref(), Some("grace.salary"));
}

#[test]
fn show_instance_list_without_source_rows_has_no_value() {
    let engine = load_ok(
        r#"
spec employee
data salary: number
spec payroll
uses staff: employee list
rule total: staff -> sum salary
spec part
data weight: number
uses components: part list
rule total: weight + components -> sum total
"#,
    );
    assert_eq!(show_list_value(&engine, "payroll", "staff"), None);
    assert_eq!(show_list_value(&engine, "part", "components"), None);
}

#[test]
fn show_has_no_fill() {
    let engine = load_ok(
        r#"
spec sample
data n: number
  -> with 42
rule r: n
"#,
    );
    let show = engine
        .show(None, "sample", Some(&crate::lists::now()))
        .expect("show");
    let entry = show.data.get("n").expect("n");
    assert!(entry.value.is_some());
}

#[test]
fn pinda_shaped_row_rule_explanation() {
    let engine = load_ok(
        r#"
spec units
uses lu: lemma units
data money: measure
  -> unit eur: 1
  -> unit usd: 0.84
  -> decimals 2
data price_per_weight: measure
  -> unit eur_per_kg: eur/kilogram
  -> decimals 2

spec processing
uses units
uses lu: lemma units
data type: text
  -> options "roast", "chocolatize"
data weight: lu.mass
data price: units.price_per_weight
data enabled: boolean
rule cost:
  0 eur
  unless enabled then price * weight

spec line_item
uses lu: lemma units
uses units
uses processing list
  -> with weight: weight, price: processing_price, type: "roast", enabled: roast
  -> with weight: weight, price: processing_price, type: "chocolatize", enabled: chocolatize
data weight: lu.mass
data price: units.price_per_weight
data processing_price: units.price_per_weight
data roast: boolean
data chocolatize: boolean
rule cost:
  price * weight + processing -> where enabled -> sum cost

spec calc
uses units
uses items: line_item list
rule total_price:
  (items -> sum cost) as usd
"#,
    );
    let response = evaluate_explain(
        &engine,
        "calc",
        data(&[(
            "items",
            list(vec![map(&[
                ("price", lit("14 eur_per_kg")),
                ("weight", lit("50 kilogram")),
                ("roast", boolean(false)),
                ("chocolatize", boolean(true)),
                ("processing_price", lit("4 eur_per_kg")),
            ])]),
        )]),
    );
    let explanation = response
        .results
        .get("total_price")
        .expect("total_price")
        .explanation
        .as_ref()
        .expect("explanation");
    let text = format_explanation(explanation);
    assert_contains(&text, "1. cost: 900.00 eur");
    assert_contains(&text, "1 of 2 are enabled");
    assert_contains(&text, "2. cost: 200.00 eur");
    assert_contains(&text, "price: 14.00 eur_per_kg");
    assert_contains(&text, "weight: 50 kilogram");
    assert!(!text.contains("items: items.1"), "{text}");
    let json = serde_json::to_value(explanation).expect("json");
    assert!(
        serde_json::to_string(&json)
            .unwrap()
            .contains("\"type\":\"row\""),
        "JSON must carry row nodes: {json}"
    );
    assert!(
        serde_json::to_string(&json)
            .unwrap()
            .contains("\"type\":\"filter\""),
        "JSON must carry filter nodes: {json}"
    );
    assert!(
        serde_json::to_string(&json)
            .unwrap()
            .contains("\"type\":\"rule_ref\""),
        "JSON must carry rule_ref for row rules: {json}"
    );
    let row_ref = first_rule_ref(&explanation.children).expect("row rule_ref");
    let narrated = crate::explanation_support::resolve_ref(explanation, row_ref);
    assert_eq!(narrated.name.rule, "cost");
    assert!(
        !narrated.body.is_empty(),
        "rules table entry must carry body"
    );
    assert_eq!(
        crate::explanation_support::rule_by_id(explanation, narrated.id).body,
        narrated.body
    );
    assert!(
        crate::explanation_support::rule_ref_names(&explanation.children)
            .iter()
            .any(|name| name == "cost"),
        "root tree must reference cost via rule_ref"
    );
}

fn first_rule_ref(nodes: &[ExplanationNode]) -> Option<&ExplanationNode> {
    for node in nodes {
        match node {
            ExplanationNode::RuleRef { .. } => return Some(node),
            ExplanationNode::Compose { operands, .. }
            | ExplanationNode::Conversion { operands, .. } => {
                if let Some(found) = first_rule_ref(operands) {
                    return Some(found);
                }
            }
            ExplanationNode::Row { key, fields, .. } => {
                if let Some(found) = first_rule_ref(key) {
                    return Some(found);
                }
                if let Some(found) = first_rule_ref(fields) {
                    return Some(found);
                }
            }
            ExplanationNode::Filter { children, .. } => {
                if let Some(found) = first_rule_ref(children) {
                    return Some(found);
                }
            }
            ExplanationNode::Data { .. }
            | ExplanationNode::DataUnused { .. }
            | ExplanationNode::Veto { .. } => {}
        }
    }
    None
}

#[test]
fn filter_zero_of_two_are_enabled() {
    let engine = load_ok(
        r#"
spec item
data enabled: boolean
data price: measure
  -> unit eur: 1.00
rule cost: price
spec bag
uses items: item list
  -> with enabled: false, price: 1 eur
  -> with enabled: false, price: 2 eur
rule total: items -> where enabled -> sum cost
"#,
    );
    let response = evaluate_explain(&engine, "bag", data(&[]));
    let explanation = response
        .results
        .get("total")
        .expect("total")
        .explanation
        .as_ref()
        .expect("explanation");
    let text = format_explanation(explanation);
    assert_contains(&text, "0 of 2 are enabled");
    assert!(!text.contains("1. cost"), "{text}");
    let json = serde_json::to_value(explanation).expect("json");
    let filter = find_type(&json, "filter").expect("filter");
    assert_eq!(filter["matched"], 0);
    assert_eq!(filter["total"], 2);
}

#[test]
fn filter_chained_where_nests_summaries() {
    let engine = load_ok(
        r#"
spec employee
data active: boolean
data salary: measure
  -> unit eur: 1.00
rule pay: salary
spec payroll
uses staff: employee list
  -> with active: true, salary: 1000 eur
  -> with active: true, salary: 2000 eur
  -> with active: false, salary: 3000 eur
rule total:
  staff -> where active -> where salary > 1500 eur -> sum pay
"#,
    );
    let response = evaluate_explain(&engine, "payroll", data(&[]));
    let explanation = response
        .results
        .get("total")
        .expect("total")
        .explanation
        .as_ref()
        .expect("explanation");
    let text = format_explanation(explanation);
    assert_contains(&text, "2 of 3 are active");
    assert_contains(&text, "1 of 2 are salary > 1500 eur");
    assert_contains(&text, "2. pay:");
}

#[test]
fn value_list_where_filter_summary() {
    let engine = load_ok(
        r#"
spec fares_card
data money: measure
  -> unit eur: 1.00
data fares: money list
  -> with 89 eur, 120 eur
rule high: fares -> where > 100 eur -> count
"#,
    );
    let response = evaluate_explain(&engine, "fares_card", data(&[]));
    let explanation = response
        .results
        .get("high")
        .expect("high")
        .explanation
        .as_ref()
        .expect("explanation");
    let text = format_explanation(explanation);
    assert_contains(&text, "1 of 2 are > 100 eur");
}

#[test]
fn appended_row_label_in_explanation() {
    let engine = load_ok(
        r#"
spec employee
data salary: measure
  -> unit eur: 1.00
spec payroll
uses staff: employee list
  -> with salary: 1000 eur
rule total:
  staff -> with salary: 2000 eur -> sum salary
"#,
    );
    let response = evaluate_explain(&engine, "payroll", data(&[]));
    let explanation = response
        .results
        .get("total")
        .expect("total")
        .explanation
        .as_ref()
        .expect("explanation");
    let text = format_explanation(explanation);
    assert_contains(&text, "1. salary:");
    assert_contains(&text, "appended 2. salary:");
}

#[test]
fn stacked_rows_have_no_connector() {
    let engine = load_ok(
        r#"
spec employee
data salary: measure
  -> unit eur: 1.00
spec payroll
uses staff: employee list
  -> with salary: 1000 eur
  -> with salary: 2000 eur
rule total: staff -> sum salary
"#,
    );
    let response = evaluate_explain(&engine, "payroll", data(&[]));
    let explanation = response
        .results
        .get("total")
        .expect("total")
        .explanation
        .as_ref()
        .expect("explanation");
    let text = format_explanation(explanation);
    assert_contains(&text, "\n   1. salary:");
    assert_contains(&text, "\n   2. salary:");
    assert!(
        !text.contains("├─ 1.") && !text.contains("└─ 1.") && !text.contains("├─ 2."),
        "row lines must be connector-less:\n{text}"
    );
}

fn find_type<'a>(value: &'a serde_json::Value, type_name: &str) -> Option<&'a serde_json::Value> {
    if value.get("type").and_then(|t| t.as_str()) == Some(type_name) {
        return Some(value);
    }
    match value {
        serde_json::Value::Object(map) => map.values().find_map(|v| find_type(v, type_name)),
        serde_json::Value::Array(items) => items.iter().find_map(|v| find_type(v, type_name)),
        _ => None,
    }
}
