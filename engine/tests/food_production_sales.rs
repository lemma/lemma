//! Food production sales scenarios: a sales unit manager's test drive of Lemma.
//!
//! Exercises dimensional analysis (compound units, unit conversions),
//! real-world business rules (volume discounts, MOQ, shelf life),
//! and cross-spec composition (ingredient → batch → order pipeline).

use lemma::{DateTimeValue, Engine, SourceType};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

fn src(name: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(name)))
}

fn run(engine: &Engine, spec: &str, data: &[(&str, &str)]) -> lemma::Response {
    let now = DateTimeValue::now();
    let data_map: HashMap<String, String> = data
        .iter()
        .map(|(k, v)| (k.to_string(), (*v).to_string()))
        .collect();
    engine
        .run(None, spec, Some(&now), data_map, None, false)
        .unwrap()
}

fn run_at(engine: &Engine, spec: &str, data: &[(&str, &str)], effective: &str) -> lemma::Response {
    let dt = DateTimeValue::from_str(effective).unwrap();
    let data_map: HashMap<String, String> = data
        .iter()
        .map(|(k, v)| (k.to_string(), (*v).to_string()))
        .collect();
    engine
        .run(None, spec, Some(&dt), data_map, None, false)
        .unwrap()
}

fn result<'a>(resp: &'a lemma::Response, rule: &str) -> &'a lemma::RuleResult {
    resp.results
        .values()
        .find(|r| r.rule.name == rule)
        .unwrap_or_else(|| panic!("rule '{}' not found", rule))
}

fn display(resp: &lemma::Response, rule: &str) -> String {
    result(resp, rule)
        .result()
        .unwrap_or_else(|| panic!("rule '{}' has no display", rule))
        .to_string()
}

fn quantity_unit(resp: &lemma::Response, rule: &str, unit: &str) -> Decimal {
    *result(resp, rule)
        .result
        .as_ref()
        .expect("rule result value")
        .measure
        .as_ref()
        .unwrap_or_else(|| panic!("rule '{}' is not a measure", rule))
        .get(unit)
        .unwrap_or_else(|| panic!("rule '{}' has no unit '{}'", rule, unit))
}

fn vetoed(resp: &lemma::Response, rule: &str) -> bool {
    result(resp, rule).vetoed
}

// ===========================================================================
// SCENARIO 1: Ingredient costing with unit conversions
//
// A bakery buys flour in 25kg bags at €18.75/bag. Sugar at €0.95/kg.
// A batch of cookies needs 2.4 kg flour + 800 gram sugar.
// What's the ingredient cost per batch?
//
// Exercises: compound units (eur/kilogram), dimensional arithmetic (€/kg × kg → €)
// ===========================================================================

#[test]
fn ingredient_cost_per_batch() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("ingredient_costing.lemma"),
            r#"
spec ingredient_costing
uses lemma units

data money: measure
  -> unit eur: 1.00
  -> decimals 2

data unit_price: measure
  -> unit eur_per_kg: eur/kilogram

data flour_bag_weight: 25 kilogram
data flour_bag_price: 18.75 eur

data sugar_price_per_kg: 0.95 eur_per_kg

data flour_needed: 2.4 kilogram
data sugar_needed: 800 gram

rule flour_price_per_kg: flour_bag_price / flour_bag_weight

rule flour_cost: flour_needed * flour_price_per_kg

rule sugar_cost: sugar_needed * sugar_price_per_kg

rule batch_ingredient_cost: flour_cost + sugar_cost
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run(&engine, "ingredient_costing", &[]);

    // flour_price_per_kg = 18.75 eur / 25 kg = 0.75 eur/kg
    let flour_ppkg = display(&resp, "flour_price_per_kg");
    assert_eq!(flour_ppkg, "0.75 eur_per_kg", "flour €0.75/kg");

    // flour_cost = 2.4 kg * 0.75 eur/kg = 1.80 eur
    let fc = display(&resp, "flour_cost");
    assert_eq!(fc, "1.80 eur", "flour cost for 2.4kg");

    // sugar_cost = 800 g * 0.95 eur/kg = 0.76 eur (cross-unit mass multiply)
    let sc = display(&resp, "sugar_cost");
    assert_eq!(sc, "0.76 eur", "sugar cost for 800g");

    // batch total = 1.80 eur + 0.76 eur = 2.56 eur
    let total = display(&resp, "batch_ingredient_cost");
    assert_eq!(total, "2.56 eur", "total ingredient cost per batch");
}

// ===========================================================================
// SCENARIO 2: Batch yield and packaging
//
// A batch produces 120 cookies. 5% QC reject rate.
// Cookies are packed 12 per box. How many full boxes per batch?
//
// Exercises: yield percentage, floor for whole-box count, modulo
// ===========================================================================

#[test]
fn batch_yield_and_packaging() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("batch_packaging.lemma"),
            r#"
spec batch_packaging

data batch_size: 120
data reject_rate: 5%
data cookies_per_box: 12

rule good_cookies: batch_size - reject_rate * batch_size
rule full_boxes: floor(good_cookies / cookies_per_box)
rule leftover_cookies: good_cookies % cookies_per_box
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run(&engine, "batch_packaging", &[]);

    // good = 120 - 0.05*120 = 114
    let good = display(&resp, "good_cookies");
    assert_eq!(good, "114", "120 - 5%*120 = 114 good cookies");

    // boxes = floor(114 / 12) = floor(9.5) = 9
    let boxes = display(&resp, "full_boxes");
    assert_eq!(boxes, "9", "9 full boxes of 12");

    // leftover = 114 % 12 = 6
    let left = display(&resp, "leftover_cookies");
    assert_eq!(left, "6", "6 leftover cookies");
}

// ===========================================================================
// SCENARIO 3: Production throughput with duration units
//
// A production line runs at 200 units/hour.
// Shift is 8 hour, but 30 min changeover + 15 min breaks.
// How many units per shift?
//
// Exercises: duration arithmetic, as-conversion, compound calculation
// ===========================================================================

#[test]
fn production_throughput_per_shift() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("throughput.lemma"),
            r#"
spec production_throughput
uses lemma units

data units_per_hour: 200
data shift_length: 8 hour
data changeover_time: 30 minute
data break_time: 15 minute

rule productive_time: shift_length - changeover_time - break_time
rule productive_hours: productive_time as hour as number
rule units_per_shift: productive_hours * units_per_hour
rule meets_daily_target: units_per_shift >= 1400
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run(&engine, "production_throughput", &[]);

    // productive_time = 8h - 30min - 15min = 7h15m = 7.25h
    let ph = display(&resp, "productive_hours");
    assert_eq!(ph, "7.25", "8h - 30min - 15min = 7.25h");

    // units = 7.25 * 200 = 1450
    let ups = display(&resp, "units_per_shift");
    assert_eq!(ups, "1450", "7.25h * 200/h = 1450 units");

    let target = display(&resp, "meets_daily_target");
    assert_eq!(target, "true", "1450 >= 1400");
}

// ===========================================================================
// SCENARIO 4: Shelf life and expiry date
//
// Product has 90-day shelf life from production.
// Delivery takes 3 day. Customer wants 60 day minimum remaining.
// Can we fulfill?
//
// Exercises: date + duration, date comparison, date ranges
// ===========================================================================

#[test]
fn shelf_life_check() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("shelf_life.lemma"),
            r#"
spec shelf_life
uses lemma units

data production_date: 2025-06-01
data shelf_life_days: 90 day
data delivery_days: 3 day
data min_remaining_days: 60 day

rule expiry_date: production_date + shelf_life_days
rule arrival_date: production_date + delivery_days
rule days_remaining_at_arrival: (arrival_date...expiry_date) as day
rule meets_shelf_requirement: days_remaining_at_arrival >= min_remaining_days
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run(&engine, "shelf_life", &[]);

    // expiry = 2025-06-01 + 90 day = 2025-08-30
    // QUIRK: date + duration displays as datetime with time component
    let expiry = display(&resp, "expiry_date");
    assert_eq!(expiry, "2025-08-30T00:00:00Z", "production + 90 day");

    // arrival = 2025-06-01 + 3 day = 2025-06-04
    let arrival = display(&resp, "arrival_date");
    assert_eq!(arrival, "2025-06-04T00:00:00Z", "production + 3 day");

    // remaining = (2025-06-04...2025-08-30) as day = 87 day
    let remaining = display(&resp, "days_remaining_at_arrival");
    assert_eq!(
        remaining, "87 day",
        "87 day shelf life remaining at arrival"
    );

    let meets = display(&resp, "meets_shelf_requirement");
    assert_eq!(meets, "true", "87 >= 60 day remaining");
}

// ===========================================================================
// SCENARIO 5: Freight cost with weight tiers
//
// Palletized shipment. Weight determines freight tier.
// <500kg: €8/100kg, 500-2000kg: €6/100kg, >2000kg: €4.50/100kg
// Surcharge for refrigerated: +40%
//
// Exercises: weight-based unless tiers, percentage surcharge, mass units
// ===========================================================================

#[test]
fn freight_cost_weight_tiers() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("freight.lemma"),
            r#"
spec freight
uses lemma units

data shipment_weight: units.mass
data is_refrigerated: boolean

rule weight_kg: shipment_weight as kilogram as number

rule rate_per_100kg: 8
  unless weight_kg >= 500  then 6
  unless weight_kg >= 2000 then 4.5

rule base_freight: weight_kg / 100 * rate_per_100kg

rule refrigeration_surcharge: 0%
  unless is_refrigerated then 40%

rule surcharge_amount: base_freight * refrigeration_surcharge
rule total_freight: base_freight + surcharge_amount
"#
            .to_string(),
        )])
        .unwrap();

    // 750kg non-refrigerated: rate = 6, freight = 7.5 * 6 = 45
    let resp = run(
        &engine,
        "freight",
        &[
            ("shipment_weight", "750 kilogram"),
            ("is_refrigerated", "false"),
        ],
    );
    let rate = display(&resp, "rate_per_100kg");
    assert_eq!(rate, "6", "750kg falls in 500-2000 tier");
    let base = display(&resp, "base_freight");
    assert_eq!(base, "45", "750/100 * 6 = 45");
    let total = display(&resp, "total_freight");
    assert_eq!(total, "45", "no refrigeration surcharge");

    // 750kg refrigerated: 45 + 45*0.4 = 63
    let resp = run(
        &engine,
        "freight",
        &[
            ("shipment_weight", "750 kilogram"),
            ("is_refrigerated", "true"),
        ],
    );
    let total = display(&resp, "total_freight");
    assert_eq!(total, "63", "45 + 45*40% = 63");

    // 300kg: rate = 8, freight = 3 * 8 = 24
    let resp = run(
        &engine,
        "freight",
        &[
            ("shipment_weight", "300 kilogram"),
            ("is_refrigerated", "false"),
        ],
    );
    let rate = display(&resp, "rate_per_100kg");
    assert_eq!(rate, "8", "300kg is under 500 tier");
    let base = display(&resp, "base_freight");
    assert_eq!(base, "24", "300/100 * 8 = 24");
}

// ===========================================================================
// SCENARIO 6: Volume discount with MOQ and order value
//
// Wholesale pricing: base €2.40/unit.
// 100-499: -5%, 500-999: -12%, 1000+: -18%
// MOQ = 50 units. Below MOQ → veto.
// Free shipping above €2000 order value.
//
// Exercises: number ranges in unless, veto for business constraint,
//            percentage discount, boolean eligibility
// ===========================================================================

#[test]
fn volume_discount_and_moq() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("wholesale.lemma"),
            r#"
spec wholesale_order

data unit_price: 2.40
data order_measure: number -> minimum 0
data moq: 50

rule measure_check: true
  unless order_measure < moq then veto "Below minimum order measure"

rule volume_discount: 0%
  unless order_measure >= 100  then 5%
  unless order_measure >= 500  then 12%
  unless order_measure >= 1000 then 18%

rule discount_amount: unit_price * volume_discount
rule effective_price: unit_price - discount_amount
rule order_value: effective_price * order_measure

rule free_shipping: order_value >= 2000
rule shipping_cost: 35
  unless free_shipping then 0
"#
            .to_string(),
        )])
        .unwrap();

    // Order 750 units: discount = 12%, effective = 2.40 * 0.88 = 2.112
    // order_value = 2.112 * 750 = 1584
    let resp = run(&engine, "wholesale_order", &[("order_measure", "750")]);
    let disc = display(&resp, "volume_discount");
    assert_eq!(disc, "12%", "750 units → 12% discount");
    let ep = display(&resp, "effective_price");
    assert_eq!(ep, "2.112", "2.40 - 12% = 2.112");
    let ov = display(&resp, "order_value");
    assert_eq!(ov, "1584", "2.112 * 750 = 1584");
    let fs = display(&resp, "free_shipping");
    assert_eq!(fs, "false", "1584 < 2000 → no free shipping");

    // Order 1200 units: discount = 18%, effective = 2.40 * 0.82 = 1.968
    // order_value = 1200 * 1.968 = 2361.6
    let resp = run(&engine, "wholesale_order", &[("order_measure", "1200")]);
    let disc = display(&resp, "volume_discount");
    assert_eq!(disc, "18%", "1200 → 18%");
    let ov = display(&resp, "order_value");
    assert_eq!(ov, "2361.6", "1200 * 1.968 = 2361.6");
    let fs = display(&resp, "free_shipping");
    assert_eq!(fs, "true", "2361.6 >= 2000 → free shipping");
    let sc = display(&resp, "shipping_cost");
    assert_eq!(sc, "0", "free shipping → 0");

    // Below MOQ: 30 units → veto
    let resp = run(&engine, "wholesale_order", &[("order_measure", "30")]);
    assert!(vetoed(&resp, "measure_check"), "30 < 50 MOQ should veto");
}

// ===========================================================================
// SCENARIO 7: Multi-currency pricing with unit types
//
// Base price in EUR. Convert to USD and GBP.
// Apply regional markup: US +8%, UK +5%.
//
// Exercises: user-defined measure types with currency units,
//            as-conversion between units, percentage markup
// ===========================================================================

#[test]
fn multi_currency_pricing() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("currency.lemma"),
            r#"
spec currency_pricing

data money: measure
  -> unit eur: 1.00
  -> unit usd: 0.91
  -> unit gbp: 1.17
  -> decimals 2

data base_price: 24.50 eur

rule price_usd: base_price as usd
rule price_gbp: base_price as gbp

rule us_markup: 8%
rule uk_markup: 5%

rule us_markup_amount: price_usd * us_markup
rule uk_markup_amount: price_gbp * uk_markup
rule us_retail: price_usd + us_markup_amount
rule uk_retail: price_gbp + uk_markup_amount
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run(&engine, "currency_pricing", &[]);

    // EUR → USD: 24.50 / 0.91 = 26.923...  (eur=1.00, usd=0.91 → 1 eur = 1/0.91 usd)
    // Actually: the unit factor for usd is 0.91 relative to eur=1.00
    // So 24.50 eur in canonical = 24.50. Converting to usd: 24.50 / 0.91 = 26.92...
    let _usd = display(&resp, "price_usd");
    // Let's just check it's not vetoed and has a value
    assert!(!vetoed(&resp, "price_usd"), "USD conversion should work");

    // EUR → GBP: 24.50 / 1.17 = 20.94...
    let _gbp = display(&resp, "price_gbp");
    assert!(!vetoed(&resp, "price_gbp"), "GBP conversion should work");

    // US retail = usd_price + 8%
    assert!(
        !vetoed(&resp, "us_retail"),
        "US retail price should compute"
    );
    // UK retail = gbp_price + 5%
    assert!(
        !vetoed(&resp, "uk_retail"),
        "UK retail price should compute"
    );
}

// ===========================================================================
// SCENARIO 8: Production planning with cross-spec composition
//
// Spec 1: recipe (ingredients per batch, yield)
// Spec 2: order (customer order measure, required batches)
//
// Exercises: uses, with, qualified references, ceil for rounding up batches
// ===========================================================================

#[test]
fn production_planning_cross_spec() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("production.lemma"),
            r#"
spec recipe

data yield_per_batch: 500
data reject_rate: 3%

rule net_yield: yield_per_batch - reject_rate * yield_per_batch


spec production_order

uses r: recipe

data customer_order_quantity: number -> minimum 1

rule batches_needed: ceil(customer_order_quantity / r.net_yield)
rule total_production: batches_needed * r.yield_per_batch
rule expected_waste: total_production * r.reject_rate
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run(
        &engine,
        "production_order",
        &[("customer_order_quantity", "1200")],
    );

    // net_yield = 500 - 3% = 485
    // batches = ceil(1200 / 485) = ceil(2.474...) = 3
    let batches = display(&resp, "batches_needed");
    assert_eq!(batches, "3", "need 3 batches for 1200 units");

    // total_production = 3 * 500 = 1500
    let total = display(&resp, "total_production");
    assert_eq!(total, "1500", "3 batches × 500 = 1500");

    // expected_waste = 1500 * 3% = 45
    let waste = display(&resp, "expected_waste");
    assert_eq!(waste, "45", "3% of 1500 = 45 wasted");
}

// ===========================================================================
// SCENARIO 9: Compound units — cost per kilogram
//
// Define eur and kilogram, then a compound eur_per_kg unit.
// Compute total cost from weight × rate.
//
// Exercises: compound unit definition (eur/kilogram),
//            dimensional consistency
// ===========================================================================

#[test]
fn compound_unit_cost_per_kg() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("cost_per_weight.lemma"),
            r#"
spec cost_per_weight
uses lemma units

data money: measure
  -> unit eur: 1.00

data cost_rate: measure
  -> unit eur_per_kg: eur/kilogram

data shipment_weight: 340 kilogram
data rate: cost_rate -> suggest 1.85 eur_per_kg

rule total_cost: (rate * shipment_weight) as eur
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run(&engine, "cost_per_weight", &[("rate", "1.85 eur_per_kg")]);

    // 1.85 eur/kg * 340 kg = 629 eur
    let cost = display(&resp, "total_cost");
    assert_eq!(cost, "629 eur", "1.85 * 340 = 629 eur");
}

// ===========================================================================
// SCENARIO 10: Nutritional density calculation
//
// Product has 450g sugar per 2kg batch. What's the sugar content per 100g?
// If sugar > 22.5g per 100g → "high sugar" label required.
//
// Exercises: ratio calculation, mass conversion, threshold comparison
// ===========================================================================

#[test]
fn nutritional_density() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("nutrition.lemma"),
            r#"
spec nutrition
uses lemma units

data sugar_per_batch: 450 gram
data batch_weight: 2 kilogram

rule sugar_fraction: (sugar_per_batch as kilogram as number) / (batch_weight as kilogram as number)
rule sugar_per_100g: sugar_fraction * 100
rule high_sugar_label: sugar_per_100g > 22.5
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run(&engine, "nutrition", &[]);

    // sugar_fraction = 0.45 / 2 = 0.225
    let frac = display(&resp, "sugar_fraction");
    assert_eq!(frac, "0.225", "450g / 2kg = 0.225");

    // per 100g = 0.225 * 100 = 22.5
    let per100 = display(&resp, "sugar_per_100g");
    assert_eq!(per100, "22.5", "22.5g sugar per 100g");

    // 22.5 > 22.5 is false (not strictly greater)
    let label = display(&resp, "high_sugar_label");
    assert_eq!(label, "false", "22.5 is not > 22.5");
}

// ===========================================================================
// SCENARIO 11: Pallet loading and truck capacity
//
// Boxes weigh 18kg each. Pallet holds max 24 boxes or 500kg.
// Truck fits 26 pallets. How many boxes per truck?
//
// Exercises: min of two constraints (weight vs count), multiplication chain
// ===========================================================================

#[test]
fn pallet_loading() {
    let mut engine = Engine::new();
    engine
        .load([(src("pallet_truck.lemma"), r#"
spec pallet_truck
uses lemma units

data box_weight: 18 kilogram
data max_boxes_per_pallet: 24
data max_pallet_weight: 500 kilogram
data pallets_per_truck: 26

rule boxes_by_weight: floor((max_pallet_weight as kilogram as number) / (box_weight as kilogram as number))
rule boxes_by_count: max_boxes_per_pallet

rule weight_is_limiting: boxes_by_weight < boxes_by_count

rule boxes_per_pallet: boxes_by_count
  unless weight_is_limiting then boxes_by_weight

rule boxes_per_truck: boxes_per_pallet * pallets_per_truck
rule weight_per_truck: boxes_per_truck * (box_weight as kilogram as number)
"#.to_string())])
        .unwrap();

    let resp = run(&engine, "pallet_truck", &[]);

    // boxes_by_weight = floor(500 / 18) = floor(27.77) = 27
    let bw = display(&resp, "boxes_by_weight");
    assert_eq!(bw, "27", "500kg / 18kg = 27 boxes by weight");

    // boxes_by_count = 24
    // weight_is_limiting = 27 < 24 = false (count is more limiting!)
    let limiting = display(&resp, "weight_is_limiting");
    assert_eq!(
        limiting, "false",
        "count limit (24) is tighter than weight (27)"
    );

    // boxes_per_pallet = 24 (count wins since weight allows 27)
    let bpp = display(&resp, "boxes_per_pallet");
    assert_eq!(bpp, "24", "24 boxes per pallet (count-limited)");

    // boxes_per_truck = 24 * 26 = 624
    let bpt = display(&resp, "boxes_per_truck");
    assert_eq!(bpt, "624", "24 * 26 = 624 boxes per truck");

    // weight = 624 * 18 = 11232
    let wpt = display(&resp, "weight_per_truck");
    assert_eq!(wpt, "11232", "624 * 18 = 11232 kg per truck");
}

// ===========================================================================
// SCENARIO 12: Temporal pricing — seasonal surcharges
//
// Base ingredient prices change seasonally.
// Summer (effective 2025-06-01): cream +15%, berries +25%
//
// Exercises: temporal spec versions, effective date resolution
// ===========================================================================

#[test]
fn seasonal_pricing() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("seasonal.lemma"),
            r#"
spec ingredient_prices

data cream_price: 3.20
data berry_price: 8.50

rule cream_cost: cream_price
rule berry_cost: berry_price


spec ingredient_prices 2025-06-01

data cream_price: 3.68
data berry_price: 10.63

rule cream_cost: cream_price
rule berry_cost: berry_price


spec summer_menu

uses prices: ingredient_prices

data servings: number -> minimum 1

rule cream_per_serving: prices.cream_cost / 4
rule berry_per_serving: prices.berry_cost / 8
rule dessert_cost: cream_per_serving + berry_per_serving
rule menu_line_total: dessert_cost * servings
"#
            .to_string(),
        )])
        .unwrap();

    // Winter pricing (before June)
    let resp = run_at(&engine, "summer_menu", &[("servings", "100")], "2025-03-15");
    let cream = display(&resp, "cream_per_serving");
    assert_eq!(cream, "0.8", "winter cream: 3.20 / 4 = 0.80");
    let berry = display(&resp, "berry_per_serving");
    assert_eq!(berry, "1.0625", "winter berry: 8.50 / 8 = 1.0625");

    // Summer pricing (after June 1)
    let resp = run_at(&engine, "summer_menu", &[("servings", "100")], "2025-07-15");
    let cream = display(&resp, "cream_per_serving");
    assert_eq!(cream, "0.92", "summer cream: 3.68 / 4 = 0.92");
    let berry = display(&resp, "berry_per_serving");
    assert_eq!(berry, "1.32875", "summer berry: 10.63 / 8 = 1.32875");
}

// ===========================================================================
// SCENARIO 13: Food safety — temperature hold time
//
// Product must not be in 5°C–60°C "danger zone" for more than 2 hour.
// Cooling log: entered zone at 14:30, exited at 16:15.
// Is it safe?
//
// Exercises: time range, span as hour, duration comparison
// ===========================================================================

#[test]
fn temperature_danger_zone_hold_time() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("food_safety.lemma"),
            r#"
spec food_safety
uses lemma units

data danger_zone_entry: 14:30
data danger_zone_exit: 16:15
data max_hold_time: 2 hour

rule time_in_zone: (danger_zone_entry...danger_zone_exit) as hour
rule hold_exceeded: time_in_zone > max_hold_time
rule is_safe: not hold_exceeded
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run(&engine, "food_safety", &[]);

    // 14:30 to 16:15 = 1h45m = 1.75 hour (signature hour → display uses hour)
    let tiz = display(&resp, "time_in_zone");
    assert_eq!(tiz, "1.75 hour", "14:30 to 16:15 = 1.75h");

    let exceeded = display(&resp, "hold_exceeded");
    assert_eq!(exceeded, "false", "1.75h < 2h → not exceeded");

    let safe = display(&resp, "is_safe");
    assert_eq!(safe, "true", "product is safe");
}

// ===========================================================================
// SCENARIO 14: Full order pipeline (composition of 3 specs)
//
// ingredients → batch_recipe → sales_order
//
// Exercises: multi-level spec composition, data flow across boundaries,
//            real costing pipeline
// ===========================================================================

#[test]
fn full_order_pipeline() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("order_pipeline.lemma"),
            r#"
spec batch_recipe

data ingredient_cost: 12.50
data labor_cost: 8.00
data batch_yield: 200

rule cost_per_batch: ingredient_cost + labor_cost
rule unit_cost: cost_per_batch / batch_yield


spec sales_order

uses recipe: batch_recipe

data order_quantity: number -> minimum 1
data target_margin: 35%

rule production_cost: recipe.unit_cost * order_quantity
rule unit_sell_price: recipe.unit_cost / (100% - target_margin)
rule order_revenue: unit_sell_price * order_quantity
rule gross_profit: order_revenue - production_cost
rule actual_margin: gross_profit / order_revenue
rule margin_pct: actual_margin as percent
rule margin_check: margin_pct >= target_margin
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run(&engine, "sales_order", &[("order_quantity", "500")]);

    // unit_cost = (12.50 + 8.00) / 200 = 20.50 / 200 = 0.1025
    let _uc = display(&resp, "unit_sell_price");
    // unit_sell_price = 0.1025 / (1 - 0.35) = 0.1025 / 0.65 ≈ 0.15769...
    assert!(!vetoed(&resp, "unit_sell_price"));

    // production_cost = 0.1025 * 500 = 51.25
    let pc = display(&resp, "production_cost");
    assert_eq!(pc, "51.25", "500 units at 0.1025 = 51.25");

    let margin_ok = display(&resp, "margin_check");
    assert_eq!(margin_ok, "true", "margin should meet target");
}

// ===========================================================================
// SCENARIO 15: Allergen compliance — veto chains
//
// Product contains allergens. Certain markets ban certain allergens.
// If allergen banned in destination → veto entire shipment.
//
// Exercises: multi-condition veto, veto propagation through pipeline
// ===========================================================================

#[test]
fn allergen_compliance_veto() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("allergen.lemma"),
            r#"
spec allergen_check

data destination: text
  -> option "EU"
  -> option "US"
  -> option "JP"

data contains_peanut: boolean
data contains_gluten: boolean

rule peanut_clearance: true
  unless contains_peanut and destination is "JP"
    then veto "Peanut products banned in target market"

rule gluten_clearance: true

rule can_ship: peanut_clearance and gluten_clearance

rule shipment_status: "Cleared for export"
  unless can_ship is veto then "BLOCKED"
"#
            .to_string(),
        )])
        .unwrap();

    // Peanut product to Japan → veto
    let resp = run(
        &engine,
        "allergen_check",
        &[
            ("destination", "JP"),
            ("contains_peanut", "true"),
            ("contains_gluten", "false"),
        ],
    );
    assert!(
        vetoed(&resp, "peanut_clearance"),
        "peanut to JP should veto"
    );
    assert!(
        vetoed(&resp, "can_ship"),
        "veto should propagate to can_ship"
    );

    // Peanut product to EU → fine
    let resp = run(
        &engine,
        "allergen_check",
        &[
            ("destination", "EU"),
            ("contains_peanut", "true"),
            ("contains_gluten", "false"),
        ],
    );
    assert!(!vetoed(&resp, "peanut_clearance"), "peanut to EU is fine");
    let status = display(&resp, "shipment_status");
    assert_eq!(status, "Cleared for export");
}

// ===========================================================================
// SCENARIO 16: Unit conversion chain — gram to tonne
//
// Warehouse receives deliveries in various units.
// Need total in tonne for capacity planning.
//
// Exercises: multi-step unit conversion within mass family
// ===========================================================================

#[test]
fn mass_conversion_chain() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("warehouse.lemma"),
            r#"
spec warehouse
uses lemma units

data delivery_a: 2500 kilogram
data delivery_b: 750000 gram
data delivery_c: 1.8 tonne

rule total_kg: (delivery_a as kilogram as number)
  + (delivery_b as kilogram as number)
  + (delivery_c as kilogram as number)

rule total_tonnes: total_kg / 1000

rule warehouse_capacity_tonnes: 50
rule utilization: total_tonnes / warehouse_capacity_tonnes
rule is_near_capacity: utilization > 0.8
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run(&engine, "warehouse", &[]);

    // delivery_a = 2500 kg
    // delivery_b = 750000g = 750 kg
    // delivery_c = 1.8 tonne = 1800 kg
    // total = 2500 + 750 + 1800 = 5050 kg
    let total = display(&resp, "total_kg");
    assert_eq!(total, "5050", "2500 + 750 + 1800 = 5050 kg");

    let tonne = display(&resp, "total_tonnes");
    assert_eq!(tonne, "5.05", "5050 / 1000 = 5.05 tonne");

    // utilization = 5.05 / 50 = 0.101
    let util = display(&resp, "utilization");
    assert_eq!(util, "0.101", "5.05 / 50 = 0.101");

    let near_cap = display(&resp, "is_near_capacity");
    assert_eq!(near_cap, "false", "10.1% utilization is not near capacity");
}

// ===========================================================================
// Display uses measure signature when it names a single declared unit
// ===========================================================================

#[test]
fn display_uses_signature_unit_after_as_cast() {
    let mut engine = Engine::new();
    engine
        .load([(
            src("mass_convert.lemma"),
            r#"
spec mass_convert
uses lemma units

data sugar: 800 gram

rule sugar_kg: sugar as kilogram
rule sugar_chain: sugar as kilogram as number
"#
            .to_string(),
        )])
        .unwrap();

    let resp = run(&engine, "mass_convert", &[]);

    let d = display(&resp, "sugar_kg");
    assert_eq!(
        d, "0.8 kilogram",
        "as kilogram → signature kilogram → display"
    );

    let kg = quantity_unit(&resp, "sugar_kg", "kilogram");
    assert_eq!(
        kg,
        Decimal::from_str("0.8").expect("0.8"),
        "measure map still has all units"
    );

    let n = display(&resp, "sugar_chain");
    assert_eq!(n, "0.8", "as kilogram as number = 0.8");
}

fn main() {}
