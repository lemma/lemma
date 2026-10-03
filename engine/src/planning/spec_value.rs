//! Spec-valued rules: candidate instances and field reads on them.
//!
//! A `uses` alias used as a value is one instance. A rule that returns that
//! alias (or another spec-valued rule) yields the union of those instances.
//! `rule.field` reads `field` on whichever instance the rule yields.

use crate::parsing::source::Source;
use crate::planning::semantics::{
    DataDefinition, DataPath, Expression, ExpressionKind, PathSegment, RulePath, SpecIdentity,
    SpecInstance, SpecMemberEnd, SpecMemberTarget,
};
use indexmap::IndexMap;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

/// Whether a rule node is in the graph, was dropped during conversion, or was never defined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RulePresence {
    Present,
    Dropped,
    Absent,
}

/// Instances a spec-valued expression can yield. Empty when it is not a spec value.
pub(crate) fn expression_candidates(
    expression: &Expression,
    rule_candidates: &HashMap<RulePath, Vec<SpecInstance>>,
    data: &IndexMap<DataPath, DataDefinition>,
    rule_presence: &impl Fn(&RulePath) -> RulePresence,
) -> Vec<SpecInstance> {
    match &expression.kind {
        ExpressionKind::Literal(literal) => match &literal.value {
            crate::planning::semantics::ValueKind::Spec(instance) => vec![instance.clone()],
            _ => Vec::new(),
        },
        ExpressionKind::RulePath(path) => linked_candidates(rule_candidates, path, rule_presence),
        ExpressionKind::SpecMember {
            base, path, name, ..
        } => {
            let bases = linked_candidates(rule_candidates, base, rule_presence);
            let mut out = Vec::new();
            for base_instance in bases {
                let Some(target) = resolve_member(data, &base_instance.prefix, path, name) else {
                    continue;
                };
                match target.end {
                    SpecMemberEnd::Instance { instance, .. } => push_unique(&mut out, instance),
                    SpecMemberEnd::Rule(rule) => {
                        if let Some(nested) = rule_candidates.get(&rule) {
                            for instance in nested {
                                push_unique(&mut out, instance.clone());
                            }
                        }
                    }
                    SpecMemberEnd::Data(_) => {}
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

fn linked_candidates(
    rule_candidates: &HashMap<RulePath, Vec<SpecInstance>>,
    rule: &RulePath,
    rule_presence: &impl Fn(&RulePath) -> RulePresence,
) -> Vec<SpecInstance> {
    if let Some(list) = rule_candidates.get(rule) {
        return list.clone();
    }
    match rule_presence(rule) {
        RulePresence::Dropped => Vec::new(),
        RulePresence::Present => {
            panic!("BUG: rule '{rule}' is referenced but has no candidate list")
        }
        RulePresence::Absent => panic!("BUG: rule '{rule}' neither in graph nor dropped"),
    }
}

fn instance_spec_name(instance: &SpecInstance) -> &str {
    instance
        .prefix
        .last()
        .unwrap_or_else(|| panic!("BUG: spec instance '{instance}' has no uses hop"))
        .spec
        .as_str()
}

fn missing_candidate_list(rule: &RulePath, rule_presence: &impl Fn(&RulePath) -> RulePresence) {
    match rule_presence(rule) {
        RulePresence::Dropped => {}
        RulePresence::Present => {
            panic!("BUG: rule '{rule}' is referenced but has no candidate list after linking")
        }
        RulePresence::Absent => {
            panic!("BUG: rule '{rule}' neither in graph nor dropped")
        }
    }
}

/// After linking stops adding edges, every referenced rule has a candidate list.
/// An empty list means the rule is not spec-valued. A missing list is a broken edge.
pub(crate) fn assert_expression_candidates_linked(
    expression: &Expression,
    rule_candidates: &HashMap<RulePath, Vec<SpecInstance>>,
    data: &IndexMap<DataPath, DataDefinition>,
    rule_presence: &impl Fn(&RulePath) -> RulePresence,
) {
    match &expression.kind {
        ExpressionKind::RulePath(path) => {
            if !rule_candidates.contains_key(path) {
                missing_candidate_list(path, rule_presence);
            }
        }
        ExpressionKind::SpecMember {
            base, path, name, ..
        } => {
            let Some(bases) = rule_candidates.get(base) else {
                missing_candidate_list(base, rule_presence);
                return;
            };
            for base_instance in bases {
                let Some(target) = resolve_member(data, &base_instance.prefix, path, name) else {
                    continue;
                };
                if let SpecMemberEnd::Rule(rule) = &target.end {
                    if !rule_candidates.contains_key(rule) {
                        match rule_presence(rule) {
                            RulePresence::Present => panic!(
                                "BUG: spec member rule '{rule}' has no candidate list after linking"
                            ),
                            RulePresence::Dropped | RulePresence::Absent => {}
                        }
                    }
                }
            }
        }
        ExpressionKind::LogicalAnd(left, right)
        | ExpressionKind::Arithmetic(left, _, right)
        | ExpressionKind::Comparison(left, _, right)
        | ExpressionKind::RangeLiteral(left, right)
        | ExpressionKind::RangeContainment(left, right) => {
            assert_expression_candidates_linked(left, rule_candidates, data, rule_presence);
            assert_expression_candidates_linked(right, rule_candidates, data, rule_presence);
        }
        ExpressionKind::UnitConversion(inner, _)
        | ExpressionKind::LogicalNegation(inner, _)
        | ExpressionKind::MathematicalComputation(_, inner)
        | ExpressionKind::DateRelative(_, inner)
        | ExpressionKind::DateCalendar(_, _, inner)
        | ExpressionKind::PastFutureRange(_, inner)
        | ExpressionKind::RangeBound(_, inner)
        | ExpressionKind::ResultIsVeto(inner) => {
            assert_expression_candidates_linked(inner, rule_candidates, data, rule_presence);
        }
        ExpressionKind::Piecewise(arms) => {
            for (condition, result) in arms {
                assert_expression_candidates_linked(
                    condition,
                    rule_candidates,
                    data,
                    rule_presence,
                );
                assert_expression_candidates_linked(result, rule_candidates, data, rule_presence);
            }
        }
        ExpressionKind::Literal(_)
        | ExpressionKind::DataPath(_)
        | ExpressionKind::Veto(_)
        | ExpressionKind::Now => {}
    }
}

fn push_unique(out: &mut Vec<SpecInstance>, instance: SpecInstance) {
    if !out.iter().any(|existing| existing == &instance) {
        out.push(instance);
    }
}

/// Field `path` + `name` on the instance at `prefix`.
///
/// `path` is `uses` hops inside the instance. `None` when a hop or the final
/// name is not data, a rule, or a `uses` import.
pub(crate) fn resolve_member(
    data: &IndexMap<DataPath, DataDefinition>,
    prefix: &[PathSegment],
    path: &[String],
    name: &str,
) -> Option<SpecMemberTarget> {
    let instance = SpecInstance {
        prefix: prefix.to_vec(),
    };
    let mut segments = prefix.to_vec();
    for hop in path {
        let import = import_at(data, &segments, hop)?;
        segments.push(import);
    }
    let data_path = DataPath {
        segments: segments.clone(),
        data: name.to_string(),
    };
    if let Some(definition) = data.get(&data_path) {
        return match definition {
            DataDefinition::Import {
                target_name,
                repository,
                effective,
                ..
            } => {
                let mut nested = segments;
                nested.push(PathSegment {
                    uses: name.to_string(),
                    repository: repository.clone(),
                    spec: target_name.clone(),
                });
                Some(SpecMemberTarget {
                    instance: instance.clone(),
                    end: SpecMemberEnd::Instance {
                        instance: SpecInstance { prefix: nested },
                        identity: SpecIdentity {
                            repository: repository.clone(),
                            spec: target_name.clone(),
                            effective: effective.clone(),
                        },
                    },
                })
            }
            DataDefinition::Value { .. }
            | DataDefinition::TypeDeclaration { .. }
            | DataDefinition::Reference { .. } => Some(SpecMemberTarget {
                instance,
                end: SpecMemberEnd::Data(data_path),
            }),
        };
    }
    let rule_path = RulePath {
        segments,
        rule: name.to_string(),
    };
    Some(SpecMemberTarget {
        instance,
        end: SpecMemberEnd::Rule(rule_path),
    })
}

fn import_at(
    data: &IndexMap<DataPath, DataDefinition>,
    segments: &[PathSegment],
    hop: &str,
) -> Option<PathSegment> {
    let path = DataPath {
        segments: segments.to_vec(),
        data: hop.to_string(),
    };
    match data.get(&path)? {
        DataDefinition::Import {
            target_name,
            repository,
            ..
        } => Some(PathSegment {
            uses: hop.to_string(),
            repository: repository.clone(),
            spec: target_name.clone(),
        }),
        DataDefinition::Value { .. }
        | DataDefinition::TypeDeclaration { .. }
        | DataDefinition::Reference { .. } => None,
    }
}

/// True when `rule` lives on `instance` or on a nested `uses` of it.
pub(crate) fn rule_under_instance(rule: &RulePath, instance: &SpecInstance) -> bool {
    let prefix = &instance.prefix;
    rule.segments.len() >= prefix.len() && rule.segments[..prefix.len()] == prefix[..]
}

/// Fill [`ExpressionKind::SpecMember::targets`] and return rule dependencies of those targets.
pub(crate) fn fill_spec_members(
    expression: &mut Expression,
    rule_candidates: &HashMap<RulePath, Vec<SpecInstance>>,
    data: &IndexMap<DataPath, DataDefinition>,
    rule_presence: &impl Fn(&RulePath) -> RulePresence,
) -> (BTreeSet<RulePath>, Vec<(Option<Source>, String)>) {
    let mut deps = BTreeSet::new();
    let mut errors = Vec::new();
    fill_spec_members_inner(
        expression,
        rule_candidates,
        data,
        rule_presence,
        &mut deps,
        &mut errors,
    );
    (deps, errors)
}

fn fill_spec_members_inner(
    expression: &mut Expression,
    rule_candidates: &HashMap<RulePath, Vec<SpecInstance>>,
    data: &IndexMap<DataPath, DataDefinition>,
    rule_presence: &impl Fn(&RulePath) -> RulePresence,
    deps: &mut BTreeSet<RulePath>,
    errors: &mut Vec<(Option<Source>, String)>,
) {
    match &mut expression.kind {
        ExpressionKind::SpecMember {
            base,
            path,
            name,
            targets,
        } => {
            let Some(bases) = rule_candidates.get(base) else {
                match rule_presence(base) {
                    RulePresence::Dropped => return,
                    RulePresence::Present => {
                        panic!("BUG: spec member base '{base}' has no candidate list")
                    }
                    RulePresence::Absent => {
                        panic!("BUG: spec member base '{base}' neither in graph nor dropped")
                    }
                }
            };
            if bases.is_empty() {
                errors.push((
                    expression.source_location.clone(),
                    format!("rule '{base}' does not return a spec"),
                ));
                return;
            }
            let path = path.clone();
            let name = name.clone();
            let mut resolved = Vec::new();
            for base_instance in bases {
                match resolve_member(data, &base_instance.prefix, &path, &name) {
                    Some(target) => match &target.end {
                        SpecMemberEnd::Rule(rule) => match rule_presence(rule) {
                            RulePresence::Present => {
                                deps.insert(rule.clone());
                                resolved.push(target);
                            }
                            RulePresence::Dropped => {}
                            RulePresence::Absent => {
                                errors.push((
                                    expression.source_location.clone(),
                                    format!(
                                        "no data or rule '{name}' on spec '{}'",
                                        instance_spec_name(base_instance)
                                    ),
                                ));
                            }
                        },
                        SpecMemberEnd::Data(_) | SpecMemberEnd::Instance { .. } => {
                            resolved.push(target);
                        }
                    },
                    None => {
                        errors.push((
                            expression.source_location.clone(),
                            format!(
                                "no data or rule '{name}' on spec '{}'",
                                instance_spec_name(base_instance)
                            ),
                        ));
                    }
                }
            }
            *targets = resolved;
        }
        ExpressionKind::LogicalAnd(left, right)
        | ExpressionKind::Arithmetic(left, _, right)
        | ExpressionKind::Comparison(left, _, right)
        | ExpressionKind::RangeLiteral(left, right)
        | ExpressionKind::RangeContainment(left, right) => {
            fill_spec_members_inner(
                Arc::make_mut(left),
                rule_candidates,
                data,
                rule_presence,
                deps,
                errors,
            );
            fill_spec_members_inner(
                Arc::make_mut(right),
                rule_candidates,
                data,
                rule_presence,
                deps,
                errors,
            );
        }
        ExpressionKind::UnitConversion(inner, _)
        | ExpressionKind::LogicalNegation(inner, _)
        | ExpressionKind::MathematicalComputation(_, inner)
        | ExpressionKind::DateRelative(_, inner)
        | ExpressionKind::DateCalendar(_, _, inner)
        | ExpressionKind::PastFutureRange(_, inner)
        | ExpressionKind::RangeBound(_, inner)
        | ExpressionKind::ResultIsVeto(inner) => {
            fill_spec_members_inner(
                Arc::make_mut(inner),
                rule_candidates,
                data,
                rule_presence,
                deps,
                errors,
            );
        }
        ExpressionKind::Piecewise(arms) => {
            for (condition, result) in arms {
                fill_spec_members_inner(
                    Arc::make_mut(condition),
                    rule_candidates,
                    data,
                    rule_presence,
                    deps,
                    errors,
                );
                fill_spec_members_inner(
                    Arc::make_mut(result),
                    rule_candidates,
                    data,
                    rule_presence,
                    deps,
                    errors,
                );
            }
        }
        ExpressionKind::Literal(_)
        | ExpressionKind::DataPath(_)
        | ExpressionKind::RulePath(_)
        | ExpressionKind::Veto(_)
        | ExpressionKind::Now => {}
    }
}
