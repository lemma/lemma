//! Compact catalog for Arc-sharing tests.
//!
//! Two carriers and two services, so identical rate cards and zone charts stay
//! on different specs. Weights `1..=10` and ZIP3 `098..=101` cover the shipment
//! inputs (`dest_zip3` `100`, billable weight about 6.9 lb).

use crate::fixture::{self, LogisticsFixture, FUEL_BRACKET_ROWS, SERVICE_IDS, ZONES};

const SHARING_WEIGHTS: usize = 10;
const SHARING_ZIP_START: usize = 98;
const SHARING_ZIP_COUNT: usize = 4;
const SHARING_SERVICES: usize = 2;

pub struct SharingLayout {
    pub carriers: Vec<String>,
    pub services: Vec<&'static str>,
    pub zones: usize,
    pub weights: usize,
    pub zip_count: usize,
    pub fuel_rows: usize,
}

impl SharingLayout {
    pub fn pipelines(&self) -> usize {
        self.carriers.len() * self.services.len()
    }

    pub fn cells_per_card(&self) -> usize {
        self.zones * self.weights
    }

    pub fn rates_spec(&self, carrier: &str, service: &str) -> String {
        format!("rates_{carrier}_{service}")
    }

    pub fn quote_spec(&self, carrier: &str, service: &str) -> String {
        format!("quote_{carrier}_{service}")
    }
}

pub fn logistics_sharing() -> (LogisticsFixture, SharingLayout) {
    let carriers = vec!["ups".to_string(), "fedex".to_string()];
    let services = SERVICE_IDS[..SHARING_SERVICES].to_vec();
    let fixture = fixture::embedded_catalog(
        "sharing",
        carriers.clone(),
        services.len(),
        ZONES,
        SHARING_WEIGHTS,
        SHARING_ZIP_START,
        SHARING_ZIP_COUNT,
    );
    let layout = SharingLayout {
        carriers,
        services,
        zones: ZONES,
        weights: SHARING_WEIGHTS,
        zip_count: SHARING_ZIP_COUNT,
        fuel_rows: FUEL_BRACKET_ROWS,
    };
    (fixture, layout)
}
