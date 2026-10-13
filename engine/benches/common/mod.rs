use indexmap::IndexMap;
use lemma::{DateGranularity, DateTimeValue, EvaluateDataValue};
use std::path::PathBuf;

pub struct Fixture {
    pub spec_name: &'static str,
    pub lemma_path: &'static str,
    pub source: &'static str,
}

pub fn effective() -> DateTimeValue {
    DateTimeValue {
        year: 2026,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: None,
        granularity: DateGranularity::Full,
    }
}

fn shipping_inputs() -> IndexMap<String, EvaluateDataValue> {
    IndexMap::from([
        ("weight".into(), "3".into()),
        ("destination".into(), "domestic".into()),
        ("is_member".into(), "false".into()),
    ])
}

fn pricing_inputs() -> IndexMap<String, EvaluateDataValue> {
    IndexMap::from([
        ("product_type".into(), "premium".into()),
        ("quantity".into(), "25".into()),
        ("unit_price".into(), "100".into()),
        ("coupon_percent".into(), "5".into()),
        ("loyalty_years".into(), "2".into()),
        ("is_member".into(), "true".into()),
        ("is_loyalty".into(), "true".into()),
        ("is_tax_exempt".into(), "false".into()),
    ])
}

fn order_pipeline_inputs() -> IndexMap<String, EvaluateDataValue> {
    IndexMap::from([
        ("customer_tier".into(), "gold".into()),
        ("payment_method".into(), "credit".into()),
        ("shipping_zone".into(), "national".into()),
        ("quantity".into(), "12".into()),
        ("unit_price".into(), "85".into()),
        ("package_weight".into(), "3.5".into()),
        ("delivery_distance".into(), "180".into()),
        ("loyalty_points".into(), "6500".into()),
        ("coupon_percent".into(), "10".into()),
        ("is_fragile".into(), "true".into()),
        ("is_express".into(), "true".into()),
        ("is_hazardous".into(), "false".into()),
        ("is_gift".into(), "false".into()),
        ("is_first_time".into(), "false".into()),
    ])
}

impl Fixture {
    pub fn inputs(&self) -> IndexMap<String, EvaluateDataValue> {
        match self.spec_name {
            "bench_shipping" => shipping_inputs(),
            "bench_pricing" => pricing_inputs(),
            "bench_order_pipeline" => order_pipeline_inputs(),
            other => panic!("BUG: no inputs for bench spec '{other}'"),
        }
    }
}

pub fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            spec_name: "bench_shipping",
            lemma_path: "engine/benches/specs/shipping.lemma",
            source: include_str!("../specs/shipping.lemma"),
        },
        Fixture {
            spec_name: "bench_pricing",
            lemma_path: "engine/benches/specs/pricing.lemma",
            source: include_str!("../specs/pricing.lemma"),
        },
        Fixture {
            spec_name: "bench_order_pipeline",
            lemma_path: "engine/benches/specs/order_pipeline.lemma",
            source: include_str!("../specs/order_pipeline.lemma"),
        },
    ]
}

pub fn source_path(label: &str) -> PathBuf {
    PathBuf::from(label)
}
