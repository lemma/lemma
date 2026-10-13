use criterion::{criterion_group, criterion_main, Criterion, Throughput};
mod common;
#[path = "common/eval.rs"]
mod eval;

fn plan_only(fixture: &common::Fixture) {
    std::hint::black_box(eval::load_engine(fixture));
}

fn bench_fixture(criterion: &mut Criterion, fixture: &common::Fixture) {
    let mut group = criterion.benchmark_group(fixture.spec_name);
    group.throughput(Throughput::Elements(1));

    group.bench_function("plan", |bencher| {
        bencher.iter(|| plan_only(fixture));
    });

    let engine = eval::load_engine(fixture);
    let rules = [eval::terminal_rule(fixture.spec_name).to_string()];

    group.bench_function("evaluate", |bencher| {
        bencher.iter(|| {
            let data = fixture.inputs();
            let response = eval::evaluate_loaded(&engine, fixture.spec_name, data, &rules, false);
            std::hint::black_box(response);
        });
    });

    group.bench_function("show", |bencher| {
        bencher.iter(|| {
            let show = engine
                .show(None, fixture.spec_name, Some(&common::effective()))
                .expect("BUG: bench fixture must show");
            std::hint::black_box(show);
        });
    });

    group.bench_function("evaluate_explain", |bencher| {
        bencher.iter(|| {
            let data = fixture.inputs();
            let response = eval::evaluate_loaded(&engine, fixture.spec_name, data, &rules, true);
            std::hint::black_box(response);
        });
    });

    group.finish();
}

fn benches(criterion: &mut Criterion) {
    for fixture in common::fixtures() {
        bench_fixture(criterion, &fixture);
    }
}

criterion_group!(group, benches);
criterion_main!(group);
