use lemma::{DateTimeValue, Engine, SourceType};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::str::FromStr;

#[test]
fn delivery_cost_converts_unit_with_show_decimals() {
    let code = r#"
spec delivery 2026-01-01

data distance: measure
  -> unit meter: 1
  -> unit kilometer: 1000

data money: measure
  -> decimals 2
  -> unit eur: 1.00
  -> unit usd: 0.84

data rate: measure
  -> unit eur_per_km: eur/kilometer

rule delivery_cost: 0.26 eur_per_km * distance
"#;

    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, code.to_string())])
        .unwrap();

    let effective = DateTimeValue {
        year: 2026,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: None,
        granularity: lemma::DateGranularity::Full,
    };

    let response = engine
        .run(
            None,
            "delivery",
            Some(&effective),
            HashMap::from([("distance".to_string(), "12 kilometer".into())]),
            None,
            false,
        )
        .expect("run");

    let delivery_cost = response
        .results
        .get("delivery_cost")
        .expect("delivery_cost rule");

    assert!(!delivery_cost.vetoed);
    let measure = delivery_cost
        .result
        .as_ref()
        .expect("rule result value")
        .measure
        .as_ref()
        .expect("measure map on delivery_cost");
    assert_eq!(
        measure.get("eur").copied(),
        Some(Decimal::from_str("3.12").expect("3.12"))
    );
    assert_eq!(
        measure.get("usd").copied(),
        Some(Decimal::from_str("3.71").expect("3.71"))
    );
}
