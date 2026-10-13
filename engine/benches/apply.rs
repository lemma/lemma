//! Cost of applying one edited spec set into an engine that already holds `width`
//! unrelated spec sets (three temporal versions each).

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use lemma_fixtures::apply::{invoice_total_source, path_source, wide_engine, EDITED_LABEL};

const WIDTHS: [usize; 3] = [16, 64, 256];

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
