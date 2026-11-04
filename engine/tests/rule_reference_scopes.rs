//! A rule reference reads the rule of the plan whose cell it is: a template's
//! reference reads the template's rule, even where the consumer has a rule of
//! the same name at another position.

use indexmap::IndexMap;
use lemma::{Engine, EvaluateDataValue};

const SOURCE: &str = r#"
spec tariff
data weight: number
rule handling: 3
rule surcharge: weight * 2 + handling

spec invoice
uses parcel: tariff
data base: number
rule net: base + handling
rule handling: 1000
rule shipping: parcel.surcharge
rule gross: net + shipping
"#;

fn inputs() -> IndexMap<String, EvaluateDataValue> {
    IndexMap::from([
        ("base".into(), "100".into()),
        ("parcel.weight".into(), "4".into()),
    ])
}

fn result(engine: &Engine, rule: &str) -> String {
    let response = engine
        .evaluate(
            None,
            "invoice",
            None,
            inputs(),
            Some(&[rule.to_string()]),
            false,
        )
        .expect("evaluate");
    response
        .results
        .get(rule)
        .and_then(|result| result.result())
        .unwrap_or_else(|| panic!("rule '{rule}' has a value"))
        .to_string()
}

#[test]
fn a_template_rule_reference_reads_the_template_rule() {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, SOURCE.to_string())])
        .expect("load");
    assert_eq!(result(&engine, "shipping"), "11");
    assert_eq!(result(&engine, "net"), "1100");
    assert_eq!(result(&engine, "gross"), "1111");
}
