//! Pure Rust evaluation engine for Lemma
//!
//! Executes pre-validated execution plans by walking each rule's
//! [`NormalForm`] equation DAG into one value table ([`tree`]). When `explain`
//! is true the walk is exhaustive (visits every cell narration displays) and a
//! second pass ([`narration`]) builds explanation trees from the filled table.
//!
//! Request state is one value table indexed by [`NormalFormId`]: the plan's
//! node table *is* the arena.

pub(crate) mod branch_semantics;
pub(crate) mod conversion_trace;
pub mod explanations;
pub mod expression;
pub(crate) mod narration;
pub mod response;
pub mod run_data;
pub(crate) mod tree;

pub use crate::computation::OperationResult;
use crate::computation::VetoType;
use crate::evaluation::response::EvaluatedRule;
use crate::planning::execution_plan::{
    reachable_data_paths, validate_value_against_type, ExecutionPlan,
};
use crate::planning::normalize::NormalFormId;
use crate::planning::semantics::{
    BoundValueKind, DataDefinition, DataPath, LemmaType, LiteralValue, ReferenceTarget, RulePath,
    ValueKind,
};
use indexmap::IndexMap;
pub use response::{Response, RuleResult};
pub use run_data::{RunData, RunDataValue};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn closest_ignored_key(needed: &str, ignored: &[String]) -> Option<String> {
    crate::string_distance::closest_name(needed, ignored)
}

/// Request-local mutable state for one plan run.
///
/// The value table is indexed by [`NormalFormId`] and doubles as memo and data
/// store. Rule reference values live in [`Self::rule_values`], filled by
/// [`tree::evaluate_rule`] before a consumer rule reference reads them. Control decisions
/// for missing-data walks are read from filled condition / scrutinee slots —
/// there is no separate log.
pub(crate) struct EvaluationContext {
    /// One slot per `plan.normal_forms` cell. Filled by data resolve and by
    /// `eval`; never cleared mid-request (values are a function of data).
    pub(crate) values: Vec<Option<OperationResult>>,
    /// One slot per `plan.rules` entry (same index as `IndexMap`). Filled by
    /// [`tree::evaluate_rule`] before consumers read rule references.
    pub(crate) rule_values: Vec<Option<OperationResult>>,
    now: LiteralValue,
    /// Ignored input keys from run data (typo hints for MissingData).
    ignored_unknown: Vec<String>,
    /// Whether this run data left any of the plan's promptable data paths unbound.
    any_promptable_data_unbound: bool,
    /// Evaluation policy: visit every cell narration displays (rewrite
    /// pre-images, arithmetic siblings after a definitive veto). Set for explain
    /// runs. Never changes a value. Unless arms after the winner are already
    /// visited by the last-wins reverse scan.
    pub(crate) exhaustive: bool,
    /// Explain runs only: narrated Rule nodes, filled in plan order after the walk.
    pub(crate) rule_explanations: HashMap<RulePath, crate::planning::explanation::ExplanationNode>,
}

impl EvaluationContext {
    fn new(plan: &ExecutionPlan, run_data: &RunData, now: LiteralValue, exhaustive: bool) -> Self {
        let mut values: Vec<Option<OperationResult>> = vec![None; plan.normal_forms.len()];

        // Caller bindings into their data-leaf slots.
        for (path, binding) in &run_data.bindings {
            let leaf = *plan
                .data_leaf
                .get(path)
                .unwrap_or_else(|| panic!("BUG: run binding for '{path}' has no data_leaf entry"));
            values[leaf.index()] = Some(binding.clone());
        }

        // Plan defaults into unbound data-leaf slots.
        for (path, definition) in &plan.data {
            let leaf = *plan
                .data_leaf
                .get(path)
                .expect("BUG: every plan.data path must have a data_leaf entry");
            if values[leaf.index()].is_some() {
                continue;
            }
            if let Some(fill) = definition.bound_fill() {
                values[leaf.index()] = Some(OperationResult::from_bound(fill.clone()));
            } else if let Some(literal) = definition.value() {
                let measure_binding_unit = definition
                    .schema_type()
                    .and_then(|schema| schema.measure_binding_unit.as_deref().map(Arc::from));
                values[leaf.index()] = Some(OperationResult::from_bound(BoundValueKind {
                    value: literal.value,
                    measure_binding_unit,
                }));
            }
        }

        // Reference chains: copy target into reference leaf (data_reference_order).
        for reference_path in &plan.data_reference_order {
            let leaf = *plan
                .data_leaf
                .get(reference_path)
                .expect("BUG: reference path missing from data_leaf");
            if values[leaf.index()].is_some() {
                continue;
            }
            match plan.data.get(reference_path) {
                Some(DataDefinition::Reference {
                    target: ReferenceTarget::Data(target_path),
                    resolved_type,
                    ..
                }) => {
                    let target_leaf = *plan
                        .data_leaf
                        .get(target_path)
                        .expect("BUG: reference target missing from data_leaf");
                    match values[target_leaf.index()].as_ref() {
                        Some(OperationResult::Veto(veto)) => {
                            values[leaf.index()] = Some(OperationResult::Veto(veto.clone()));
                        }
                        Some(OperationResult::Value(bound)) => {
                            let copied = bound.to_literal();
                            match validate_value_against_type(
                                resolved_type.as_ref(),
                                &copied,
                                &plan.resolved_types.unit_index,
                            ) {
                                Ok(()) => {
                                    values[leaf.index()] =
                                        Some(OperationResult::from_bound(bound.clone()));
                                }
                                Err(msg) => {
                                    values[leaf.index()] = Some(OperationResult::Veto(
                                        VetoType::computation(format!(
                                            "Reference '{}' violates declared constraint: {}",
                                            reference_path, msg
                                        )),
                                    ));
                                }
                            }
                        }
                        None => {}
                    }
                }
                Some(DataDefinition::Reference {
                    target: ReferenceTarget::Rule(_),
                    ..
                }) => unreachable!(
                    "BUG: data_reference_order holds only data-target references; '{}' targets a rule",
                    reference_path
                ),
                Some(
                    DataDefinition::Value { .. }
                    | DataDefinition::TypeDeclaration { .. }
                    | DataDefinition::Import { .. },
                ) => unreachable!(
                    "BUG: data_reference_order entry '{}' is not a reference",
                    reference_path
                ),
                None => unreachable!(
                    "BUG: data_reference_order references missing data path '{}'",
                    reference_path
                ),
            }
        }

        let any_promptable_data_unbound = plan.promptable_data_paths().any(|path| {
            let leaf = *plan
                .data_leaf
                .get(path)
                .expect("BUG: promptable path missing from data_leaf");
            values[leaf.index()].is_none()
        });

        Self {
            values,
            rule_values: vec![None; plan.rules.len()],
            now,
            ignored_unknown: run_data.ignored_unknown.clone(),
            any_promptable_data_unbound,
            exhaustive,
            rule_explanations: HashMap::new(),
        }
    }

    /// Stored value for a rule previously evaluated in this request.
    pub(crate) fn rule_value<'a>(
        &'a self,
        plan: &ExecutionPlan,
        path: &RulePath,
    ) -> &'a OperationResult {
        self.rule_values[plan.rule_index(path).index()]
            .as_ref()
            .unwrap_or_else(|| {
                panic!(
                    "BUG: rule '{}' rule reference read before evaluation",
                    path.rule
                )
            })
    }

    pub(crate) fn now(&self) -> &LiteralValue {
        &self.now
    }

    /// Planned schema type for displaying a data path value.
    ///
    /// Binding priority: schema/`as` > settled > fill > suggest > first declared.
    /// Reuses the plan's [`Arc`] when no binding overrides the unit.
    #[must_use]
    pub(crate) fn data_display_type(
        &self,
        plan: &ExecutionPlan,
        path: &DataPath,
    ) -> Arc<LemmaType> {
        let def = plan
            .data
            .get(path)
            .expect("BUG: data path leaf missing from plan.data");
        let schema = def
            .resolved_type_arc()
            .expect("BUG: data path leaf missing schema type");
        let settled = self.data_slot(plan, path).and_then(OperationResult::value);
        crate::planning::semantics::lemma_type_arc_with_display_binding(
            schema,
            settled,
            def.bound_fill(),
            def.bound_suggestion(),
        )
    }

    /// Rule result type for display.
    ///
    /// Binding priority: `as` on planned type > settled > fill > suggest on a data-path leaf >
    /// first declared. Reuses the planned [`Arc`] when no binding overrides the unit.
    #[must_use]
    pub(crate) fn rule_result_type(
        &self,
        plan: &ExecutionPlan,
        rule: &crate::planning::execution_plan::ExecutableRule,
    ) -> Arc<LemmaType> {
        let planned = &rule.rule_type;
        if planned.measure_binding_unit.is_some() {
            return Arc::clone(planned);
        }
        let settled = self
            .rule_values
            .get(plan.rule_index(&rule.path).index())
            .and_then(|slot| slot.as_ref())
            .and_then(OperationResult::value);
        match &plan.normal_form(rule.normal_form).kind {
            crate::planning::normalize::NormalFormKind::Leaf(
                crate::planning::normalize::LeafKind::DataPath(path),
            ) => {
                let def = plan
                    .data
                    .get(path)
                    .expect("BUG: rule DataPath leaf missing from plan.data");
                crate::planning::semantics::lemma_type_arc_with_display_binding(
                    planned,
                    settled,
                    def.bound_fill(),
                    def.bound_suggestion(),
                )
            }
            _ => crate::planning::semantics::lemma_type_arc_with_display_binding(
                planned, settled, None, None,
            ),
        }
    }

    /// Slot for a data path's leaf cell.
    pub(crate) fn data_slot(
        &self,
        plan: &ExecutionPlan,
        data_path: &DataPath,
    ) -> Option<&OperationResult> {
        let leaf = *plan
            .data_leaf
            .get(data_path)
            .unwrap_or_else(|| panic!("BUG: data path '{data_path}' has no data_leaf entry"));
        self.values[leaf.index()].as_ref()
    }

    pub(crate) fn missing_data_suggestion(&self, data_path: &DataPath) -> Option<String> {
        closest_ignored_key(&data_path.input_key(), &self.ignored_unknown)
    }

    /// Whether this evaluation has a value or a veto for `data_path`.
    fn is_data_bound(&self, plan: &ExecutionPlan, data_path: &DataPath) -> bool {
        self.data_slot(plan, data_path).is_some()
    }

    /// Promptable data paths this rule still needs, in evaluation / decision-tree order.
    ///
    /// First key is the next fact the live tree needs. Returns immediately when the run
    /// data bound every promptable path: no rule can report missing data. Otherwise walks
    /// from `rule_root` deriving liveness from filled condition/scrutinee slots, maps each
    /// leaf through [`ExecutionPlan::promptable_data_path`], and keeps unbound keys.
    pub(crate) fn missing_data_for_rule(
        &self,
        plan: &ExecutionPlan,
        rule_root: NormalFormId,
    ) -> Vec<String> {
        if !self.any_promptable_data_unbound {
            return Vec::new();
        }
        let reachable = reachable_data_paths(plan, rule_root, &self.values);
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for path in &reachable {
            let Some(promptable) = plan.promptable_data_path(path) else {
                continue;
            };
            if self.is_data_bound(plan, promptable) {
                continue;
            }
            let key = promptable.input_key();
            if seen.insert(key.clone()) {
                out.push(key);
            }
        }
        out
    }
}

/// Evaluates Lemma rules within their spec context
#[derive(Default)]
pub(crate) struct Evaluator;

impl Evaluator {
    /// Evaluate an execution plan: walk each requested local rule's NormalForm,
    /// then report requested results.
    ///
    /// Rule references are evaluation boundaries: a dependency's value is read from
    /// [`EvaluationContext::rule_values`], never by re-entering its body.
    /// Unbound live inputs are reported per rule as `missing_data` (reachable
    /// under control decisions derived from filled slots, intersected with
    /// unbound promptable paths). When `explain` is true the walk is exhaustive
    /// and every rule with a value is narrated in plan order
    /// ([`narration::narrate_rule`]) before the response is built.
    pub(crate) fn evaluate(
        &self,
        plan: &ExecutionPlan,
        run_data: &RunData,
        now: LiteralValue,
        response_rules: &std::collections::HashSet<RulePath>,
        explain: bool,
    ) -> Response {
        let effective = match &now.value {
            ValueKind::Date(date) => date.to_string(),
            other => panic!("BUG: evaluation now must be a date, got {other:?}"),
        };
        let mut context = EvaluationContext::new(plan, run_data, now, explain);

        let mut response = Response {
            spec_name: plan.spec_name.clone(),
            effective,
            // Set by `Engine::run` after evaluation from the plan's cached
            // version window (`ExecutionPlan::effective_from` / `effective_to`).
            spec_effective_from: None,
            spec_effective_to: None,
            results: IndexMap::new(),
        };

        let requested: Vec<&crate::planning::execution_plan::ExecutableRule> = plan
            .rules
            .values()
            .filter(|rule| response_rules.contains(&rule.path))
            .collect();

        for exec_rule in &requested {
            tree::ensure_rule_values(plan.rule_index(&exec_rule.path), plan, &mut context);
        }

        if explain {
            for (index, rule) in plan.rules.values().enumerate() {
                if context.rule_values[index].is_some() {
                    narration::narrate_rule(rule, plan, &mut context);
                }
            }
        }

        for exec_rule in requested {
            let result = context.rule_value(plan, &exec_rule.path).clone();
            let rule_type = context.rule_result_type(plan, exec_rule);

            let explanation = explain.then(|| {
                narration::explanation_for(
                    exec_rule,
                    plan,
                    &context,
                    &result,
                    Arc::clone(&rule_type),
                )
            });

            let missing_data = match &result {
                OperationResult::Veto(VetoType::MissingData { .. }) => {
                    context.missing_data_for_rule(plan, exec_rule.normal_form)
                }
                _ => Vec::new(),
            };

            response.add_result(RuleResult::from_operation_result(
                EvaluatedRule {
                    name: exec_rule.path.input_key(),
                    path: exec_rule.path.clone(),
                    source_location: exec_rule.source.clone(),
                    rule_type: Arc::clone(&rule_type),
                },
                &result,
                rule_type.as_ref(),
                &plan.family_units,
                explanation,
                missing_data,
            ));
        }

        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::ast::DateTimeValue;
    use crate::Engine;

    #[test]
    fn reference_runtime_value_carries_resolved_type_not_target_type() {
        let code = r#"
spec inner
data slot: number -> minimum 0 -> maximum 100

spec source_spec
data v: 5

spec outer
uses i: inner
  -> with slot: src.v
uses src: source_spec
rule r: i.slot
"#;
        let mut engine = Engine::new();
        engine
            .load([(
                crate::SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from(
                    "ref_invariant.lemma",
                ))),
                code.to_string(),
            )])
            .expect("must load");

        let plan_basis = engine
            .plans
            .get_plans(None, "outer")
            .and_then(|plans| plans.values().next())
            .expect("must plan");

        let reference_path = plan_basis
            .data
            .iter()
            .find_map(|(path, def)| match def {
                DataDefinition::Reference { .. } => Some(path.clone()),
                _ => None,
            })
            .expect("plan must contain the reference for `i.slot`");

        let resolved_type = match plan_basis.data.get(&reference_path).expect("entry exists") {
            DataDefinition::Reference { resolved_type, .. } => Arc::clone(resolved_type),
            _ => unreachable!("filter above kept only Reference entries"),
        };

        let run_data = RunData::default();

        let now = DateTimeValue::now();
        let now_lit = LiteralValue {
            value: crate::planning::semantics::ValueKind::Date(
                crate::planning::semantics::date_time_to_semantic(&now),
            ),
        };
        let context = EvaluationContext::new(plan_basis, &run_data, now_lit, false);

        let stored = context
            .data_slot(plan_basis, &reference_path)
            .expect("EvaluationContext must populate reference path with the copied value");

        let OperationResult::Value(value) = stored else {
            panic!("reference path must hold a value, got {stored:?}");
        };

        // Type lives on DataDefinition::Reference.resolved_type, not LiteralValue.
        assert!(
            matches!(
                resolved_type.specifications,
                crate::planning::semantics::TypeSpecification::Number {
                    minimum: Some(_),
                    maximum: Some(_),
                    ..
                }
            ),
            "reference resolved_type must keep LHS constraints, got {:?}",
            resolved_type.specifications
        );
        assert!(
            matches!(
                value.value,
                crate::planning::semantics::ValueKind::Number(_)
            ),
            "stored value must be the copied number, got {:?}",
            value.value
        );
    }
}
