//! Port of `benchmarks/java` `SpecGenerator.logistics`: multi-spec logistics
//! rating workspace on the research ladder 1050 / 6300 / 18900 / 126000 rate cells.
//!
//! Catalog tables (ZIP3→zone, zone×lb rates, fuel brackets) are instance lists
//! with `-> key` / `find`. The Rust and Java generators must emit byte-identical
//! source for the embedded placement; [`LADDER`] carries the expected length of
//! each profile and [`logistics`] asserts it.
//!
//! [`logistics_external_catalog`] emits the same workspace with empty keyed lists
//! and puts the catalog rows in [`LogisticsFixture::catalog_json`] for run data.

use indexmap::IndexMap;
use lemma::{EvaluateDataValue, ResourceLimits};
use serde_json::{json, Value};
use std::fmt::Write;

/// UPS Ground zones 2 to 8.
pub const ZONES: usize = 7;
/// 1 to 150 lb breaks.
pub const WEIGHTS: usize = 150;
/// Cells in one Ground matrix.
pub const GROUND_CELLS: usize = ZONES * WEIGHTS;
/// Services on the UPS zone chart header.
pub const SERVICES: usize = 6;
/// US ZIP3 prefixes.
pub const ZIP3_COUNT: usize = 900;
/// D2C multi-carrier count.
pub const D2C_CARRIERS: usize = 3;
/// Enterprise low contract count.
pub const ENTERPRISE_CONTRACTS: usize = 20;
/// Open end for last weight / fuel band (`[lo, hi)` containment).
const BAND_OPEN_END: i32 = 1_000_000;

const SERVICE_IDS: [&str; SERVICES] = [
    "ground",
    "select_3day",
    "air_2day",
    "air_2day_am",
    "nda_saver",
    "nda",
];

/// One rung of the research ladder with the source length the Java generator
/// produces for it.
pub struct Rung {
    pub rate_cells: usize,
    pub source_bytes: usize,
}

pub const LADDER: [Rung; 4] = [
    Rung {
        rate_cells: GROUND_CELLS,
        source_bytes: 87_898,
    },
    Rung {
        rate_cells: GROUND_CELLS * SERVICES,
        source_bytes: 509_122,
    },
    Rung {
        rate_cells: GROUND_CELLS * SERVICES * D2C_CARRIERS,
        source_bytes: 1_520_554,
    },
    Rung {
        rate_cells: GROUND_CELLS * SERVICES * ENTERPRISE_CONTRACTS,
        source_bytes: 10_121_625,
    },
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum CatalogPlacement {
    /// Rows embedded as `-> with` (Java-identical source).
    Embedded,
    /// Empty keyed lists; rows in [`LogisticsFixture::catalog_json`].
    External,
}

pub struct LogisticsFixture {
    pub profile: &'static str,
    pub rate_cells: usize,
    pub source: String,
    pub limits: ResourceLimits,
    /// Present for [`logistics_external_catalog`]: shipment scalars + catalog lists
    /// keyed by `rate_shop` input paths (`q0.r.cells`, …).
    pub catalog_json: Option<Value>,
}

impl LogisticsFixture {
    pub const SPEC_NAME: &'static str = "rate_shop";
    pub const TERMINAL_RULE: &'static str = "cheapest";

    /// Shipment inputs, same values as the Java `shipmentData()`.
    pub fn inputs() -> IndexMap<String, EvaluateDataValue> {
        IndexMap::from([
            ("length_in".into(), "12".into()),
            ("width_in".into(), "10".into()),
            ("height_in".into(), "8".into()),
            ("actual_lb".into(), "5".into()),
            ("dest_zip3".into(), "100".into()),
            ("is_residential".into(), "true".into()),
            ("is_das".into(), "false".into()),
            ("diesel_usd".into(), "4.50".into()),
            ("jet_usd".into(), "2.40".into()),
            ("discount_pct".into(), "10".into()),
        ])
    }
}

struct Profile {
    name: &'static str,
    carriers: Vec<String>,
    service_count: usize,
    rate_cells: usize,
}

fn profile(rate_cells: usize) -> Profile {
    match rate_cells {
        GROUND_CELLS => Profile {
            name: "ground",
            carriers: vec!["ups".to_string()],
            service_count: 1,
            rate_cells,
        },
        cells if cells == GROUND_CELLS * SERVICES => Profile {
            name: "carrier",
            carriers: vec!["ups".to_string()],
            service_count: SERVICES,
            rate_cells,
        },
        cells if cells == GROUND_CELLS * SERVICES * D2C_CARRIERS => Profile {
            name: "d2c",
            carriers: vec![
                "ups".to_string(),
                "fedex".to_string(),
                "regional".to_string(),
            ],
            service_count: SERVICES,
            rate_cells,
        },
        cells if cells == GROUND_CELLS * SERVICES * ENTERPRISE_CONTRACTS => Profile {
            name: "enterprise",
            carriers: (1..=ENTERPRISE_CONTRACTS)
                .map(|index| format!("contract_{index:02}"))
                .collect(),
            service_count: SERVICES,
            rate_cells,
        },
        other => panic!(
            "BUG: logistics rate_cells must be one of {:?}, got {other}",
            LADDER.map(|rung| rung.rate_cells)
        ),
    }
}

/// Mirrors the Java `BenchLimits.forSize(limitScale)` overrides.
fn limits_for_scale(scale: usize, placement: CatalogPlacement) -> ResourceLimits {
    let scale = scale.max(1_000);
    let source_bytes = (16 * 1024 * 1024).max(scale * 256);
    let mut limits = ResourceLimits {
        max_source_size_bytes: source_bytes,
        max_expression_count: 65_536.max(scale * 64),
        max_normalized_expression_nodes: 100_000.max(scale * 128),
        max_loaded_bytes: (100 * 1024 * 1024).max(source_bytes * 4),
        ..ResourceLimits::default()
    };
    if placement == CatalogPlacement::External {
        // Largest single list is one rate card (`GROUND_CELLS` = 1050).
        limits.max_list_rows = GROUND_CELLS.max(limits.max_list_rows);
        limits.max_list_nodes = GROUND_CELLS.max(limits.max_list_nodes);
    }
    limits
}

/// Java `%.2f`. Every emitted price is an exact two-decimal value in real
/// arithmetic (sums of two-decimal terms), so the double sits within 1e-13 of
/// it and both Java's half-up-on-shortest-digits and Rust's exact-binary
/// rounding land on the same two decimals; the byte-length assertion guards it.
fn two_decimals(value: f64) -> String {
    format!("{value:.2}")
}

/// Embedded catalog rows (byte-identical to the Java generator).
pub fn logistics(rate_cells: usize) -> LogisticsFixture {
    build(rate_cells, CatalogPlacement::Embedded)
}

/// Same workspace with empty keyed lists; catalog rows in `catalog_json`.
pub fn logistics_external_catalog(rate_cells: usize) -> LogisticsFixture {
    build(rate_cells, CatalogPlacement::External)
}

fn build(rate_cells: usize, placement: CatalogPlacement) -> LogisticsFixture {
    let profile = profile(rate_cells);
    let rung = LADDER
        .iter()
        .find(|rung| rung.rate_cells == rate_cells)
        .expect("BUG: profile() accepted a rate_cells value absent from LADDER");
    let capacity = match placement {
        CatalogPlacement::Embedded => rung.source_bytes.max(rate_cells * 48),
        CatalogPlacement::External => 64 * 1024,
    };
    let mut out = String::with_capacity(capacity);
    let mut catalog = match placement {
        CatalogPlacement::Embedded => None,
        CatalogPlacement::External => Some(shipment_catalog_object()),
    };

    emit_row_specs(&mut out);
    out.push('\n');
    emit_accessorials(&mut out);
    out.push('\n');

    let fuel_ground_rows = fuel_bracket_rows(2.50, 20.0, 0.27, 0.25);
    let fuel_air_rows = fuel_bracket_rows(1.50, 18.0, 0.05, 0.25);
    emit_fuel_spec(
        &mut out,
        "fuel_ground",
        "Ground fuel % from diesel index brackets (weekly table shape).",
        "diesel_usd",
        &fuel_ground_rows,
        placement,
    );
    out.push('\n');
    emit_fuel_spec(
        &mut out,
        "fuel_air",
        "Air fuel % from jet index brackets.",
        "jet_usd",
        &fuel_air_rows,
        placement,
    );
    out.push('\n');

    let fuel_ground_json = fuel_brackets_json(&fuel_ground_rows);
    let fuel_air_json = fuel_brackets_json(&fuel_air_rows);

    let mut quote_spec_names = Vec::new();
    let mut quote_index = 0usize;
    for carrier in &profile.carriers {
        let zone_tables = zone_tables(profile.service_count);
        emit_zones_spec(&mut out, carrier, &zone_tables, placement);
        out.push('\n');
        for (service_index, service) in SERVICE_IDS
            .iter()
            .copied()
            .enumerate()
            .take(profile.service_count)
        {
            let cells = rate_cell_rows(service_index);
            emit_rates_spec(&mut out, carrier, service, &cells, placement);
            out.push('\n');
            let quote_name = format!("quote_{carrier}_{service}");
            emit_quote_pipeline(&mut out, &quote_name, carrier, service, service_index);
            out.push('\n');

            if let Some(catalog) = catalog.as_mut() {
                let catalog_obj = catalog
                    .as_object_mut()
                    .expect("BUG: catalog_json is a JSON object");
                catalog_obj.insert(format!("q{quote_index}.r.cells"), rate_cells_json(&cells));
                catalog_obj.insert(
                    format!("q{quote_index}.z.{service}_zones"),
                    zones_json(&zone_tables[service_index]),
                );
                let fuel_json = if service_index >= 2 {
                    fuel_air_json.clone()
                } else {
                    fuel_ground_json.clone()
                };
                catalog_obj.insert(format!("q{quote_index}.f.brackets"), fuel_json);
            }

            quote_spec_names.push(quote_name);
            quote_index += 1;
        }
    }

    emit_rate_shop(&mut out, &quote_spec_names);
    out.push('\n');

    if placement == CatalogPlacement::Embedded {
        assert_eq!(
            out.len(),
            rung.source_bytes,
            "logistics {} source diverged from the Java generator (actual {})",
            profile.name,
            out.len()
        );
    }

    let limit_scale =
        profile.rate_cells + profile.carriers.len() * ZIP3_COUNT * profile.service_count + 256;

    LogisticsFixture {
        profile: profile.name,
        rate_cells: profile.rate_cells,
        source: out,
        limits: limits_for_scale(limit_scale, placement),
        catalog_json: catalog,
    }
}

fn shipment_catalog_object() -> Value {
    json!({
        "length_in": "12",
        "width_in": "10",
        "height_in": "8",
        "actual_lb": "5",
        "dest_zip3": "100",
        "is_residential": true,
        "is_das": false,
        "diesel_usd": "4.50",
        "jet_usd": "2.40",
        "discount_pct": "10",
    })
}

fn emit_row_specs(out: &mut String) {
    out.push_str("spec zip_zone\n");
    out.push_str("data dest_zip3: text\n");
    out.push_str("data zone: number\n\n");

    out.push_str("spec rate_cell\n");
    out.push_str("data zone: number\n");
    out.push_str("data band: number range\n");
    out.push_str("data price: number\n\n");

    out.push_str("spec fuel_bracket\n");
    out.push_str("data band: number range\n");
    out.push_str("data pct: ratio\n");
}

fn emit_accessorials(out: &mut String) {
    out.push_str("spec accessorials\n");
    out.push_str("\"\"\"Parcel accessorial schedule (tens of fees, not thousands).\"\"\"\n\n");
    out.push_str("data is_residential: boolean\n");
    out.push_str("data is_das: boolean\n");
    out.push_str("data billable_lb: number\n");
    out.push_str("data longest_side_in: number\n\n");
    out.push_str("rule residential_fee: 0\n");
    out.push_str("  unless is_residential then 6.50\n\n");
    out.push_str("rule das_fee: 0\n");
    out.push_str("  unless is_das and is_residential then 4.20\n");
    out.push_str("  unless is_das and not is_residential then 2.80\n\n");
    out.push_str("rule ahs_weight_fee: 0\n");
    out.push_str("  unless billable_lb > 50 then 30\n\n");
    out.push_str("rule ahs_length_fee: 0\n");
    out.push_str("  unless longest_side_in > 48 then 30\n\n");
    out.push_str("rule large_package_fee: 0\n");
    out.push_str("  unless longest_side_in >= 96 then 95\n\n");
    out.push_str("rule accessorial_total:\n");
    out.push_str(
        "  residential_fee + das_fee + ahs_weight_fee + ahs_length_fee + large_package_fee\n",
    );
}

struct FuelBracketRow {
    band_lo: String,
    band_hi: String,
    pct: String,
}

fn fuel_bracket_rows(
    start_price: f64,
    start_pct: f64,
    price_step: f64,
    pct_step: f64,
) -> Vec<FuelBracketRow> {
    let mut rows = Vec::with_capacity(25);
    let mut price = start_price;
    let mut pct = start_pct;
    rows.push(FuelBracketRow {
        band_lo: "0".to_string(),
        band_hi: two_decimals(start_price),
        pct: two_decimals(start_pct),
    });
    for _ in 0..23 {
        let next = price + price_step;
        rows.push(FuelBracketRow {
            band_lo: two_decimals(price),
            band_hi: two_decimals(next),
            pct: two_decimals(pct),
        });
        price = next;
        pct += pct_step;
    }
    rows.push(FuelBracketRow {
        band_lo: two_decimals(price),
        band_hi: BAND_OPEN_END.to_string(),
        pct: two_decimals(pct),
    });
    rows
}

fn fuel_brackets_json(rows: &[FuelBracketRow]) -> Value {
    Value::Array(
        rows.iter()
            .map(|row| {
                json!({
                    "band": format!("{}...{}", row.band_lo, row.band_hi),
                    "pct": format!("{}%", row.pct),
                })
            })
            .collect(),
    )
}

fn emit_fuel_spec(
    out: &mut String,
    name: &str,
    doc: &str,
    index_data: &str,
    rows: &[FuelBracketRow],
    placement: CatalogPlacement,
) {
    writeln!(out, "spec {name}").expect("BUG: writing to String never fails");
    writeln!(out, "\"\"\"{doc}\"\"\"\n").expect("BUG: writing to String never fails");
    out.push_str("uses brackets: fuel_bracket list\n");
    out.push_str("  -> key band\n");
    if placement == CatalogPlacement::Embedded {
        for row in rows {
            writeln!(
                out,
                "  -> with band: {}...{}, pct: {}%",
                row.band_lo, row.band_hi, row.pct
            )
            .expect("BUG: writing to String never fails");
        }
    }
    out.push('\n');
    writeln!(out, "data {index_data}: number\n").expect("BUG: writing to String never fails");
    writeln!(out, "rule fuel_pct: (brackets -> find {index_data}).pct")
        .expect("BUG: writing to String never fails");
}

struct ZoneRow {
    dest_zip3: String,
    zone: usize,
}

fn zone_tables(service_count: usize) -> Vec<Vec<ZoneRow>> {
    (0..service_count)
        .map(|service_index| {
            (0..ZIP3_COUNT)
                .map(|zip3| ZoneRow {
                    dest_zip3: format!("{zip3:03}"),
                    zone: 2 + (zip3 + service_index) % ZONES,
                })
                .collect()
        })
        .collect()
}

fn zones_json(rows: &[ZoneRow]) -> Value {
    Value::Array(
        rows.iter()
            .map(|row| {
                json!({
                    "dest_zip3": row.dest_zip3,
                    "zone": row.zone.to_string(),
                })
            })
            .collect(),
    )
}

fn emit_zones_spec(
    out: &mut String,
    carrier: &str,
    zone_tables: &[Vec<ZoneRow>],
    placement: CatalogPlacement,
) {
    writeln!(out, "spec zones_{carrier}").expect("BUG: writing to String never fails");
    out.push_str("\"\"\"ZIP3 → zone for ship-from origin (one warehouse chart).\"\"\"\n\n");
    for (service_index, service) in SERVICE_IDS
        .iter()
        .copied()
        .enumerate()
        .take(zone_tables.len())
    {
        writeln!(out, "uses {service}_zones: zip_zone list")
            .expect("BUG: writing to String never fails");
        out.push_str("  -> key dest_zip3\n");
        if placement == CatalogPlacement::Embedded {
            for row in &zone_tables[service_index] {
                writeln!(
                    out,
                    "  -> with dest_zip3: \"{}\", zone: {}",
                    row.dest_zip3, row.zone
                )
                .expect("BUG: writing to String never fails");
            }
        }
        out.push('\n');
    }
    out.push_str("data dest_zip3: text\n\n");
    for service in SERVICE_IDS.iter().copied().take(zone_tables.len()) {
        writeln!(
            out,
            "rule {service}_zone: ({service}_zones -> find dest_zip3).zone"
        )
        .expect("BUG: writing to String never fails");
    }
}

struct RateCellRow {
    zone: usize,
    band_lo: i32,
    band_hi: i32,
    price: String,
}

fn rate_cell_rows(service_index: usize) -> Vec<RateCellRow> {
    let mut rows = Vec::with_capacity(GROUND_CELLS);
    for zone_index in 0..ZONES {
        let zone = 2 + zone_index;
        for lb in 1..=WEIGHTS {
            let price = 8.0 + zone as f64 * 1.15 + lb as f64 * 0.42 + service_index as f64 * 0.75;
            let band_hi = if lb == WEIGHTS {
                BAND_OPEN_END
            } else {
                (lb + 1) as i32
            };
            rows.push(RateCellRow {
                zone,
                band_lo: lb as i32,
                band_hi,
                price: two_decimals(price),
            });
        }
    }
    rows
}

fn rate_cells_json(rows: &[RateCellRow]) -> Value {
    Value::Array(
        rows.iter()
            .map(|row| {
                json!({
                    "zone": row.zone.to_string(),
                    "band": format!("{}...{}", row.band_lo, row.band_hi),
                    "price": row.price,
                })
            })
            .collect(),
    )
}

fn emit_rates_spec(
    out: &mut String,
    carrier: &str,
    service: &str,
    cells: &[RateCellRow],
    placement: CatalogPlacement,
) {
    writeln!(out, "spec rates_{carrier}_{service}").expect("BUG: writing to String never fails");
    writeln!(
        out,
        "\"\"\"Zone × lb rate card ({GROUND_CELLS} cells).\"\"\"\n"
    )
    .expect("BUG: writing to String never fails");
    out.push_str("uses cells: rate_cell list\n");
    out.push_str("  -> key zone, band\n");
    if placement == CatalogPlacement::Embedded {
        for row in cells {
            writeln!(
                out,
                "  -> with zone: {}, band: {}...{}, price: {}",
                row.zone, row.band_lo, row.band_hi, row.price
            )
            .expect("BUG: writing to String never fails");
        }
    }
    out.push('\n');
    out.push_str("data zone: number\n");
    out.push_str("data billable_lb: number\n\n");
    out.push_str("rule base_rate: (cells -> find zone, billable_lb).price\n");
}

fn emit_quote_pipeline(
    out: &mut String,
    quote_name: &str,
    carrier: &str,
    service: &str,
    service_index: usize,
) {
    let air = service_index >= 2;
    let fuel_spec = if air { "fuel_air" } else { "fuel_ground" };
    let fuel_data = if air { "jet_usd" } else { "diesel_usd" };

    writeln!(out, "spec {quote_name}").expect("BUG: writing to String never fails");
    writeln!(
        out,
        "\"\"\"Rating pipeline for {carrier} {service}.\"\"\"\n"
    )
    .expect("BUG: writing to String never fails");

    writeln!(out, "uses z: zones_{carrier}").expect("BUG: writing to String never fails");
    out.push_str("  -> with dest_zip3: dest_zip3\n\n");
    writeln!(out, "uses r: rates_{carrier}_{service}").expect("BUG: writing to String never fails");
    out.push_str("  -> with zone: zone\n");
    out.push_str("  -> with billable_lb: billable_lb\n\n");
    out.push_str("uses a: accessorials\n");
    out.push_str("  -> with is_residential: is_residential\n");
    out.push_str("  -> with is_das: is_das\n");
    out.push_str("  -> with billable_lb: billable_lb\n");
    out.push_str("  -> with longest_side_in: length_in\n\n");
    writeln!(out, "uses f: {fuel_spec}").expect("BUG: writing to String never fails");
    writeln!(out, "  -> with {fuel_data}: {fuel_data}\n")
        .expect("BUG: writing to String never fails");

    out.push_str("data length_in: number\n");
    out.push_str("data width_in: number\n");
    out.push_str("data height_in: number\n");
    out.push_str("data actual_lb: number\n");
    out.push_str("data dest_zip3: text\n");
    out.push_str("data is_residential: boolean\n");
    out.push_str("data is_das: boolean\n");
    out.push_str("data diesel_usd: number\n");
    out.push_str("data jet_usd: number\n");
    out.push_str("data discount_pct: number\n\n");

    out.push_str("rule dim_lb: (length_in * width_in * height_in) / 139\n");
    out.push_str("rule billable_lb: actual_lb\n");
    out.push_str("  unless dim_lb > actual_lb then dim_lb\n");
    writeln!(out, "rule zone: z.{service}_zone").expect("BUG: writing to String never fails");
    out.push_str("rule base: r.base_rate\n");
    out.push_str("rule accessorials_total: a.accessorial_total\n");
    out.push_str("rule pre_fuel: base + accessorials_total\n");
    out.push_str("rule fuel_amount: pre_fuel * f.fuel_pct\n");
    out.push_str("rule with_fuel: pre_fuel + fuel_amount\n");
    out.push_str("rule discount_amount: with_fuel * (discount_pct / 100)\n");
    out.push_str("rule with_discount: with_fuel - discount_amount\n");
    out.push_str("rule total: with_discount\n");
}

fn emit_rate_shop(out: &mut String, quote_spec_names: &[String]) {
    out.push_str("spec rate_shop\n");
    out.push_str("\"\"\"Wide rate-shop: compare carrier×service quotes, pick cheapest.\"\"\"\n\n");

    for (index, quote) in quote_spec_names.iter().enumerate() {
        writeln!(out, "uses q{index}: {quote}").expect("BUG: writing to String never fails");
        out.push_str("  -> with length_in: length_in\n");
        out.push_str("  -> with width_in: width_in\n");
        out.push_str("  -> with height_in: height_in\n");
        out.push_str("  -> with actual_lb: actual_lb\n");
        out.push_str("  -> with dest_zip3: dest_zip3\n");
        out.push_str("  -> with is_residential: is_residential\n");
        out.push_str("  -> with is_das: is_das\n");
        out.push_str("  -> with diesel_usd: diesel_usd\n");
        out.push_str("  -> with jet_usd: jet_usd\n");
        out.push_str("  -> with discount_pct: discount_pct\n\n");
    }

    out.push_str("data length_in: number\n");
    out.push_str("data width_in: number\n");
    out.push_str("data height_in: number\n");
    out.push_str("data actual_lb: number\n");
    out.push_str("data dest_zip3: text\n");
    out.push_str("data is_residential: boolean\n");
    out.push_str("data is_das: boolean\n");
    out.push_str("data diesel_usd: number\n");
    out.push_str("data jet_usd: number\n");
    out.push_str("data discount_pct: number\n\n");

    for index in 0..quote_spec_names.len() {
        writeln!(out, "rule quote_{index}: q{index}.total")
            .expect("BUG: writing to String never fails");
    }
    out.push('\n');
    if quote_spec_names.len() == 1 {
        out.push_str("rule cheapest: quote_0\n");
    } else {
        out.push_str("rule min_1: quote_0\n");
        out.push_str("  unless quote_1 < quote_0 then quote_1\n");
        for index in 2..quote_spec_names.len() {
            let previous = index - 1;
            writeln!(out, "rule min_{index}: min_{previous}")
                .expect("BUG: writing to String never fails");
            writeln!(
                out,
                "  unless quote_{index} < min_{previous} then quote_{index}"
            )
            .expect("BUG: writing to String never fails");
        }
        writeln!(out, "rule cheapest: min_{}", quote_spec_names.len() - 1)
            .expect("BUG: writing to String never fails");
    }
}
