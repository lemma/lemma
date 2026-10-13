#[path = "support/get_rule_value.rs"]
mod get_rule_value;

use get_rule_value::get_rule_value;
use lemma::{Engine, SourceType};

const CHAIN_TERMS: usize = 20_000;
const SMALL_STACK: usize = 256 * 1024;

fn on_small_stack<F>(body: F)
where
    F: FnOnce() + Send + 'static,
{
    std::thread::Builder::new()
        .stack_size(SMALL_STACK)
        .spawn(body)
        .expect("spawn small-stack thread")
        .join()
        .expect("test body must not panic");
}

fn load_rule(rule_body: &str) -> Engine {
    load_source(&format!("spec chain\nrule r: {rule_body}\n"))
}

fn load_source(code: &str) -> Engine {
    let mut engine = Engine::new();
    engine
        .load([(SourceType::Volatile, code)])
        .expect("chain spec must load");
    engine
}

fn terms(operator: &str, first: &str, rest: &str, count: usize) -> String {
    let mut out = String::from(first);
    for _ in 1..count {
        out.push_str(operator);
        out.push_str(rest);
    }
    out
}

fn rule_value(engine: &Engine) -> String {
    get_rule_value(engine, "chain", "r").to_string()
}

#[test]
fn plus_chain_of_many_terms_plans_and_evaluates() {
    on_small_stack(|| {
        let engine = load_rule(&terms(" + ", "1", "1", CHAIN_TERMS));
        assert_eq!(rule_value(&engine), CHAIN_TERMS.to_string());
    });
}

#[test]
fn minus_chain_is_left_fold() {
    on_small_stack(|| {
        let body = format!("1 - {}", terms(" - ", "1", "1", CHAIN_TERMS - 1));
        let engine = load_rule(&body);
        let expected = 1 - (CHAIN_TERMS as i64 - 1);
        assert_eq!(rule_value(&engine), expected.to_string());
    });
}

#[test]
fn divide_chain_is_left_fold() {
    on_small_stack(|| {
        let body = format!("1000000{}", " / 1".repeat(CHAIN_TERMS - 1));
        let engine = load_rule(&body);
        assert_eq!(rule_value(&engine), "1000000");
    });
}

#[test]
fn multiply_chain_of_many_terms_plans_and_evaluates() {
    on_small_stack(|| {
        let engine = load_rule(&terms(" * ", "1", "1", CHAIN_TERMS));
        assert_eq!(rule_value(&engine), "1");
    });
}

#[test]
fn modulo_chain_is_left_fold() {
    on_small_stack(|| {
        let code = format!(
            "spec chain\ndata x: 3\ndata seven: 7\nrule r: x{}\n",
            " mod seven".repeat(CHAIN_TERMS - 1)
        );
        let engine = load_source(&code);
        assert_eq!(rule_value(&engine), "3");
    });
}

#[test]
fn mixed_precedence_chain_plans_and_evaluates() {
    on_small_stack(|| {
        let groups = CHAIN_TERMS / 4;
        let body = format!("1{}", " + 2 * 1 - 1 / 1".repeat(groups));
        let engine = load_rule(&body);
        assert_eq!(rule_value(&engine), (1 + groups).to_string());
    });
}

#[test]
fn and_chain_of_many_terms_plans_and_evaluates() {
    on_small_stack(|| {
        let engine = load_rule(&terms(" and ", "true", "true", CHAIN_TERMS));
        assert_eq!(rule_value(&engine), "true");
    });
}

#[test]
fn as_chain_of_many_steps_plans_and_evaluates() {
    on_small_stack(|| {
        let prelude = "spec chain\ndata money: measure\n  -> unit eur: 1.0\n  -> unit usd: 1.1\n";
        let chained = load_source(&format!(
            "{prelude}rule r: 1 eur{}\n",
            " as eur".repeat(CHAIN_TERMS)
        ));
        let single = load_source(&format!("{prelude}rule r: 1 eur\n"));
        assert_eq!(rule_value(&chained), rule_value(&single));
    });
}

#[test]
fn range_bound_chain_of_many_terms_plans_and_evaluates() {
    on_small_stack(|| {
        let lower = terms(" + ", "0", "0", CHAIN_TERMS);
        let engine = load_rule(&format!("5 in {lower}...10"));
        assert_eq!(rule_value(&engine), "true");
    });
}

#[test]
fn mixed_unit_chain_matches_parenthesized_left_fold() {
    let prelude = "spec chain\ndata money: measure\n  -> unit eur: 1.0\n  -> unit usd: 1.1\n";
    let chain = load_source(&format!("{prelude}rule r: 1 eur + 2 usd + 3 eur\n"));
    let folded = load_source(&format!("{prelude}rule r: ((1 eur + 2 usd) + 3 eur)\n"));
    assert_eq!(rule_value(&chain), rule_value(&folded));
}

#[test]
fn chain_type_error_location_spans_from_chain_start_to_failing_operand() {
    let code = "spec chain\nrule r: 1 + 1 + 1 + true\n";
    let mut engine = Engine::new();
    let errors = match engine.load([(SourceType::Volatile, code)]) {
        Ok(_) => panic!("boolean operand in number chain must fail to load"),
        Err(errors) => errors,
    };
    let source = errors
        .iter()
        .find_map(|error| error.source_location())
        .expect("type error must carry a source location");
    let chain_start = code.find("1 + 1").expect("chain start");
    let operand_end = code.find("true").expect("operand") + "true".len();
    assert_eq!(source.span.start, chain_start);
    assert_eq!(source.span.end, operand_end);
}

#[test]
fn trivial_rule_fits_small_stack() {
    on_small_stack(|| {
        let engine = load_rule("1");
        assert_eq!(rule_value(&engine), "1");
    });
}

#[test]
fn nesting_at_depth_limit_evaluates_on_small_stack() {
    on_small_stack(|| {
        let body = format!("{}1{}", "(".repeat(6), ")".repeat(6));
        let engine = load_rule(&body);
        assert_eq!(rule_value(&engine), "1");
    });
}

#[test]
fn not_nesting_at_depth_limit_evaluates_on_small_stack() {
    on_small_stack(|| {
        let body = format!("{}true{}", "not(".repeat(6), ")".repeat(6));
        let engine = load_rule(&body);
        assert_eq!(rule_value(&engine), "true");
    });
}

#[test]
fn parenthesis_nesting_beyond_limit_is_error_on_small_stack() {
    on_small_stack(|| {
        let depth = 20_000;
        let code = format!(
            "spec chain\nrule r: {}1{}\n",
            "(".repeat(depth),
            ")".repeat(depth)
        );
        let mut engine = Engine::new();
        assert!(
            engine
                .load([(SourceType::Volatile, code.as_str())])
                .is_err(),
            "nesting beyond max_expression_depth must return Error, not overflow"
        );
    });
}

#[test]
fn not_nesting_beyond_limit_is_error_on_small_stack() {
    on_small_stack(|| {
        let code = format!(
            "spec chain\nrule r: {}true{}\n",
            "not(".repeat(20_000),
            ")".repeat(20_000)
        );
        let mut engine = Engine::new();
        assert!(
            engine
                .load([(SourceType::Volatile, code.as_str())])
                .is_err(),
            "nesting beyond max_expression_depth must return Error, not overflow"
        );
    });
}
