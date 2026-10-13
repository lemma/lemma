//! Spec-valued rules: candidate instances and field reads on them.
//!
//! A `uses` alias used as a value is one instance. A rule that returns that
//! alias (or another spec-valued rule) yields the union of those instances.
//! `rule.field` reads `field` on whichever instance the rule yields.

use crate::parsing::arena::{ExprId, ExprRef};
use crate::parsing::source::Source;
use crate::planning::execution_plan::ExecutionPlan;
use crate::planning::graph::peel_rule_from;
use crate::planning::normalize::lookup_data_definition;
use crate::planning::semantics::{
    DataDefinition, DataPath, Expression, ExpressionKind, PathSegment, RulePath, SpecIdentity,
    SpecInstance, SpecMemberEnd, SpecMemberTarget, SpecValue,
};
use indexmap::IndexMap;
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
    expression: ExprRef<'_, ExpressionKind>,
    rule_candidates: &HashMap<RulePath, Vec<SpecInstance>>,
    data: &IndexMap<DataPath, DataDefinition>,
    templates: &BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    rule_presence: &impl Fn(&RulePath) -> RulePresence,
) -> Vec<SpecInstance> {
    let mut members = Vec::new();
    let mut base = expression;
    while let ExpressionKind::SpecMember {
        base: child,
        path,
        name,
        ..
    } = base.kind()
    {
        members.push((path, name));
        base = base.child(*child);
    }
    let mut candidates = match base.kind() {
        ExpressionKind::Literal(literal) => match &literal.value {
            crate::planning::semantics::ValueKind::Spec(SpecValue::Fixed(instance)) => {
                vec![instance.clone()]
            }
            _ => Vec::new(),
        },
        ExpressionKind::RulePath(path) => {
            linked_candidates(rule_candidates, data, templates, path, rule_presence)
        }
        _ => Vec::new(),
    };
    while let Some((path, name)) = members.pop() {
        candidates = member_candidates(
            candidates,
            rule_candidates,
            data,
            templates,
            path,
            name,
            rule_presence,
        );
    }
    candidates
}

/// Instances `bases.path.name` can yield, given the candidates of its base.
fn member_candidates(
    bases: Vec<SpecInstance>,
    rule_candidates: &HashMap<RulePath, Vec<SpecInstance>>,
    data: &IndexMap<DataPath, DataDefinition>,
    templates: &BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    path: &[String],
    name: &str,
    rule_presence: &impl Fn(&RulePath) -> RulePresence,
) -> Vec<SpecInstance> {
    let mut out = Vec::new();
    for base_instance in bases {
        let Some(target) = resolve_member(data, templates, &base_instance.prefix, path, name)
        else {
            continue;
        };
        match target.end {
            SpecMemberEnd::Instance { instance, .. } => push_unique(&mut out, instance),
            SpecMemberEnd::Rule(rule) => {
                for instance in
                    linked_candidates(rule_candidates, data, templates, &rule, rule_presence)
                {
                    push_unique(&mut out, instance);
                }
            }
            SpecMemberEnd::Data(_) => {}
        }
    }
    out
}

fn linked_candidates(
    rule_candidates: &HashMap<RulePath, Vec<SpecInstance>>,
    data: &IndexMap<DataPath, DataDefinition>,
    templates: &BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    rule: &RulePath,
    rule_presence: &impl Fn(&RulePath) -> RulePresence,
) -> Vec<SpecInstance> {
    if let Some(list) = rule_candidates.get(rule) {
        return list.clone();
    }
    match rule_presence(rule) {
        RulePresence::Dropped => Vec::new(),
        RulePresence::Present if !rule.segments.is_empty() => {
            template_rule_candidates(data, templates, rule)
        }
        RulePresence::Present => {
            panic!("BUG: rule '{rule}' is referenced but has no candidate list")
        }
        RulePresence::Absent => panic!("BUG: rule '{rule}' neither in graph nor dropped"),
    }
}

/// Candidates of named-`uses` rule `rule` (non-empty segments): the template plan's own
/// list, each instance re-qualified under `rule`'s hops.
fn template_rule_candidates(
    data: &IndexMap<DataPath, DataDefinition>,
    templates: &BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    rule: &RulePath,
) -> Vec<SpecInstance> {
    let (template, local) = peel_rule_from(data, templates, rule)
        .unwrap_or_else(|| panic!("BUG: named-`uses` rule '{rule}' does not peel"));
    template
        .rule_candidates
        .get(local)
        .unwrap_or_else(|| {
            panic!(
                "BUG: template '{}' rule '{local}' has no candidate list",
                template.spec_name
            )
        })
        .iter()
        .map(|instance| SpecInstance {
            prefix: rule
                .segments
                .iter()
                .chain(instance.prefix.iter())
                .cloned()
                .collect(),
        })
        .collect()
}

fn instance_spec_name(instance: &SpecInstance) -> &str {
    instance
        .prefix
        .last()
        .unwrap_or_else(|| panic!("BUG: spec instance '{instance}' has no uses hop"))
        .spec
        .as_str()
}

fn missing_candidate_list(
    data: &IndexMap<DataPath, DataDefinition>,
    templates: &BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    rule: &RulePath,
    rule_presence: &impl Fn(&RulePath) -> RulePresence,
) {
    match rule_presence(rule) {
        RulePresence::Dropped => {}
        RulePresence::Present if !rule.segments.is_empty() => {
            template_rule_candidates(data, templates, rule);
        }
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
    expression: ExprRef<'_, ExpressionKind>,
    rule_candidates: &HashMap<RulePath, Vec<SpecInstance>>,
    data: &IndexMap<DataPath, DataDefinition>,
    templates: &BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    rule_presence: &impl Fn(&RulePath) -> RulePresence,
) {
    let mut pending = vec![expression];
    while let Some(node) = pending.pop() {
        match node.kind() {
            ExpressionKind::RulePath(path) => {
                if !rule_candidates.contains_key(path) {
                    missing_candidate_list(data, templates, path, rule_presence);
                }
            }
            ExpressionKind::SpecMember {
                base, path, name, ..
            } => {
                let base_ref = node.child(*base);
                pending.push(base_ref);
                if let ExpressionKind::RulePath(rule) = base_ref.kind() {
                    if rule.segments.is_empty() && !rule_candidates.contains_key(rule) {
                        continue;
                    }
                }
                for base_instance in
                    expression_candidates(base_ref, rule_candidates, data, templates, rule_presence)
                {
                    let Some(target) =
                        resolve_member(data, templates, &base_instance.prefix, path, name)
                    else {
                        continue;
                    };
                    if let SpecMemberEnd::Rule(rule) = &target.end {
                        if !rule_candidates.contains_key(rule) {
                            missing_candidate_list(data, templates, rule, rule_presence);
                        }
                    }
                }
            }
            _ => node.push_children(&mut pending),
        }
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
    templates: &BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    prefix: &[PathSegment],
    path: &[String],
    name: &str,
) -> Option<SpecMemberTarget> {
    let instance = SpecInstance {
        prefix: prefix.to_vec(),
    };
    let mut segments = prefix.to_vec();
    for hop in path {
        let import = import_at(data, templates, &segments, hop)?;
        segments.push(import);
    }
    let data_path = DataPath {
        segments: segments.clone(),
        data: name.to_string(),
    };
    if let Some(definition) = lookup_data_definition(data, templates, &data_path) {
        return match definition {
            DataDefinition::Import { identity, .. } => {
                let mut nested = segments;
                nested.push(PathSegment {
                    uses: name.to_string(),
                    repository: identity.repository.clone(),
                    spec: identity.spec.clone(),
                });
                Some(SpecMemberTarget {
                    instance: instance.clone(),
                    end: SpecMemberEnd::Instance {
                        instance: SpecInstance { prefix: nested },
                        identity: identity.clone(),
                    },
                })
            }
            DataDefinition::Value { .. }
            | DataDefinition::TypeDeclaration { .. }
            | DataDefinition::InstanceList { .. }
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
    if peel_rule_from(data, templates, &rule_path).is_some() {
        return Some(SpecMemberTarget {
            instance,
            end: SpecMemberEnd::Rule(rule_path),
        });
    }
    None
}

fn import_at(
    data: &IndexMap<DataPath, DataDefinition>,
    templates: &BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    segments: &[PathSegment],
    hop: &str,
) -> Option<PathSegment> {
    let path = DataPath {
        segments: segments.to_vec(),
        data: hop.to_string(),
    };
    let definition = lookup_data_definition(data, templates, &path)?;
    match definition {
        DataDefinition::Import { identity, .. } => Some(PathSegment {
            uses: hop.to_string(),
            repository: identity.repository.clone(),
            spec: identity.spec.clone(),
        }),
        DataDefinition::Value { .. }
        | DataDefinition::TypeDeclaration { .. }
        | DataDefinition::InstanceList { .. }
        | DataDefinition::Reference { .. } => None,
    }
}

/// True when `rule` lives on `instance` or on a nested `uses` of it.
pub(crate) fn rule_under_instance(rule: &RulePath, instance: &SpecInstance) -> bool {
    let prefix = &instance.prefix;
    rule.segments.len() >= prefix.len() && rule.segments[..prefix.len()] == prefix[..]
}

/// Fill [`ExpressionKind::SpecMember::targets`] and return rule dependencies of those targets.
///
/// Member nodes are resolved bottom-up, so a member's base is resolved before the member.
/// Targets are collected first and written once the tree is no longer borrowed.
pub(crate) fn fill_spec_members(
    expression: &mut Expression,
    rule_candidates: &HashMap<RulePath, Vec<SpecInstance>>,
    data: &IndexMap<DataPath, DataDefinition>,
    templates: &BTreeMap<SpecIdentity, Arc<ExecutionPlan>>,
    rule_presence: &impl Fn(&RulePath) -> RulePresence,
) -> (BTreeSet<RulePath>, Vec<(Option<Source>, String)>) {
    let mut deps = BTreeSet::new();
    let mut errors = Vec::new();
    let mut filled: Vec<(ExprId, Vec<SpecMemberTarget>)> = Vec::new();
    for id in expression.table.ids() {
        let node = expression.at(id);
        let ExpressionKind::SpecMember {
            base, path, name, ..
        } = node.kind()
        else {
            continue;
        };
        let source = node.source();
        let base_ref = node.child(*base);
        if let ExpressionKind::RulePath(rule) = base_ref.kind() {
            if !rule_candidates.contains_key(rule) {
                match rule_presence(rule) {
                    RulePresence::Dropped => continue,
                    RulePresence::Present if !rule.segments.is_empty() => {}
                    RulePresence::Present => {
                        panic!("BUG: spec member base '{rule}' has no candidate list")
                    }
                    RulePresence::Absent => {
                        panic!("BUG: spec member base '{rule}' neither in graph nor dropped")
                    }
                }
            }
        }
        let bases =
            expression_candidates(base_ref, rule_candidates, data, templates, rule_presence);
        let mut resolved = Vec::new();
        for base_instance in &bases {
            match resolve_member(data, templates, &base_instance.prefix, path, name) {
                Some(target) => match &target.end {
                    SpecMemberEnd::Rule(rule) => match rule_presence(rule) {
                        RulePresence::Present => {
                            deps.insert(rule.clone());
                            resolved.push(target);
                        }
                        RulePresence::Dropped => {}
                        RulePresence::Absent => {
                            errors.push((
                                source.cloned(),
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
                        source.cloned(),
                        format!(
                            "no data or rule '{name}' on spec '{}'",
                            instance_spec_name(base_instance)
                        ),
                    ));
                }
            }
        }
        filled.push((id, resolved));
    }
    for (id, targets) in filled {
        match expression.table.kind_mut(id) {
            ExpressionKind::SpecMember { targets: slot, .. } => *slot = targets,
            _ => unreachable!("BUG: filled node is not a spec member"),
        }
    }
    (deps, errors)
}
