use lemma::DateTimeValue;
use lemma::{Engine, Error, ResourceLimits};
use std::time::Instant;

#[test]
fn test_source_size_limit() {
    let limits = ResourceLimits {
        max_source_size_bytes: 100,
        ..ResourceLimits::default()
    };

    let mut engine = Engine::with_limits(limits);

    // Create a file larger than 100 bytes
    let large_code = "spec test\ndata x: 1\n".repeat(10); // ~200 bytes

    let result = engine.load([(lemma::SourceType::Volatile, &large_code.to_string())]);

    let load_err = result.unwrap_err();
    let limit_err = find_resource_limit_name(&load_err.errors)
        .expect("expected at least one ResourceLimitExceeded");
    assert_eq!(limit_err, "max_source_size_bytes");
}

#[test]
fn expression_exceeding_max_depth_is_rejected() {
    let limits = ResourceLimits {
        max_expression_depth: 5,
        ..ResourceLimits::default()
    };
    // 5 nested parens = depth 6 (1 for rule expr + 5 for parens)
    let code = "spec test\ndata x: 1\nrule r: (((((1 + 1) + 1) + 1) + 1) + 1) + 1";
    let mut engine = Engine::with_limits(limits);
    let result = engine.load([(lemma::SourceType::Volatile, code.to_string())]);
    let load_err = result.unwrap_err();
    let limit_err = find_resource_limit_name(&load_err.errors)
        .expect("expected ResourceLimitExceeded for expression depth");
    assert_eq!(limit_err, "max_expression_depth");
}

#[test]
fn expression_depth_error_has_source_location() {
    let limits = ResourceLimits {
        max_expression_depth: 3,
        ..ResourceLimits::default()
    };
    let code = "spec test\ndata x: 1\nrule r: (((1 + 1) + 1) + 1) + 1";
    let mut engine = Engine::with_limits(limits);
    let result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("test.lemma"))),
        code.to_string(),
    )]);
    let load_err = result.unwrap_err();
    let err = load_err
        .errors
        .iter()
        .find(|e| matches!(e, Error::ResourceLimitExceeded { .. }))
        .expect("expected ResourceLimitExceeded");
    let source = err
        .location()
        .expect("depth error should have source location");
    assert_eq!(source.source_type.to_string(), "test.lemma");
    assert!(source.span.line > 0, "source line should be set");
}

// --- Expression count limits ---

#[test]
fn expression_count_exceeding_limit_is_rejected() {
    let limits = ResourceLimits {
        max_expression_count: 3,
        ..ResourceLimits::default()
    };
    // a + b + c + d → 7 nodes (4 refs + 3 arithmetic), exceeds 3
    let code = "spec test\ndata a: 1\ndata b: 2\ndata c: 3\ndata d: 4\nrule r: a + b + c + d";
    let mut engine = Engine::with_limits(limits);
    let result = engine.load([(lemma::SourceType::Volatile, code.to_string())]);
    let load_err = result.unwrap_err();
    let limit_err = find_resource_limit_name(&load_err.errors)
        .expect("expected ResourceLimitExceeded for expression count");
    assert_eq!(limit_err, "max_expression_count");
}

#[test]
fn expression_count_catches_deep_sqrt_without_depth_guard() {
    let limits = ResourceLimits {
        max_expression_count: 20,
        max_expression_depth: 1000, // intentionally high — rely on count
        ..ResourceLimits::default()
    };
    let mut expr = String::from("1");
    for _ in 0..50 {
        expr = format!("sqrt {}", expr);
    }
    let code = format!("spec test\ndata x: 1\nrule r: {}", expr);
    let mut engine = Engine::with_limits(limits);
    let result = engine.load([(lemma::SourceType::Volatile, &code.to_string())]);
    let load_err = result.unwrap_err();
    let limit_err = find_resource_limit_name(&load_err.errors)
        .expect("expression count should catch deep sqrt even when depth limit is high");
    assert_eq!(limit_err, "max_expression_count");
}

#[test]
fn expression_count_error_has_source_location() {
    let limits = ResourceLimits {
        max_expression_count: 2,
        ..ResourceLimits::default()
    };
    let code = "spec test\ndata x: 1\nrule r: x + 1 + 2";
    let mut engine = Engine::with_limits(limits);
    let result = engine.load([(
        lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("test.lemma"))),
        code.to_string(),
    )]);
    let load_err = result.unwrap_err();
    let err = load_err
        .errors
        .iter()
        .find(|e| matches!(e, Error::ResourceLimitExceeded { .. }))
        .expect("expected ResourceLimitExceeded");
    let source = err
        .location()
        .expect("expression count error should have source location");
    assert_eq!(source.source_type.to_string(), "test.lemma");
}

#[test]
fn test_data_value_size_limit() {
    let limits = ResourceLimits {
        max_data_value_bytes: 50,
        ..ResourceLimits::default()
    };

    let mut engine = Engine::with_limits(limits);
    engine
        .load([(
            lemma::SourceType::Volatile,
            "spec test\ndata name: text\nrule result: name".to_string(),
        )])
        .unwrap();

    let large_string = "a".repeat(100);
    let mut data = std::collections::HashMap::new();
    data.insert("name".to_string(), large_string.to_string());

    let now = DateTimeValue::now();
    let response = engine
        .run(None, "test", Some(&now), data, None, false)
        .expect("oversized data value must not abort evaluation");
    let result = response.results.get("result").expect("result rule");
    assert!(
        result.vetoed,
        "oversized data value must veto the dependent rule"
    );
    let reason = result.veto_reason.as_deref().expect("veto reason");
    assert!(
        reason.contains("Data name [text]:") && reason.contains("size limit"),
        "veto reason must name field, type, and size limit, got: {reason}"
    );
}

// --- Name length limits ---

/// Helper to extract the `limit_name` from the first `ResourceLimitExceeded` in a list of errors.
fn find_resource_limit_name(errors: &[Error]) -> Option<String> {
    errors.iter().find_map(|e| match e {
        Error::ResourceLimitExceeded { limit_name, .. } => Some(limit_name.clone()),
        _ => None,
    })
}

/// Wide unless chain over distinct data fields — many unique NormalForm cells,
/// no Rule-overlay sharing. Must hit `max_normalized_expression_nodes`.
#[test]
fn non_sharing_wide_unless_hits_normalized_node_budget() {
    let arm_count = 40;
    let mut code = String::from("spec blowup\n");
    for i in 0..arm_count {
        code.push_str(&format!("data d{i}: boolean\n"));
    }
    code.push_str("rule r: 0\n");
    for i in 0..arm_count {
        code.push_str(&format!("  unless d{i} then {i}\n"));
    }

    let limits = ResourceLimits {
        max_normalized_expression_nodes: 50,
        max_expression_count: 100_000,
        max_expression_depth: 1_000,
        ..ResourceLimits::default()
    };
    let mut engine = Engine::with_limits(limits);
    let result = engine.load([(lemma::SourceType::Volatile, code)]);
    let load_err = result.expect_err("wide non-sharing unless must exceed NF cell budget");
    let limit_err = find_resource_limit_name(&load_err.errors)
        .expect("expected ResourceLimitExceeded for normalized nodes");
    assert_eq!(limit_err, "max_normalized_expression_nodes");
}

// --- Spec dependency depth / DAG size limits ---

/// `spec s0` uses `s1`, which uses `s2`, ... down to `s{levels-1}` (a leaf).
fn spec_chain(levels: usize) -> String {
    let mut code = String::new();
    for i in 0..levels {
        code.push_str(&format!("spec s{i}\n"));
        if i + 1 < levels {
            code.push_str(&format!("uses dep: s{}\n", i + 1));
            code.push_str(&format!("rule r{i}: dep.r{}\n\n", i + 1));
        } else {
            code.push_str(&format!("rule r{i}: 1\n"));
        }
    }
    code
}

#[test]
fn spec_chain_exceeding_dependency_depth_is_rejected() {
    let limits = ResourceLimits {
        max_spec_dependency_depth: 3,
        ..ResourceLimits::default()
    };
    let mut engine = Engine::with_limits(limits);
    let result = engine.load([(lemma::SourceType::Volatile, spec_chain(6))]);
    let load_err = result.unwrap_err();
    let limit_err = find_resource_limit_name(&load_err.errors)
        .expect("expected ResourceLimitExceeded for dependency depth");
    assert_eq!(limit_err, "max_spec_dependency_depth");
}

#[test]
fn spec_chain_within_dependency_depth_loads() {
    let limits = ResourceLimits {
        max_spec_dependency_depth: 3,
        ..ResourceLimits::default()
    };
    let mut engine = Engine::with_limits(limits);
    engine
        .load([(lemma::SourceType::Volatile, spec_chain(4))])
        .expect("chain of 4 has depth 3, within the limit");
}

#[test]
fn spec_chain_within_default_depth_loads() {
    let mut engine = Engine::new();
    engine
        .load([(lemma::SourceType::Volatile, spec_chain(6))])
        .expect("chain of 6 loads under the default depth limit");
}

#[test]
fn dag_exceeding_max_specs_is_rejected() {
    let limits = ResourceLimits {
        max_dag_specs: 3,
        ..ResourceLimits::default()
    };
    // Root imports 4 leaves: DAG holds 5 specs, over the limit of 3.
    let code = r#"
spec leaf_a
rule r: 1

spec leaf_b
rule r: 1

spec leaf_c
rule r: 1

spec leaf_d
rule r: 1

spec root
uses a: leaf_a
uses b: leaf_b
uses c: leaf_c
uses d: leaf_d
rule total: a.r + b.r + c.r + d.r
"#;
    let mut engine = Engine::with_limits(limits);
    let result = engine.load([(lemma::SourceType::Volatile, code.to_string())]);
    let load_err = result.unwrap_err();
    let limit_err = find_resource_limit_name(&load_err.errors)
        .expect("expected ResourceLimitExceeded for DAG size");
    assert_eq!(limit_err, "max_dag_specs");
}

#[test]
fn spec_name_exceeding_max_length_is_rejected() {
    let name = "a".repeat(lemma::MAX_SPEC_NAME_LENGTH + 1);
    let code = format!("spec {name}\ndata x: 1");
    let mut engine = Engine::default();
    let result = engine.load([(lemma::SourceType::Volatile, &code.to_string())]);
    let load_err = result.unwrap_err();
    let limit_err = find_resource_limit_name(&load_err.errors)
        .expect("expected ResourceLimitExceeded for spec name");
    assert_eq!(limit_err, "max_spec_name_length");
}

#[test]
fn data_name_exceeding_max_length_is_rejected() {
    let name = "a".repeat(lemma::MAX_DATA_NAME_LENGTH + 1);
    let code = format!("spec test\ndata {name}: 1");
    let mut engine = Engine::default();
    let result = engine.load([(lemma::SourceType::Volatile, &code.to_string())]);
    let load_err = result.unwrap_err();
    let limit_err = find_resource_limit_name(&load_err.errors)
        .expect("expected ResourceLimitExceeded for data name");
    assert_eq!(limit_err, "max_data_name_length");
}

#[test]
fn data_binding_name_exceeding_max_length_is_rejected() {
    let name = "a".repeat(lemma::MAX_DATA_NAME_LENGTH + 1);
    let code = format!("spec test\ndata other.{name}: 1");
    let mut engine = Engine::default();
    let result = engine.load([(lemma::SourceType::Volatile, &code.to_string())]);
    let load_err = result.unwrap_err();
    let limit_err = find_resource_limit_name(&load_err.errors)
        .expect("expected ResourceLimitExceeded for data binding name");
    assert_eq!(limit_err, "max_data_name_length");
}

#[test]
fn rule_name_exceeding_max_length_is_rejected() {
    let name = "a".repeat(lemma::MAX_RULE_NAME_LENGTH + 1);
    let code = format!("spec test\nrule {name}: 1");
    let mut engine = Engine::default();
    let result = engine.load([(lemma::SourceType::Volatile, &code.to_string())]);
    let load_err = result.unwrap_err();
    let limit_err = find_resource_limit_name(&load_err.errors)
        .expect("expected ResourceLimitExceeded for rule name");
    assert_eq!(limit_err, "max_rule_name_length");
}

#[test]
fn data_type_name_exceeding_max_length_is_rejected() {
    let name = "a".repeat(lemma::MAX_DATA_NAME_LENGTH + 1);
    let code = format!("spec test\ndata {name}: number\ndata x: 1");
    let mut engine = Engine::default();
    let result = engine.load([(lemma::SourceType::Volatile, &code.to_string())]);
    let load_err = result.unwrap_err();
    let rle = find_resource_limit_name(&load_err.errors)
        .expect("expected ResourceLimitExceeded for data name");
    assert_eq!(rle, "max_data_name_length");
}

#[test]
fn data_import_name_exceeding_max_length_is_rejected() {
    let name = "a".repeat(lemma::MAX_DATA_NAME_LENGTH + 1);
    let code =
        format!("spec other\ndata v: number\n\nspec test\nuses {name}: other\ndata x: number");
    let mut engine = Engine::default();
    let result = engine.load([(lemma::SourceType::Volatile, &code.to_string())]);
    let load_err = result.unwrap_err();
    let rle = find_resource_limit_name(&load_err.errors)
        .expect("expected ResourceLimitExceeded for data import name");
    assert_eq!(rle, "max_data_name_length");
}

/// Scaling test: incremental rule counts to find performance cliffs.
#[test]
fn performance_test_10k_rules() {
    use std::collections::HashMap;
    use std::fmt::Write;

    const NODES_PER_RULE: usize = 19;

    fn build_wide_spec(spec_name: &str, num_rules: usize) -> String {
        let mut code = String::with_capacity(num_rules * 60);
        write!(code, "spec {spec_name}\ndata x: 1\n").unwrap();
        for i in 0..num_rules {
            writeln!(code, "rule r_{i}: x + x + x + x + x + x + x + x + x + x").unwrap();
        }
        code
    }

    let num_rules = 10000;
    let nodes = num_rules * NODES_PER_RULE;
    let code = build_wide_spec("test", num_rules);
    let bytes = code.len();
    let limits = ResourceLimits {
        max_source_size_bytes: 100 * 1024 * 1024,
        max_expression_count: nodes + 1000,
        ..ResourceLimits::default()
    };
    let mut engine = Engine::with_limits(limits);

    let start = Instant::now();
    engine
        .load([(lemma::SourceType::Volatile, &code.to_string())])
        .unwrap_or_else(|errs| panic!("{num_rules} rules failed: {:?}", errs));
    let elapsed = start.elapsed();

    let now = DateTimeValue::now();
    let eval_start = Instant::now();
    let resp = engine
        .run(None, "test", Some(&now), HashMap::new(), None, false)
        .unwrap();
    let eval_time = eval_start.elapsed();

    eprintln!(
        "{num_rules:>6} rules ({nodes:>7} nodes, {bytes:>8} bytes): parse+plan {elapsed:>8.2?}  eval {eval_time:>8.2?}  result={:?}",
        resp.results[0].result()
    );
}

/// Scaling test: deep rule dependency chains (linear + binary tree).
#[test]
fn bench_deep_chains() {
    bench_deep_chains_body();
}

fn bench_deep_chains_body() {
    use std::collections::HashMap;
    use std::fmt::Write;

    fn build_linear_chain(num_rules: usize) -> String {
        let mut code = String::with_capacity(num_rules * 30);
        write!(code, "spec chain\ndata x: 1\nrule r_0: x\n").unwrap();
        for i in 1..num_rules {
            writeln!(code, "rule r_{i}: r_{} + 1", i - 1).unwrap();
        }
        code
    }

    fn build_binary_tree(depth: u32) -> String {
        let leaves = 1_usize << depth;
        let total_rules = (1 << (depth + 1)) - 1;
        let mut code = String::with_capacity(total_rules * 45);
        write!(code, "spec tree\ndata x: 1\n").unwrap();
        for i in 0..leaves {
            writeln!(code, "rule r_0_{i}: x").unwrap();
        }
        for level in 1..=depth {
            let level_size = 1 << (depth - level);
            for j in 0..level_size {
                let left = 2 * j;
                let right = 2 * j + 1;
                writeln!(
                    code,
                    "rule r_{level}_{j}: r_{}_{left} + r_{}_{right}",
                    level - 1,
                    level - 1
                )
                .unwrap();
            }
        }
        code
    }

    const LINEAR_NODES_PER_RULE: usize = 5;
    const TREE_LEAF_NODES: usize = 2;
    const TREE_INTERNAL_NODES: usize = 5;

    eprintln!("--- Linear chain ---");
    for num_rules in [50, 100, 200] {
        let code = build_linear_chain(num_rules);
        let est_nodes = num_rules * LINEAR_NODES_PER_RULE;
        let limits = ResourceLimits {
            max_source_size_bytes: 100 * 1024 * 1024,
            max_expression_count: est_nodes + 1000,
            ..ResourceLimits::default()
        };
        let mut engine = Engine::with_limits(limits);

        let start = Instant::now();
        engine
            .load([(
                lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "chain.lemma",
                ))),
                &code.to_string(),
            )])
            .unwrap_or_else(|errs| panic!("linear {num_rules} rules failed: {:?}", errs));
        let elapsed = start.elapsed();

        let now = DateTimeValue::now();
        let eval_start = Instant::now();
        let resp = engine
            .run(None, "chain", Some(&now), HashMap::new(), None, false)
            .unwrap();
        let eval_time = eval_start.elapsed();

        eprintln!(
            "chain {num_rules:>6} rules (~{est_nodes:>6} nodes): parse+plan {elapsed:>8.2?}  eval {eval_time:>8.2?}  result={:?}",
            resp.results[0].result()
        );
    }

    eprintln!("--- Binary tree ---");
    for depth in [4, 6, 8] {
        let leaves = 1_usize << depth;
        let total_rules = (1 << (depth + 1)) - 1;
        let est_nodes = leaves * TREE_LEAF_NODES + (total_rules - leaves) * TREE_INTERNAL_NODES;
        let code = build_binary_tree(depth);
        let limits = ResourceLimits {
            max_source_size_bytes: 100 * 1024 * 1024,
            max_expression_count: est_nodes + 1000,
            ..ResourceLimits::default()
        };
        let mut engine = Engine::with_limits(limits);

        let start = Instant::now();
        engine
            .load([(
                lemma::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "tree.lemma",
                ))),
                &code.to_string(),
            )])
            .unwrap_or_else(|errs| panic!("tree depth {depth} failed: {:?}", errs));
        let elapsed = start.elapsed();

        let now = DateTimeValue::now();
        let eval_start = Instant::now();
        let resp = engine
            .run(None, "tree", Some(&now), HashMap::new(), None, false)
            .unwrap();
        let eval_time = eval_start.elapsed();

        eprintln!(
            "tree  {total_rules:>6} rules (depth {depth:>2}, ~{est_nodes:>6} nodes): parse+plan {elapsed:>8.2?}  eval {eval_time:>8.2?}  result={:?}",
            resp.results[0].result()
        );
    }
}

/// Long linear rule chain stays under `max_normal_form_depth` because embeds are leaves.
#[test]
fn linear_chain_loads_under_tight_normal_form_depth() {
    let mut code = String::from("spec chain\ndata x0: number\nrule r1: x0 + 1\n");
    for i in 2..=100 {
        code.push_str(&format!("rule r{i}: r{} + 1\n", i - 1));
    }
    let limits = ResourceLimits {
        max_normal_form_depth: 3,
        ..ResourceLimits::default()
    };
    let mut engine = Engine::with_limits(limits);
    engine
        .load([(lemma::SourceType::Volatile, code)])
        .expect("100-rule chain must load when embeds count as depth 1");
}

/// Long linear rule chain stays under `max_normalized_expression_nodes` because embeds are one cell.
#[test]
fn linear_chain_loads_under_tight_normalized_node_budget() {
    let mut code = String::from("spec chain\ndata x0: number\nrule r1: x0 + 1\n");
    for i in 2..=100 {
        code.push_str(&format!("rule r{i}: r{} + 1\n", i - 1));
    }
    let limits = ResourceLimits {
        max_normalized_expression_nodes: 3,
        ..ResourceLimits::default()
    };
    let mut engine = Engine::with_limits(limits);
    engine
        .load([(lemma::SourceType::Volatile, code)])
        .expect("100-rule chain must load when embeds count as one cell");
}

/// Nested math ops in one rule body still hit `max_normal_form_depth`.
#[test]
fn nested_math_exceeding_normal_form_depth_is_rejected() {
    let limits = ResourceLimits {
        max_normal_form_depth: 3,
        max_expression_depth: 20,
        ..ResourceLimits::default()
    };
    let code = "spec deep\ndata x: number\nrule r: sqrt (sqrt (sqrt (sqrt x)))";
    let mut engine = Engine::with_limits(limits);
    let result = engine.load([(lemma::SourceType::Volatile, code.to_string())]);
    let load_err = result.expect_err("four nested sqrt must exceed depth 3");
    let limit_err = find_resource_limit_name(&load_err.errors)
        .expect("expected ResourceLimitExceeded for normal form depth");
    assert_eq!(limit_err, "max_normal_form_depth");
}
