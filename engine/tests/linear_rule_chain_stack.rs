//! Linear rule-chain stack safety: tip-only eval and deep load must not abort.
//!
//! Rule references are evaluation boundaries; `normal_form_depth` is iterative with rule references
//! as leaves. These tests pin success (correct values / successful load) on
//! small stacks that previously SIGSEGV'd.

use lemma::{DateTimeValue, Engine, SourceType};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::fmt::Write;
use std::str::FromStr;

const ONE_MIB: usize = 1024 * 1024;

fn deep_chain_source(n: usize) -> String {
    assert!(n >= 1, "chain needs at least r1");
    let mut code = String::with_capacity(n * 32);
    write!(code, "spec bench_deep\ndata x0: number\nrule r1: x0 + 1\n").unwrap();
    for i in 2..=n {
        writeln!(code, "rule r{i}: r{} + 1", i - 1).unwrap();
    }
    code
}

fn load_deep(n: usize) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, deep_chain_source(n))])
        .unwrap_or_else(|errs| {
            panic!("load N={n} must succeed or return Error, not abort: {errs:?}")
        });
    engine
}

fn x0_zero() -> HashMap<String, String> {
    let mut data = HashMap::new();
    data.insert("x0".to_string(), "0".to_string());
    data
}

fn rule_number(response: &lemma::Response, rule: &str) -> Decimal {
    let result = response.get(rule).unwrap_or_else(|_| panic!("rule {rule}"));
    assert!(!result.vetoed, "rule {rule} vetoed");
    Decimal::from_str(
        result
            .value
            .as_ref()
            .expect("rule result value")
            .number
            .as_ref()
            .expect("number payload"),
    )
    .expect("decimal")
}

fn run_tip(engine: &Engine, tip: &str, explain: bool) -> lemma::Response {
    let now = DateTimeValue::now();
    engine
        .run(
            None,
            "bench_deep",
            Some(&now),
            x0_zero(),
            Some(&[tip.to_string()]),
            explain,
        )
        .unwrap_or_else(|e| panic!("run {tip} must succeed or return Error, not abort: {e:?}"))
}

fn run_tip_on_stack(
    engine: Engine,
    tip: String,
    stack_size: usize,
    explain: bool,
) -> lemma::Response {
    let handle = std::thread::Builder::new()
        .stack_size(stack_size)
        .spawn(move || run_tip(&engine, &tip, explain))
        .unwrap_or_else(|e| {
            panic!("spawn with stack_size={stack_size} failed (do not silently raise stack): {e}")
        });
    match handle.join() {
        Ok(response) => response,
        Err(_) => panic!("eval thread panicked or aborted for tip (stack overflow)"),
    }
}

/// Control: crash tracks requested dependency depth, not Spec size alone.
#[test]
fn deep_chain_tip_r1_on_1mib_stack_succeeds() {
    let engine = load_deep(200);
    let response = run_tip_on_stack(engine, "r1".to_string(), ONE_MIB, false);
    assert_eq!(rule_number(&response, "r1"), Decimal::from(1));
}

/// Tip-only eval of r56 / r100 on a JNI-scale stack.
#[test]
fn deep_chain_tip_r56_and_r100_on_1mib_stack_succeeds() {
    let engine = load_deep(200);
    let response_56 = run_tip_on_stack(engine, "r56".to_string(), ONE_MIB, false);
    assert_eq!(rule_number(&response_56, "r56"), Decimal::from(56));

    let engine = load_deep(200);
    let response_100 = run_tip_on_stack(engine, "r100".to_string(), ONE_MIB, false);
    assert_eq!(rule_number(&response_100, "r100"), Decimal::from(100));
}

/// Tip-only explain of r56 on a JNI-scale stack.
#[test]
fn deep_chain_tip_r56_explain_on_1mib_stack_succeeds() {
    let engine = load_deep(200);
    let response = run_tip_on_stack(engine, "r56".to_string(), ONE_MIB, true);
    assert_eq!(rule_number(&response, "r56"), Decimal::from(56));
    let explanation = response
        .get("r56")
        .expect("r56")
        .explanation
        .as_ref()
        .expect("explain");
    assert_eq!(explanation.name.rule, "r56");
}

/// Tip-only eval of r1000 on the default test stack.
#[test]
fn deep_chain_tip_r1000_on_default_stack_succeeds() {
    let engine = load_deep(1000);
    let response = run_tip(&engine, "r1000", false);
    assert_eq!(rule_number(&response, "r1000"), Decimal::from(1000));
}

/// Tip-only explain of r100 on the default test stack.
#[test]
fn deep_chain_tip_r100_explain_on_default_stack_succeeds() {
    let engine = load_deep(200);
    let response = run_tip(&engine, "r100", true);
    assert_eq!(rule_number(&response, "r100"), Decimal::from(100));
    let explanation = response
        .get("r100")
        .expect("r100")
        .explanation
        .as_ref()
        .expect("explain");
    assert_eq!(explanation.name.rule, "r100");
}

/// Load of a 1600-rule linear chain must not abort during planning.
#[test]
fn load_linear_chain_1600_succeeds() {
    let _engine = load_deep(1600);
}

/// Load of a 5000-rule linear chain must not abort during planning.
#[test]
fn load_linear_chain_5000_succeeds() {
    let _engine = load_deep(5000);
}

/// Load of N=5000 on a JNI-scale stack (planning walks must not recurse).
#[test]
fn load_linear_chain_5000_on_1mib_stack_succeeds() {
    let handle = std::thread::Builder::new()
        .stack_size(ONE_MIB)
        .spawn(|| {
            let _engine = load_deep(5000);
        })
        .unwrap_or_else(|e| {
            panic!("spawn with stack_size={ONE_MIB} failed (do not silently raise stack): {e}")
        });
    handle
        .join()
        .expect("load thread panicked or aborted (stack overflow)");
}
