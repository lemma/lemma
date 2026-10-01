//! Structural quality analysis over loaded specs.
//!
//! Advisory only: never affects planning or evaluation. Distinct from [`Error`] /
//! Veto / panic.

use crate::error::EngineErrorSource;
use crate::literals::{BooleanValue, Value};
use crate::parsing::ast::{
    ComparisonComputation, DataValue, Expression, ExpressionKind, LemmaData, LemmaRule, LemmaSpec,
    NegationType, ParentType, PrimitiveKind, Span, TypeConstraintCommand,
};
use crate::parsing::source::{Source, SourceType};
use crate::Engine;
use serde::{Deserialize, Serialize};
use std::fmt;

/// One structural quality Recommendation from [`Engine::quality`].
///
/// Wire JSON uses `source` ([`EngineErrorSource`]); runtime keeps [`Source`] as
/// `source_location` for Display and analysis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recommendation {
    /// Advisory prose only. Does not encode which temporal slice was analyzed.
    pub message: String,
    pub repository: Option<String>,
    pub spec: String,
    pub effective_from: Option<crate::parsing::ast::DateTimeValue>,
    pub source_location: Source,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RecommendationWire {
    message: String,
    repository: Option<String>,
    spec: String,
    effective_from: Option<crate::parsing::ast::DateTimeValue>,
    source: EngineErrorSource,
}

impl From<&Recommendation> for RecommendationWire {
    fn from(r: &Recommendation) -> Self {
        Self {
            message: r.message.clone(),
            repository: r.repository.clone(),
            spec: r.spec.clone(),
            effective_from: r.effective_from.clone(),
            source: EngineErrorSource::from(&r.source_location),
        }
    }
}

impl From<RecommendationWire> for Recommendation {
    fn from(w: RecommendationWire) -> Self {
        let source_type =
            SourceType::from_binding_label(&w.source.attribute).unwrap_or(SourceType::Volatile);
        let end = w.source.length;
        Self {
            message: w.message,
            repository: w.repository,
            spec: w.spec,
            effective_from: w.effective_from,
            source_location: Source::new(
                source_type,
                Span {
                    start: 0,
                    end,
                    line: w.source.line,
                    col: w.source.column,
                },
            ),
        }
    }
}

impl Serialize for Recommendation {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        RecommendationWire::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Recommendation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        RecommendationWire::deserialize(deserializer).map(Self::from)
    }
}

impl fmt::Display for Recommendation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "In spec '{}'", self.spec)?;
        match &self.effective_from {
            Some(dt) => write!(f, " (effective from {dt})")?,
            None => write!(f, " (effective from beginning)")?,
        }
        write!(f, ": {}", self.message)?;
        write!(
            f,
            " at {}:{}:{}",
            self.source_location.source_type,
            self.source_location.span.line,
            self.source_location.span.col
        )
    }
}

impl Engine {
    /// Structural quality Recommendations across all loaded specs and their relationships.
    ///
    /// Advisory only: never affects planning or evaluation. Skips dependency
    /// repositories (embedded stdlib and `@owner/repo` imports).
    #[must_use]
    pub fn quality(&self) -> Vec<Recommendation> {
        let mut out = Vec::new();
        for (repository, inner) in self.context.repositories().iter() {
            if repository.dependency.is_some() {
                continue;
            }
            let repo_name = repository.name.clone();
            for (_, spec_set) in inner.iter() {
                for spec in spec_set.iter_specs() {
                    analyze_spec(repo_name.clone(), spec, &mut out);
                }
            }
        }
        out.sort_by(|a, b| {
            (
                a.repository.as_deref().unwrap_or(""),
                a.spec.as_str(),
                a.source_location.span.line,
                a.source_location.span.col,
                a.source_location.span.start,
            )
                .cmp(&(
                    b.repository.as_deref().unwrap_or(""),
                    b.spec.as_str(),
                    b.source_location.span.line,
                    b.source_location.span.col,
                    b.source_location.span.start,
                ))
        });
        out
    }
}

fn analyze_spec(repository: Option<String>, spec: &LemmaSpec, out: &mut Vec<Recommendation>) {
    for data in &spec.data {
        analyze_data(repository.clone(), spec, data, out);
    }
    for rule in &spec.rules {
        analyze_rule(repository.clone(), spec, rule, out);
    }
}

fn analyze_import(
    repository: Option<String>,
    spec: &LemmaSpec,
    bindings: &[crate::parsing::ast::UsesBinding],
    out: &mut Vec<Recommendation>,
) {
    for binding in bindings {
        if !binding.deprecated_standalone_with {
            continue;
        }
        out.push(Recommendation {
            message: "Standalone `with alias.field: …` is deprecated; nest under the matching `uses` line as `  -> with field: …`.".to_string(),
            repository: repository.clone(),
            spec: spec.name.clone(),
            effective_from: spec.effective_from.to_option(),
            source_location: binding.source_location.clone(),
        });
    }
}

fn analyze_data(
    repository: Option<String>,
    spec: &LemmaSpec,
    data: &LemmaData,
    out: &mut Vec<Recommendation>,
) {
    if let DataValue::Import { bindings, .. } = &data.value {
        analyze_import(repository.clone(), spec, bindings, out);
        return;
    }

    let DataValue::Definition {
        base,
        constraints,
        value: _,
    } = &data.value
    else {
        return;
    };

    let name = data.reference.name.clone();
    let constraints = constraints.as_deref().unwrap_or(&[]);
    let has_help = constraints
        .iter()
        .any(|row| matches!(row.command, TypeConstraintCommand::Help));
    let has_option = constraints.iter().any(|row| {
        matches!(
            row.command,
            TypeConstraintCommand::Option | TypeConstraintCommand::Options
        )
    });
    let has_minimum = constraints
        .iter()
        .any(|row| matches!(row.command, TypeConstraintCommand::Minimum));
    let has_maximum = constraints
        .iter()
        .any(|row| matches!(row.command, TypeConstraintCommand::Maximum));

    for row in constraints {
        if !row.deprecated_without_colon {
            continue;
        }
        out.push(Recommendation {
            message: "`-> unit name value` is deprecated; use `-> unit name: value`.".to_string(),
            repository: repository.clone(),
            spec: spec.name.clone(),
            effective_from: spec.effective_from.to_option(),
            source_location: row.source_location.clone(),
        });
    }

    if !has_help {
        out.push(Recommendation {
            message: format!(
                "`{name}` has no `-> help`. Consider adding a message to help users understand this data."
            ),
            repository: repository.clone(),
            spec: spec.name.clone(),
            effective_from: spec.effective_from.to_option(),
            source_location: data.source_location.clone(),
        });
    }

    if is_primitive_bounded_quantity(base.as_ref()) && (!has_minimum || !has_maximum) {
        let gap = match (has_minimum, has_maximum) {
            (false, false) => "no `-> minimum` or `-> maximum`",
            (true, false) => "no `-> maximum`",
            (false, true) => "no `-> minimum`",
            (true, true) => unreachable!("BUG: both bounds present but entered missing-bounds arm"),
        };
        out.push(Recommendation {
            message: format!(
                "`{name}` has {gap}. Consider adding bounds so out-of-range values are rejected."
            ),
            repository: repository.clone(),
            spec: spec.name.clone(),
            effective_from: spec.effective_from.to_option(),
            source_location: data.source_location.clone(),
        });
    }

    if is_primitive_text(base.as_ref()) && !has_option {
        out.push(Recommendation {
            message: format!(
                "`{name}` accepts any text. Adding `-> option` values allows forms and APIs to offer choices."
            ),
            repository,
            spec: spec.name.clone(),
            effective_from: spec.effective_from.to_option(),
            source_location: data.source_location.clone(),
        });
    }
}

fn unwrap_parent_type(base: Option<&ParentType>) -> Option<&ParentType> {
    match base {
        Some(ParentType::Qualified { inner, .. } | ParentType::Ranged { inner }) => {
            unwrap_parent_type(Some(inner.as_ref()))
        }
        other => other,
    }
}

fn is_primitive_text(base: Option<&ParentType>) -> bool {
    matches!(
        unwrap_parent_type(base),
        Some(ParentType::Primitive {
            primitive: PrimitiveKind::Text
        })
    )
}

fn is_primitive_bounded_quantity(base: Option<&ParentType>) -> bool {
    matches!(
        unwrap_parent_type(base),
        Some(ParentType::Primitive {
            primitive: PrimitiveKind::Number | PrimitiveKind::Measure | PrimitiveKind::Ratio
        })
    )
}

fn analyze_rule(
    repository: Option<String>,
    spec: &LemmaSpec,
    rule: &LemmaRule,
    out: &mut Vec<Recommendation>,
) {
    walk_expr_for_ambiguous_and(repository.clone(), spec, rule, &rule.expression, out);
    for unless in &rule.unless_clauses {
        walk_expr_for_ambiguous_and(repository.clone(), spec, rule, &unless.condition, out);
        walk_expr_for_ambiguous_and(repository.clone(), spec, rule, &unless.result, out);
    }

    analyze_redundant_boolean_default_unless(repository.clone(), spec, rule, out);

    if is_boolean_literal(&rule.expression)
        && !rule.unless_clauses.is_empty()
        && rule
            .unless_clauses
            .iter()
            .all(|u| matches!(u.result.kind, ExpressionKind::Veto(_)))
    {
        out.push(Recommendation {
            message: format!(
                "`{}` treats a yes/no outcome as veto. Consider `false` or `no` when denying — veto means there is no answer, and it blocks every rule that depends on this one.",
                rule.name
            ),
            repository,
            spec: spec.name.clone(),
            effective_from: spec.effective_from.to_option(),
            source_location: rule.source_location.clone(),
        });
    }
}

fn boolean_literal_truth(expr: &Expression) -> Option<bool> {
    match &expr.kind {
        ExpressionKind::Literal(Value::Boolean(b)) => Some(bool::from(*b)),
        _ => None,
    }
}

fn analyze_redundant_boolean_default_unless(
    repository: Option<String>,
    spec: &LemmaSpec,
    rule: &LemmaRule,
    out: &mut Vec<Recommendation>,
) {
    if rule.unless_clauses.len() != 1 {
        return;
    }
    let unless = &rule.unless_clauses[0];
    if boolean_literal_truth(&rule.expression) != Some(false) {
        return;
    }
    if boolean_literal_truth(&unless.result) != Some(true) {
        return;
    }
    let condition = format!("{}", unless.condition);
    out.push(Recommendation {
        message: format!(
            "`{}` uses `no` (or `false`) with a single `unless … then yes`. Equivalent to `{condition}`; use a direct expression instead.",
            rule.name
        ),
        repository,
        spec: spec.name.clone(),
        effective_from: spec.effective_from.to_option(),
        source_location: rule.source_location.clone(),
    });
}

fn is_boolean_literal(expr: &Expression) -> bool {
    boolean_literal_truth(expr).is_some()
}

/// Conjunct shape for ambiguous `and` detection (AST only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoolConjunct {
    Bare,
    NotRef,
    IsFalse,
    Other,
}

fn classify_bool_conjunct(expr: &Expression) -> BoolConjunct {
    match &expr.kind {
        ExpressionKind::Reference(_) => BoolConjunct::Bare,
        ExpressionKind::LogicalNegation(inner, NegationType::Not)
            if matches!(inner.kind, ExpressionKind::Reference(_)) =>
        {
            BoolConjunct::NotRef
        }
        ExpressionKind::Comparison(left, ComparisonComputation::Is, right)
            if matches!(left.kind, ExpressionKind::Reference(_))
                && matches!(
                    right.kind,
                    ExpressionKind::Literal(Value::Boolean(BooleanValue::False | BooleanValue::No))
                ) =>
        {
            BoolConjunct::IsFalse
        }
        _ => BoolConjunct::Other,
    }
}

fn is_ambiguous_logical_and(left: &Expression, right: &Expression) -> bool {
    matches!(
        (classify_bool_conjunct(left), classify_bool_conjunct(right)),
        (BoolConjunct::Bare, BoolConjunct::NotRef)
            | (BoolConjunct::NotRef, BoolConjunct::Bare)
            | (BoolConjunct::Bare, BoolConjunct::IsFalse)
            | (BoolConjunct::IsFalse, BoolConjunct::Bare)
    )
}

const AMBIGUOUS_AND_MESSAGE: &str = "Boolean `and` mixes a bare name with `not` / `is false`. Readers misread scope. Prefer parallel probes (`not x and y is true`, or `x is false and y is true`) or parentheses.";

fn walk_expr_for_ambiguous_and(
    repository: Option<String>,
    spec: &LemmaSpec,
    rule: &LemmaRule,
    expr: &Expression,
    out: &mut Vec<Recommendation>,
) {
    match &expr.kind {
        ExpressionKind::LogicalAnd(left, right) => {
            if is_ambiguous_logical_and(left, right) {
                out.push(Recommendation {
                    message: AMBIGUOUS_AND_MESSAGE.to_string(),
                    repository: repository.clone(),
                    spec: spec.name.clone(),
                    effective_from: spec.effective_from.to_option(),
                    source_location: expr
                        .source_location
                        .clone()
                        .unwrap_or_else(|| rule.source_location.clone()),
                });
            }
            walk_expr_for_ambiguous_and(repository.clone(), spec, rule, left, out);
            walk_expr_for_ambiguous_and(repository, spec, rule, right, out);
        }
        ExpressionKind::DateRelative(_, inner)
        | ExpressionKind::PastFutureRange(_, inner)
        | ExpressionKind::UnitConversion(inner, _)
        | ExpressionKind::LogicalNegation(inner, _)
        | ExpressionKind::MathematicalComputation(_, inner)
        | ExpressionKind::RangeBound(_, inner)
        | ExpressionKind::ResultIsVeto(inner) => {
            walk_expr_for_ambiguous_and(repository, spec, rule, inner, out);
        }
        ExpressionKind::DateCalendar(_, _, inner) => {
            walk_expr_for_ambiguous_and(repository, spec, rule, inner, out);
        }
        ExpressionKind::RangeLiteral(left, right)
        | ExpressionKind::RangeContainment(left, right)
        | ExpressionKind::Arithmetic(left, _, right)
        | ExpressionKind::Comparison(left, _, right) => {
            walk_expr_for_ambiguous_and(repository.clone(), spec, rule, left, out);
            walk_expr_for_ambiguous_and(repository, spec, rule, right, out);
        }
        ExpressionKind::Literal(_)
        | ExpressionKind::Reference(_)
        | ExpressionKind::Now
        | ExpressionKind::Veto(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::source::SourceType;

    fn load(code: &str) -> Engine {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("test.lemma"))),
                code.to_string(),
            )])
            .expect("BUG: test source must load");
        engine
    }

    fn effective_display(r: &Recommendation) -> Option<String> {
        r.effective_from.as_ref().map(|d| d.to_string())
    }

    #[test]
    fn origin_without_commentary_does_not_flag_optional_gaps() {
        let engine = load("spec pricing\ndata x: number\nrule y: x\n");
        let recs = engine.quality();
        assert!(
            recs.iter().all(|r| {
                !r.message.contains("commentary")
                    && !r.message.contains("effective date")
                    && !r.message.contains("-> suggest")
            }),
            "optional gaps must not be recommended: {recs:?}"
        );
        let help = recs
            .iter()
            .find(|r| r.message.contains("no `-> help`") && r.message.contains("x"))
            .expect("missing help");
        assert!(
            help.message.contains("Consider adding a message"),
            "got: {}",
            help.message
        );
        assert_eq!(help.spec, "pricing");
        assert_eq!(help.effective_from, None);
    }

    #[test]
    fn clean_spec_has_no_recommendations() {
        let engine = load(
            r#"spec pricing 2026-01-01
"""
Bulk pricing.
"""

data qty: number
  -> minimum 0
  -> maximum 1000000
  -> help "Order quantity."

rule total: qty
"#,
        );
        assert!(engine.quality().is_empty(), "got: {:?}", engine.quality());
    }

    #[test]
    fn deprecated_standalone_with_emits_quality_recommendation() {
        let engine = load(
            r#"spec inner
data x: number

spec outer
uses i: inner
with i.x: 42
rule r: i.x
"#,
        );
        let recs = engine.quality();
        let hit = recs
            .iter()
            .find(|r| r.message.contains("deprecated") && r.spec == "outer")
            .expect("deprecated standalone with must produce quality recommendation");
        assert!(hit.message.contains("-> with"), "got: {}", hit.message);
        assert_eq!(hit.source_location.span.line, 6);
    }

    #[test]
    fn block_uses_binding_has_no_deprecated_recommendation() {
        let engine = load(
            r#"spec inner
data x: number

spec outer
uses i: inner
  -> with x: 42
rule r: i.x
"#,
        );
        assert!(
            engine
                .quality()
                .iter()
                .all(|r| !r.message.contains("deprecated")),
            "block syntax must not flag deprecated: {:?}",
            engine.quality()
        );
    }

    #[test]
    fn number_without_bounds() {
        let engine = load(
            r#"spec pricing 2026-01-01
"""
x
"""

data qty: number
  -> help "Order quantity."
rule total: qty
"#,
        );
        let hit = engine
            .quality()
            .into_iter()
            .find(|r| {
                r.message.contains("no `-> minimum` or `-> maximum`") && r.message.contains("qty")
            })
            .expect("missing bounds");
        assert!(
            hit.message.contains("Consider adding bounds"),
            "got: {}",
            hit.message
        );
    }

    #[test]
    fn number_with_only_minimum_still_flagged() {
        let engine = load(
            r#"spec pricing 2026-01-01
"""
x
"""

data qty: number
  -> minimum 0
  -> help "Order quantity."
rule total: qty
"#,
        );
        assert!(
            engine.quality().iter().any(|r| {
                r.message.contains("no `-> maximum`")
                    && !r.message.contains("no `-> minimum` or")
                    && r.message.contains("qty")
            }),
            "only minimum must flag missing maximum: {:?}",
            engine.quality()
        );
    }

    #[test]
    fn number_with_only_maximum_still_flagged() {
        let engine = load(
            r#"spec pricing 2026-01-01
"""
x
"""

data qty: number
  -> maximum 100
  -> help "Order quantity."
rule total: qty
"#,
        );
        assert!(
            engine.quality().iter().any(|r| {
                r.message.contains("no `-> minimum`")
                    && !r.message.contains("or `-> maximum`")
                    && r.message.contains("qty")
            }),
            "only maximum must flag missing minimum: {:?}",
            engine.quality()
        );
    }

    #[test]
    fn number_with_min_and_max_clean() {
        let engine = load(
            r#"spec pricing 2026-01-01
"""
x
"""

data qty: number
  -> minimum 0
  -> maximum 100
  -> help "Order quantity."
rule total: qty
"#,
        );
        assert!(
            !engine
                .quality()
                .iter()
                .any(|r| r.message.contains("no `-> minimum` or `-> maximum`")),
            "min+max must not flag bounds: {:?}",
            engine.quality()
        );
    }

    #[test]
    fn measure_and_ratio_without_bounds() {
        let engine = load(
            r#"spec pricing 2026-01-01
"""
x
"""

data price: measure
  -> unit eur: 1
  -> help "Unit price."
data discount: ratio
  -> help "Discount rate."
rule total: price
rule rate: discount
"#,
        );
        let recs = engine.quality();
        assert!(
            recs.iter().any(|r| {
                r.message.contains("no `-> minimum` or `-> maximum`") && r.message.contains("price")
            }),
            "measure must flag: {recs:?}"
        );
        assert!(
            recs.iter().any(|r| {
                r.message.contains("no `-> minimum` or `-> maximum`")
                    && r.message.contains("discount")
            }),
            "ratio must flag: {recs:?}"
        );
    }

    #[test]
    fn text_not_flagged_for_bounds() {
        let engine = load(
            r#"spec pricing 2026-01-01
"""
x
"""

data status: text
  -> help "Status."
rule ok: status is "active"
"#,
        );
        assert!(
            !engine
                .quality()
                .iter()
                .any(|r| r.message.contains("no `-> minimum` or `-> maximum`")),
            "text must not get bounds rec: {:?}",
            engine.quality()
        );
    }

    #[test]
    fn data_missing_help() {
        let engine = load(
            r#"spec pricing 2026-01-01
"""
x
"""

data qty: number
rule total: qty
"#,
        );
        let hit = engine
            .quality()
            .into_iter()
            .find(|r| r.message.contains("no `-> help`") && r.message.contains("qty"))
            .expect("missing help");
        assert!(
            hit.message.contains("Consider adding a message"),
            "got: {}",
            hit.message
        );
        assert_eq!(hit.spec, "pricing");
        assert_eq!(effective_display(&hit).as_deref(), Some("2026-01-01"));
        assert!(
            hit.to_string().starts_with(
                "In spec 'pricing' (effective from 2026-01-01): `qty` has no `-> help`"
            ),
            "Display must include effective_from, got: {}",
            hit
        );
        assert!(
            hit.to_string().contains(" at test.lemma:"),
            "Display must append source location, got: {}",
            hit
        );
    }

    #[test]
    fn text_without_options() {
        let engine = load(
            r#"spec pricing 2026-01-01
"""
x
"""

data status: text
  -> help "Status."
rule ok: status is "active"
"#,
        );
        let hit = engine
            .quality()
            .into_iter()
            .find(|r| r.message.contains("accepts any text") && r.message.contains("status"))
            .expect("text without options");
        assert!(
            hit.message.contains("offer choices"),
            "got: {}",
            hit.message
        );
        assert_eq!(hit.spec, "pricing");
        assert_eq!(effective_display(&hit).as_deref(), Some("2026-01-01"));
    }

    #[test]
    fn veto_as_rejection_cascade() {
        let engine = load(
            r#"spec eligibility 2026-01-01
"""
Age gate.
"""

data age: number
  -> help "Customer age."

rule is_eligible: true
  unless age < 18 then veto "Must be 18+"
"#,
        );
        let hit = engine
            .quality()
            .into_iter()
            .find(|r| {
                r.message.contains("treats a yes/no outcome as veto")
                    && r.message.contains("is_eligible")
            })
            .expect("veto cascade");
        assert_eq!(hit.spec, "eligibility");
        assert_eq!(effective_display(&hit).as_deref(), Some("2026-01-01"));
    }

    #[test]
    fn boolean_denial_is_not_cascade() {
        let engine = load(
            r#"spec eligibility 2026-01-01
"""
Age gate.
"""

data age: number
  -> help "Customer age."

rule is_eligible: true
  unless age < 18 then false
"#,
        );
        assert!(!engine
            .quality()
            .iter()
            .any(|r| r.message.contains("treats a yes/no outcome as veto")));
    }

    #[test]
    fn stdlib_dependency_not_reported() {
        let engine = load(
            r#"spec ship 2026-01-01
"""
Weight check.
"""

uses lemma units

data package_weight: units.mass
  -> help "Package weight."

rule heavy: package_weight > 20 kilogram
"#,
        );
        let recs = engine.quality();
        assert!(
            recs.iter().all(|r| r.spec != "units"),
            "stdlib units must not appear: {recs:?}"
        );
    }

    #[test]
    fn temporal_slices_distinct_by_effective_from() {
        let engine = load(
            r#"spec pricing 1933-01-01
"""
Old.
"""

data qty: number
rule total: qty

spec pricing 2026-01-01
"""
New.
"""

data qty: number
rule total: qty
"#,
        );
        let helps: Vec<_> = engine
            .quality()
            .into_iter()
            .filter(|r| r.message.contains("no `-> help`"))
            .collect();
        assert_eq!(helps.len(), 2, "got: {helps:?}");
        assert!(
            helps.iter().any(|r| {
                r.spec == "pricing" && effective_display(r).as_deref() == Some("1933-01-01")
            }),
            "missing 1933 slice: {helps:?}"
        );
        assert!(
            helps.iter().any(|r| {
                r.spec == "pricing" && effective_display(r).as_deref() == Some("2026-01-01")
            }),
            "missing 2026 slice: {helps:?}"
        );
        assert!(
            helps
                .iter()
                .all(|r| !r.message.contains("1933") && !r.message.contains("2026")),
            "message must not carry temporal identity: {helps:?}"
        );
        let displays: Vec<String> = helps.iter().map(|r| r.to_string()).collect();
        assert!(
            displays
                .iter()
                .any(|s| s.contains("In spec 'pricing' (effective from 1933-01-01)")),
            "Display missing 1933: {displays:?}"
        );
        assert!(
            displays
                .iter()
                .any(|s| s.contains("In spec 'pricing' (effective from 2026-01-01)")),
            "Display missing 2026: {displays:?}"
        );
    }

    #[test]
    fn recommendation_wire_json_uses_source_not_source_location() {
        let engine = load(
            r#"spec pricing 2026-01-01
"""
x
"""

data qty: number
rule total: qty
"#,
        );
        let hit = engine
            .quality()
            .into_iter()
            .find(|r| r.message.contains("no `-> help`"))
            .expect("missing help");
        let json = serde_json::to_value(&hit).expect("serialize");
        assert!(json.get("source").is_some(), "wire must use source: {json}");
        assert!(
            json.get("source_location").is_none(),
            "wire must not expose source_location: {json}"
        );
        assert_eq!(json["source"]["attribute"], serde_json::json!("test.lemma"));
        let round: Recommendation = serde_json::from_value(json).expect("deserialize");
        assert_eq!(round.spec, "pricing");
        assert!(round.message.contains("no `-> help`"));
    }

    fn and_chain_spec(rule_body: &str) -> String {
        format!(
            r#"spec gate 2026-01-01
"""
Gate.
"""

data ready: boolean
  -> help "Ready?"
data eligible: boolean
  -> help "Eligible?"

rule pass: no
  {rule_body}
"#
        )
    }

    fn ambiguous_and_hits(engine: &Engine) -> Vec<Recommendation> {
        engine
            .quality()
            .into_iter()
            .filter(|r| r.message.contains("Boolean `and` mixes a bare name"))
            .collect()
    }

    #[test]
    fn flags_not_ref_and_bare() {
        let engine = load(&and_chain_spec("unless not ready and eligible then yes"));
        let hits = ambiguous_and_hits(&engine);
        assert_eq!(hits.len(), 1, "got: {hits:?}");
        assert_eq!(hits[0].spec, "gate");
        assert!(
            hits[0].message.contains("Prefer parallel probes"),
            "got: {}",
            hits[0].message
        );
    }

    #[test]
    fn flags_bare_and_is_false() {
        let engine = load(&and_chain_spec(
            "unless ready and eligible is false then yes",
        ));
        let hits = ambiguous_and_hits(&engine);
        assert_eq!(hits.len(), 1, "got: {hits:?}");
        assert_eq!(hits[0].spec, "gate");
    }

    #[test]
    fn clean_not_and_is_true() {
        let engine = load(&and_chain_spec(
            "unless not ready and eligible is true then yes",
        ));
        assert!(
            ambiguous_and_hits(&engine).is_empty(),
            "got: {:?}",
            ambiguous_and_hits(&engine)
        );
    }

    #[test]
    fn clean_is_false_and_is_true() {
        let engine = load(&and_chain_spec(
            "unless ready is false and eligible is true then yes",
        ));
        assert!(
            ambiguous_and_hits(&engine).is_empty(),
            "got: {:?}",
            ambiguous_and_hits(&engine)
        );
    }

    #[test]
    fn clean_unary_not() {
        let engine = load(&and_chain_spec("unless not ready then yes"));
        assert!(
            ambiguous_and_hits(&engine).is_empty(),
            "got: {:?}",
            ambiguous_and_hits(&engine)
        );
    }

    #[test]
    fn clean_bare_and_bare() {
        let engine = load(&and_chain_spec("unless ready and eligible then yes"));
        assert!(
            ambiguous_and_hits(&engine).is_empty(),
            "got: {:?}",
            ambiguous_and_hits(&engine)
        );
    }

    fn redundant_unless_hits(engine: &Engine) -> Vec<Recommendation> {
        engine
            .quality()
            .into_iter()
            .filter(|r| r.message.contains("single `unless … then yes`"))
            .collect()
    }

    #[test]
    fn flags_redundant_no_unless_yes() {
        let engine = load(&and_chain_spec("unless ready and eligible then yes"));
        let hits = redundant_unless_hits(&engine);
        assert_eq!(hits.len(), 1, "got: {hits:?}");
        assert!(
            hits[0].message.contains("ready and eligible"),
            "{}",
            hits[0].message
        );
    }

    #[test]
    fn flags_redundant_false_unless_true() {
        let engine = load(
            r#"spec gate
data ready: boolean

rule pass: false
  unless ready then true
"#,
        );
        let hits = redundant_unless_hits(&engine);
        assert_eq!(hits.len(), 1, "got: {hits:?}");
    }

    #[test]
    fn flags_redundant_no_unless_not_ready_then_yes() {
        let engine = load(&and_chain_spec("unless not ready then yes"));
        let hits = redundant_unless_hits(&engine);
        assert_eq!(hits.len(), 1, "got: {hits:?}");
        assert!(hits[0].message.contains("not ready"), "{}", hits[0].message);
    }

    #[test]
    fn clean_symmetric_yes_unless_no() {
        let engine = load(
            r#"spec gate
data ready: boolean
data eligible: boolean

rule pass: yes
  unless ready and eligible then no
"#,
        );
        assert!(
            redundant_unless_hits(&engine).is_empty(),
            "got: {:?}",
            redundant_unless_hits(&engine)
        );
    }

    #[test]
    fn clean_multi_unless_discount() {
        let engine = load(
            r#"spec vip_discount
data qty: number
  -> minimum 0
data is_vip: boolean

rule discount: 0%
  unless qty >= 10 then 10%
  unless qty >= 50 then 20%
  unless is_vip    then 25%
"#,
        );
        assert!(
            redundant_unless_hits(&engine).is_empty(),
            "got: {:?}",
            redundant_unless_hits(&engine)
        );
    }

    #[test]
    fn clean_multi_unless_needs_jacket() {
        let engine = load(
            r#"spec weather
uses lemma units

data temperature: measure
  -> unit celsius: 1.0
data is_raining: boolean
data wind_speed: number

rule needs_jacket: no
  unless temperature < 15 celsius then yes
  unless is_raining               then yes
  unless wind_speed > 20          then yes
"#,
        );
        assert!(
            redundant_unless_hits(&engine).is_empty(),
            "got: {:?}",
            redundant_unless_hits(&engine)
        );
    }

    #[test]
    fn clean_direct_boolean_rule() {
        let engine = load(
            r#"spec gate
data ready: boolean
data eligible: boolean

rule pass: ready and eligible
"#,
        );
        assert!(
            redundant_unless_hits(&engine).is_empty(),
            "got: {:?}",
            redundant_unless_hits(&engine)
        );
    }

    #[test]
    fn clean_non_boolean_unless() {
        let engine = load(
            r#"spec pricing
data qty: number

rule discount: 0%
  unless qty >= 10 then 10%
"#,
        );
        assert!(
            redundant_unless_hits(&engine).is_empty(),
            "got: {:?}",
            redundant_unless_hits(&engine)
        );
    }

    #[test]
    fn deprecated_unit_without_colon_emits_quality_recommendation() {
        let engine = load(
            r#"spec money_spec
data money: measure
  -> unit eur 1.00
  -> help "Money amount."
"#,
        );
        let hit = engine
            .quality()
            .into_iter()
            .find(|r| r.message.contains("deprecated") && r.spec == "money_spec")
            .expect("deprecated unit without colon must produce quality recommendation");
        assert!(
            hit.message.contains("unit") && hit.message.contains(":"),
            "got: {}",
            hit.message
        );
    }

    #[test]
    fn unit_with_colon_has_no_deprecated_unit_recommendation() {
        let engine = load(
            r#"spec money_spec
data money: measure
  -> unit eur: 1.00
  -> help "Money amount."
"#,
        );
        assert!(
            engine
                .quality()
                .iter()
                .all(|r| !r.message.contains("deprecated")),
            "colon unit syntax must not flag deprecated: {:?}",
            engine.quality()
        );
    }
}
