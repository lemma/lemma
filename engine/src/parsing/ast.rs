//! AST types
//!
//! Infrastructure (Span, DepthTracker) and spec/data/rule/expression/value types from parsing.
//!
//! # Human `Display` vs canonical `AsLemmaSource`
//!
//! [`DataValue`] and [`CommandArg`] use human-oriented `Display` (stable for
//! `to_string()`, logs, APIs). [`Expression`] and [`LemmaRule`]/[`LemmaSpec`]
//! use canonical Lemma source for literals via [`AsLemmaSource`] around
//! [`Value`]. Wrap [`DataValue`] in [`AsLemmaSource`] when emitting
//! round-trippable source (e.g. the formatter).
//!
//! Logical identifier names (spec, data, rule, unit, reference path segments) are stored
//! as ASCII lowercase after parse. String literals and text option values are unchanged.

/// Fold a logical identifier name to canonical ASCII lowercase.
pub(crate) fn ascii_lowercase_logical_name(name: String) -> String {
    name.to_ascii_lowercase()
}

/// Span representing a location in source code
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub col: usize,
}

/// Tracks expression nesting depth during parsing to prevent stack overflow
pub struct DepthTracker {
    depth: usize,
    max_depth: usize,
}

impl DepthTracker {
    pub fn with_max_depth(max_depth: usize) -> Self {
        Self {
            depth: 0,
            max_depth,
        }
    }

    /// Returns Ok(()) if within limits, Err(current_depth) if exceeded.
    pub fn push_depth(&mut self) -> Result<(), usize> {
        self.depth += 1;
        if self.depth > self.max_depth {
            return Err(self.depth);
        }
        Ok(())
    }

    pub fn pop_depth(&mut self) {
        if self.depth > 0 {
            self.depth -= 1;
        }
    }

    pub fn max_depth(&self) -> usize {
        self.max_depth
    }
}

impl Default for DepthTracker {
    fn default() -> Self {
        Self {
            depth: 0,
            max_depth: 5,
        }
    }
}

// -----------------------------------------------------------------------------
// Spec, data, rule, expression and value types
// -----------------------------------------------------------------------------

use crate::parsing::source::Source;
use rust_decimal::Decimal;
use serde::Serialize;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

pub use crate::literals::{BooleanValue, DateTimeValue, TimeValue, TimezoneValue, Value};

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum EffectiveDate {
    Origin,
    DateTimeValue(crate::DateTimeValue),
}

impl EffectiveDate {
    pub fn as_ref(&self) -> Option<&crate::DateTimeValue> {
        match self {
            EffectiveDate::Origin => None,
            EffectiveDate::DateTimeValue(dt) => Some(dt),
        }
    }

    pub fn from_option(opt: Option<crate::DateTimeValue>) -> Self {
        match opt {
            None => EffectiveDate::Origin,
            Some(dt) => EffectiveDate::DateTimeValue(dt),
        }
    }

    pub fn to_option(&self) -> Option<crate::DateTimeValue> {
        match self {
            EffectiveDate::Origin => None,
            EffectiveDate::DateTimeValue(dt) => Some(dt.clone()),
        }
    }

    pub fn is_origin(&self) -> bool {
        matches!(self, EffectiveDate::Origin)
    }
}

impl PartialOrd for EffectiveDate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EffectiveDate {
    // As ref returns None for Origin, so Origin < DateTimeValue(_).
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_ref().cmp(&other.as_ref())
    }
}

impl fmt::Display for EffectiveDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EffectiveDate::Origin => Ok(()),
            EffectiveDate::DateTimeValue(dt) => write!(f, "{}", dt),
        }
    }
}

/// A Lemma repository header. Identity carrier; never owns specs.
///
/// `name` includes the `@` prefix when present (e.g. `Some("@jack/finance")`).
/// `None` for the default unnamed repository. Identity (used by
/// `PartialEq`, `Eq`, `Hash`, and `Ord` for `BTreeMap` keying) is just `name`.
/// `dependency`, `start_line` and `source_type` are metadata excluded from identity.
///
/// `dependency` is the provenance guard: `None` for locally loaded repos,
/// `Some(id)` for repos introduced by a dependency. All specs in a repo must
/// share the same `dependency` value — the engine rejects mismatches at load time.
///
/// The parser fills [`LemmaRepository`] for each `repo` section before grouping specs in
/// [`ParseResult`]; loaders set `dependency` when inserting dependency bundles.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LemmaRepository {
    /// Repository name, including `@` when present. `None` for the default unnamed repository.
    pub name: Option<String>,
    /// Dependency provenance: `None` for locally loaded repos, `Some(id)` for dependency repos.
    /// Not part of identity — used as an isolation guard at load time.
    pub dependency: Option<String>,
    pub start_line: usize,
    pub source_type: Option<crate::parsing::source::SourceType>,
}

impl LemmaRepository {
    #[must_use]
    pub fn new(name: Option<String>) -> Self {
        Self {
            name: name.map(ascii_lowercase_logical_name),
            dependency: None,
            start_line: 1,
            source_type: None,
        }
    }

    #[must_use]
    pub fn with_start_line(mut self, start_line: usize) -> Self {
        self.start_line = start_line;
        self
    }

    #[must_use]
    pub fn with_source_type(mut self, source_type: crate::parsing::source::SourceType) -> Self {
        self.source_type = Some(source_type);
        self
    }

    #[must_use]
    pub fn with_dependency(mut self, dependency_id: impl Into<String>) -> Self {
        self.dependency = Some(dependency_id.into());
        self
    }
}

impl PartialEq for LemmaRepository {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for LemmaRepository {}

impl PartialOrd for LemmaRepository {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LemmaRepository {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(&other.name)
    }
}

impl Hash for LemmaRepository {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

/// Textual repository qualifier as written in source (for example `@iso/countries`).
/// `name` stores the qualifier verbatim, including a leading `@` when present. The planner
/// resolves a [`RepositoryQualifier`] to an `Arc<LemmaRepository>` against the active context.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct RepositoryQualifier {
    pub name: String,
}

impl RepositoryQualifier {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: ascii_lowercase_logical_name(name.into()),
        }
    }

    /// Whether this repository qualifier refers to a registry (e.g., starts with `@`).
    #[must_use]
    pub fn is_registry(&self) -> bool {
        self.name.starts_with('@')
    }
}

impl fmt::Display for RepositoryQualifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// A Lemma spec containing data and rules.
///
/// `name` is always the bare spec set name (no `@`, no dots, no slashes). The
/// owning repository — and, transitively, whether the spec is loaded from a registry
/// bundle — is preserved through the structural relationship in
/// [`crate::engine::Context`], not via fields on this structure.
///
/// Context identity is `(repository, name, EffectiveDate)` or `std::ptr::eq` on a
/// Context-owned row — not [`PartialEq`]. [`PartialEq`] is full AST equality
/// (including statement [`Source`] spans) for [`crate::Engine::update`] skip only.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LemmaSpec {
    pub name: String,
    pub effective_from: EffectiveDate,
    pub source_type: Option<crate::parsing::source::SourceType>,
    pub start_line: usize,
    pub commentary: Option<String>,
    pub data: Vec<LemmaData>,
    pub rules: Vec<LemmaRule>,
    pub meta_fields: Vec<MetaField>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MetaField {
    pub key: String,
    pub value: Value,
    pub source_location: Source,
}

impl fmt::Display for MetaField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "meta {}: {}", self.key, AsLemmaSource(&self.value))
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LemmaData {
    pub reference: Reference,
    pub value: DataValue,
    pub source_location: Source,
}

/// An unless clause that provides an alternative result
///
/// Unless clauses are evaluated in order, and the last matching condition wins.
/// This matches natural language: "X unless A then Y, unless B then Z" - if both
/// A and B are true, Z is returned (the last match).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UnlessClause {
    pub condition: Expression,
    pub result: Expression,
    pub source_location: Source,
}

/// A rule with a single expression and optional unless clauses
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LemmaRule {
    pub name: String,
    pub expression: Expression,
    pub unless_clauses: Vec<UnlessClause>,
    pub source_location: Source,
}

/// An expression that can be evaluated, with source location
///
/// Expressions use semantic equality - two expressions with the same
/// structure (kind) are equal regardless of source location.
/// Hash is not implemented for AST Expression; use planning::semantics::Expression as map keys.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Expression {
    pub kind: ExpressionKind,
    pub source_location: Option<Source>,
}

impl Expression {
    /// Create a new expression with kind and source location
    #[must_use]
    pub fn new(kind: ExpressionKind, source_location: Source) -> Self {
        Self {
            kind,
            source_location: Some(source_location),
        }
    }
}

/// Semantic equality - compares expressions by structure only, ignoring source location
impl PartialEq for Expression {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl Eq for Expression {}

/// Whether a date is relative to `now` in the past or future direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateRelativeKind {
    InPast,
    InFuture,
}

/// Calendar-period membership checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateCalendarKind {
    Current,
    Past,
    Future,
    NotIn,
}

/// Granularity of a calendar-period check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalendarPeriodUnit {
    Year,
    Month,
    Week,
}

impl CalendarPeriodUnit {
    #[must_use]
    pub fn from_keyword(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "year" => Some(Self::Year),
            "month" => Some(Self::Month),
            "week" => Some(Self::Week),
            _ => None,
        }
    }
}

impl fmt::Display for DateRelativeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DateRelativeKind::InPast => write!(f, "in past"),
            DateRelativeKind::InFuture => write!(f, "in future"),
        }
    }
}

impl fmt::Display for DateCalendarKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DateCalendarKind::Current => write!(f, "in calendar"),
            DateCalendarKind::Past => write!(f, "in past calendar"),
            DateCalendarKind::Future => write!(f, "in future calendar"),
            DateCalendarKind::NotIn => write!(f, "not in calendar"),
        }
    }
}

impl fmt::Display for CalendarPeriodUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CalendarPeriodUnit::Year => write!(f, "year"),
            CalendarPeriodUnit::Month => write!(f, "month"),
            CalendarPeriodUnit::Week => write!(f, "week"),
        }
    }
}

/// The kind/type of expression
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpressionKind {
    /// Parse-time literal value (type will be resolved during planning)
    Literal(Value),
    /// Unresolved reference (identifier or dot path). Resolved during planning to DataPath or RulePath.
    Reference(Reference),
    /// The `now` keyword — resolves to the evaluation datetime (= effective).
    Now,
    /// Date-relative sugar: `<date_expr> in past` / `<date_expr> in future`
    /// Fields: (kind, date_expression)
    DateRelative(DateRelativeKind, Arc<Expression>),
    /// Calendar-period sugar: `<date_expr> in [past|future] calendar year|month|week`
    /// Fields: (kind, unit, date_expression)
    DateCalendar(DateCalendarKind, CalendarPeriodUnit, Arc<Expression>),
    /// Range literal: `{left_expr}...{right_expr}`
    RangeLiteral(Arc<Expression>, Arc<Expression>),
    /// Relative date range: `past 7 day` / `future 30 day`
    PastFutureRange(DateRelativeKind, Arc<Expression>),
    /// Range containment: `{value_expr} in {range_expr}`
    RangeContainment(Arc<Expression>, Arc<Expression>),
    LogicalAnd(Arc<Expression>, Arc<Expression>),
    Arithmetic(Arc<Expression>, ArithmeticComputation, Arc<Expression>),
    Comparison(Arc<Expression>, ComparisonComputation, Arc<Expression>),
    UnitConversion(Arc<Expression>, ConversionTarget),
    LogicalNegation(Arc<Expression>, NegationType),
    MathematicalComputation(MathematicalComputation, Arc<Expression>),
    Veto(VetoExpression),
    /// `expr is veto` / `veto is expr` — boolean: whether evaluating `expr` yields `OperationResult::Veto`.
    ResultIsVeto(Arc<Expression>),
}

/// Unresolved reference from parser
///
/// Reference to a data or rule (identifier or dot path).
///
/// Used in expressions and in LemmaData. During planning, references
/// are resolved to DataPath or RulePath (semantics layer).
/// Examples:
/// - Local "age": segments=[], name="age"
/// - Cross-spec "employee.salary": segments=["employee"], name="salary"
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Reference {
    pub segments: Vec<String>,
    pub name: String,
}

impl Reference {
    #[must_use]
    pub fn local(name: String) -> Self {
        Self {
            segments: Vec::new(),
            name: ascii_lowercase_logical_name(name),
        }
    }

    #[must_use]
    pub fn from_path(path: Vec<String>) -> Self {
        if path.is_empty() {
            Self {
                segments: Vec::new(),
                name: String::new(),
            }
        } else {
            // Safe: path is non-empty.
            let name = ascii_lowercase_logical_name(path[path.len() - 1].clone());
            let segments = path[..path.len() - 1]
                .iter()
                .map(|segment| ascii_lowercase_logical_name(segment.clone()))
                .collect();
            Self { segments, name }
        }
    }

    #[must_use]
    pub fn is_local(&self) -> bool {
        self.segments.is_empty()
    }

    #[must_use]
    pub fn full_path(&self) -> Vec<String> {
        let mut path = self.segments.clone();
        path.push(self.name.clone());
        path
    }
}

impl fmt::Display for Reference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for segment in &self.segments {
            write!(f, "{}.", segment)?;
        }
        write!(f, "{}", self.name)
    }
}

/// Arithmetic computations
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArithmeticComputation {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Power,
}

/// Comparison computations
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonComputation {
    GreaterThan,
    LessThan,
    GreaterThanOrEqual,
    LessThanOrEqual,
    Is,
    IsNot,
}

impl ComparisonComputation {
    /// Check if this is an equality comparison (`is`)
    #[must_use]
    pub fn is_equal(&self) -> bool {
        matches!(self, ComparisonComputation::Is)
    }

    /// Check if this is an inequality comparison (`is not`)
    #[must_use]
    pub fn is_not_equal(&self) -> bool {
        matches!(self, ComparisonComputation::IsNot)
    }
}

/// The target type for `as` cast expressions (e.g. `as number`, `as eur`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionTarget {
    Type(PrimitiveKind),
    Unit { unit_name: String },
}

/// Types of logical negation
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NegationType {
    Not,
}

/// A veto expression that prohibits any valid verdict from the rule
///
/// A veto prevents the rule from producing any valid result. This is used for
/// validation and constraint enforcement — distinct from boolean `false`.
///
/// Example: `veto "Must be over 18"` - blocks the rule entirely with a message
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct VetoExpression {
    pub message: Option<String>,
}

/// Mathematical computations
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MathematicalComputation {
    Sqrt,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Log,
    Exp,
    Abs,
    Floor,
    Ceil,
    Round,
}

/// A spec reference written in source.
///
/// `name` is the bare spec name (no `@`, no dots, no slashes).
/// [`SpecRef::repository`] is `None` for same-repository references, or
/// `Some(RepositoryQualifier)` when a repository qualifier was written before the spec name.
/// `effective` carries an optional explicit pin written next to the spec name.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpecRef {
    /// Optional explicit repository qualifier. `None` means the reference resolves against
    /// the consumer spec's own repository.
    pub repository: Option<RepositoryQualifier>,
    /// The spec name.
    pub name: String,
    /// Optional explicit effective datetime pin written in source.
    pub effective: Option<DateTimeValue>,
    /// Source span of the repository qualifier (when `repository` is present).
    pub repository_span: Option<Span>,
    /// Source span of `name` and optional `effective`.
    pub target_span: Option<Span>,
}

impl std::fmt::Display for SpecRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(qualifier) = &self.repository {
            write!(f, "{} ", qualifier)?;
        }
        write!(f, "{}", self.name)?;
        if let Some(d) = &self.effective {
            write!(f, " {}", d)?;
        }
        Ok(())
    }
}

impl SpecRef {
    /// Same-repository reference: resolution uses the consumer's repository.
    pub fn same_repository(name: impl Into<String>) -> Self {
        Self {
            name: ascii_lowercase_logical_name(name.into()),
            repository: None,
            effective: None,
            repository_span: None,
            target_span: None,
        }
    }

    /// Cross-repository reference with an explicit repository qualifier.
    pub fn cross_repository(name: impl Into<String>, qualifier: RepositoryQualifier) -> Self {
        Self {
            name: ascii_lowercase_logical_name(name.into()),
            repository: Some(qualifier),
            effective: None,
            repository_span: None,
            target_span: None,
        }
    }

    /// Resolve the effective instant for this reference given the planning slice's `effective`.
    /// Explicit qualifier on the reference wins; otherwise inherits the slice instant.
    pub fn at(&self, effective: &EffectiveDate) -> EffectiveDate {
        self.effective
            .clone()
            .map_or_else(|| effective.clone(), EffectiveDate::DateTimeValue)
    }

    /// Concrete instant for evaluation or navigation: explicit ref pin wins, else consumer bound.
    /// Returns `None` when neither side names a concrete instant (consumer at Origin, no ref pin).
    pub fn resolved_instant(
        &self,
        consumer_effective_from: Option<&DateTimeValue>,
    ) -> Option<DateTimeValue> {
        self.effective
            .clone()
            .or_else(|| consumer_effective_from.cloned())
    }
}

/// A single factor in a compound unit expression.
///
/// `measure_ref` is the name of the referenced unit (e.g. `"meter"`, `"second"`).
/// `exp` is the integer exponent, positive for numerator and negative for denominator.
/// For example `meter/second^2` produces:
/// - `UnitFactor { measure_ref: "meter", exp: 1 }`
/// - `UnitFactor { measure_ref: "second", exp: -2 }`
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UnitFactor {
    pub measure_ref: String,
    pub exp: i32,
}

/// The argument to a `-> unit <name> ...` command, either a plain numeric
/// conversion factor or a compound unit expression.
///
/// - `Factor(v)` — simple unit: `-> unit meter: 1`, `-> unit kilometer: 1000`
/// - `Expr(prefix, factors)` — compound unit: `-> unit mps: meter/second`,
///   `-> unit kmh: 3.6 meter/second`
///   The `prefix` is an additional scalar multiplier beyond what the unit
///   factor references contribute; it defaults to `1` when omitted.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UnitArg {
    Factor(#[serde(with = "crate::literals::decimal_string_serde")] Decimal),
    Expr(
        #[serde(with = "crate::literals::decimal_string_serde")] Decimal,
        Vec<UnitFactor>,
    ),
}

impl fmt::Display for UnitArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UnitArg::Factor(v) => write!(f, "{}", v),
            UnitArg::Expr(prefix, factors) => {
                if *prefix != Decimal::ONE {
                    write!(f, "{} ", prefix)?;
                }
                for (index, factor) in factors.iter().enumerate() {
                    if factor.exp == 0 {
                        unreachable!("BUG: unit factor exponent cannot be zero");
                    }
                    if factor.exp > 0 {
                        if index > 0 {
                            write!(f, " * ")?;
                        }
                        write!(f, "{}", factor.measure_ref)?;
                        if factor.exp != 1 {
                            write!(f, "^{}", factor.exp)?;
                        }
                    } else {
                        let denominator_started =
                            factors[..index].iter().any(|prior| prior.exp < 0);
                        if denominator_started {
                            write!(f, " * ")?;
                        } else {
                            write!(f, "/")?;
                        }
                        write!(f, "{}", factor.measure_ref)?;
                        let positive_exp = factor
                            .exp
                            .checked_neg()
                            .expect("BUG: negative unit factor exponent");
                        if positive_exp != 1 {
                            write!(f, "^{}", positive_exp)?;
                        }
                    }
                }
                Ok(())
            }
        }
    }
}

/// A parsed constraint command argument, preserving the literal kind from the
/// grammar rule `command_arg: { number_literal | boolean_literal | text_literal | label }`.
///
/// Three grammatical kinds appear after a constraint command:
/// - **Literal** — a fully-typed value carrying the literal kind the parser
///   recognised (`Number`, `Ratio`, `Measure`, `Date`, `Time`,
///   `Boolean`, `Text`). Stored as the canonical [`crate::literals::Value`]
///   so downstream consumers match on the variant rather than re-parsing strings.
/// - **Label** — a bare identifier used as a name (e.g. the unit name `eur`
///   in `unit eur 1.00`, or a primitive type keyword used as an option label).
/// - **UnitExpr** — compound unit expression produced by the parser for
///   `-> unit <name> ...` commands. Only appears as the second argument of a
///   `Unit` command; the first argument is always the unit name as `Label`.
///
/// Planning validates each command's args against the variant kinds it accepts
/// and rejects mismatches without coercion (a `Text` literal is never a `Number`,
/// a `Ratio` literal is never a bare `Number`, etc.).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CommandArg {
    /// A typed literal value parsed by [`crate::parsing::parser::Parser::parse_literal_value`].
    Literal(crate::literals::Value),
    /// An identifier used as a name (unit name, option keyword, etc.).
    Label(String),
    /// A unit argument produced by the parser for `-> unit <name> ...` commands.
    UnitExpr(UnitArg),
}

impl fmt::Display for CommandArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandArg::Literal(v) => write!(f, "{}", v),
            CommandArg::Label(s) => write!(f, "{}", s),
            CommandArg::UnitExpr(unit_arg) => write!(f, "{}", unit_arg),
        }
    }
}

/// Constraint command for type definitions. Derived from lexer tokens; no string matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeConstraintCommand {
    Help,
    Suggest,
    Fill,
    Unit,
    Trait,
    Minimum,
    Maximum,
    Lower,
    Upper,
    Decimals,
    Option,
    Options,
    Length,
}

impl fmt::Display for TypeConstraintCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            TypeConstraintCommand::Help => "help",
            TypeConstraintCommand::Suggest => "suggest",
            TypeConstraintCommand::Fill => "fill",
            TypeConstraintCommand::Unit => "unit",
            TypeConstraintCommand::Trait => "trait",
            TypeConstraintCommand::Minimum => "minimum",
            TypeConstraintCommand::Maximum => "maximum",
            TypeConstraintCommand::Lower => "lower",
            TypeConstraintCommand::Upper => "upper",
            TypeConstraintCommand::Decimals => "decimals",
            TypeConstraintCommand::Option => "option",
            TypeConstraintCommand::Options => "options",
            TypeConstraintCommand::Length => "length",
        };
        write!(f, "{}", s)
    }
}

/// Parses a constraint command name. Returns None for unknown (parser returns error).
#[must_use]
pub fn try_parse_type_constraint_command(s: &str) -> Option<TypeConstraintCommand> {
    // ASCII fold without allocating a lowercased String on every `->` command.
    match s.trim() {
        s if s.eq_ignore_ascii_case("help") => Some(TypeConstraintCommand::Help),
        s if s.eq_ignore_ascii_case("suggest") => Some(TypeConstraintCommand::Suggest),
        s if s.eq_ignore_ascii_case("fill") => Some(TypeConstraintCommand::Fill),
        s if s.eq_ignore_ascii_case("unit") => Some(TypeConstraintCommand::Unit),
        s if s.eq_ignore_ascii_case("trait") => Some(TypeConstraintCommand::Trait),
        s if s.eq_ignore_ascii_case("minimum") => Some(TypeConstraintCommand::Minimum),
        s if s.eq_ignore_ascii_case("maximum") => Some(TypeConstraintCommand::Maximum),
        s if s.eq_ignore_ascii_case("lower") => Some(TypeConstraintCommand::Lower),
        s if s.eq_ignore_ascii_case("upper") => Some(TypeConstraintCommand::Upper),
        s if s.eq_ignore_ascii_case("decimals") => Some(TypeConstraintCommand::Decimals),
        s if s.eq_ignore_ascii_case("option") => Some(TypeConstraintCommand::Option),
        s if s.eq_ignore_ascii_case("options") => Some(TypeConstraintCommand::Options),
        s if s.eq_ignore_ascii_case("length") => Some(TypeConstraintCommand::Length),
        _ => None,
    }
}

/// Whether a `->` continuation uses assignment shape (`key: value`) or space-separated args.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuationShape {
    Assignment,
    SpaceSeparated,
}

impl TypeConstraintCommand {
    #[must_use]
    pub fn continuation_shape(self) -> ContinuationShape {
        match self {
            TypeConstraintCommand::Unit => ContinuationShape::Assignment,
            TypeConstraintCommand::Help
            | TypeConstraintCommand::Suggest
            | TypeConstraintCommand::Fill
            | TypeConstraintCommand::Trait
            | TypeConstraintCommand::Minimum
            | TypeConstraintCommand::Maximum
            | TypeConstraintCommand::Lower
            | TypeConstraintCommand::Upper
            | TypeConstraintCommand::Decimals
            | TypeConstraintCommand::Option
            | TypeConstraintCommand::Options
            | TypeConstraintCommand::Length => ContinuationShape::SpaceSeparated,
        }
    }
}

/// One `-> command …` row on a [`DataValue::Definition`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Constraint {
    pub command: TypeConstraintCommand,
    pub args: Vec<CommandArg>,
    pub source_location: crate::parsing::source::Source,
    /// Parsed from deprecated `unit name value` without colon (removed in a future release).
    pub deprecated_without_colon: bool,
}

impl Constraint {
    #[must_use]
    pub fn new(
        command: TypeConstraintCommand,
        args: Vec<CommandArg>,
        source_location: crate::parsing::source::Source,
    ) -> Self {
        Self {
            command,
            args,
            source_location,
            deprecated_without_colon: false,
        }
    }
}

#[cfg(test)]
pub(crate) fn test_constraint(command: TypeConstraintCommand, args: Vec<CommandArg>) -> Constraint {
    Constraint::new(
        command,
        args,
        crate::parsing::source::Source::new(
            crate::parsing::source::SourceType::Volatile,
            Span {
                start: 0,
                end: 0,
                line: 1,
                col: 0,
            },
        ),
    )
}

/// Right-hand side of a `uses` block `-> with` binding: literal value or reference to copy.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WithRhs {
    Literal(Value),
    Reference { target: Reference },
}

/// One `-> with path: value` row under a [`DataValue::Import`] block.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct UsesBinding {
    /// Path relative to the imported spec (no import alias prefix).
    pub path: Reference,
    pub rhs: WithRhs,
    pub source_location: Source,
    /// Parsed from deprecated standalone `with alias.path: …` (removed in a future release).
    pub deprecated_standalone_with: bool,
}

/// Prefix import alias onto a relative binding path (`pricing.tax_rate` under alias `line` → `line.pricing.tax_rate`).
#[must_use]
pub fn prefix_reference(alias: &str, relative: &Reference) -> Reference {
    let mut segments = vec![ascii_lowercase_logical_name(alias.to_string())];
    segments.extend(relative.segments.iter().cloned());
    Reference {
        segments,
        name: relative.name.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
/// Parse-time data value (before type resolution)
pub enum DataValue {
    /// Declares data: optional explicit parent type, optional constraints (`-> ...`),
    /// and optional literal value.
    ///
    /// Examples:
    /// - `data x: 3.14` → `base: None`, `value: Some(Number)`
    /// - `data x: number -> minimum 0` → `base: Some(Number)`, `constraints: Some(...)`
    /// - `data x: finance.money` → `base: Some(Qualified { spec_alias: "finance", inner: Custom("money") })`
    Definition {
        base: Option<ParentType>,
        constraints: Option<Vec<Constraint>>,
        value: Option<Value>,
    },
    /// Import from another spec (surface syntax is `uses`; alias is [`LemmaData::reference`]).
    Import {
        spec_ref: SpecRef,
        bindings: Vec<UsesBinding>,
    },
}

impl DataValue {
    #[must_use]
    pub fn import(spec_ref: SpecRef) -> Self {
        Self::Import {
            spec_ref,
            bindings: Vec::new(),
        }
    }

    /// Whether this is only a literal RHS (`data x: 3.14`), valid as a binding value.
    #[must_use]
    pub fn is_definition_literal_only(&self) -> bool {
        matches!(
            self,
            DataValue::Definition {
                base: None,
                constraints: None,
                value: Some(_),
            }
        )
    }

    /// Whether planning must resolve this [`LemmaData`] row through the type resolver / named types.
    #[must_use]
    pub fn definition_needs_type_resolution(&self) -> bool {
        match self {
            DataValue::Definition { base: Some(_), .. }
            | DataValue::Definition {
                constraints: Some(_),
                ..
            } => true,
            DataValue::Definition {
                base: None,
                constraints: None,
                value: Some(v),
            } => !matches!(v, Value::NumberWithUnit(_, _)),
            DataValue::Import { .. } | DataValue::Definition { .. } => false,
        }
    }
}

/// Render a chain of `-> command args ...` constraints for display purposes.
/// Shared between [`DataValue::Definition`] constraint chains.
fn format_constraint_chain(constraints: &[Constraint]) -> String {
    constraints
        .iter()
        .map(|row| format_constraint_as_source(&row.command, &row.args))
        .collect::<Vec<_>>()
        .join(" -> ")
}

impl fmt::Display for DataValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataValue::Definition {
                base,
                constraints,
                value,
            } => {
                if base.is_none() && constraints.is_none() {
                    return match value {
                        Some(v) => write!(f, "{}", v),
                        None => Ok(()),
                    };
                }
                let base_str = match base.as_ref() {
                    Some(b) => format!("{b}"),
                    None => match value {
                        Some(v) => {
                            if let Some(ref constraints_vec) = constraints {
                                let constraint_str = format_constraint_chain(constraints_vec);
                                return write!(f, "{v} -> {constraint_str}");
                            }
                            return write!(f, "{v}");
                        }
                        None => String::new(),
                    },
                };
                if let Some(ref constraints_vec) = constraints {
                    let constraint_str = format_constraint_chain(constraints_vec);
                    write!(f, "{base_str} -> {constraint_str}")
                } else {
                    write!(f, "{base_str}")
                }
            }
            DataValue::Import {
                spec_ref,
                bindings: _,
            } => {
                write!(f, "uses {}", spec_ref)
            }
        }
    }
}

impl LemmaData {
    #[must_use]
    pub fn new(reference: Reference, value: DataValue, source_location: Source) -> Self {
        Self {
            reference,
            value,
            source_location,
        }
    }
}

impl LemmaSpec {
    #[must_use]
    pub fn new(name: String) -> Self {
        Self {
            name: ascii_lowercase_logical_name(name),
            effective_from: EffectiveDate::Origin,
            source_type: None,
            start_line: 1,
            commentary: None,
            data: Vec::new(),
            rules: Vec::new(),
            meta_fields: Vec::new(),
        }
    }

    /// Temporal range start. Origin (None) means −∞.
    pub fn effective_from(&self) -> Option<&DateTimeValue> {
        self.effective_from.as_ref()
    }

    #[must_use]
    pub fn with_source_type(mut self, source_type: crate::parsing::source::SourceType) -> Self {
        self.source_type = Some(source_type);
        self
    }

    #[must_use]
    pub fn with_start_line(mut self, start_line: usize) -> Self {
        self.start_line = start_line;
        self
    }

    #[must_use]
    pub fn set_commentary(mut self, commentary: String) -> Self {
        self.commentary = Some(commentary);
        self
    }

    #[must_use]
    pub fn add_data(mut self, data: LemmaData) -> Self {
        self.data.push(data);
        self
    }

    #[must_use]
    pub fn add_rule(mut self, rule: LemmaRule) -> Self {
        self.rules.push(rule);
        self
    }

    #[must_use]
    pub fn add_meta_field(mut self, meta: MetaField) -> Self {
        self.meta_fields.push(meta);
        self
    }
}

impl fmt::Display for LemmaSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "spec {}", self.name)?;
        if let EffectiveDate::DateTimeValue(ref af) = self.effective_from {
            write!(f, " {}", af)?;
        }
        writeln!(f)?;

        if let Some(ref commentary) = self.commentary {
            writeln!(f, "\"\"\"")?;
            writeln!(f, "{}", commentary)?;
            writeln!(f, "\"\"\"")?;
        }

        if !self.data.is_empty() {
            writeln!(f)?;
            for data in &self.data {
                write!(f, "{}", data)?;
            }
        }

        if !self.rules.is_empty() {
            writeln!(f)?;
            for (index, rule) in self.rules.iter().enumerate() {
                if index > 0 {
                    writeln!(f)?;
                }
                write!(f, "{}", rule)?;
            }
        }

        if !self.meta_fields.is_empty() {
            writeln!(f)?;
            for meta in &self.meta_fields {
                writeln!(f, "{}", meta)?;
            }
        }

        Ok(())
    }
}

impl fmt::Display for LemmaData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "data {}: {}", self.reference, self.value)
    }
}

impl fmt::Display for LemmaRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "rule {}: {}", self.name, self.expression)?;
        for unless_clause in &self.unless_clauses {
            write!(
                f,
                "\n  unless {} then {}",
                unless_clause.condition, unless_clause.result
            )?;
        }
        writeln!(f)?;
        Ok(())
    }
}

/// Precedence level for an expression kind.
///
/// Higher values bind tighter. Used by `Expression::Display` and the formatter
/// to insert parentheses only where needed.
///
/// `RangeLiteral` (type construction via `...`) binds above all arithmetic; only atoms bind
/// above range. Parser climb in [`crate::parsing::parser::Parser`] must match this table.
pub fn expression_precedence(kind: &ExpressionKind) -> u8 {
    match kind {
        ExpressionKind::LogicalAnd(..) => 2,
        ExpressionKind::LogicalNegation(..) => 3,
        ExpressionKind::Comparison(..) | ExpressionKind::ResultIsVeto(..) => 4,
        ExpressionKind::RangeContainment(..) => 4,
        ExpressionKind::DateRelative(..) | ExpressionKind::DateCalendar(..) => 4,
        ExpressionKind::Arithmetic(_, op, _) => arithmetic_precedence(op),
        ExpressionKind::UnitConversion(..) => 8,
        ExpressionKind::RangeLiteral(..) => 9,
        ExpressionKind::MathematicalComputation(..) => 10,
        ExpressionKind::PastFutureRange(..) => 10,
        ExpressionKind::Literal(..)
        | ExpressionKind::Reference(..)
        | ExpressionKind::Now
        | ExpressionKind::Veto(..) => 10,
    }
}

/// Precedence for an arithmetic operator. Must match [`expression_precedence`].
pub fn arithmetic_precedence(op: &ArithmeticComputation) -> u8 {
    match op {
        ArithmeticComputation::Add | ArithmeticComputation::Subtract => 5,
        ArithmeticComputation::Multiply
        | ArithmeticComputation::Divide
        | ArithmeticComputation::Modulo => 6,
        ArithmeticComputation::Power => 7,
    }
}

/// Operand position under a parent operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandSide {
    Left,
    Right,
}

/// Associativity of a binary (or n-ary chain) operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Associativity {
    Left,
    Right,
}

/// Whether a child expression must be wrapped in parentheses when printed under a parent.
///
/// - `parent_assoc == None`: unary / prefix parent — wrap only when `child_prec < parent_prec`.
/// - `Some(Left)`: wrap looser children, and same-prec **right** children.
/// - `Some(Right)`: wrap looser children, and same-prec **left** children.
pub fn operand_needs_parentheses(
    child_prec: u8,
    parent_prec: u8,
    side: OperandSide,
    parent_assoc: Option<Associativity>,
) -> bool {
    if child_prec < parent_prec {
        return true;
    }
    if child_prec > parent_prec {
        return false;
    }
    match parent_assoc {
        None => false,
        Some(Associativity::Left) => matches!(side, OperandSide::Right),
        Some(Associativity::Right) => matches!(side, OperandSide::Left),
    }
}

pub fn arithmetic_associativity(op: &ArithmeticComputation) -> Associativity {
    match op {
        ArithmeticComputation::Power => Associativity::Right,
        ArithmeticComputation::Add
        | ArithmeticComputation::Subtract
        | ArithmeticComputation::Multiply
        | ArithmeticComputation::Divide
        | ArithmeticComputation::Modulo => Associativity::Left,
    }
}

fn write_expression_child(
    f: &mut fmt::Formatter<'_>,
    child: &Expression,
    parent_prec: u8,
    side: OperandSide,
    parent_assoc: Option<Associativity>,
) -> fmt::Result {
    let child_prec = expression_precedence(&child.kind);
    if operand_needs_parentheses(child_prec, parent_prec, side, parent_assoc) {
        write!(f, "({})", child)
    } else {
        write!(f, "{}", child)
    }
}

impl fmt::Display for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ExpressionKind::Literal(lit) => write!(f, "{}", AsLemmaSource(lit)),
            ExpressionKind::Reference(r) => write!(f, "{}", r),
            ExpressionKind::Arithmetic(left, op, right) => {
                let my_prec = expression_precedence(&self.kind);
                let assoc = Some(arithmetic_associativity(op));
                write_expression_child(f, left, my_prec, OperandSide::Left, assoc)?;
                write!(f, " {} ", op)?;
                write_expression_child(f, right, my_prec, OperandSide::Right, assoc)
            }
            ExpressionKind::Comparison(left, op, right) => {
                let my_prec = expression_precedence(&self.kind);
                write_expression_child(f, left, my_prec, OperandSide::Left, None)?;
                write!(f, " {} ", op)?;
                write_expression_child(f, right, my_prec, OperandSide::Right, None)
            }
            ExpressionKind::UnitConversion(value, target) => {
                let my_prec = expression_precedence(&self.kind);
                write_expression_child(f, value, my_prec, OperandSide::Left, None)?;
                write!(f, " as {}", target)
            }
            ExpressionKind::LogicalNegation(expr, negation) => {
                if let (NegationType::Not, ExpressionKind::ResultIsVeto(operand)) =
                    (negation, &expr.kind)
                {
                    let my_prec = expression_precedence(&self.kind);
                    write_expression_child(f, operand, my_prec, OperandSide::Left, None)?;
                    write!(f, " is not veto")
                } else {
                    let my_prec = expression_precedence(&self.kind);
                    write!(f, "not ")?;
                    write_expression_child(f, expr, my_prec, OperandSide::Right, None)
                }
            }
            ExpressionKind::ResultIsVeto(operand) => {
                let my_prec = expression_precedence(&self.kind);
                write_expression_child(f, operand, my_prec, OperandSide::Left, None)?;
                write!(f, " is veto")
            }
            ExpressionKind::LogicalAnd(left, right) => {
                let my_prec = expression_precedence(&self.kind);
                let assoc = Some(Associativity::Left);
                write_expression_child(f, left, my_prec, OperandSide::Left, assoc)?;
                write!(f, " and ")?;
                write_expression_child(f, right, my_prec, OperandSide::Right, assoc)
            }
            ExpressionKind::MathematicalComputation(op, operand) => {
                let my_prec = expression_precedence(&self.kind);
                write!(f, "{} ", op)?;
                write_expression_child(f, operand, my_prec, OperandSide::Right, None)
            }
            ExpressionKind::Veto(veto) => match &veto.message {
                Some(msg) => write!(f, "veto {}", quote_lemma_text(msg)),
                None => write!(f, "veto"),
            },
            ExpressionKind::Now => write!(f, "now"),
            ExpressionKind::DateRelative(kind, date_expr) => {
                write!(f, "{} {}", date_expr, kind)?;
                Ok(())
            }
            ExpressionKind::DateCalendar(kind, unit, date_expr) => {
                write!(f, "{} {} {}", date_expr, kind, unit)
            }
            ExpressionKind::RangeLiteral(left, right) => {
                let my_prec = expression_precedence(&self.kind);
                write_expression_child(f, left, my_prec, OperandSide::Left, None)?;
                write!(f, "...")?;
                write_expression_child(f, right, my_prec, OperandSide::Right, None)
            }
            ExpressionKind::PastFutureRange(kind, offset_expr) => {
                match kind {
                    DateRelativeKind::InPast => write!(f, "past ")?,
                    DateRelativeKind::InFuture => write!(f, "future ")?,
                }
                let my_prec = expression_precedence(&self.kind);
                write_expression_child(f, offset_expr, my_prec, OperandSide::Right, None)
            }
            ExpressionKind::RangeContainment(value, range) => {
                let my_prec = expression_precedence(&self.kind);
                write_expression_child(f, value, my_prec, OperandSide::Left, None)?;
                write!(f, " in ")?;
                write_expression_child(f, range, my_prec, OperandSide::Right, None)
            }
        }
    }
}

impl fmt::Display for ConversionTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConversionTarget::Type(kind) => write!(f, "{kind}"),
            ConversionTarget::Unit { unit_name } => write!(f, "{unit_name}"),
        }
    }
}

impl fmt::Display for ArithmeticComputation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArithmeticComputation::Add => write!(f, "+"),
            ArithmeticComputation::Subtract => write!(f, "-"),
            ArithmeticComputation::Multiply => write!(f, "*"),
            ArithmeticComputation::Divide => write!(f, "/"),
            ArithmeticComputation::Modulo => write!(f, "%"),
            ArithmeticComputation::Power => write!(f, "^"),
        }
    }
}

impl fmt::Display for ComparisonComputation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ComparisonComputation::GreaterThan => write!(f, ">"),
            ComparisonComputation::LessThan => write!(f, "<"),
            ComparisonComputation::GreaterThanOrEqual => write!(f, ">="),
            ComparisonComputation::LessThanOrEqual => write!(f, "<="),
            ComparisonComputation::Is => write!(f, "is"),
            ComparisonComputation::IsNot => write!(f, "is not"),
        }
    }
}

impl fmt::Display for MathematicalComputation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MathematicalComputation::Sqrt => write!(f, "sqrt"),
            MathematicalComputation::Sin => write!(f, "sin"),
            MathematicalComputation::Cos => write!(f, "cos"),
            MathematicalComputation::Tan => write!(f, "tan"),
            MathematicalComputation::Asin => write!(f, "asin"),
            MathematicalComputation::Acos => write!(f, "acos"),
            MathematicalComputation::Atan => write!(f, "atan"),
            MathematicalComputation::Log => write!(f, "log"),
            MathematicalComputation::Exp => write!(f, "exp"),
            MathematicalComputation::Abs => write!(f, "abs"),
            MathematicalComputation::Floor => write!(f, "floor"),
            MathematicalComputation::Ceil => write!(f, "ceil"),
            MathematicalComputation::Round => write!(f, "round"),
        }
    }
}

// -----------------------------------------------------------------------------
// Primitive type kinds and parent type references
// -----------------------------------------------------------------------------

/// Built-in primitive type kind. Single source of truth for type keywords.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrimitiveKind {
    Boolean,
    Measure,
    MeasureRange,
    Number,
    NumberRange,
    Ratio,
    RatioRange,
    Text,
    Date,
    DateRange,
    Time,
    TimeRange,
}

impl std::fmt::Display for PrimitiveKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            PrimitiveKind::Boolean => "boolean",
            PrimitiveKind::Measure => "measure",
            PrimitiveKind::MeasureRange => "measure range",
            PrimitiveKind::Number => "number",
            PrimitiveKind::NumberRange => "number range",
            PrimitiveKind::Ratio => "ratio",
            PrimitiveKind::RatioRange => "ratio range",
            PrimitiveKind::Text => "text",
            PrimitiveKind::Date => "date",
            PrimitiveKind::DateRange => "date range",
            PrimitiveKind::Time => "time",
            PrimitiveKind::TimeRange => "time range",
        };
        write!(f, "{}", s)
    }
}

/// Parent type in a type definition: built-in primitive or custom type name.
///
/// `name` is the declared type name (the data name that introduces this type).
/// For `data temperature: measure`, name = "temperature", primitive = Measure.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ParentType {
    Primitive {
        primitive: PrimitiveKind,
    },
    Custom {
        name: String,
    },
    /// Parent type defined in another spec: `spec_alias.inner` (e.g. `data x: finance.money`).
    /// `inner` must be [`ParentType::Primitive`] or [`ParentType::Custom`], not nested [`ParentType::Qualified`].
    Qualified {
        spec_alias: String,
        inner: Box<ParentType>,
    },
    /// Range over an element type: `<inner> range` (e.g. `money range`, `date range`).
    Ranged {
        inner: Box<ParentType>,
    },
}

impl std::fmt::Display for ParentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParentType::Primitive { primitive } => write!(f, "{}", primitive),
            ParentType::Custom { name } => write!(f, "{}", name),
            ParentType::Qualified { spec_alias, inner } => {
                write!(f, "{spec_alias}.{inner}")
            }
            ParentType::Ranged { inner } => write!(f, "{inner} range"),
        }
    }
}

// =============================================================================
// AsLemmaSource<Value> — canonical literal formatting
// =============================================================================

/// Wrap a value to emit canonical Lemma source (round-trippable). See module docs.
pub struct AsLemmaSource<'a, T: ?Sized>(pub &'a T);

/// Escape a string and wrap it in double quotes for Lemma source output.
/// Handles `\` and `"` escaping.
pub fn quote_lemma_text(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{}\"", escaped)
}

/// Format a Decimal for Lemma source, preserving precision (trailing zeros).
/// Strips the fractional part only when it is zero (e.g. `100` stays `"100"`,
/// `1.00` stays `"1.00"`). Inserts underscore separators in the integer part
/// when it has 4+ digits (e.g. `30000000.50` → `"30_000_000.50"`).
fn format_decimal_source(n: &Decimal) -> String {
    let raw = if n.fract().is_zero() {
        n.trunc().to_string()
    } else {
        n.to_string()
    };
    group_digits(&raw)
}

/// Insert `_` every 3 digits in the integer part of a numeric string.
/// Handles optional leading `-`/`+` sign and optional fractional part.
/// Only groups when the integer part has 4 or more digits.
fn group_digits(s: &str) -> String {
    let (sign, rest) = if s.starts_with('-') || s.starts_with('+') {
        (&s[..1], &s[1..])
    } else {
        ("", s)
    };

    let (int_part, frac_part) = match rest.find('.') {
        Some(pos) => (&rest[..pos], &rest[pos..]),
        None => (rest, ""),
    };

    if int_part.len() < 4 {
        return s.to_string();
    }

    let mut grouped = String::with_capacity(int_part.len() + int_part.len() / 3);
    for (i, ch) in int_part.chars().enumerate() {
        let digits_remaining = int_part.len() - i;
        if i > 0 && digits_remaining % 3 == 0 {
            grouped.push('_');
        }
        grouped.push(ch);
    }

    format!("{}{}{}", sign, grouped, frac_part)
}

impl<'a> fmt::Display for AsLemmaSource<'a, CommandArg> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use crate::literals::Value;
        match self.0 {
            CommandArg::Literal(Value::Text(s)) => write!(f, "{}", quote_lemma_text(s)),
            CommandArg::Literal(Value::Number(d)) => {
                write!(f, "{}", group_digits(&d.to_string()))
            }
            CommandArg::Literal(Value::Boolean(bv)) => write!(f, "{}", bv),
            CommandArg::Literal(Value::NumberWithUnit(d, unit)) => {
                write!(f, "{} {}", group_digits(&d.to_string()), unit)
            }
            CommandArg::Literal(value @ Value::Range(_, _)) => {
                write!(f, "{}", AsLemmaSource(value))
            }
            CommandArg::Literal(Value::Date(dt)) => write!(f, "{}", dt),
            CommandArg::Literal(Value::Time(t)) => write!(f, "{}", t),
            CommandArg::Label(s) => write!(f, "{}", s),
            CommandArg::UnitExpr(unit_arg) => write!(f, "{}", unit_arg),
        }
    }
}

/// Format `command key: value` for assignment continuations (`unit`, `with`).
pub(crate) fn format_assignment_continuation(
    command: &str,
    key: &str,
    value: &impl fmt::Display,
) -> String {
    format!("{} {}: {}", command, key, value)
}

/// Format a single constraint command and its args as valid Lemma source.
pub(crate) fn format_constraint_as_source(
    cmd: &TypeConstraintCommand,
    args: &[CommandArg],
) -> String {
    if *cmd == TypeConstraintCommand::Unit {
        let Some(CommandArg::Label(name)) = args.first() else {
            return cmd.to_string();
        };
        let Some(CommandArg::UnitExpr(unit_arg)) = args.get(1) else {
            return format!("{} {}", cmd, name);
        };
        return format_assignment_continuation("unit", name, unit_arg);
    }

    if args.is_empty() {
        cmd.to_string()
    } else {
        let args_str: Vec<String> = args
            .iter()
            .map(|a| format!("{}", AsLemmaSource(a)))
            .collect();
        format!("{} {}", cmd, args_str.join(" "))
    }
}

/// Format a constraint list as valid Lemma source.
/// Returns the `cmd arg -> cmd arg` portion joined by `separator`.
fn format_constraints_as_source(constraints: &[Constraint], separator: &str) -> String {
    constraints
        .iter()
        .map(|row| format_constraint_as_source(&row.command, &row.args))
        .collect::<Vec<_>>()
        .join(separator)
}

// -- Display for AsLemmaSource<Value> ----------------------------------------

impl<'a> fmt::Display for AsLemmaSource<'a, Value> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Value::Number(n) => write!(f, "{}", format_decimal_source(n)),
            Value::Text(s) => write!(f, "{}", quote_lemma_text(s)),
            Value::Date(dt) => match dt.granularity {
                crate::literals::DateGranularity::Year => write!(f, "{:04}", dt.year),
                crate::literals::DateGranularity::YearMonth => {
                    write!(f, "{:04}-{:02}", dt.year, dt.month)
                }
                crate::literals::DateGranularity::IsoWeek { iso_year, week } => {
                    write!(f, "{:04}-W{:02}", iso_year, week)
                }
                crate::literals::DateGranularity::Full => {
                    write!(f, "{:04}-{:02}-{:02}", dt.year, dt.month, dt.day)
                }
                crate::literals::DateGranularity::DateTime => {
                    write!(
                        f,
                        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
                        dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second
                    )?;
                    if let Some(tz) = &dt.timezone {
                        write!(f, "{}", tz)?;
                    }
                    Ok(())
                }
            },
            Value::Time(t) => {
                write!(f, "{:02}:{:02}:{:02}", t.hour, t.minute, t.second)?;
                if let Some(tz) = &t.timezone {
                    write!(f, "{}", tz)?;
                }
                Ok(())
            }
            Value::Boolean(b) => write!(f, "{}", b),
            Value::NumberWithUnit(n, u) => match u.as_str() {
                "percent" => write!(f, "{}%", format_decimal_source(n)),
                "permille" => write!(f, "{}%%", format_decimal_source(n)),
                unit => write!(f, "{} {}", format_decimal_source(n), unit),
            },
            Value::Range(left, right) => {
                write!(
                    f,
                    "{}...{}",
                    AsLemmaSource(left.as_ref()),
                    AsLemmaSource(right.as_ref())
                )
            }
        }
    }
}

// -- AsLemmaSource: DataValue (formatter / round-trip) ---

impl<'a> fmt::Display for AsLemmaSource<'a, DataValue> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            DataValue::Definition {
                base,
                constraints,
                value,
            } => {
                if base.is_none() && constraints.is_none() {
                    if let Some(v) = value {
                        return write!(f, "{}", AsLemmaSource(v));
                    }
                }
                let base_str = match base.as_ref() {
                    Some(b) => format!("{}", b),
                    None => match value {
                        Some(v) => {
                            if let Some(ref constraints_vec) = constraints {
                                let constraint_str =
                                    format_constraints_as_source(constraints_vec, " -> ");
                                return write!(f, "{} -> {}", AsLemmaSource(v), constraint_str);
                            }
                            return write!(f, "{}", AsLemmaSource(v));
                        }
                        None => String::new(),
                    },
                };
                if let Some(ref constraints_vec) = constraints {
                    let constraint_str = format_constraints_as_source(constraints_vec, " -> ");
                    write!(f, "{} -> {}", base_str, constraint_str)
                } else {
                    write!(f, "{}", base_str)
                }
            }
            DataValue::Import {
                spec_ref,
                bindings: _,
            } => {
                write!(f, "uses {}", spec_ref)
            }
        }
    }
}

pub(crate) fn canonicalize_value(value: &mut Value) {
    if let Value::NumberWithUnit(_, unit) = value {
        *unit = ascii_lowercase_logical_name(std::mem::take(unit));
    }
}

pub(crate) fn canonicalize_reference(reference: &mut Reference) {
    for segment in &mut reference.segments {
        *segment = ascii_lowercase_logical_name(std::mem::take(segment));
    }
    reference.name = ascii_lowercase_logical_name(std::mem::take(&mut reference.name));
}

pub(crate) fn canonicalize_spec_ref(spec_ref: &mut SpecRef) {
    spec_ref.name = ascii_lowercase_logical_name(std::mem::take(&mut spec_ref.name));
    if let Some(qualifier) = spec_ref.repository.as_mut() {
        qualifier.name = ascii_lowercase_logical_name(std::mem::take(&mut qualifier.name));
    }
}

pub(crate) fn canonicalize_parent_type(parent: &mut ParentType) {
    match parent {
        ParentType::Custom { name } => {
            *name = ascii_lowercase_logical_name(std::mem::take(name));
        }
        ParentType::Qualified { spec_alias, inner } => {
            *spec_alias = ascii_lowercase_logical_name(std::mem::take(spec_alias));
            canonicalize_parent_type(inner);
        }
        ParentType::Ranged { inner } => {
            canonicalize_parent_type(inner);
        }
        ParentType::Primitive { .. } => {}
    }
}

pub(crate) fn canonicalize_unit_factor(factor: &mut UnitFactor) {
    factor.measure_ref = ascii_lowercase_logical_name(std::mem::take(&mut factor.measure_ref));
}

pub(crate) fn canonicalize_unit_arg(unit_arg: &mut UnitArg) {
    if let UnitArg::Expr(_, factors) = unit_arg {
        for factor in factors {
            canonicalize_unit_factor(factor);
        }
    }
}

pub(crate) fn canonicalize_command_arg(command_arg: &mut CommandArg) {
    match command_arg {
        CommandArg::Literal(value) => canonicalize_value(value),
        CommandArg::Label(label) => {
            *label = ascii_lowercase_logical_name(std::mem::take(label));
        }
        CommandArg::UnitExpr(unit_arg) => canonicalize_unit_arg(unit_arg),
    }
}

pub(crate) fn canonicalize_constraints(constraints: &mut [Constraint]) {
    for row in constraints {
        for arg in &mut row.args {
            canonicalize_command_arg(arg);
        }
    }
}

pub(crate) fn canonicalize_expression(expression: &mut Expression) {
    match &mut expression.kind {
        ExpressionKind::Literal(value) => canonicalize_value(value),
        ExpressionKind::Reference(reference) => canonicalize_reference(reference),
        ExpressionKind::Now => {}
        ExpressionKind::DateRelative(_, expression) => {
            canonicalize_expression(Arc::make_mut(expression));
        }
        ExpressionKind::DateCalendar(_, _, expression) => {
            canonicalize_expression(Arc::make_mut(expression));
        }
        ExpressionKind::RangeLiteral(left, right) => {
            canonicalize_expression(Arc::make_mut(left));
            canonicalize_expression(Arc::make_mut(right));
        }
        ExpressionKind::PastFutureRange(_, expression) => {
            canonicalize_expression(Arc::make_mut(expression));
        }
        ExpressionKind::RangeContainment(value, range) => {
            canonicalize_expression(Arc::make_mut(value));
            canonicalize_expression(Arc::make_mut(range));
        }
        ExpressionKind::LogicalAnd(left, right) => {
            canonicalize_expression(Arc::make_mut(left));
            canonicalize_expression(Arc::make_mut(right));
        }
        ExpressionKind::Arithmetic(left, _, right) => {
            canonicalize_expression(Arc::make_mut(left));
            canonicalize_expression(Arc::make_mut(right));
        }
        ExpressionKind::Comparison(left, _, right) => {
            canonicalize_expression(Arc::make_mut(left));
            canonicalize_expression(Arc::make_mut(right));
        }
        ExpressionKind::UnitConversion(expression, _) => {
            canonicalize_expression(Arc::make_mut(expression));
        }
        ExpressionKind::LogicalNegation(expression, _) => {
            canonicalize_expression(Arc::make_mut(expression));
        }
        ExpressionKind::MathematicalComputation(_, expression) => {
            canonicalize_expression(Arc::make_mut(expression));
        }
        ExpressionKind::Veto(_) => {}
        ExpressionKind::ResultIsVeto(expression) => {
            canonicalize_expression(Arc::make_mut(expression));
        }
    }
}

pub(crate) fn canonicalize_unless_clause(unless_clause: &mut UnlessClause) {
    canonicalize_expression(&mut unless_clause.condition);
    canonicalize_expression(&mut unless_clause.result);
}

pub(crate) fn canonicalize_data_value(data_value: &mut DataValue) {
    match data_value {
        DataValue::Definition {
            base,
            constraints,
            value,
        } => {
            if let Some(base) = base {
                canonicalize_parent_type(base);
            }
            if let Some(constraints) = constraints {
                canonicalize_constraints(constraints);
            }
            if let Some(value) = value {
                canonicalize_value(value);
            }
        }
        DataValue::Import { spec_ref, bindings } => {
            canonicalize_spec_ref(spec_ref);
            for binding in bindings {
                canonicalize_reference(&mut binding.path);
                match &mut binding.rhs {
                    WithRhs::Literal(value) => canonicalize_value(value),
                    WithRhs::Reference { target } => canonicalize_reference(target),
                }
            }
        }
    }
}

pub(crate) fn canonicalize_lemma_data(data: &mut LemmaData) {
    canonicalize_reference(&mut data.reference);
    canonicalize_data_value(&mut data.value);
}

pub(crate) fn canonicalize_lemma_rule(rule: &mut LemmaRule) {
    rule.name = ascii_lowercase_logical_name(std::mem::take(&mut rule.name));
    canonicalize_expression(&mut rule.expression);
    for unless_clause in &mut rule.unless_clauses {
        canonicalize_unless_clause(unless_clause);
    }
}

pub(crate) fn canonicalize_lemma_spec(spec: &mut LemmaSpec) {
    spec.name = ascii_lowercase_logical_name(std::mem::take(&mut spec.name));
    for meta in &mut spec.meta_fields {
        meta.key = ascii_lowercase_logical_name(std::mem::take(&mut meta.key));
    }
    for data in &mut spec.data {
        canonicalize_lemma_data(data);
    }
    for rule in &mut spec.rules {
        canonicalize_lemma_rule(rule);
    }
}

pub(crate) fn canonicalize_repository(repository: &mut LemmaRepository) {
    if let Some(name) = repository.name.take() {
        repository.name = Some(ascii_lowercase_logical_name(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::literals::DateGranularity;

    #[test]
    fn test_conversion_target_display() {
        assert_eq!(
            format!("{}", ConversionTarget::Type(PrimitiveKind::Number)),
            "number"
        );
    }

    #[test]
    fn test_value_number_with_unit_ratio_display() {
        use rust_decimal::Decimal;
        use std::str::FromStr;
        let percent =
            Value::NumberWithUnit(Decimal::from_str("10").unwrap(), "percent".to_string());
        assert_eq!(format!("{}", percent), "10%");
        let permille =
            Value::NumberWithUnit(Decimal::from_str("5").unwrap(), "permille".to_string());
        assert_eq!(format!("{}", permille), "5%%");
    }

    #[test]
    fn test_datetime_value_display() {
        let dt = DateTimeValue {
            year: 2024,
            month: 12,
            day: 25,
            hour: 14,
            minute: 30,
            second: 45,
            microsecond: 0,
            timezone: Some(TimezoneValue {
                offset_hours: 1,
                offset_minutes: 0,
            }),

            granularity: DateGranularity::DateTime,
        };
        assert_eq!(format!("{}", dt), "2024-12-25T14:30:45+01:00");
    }

    #[test]
    fn test_datetime_value_display_date_only() {
        let dt = DateTimeValue {
            year: 2026,
            month: 3,
            day: 4,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,

            granularity: DateGranularity::Full,
        };
        assert_eq!(format!("{}", dt), "2026-03-04");
    }

    #[test]
    fn test_datetime_value_display_microseconds() {
        let dt = DateTimeValue {
            year: 2026,
            month: 2,
            day: 23,
            hour: 14,
            minute: 30,
            second: 45,
            microsecond: 123456,
            timezone: Some(TimezoneValue {
                offset_hours: 0,
                offset_minutes: 0,
            }),

            granularity: DateGranularity::DateTime,
        };
        assert_eq!(format!("{}", dt), "2026-02-23T14:30:45.123456Z");
    }

    #[test]
    fn test_datetime_microsecond_in_ordering() {
        let a = DateTimeValue {
            year: 2026,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 100,
            timezone: None,

            granularity: DateGranularity::DateTime,
        };
        let b = DateTimeValue {
            year: 2026,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 200,
            timezone: None,

            granularity: DateGranularity::DateTime,
        };
        assert!(a < b);
    }

    #[test]
    fn test_datetime_parse_iso_week() {
        let dt: DateTimeValue = "2026-W01".parse().unwrap();
        assert_eq!(dt.year, 2025);
        assert_eq!(dt.month, 12);
        assert_eq!(dt.day, 29);
        assert_eq!(dt.microsecond, 0);
        assert_eq!(dt.to_string(), "2026-W01");
        assert!(matches!(
            dt.granularity,
            DateGranularity::IsoWeek {
                iso_year: 2026,
                week: 1
            }
        ));
    }

    #[test]
    fn test_negation_types() {
        let json = serde_json::to_string(&NegationType::Not).expect("serialize NegationType");
        let decoded: NegationType = serde_json::from_str(&json).expect("deserialize NegationType");
        assert_eq!(decoded, NegationType::Not);
    }

    #[test]
    fn parent_type_primitive_serde_externally_tagged() {
        let p = ParentType::Primitive {
            primitive: PrimitiveKind::Number,
        };
        let json = serde_json::to_string(&p).expect("ParentType::Primitive must serialize");
        assert!(json.contains("\"Primitive\"") && json.contains("\"primitive\""));
        let back: ParentType = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, p);
    }

    // =====================================================================
    // DataValue Display — constraint formatting
    // =====================================================================

    fn text_arg(s: &str) -> CommandArg {
        CommandArg::Literal(crate::literals::Value::Text(s.to_string()))
    }

    fn number_arg(s: &str) -> CommandArg {
        let d: rust_decimal::Decimal = s.parse().expect("decimal");
        CommandArg::Literal(crate::literals::Value::Number(d))
    }

    fn boolean_arg(b: BooleanValue) -> CommandArg {
        CommandArg::Literal(crate::literals::Value::Boolean(b))
    }

    fn measure_arg(value: &str, unit: &str) -> CommandArg {
        let d: rust_decimal::Decimal = value.parse().expect("decimal");
        CommandArg::Literal(crate::literals::Value::NumberWithUnit(d, unit.to_string()))
    }

    fn duration_arg(value: &str, unit: &str) -> CommandArg {
        let d: rust_decimal::Decimal = value.parse().expect("decimal");
        CommandArg::Literal(crate::literals::Value::NumberWithUnit(d, unit.to_string()))
    }

    #[test]
    fn as_lemma_source_text_default_is_quoted() {
        let fv = DataValue::Definition {
            base: Some(ParentType::Primitive {
                primitive: PrimitiveKind::Text,
            }),
            constraints: Some(vec![test_constraint(
                TypeConstraintCommand::Suggest,
                vec![text_arg("single")],
            )]),
            value: None,
        };
        assert_eq!(
            format!("{}", AsLemmaSource(&fv)),
            "text -> suggest \"single\""
        );
    }

    #[test]
    fn as_lemma_source_number_default_not_quoted() {
        let fv = DataValue::Definition {
            base: Some(ParentType::Primitive {
                primitive: PrimitiveKind::Number,
            }),
            constraints: Some(vec![test_constraint(
                TypeConstraintCommand::Suggest,
                vec![number_arg("10")],
            )]),
            value: None,
        };
        assert_eq!(format!("{}", AsLemmaSource(&fv)), "number -> suggest 10");
    }

    #[test]
    fn as_lemma_source_help_always_quoted() {
        let fv = DataValue::Definition {
            base: Some(ParentType::Primitive {
                primitive: PrimitiveKind::Number,
            }),
            constraints: Some(vec![test_constraint(
                TypeConstraintCommand::Help,
                vec![text_arg("Enter a measure")],
            )]),
            value: None,
        };
        assert_eq!(
            format!("{}", AsLemmaSource(&fv)),
            "number -> help \"Enter a measure\""
        );
    }

    #[test]
    fn as_lemma_source_text_option_quoted() {
        let fv = DataValue::Definition {
            base: Some(ParentType::Primitive {
                primitive: PrimitiveKind::Text,
            }),
            constraints: Some(vec![
                test_constraint(TypeConstraintCommand::Option, vec![text_arg("active")]),
                test_constraint(TypeConstraintCommand::Option, vec![text_arg("inactive")]),
            ]),
            value: None,
        };
        assert_eq!(
            format!("{}", AsLemmaSource(&fv)),
            "text -> option \"active\" -> option \"inactive\""
        );
    }

    #[test]
    fn as_lemma_source_measure_unit_not_quoted() {
        let fv = DataValue::Definition {
            base: Some(ParentType::Primitive {
                primitive: PrimitiveKind::Measure,
            }),
            constraints: Some(vec![
                test_constraint(
                    TypeConstraintCommand::Unit,
                    vec![
                        CommandArg::Label("eur".to_string()),
                        CommandArg::UnitExpr(UnitArg::Factor(decimal("1.00"))),
                    ],
                ),
                test_constraint(
                    TypeConstraintCommand::Unit,
                    vec![
                        CommandArg::Label("usd".to_string()),
                        CommandArg::UnitExpr(UnitArg::Factor(decimal("0.91"))),
                    ],
                ),
            ]),
            value: None,
        };
        assert_eq!(
            format!("{}", AsLemmaSource(&fv)),
            "measure -> unit eur: 1.00 -> unit usd: 0.91"
        );
    }

    #[test]
    fn as_lemma_source_measure_minimum_with_unit() {
        let fv = DataValue::Definition {
            base: Some(ParentType::Primitive {
                primitive: PrimitiveKind::Measure,
            }),
            constraints: Some(vec![test_constraint(
                TypeConstraintCommand::Minimum,
                vec![measure_arg("0", "eur")],
            )]),
            value: None,
        };
        assert_eq!(
            format!("{}", AsLemmaSource(&fv)),
            "measure -> minimum 0 eur"
        );
    }

    #[test]
    fn as_lemma_source_boolean_default() {
        let fv = DataValue::Definition {
            base: Some(ParentType::Primitive {
                primitive: PrimitiveKind::Boolean,
            }),
            constraints: Some(vec![test_constraint(
                TypeConstraintCommand::Suggest,
                vec![boolean_arg(BooleanValue::True)],
            )]),
            value: None,
        };
        assert_eq!(format!("{}", AsLemmaSource(&fv)), "boolean -> suggest true");
    }

    #[test]
    fn as_lemma_source_duration_default() {
        let fv = DataValue::Definition {
            base: Some(ParentType::Custom {
                name: "duration".to_string(),
            }),
            constraints: Some(vec![test_constraint(
                TypeConstraintCommand::Suggest,
                vec![duration_arg("40", "hour")],
            )]),
            value: None,
        };
        assert_eq!(
            format!("{}", AsLemmaSource(&fv)),
            "duration -> suggest 40 hour"
        );
    }

    #[test]
    fn as_lemma_source_named_type_default_quoted() {
        // Named types (user-defined): the parser produces a typed Text literal for
        // quoted suggestion values like `suggest "single"`.
        let fv = DataValue::Definition {
            base: Some(ParentType::Custom {
                name: "filing_status_type".to_string(),
            }),
            constraints: Some(vec![test_constraint(
                TypeConstraintCommand::Suggest,
                vec![text_arg("single")],
            )]),
            value: None,
        };
        assert_eq!(
            format!("{}", AsLemmaSource(&fv)),
            "filing_status_type -> suggest \"single\""
        );
    }

    #[test]
    fn as_lemma_source_help_escapes_quotes() {
        let fv = DataValue::Definition {
            base: Some(ParentType::Primitive {
                primitive: PrimitiveKind::Text,
            }),
            constraints: Some(vec![test_constraint(
                TypeConstraintCommand::Help,
                vec![text_arg("say \"hello\"")],
            )]),
            value: None,
        };
        assert_eq!(
            format!("{}", AsLemmaSource(&fv)),
            "text -> help \"say \\\"hello\\\"\""
        );
    }

    fn unit_arg_expr(prefix: Decimal, factors: &[(&str, i32)]) -> UnitArg {
        UnitArg::Expr(
            prefix,
            factors
                .iter()
                .map(|(measure_ref, exp)| UnitFactor {
                    measure_ref: (*measure_ref).to_string(),
                    exp: *exp,
                })
                .collect(),
        )
    }

    #[test]
    fn unit_arg_display_metre_per_second() {
        let arg = unit_arg_expr(Decimal::ONE, &[("meter", 1), ("second", -1)]);
        assert_eq!(format!("{arg}"), "meter/second");
        assert!(
            !format!("{arg}").contains("second^-1"),
            "must not print denominator as negative exponent"
        );
    }

    #[test]
    fn unit_arg_display_meter_per_second_squared() {
        let arg = unit_arg_expr(Decimal::ONE, &[("meter", 1), ("second", -2)]);
        assert_eq!(format!("{arg}"), "meter/second^2");
    }

    #[test]
    fn unit_arg_display_kg_times_mps2() {
        let arg = unit_arg_expr(Decimal::ONE, &[("kg", 1), ("mps2", 1)]);
        assert_eq!(format!("{arg}"), "kg * mps2");
    }

    #[test]
    fn unit_arg_display_numeric_prefix_metre_per_second() {
        use std::str::FromStr;
        let prefix = Decimal::from_str("3.6").expect("decimal");
        let arg = unit_arg_expr(prefix, &[("meter", 1), ("second", -1)]);
        assert_eq!(format!("{arg}"), "3.6 meter/second");
    }

    #[test]
    fn unit_arg_display_metre_per_second_times_kg() {
        let arg = unit_arg_expr(Decimal::ONE, &[("meter", 1), ("second", -1), ("kg", 1)]);
        assert_eq!(format!("{arg}"), "meter/second * kg");
    }

    #[test]
    fn unit_arg_display_kg_meter_per_second_squared() {
        let arg = unit_arg_expr(Decimal::ONE, &[("kg", 1), ("meter", 1), ("second", -2)]);
        assert_eq!(format!("{arg}"), "kg * meter/second^2");
    }

    // ─── Assignment continuation formatting (red until formatter lands) ───────

    #[test]
    fn format_constraint_as_source_unit_factor_uses_assignment_colon() {
        let args = vec![
            CommandArg::Label("eur".to_string()),
            CommandArg::UnitExpr(UnitArg::Factor(decimal("1"))),
        ];
        assert_eq!(
            format_constraint_as_source(&TypeConstraintCommand::Unit, &args),
            "unit eur: 1"
        );
    }

    #[test]
    fn format_constraint_as_source_unit_compound_uses_assignment_colon() {
        let arg = unit_arg_expr(decimal("3.6"), &[("meter", 1), ("second", -1)]);
        let args = vec![
            CommandArg::Label("kmh".to_string()),
            CommandArg::UnitExpr(arg),
        ];
        assert_eq!(
            format_constraint_as_source(&TypeConstraintCommand::Unit, &args),
            "unit kmh: 3.6 meter/second"
        );
    }

    #[test]
    fn format_constraint_as_source_unit_factor_one_decimal_uses_assignment_colon() {
        let args = vec![
            CommandArg::Label("eur".to_string()),
            CommandArg::UnitExpr(UnitArg::Factor(decimal("1.00"))),
        ];
        assert_eq!(
            format_constraint_as_source(&TypeConstraintCommand::Unit, &args),
            "unit eur: 1.00"
        );
    }

    #[test]
    fn as_lemma_source_measure_unit_uses_assignment_colon() {
        let fv = DataValue::Definition {
            base: Some(ParentType::Primitive {
                primitive: PrimitiveKind::Measure,
            }),
            constraints: Some(vec![
                test_constraint(
                    TypeConstraintCommand::Unit,
                    vec![
                        CommandArg::Label("eur".to_string()),
                        CommandArg::UnitExpr(UnitArg::Factor(decimal("1.00"))),
                    ],
                ),
                test_constraint(
                    TypeConstraintCommand::Unit,
                    vec![
                        CommandArg::Label("usd".to_string()),
                        CommandArg::UnitExpr(UnitArg::Factor(decimal("0.91"))),
                    ],
                ),
            ]),
            value: None,
        };
        assert_eq!(
            format!("{}", AsLemmaSource(&fv)),
            "measure -> unit eur: 1.00 -> unit usd: 0.91"
        );
    }

    fn decimal(value: &str) -> rust_decimal::Decimal {
        use std::str::FromStr;
        rust_decimal::Decimal::from_str(value).expect("decimal literal in test")
    }
}
