//! A restored snapshot keeps the Arc identity that planning and sharing depend on.

use indexmap::IndexMap;
use lemma::{DateTimeValue, Engine, EvaluateDataValue, SourceType};
use std::collections::HashSet;

const PART_SOURCE: &str = r#"
spec part
data quantity: number
uses components: part list
rule unit_cost: quantity + (components -> sum unit_cost)
"#;

const ORDER_SOURCE: &str = r#"
spec order
uses lines: part list
rule total: lines -> sum unit_cost
"#;

const SHARED_TEMPLATES_SOURCE: &str = r#"
spec body
data n: number
rule doubled: n * 2

spec left
uses b: body
  -> with n: n
data n: number
rule out: b.doubled

spec right
uses b: body
  -> with n: n
data n: number
rule out: b.doubled

spec root
uses l: left
  -> with n: n
uses r: right
  -> with n: n
data n: number
rule total: l.out + r.out
"#;

fn restore(engine: &Engine) -> Engine {
    let bytes = engine.snapshot().expect("snapshot");
    Engine::from_snapshot(&bytes).expect("restore")
}

fn leaf(quantity: &str) -> EvaluateDataValue {
    EvaluateDataValue::Map(
        [(
            "quantity".to_string(),
            EvaluateDataValue::Literal(quantity.into()),
        )]
        .into_iter()
        .collect(),
    )
}

fn part_with_children(quantity: &str, children: Vec<EvaluateDataValue>) -> EvaluateDataValue {
    EvaluateDataValue::Map(
        [
            (
                "quantity".to_string(),
                EvaluateDataValue::Literal(quantity.into()),
            ),
            ("components".to_string(), EvaluateDataValue::List(children)),
        ]
        .into_iter()
        .collect(),
    )
}

fn order_total(engine: &Engine) -> String {
    let lines = EvaluateDataValue::List(vec![leaf("2"), part_with_children("3", vec![leaf("1")])]);
    let mut data = IndexMap::new();
    data.insert("lines".to_string(), lines);
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(
            None,
            "order",
            Some(&now),
            data,
            Some(&["total".to_string()]),
            false,
        )
        .expect("order must evaluate");
    response
        .results
        .get("total")
        .expect("total")
        .result()
        .expect("total must have a value")
        .to_string()
}

#[test]
fn restored_engine_plans_self_list_edge_like_original() {
    let mut original = Engine::new();
    original
        .load([(SourceType::Volatile, PART_SOURCE.to_string())])
        .expect("part must load");
    let mut restored = restore(&original);

    original
        .load([(SourceType::Volatile, ORDER_SOURCE.to_string())])
        .expect("order over a self-list must plan in the original engine");
    restored
        .load([(SourceType::Volatile, ORDER_SOURCE.to_string())])
        .expect("order over a self-list must plan in the restored engine");

    assert_eq!(order_total(&restored), order_total(&original));
    assert_eq!(order_total(&original), "6");
}

fn distinct_template_arcs(engine: &Engine) -> (usize, usize) {
    let ptrs = engine.debug_row_template_arc_ptrs(None, "root");
    let distinct = ptrs
        .iter()
        .map(|(_, _, ptr)| *ptr)
        .collect::<HashSet<usize>>()
        .len();
    (ptrs.len(), distinct)
}

#[test]
fn restored_row_templates_keep_their_sharing() {
    let mut original = Engine::new();
    original
        .load([(SourceType::Volatile, SHARED_TEMPLATES_SOURCE.to_string())])
        .expect("shared templates must plan");
    let (entries, distinct) = distinct_template_arcs(&original);
    assert!(
        distinct < entries,
        "precondition: original shares template Arcs ({distinct} distinct of {entries})"
    );
    let restored = restore(&original);
    assert_eq!(
        distinct_template_arcs(&restored),
        (entries, distinct),
        "restored templates must share the same Arcs as the original"
    );
}
