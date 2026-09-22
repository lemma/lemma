//! Planning module integration tests (formerly `internal_tests` in `planning/mod.rs`).

use crate::engine::Context;
use crate::limits::ResourceLimits;
use crate::literals::DateGranularity;
use crate::parsing::ast::{
    DataValue, LemmaData, LemmaRepository, LemmaSpec, ParentType, Reference, Span,
};
use crate::parsing::parse;
use crate::parsing::source::Source;
use crate::planning::execution_plan::ExecutionPlan;
use crate::planning::plan;
use crate::planning::semantics::{DataPath, PathSegment, TypeDefiningSpec, TypeExtends};
use crate::Error;
use std::collections::HashMap;
use std::sync::Arc;

/// Plan every spec set of a fresh context, starting from no committed plans: the
/// pass a first load performs, where every spec set is new and therefore changed.
fn plan_everything(ctx: &Context) -> crate::planning::PlanningResult {
    let changed: Vec<(std::sync::Arc<crate::LemmaRepository>, String)> = ctx
        .repositories()
        .iter()
        .flat_map(|(repository, by_name)| {
            by_name
                .keys()
                .map(move |spec_name| (std::sync::Arc::clone(repository), spec_name.clone()))
        })
        .collect();
    plan(
        ctx,
        &ResourceLimits::default(),
        &crate::planning::ReplanScope::from_changed_sets(ctx, changed),
        &crate::planning::PlanStore::new(),
    )
}

/// Test helper: plan a single spec and return its execution plan.
fn plan_single(
    main_spec: &LemmaSpec,
    all_specs: &[LemmaSpec],
) -> Result<ExecutionPlan, Vec<Error>> {
    let mut ctx = Context::new();
    let repository = ctx.workspace();
    for spec in all_specs {
        ctx.insert_spec(Arc::clone(&repository), spec.clone())?;
    }
    let main_spec_arc = ctx
        .spec_set(&repository, main_spec.name.as_str())
        .and_then(|ss| ss.get_exact(main_spec.effective_from()).cloned())
        .expect("main_spec must be in all_specs");
    let result = plan_everything(&ctx);
    if !result.errors.is_empty() {
        return Err(result.errors);
    }
    let Some(plans) = result
        .plans
        .get_plans(repository.name.as_deref(), &main_spec_arc.name)
    else {
        return Err(vec![Error::validation(
            format!("No execution plan produced for spec '{}'", main_spec.name),
            Some(crate::planning::semantics::Source::new(
                crate::parsing::source::SourceType::Volatile,
                crate::parsing::ast::Span {
                    start: 0,
                    end: 0,
                    line: 1,
                    col: 0,
                },
            )),
            None::<String>,
        )]);
    };
    if plans.is_empty() {
        Err(vec![Error::validation(
            format!("No execution plan produced for spec '{}'", main_spec.name),
            Some(crate::planning::semantics::Source::new(
                crate::parsing::source::SourceType::Volatile,
                crate::parsing::ast::Span {
                    start: 0,
                    end: 0,
                    line: 1,
                    col: 0,
                },
            )),
            None::<String>,
        )])
    } else {
        Ok(plans.values().next().expect("plan").clone())
    }
}

#[test]
fn test_basic_validation() {
    let input = r#"spec person
data name: "John"
data age: 25
rule is_adult: age >= 18"#;

    let specs: Vec<_> = parse(
        input,
        crate::parsing::source::SourceType::Volatile,
        &ResourceLimits::default(),
    )
    .unwrap()
    .into_flattened_specs();

    let mut sources = HashMap::new();
    sources.insert(
        crate::parsing::source::SourceType::Volatile,
        input.to_string(),
    );

    for spec in &specs {
        let result = plan_single(spec, &specs);
        assert!(
            result.is_ok(),
            "Basic validation should pass: {:?}",
            result.err()
        );
    }
}

#[test]
fn test_duplicate_data() {
    let input = r#"spec person
data name: "John"
data name: "Jane""#;

    let specs: Vec<_> = parse(
        input,
        crate::parsing::source::SourceType::Volatile,
        &ResourceLimits::default(),
    )
    .unwrap()
    .into_flattened_specs();

    let mut sources = HashMap::new();
    sources.insert(
        crate::parsing::source::SourceType::Volatile,
        input.to_string(),
    );

    let result = plan_single(&specs[0], &specs);

    assert!(
        result.is_err(),
        "Duplicate data should cause validation error"
    );
    let errors = result.unwrap_err();
    let error_string = errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    assert!(
        error_string.contains("already used"),
        "Error should mention duplicate data: {}",
        error_string
    );
    assert!(error_string.contains("name"));
}

#[test]
fn mixed_type_range_literal_is_planning_error_not_panic() {
    let input = r#"spec demo
data x: 1 ... yes"#;

    let specs: Vec<_> = parse(
        input,
        crate::parsing::source::SourceType::Volatile,
        &ResourceLimits::default(),
    )
    .unwrap()
    .into_flattened_specs();

    let result = plan_single(&specs[0], &specs);

    let errors = result.expect_err("mixed-type range literal must be a planning error");
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one planning error, got: {:?}",
        errors.iter().map(|e| e.to_string()).collect::<Vec<_>>()
    );
    let error_string = errors[0].to_string();
    assert!(
        error_string.contains(
            "range endpoints must have the same supported base type, got number and boolean"
        ),
        "unexpected error message: {}",
        error_string
    );
}

#[test]
fn text_range_literal_is_planning_error_not_panic() {
    let input = r#"spec demo
data x: "a" ... "b""#;

    let specs: Vec<_> = parse(
        input,
        crate::parsing::source::SourceType::Volatile,
        &ResourceLimits::default(),
    )
    .unwrap()
    .into_flattened_specs();

    let result = plan_single(&specs[0], &specs);

    let errors = result.expect_err("text range literal must be a planning error");
    assert_eq!(
        errors.len(),
        1,
        "expected exactly one planning error, got: {:?}",
        errors.iter().map(|e| e.to_string()).collect::<Vec<_>>()
    );
    let error_string = errors[0].to_string();
    assert!(
        error_string
            .contains("range endpoints must have the same supported base type, got text and text"),
        "unexpected error message: {}",
        error_string
    );
}

#[test]
fn qualified_type_from_spec_with_type_errors_is_planning_error_not_panic() {
    let input = r#"spec b
data money: number -> minimum 10 -> maximum 5

spec a
uses b
data x: b.money"#;

    let specs: Vec<_> = parse(
        input,
        crate::parsing::source::SourceType::Volatile,
        &ResourceLimits::default(),
    )
    .unwrap()
    .into_flattened_specs();

    let result = plan_single(&specs[0], &specs);

    let errors = result.expect_err("failing import target must be a planning error");
    let error_string = errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    assert!(
        error_string.contains("minimum"),
        "expected the import target's own type error to be reported: {}",
        error_string
    );
    assert!(
        error_string.contains(
            "Cannot resolve type 'money' from spec 'b' (via import 'b'): spec 'b' failed type resolution"
        ),
        "expected the consumer's qualified type resolution error to be reported: {}",
        error_string
    );
}

#[test]
fn test_duplicate_rules() {
    let input = r#"spec person
data age: 25
rule is_adult: age >= 18
rule is_adult: age >= 21"#;

    let specs: Vec<_> = parse(
        input,
        crate::parsing::source::SourceType::Volatile,
        &ResourceLimits::default(),
    )
    .unwrap()
    .into_flattened_specs();

    let mut sources = HashMap::new();
    sources.insert(
        crate::parsing::source::SourceType::Volatile,
        input.to_string(),
    );

    let result = plan_single(&specs[0], &specs);

    assert!(
        result.is_err(),
        "Duplicate rules should cause validation error"
    );
    let errors = result.unwrap_err();
    let error_string = errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    assert!(
        error_string.contains("Duplicate rule"),
        "Error should mention duplicate rule: {}",
        error_string
    );
    assert!(error_string.contains("is_adult"));
}

#[test]
fn test_circular_dependency() {
    let input = r#"spec test
rule a: b
rule b: a"#;

    let specs: Vec<_> = parse(
        input,
        crate::parsing::source::SourceType::Volatile,
        &ResourceLimits::default(),
    )
    .unwrap()
    .into_flattened_specs();

    let mut sources = HashMap::new();
    sources.insert(
        crate::parsing::source::SourceType::Volatile,
        input.to_string(),
    );

    let result = plan_single(&specs[0], &specs);

    assert!(
        result.is_err(),
        "Circular dependency should cause validation error"
    );
    let errors = result.unwrap_err();
    let error_string = errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    assert!(error_string.contains("Circular dependency") || error_string.contains("circular"));
}

#[test]
fn test_multiple_specs() {
    let input = r#"spec person
data name: "John"
data age: 25

spec company
data name: "Acme Corp"
uses employee: person"#;

    let specs: Vec<_> = parse(
        input,
        crate::parsing::source::SourceType::Volatile,
        &ResourceLimits::default(),
    )
    .unwrap()
    .into_flattened_specs();

    let mut sources = HashMap::new();
    sources.insert(
        crate::parsing::source::SourceType::Volatile,
        input.to_string(),
    );

    let result = plan_single(&specs[0], &specs);

    assert!(
        result.is_ok(),
        "Multiple specs should validate successfully: {:?}",
        result.err()
    );
}

#[test]
fn test_invalid_spec_reference() {
    let input = r#"spec person
data name: "John"
uses contract: nonexistent"#;

    let specs: Vec<_> = parse(
        input,
        crate::parsing::source::SourceType::Volatile,
        &ResourceLimits::default(),
    )
    .unwrap()
    .into_flattened_specs();

    let mut sources = HashMap::new();
    sources.insert(
        crate::parsing::source::SourceType::Volatile,
        input.to_string(),
    );

    let result = plan_single(&specs[0], &specs);

    assert!(
        result.is_err(),
        "Invalid spec reference should cause validation error"
    );
    let errors = result.unwrap_err();
    let error_string = errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    assert!(
        error_string.contains("not found")
            || error_string.contains("Spec")
            || (error_string.contains("nonexistent") && error_string.contains("depends")),
        "Error should mention spec reference issue: {}",
        error_string
    );
    assert!(error_string.contains("nonexistent"));
}

#[test]
fn test_definition_empty_base_returns_lemma_error() {
    let mut spec = LemmaSpec::new("test".to_string());
    let source = Source::new(
        crate::parsing::source::SourceType::Volatile,
        Span {
            start: 0,
            end: 10,
            line: 1,
            col: 0,
        },
    );
    spec.data.push(LemmaData::new(
        Reference {
            segments: vec![],
            name: "x".to_string(),
        },
        DataValue::Definition {
            base: Some(ParentType::Custom {
                name: String::new(),
            }),
            constraints: None,
            value: None,
        },
        source,
    ));

    let specs = vec![spec.clone()];
    let mut sources = HashMap::new();
    sources.insert(
        crate::parsing::source::SourceType::Volatile,
        "spec test\ndata x:".to_string(),
    );

    let result = plan_single(&spec, &specs);
    assert!(
        result.is_err(),
        "Definition with empty base should fail planning"
    );
    let errors = result.unwrap_err();
    let combined = errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        combined.contains("Unknown parent ''"),
        "Error should mention empty/unknown type; got: {}",
        combined
    );
}

#[test]
fn test_data_binding_with_custom_type_resolves_in_correct_spec_context() {
    // This is a planning-level test: ensure data bindings resolve custom types correctly
    // when the type is defined in a different spec than the binding.
    //
    // spec one:
    //   data money: number
    //   data x: money
    // spec two:
    //   with one
    //   with one.x: 7
    //   rule getx: one.x
    let code = r#"
spec one
data money: number
data x: money

spec two
uses one
  -> with x: 7
rule getx: one.x
"#;

    let specs = parse(
        code,
        crate::parsing::source::SourceType::Volatile,
        &ResourceLimits::default(),
    )
    .unwrap()
    .into_flattened_specs();
    let spec_two = specs.iter().find(|d| d.name == "two").unwrap();

    let mut sources = HashMap::new();
    sources.insert(
        crate::parsing::source::SourceType::Volatile,
        code.to_string(),
    );
    let execution_plan = plan_single(spec_two, &specs).expect("planning should succeed");

    // Verify that one.x keeps its declared custom type name while resolving in spec one.
    let one_x_path = DataPath {
        segments: vec![PathSegment {
            uses: "one".to_string(),
            repository: None,
            spec: "one".to_string(),
        }],
        data: "x".to_string(),
    };

    let one_x_type = execution_plan
        .data
        .get(&one_x_path)
        .and_then(|d| d.schema_type())
        .expect("one.x should have a resolved type");

    assert_eq!(
        one_x_type.name(),
        "x",
        "one.x should have declared type 'x', got: {}",
        one_x_type.name()
    );
    assert!(one_x_type.is_number(), "money should be number-based");
}

#[test]
fn test_data_definition_from_spec_has_import_defining_spec() {
    let code = r#"
spec examples
data money: measure
  -> unit eur: 1.00

spec checkout
uses examples
data money: measure
  -> unit eur: 1.00
data local_price: money
data imported_price: examples.money
"#;

    let specs = parse(
        code,
        crate::parsing::source::SourceType::Volatile,
        &ResourceLimits::default(),
    )
    .unwrap()
    .into_flattened_specs();

    let mut ctx = Context::new();
    let repository = ctx.workspace();
    for spec in &specs {
        ctx.insert_spec(Arc::clone(&repository), spec.clone())
            .expect("insert spec");
    }

    let checkout_arc = ctx
        .spec_set(&repository, "checkout")
        .and_then(|ss| ss.get_exact(None).cloned())
        .expect("checkout spec should be present");

    let result = plan_everything(&ctx);
    assert!(
        result.errors.is_empty(),
        "No checkout planning errors expected, got: {:?}",
        result.errors
    );
    let checkout_plans = result
        .plans
        .get_plans(repository.name.as_deref(), &checkout_arc.name)
        .expect("checkout result should exist");
    assert!(
        !checkout_plans.is_empty(),
        "checkout should produce at least one plan"
    );
    let execution_plan = checkout_plans.values().next().expect("checkout plan");

    let local_type = execution_plan
        .data
        .get(&DataPath::new(vec![], "local_price".to_string()))
        .and_then(|d| d.schema_type())
        .expect("local_price should have schema type");
    let imported_type = execution_plan
        .data
        .get(&DataPath::new(vec![], "imported_price".to_string()))
        .and_then(|d| d.schema_type())
        .expect("imported_price should have schema type");

    match &local_type.extends {
        TypeExtends::Custom {
            defining_spec: TypeDefiningSpec::Local,
            ..
        } => {}
        other => panic!(
            "local_price should resolve as local defining_spec, got {:?}",
            other
        ),
    }

    match &imported_type.extends {
        TypeExtends::Custom {
            defining_spec: TypeDefiningSpec::Import,
            ..
        } => {}
        other => panic!(
            "imported_price should resolve as import defining_spec, got {:?}",
            other
        ),
    }
}

#[test]
fn test_plan_with_registry_grouped_specs() {
    let source = r#"spec somespec
data quantity: 10

spec example
uses inventory: somespec
rule total_measure: inventory.quantity"#;

    let parsed = parse(
        source,
        crate::parsing::source::SourceType::Volatile,
        &ResourceLimits::default(),
    )
    .unwrap();
    assert_eq!(parsed.flatten_specs().len(), 2);

    let mut ctx = Context::new();
    let repository = Arc::new(
        LemmaRepository::new(Some("@user/workspace".to_string()))
            .with_dependency("@user/workspace")
            .with_start_line(1)
            .with_source_type(crate::parsing::source::SourceType::Volatile),
    );
    for spec in parsed.flatten_specs() {
        ctx.insert_spec(Arc::clone(&repository), spec.clone())
            .expect("insert spec");
    }

    let result = plan_everything(&ctx);
    assert!(
        result.errors.is_empty(),
        "Planning under registry-scoped specs should succeed: {:?}",
        result.errors
    );
    let example_set = result
        .plans
        .get_plans(repository.name.as_deref(), "example")
        .expect("example result must exist");
    assert!(
        !example_set.is_empty(),
        "expected at least one plan for registry-grouped example"
    );
}

#[test]
fn test_multiple_independent_errors_are_all_reported() {
    // A spec referencing a non-existing import AND a non-existing
    // spec should report errors for BOTH, not just stop at the first.
    let source = r#"spec demo
uses type_src: nonexistent_type_source
  -> with amount: 10
uses helper: nonexistent_spec
data price: 10
rule total: helper.value + price"#;

    let specs = parse(
        source,
        crate::parsing::source::SourceType::Volatile,
        &ResourceLimits::default(),
    )
    .unwrap()
    .into_flattened_specs();

    let mut sources = HashMap::new();
    sources.insert(
        crate::parsing::source::SourceType::Volatile,
        source.to_string(),
    );

    let result = plan_single(&specs[0], &specs);
    assert!(result.is_err(), "Planning should fail with multiple errors");

    let errors = result.unwrap_err();
    let all_messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
    let combined = all_messages.join("\n");

    assert!(
        combined.contains("nonexistent_type_source"),
        "Should report import error for 'nonexistent_type_source'. Got:\n{}",
        combined
    );

    // Must also report the spec reference error (not just the import error)
    assert!(
        combined.contains("nonexistent_spec"),
        "Should report spec reference error for 'nonexistent_spec'. Got:\n{}",
        combined
    );

    // Should have at least 2 distinct kinds of errors (import + spec ref)
    assert!(
        errors.len() >= 2,
        "Expected at least 2 errors, got {}: {}",
        errors.len(),
        combined
    );

    let data_import_err = errors
        .iter()
        .find(|e| e.to_string().contains("nonexistent_type_source"))
        .expect("import error");
    let loc = data_import_err
        .location()
        .expect("import error should carry source location");
    assert_eq!(
        loc.source_type,
        crate::parsing::source::SourceType::Volatile
    );
    assert_ne!(
        (loc.span.start, loc.span.end),
        (0, 0),
        "import error span should not be empty"
    );
}

#[test]
fn test_type_error_does_not_suppress_cross_spec_data_error() {
    // When a import fails, errors about cross-spec data references
    // (e.g. ext.some_data where ext is a spec ref to a non-existing spec)
    // must still be reported.
    let source = r#"spec demo
uses cur: missing_spec
  -> with currency: 10
uses ext: also_missing
rule val: ext.some_data"#;

    let specs = parse(
        source,
        crate::parsing::source::SourceType::Volatile,
        &ResourceLimits::default(),
    )
    .unwrap()
    .into_flattened_specs();

    let mut sources = HashMap::new();
    sources.insert(
        crate::parsing::source::SourceType::Volatile,
        source.to_string(),
    );

    let result = plan_single(&specs[0], &specs);
    assert!(result.is_err());

    let errors = result.unwrap_err();
    let combined: String = errors
        .iter()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        combined.contains("missing_spec"),
        "Should report import error about 'missing_spec'. Got:\n{}",
        combined
    );

    // The spec reference error about 'also_missing' should ALSO be reported
    assert!(
        combined.contains("also_missing"),
        "Should report error about 'also_missing'. Got:\n{}",
        combined
    );
}

#[test]
fn test_spec_dag_orders_dep_before_consumer() {
    let source = r#"spec dep 2025-01-01
data money: number
data x: money

spec consumer 2025-01-01
uses dep
data imported_amount: dep.money
rule passthrough: imported_amount"#;
    let specs = parse(
        source,
        crate::parsing::source::SourceType::Volatile,
        &ResourceLimits::default(),
    )
    .unwrap()
    .into_flattened_specs();

    let mut ctx = Context::new();
    let repository = ctx.workspace();
    for spec in &specs {
        ctx.insert_spec(Arc::clone(&repository), spec.clone())
            .expect("insert spec");
    }

    let dt = crate::DateTimeValue {
        year: 2025,
        month: 1,
        day: 1,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: None,
        granularity: DateGranularity::Full,
    };
    let effective = crate::parsing::ast::EffectiveDate::DateTimeValue(dt);
    let consumer_arc = ctx
        .spec_set(&repository, "consumer")
        .and_then(|ss| ss.spec_at(&effective))
        .expect("consumer spec");
    let ordered_dependencies = crate::planning::discovery::discover_dependency_order(
        &ctx,
        consumer_arc,
        &effective,
        &crate::ResourceLimits::default(),
    )
    .expect("dependency order should succeed");
    let ordered_names: Vec<String> = ordered_dependencies
        .iter()
        .map(|s| s.spec.name.clone())
        .collect();
    let dep_idx = ordered_names
        .iter()
        .position(|n| n == "dep")
        .expect("dep must exist");
    let consumer_idx = ordered_names
        .iter()
        .position(|n| n == "consumer")
        .expect("consumer must exist");
    assert!(
        dep_idx < consumer_idx,
        "dependency must be planned before dependent. order={:?}",
        ordered_names
    );
}

#[test]
fn test_spec_dependency_cycle_surfaces_as_spec_error_and_populates_results() {
    let source = r#"spec a 2025-01-01
uses dep_b: b
data amount: number

spec b 2025-01-01
uses src_a: a
data imported_value: src_a.amount
"#;
    let specs = parse(
        source,
        crate::parsing::source::SourceType::Volatile,
        &ResourceLimits::default(),
    )
    .unwrap()
    .into_flattened_specs();

    let mut ctx = Context::new();
    let repository = ctx.workspace();
    for spec in &specs {
        ctx.insert_spec(Arc::clone(&repository), spec.clone())
            .expect("insert spec");
    }

    let result = plan_everything(&ctx);

    let spec_errors: Vec<String> = result.errors.iter().map(|e| e.to_string()).collect();
    assert!(
        spec_errors
            .iter()
            .any(|e| e.contains("Spec dependency cycle")),
        "expected cycle error on spec, got: {spec_errors:?}",
    );

    assert!(
        result
            .plans
            .get_plans(repository.name.as_deref(), "b")
            .is_none_or(|plans| plans.is_empty()),
        "cyclic spec 'b' must not install compiled plans"
    );
}
