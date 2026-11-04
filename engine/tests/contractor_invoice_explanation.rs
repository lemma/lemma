//! Structural explanation trees for the contractor invoice spec.

use indexmap::IndexMap;
use lemma::{format_explanation, DateTimeValue, Engine, EvaluateDataValue};

const CALC_SPEC: &str = r#"
spec calc

data money: measure
  -> unit eur: 1

data hourly_rate: 85.00 eur
data hours_worked: 37.5
data is_rush: boolean
data is_super_rush: boolean

rule labor: hourly_rate * hours_worked
rule rush_surcharge: 0 eur
  unless is_rush then labor * 25%
  unless is_super_rush then labor * 50%
rule subtotal: labor + rush_surcharge
rule vat: subtotal * 21%
rule total: subtotal + vat
"#;

fn evaluate_calc(data: IndexMap<String, EvaluateDataValue>) -> lemma::Explanation {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, CALC_SPEC.to_string())])
        .expect("calc spec loads");
    let now = DateTimeValue::now();
    let response = engine
        .evaluate(None, "calc", Some(&now), data, None, true)
        .expect("calc eval succeeds");
    response
        .results
        .values()
        .find(|r| r.rule.name == "total")
        .expect("total rule evaluated")
        .explanation
        .clone()
        .expect("explanation always built")
}

const NEITHER_UNLESS_MATCHES: &str = "\
total: 3856.875 eur
└─ subtotal + vat
   ├─ subtotal: 3187.5 eur
   │  └─ labor + rush_surcharge
   │     ├─ labor: 3187.5 eur
   │     │  └─ hourly_rate * hours_worked
   │     │     ├─ hourly_rate: 85 eur
   │     │     └─ hours_worked: 37.5
   │     └─ rush_surcharge: 0 eur
   │        ├─ is_rush is false
   │        └─ is_super_rush is false
   └─ vat: 669.375 eur
      └─ subtotal * 21%
         └─ subtotal: 3187.5 eur (see above)";

const IS_RUSH_ONLY: &str = "\
total: 4821.09375 eur
└─ subtotal + vat
   ├─ subtotal: 3984.375 eur
   │  └─ labor + rush_surcharge
   │     ├─ labor: 3187.5 eur
   │     │  └─ hourly_rate * hours_worked
   │     │     ├─ hourly_rate: 85 eur
   │     │     └─ hours_worked: 37.5
   │     └─ rush_surcharge: 796.875 eur
   │        ├─ is_super_rush is false
   │        ├─ is_rush
   │        └─ labor * 25%
   │           └─ labor: 3187.5 eur (see above)
   └─ vat: 836.71875 eur
      └─ subtotal * 21%
         └─ subtotal: 3984.375 eur (see above)";

const IS_SUPER_RUSH: &str = "\
total: 5785.3125 eur
└─ subtotal + vat
   ├─ subtotal: 4781.25 eur
   │  └─ labor + rush_surcharge
   │     ├─ labor: 3187.5 eur
   │     │  └─ hourly_rate * hours_worked
   │     │     ├─ hourly_rate: 85 eur
   │     │     └─ hours_worked: 37.5
   │     └─ rush_surcharge: 1593.75 eur
   │        ├─ is_super_rush
   │        └─ labor * 50%
   │           └─ labor: 3187.5 eur (see above)
   └─ vat: 1004.0625 eur
      └─ subtotal * 21%
         └─ subtotal: 4781.25 eur (see above)";

#[test]
fn contractor_invoice_neither_unless_matches() {
    let mut data = IndexMap::new();
    data.insert("is_rush".into(), "false".into());
    data.insert("is_super_rush".into(), "false".into());
    let explanation = evaluate_calc(data);
    assert_eq!(format_explanation(&explanation), NEITHER_UNLESS_MATCHES);
}

#[test]
fn contractor_invoice_is_rush_only() {
    let mut data = IndexMap::new();
    data.insert("is_rush".into(), "true".into());
    data.insert("is_super_rush".into(), "false".into());
    let explanation = evaluate_calc(data);
    assert_eq!(format_explanation(&explanation), IS_RUSH_ONLY);
}

#[test]
fn contractor_invoice_is_super_rush() {
    let mut data = IndexMap::new();
    data.insert("is_rush".into(), "true".into());
    data.insert("is_super_rush".into(), "true".into());
    let explanation = evaluate_calc(data);
    assert_eq!(format_explanation(&explanation), IS_SUPER_RUSH);
}
