// Criterion cases synced with xtask/src/benchmarks/cli.rs HTTP_BENCH_CASES.
use criterion::{criterion_group, criterion_main, Criterion};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const PORT: u16 = 19877;
const BASE: &str = "http://127.0.0.1:19877";

struct ServerGuard(Child);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        if self
            .0
            .try_wait()
            .expect("BUG: query bench server status")
            .is_none()
        {
            self.0.kill().expect("BUG: kill bench server");
            self.0.wait().expect("BUG: reap bench server");
        }
    }
}

fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn start_server() -> ServerGuard {
    let examples_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../engine/documentation/examples");

    let child = Command::new(env!("CARGO_BIN_EXE_lemma"))
        .arg("server")
        .args(["--path", &examples_dir.to_string_lossy()])
        .args(["--port", &PORT.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn lemma server");

    if !wait_for_port(PORT, Duration::from_secs(10)) {
        panic!("lemma server did not become ready on port {PORT}");
    }

    ServerGuard(child)
}

fn bench_evaluate(c: &mut Criterion) {
    let server = start_server();
    let client = reqwest::blocking::Client::new();

    let mut group = c.benchmark_group("evaluate");
    group.measurement_time(std::time::Duration::from_secs(10));

    // --- simple: coffee order (7 rules, basic arithmetic + unless) ---
    group.bench_function("coffee_order", |b| {
        b.iter(|| {
            let resp = client
                .post(format!("{BASE}/coffee_order"))
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body("product=latte&size=large&number_of_cups=3&has_loyalty_card=true&age=30")
                .send()
                .expect("POST coffee_order");
            assert!(resp.status().is_success());
        });
    });

    // --- medium: library fees (5 rules, conditionals) ---
    group.bench_function("library_fees", |b| {
        b.iter(|| {
            let resp = client
                .post(format!("{BASE}/library_fees"))
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body("book_type=reference&days_overdue=14&is_first_offense=false")
                .send()
                .expect("POST library_fees");
            assert!(resp.status().is_success());
        });
    });

    // --- complex: Dutch net salary (20+ rules, 3 tax brackets, credits) ---
    group.bench_function("dutch_salary", |b| {
        b.iter(|| {
            let resp = client
                .post(format!("{BASE}/net_salary"))
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body("gross_salary=5000 eur&pay_period=month&income_source=employment&pension_contribution=150 eur&payroll_tax_credit=true")
                .send()
                .expect("POST net_salary");
            assert!(resp.status().is_success());
        });
    });

    group.finish();

    // --- show retrieval (GET, no evaluation) ---
    let mut show_group = c.benchmark_group("show");

    show_group.bench_function("dutch_salary", |b| {
        b.iter(|| {
            let resp = client
                .get(format!("{BASE}/net_salary"))
                .send()
                .expect("GET net_salary");
            assert!(resp.status().is_success());
        });
    });

    show_group.finish();
    drop(server);
}

criterion_group!(benches, bench_evaluate);
criterion_main!(benches);
