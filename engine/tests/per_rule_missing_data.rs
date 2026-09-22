//! Per-rule `RuleResult.missing_data` from `Engine::run`.
//!
//! Asserts on Rust `Response` / `RuleResult` structures (not JSON).

use lemma::{DateTimeValue, Engine, ExplanationNode, RuleResult};
use std::collections::HashMap;
use std::sync::Arc;

fn load(engine: &mut Engine, code: &str) {
    engine
        .load([(lemma::SourceType::Volatile, code.to_string())])
        .expect("spec must load");
}

fn run(
    engine: &Engine,
    spec: &str,
    data: HashMap<String, String>,
    rules: Option<&[String]>,
    explain: bool,
) -> lemma::Response {
    let now = DateTimeValue::now();
    engine
        .run(None, spec, Some(&now), data, rules, explain)
        .expect("evaluation must succeed")
}

fn rule<'a>(response: &'a lemma::Response, name: &str) -> &'a RuleResult {
    response.results.get(name).unwrap_or_else(|| {
        panic!(
            "rule '{name}' missing from results: {:?}",
            response.results.keys().collect::<Vec<_>>()
        )
    })
}

fn missing<'a>(response: &'a lemma::Response, name: &str) -> &'a [String] {
    rule(response, name).missing_data()
}

fn show_has(engine: &Engine, spec: &str, key: &str) {
    let now = DateTimeValue::now();
    let show = engine.show(None, spec, Some(&now)).expect("show");
    assert!(
        show.data.contains_key(key),
        "show.data must contain {key:?}; keys={:?}",
        show.data.keys().collect::<Vec<_>>()
    );
}

// --- H. AND / Piecewise control release precision ---
//
// These cases pin the behavioral invariants tested by the deleted data_releases_* unit tests in
// execution_plan.rs and extend coverage with one new case (h5) that the old precomputed
// release table could not satisfy: a path released by two distinct control nodes, each blocking
// one route, where the old code over-reported the path as still missing.

/// Porting data_releases_and_does_not_release_path_still_reachable_outside:
/// When the AND short-circuits (flag=false), `expensive` is still needed for the
/// default body — it must remain in missing_data.
#[test]
fn h1_and_short_circuit_does_not_release_path_reachable_via_default_body() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data flag: boolean
data expensive: number
rule main: expensive
  unless flag and (expensive > 100) then 0
"#,
    );
    let mut data = HashMap::new();
    data.insert("flag".to_string(), "false".into());
    let response = run(&engine, "demo", data, None, false);
    let md = missing(&response, "main");
    assert!(
        md.contains(&"expensive".to_string()),
        "expensive is still needed via default body; must not be released when flag=false: {md:?}"
    );
}

/// Porting data_releases_nested_and_each_node_has_own_entry:
/// a and b and c — when a=false, both b and c must be released.
#[test]
fn h2_nested_and_first_false_releases_all_right_operands() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data a: boolean
data b: boolean
data c: boolean
rule main: a and b and c
"#,
    );
    let mut data = HashMap::new();
    data.insert("a".to_string(), "false".into());
    let response = run(&engine, "demo", data, None, false);
    let md = missing(&response, "main");
    assert!(
        !md.contains(&"b".to_string()),
        "b must be released when a=false (outer And short-circuits): {md:?}"
    );
    assert!(
        !md.contains(&"c".to_string()),
        "c must be released when a=false (outer And short-circuits): {md:?}"
    );
}

/// Nested AND: when a=true but b=false, c is released and a is not listed.
#[test]
fn h3_nested_and_middle_false_releases_rightmost_only() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data a: boolean
data b: boolean
data c: boolean
rule main: a and b and c
"#,
    );
    let mut data = HashMap::new();
    data.insert("a".to_string(), "true".into());
    data.insert("b".to_string(), "false".into());
    let response = run(&engine, "demo", data, None, false);
    let md = missing(&response, "main");
    assert!(
        !md.contains(&"c".to_string()),
        "c must be released when b=false (inner And short-circuits): {md:?}"
    );
    assert!(
        !md.contains(&"a".to_string()),
        "a is supplied, must not appear: {md:?}"
    );
    assert!(
        !md.contains(&"b".to_string()),
        "b is supplied, must not appear: {md:?}"
    );
}

/// Porting data_releases_piecewise_taken_multi_edge_releases_shared_body:
/// `shared` appears in both the default body and flag_a's arm body. When flag_b
/// arm is Taken, all routes to `shared` are dead — it must not appear in missing_data.
#[test]
fn h4_piecewise_taken_arm_releases_shared_body_all_routes_dead() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data shared: number
data flag_a: boolean
data flag_b: boolean
rule main: shared
  unless flag_a then shared
  unless flag_b then 0
"#,
    );
    // flag_b arm is taken: default body (shared) and flag_a body (shared) are both dead.
    let mut data = HashMap::new();
    data.insert("flag_b".to_string(), "true".into());
    let response = run(&engine, "demo", data, None, false);
    let md = missing(&response, "main");
    assert!(
        !md.contains(&"shared".to_string()),
        "all routes to shared are dead when flag_b arm is taken; must be released: {md:?}"
    );
    assert!(
        !md.contains(&"flag_a".to_string()),
        "flag_a's arm is dead when flag_b arm is taken: {md:?}"
    );
}

/// Porting data_releases_piecewise_not_taken_releases_arm_body /
/// data_releases_piecewise_default_wins_releases_unless_bodies:
/// When the unless condition is false (arm NOT taken, default wins), the arm body is not
/// needed and must not appear in missing_data.
#[test]
fn h5_piecewise_arm_not_taken_arm_body_absent_from_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data flag: boolean
data expensive: number
rule main: 1
  unless flag then expensive
"#,
    );
    let mut data = HashMap::new();
    data.insert("flag".to_string(), "false".into());
    let response = run(&engine, "demo", data, None, false);
    let md = missing(&response, "main");
    assert!(
        !md.contains(&"expensive".to_string()),
        "flag=false → default wins → arm body expensive must be released: {md:?}"
    );
}

/// New precision case — two distinct piecewise control nodes each blocking one route to
/// `expensive`.
///
/// Spec structure (two rules, because Lemma does not allow nested `unless` in expressions):
/// ```
/// rule guarded: expensive
///   unless flag_inner then 0
///
/// rule main: expensive
///   unless flag_outer then guarded
/// ```
///
/// The NormalForm DAG for `main` (after rule inlining) contains:
///   P_outer = Piecewise(flag_outer): default=expensive, arm=P_inner
///   P_inner = Piecewise(flag_inner): default=expensive, arm=0
///
/// Routes to `expensive`:
///   Route 1: P_outer default body  → dead when P_outer arm is Taken (flag_outer=true)
///   Route 2: P_inner default body  → dead when P_inner arm is Taken (flag_inner=true)
///
/// When flag_outer=true AND flag_inner=true, and `expensive` not provided:
///   - P_outer Taken: Route 1 dead. Result comes from P_inner.
///   - P_inner Taken: Route 2 dead. Result = 0.
///   - Every route to `expensive` is blocked → `expensive` must not appear in missing_data.
///
/// Why the precomputed release table (before Part 3) fails:
/// `fill_structural_needed` walks all DAG nodes, so structural_needed["main"] includes
/// `expensive` via both routes. Then:
/// - P_outer arm-taken analysis: slots["expensive"] = {default_leaf, P_inner} — P_inner
///   also carries expensive, so dead_children={default_leaf} does not cover all slots →
///   not released.
/// - P_inner arm-taken analysis: bypass_leaves(main_root, skip=P_inner) traverses P_outer
///   and collects `expensive` from P_outer's default body → expensive is in bypass →
///   released_for_dead_children skips it.
///
/// Result: released={}, expensive remains in structural_needed−bound → over-reported.
///
/// The decision-aware reachability walk (Part 3) prunes from the actual root with actual
/// control decisions simultaneously and correctly finds `expensive` unreachable.
///
/// This test pins the precision case that the walk must satisfy.
#[test]
fn h6_two_control_nodes_each_blocking_one_route_expensive_released() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data flag_outer: boolean
data flag_inner: boolean
data expensive: number

rule guarded: expensive
  unless flag_inner then 0

rule main: expensive
  unless flag_outer then guarded
"#,
    );
    let mut data = HashMap::new();
    data.insert("flag_outer".to_string(), "true".into());
    data.insert("flag_inner".to_string(), "true".into());
    let response = run(&engine, "demo", data, None, false);
    let md = missing(&response, "main");
    assert!(
        !md.contains(&"expensive".to_string()),
        "both routes to expensive are blocked by two independent piecewise decisions; \
         expensive must be released — not over-reported as missing: {md:?}"
    );
}

// --- A. Complete vs incomplete ---

#[test]
fn a1_full_overlay_all_missing_data_empty() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data a: number
data b: number
rule main: a + b
"#,
    );
    let mut data = HashMap::new();
    data.insert("a".to_string(), "1".into());
    data.insert("b".to_string(), "2".into());
    let response = run(&engine, "demo", data, None, false);
    let main = rule(&response, "main");
    assert!(
        !main.vetoed,
        "must not missing-data veto: {:?}",
        main.veto_reason
    );
    assert!(
        main.missing_data().is_empty(),
        "expected empty missing_data, got {:?}",
        main.missing_data()
    );
}

#[test]
fn a2_no_inputs_lists_all_operands_in_declaration_order() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data a: number
data b: number
data c: number
rule main: a + b + c
"#,
    );
    let response = run(&engine, "demo", HashMap::new(), None, false);
    assert_eq!(missing(&response, "main"), &["a", "b", "c"]);
}

#[test]
fn a3_partial_overlay_omits_supplied_keys() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data a: number
data b: number
data c: number
rule main: a + b + c
"#,
    );
    let mut data = HashMap::new();
    data.insert("a".to_string(), "1".into());
    let response = run(&engine, "demo", data, None, false);
    assert_eq!(missing(&response, "main"), &["b", "c"]);
}

#[test]
fn a4_prefill_absent_suggest_present_in_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data prefilled: 10
data suggested: number -> suggest 5
data required: number
rule main: prefilled + suggested + required
"#,
    );
    let response = run(&engine, "demo", HashMap::new(), None, false);
    let md = missing(&response, "main");
    assert!(
        !md.contains(&"prefilled".to_string()),
        "prefill must not be missing: {md:?}"
    );
    assert!(
        md.contains(&"suggested".to_string()),
        "suggest unbound must be missing: {md:?}"
    );
    assert!(
        md.contains(&"required".to_string()),
        "required unbound must be missing: {md:?}"
    );
}

// --- B. Per-rule scoping ---

#[test]
fn b1_two_rules_do_not_cross_leak_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data x: number
data y: number
rule r1: x
rule r2: y
"#,
    );
    let response = run(
        &engine,
        "demo",
        HashMap::new(),
        Some(&["r1".to_string(), "r2".to_string()]),
        false,
    );
    assert_eq!(missing(&response, "r1"), &["x"]);
    assert_eq!(missing(&response, "r2"), &["y"]);
}

#[test]
fn b2_request_only_r1_excludes_y_from_results() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data x: number
data y: number
rule r1: x
rule r2: y
"#,
    );
    let response = run(
        &engine,
        "demo",
        HashMap::new(),
        Some(&["r1".to_string()]),
        false,
    );
    assert!(response.results.contains_key("r1"), "r1 must be present");
    assert!(
        !response.results.contains_key("r2"),
        "r2 must not be in results when unrequested"
    );
    assert_eq!(missing(&response, "r1"), &["x"]);
}

#[test]
fn b3_shared_unbound_input_listed_on_both_rules() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data s: number
data t: number
rule r1: s + 1
rule r2: s + t
"#,
    );
    let response = run(
        &engine,
        "demo",
        HashMap::new(),
        Some(&["r1".to_string(), "r2".to_string()]),
        false,
    );
    assert!(
        missing(&response, "r1").contains(&"s".to_string()),
        "r1 must list shared s: {:?}",
        missing(&response, "r1")
    );
    assert!(
        missing(&response, "r2").contains(&"s".to_string()),
        "r2 must list shared s (memo replay): {:?}",
        missing(&response, "r2")
    );
    assert!(
        missing(&response, "r2").contains(&"t".to_string()),
        "r2 must list t: {:?}",
        missing(&response, "r2")
    );

    let mut data = HashMap::new();
    data.insert("s".to_string(), "3".into());
    let response = run(
        &engine,
        "demo",
        data,
        Some(&["r1".to_string(), "r2".to_string()]),
        false,
    );
    assert!(
        !missing(&response, "r1").contains(&"s".to_string()),
        "r1 must not list supplied s: {:?}",
        missing(&response, "r1")
    );
    assert!(
        !missing(&response, "r2").contains(&"s".to_string()),
        "r2 must not list supplied s: {:?}",
        missing(&response, "r2")
    );
    assert_eq!(missing(&response, "r2"), &["t"]);
}

#[test]
fn b4_only_incomplete_sibling_has_non_empty_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data ready: 1
data need: number
rule ok: ready
rule incomplete: need
"#,
    );
    let response = run(&engine, "demo", HashMap::new(), None, false);
    assert!(
        missing(&response, "ok").is_empty(),
        "ok must have empty missing_data: {:?}",
        missing(&response, "ok")
    );
    assert_eq!(missing(&response, "incomplete"), &["need"]);
}

// --- C. Unless / piecewise ---

const CHOOSER: &str = r#"
spec chooser
data mode: text -> options "simple" "complex"
data simple_input: number
data complex_input_a: number
data complex_input_b: number
rule result: veto "pick mode"
  unless mode is "simple" then simple_input
  unless mode is "complex" then complex_input_a + complex_input_b
"#;

#[test]
fn c1_mode_unbound_lists_mode_and_live_arm_inputs() {
    let mut engine = Engine::new();
    load(&mut engine, CHOOSER);
    let response = run(
        &engine,
        "chooser",
        HashMap::new(),
        Some(&["result".to_string()]),
        false,
    );
    let md = missing(&response, "result");
    assert!(md.contains(&"mode".to_string()), "mode: {md:?}");
    assert!(
        md.contains(&"simple_input".to_string()),
        "default/simple live: {md:?}"
    );
    assert!(
        md.contains(&"complex_input_a".to_string()),
        "complex arm live while mode unbound: {md:?}"
    );
    assert!(
        md.contains(&"complex_input_b".to_string()),
        "complex arm live while mode unbound: {md:?}"
    );
}

#[test]
fn c2_mode_simple_prunes_complex_inputs() {
    let mut engine = Engine::new();
    load(&mut engine, CHOOSER);
    let mut data = HashMap::new();
    data.insert("mode".to_string(), "simple".into());
    let response = run(
        &engine,
        "chooser",
        data,
        Some(&["result".to_string()]),
        false,
    );
    let md = missing(&response, "result");
    assert!(
        !md.contains(&"complex_input_a".to_string()),
        "complex pruned: {md:?}"
    );
    assert!(
        !md.contains(&"complex_input_b".to_string()),
        "complex pruned: {md:?}"
    );
    assert!(
        md.contains(&"simple_input".to_string()),
        "simple_input still missing: {md:?}"
    );
    assert!(!md.contains(&"mode".to_string()), "mode supplied: {md:?}");
}

#[test]
fn c3_mode_complex_prunes_simple_input() {
    let mut engine = Engine::new();
    load(&mut engine, CHOOSER);
    let mut data = HashMap::new();
    data.insert("mode".to_string(), "complex".into());
    let response = run(
        &engine,
        "chooser",
        data,
        Some(&["result".to_string()]),
        false,
    );
    let md = missing(&response, "result");
    assert!(
        !md.contains(&"simple_input".to_string()),
        "simple pruned: {md:?}"
    );
    assert!(md.contains(&"complex_input_a".to_string()), "{md:?}");
    assert!(md.contains(&"complex_input_b".to_string()), "{md:?}");
}

#[test]
fn c4_unless_wins_releases_default_arm_data() {
    // Default arm uses `expensive`; unless arm uses `threshold`. When unless wins,
    // on_arm_taken releases default-only paths — expensive absent from missing_data.
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data expensive: number
data threshold: number
rule main: expensive * 2
  unless now > 1970-01-01 then threshold + 1
"#,
    );
    let mut data = HashMap::new();
    data.insert("threshold".to_string(), "4".into());
    let response = run(&engine, "demo", data, Some(&["main".to_string()]), false);
    let md = missing(&response, "main");
    assert!(
        !md.contains(&"expensive".to_string()),
        "unless win must release default-only expensive: {md:?}"
    );
    assert!(
        !md.contains(&"threshold".to_string()),
        "threshold supplied: {md:?}"
    );
    assert!(
        md.is_empty(),
        "no unbound live inputs when unless wins with threshold: {md:?}"
    );
}

#[test]
fn c5_statically_dead_unless_arm_data_absent() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data flag: boolean
rule main: 1
  unless flag and (1 > 2) then 2
"#,
    );
    let response = run(&engine, "demo", HashMap::new(), None, false);
    let md = missing(&response, "main");
    assert!(
        !md.contains(&"flag".to_string()),
        "statically dead unless must not require flag: {md:?}"
    );
    assert!(
        !response.results["main"].vetoed,
        "statically collapsed default must evaluate: {:?}",
        response.results["main"]
    );
}

// --- D. And / remaining-live ---

#[test]
fn d1_false_and_does_not_list_right_operand() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data flag: boolean
data expensive: number
rule main: flag and expensive > 0
"#,
    );
    let mut data = HashMap::new();
    data.insert("flag".to_string(), "false".into());
    let response = run(&engine, "demo", data, None, false);
    let md = missing(&response, "main");
    assert!(
        !md.contains(&"expensive".to_string()),
        "false and must not need expensive: {md:?}"
    );
}

#[test]
fn d2_left_missing_still_lists_remaining_operands() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data a: number
data b: number
data c: number
rule main: a + b + c
"#,
    );
    let response = run(&engine, "demo", HashMap::new(), None, false);
    let md = missing(&response, "main");
    assert!(md.contains(&"a".to_string()), "{md:?}");
    assert!(md.contains(&"b".to_string()), "{md:?}");
    assert!(md.contains(&"c".to_string()), "{md:?}");
}

/// Product left MissingData must still walk the loading embed so unless last-match
/// dead edges record. `is_smoker=true` (last unless) prunes former/years.
#[test]
fn d3_product_missing_left_still_prunes_unless_under_right_embed() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data missing_factor: number
data is_smoker: boolean
data is_former_smoker: boolean
data years_since_quit: number
rule loading: 1
  unless is_former_smoker then years_since_quit + 1
  unless is_smoker then 2
rule main: missing_factor * loading
"#,
    );
    let mut data = HashMap::new();
    data.insert("is_smoker".to_string(), "true".into());
    let response = run(&engine, "demo", data, Some(&["main".to_string()]), false);
    let md = missing(&response, "main");
    assert!(
        md.contains(&"missing_factor".to_string()),
        "live unbound product operand must remain: {md:?}"
    );
    assert!(
        !md.contains(&"is_former_smoker".to_string()),
        "former arm must be dead when last unless is_smoker=true: {md:?}"
    );
    assert!(
        !md.contains(&"years_since_quit".to_string()),
        "years_since_quit must be dead when last unless is_smoker=true: {md:?}"
    );
}

/// And Propagate (left MissingData): value walk and missing_data both leave the
/// right conjunct unreachable (matches `evaluate_and`). The `loading` arms are
/// absent because the right conjunct is never reached, whatever `is_smoker` is.
#[test]
fn d4_and_propagate_leaves_right_conjunct_unreachable() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data missing_flag: boolean
data is_smoker: boolean
data is_former_smoker: boolean
data years_since_quit: number
rule loading: 1
  unless is_former_smoker then years_since_quit + 1
  unless is_smoker then 2
rule main: missing_flag and (loading > 0)
"#,
    );
    let mut data = HashMap::new();
    data.insert("is_smoker".to_string(), "true".into());
    let response = run(&engine, "demo", data, Some(&["main".to_string()]), false);
    let md = missing(&response, "main");
    assert!(
        md.contains(&"missing_flag".to_string()),
        "propagating left MissingData must remain listed: {md:?}"
    );
    assert!(
        !md.contains(&"is_former_smoker".to_string()),
        "right conjunct is unreachable after left MissingData: {md:?}"
    );
    assert!(
        !md.contains(&"years_since_quit".to_string()),
        "right conjunct is unreachable after left MissingData: {md:?}"
    );
}

/// Definitive Computation veto must not continue sibling walk for missing_data —
/// settled answer ⇒ empty missing_data even if later operands stay unbound.
#[test]
fn d5_product_computation_veto_empties_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data denom: number
data is_smoker: boolean
data is_former_smoker: boolean
data years_since_quit: number
rule loading: 1
  unless is_former_smoker then years_since_quit + 1
  unless is_smoker then 2
rule main: (1 / denom) * loading
"#,
    );
    let mut data = HashMap::new();
    data.insert("denom".to_string(), "0".into());
    let plain = run(
        &engine,
        "demo",
        data.clone(),
        Some(&["main".to_string()]),
        false,
    );
    let explained = run(&engine, "demo", data, Some(&["main".to_string()]), true);
    let main = rule(&plain, "main");
    assert!(main.vetoed, "div by zero must veto");
    assert!(
        main.missing_data().is_empty(),
        "settled Computation must report empty missing_data: {:?}",
        main.missing_data()
    );
    assert_eq!(
        missing(&plain, "main"),
        missing(&explained, "main"),
        "explain parity for settled empty missing_data"
    );
}

/// UserDefined early factor likewise settles — no leftover loading keys.
#[test]
fn d6_product_user_defined_veto_empties_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data age: number
data is_smoker: boolean
data is_former_smoker: boolean
data years_since_quit: number
rule base: veto "no rate"
  unless age < 70 then 10
rule loading: 1
  unless is_former_smoker then years_since_quit + 1
  unless is_smoker then 2
rule main: base * loading
"#,
    );
    let mut data = HashMap::new();
    data.insert("age".to_string(), "80".into());
    let response = run(&engine, "demo", data, Some(&["main".to_string()]), false);
    let main = rule(&response, "main");
    assert!(main.vetoed);
    assert!(
        main.veto_reason
            .as_deref()
            .is_some_and(|r| r.contains("no rate")),
        "{:?}",
        main.veto_reason
    );
    assert!(
        main.missing_data().is_empty(),
        "settled UserDefined must report empty missing_data: {:?}",
        main.missing_data()
    );
}

/// Later definitive veto wins over earlier MissingData during continue-eval
/// (life_plus-shaped: unbound sum factor + age UserDefined via base).
#[test]
fn d7_later_definitive_veto_wins_over_earlier_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data missing_factor: number
data age: number
rule base: veto "no rate"
  unless age < 70 then 10
rule main: missing_factor * base
"#,
    );
    let mut data = HashMap::new();
    data.insert("age".to_string(), "80".into());
    let plain = run(
        &engine,
        "demo",
        data.clone(),
        Some(&["main".to_string()]),
        false,
    );
    let explained = run(&engine, "demo", data, Some(&["main".to_string()]), true);
    let main = rule(&plain, "main");
    assert!(main.vetoed);
    assert!(
        main.veto_reason
            .as_deref()
            .is_some_and(|r| r.contains("no rate")),
        "definitive base veto must win over missing_factor MissingData: {:?}",
        main.veto_reason
    );
    assert!(
        !main.is_missing_data(),
        "rule must be settled, not a MissingData veto"
    );
    assert!(
        main.missing_data().is_empty(),
        "settled definitive must clear missing_data: {:?}",
        main.missing_data()
    );
    assert_eq!(
        missing(&plain, "main"),
        missing(&explained, "main"),
        "explain parity"
    );
}

/// And left MissingData + right definitive: MissingData wins intake (`false` can still answer).
/// The right conjunct is never visited (parity with d4), so `age` is not listed.
#[test]
fn d8_and_missing_left_is_missing_data_despite_later_definitive_veto() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data missing_flag: boolean
data age: number
rule base: veto "no rate"
  unless age < 70 then 10
rule main: missing_flag and (base > 0)
"#,
    );
    let mut data = HashMap::new();
    data.insert("age".to_string(), "80".into());
    let plain = run(
        &engine,
        "demo",
        data.clone(),
        Some(&["main".to_string()]),
        false,
    );
    let explained = run(&engine, "demo", data, Some(&["main".to_string()]), true);
    let main = rule(&plain, "main");
    assert!(main.vetoed);
    assert!(
        main.is_missing_data(),
        "unbound left And must be MissingData even when right settles veto: {:?}",
        main.veto_reason
    );
    assert!(
        main.missing_data().contains(&"missing_flag".to_string()),
        "missing_flag must remain outcome-relevant: {:?}",
        main.missing_data()
    );
    assert_eq!(
        main.veto_reason.as_deref(),
        Some("Missing data: missing_flag"),
        "And result must stay MissingData-shaped, not settle on no rate alone: {:?}",
        main.veto_reason
    );
    assert_eq!(
        missing(&plain, "main"),
        missing(&explained, "main"),
        "explain parity"
    );
}

/// `false and veto` answers false: unbound left is outcome-relevant because a false binding
/// does not need the right conjunct.
#[test]
fn d8c_false_and_later_definitive_veto_answers_false() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data missing_flag: boolean
data age: number
rule base: veto "no rate"
  unless age < 70 then 10
rule main: missing_flag and (base > 0)
"#,
    );
    let mut data = HashMap::new();
    data.insert("missing_flag".to_string(), "false".into());
    data.insert("age".to_string(), "80".into());
    let response = run(&engine, "demo", data, Some(&["main".to_string()]), false);
    let main = rule(&response, "main");
    assert!(
        !main.vetoed,
        "false and … must answer false, not inherit right veto: {:?}",
        main.veto_reason
    );
    assert!(!main.is_missing_data(), "false And is settled");
    assert!(
        main.missing_data().is_empty(),
        "settled false must clear missing_data: {:?}",
        main.missing_data()
    );
    assert_eq!(
        main.result.as_ref().and_then(|v| v.boolean),
        Some(false),
        "false and veto must be boolean false, got {:?}",
        main.result()
    );
}

/// Left definitive veto settles And; right conjunct is not visited (value or explain).
#[test]
fn d8b_and_left_definitive_veto_does_not_explore_right() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
rule x: veto "left veto"
rule y: veto "right veto"
rule z: x and y
"#,
    );
    let plain = run(
        &engine,
        "demo",
        HashMap::new(),
        Some(&["z".to_string()]),
        false,
    );
    let explained = run(
        &engine,
        "demo",
        HashMap::new(),
        Some(&["z".to_string()]),
        true,
    );
    let z = rule(&plain, "z");
    assert!(z.vetoed, "z must veto when left conjunct vetoes");
    assert!(
        !z.is_missing_data(),
        "definitive And must not be MissingData"
    );
    assert!(
        z.missing_data().is_empty(),
        "definitive And must clear missing_data: {:?}",
        z.missing_data()
    );
    assert_eq!(
        z.veto_reason.as_deref(),
        Some("left veto"),
        "And settles on left definitive veto: {:?}",
        z.veto_reason
    );
    let z_explained = rule(&explained, "z");
    assert_eq!(
        z_explained.veto_reason.as_deref(),
        Some("left veto"),
        "explain must agree on left veto"
    );
    let explanation = z_explained.explanation.as_ref().expect("explain on");
    let names = explanation_rule_names(&explanation.children);
    assert!(
        names.iter().any(|n| n == "x"),
        "left conjunct x must be an explanation child: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n == "y"),
        "right conjunct y must not run after left Propagate: {names:?}"
    );
}

fn explanation_rule_names(nodes: &[ExplanationNode]) -> Vec<String> {
    let mut names = Vec::new();
    fn walk(node: &ExplanationNode, names: &mut Vec<String>) {
        match node {
            ExplanationNode::Rule { name, children, .. } => {
                names.push(name.rule.clone());
                for child in children {
                    walk(child, names);
                }
            }
            ExplanationNode::Compose { operands, .. }
            | ExplanationNode::Conversion { operands, .. } => {
                for child in operands {
                    walk(child, names);
                }
            }
            ExplanationNode::Data { .. }
            | ExplanationNode::DataUnused { .. }
            | ExplanationNode::Veto { .. } => {}
        }
    }
    for node in nodes {
        walk(node, &mut names);
    }
    names
}

/// Comparison left MissingData must still walk right embed so unless last-match prunes
/// (parity with d3 product operand order).
#[test]
fn d9_comparison_missing_left_still_prunes_unless_under_right_embed() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data missing_threshold: number
data is_smoker: boolean
data is_former_smoker: boolean
data years_since_quit: number
rule loading: 1
  unless is_former_smoker then years_since_quit + 1
  unless is_smoker then 2
rule main: missing_threshold > loading
"#,
    );
    let mut data = HashMap::new();
    data.insert("is_smoker".to_string(), "true".into());
    let response = run(&engine, "demo", data, Some(&["main".to_string()]), false);
    let md = missing(&response, "main");
    assert!(
        md.contains(&"missing_threshold".to_string()),
        "live unbound comparison left must remain: {md:?}"
    );
    assert!(
        !md.contains(&"is_former_smoker".to_string()),
        "former arm must be dead when last unless is_smoker=true: {md:?}"
    );
    assert!(
        !md.contains(&"years_since_quit".to_string()),
        "years_since_quit must be dead when last unless is_smoker=true: {md:?}"
    );
}

/// Comparison left MissingData + right definitive: definitive must win (parity with d7).
#[test]
fn d10_comparison_later_definitive_veto_wins_over_earlier_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data missing_threshold: number
data age: number
rule base: veto "no rate"
  unless age < 70 then 10
rule main: missing_threshold > base
"#,
    );
    let mut data = HashMap::new();
    data.insert("age".to_string(), "80".into());
    let plain = run(
        &engine,
        "demo",
        data.clone(),
        Some(&["main".to_string()]),
        false,
    );
    let explained = run(&engine, "demo", data, Some(&["main".to_string()]), true);
    let main = rule(&plain, "main");
    assert!(main.vetoed);
    assert!(
        main.veto_reason
            .as_deref()
            .is_some_and(|r| r.contains("no rate")),
        "definitive base veto must win over missing_threshold MissingData: {:?}",
        main.veto_reason
    );
    assert!(
        !main.is_missing_data(),
        "rule must be settled, not a MissingData veto"
    );
    assert!(
        main.missing_data().is_empty(),
        "settled definitive must clear missing_data: {:?}",
        main.missing_data()
    );
    assert_eq!(
        missing(&plain, "main"),
        missing(&explained, "main"),
        "explain parity"
    );
}

/// RangeLiteral left MissingData + right definitive: definitive must win (parity with d10).
#[test]
fn d11_range_literal_later_definitive_veto_wins_over_earlier_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data missing_threshold: number
data age: number
rule base: veto "no rate"
  unless age < 70 then 10
rule main: missing_threshold...base
"#,
    );
    let mut data = HashMap::new();
    data.insert("age".to_string(), "80".into());
    let plain = run(
        &engine,
        "demo",
        data.clone(),
        Some(&["main".to_string()]),
        false,
    );
    let explained = run(&engine, "demo", data, Some(&["main".to_string()]), true);
    let main = rule(&plain, "main");
    assert!(main.vetoed);
    assert!(
        main.veto_reason
            .as_deref()
            .is_some_and(|r| r.contains("no rate")),
        "definitive base veto must win over missing_threshold MissingData: {:?}",
        main.veto_reason
    );
    assert!(
        !main.is_missing_data(),
        "rule must be settled, not a MissingData veto"
    );
    assert!(
        main.missing_data().is_empty(),
        "settled definitive must clear missing_data: {:?}",
        main.missing_data()
    );
    assert_eq!(
        missing(&plain, "main"),
        missing(&explained, "main"),
        "explain parity"
    );
}

/// RangeContainment left MissingData + right definitive: definitive must win (parity with d10).
#[test]
fn d12_range_containment_later_definitive_veto_wins_over_earlier_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data missing_threshold: number
data age: number
rule base: veto "no rate"
  unless age < 70 then 10
rule main: missing_threshold in 0...base
"#,
    );
    let mut data = HashMap::new();
    data.insert("age".to_string(), "80".into());
    let plain = run(
        &engine,
        "demo",
        data.clone(),
        Some(&["main".to_string()]),
        false,
    );
    let explained = run(&engine, "demo", data, Some(&["main".to_string()]), true);
    let main = rule(&plain, "main");
    assert!(main.vetoed);
    assert!(
        main.veto_reason
            .as_deref()
            .is_some_and(|r| r.contains("no rate")),
        "definitive base veto must win over missing_threshold MissingData: {:?}",
        main.veto_reason
    );
    assert!(
        !main.is_missing_data(),
        "rule must be settled, not a MissingData veto"
    );
    assert!(
        main.missing_data().is_empty(),
        "settled definitive must clear missing_data: {:?}",
        main.missing_data()
    );
    assert_eq!(
        missing(&plain, "main"),
        missing(&explained, "main"),
        "explain parity"
    );
}

/// Settled comparison left + right definitive veto: right veto wins via evaluate_binary compose.
#[test]
fn d13_comparison_settled_left_right_definitive_veto() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data age: number
rule base: veto "no rate"
  unless age < 70 then 10
rule main: 100 > base
"#,
    );
    let mut data = HashMap::new();
    data.insert("age".to_string(), "80".into());
    let plain = run(
        &engine,
        "demo",
        data.clone(),
        Some(&["main".to_string()]),
        false,
    );
    let explained = run(&engine, "demo", data, Some(&["main".to_string()]), true);
    let main = rule(&plain, "main");
    assert!(main.vetoed);
    assert!(
        main.veto_reason
            .as_deref()
            .is_some_and(|r| r.contains("no rate")),
        "right embed veto must win when left is settled: {:?}",
        main.veto_reason
    );
    assert!(
        !main.is_missing_data(),
        "rule must be settled, not a MissingData veto"
    );
    assert!(
        main.missing_data().is_empty(),
        "settled definitive must clear missing_data: {:?}",
        main.missing_data()
    );
    assert_eq!(
        missing(&plain, "main"),
        missing(&explained, "main"),
        "explain parity"
    );
}

/// Settled RangeLiteral left + right definitive veto (parity with d13).
#[test]
fn d14_range_literal_settled_left_right_definitive_veto() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data age: number
rule base: veto "no rate"
  unless age < 70 then 10
rule main: 0...base
"#,
    );
    let mut data = HashMap::new();
    data.insert("age".to_string(), "80".into());
    let plain = run(
        &engine,
        "demo",
        data.clone(),
        Some(&["main".to_string()]),
        false,
    );
    let explained = run(&engine, "demo", data, Some(&["main".to_string()]), true);
    let main = rule(&plain, "main");
    assert!(main.vetoed);
    assert!(
        main.veto_reason
            .as_deref()
            .is_some_and(|r| r.contains("no rate")),
        "right embed veto must win when left is settled: {:?}",
        main.veto_reason
    );
    assert!(
        !main.is_missing_data(),
        "rule must be settled, not a MissingData veto"
    );
    assert!(
        main.missing_data().is_empty(),
        "settled definitive must clear missing_data: {:?}",
        main.missing_data()
    );
    assert_eq!(
        missing(&plain, "main"),
        missing(&explained, "main"),
        "explain parity"
    );
}

/// Settled RangeContainment value + right definitive veto (parity with d13).
#[test]
fn d15_range_containment_settled_left_right_definitive_veto() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data age: number
rule base: veto "no rate"
  unless age < 70 then 10
rule main: 50 in 0...base
"#,
    );
    let mut data = HashMap::new();
    data.insert("age".to_string(), "80".into());
    let plain = run(
        &engine,
        "demo",
        data.clone(),
        Some(&["main".to_string()]),
        false,
    );
    let explained = run(&engine, "demo", data, Some(&["main".to_string()]), true);
    let main = rule(&plain, "main");
    assert!(main.vetoed);
    assert!(
        main.veto_reason
            .as_deref()
            .is_some_and(|r| r.contains("no rate")),
        "right embed veto must win when left is settled: {:?}",
        main.veto_reason
    );
    assert!(
        !main.is_missing_data(),
        "rule must be settled, not a MissingData veto"
    );
    assert!(
        main.missing_data().is_empty(),
        "settled definitive must clear missing_data: {:?}",
        main.missing_data()
    );
    assert_eq!(
        missing(&plain, "main"),
        missing(&explained, "main"),
        "explain parity"
    );
}

// --- E. Embeds / references ---

#[test]
fn e1_rule_ref_surfaces_inner_unbound_on_outer_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec inner
data slot: number
rule half: slot / 2

spec outer
uses i: inner
rule r: i.half + 1
"#,
    );
    let response = run(&engine, "outer", HashMap::new(), None, false);
    let md = missing(&response, "r");
    assert!(
        md.iter()
            .any(|k| k == "i.slot" || k.ends_with(".slot") || k == "slot"),
        "outer must list inner unbound slot via embed: {md:?}"
    );
}

#[test]
fn e2_with_rule_ref_binding_omitted_from_missing_data() {
    let code = r#"
spec bag
data weight: number
data item_cost: number
rule total: weight * item_cost

spec calc
uses bag
  -> with item_cost: item_cost
data type_of_nut: text -> options "peanut" "cashew"
rule item_cost: 1
  unless type_of_nut is "cashew" then 2
rule total: bag.total
"#;
    let mut engine = Engine::new();
    engine
        .load([(
            lemma::SourceType::Path(Arc::new(std::path::PathBuf::from("with_bind.lemma"))),
            code.to_string(),
        )])
        .expect("load");
    let response = run(
        &engine,
        "calc",
        HashMap::new(),
        Some(&["total".to_string()]),
        false,
    );
    let md = missing(&response, "total");
    assert!(
        !md.contains(&"bag.item_cost".to_string()),
        "with-bound rule ref must not appear: {md:?}"
    );
}

#[test]
fn e3_nested_input_key_matches_show_data_key() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec inner
data slot: number
rule half: slot / 2

spec outer
uses i: inner
rule r: i.half
"#,
    );
    let response = run(&engine, "outer", HashMap::new(), None, false);
    let md = missing(&response, "r");
    assert!(!md.is_empty(), "expected nested missing key: {md:?}");
    for key in md {
        show_has(&engine, "outer", key);
    }
}

// --- F. Explain parity ---

#[test]
fn f1_explain_true_false_same_missing_data_and_outcomes() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data a: number
data b: number
rule main: a + b
"#,
    );
    let mut data = HashMap::new();
    data.insert("a".to_string(), "1".into());
    let plain = run(&engine, "demo", data.clone(), None, false);
    let explained = run(&engine, "demo", data, None, true);
    assert_eq!(
        missing(&plain, "main"),
        missing(&explained, "main"),
        "missing_data must match"
    );
    let p = rule(&plain, "main");
    let e = rule(&explained, "main");
    assert_eq!(p.vetoed, e.vetoed);
    assert_eq!(p.result(), e.result());
    assert_eq!(p.veto_reason, e.veto_reason);
}

#[test]
fn f2_fully_supplied_with_explain_missing_data_empty() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data a: number
rule main: a
"#,
    );
    let mut data = HashMap::new();
    data.insert("a".to_string(), "9".into());
    let response = run(&engine, "demo", data, None, true);
    assert!(
        missing(&response, "main").is_empty(),
        "{:?}",
        missing(&response, "main")
    );
}

// --- G. Structure / show keys / order ---

#[test]
fn g2_every_missing_data_key_exists_in_show() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data a: number
data b: number
rule main: a + b
"#,
    );
    let response = run(&engine, "demo", HashMap::new(), None, false);
    let md = missing(&response, "main");
    assert_eq!(md, &["a", "b"]);
    for key in md {
        show_has(&engine, "demo", key);
    }
}

/// Reference-bound chain: local `code` inherits type from dependency `prev.code`,
/// dependency slot is bound via `with prev.code: code`, and rule references `prev.name`
/// which needs `prev.code`. The ultimate promptable target is `code`, not `prev.code`.
///
/// Bug: `show.data` is empty while `missing_data` contains `code`.
/// Both must agree: every key in `missing_data` must exist in `show.data`.
#[test]
fn g2b_reference_bound_chain_missing_data_key_exists_in_show() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec prev
data code: text -> options "AA" "BB"
rule name: veto "no code"
  unless code is "AA" then "Alpha"
  unless code is "BB" then "Beta"

spec current
uses prev
  -> with code: code
data code: prev.code -> option "SS"
rule name: prev.name
"#,
    );
    let response = run(
        &engine,
        "current",
        HashMap::new(),
        Some(&["name".to_string()]),
        false,
    );
    let md = missing(&response, "name");

    let now = DateTimeValue::now();
    let show = engine.show(None, "current", Some(&now)).expect("show");
    let show_keys: Vec<_> = show.data.keys().collect();

    assert!(
        md.contains(&"code".to_string()),
        "promptable input 'code' must appear in missing_data: {md:?}"
    );
    assert!(
        !md.contains(&"prev.code".to_string()),
        "internal reference 'prev.code' must NOT appear in missing_data: {md:?}"
    );
    assert!(
        show.data.contains_key("code"),
        "BUG: show.data must contain 'code' (the ultimate promptable target)\n\
         missing_data={md:?}\n\
         show.data.keys={show_keys:?}\n\
         These must agree: every missing_data key must exist in show.data"
    );

    let name = rule(&response, "name");
    assert!(name.vetoed, "unbound code must veto name");
    let reason = name.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.contains("Missing data: code"),
        "veto must name promptable input key 'code', not internal reference path; got: {reason}"
    );
    assert!(
        !reason.contains("prev"),
        "veto must not expose internal reference path; got: {reason}"
    );
}

fn assert_attach_invariant(response: &lemma::Response) {
    use lemma::VetoType;
    for result in response.results.values() {
        if !result.is_missing_data() {
            continue;
        }
        assert!(
            !result.missing_data().is_empty(),
            "MissingData rule '{}' must not attach empty missing_data",
            result.rule.name
        );
        let VetoType::MissingData { data, .. } = result
            .veto_detail
            .as_ref()
            .expect("BUG: is_missing_data without MissingData veto_detail")
        else {
            panic!("BUG: is_missing_data without MissingData veto_detail");
        };
        let key = data.input_key();
        assert!(
            result.missing_data().iter().any(|listed| listed == &key),
            "veto path {key} must appear in missing_data {:?} for rule '{}'",
            result.missing_data(),
            result.rule.name
        );
    }
}

/// Continue-eval paths that still settle MissingData must satisfy attach: veto path ∈ list.
#[test]
fn g4_attach_invariant_holds_after_continue_eval() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data missing_threshold: number
data is_smoker: boolean
data is_former_smoker: boolean
data years_since_quit: number
rule loading: 1
  unless is_former_smoker then years_since_quit + 1
  unless is_smoker then 2
rule main: missing_threshold > loading
"#,
    );
    let mut data = HashMap::new();
    data.insert("is_smoker".to_string(), "true".into());
    let comparison = run(&engine, "demo", data, Some(&["main".to_string()]), false);
    assert_attach_invariant(&comparison);
    assert!(
        missing(&comparison, "main").contains(&"missing_threshold".to_string()),
        "comparison continue-eval must keep live unbound left: {:?}",
        missing(&comparison, "main")
    );

    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data missing_flag: boolean
data is_smoker: boolean
data is_former_smoker: boolean
data years_since_quit: number
rule loading: 1
  unless is_former_smoker then years_since_quit + 1
  unless is_smoker then 2
rule main: missing_flag and (loading > 0)
"#,
    );
    let mut data = HashMap::new();
    data.insert("is_smoker".to_string(), "true".into());
    let and_case = run(&engine, "demo", data, Some(&["main".to_string()]), false);
    assert_attach_invariant(&and_case);
    assert!(
        missing(&and_case, "main").contains(&"missing_flag".to_string()),
        "and continue-eval must keep live unbound left: {:?}",
        missing(&and_case, "main")
    );

    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data a: number
data b: number
data c: number
rule main: a + b + c
"#,
    );
    let product = run(&engine, "demo", HashMap::new(), None, false);
    assert_attach_invariant(&product);
    assert!(
        missing(&product, "main").contains(&"a".to_string()),
        "product continue-eval must keep first missing operand: {:?}",
        missing(&product, "main")
    );
}

/// Engine::run must never attach MissingData veto with empty or mismatched missing_data.
#[test]
fn g5_engine_run_never_attaches_empty_missing_data() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data a: number
data b: number
rule main: a + b
"#,
    );
    assert_attach_invariant(&run(&engine, "demo", HashMap::new(), None, false));

    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data missing_threshold: number
data is_smoker: boolean
data is_former_smoker: boolean
data years_since_quit: number
rule loading: 1
  unless is_former_smoker then years_since_quit + 1
  unless is_smoker then 2
rule main: missing_threshold > loading
"#,
    );
    let mut data = HashMap::new();
    data.insert("is_smoker".to_string(), "true".into());
    assert_attach_invariant(&run(
        &engine,
        "demo",
        data,
        Some(&["main".to_string()]),
        false,
    ));

    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec inner
data slot: number
rule half: slot / 2

spec outer
uses i: inner
rule r: i.half + 1
"#,
    );
    assert_attach_invariant(&run(&engine, "outer", HashMap::new(), None, false));
}

#[test]
fn g3_missing_data_order_follows_evaluation_order() {
    let mut engine = Engine::new();
    load(&mut engine, CHOOSER);
    let response = run(
        &engine,
        "chooser",
        HashMap::new(),
        Some(&["result".to_string()]),
        false,
    );
    assert_eq!(
        missing(&response, "result"),
        &["mode", "complex_input_a", "complex_input_b", "simple_input"]
    );
}

// --- Evaluation-order missing_data (decision-tree preorder) ---

#[test]
fn missing_data_asks_control_before_gated_body() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data expensive: number
data flag: boolean
rule main: expensive unless flag then 0
"#,
    );
    let unbound = run(&engine, "demo", HashMap::new(), None, false);
    assert_eq!(
        missing(&unbound, "main"),
        &["flag", "expensive"],
        "last-match unless: ask flag before gated expensive"
    );

    let mut data = HashMap::new();
    data.insert("flag".to_string(), "true".into());
    let gated = run(&engine, "demo", data, None, false);
    assert!(
        missing(&gated, "main").is_empty(),
        "flag true takes arm body 0; expensive absent from missing_data: {:?}",
        missing(&gated, "main")
    );
    assert!(
        !rule(&gated, "main").is_missing_data(),
        "flag true settles the rule"
    );
}

#[test]
fn missing_data_last_unless_first_mixed_scrutinees() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data x: number
data y: number
data a: boolean
data b: boolean
rule r: 0 unless a then x unless b then y
"#,
    );
    let response = run(&engine, "demo", HashMap::new(), None, false);
    assert_eq!(
        missing(&response, "r"),
        &["b", "y", "a", "x"],
        "reverse unless scan order regardless of declaration order"
    );
}

#[test]
fn missing_data_arithmetic_left_to_right() {
    let mut engine = Engine::new();
    load(
        &mut engine,
        r#"
spec demo
data zebra: number
data alpha: number
rule total: alpha + zebra
"#,
    );
    let response = run(&engine, "demo", HashMap::new(), None, false);
    assert_eq!(
        missing(&response, "total"),
        &["alpha", "zebra"],
        "sum operands in evaluation order, not declaration order"
    );
}

#[test]
fn missing_data_folded_matches_unfolded_piecewise_order() {
    let expected = ["mode", "complex_input_a", "complex_input_b", "simple_input"];

    let mut folded = Engine::new();
    load(&mut folded, CHOOSER);
    let folded_response = run(
        &folded,
        "chooser",
        HashMap::new(),
        Some(&["result".to_string()]),
        false,
    );
    assert_eq!(missing(&folded_response, "result"), &expected);

    let mut unfolded = Engine::new();
    load(
        &mut unfolded,
        r#"
spec chooser
data mode: text -> options "simple" "complex"
data simple_input: number
data complex_input_a: number
data complex_input_b: number
rule result: veto "pick mode"
  unless mode is "simple" and true then simple_input
  unless mode is "complex" then complex_input_a + complex_input_b
"#,
    );
    let unfolded_response = run(
        &unfolded,
        "chooser",
        HashMap::new(),
        Some(&["result".to_string()]),
        false,
    );
    assert_eq!(
        missing(&unfolded_response, "result"),
        &expected,
        "non-foldable arm condition must keep same evaluation order as OrderedDispatch"
    );
}

#[test]
fn missing_data_interleaved_mixed_scrutinees() {
    let code = r#"
spec chooser
data mode: text -> options "simple" "complex"
data priority: text -> options "low" "high"
data simple_input: number
data complex_input_a: number
data complex_input_b: number
rule result: veto "pick mode"
  unless mode is "simple" then simple_input
  unless priority is "high" then 100
  unless mode is "complex" then complex_input_a + complex_input_b
"#;
    let mut engine = Engine::new();
    load(&mut engine, code);

    let unbound = run(
        &engine,
        "chooser",
        HashMap::new(),
        Some(&["result".to_string()]),
        false,
    );
    assert_eq!(
        missing(&unbound, "result"),
        &[
            "mode",
            "complex_input_a",
            "complex_input_b",
            "priority",
            "simple_input"
        ]
    );

    let mut complex = HashMap::new();
    complex.insert("mode".to_string(), "complex".into());
    let complex_response = run(
        &engine,
        "chooser",
        complex,
        Some(&["result".to_string()]),
        false,
    );
    assert_eq!(
        missing(&complex_response, "result"),
        &["complex_input_a", "complex_input_b"]
    );

    let mut high = HashMap::new();
    high.insert("priority".to_string(), "high".into());
    let high_alone = run(
        &engine,
        "chooser",
        high,
        Some(&["result".to_string()]),
        false,
    );
    assert_eq!(
        missing(&high_alone, "result"),
        &["mode", "complex_input_a", "complex_input_b", "simple_input"],
        "priority high alone cannot settle: later complex arm still MissingData on unbound mode"
    );

    let mut middle_wins = HashMap::new();
    middle_wins.insert("mode".to_string(), "simple".into());
    middle_wins.insert("priority".to_string(), "high".into());
    let middle = run(
        &engine,
        "chooser",
        middle_wins,
        Some(&["result".to_string()]),
        false,
    );
    assert!(
        missing(&middle, "result").is_empty(),
        "mode not complex + priority high: middle arm wins; missing_data empty: {:?}",
        missing(&middle, "result")
    );
}
