//! Cost of applying one edited spec set into an engine that already holds `width`
//! unrelated spec sets (three temporal versions each).

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use lemma::{Engine, SourceType};
use std::path::PathBuf;
use std::sync::Arc;

const WIDTHS: [usize; 3] = [16, 64, 256];
const EDITED_SPEC: &str = "invoice_total";
const EDITED_LABEL: &str = "apply/invoice_total.lemma";

fn path_source(label: &str) -> SourceType {
    SourceType::Path(Arc::new(PathBuf::from(label)))
}

/// The edited spec's source with `threshold` as its volume discount cut-off, so each
/// apply submits different text.
fn invoice_total_source(threshold: u32) -> String {
    format!(
        "spec {EDITED_SPEC}\n\
         data quantity: number\n\
         data unit_price: number\n\
         rule net_amount: quantity * unit_price\n\
         rule volume_discount: 0%\n\
         \x20 unless quantity > {threshold} then 5%\n\
         rule discount_amount: net_amount * volume_discount\n\
         rule total: net_amount - discount_amount\n"
    )
}

fn wide_sources(width: usize) -> Vec<(SourceType, String)> {
    let mut sources: Vec<(SourceType, String)> = Vec::with_capacity(width * 3 + 1);
    for index in 0..width {
        for (effective, rate, label) in [
            ("", "21%", "origin"),
            (" 2025-07-01", "19%", "2025-07-01"),
            (" 2026-01-01", "20%", "2026-01-01"),
        ] {
            sources.push((
                path_source(&format!("apply/vat_rate_{index}_{label}.lemma")),
                format!(
                    "spec vat_rate_{index}{effective}\n\
                     data net_amount: number\n\
                     rule vat_due: net_amount * {rate}\n\
                     rule gross_amount: net_amount + vat_due\n"
                ),
            ));
        }
    }
    sources.push((path_source(EDITED_LABEL), invoice_total_source(10)));
    sources
}

fn wide_engine(width: usize) -> Engine {
    let mut engine = Engine::new();
    engine
        .load(wide_sources(width))
        .expect("BUG: apply workspace must load");
    engine
}

fn bench_wide_edit(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("apply/wide_edit");
    for width in WIDTHS {
        let mut engine = wide_engine(width);
        let mut threshold = 10_u32;
        group.bench_with_input(BenchmarkId::from_parameter(width), &width, |bencher, _| {
            bencher.iter(|| {
                threshold += 1;
                engine
                    .update(
                        None,
                        invoice_total_source(threshold),
                        path_source(EDITED_LABEL),
                    )
                    .expect("BUG: apply edit must apply");
            });
        });
    }
    group.finish();
}

criterion_group!(group, bench_wide_edit);
criterion_main!(group);
