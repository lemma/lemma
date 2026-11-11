use crate::computation::{OperationResult, VetoType};
use crate::evaluation::explanations::Explanation;

use crate::error::Error;
use crate::evaluation::evaluate_data::EvaluateDataValue;
use crate::evaluation::rows::{row_result, take_row_context};
use crate::evaluation::tree::ensure_rule_values;
use crate::evaluation::EvaluationContext;
use crate::parsing::ast::DateTimeValue;
use crate::planning::execution_plan::ExecutionPlan;
use crate::planning::semantics::{
    DataDefinition, DataPath, InstanceKey, LemmaType, LiteralValue, ReferenceEnd, RulePath, Source,
    SpecValue, ValueKind,
};
use crate::planning::unit_family::FamilyUnitCatalog;
use crate::result_value::{
    rule_result_value_failure_message, rule_result_value_from_literal, RowReference, RowResult,
    RuleResultValue, RuleResultValueFailure, SpecResult, SpecRuleResult,
};
use indexmap::IndexMap;
use std::collections::HashMap;
use std::sync::Arc;

/// Rule info with resolved expressions for use in evaluation response.
/// Evaluation uses only semantics types; no parsing types.
#[derive(Debug, Clone)]
pub struct EvaluatedRule {
    pub name: String,
    pub path: RulePath,
    pub source_location: Source,
    pub rule_type: Arc<LemmaType>,
}

/// Response from evaluating a Lemma spec
#[derive(Debug, Clone)]
pub struct Response {
    pub spec_name: String,
    pub effective: String,
    /// Declared temporal window `[spec_effective_from, spec_effective_to)` of the
    /// resolved spec version. Set by [`crate::Engine::evaluate`] after evaluation;
    /// `None` here (evaluation-internal construction) until that assignment.
    pub spec_effective_from: Option<DateTimeValue>,
    pub spec_effective_to: Option<DateTimeValue>,
    pub results: IndexMap<String, RuleResult>,
    /// Rows reached by `results`, indexed by [`RowReference::index`].
    pub rows: Vec<RowResult>,
}

/// Result of evaluating a single rule. Struct fields match the API JSON shape.
#[derive(Debug, Clone)]
pub struct RuleResult {
    pub rule: EvaluatedRule,
    pub veto_detail: Option<VetoType>,

    pub vetoed: bool,
    pub veto_reason: Option<String>,
    pub rule_type: String,

    /// Flattened value fields, including `result` when the rule is not vetoed.
    pub result: Option<RuleResultValue>,
    pub explanation: Option<Explanation>,
    /// Unbound caller data paths still live for this rule under the current run data
    /// (`DataPath::input_key` strings; subset of `Show.data` keys with non-empty
    /// `needed_by_rules`). Ordered by evaluation / decision-tree preorder (first key =
    /// next fact the live tree needs); `Show.data` stays declaration order.
    missing_data: Vec<String>,
    /// Closed options for keys in [`Self::missing_data`]. A key with no entry is not
    /// constrained by a set. An empty list is a closed set with nothing valid.
    /// Each value is what the next evaluate accepts.
    missing_data_options: IndexMap<String, Vec<EvaluateDataValue>>,
}

impl RuleResult {
    /// Engine-rendered one-liner from the flattened [`RuleResultValue`].
    #[must_use]
    pub fn result(&self) -> Option<&str> {
        self.result
            .as_ref()
            .and_then(|value| value.result.as_deref())
    }

    /// True when this result is a [`VetoType::MissingData`] or [`VetoType::MissingRowData`] veto.
    ///
    /// Value and other vetoes are settled answers; leftover live keys in
    /// [`Self::missing_data`] must not drive prompts or human "Missing data" display.
    #[must_use]
    pub fn is_missing_data(&self) -> bool {
        matches!(
            self.veto_detail.as_ref(),
            Some(VetoType::MissingData { .. } | VetoType::MissingRowData { .. })
        )
    }

    /// Unbound caller data paths still live for this rule (`DataPath::input_key`),
    /// in evaluation / decision-tree order (contrast `Show.data` declaration order).
    #[must_use]
    pub fn missing_data(&self) -> &[String] {
        &self.missing_data
    }

    /// Closed sets for this rule's missing inputs. Keys are a subset of [`Self::missing_data`].
    #[must_use]
    pub fn missing_data_options(&self) -> &IndexMap<String, Vec<EvaluateDataValue>> {
        &self.missing_data_options
    }

    pub(crate) fn with_missing_data_options(
        mut self,
        missing_data_options: IndexMap<String, Vec<EvaluateDataValue>>,
    ) -> Self {
        for key in missing_data_options.keys() {
            if !self.missing_data.iter().any(|listed| listed == key) {
                panic!(
                    "BUG: missing_data_options key '{key}' is not in missing_data {:?}",
                    self.missing_data
                );
            }
        }
        self.missing_data_options = missing_data_options;
        self
    }

    /// Build a [`RuleResult`] for API output from a rule evaluation result.
    ///
    /// Measure and ratio payloads expand into every unit in the result type's family.
    pub(crate) fn from_operation_result(
        rule: EvaluatedRule,
        operation_result: &OperationResult,
        rule_type: &LemmaType,
        family_units: &FamilyUnitCatalog,
        explanation: Option<Explanation>,
        missing_data: Vec<String>,
    ) -> Self {
        Self::from_evaluated(
            rule,
            operation_result,
            rule_type,
            family_units,
            explanation,
            missing_data,
            None,
        )
    }

    pub(crate) fn from_evaluated(
        rule: EvaluatedRule,
        operation_result: &OperationResult,
        rule_type: &LemmaType,
        family_units: &FamilyUnitCatalog,
        explanation: Option<Explanation>,
        missing_data: Vec<String>,
        runtime_scales: Option<
            &std::collections::HashMap<String, crate::computation::rational::RationalInteger>,
        >,
    ) -> Self {
        match operation_result {
            OperationResult::Veto(VetoType::MissingData { data, .. }) => {
                let key = data.input_key();
                if !missing_data.iter().any(|listed| listed == &key) {
                    panic!("BUG: MissingData path {key} not in missing_data {missing_data:?}");
                }
            }
            OperationResult::Veto(VetoType::MissingRowData { key }) => {
                if !missing_data.contains(key) {
                    panic!("BUG: missing row field {key} not in missing_data {missing_data:?}");
                }
            }
            _ => {
                if !missing_data.is_empty() {
                    panic!(
                        "BUG: missing_data must be empty when result is not MissingData: {missing_data:?}"
                    );
                }
            }
        }
        let rule_type_name = rule_type.name().to_string();
        match operation_result {
            OperationResult::Veto(veto) => Self::vetoed_with(
                rule,
                veto.clone(),
                rule_type_name,
                explanation,
                missing_data,
                IndexMap::new(),
            ),
            OperationResult::Value(bound) => {
                if (rule_type.is_measure() || rule_type.is_measure_range())
                    && !crate::planning::semantics::measure_family_has_expressible_unit(
                        rule_type,
                        runtime_scales,
                    )
                {
                    return Self::vetoed_with(
                        rule,
                        VetoType::computation(format!(
                            "no conversion factor is known for any unit of {}",
                            rule_type.name()
                        )),
                        rule_type_name,
                        explanation,
                        missing_data,
                        IndexMap::new(),
                    );
                }
                match rule_result_value_from_literal(
                    &bound.to_literal(),
                    rule_type,
                    family_units,
                    runtime_scales,
                ) {
                    Ok(value) => Self {
                        rule,
                        veto_detail: None,
                        vetoed: false,
                        veto_reason: None,
                        rule_type: rule_type_name,
                        result: Some(value),
                        explanation,
                        missing_data,
                        missing_data_options: IndexMap::new(),
                    },
                    Err(failure) => vetoed_rule_result_for_rule_result_value_failure(
                        rule,
                        rule_type,
                        explanation,
                        failure,
                        missing_data,
                    ),
                }
            }
        }
    }

    fn vetoed_with(
        rule: EvaluatedRule,
        veto: VetoType,
        rule_type: String,
        explanation: Option<Explanation>,
        missing_data: Vec<String>,
        missing_data_options: IndexMap<String, Vec<EvaluateDataValue>>,
    ) -> Self {
        Self {
            rule,
            veto_reason: match &veto {
                VetoType::UserDefined { message: None } => None,
                _ => Some(veto.to_string()),
            },
            veto_detail: Some(veto),
            vetoed: true,
            rule_type,
            result: None,
            explanation,
            missing_data,
            missing_data_options,
        }
    }

    /// Reconstruct the evaluated [`LiteralValue`] from committed [`RuleResultValue`] fields.
    ///
    /// Panics if the rule is vetoed or fields cannot be reconstructed.
    pub fn to_literal(&self) -> LiteralValue {
        assert!(
            !self.vetoed,
            "BUG: to_literal called on vetoed rule '{}'",
            self.rule.name
        );
        let value = self
            .result
            .as_ref()
            .unwrap_or_else(|| panic!("BUG: non-vetoed rule '{}' missing result", self.rule.name));
        value.to_literal(&self.rule.rule_type)
    }
}

fn vetoed_rule_result_for_rule_result_value_failure(
    rule: EvaluatedRule,
    rule_type: &LemmaType,
    explanation: Option<Explanation>,
    failure: RuleResultValueFailure,
    missing_data: Vec<String>,
) -> RuleResult {
    RuleResult::vetoed_with(
        rule,
        VetoType::computation(rule_result_value_failure_message(failure).to_string()),
        rule_type.name().to_string(),
        explanation,
        missing_data,
        IndexMap::new(),
    )
}

impl Response {
    /// Looks up a rule result by name.
    ///
    /// Returns an error if the rule is not found.
    pub fn get(&self, rule_name: &str) -> Result<&RuleResult, Error> {
        self.results
            .get(rule_name)
            .ok_or_else(|| crate::error::Error::rule_not_found(rule_name, None::<String>))
    }

    pub fn add_result(&mut self, result: RuleResult) {
        self.results.insert(result.rule.name.clone(), result);
    }
}

/// Distinct rows reached by one response, in first-reached order. Each row is filled
/// once and referenced by index everywhere else, so output grows with the number of
/// rows, not with the number of paths that reach them.
#[derive(Default)]
pub(crate) struct RowTable {
    index: HashMap<InstanceKey, usize>,
    rows: Vec<Option<RowResult>>,
}

impl RowTable {
    pub(crate) fn into_rows(self) -> Vec<RowResult> {
        self.rows
            .into_iter()
            .enumerate()
            .map(|(index, row)| row.unwrap_or_else(|| panic!("BUG: row {index} never filled")))
            .collect()
    }
}

/// Fill every spec value inside `literal`'s API `value`: a planned instance (a `uses`
/// import or a row written in a spec, at any depth) gets its data and rules inline,
/// any other row a reference into `table`.
pub(crate) fn fill_spec_values(
    literal: &ValueKind,
    value: &mut RuleResultValue,
    plan: &ExecutionPlan,
    ctx: &mut EvaluationContext,
    table: &mut RowTable,
) {
    if matches!(
        literal,
        ValueKind::Number(_)
            | ValueKind::Measure(_)
            | ValueKind::Ratio(_)
            | ValueKind::Text(_)
            | ValueKind::Date(_)
            | ValueKind::Time(_)
            | ValueKind::Boolean(_)
            | ValueKind::Range(_, _)
    ) {
        return;
    }
    let mut walk = Walk {
        root: plan,
        base: ctx,
        table,
        scopes: Vec::new(),
        ancestors: Vec::new(),
    };
    let root = Frame::fill(literal.clone(), std::mem::take(value));
    *value = walk.run(root).into_value();
}

/// Context of an instance whose members are being walked, over its template.
struct Scope<'a> {
    plan: &'a ExecutionPlan,
    ctx: EvaluationContext,
}

/// Shared state of one fill walk. `root` is the plan being evaluated and `base` its
/// context; `scopes` holds the instance contexts entered, innermost last, taken out of
/// the context cache while their members are walked.
struct Walk<'a, 'b> {
    root: &'a ExecutionPlan,
    base: &'b mut EvaluationContext,
    table: &'b mut RowTable,
    scopes: Vec<Scope<'a>>,
    ancestors: Vec<InstanceKey>,
}

impl<'a> Walk<'a, '_> {
    fn ctx(&mut self) -> &mut EvaluationContext {
        match self.scopes.last_mut() {
            Some(scope) => &mut scope.ctx,
            None => &mut *self.base,
        }
    }

    /// Plan and context `key`'s context is built from: its owner's scope while the walk
    /// is inside it, else the root, which finds a root-owned row's bindings in itself
    /// and any other owner's context in the cache.
    fn owner_scope(&mut self, key: &InstanceKey) -> (&'a ExecutionPlan, &mut EvaluationContext) {
        let position = key.owner().and_then(|owner| {
            self.scopes
                .iter()
                .rposition(|scope| scope.ctx.owner_row.as_ref() == Some(owner))
        });
        match position {
            Some(position) => {
                let scope = &mut self.scopes[position];
                (scope.plan, &mut scope.ctx)
            }
            None => (self.root, &mut *self.base),
        }
    }

    fn run(&mut self, root: Frame<'a>) -> Output {
        let mut frames = vec![root];
        let mut input = None;
        loop {
            let frame = frames.last_mut().expect("BUG: fill walk frame stack empty");
            match frame.resume(self, input.take()) {
                Step::Call(child) => frames.push(child),
                Step::Return(output) => {
                    frames.pop();
                    if frames.is_empty() {
                        return output;
                    }
                    input = Some(output);
                }
            }
        }
    }
}

enum Frame<'a> {
    Fill(Box<FillFrame>),
    Instance(InstanceFrame),
    Members(Box<MembersFrame<'a>>),
}

impl<'a> Frame<'a> {
    fn fill(literal: ValueKind, value: RuleResultValue) -> Self {
        Frame::Fill(Box::new(FillFrame::new(literal, value)))
    }

    fn resume(&mut self, walk: &mut Walk<'a, '_>, input: Option<Output>) -> Step<'a> {
        match self {
            Frame::Fill(frame) => frame.resume(walk, input),
            Frame::Instance(frame) => frame.resume(walk, input),
            Frame::Members(frame) => frame.resume(walk, input),
        }
    }
}

enum Step<'a> {
    Call(Frame<'a>),
    Return(Output),
}

enum Output {
    Value(Box<RuleResultValue>),
    Spec(Box<SpecResult>),
    Row(RowReference),
    Members(FilledMembers),
}

impl Output {
    fn into_value(self) -> RuleResultValue {
        match self {
            Output::Value(value) => *value,
            _ => panic!("BUG: fill walk expected a value output"),
        }
    }

    fn into_spec(self) -> SpecResult {
        match self {
            Output::Spec(spec) => *spec,
            _ => panic!("BUG: fill walk expected a spec output"),
        }
    }

    fn into_row(self) -> RowReference {
        match self {
            Output::Row(row) => row,
            _ => panic!("BUG: fill walk expected a row output"),
        }
    }

    fn into_members(self) -> FilledMembers {
        match self {
            Output::Members(members) => members,
            _ => panic!("BUG: fill walk expected a members output"),
        }
    }
}

fn child_output(input: Option<Output>) -> Output {
    input.expect("BUG: fill walk frame resumed without child output")
}

enum FillState {
    Start(ValueKind),
    AwaitSpec,
    /// The table row a value publishes under, labelled with the value's instance.
    AwaitRow(Option<String>),
    Items(
        std::vec::IntoIter<(ValueKind, RuleResultValue)>,
        Vec<RuleResultValue>,
    ),
}

/// Fills one value: a planned instance gets its data and rules inline, any other row a
/// reference into the table, a list fills each entry in order.
struct FillFrame {
    value: RuleResultValue,
    state: Option<FillState>,
}

impl FillFrame {
    fn new(literal: ValueKind, value: RuleResultValue) -> Self {
        Self {
            value,
            state: Some(FillState::Start(literal)),
        }
    }

    fn resume<'a>(&mut self, walk: &mut Walk<'a, '_>, input: Option<Output>) -> Step<'a> {
        match self
            .state
            .take()
            .expect("BUG: fill frame resumed after completion")
        {
            FillState::Start(literal) => {
                assert!(
                    input.is_none(),
                    "BUG: fill frame started with a child output"
                );
                self.start(walk, literal)
            }
            FillState::AwaitSpec => {
                self.value.spec = Some(Box::new(child_output(input).into_spec()));
                self.finish()
            }
            FillState::AwaitRow(instance) => {
                let mut reference = child_output(input).into_row();
                reference.instance = instance;
                self.value.row = Some(reference);
                self.finish()
            }
            FillState::Items(items, mut filled) => {
                filled.push(child_output(input).into_value());
                self.next_item(items, filled)
            }
        }
    }

    fn start<'a>(&mut self, walk: &mut Walk<'a, '_>, literal: ValueKind) -> Step<'a> {
        match literal {
            ValueKind::Spec(SpecValue::Row(key)) => {
                if key.is_planned() {
                    assert!(
                        self.value.spec.is_some(),
                        "BUG: planned instance '{key}' without spec arm"
                    );
                    if walk.ancestors.contains(&key) {
                        panic!("BUG: spec result walked instance '{key}' twice");
                    }
                    self.state = Some(FillState::AwaitSpec);
                    return Step::Call(Frame::Instance(InstanceFrame::new(key, Publication::Spec)));
                }
                assert!(self.value.row.is_none(), "BUG: row '{key}' filled twice");
                let (table, instance) = key.table_row();
                let table = table.clone();
                self.state = Some(FillState::AwaitRow(instance));
                Step::Call(Frame::Instance(InstanceFrame::new(
                    table,
                    Publication::Table,
                )))
            }
            ValueKind::List(rows) => {
                let entries = self
                    .value
                    .list
                    .take()
                    .expect("BUG: list value without list arm");
                assert_eq!(
                    entries.len(),
                    rows.len(),
                    "BUG: list arm row count differs from the list value"
                );
                let filled = Vec::with_capacity(entries.len());
                let items: Vec<(ValueKind, RuleResultValue)> = rows
                    .iter()
                    .map(|row| row.value.clone())
                    .zip(entries)
                    .collect();
                self.next_item(items.into_iter(), filled)
            }
            ValueKind::Number(_)
            | ValueKind::Measure(_)
            | ValueKind::UnscaledMeasure { .. }
            | ValueKind::Ratio(_)
            | ValueKind::Text(_)
            | ValueKind::Date(_)
            | ValueKind::Time(_)
            | ValueKind::Boolean(_)
            | ValueKind::Range(_, _) => self.finish(),
            ValueKind::Spec(SpecValue::Fixed(_)) => {
                unreachable!("BUG: evaluation produces only row spec values")
            }
        }
    }

    fn next_item<'a>(
        &mut self,
        mut items: std::vec::IntoIter<(ValueKind, RuleResultValue)>,
        filled: Vec<RuleResultValue>,
    ) -> Step<'a> {
        match items.next() {
            Some((literal, entry)) => {
                self.state = Some(FillState::Items(items, filled));
                Step::Call(Frame::fill(literal, entry))
            }
            None => {
                self.value.list = Some(filled);
                self.finish()
            }
        }
    }

    fn finish<'a>(&mut self) -> Step<'a> {
        Step::Return(Output::Value(Box::new(std::mem::take(&mut self.value))))
    }
}

/// Where a reached instance publishes.
#[derive(Clone, Copy)]
enum Publication {
    /// A reference into the response rows table: rows supplied by evaluation data or
    /// appended by a rule, and anything under them.
    Table,
    /// Inline as the `spec` arm: `uses` imports and rows written in a spec, at any depth.
    Spec,
    /// Only its rules, handed to the walk of the instance that holds it as `uses` slot.
    Rules,
}

/// Members of instance `key`, filled in its own context over its template plan. A
/// table row is filled once and referenced by index everywhere else.
struct InstanceFrame {
    key: InstanceKey,
    publication: Publication,
    index: usize,
}

impl InstanceFrame {
    fn new(key: InstanceKey, publication: Publication) -> Self {
        Self {
            key,
            publication,
            index: 0,
        }
    }

    fn reference(&self, index: usize) -> RowReference {
        RowReference {
            repository: self.key.row.identity.repository.clone(),
            spec: self.key.row.identity.spec.clone(),
            effective: self.key.row.identity.effective.to_string(),
            instance: self.key.path(),
            index,
        }
    }

    fn resume<'a>(&mut self, walk: &mut Walk<'a, '_>, input: Option<Output>) -> Step<'a> {
        match input {
            None => {
                match self.publication {
                    Publication::Table => {
                        if let Some(index) = walk.table.index.get(&self.key).copied() {
                            return Step::Return(Output::Row(self.reference(index)));
                        }
                        let index = walk.table.rows.len();
                        walk.table.index.insert(self.key.clone(), index);
                        walk.table.rows.push(None);
                        self.index = index;
                    }
                    Publication::Spec => walk.ancestors.push(self.key.clone()),
                    Publication::Rules => {}
                }
                let template = walk.root.row_template(&self.key.row.identity);
                let (owner_plan, owner_ctx) = walk.owner_scope(&self.key);
                let ctx = take_row_context(&self.key, owner_plan, template, owner_ctx);
                walk.scopes.push(Scope {
                    plan: template,
                    ctx,
                });
                Step::Call(Frame::Members(Box::new(MembersFrame::new(
                    template,
                    self.key.clone(),
                    matches!(self.publication, Publication::Rules),
                ))))
            }
            Some(output) => {
                let (data, rules) = output.into_members();
                let scope = walk
                    .scopes
                    .pop()
                    .expect("BUG: instance scope ended before it was entered");
                walk.ctx()
                    .caches
                    .row_contexts
                    .borrow_mut()
                    .insert(self.key.clone(), scope.ctx);
                match self.publication {
                    Publication::Table => {
                        walk.table.rows[self.index] = Some(RowResult {
                            repository: self.key.row.identity.repository.clone(),
                            spec: self.key.row.identity.spec.clone(),
                            effective: self.key.row.identity.effective.to_string(),
                            data,
                            rules,
                        });
                        Step::Return(Output::Row(self.reference(self.index)))
                    }
                    Publication::Spec => {
                        let popped = walk.ancestors.pop();
                        assert!(
                            popped.as_ref() == Some(&self.key),
                            "BUG: spec result of '{}' ended on another instance",
                            self.key
                        );
                        let path = self.key.segments().unwrap_or_else(|| {
                            panic!("BUG: planned instance '{}' has no hops", self.key)
                        });
                        Step::Return(Output::Spec(Box::new(SpecResult {
                            repository: self.key.row.identity.repository.clone(),
                            spec: self.key.row.identity.spec.clone(),
                            effective: self.key.row.identity.effective.to_string(),
                            instance: self.key.to_string(),
                            path: path.iter().map(crate::api::PathSegment::from).collect(),
                            data,
                            rules,
                        })))
                    }
                    Publication::Rules => Step::Return(Output::Members((data, rules))),
                }
            }
        }
    }
}

/// An entry whose value is being filled, with the key it is inserted under once filled,
/// or the `uses` slot whose rules are being filled.
enum Awaiting {
    Data(String, SpecRuleResult),
    Rule(String, SpecRuleResult),
    Import(String),
}

/// Data then rules of instance `row` over its template `plan`, then the rules of every
/// `uses` instance under it, keyed by their alias hops (`sub.total`). Missing data is
/// reported under the instance's path.
struct MembersFrame<'a> {
    plan: &'a ExecutionPlan,
    row: InstanceKey,
    data_keys: std::vec::IntoIter<(&'a DataPath, &'a DataDefinition)>,
    rule_paths: std::vec::IntoIter<&'a RulePath>,
    imports: std::vec::IntoIter<&'a DataPath>,
    data: IndexMap<String, SpecRuleResult>,
    rules: IndexMap<String, SpecRuleResult>,
    awaiting: Option<Awaiting>,
}

impl<'a> MembersFrame<'a> {
    fn new(plan: &'a ExecutionPlan, row: InstanceKey, rules_only: bool) -> Self {
        let data_keys: Vec<(&'a DataPath, &'a DataDefinition)> = if rules_only {
            Vec::new()
        } else {
            plan.input_slots().collect()
        };
        let imports: Vec<&'a DataPath> = plan
            .data
            .iter()
            .filter(|(_, definition)| matches!(definition, DataDefinition::Import { .. }))
            .map(|(path, _)| path)
            .collect();
        Self {
            plan,
            row,
            data_keys: data_keys.into_iter(),
            rule_paths: plan.rules.keys().collect::<Vec<_>>().into_iter(),
            imports: imports.into_iter(),
            data: IndexMap::new(),
            rules: IndexMap::new(),
            awaiting: None,
        }
    }

    fn resume(&mut self, walk: &mut Walk<'a, '_>, input: Option<Output>) -> Step<'a> {
        match (self.awaiting.take(), input) {
            (None, None) => {}
            (Some(Awaiting::Data(key, mut entry)), Some(output)) => {
                entry.result = Some(output.into_value());
                self.data.insert(key, entry);
            }
            (Some(Awaiting::Rule(key, mut entry)), Some(output)) => {
                entry.result = Some(output.into_value());
                self.rules.insert(key, entry);
            }
            (Some(Awaiting::Import(alias)), Some(output)) => {
                let (data, rules) = output.into_members();
                assert!(
                    data.is_empty(),
                    "BUG: rules of `uses {alias}` were filled with data"
                );
                for (key, entry) in rules {
                    self.rules.insert(format!("{alias}.{key}"), entry);
                }
            }
            _ => panic!("BUG: members frame resumed with an unexpected child output"),
        }
        while let Some((path, definition)) = self.data_keys.next() {
            if let Some(step) = self.data_item(walk, path, definition) {
                return step;
            }
        }
        while let Some(path) = self.rule_paths.next() {
            if let Some(step) = self.rule_item(walk, path) {
                return step;
            }
        }
        if let Some(slot) = self.imports.next() {
            let nested = InstanceKey::new(
                Arc::clone(self.plan.import_row(Some(&self.row.row), slot, None)),
                Some(self.row.clone()),
            );
            self.awaiting = Some(Awaiting::Import(slot.input_key()));
            return Step::Call(Frame::Instance(InstanceFrame::new(
                nested,
                Publication::Rules,
            )));
        }
        Step::Return(Output::Members((
            std::mem::take(&mut self.data),
            std::mem::take(&mut self.rules),
        )))
    }

    fn data_item(
        &mut self,
        walk: &mut Walk<'a, '_>,
        path: &'a DataPath,
        definition: &'a DataDefinition,
    ) -> Option<Step<'a>> {
        let plan = self.plan;
        definition.schema_type()?;
        let key = path.input_key();
        let slot = match walk.ctx().data_slot(plan, path).cloned() {
            Some(slot) => slot,
            None => match plan.reference_ends.get(path) {
                Some(ReferenceEnd::Rule(rule)) => {
                    ensure_rule_values(plan.rule_index(rule), plan, walk.ctx());
                    walk.ctx().rule_value(plan, rule).clone()
                }
                Some(ReferenceEnd::Promptable(_)) | None => return None,
            },
        };
        if matches!(slot, OperationResult::Veto(VetoType::MissingData { .. })) {
            return None;
        }
        let display_type =
            crate::evaluation::EvaluationContext::data_display_type(plan, path, &slot);
        let mut entry = spec_entry(
            &slot,
            key.clone(),
            RulePath {
                segments: path.segments.clone(),
                rule: path.data.clone(),
            },
            definition.source().clone(),
            display_type.as_ref(),
            &plan.family_units,
            Vec::new(),
        );
        let OperationResult::Value(bound) = slot else {
            self.data.insert(key, entry);
            return None;
        };
        let Some(value) = entry.result.take() else {
            self.data.insert(key, entry);
            return None;
        };
        self.awaiting = Some(Awaiting::Data(key, entry));
        Some(Step::Call(Frame::fill(bound.value, value)))
    }

    fn rule_item(&mut self, walk: &mut Walk<'a, '_>, path: &'a RulePath) -> Option<Step<'a>> {
        let plan = self.plan;
        let exec = plan
            .rules
            .get(path)
            .expect("BUG: rule path missing from plan");
        ensure_rule_values(plan.rule_index(path), plan, walk.ctx());
        let local = walk.ctx().rule_value(plan, path).clone();
        let missing_data = if local.is_missing_data() {
            walk.ctx()
                .missing_data_for_rule(plan, exec.normal_form, Some(&self.row))
        } else {
            Vec::new()
        };
        let operation = row_result(local, &self.row, plan);
        let key = path.input_key();
        let mut entry = spec_entry(
            &operation,
            key.clone(),
            path.clone(),
            exec.source.clone(),
            exec.rule_type.as_ref(),
            &plan.family_units,
            missing_data,
        );
        match operation {
            OperationResult::Value(bound) => match entry.result.take() {
                Some(value) => {
                    self.awaiting = Some(Awaiting::Rule(key, entry));
                    Some(Step::Call(Frame::fill(bound.value, value)))
                }
                None => {
                    self.rules.insert(key, entry);
                    None
                }
            },
            OperationResult::Veto(_) => {
                self.rules.insert(key, entry);
                None
            }
        }
    }
}

type FilledMembers = (
    IndexMap<String, SpecRuleResult>,
    IndexMap<String, SpecRuleResult>,
);

fn spec_entry(
    operation: &OperationResult,
    name: String,
    path: RulePath,
    source: Source,
    display_type: &LemmaType,
    family_units: &FamilyUnitCatalog,
    missing_data: Vec<String>,
) -> SpecRuleResult {
    let built = RuleResult::from_operation_result(
        EvaluatedRule {
            name,
            path,
            source_location: source,
            rule_type: Arc::new(display_type.clone()),
        },
        operation,
        display_type,
        family_units,
        None,
        missing_data,
    );
    SpecRuleResult {
        vetoed: built.vetoed,
        veto_reason: built.veto_reason.clone(),
        rule_type: built.rule_type.clone(),
        result: built.result.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::literals::DateGranularity;
    use crate::parsing::ast::Span;
    use crate::planning::semantics::{
        primitive_number_arc, BaseMeasureVector, DataPath, LemmaType, LiteralValue, MeasureUnit,
        MeasureUnits, RatioUnit, RatioUnits, RulePath, TypeExtends, TypeSpecification, ValueKind,
    };
    use crate::planning::unit_family::FamilyUnitCatalog;
    use crate::planning::unit_index::{UnitIndex, UnitOwner};
    use rust_decimal::Decimal;
    use std::str::FromStr;
    use std::sync::Arc;

    fn dec(s: &str) -> Decimal {
        Decimal::from_str(s).expect("BUG: test decimal literal")
    }

    fn empty_family_catalog() -> FamilyUnitCatalog {
        FamilyUnitCatalog::default()
    }

    fn family_catalog_for_index(index: &UnitIndex) -> FamilyUnitCatalog {
        FamilyUnitCatalog::build(index)
    }

    fn family_catalog_for_money(money: &LemmaType) -> FamilyUnitCatalog {
        family_catalog_for_index(&unit_index_for_money(money))
    }

    fn family_catalog_for_ratio(ratio_type: &LemmaType) -> FamilyUnitCatalog {
        family_catalog_for_index(&unit_index_for_ratio(ratio_type))
    }

    fn unit_index_for_money(money: &LemmaType) -> UnitIndex {
        let mut index = UnitIndex::new();
        let arc = Arc::new(money.clone());
        for unit_name in money.measure_unit_names().into_iter().flatten() {
            index.insert_owner(
                unit_name.to_string(),
                UnitOwner::Named {
                    import_path: Vec::new(),
                    type_name: "money".into(),
                    lemma_type: Arc::clone(&arc),
                },
            );
        }
        index
    }

    fn unit_index_for_ratio(ratio_type: &LemmaType) -> UnitIndex {
        let mut index = UnitIndex::new();
        let arc = Arc::new(ratio_type.clone());
        let type_name = ratio_type.name();
        for unit_name in ratio_type.ratio_unit_names().into_iter().flatten() {
            index.insert_owner(
                unit_name.to_string(),
                UnitOwner::Named {
                    import_path: Vec::new(),
                    type_name: type_name.clone(),
                    lemma_type: Arc::clone(&arc),
                },
            );
        }
        index
    }

    fn dummy_source() -> Source {
        Source::new(
            crate::parsing::source::SourceType::Volatile,
            Span {
                start: 0,
                end: 0,
                line: 1,
                col: 1,
            },
        )
    }

    fn dummy_evaluated_rule(name: &str, rule_type: &LemmaType) -> EvaluatedRule {
        EvaluatedRule {
            name: name.to_string(),
            path: RulePath::new(vec![], name.to_string()),
            source_location: dummy_source(),
            rule_type: Arc::new(rule_type.clone()),
        }
    }

    #[test]
    fn test_response_serialization() {
        let mut results = IndexMap::new();
        results.insert(
            "test_rule".to_string(),
            RuleResult::from_operation_result(
                dummy_evaluated_rule("test_rule", primitive_number_arc().as_ref()),
                &OperationResult::from_literal(LiteralValue::number_from_decimal(Decimal::from(
                    42,
                ))),
                primitive_number_arc().as_ref(),
                &empty_family_catalog(),
                None,
                Vec::new(),
            ),
        );
        let response = Response {
            spec_name: "test_spec".to_string(),
            effective: "2026-01-01".to_string(),
            spec_effective_from: None,
            spec_effective_to: None,
            results,
            rows: Vec::new(),
        };

        let json = serde_json::to_string(&crate::api::Response::from(&response)).unwrap();
        assert!(json.contains("test_spec"));
        assert!(json.contains("test_rule"));
        assert!(json.contains("\"number\":\"42\""));
        assert!(!json.contains("lemma_type"));
    }

    #[test]
    fn response_number_json_never_uses_fraction_notation() {
        use crate::computation::rational::decimal_to_rational;

        let rational = decimal_to_rational(Decimal::new(1, 1) / Decimal::new(3, 1)).unwrap();
        let decimal_string = rational.try_to_decimal().unwrap().to_string();
        let mut results = IndexMap::new();
        results.insert(
            "third".to_string(),
            RuleResult::from_operation_result(
                dummy_evaluated_rule("third", primitive_number_arc().as_ref()),
                &OperationResult::from_literal(LiteralValue::number_from_decimal(
                    rational.try_to_decimal().unwrap(),
                )),
                primitive_number_arc().as_ref(),
                &empty_family_catalog(),
                None,
                Vec::new(),
            ),
        );
        // Override committed decimal number field to match serialization path under test
        if let Some(rule) = results.get_mut("third") {
            let decimal = rational.try_to_decimal().unwrap();
            rule.result = Some(crate::result_value::RuleResultValue {
                result: Some(decimal_string.clone()),
                number: Some(decimal),
                ..Default::default()
            });
        }

        let response = Response {
            spec_name: "test".to_string(),
            effective: "test".to_string(),
            spec_effective_from: None,
            spec_effective_to: None,
            results,
            rows: Vec::new(),
        };

        let json: serde_json::Value = serde_json::from_str(
            &serde_json::to_string(&crate::api::Response::from(&response)).unwrap(),
        )
        .unwrap();
        let number = json["results"]["third"]["number"]
            .as_str()
            .expect("number must be a JSON string");
        assert!(
            !number.contains('/'),
            "API decimal string must not use fraction notation, got {number}"
        );
    }

    #[test]
    fn test_rule_result_veto() {
        let missing = RuleResult::from_operation_result(
            dummy_evaluated_rule("rule3", &LemmaType::veto_type()),
            &OperationResult::Veto(VetoType::missing_data(
                DataPath::new(vec![], "data1".to_string()),
                None,
            )),
            &LemmaType::veto_type(),
            &empty_family_catalog(),
            None,
            vec!["data1".to_string()],
        );
        assert!(missing.vetoed);
        assert!(missing.veto_reason.as_ref().unwrap().contains("data1"));

        let veto = RuleResult::from_operation_result(
            dummy_evaluated_rule("rule4", &LemmaType::veto_type()),
            &OperationResult::Veto(VetoType::UserDefined {
                message: Some("Vetoed".to_string()),
            }),
            &LemmaType::veto_type(),
            &empty_family_catalog(),
            None,
            Vec::new(),
        );
        assert_eq!(veto.veto_reason.as_deref(), Some("Vetoed"));
    }

    /// Attach hole: MissingData + empty missing_data is a BUG, not a silent stall.
    #[test]
    #[should_panic(expected = "BUG: MissingData")]
    fn missing_data_veto_must_not_attach_empty_missing_data_list() {
        RuleResult::from_operation_result(
            dummy_evaluated_rule("rule3", &LemmaType::veto_type()),
            &OperationResult::Veto(VetoType::missing_data(
                DataPath::new(vec![], "data1".to_string()),
                None,
            )),
            &LemmaType::veto_type(),
            &empty_family_catalog(),
            None,
            Vec::new(),
        );
    }

    #[test]
    #[should_panic(expected = "BUG: MissingData")]
    fn missing_data_veto_must_include_veto_path_in_list() {
        RuleResult::from_operation_result(
            dummy_evaluated_rule("rule3", &LemmaType::veto_type()),
            &OperationResult::Veto(VetoType::missing_data(
                DataPath::new(vec![], "data1".to_string()),
                None,
            )),
            &LemmaType::veto_type(),
            &empty_family_catalog(),
            None,
            vec!["other".to_string()],
        );
    }

    #[test]
    #[should_panic(expected = "BUG: missing_data must be empty")]
    fn non_missing_data_result_must_not_attach_leftover_missing_data() {
        RuleResult::from_operation_result(
            dummy_evaluated_rule("rule4", &LemmaType::veto_type()),
            &OperationResult::Veto(VetoType::UserDefined {
                message: Some("Vetoed".to_string()),
            }),
            &LemmaType::veto_type(),
            &empty_family_catalog(),
            None,
            vec!["leftover".to_string()],
        );
    }

    #[test]
    fn rule_result_value_out_of_memory_is_not_decimal_limit_veto() {
        let result = vetoed_rule_result_for_rule_result_value_failure(
            dummy_evaluated_rule("rule", primitive_number_arc().as_ref()),
            primitive_number_arc().as_ref(),
            None,
            RuleResultValueFailure::OutOfMemory,
            Vec::new(),
        );
        assert_eq!(
            result.veto_reason.as_deref(),
            Some("The calculation ran out of memory.")
        );
        assert_ne!(
            result.veto_reason.as_deref(),
            Some("The calculated result exceeds the decimal value limit.")
        );
    }

    #[test]
    fn rule_result_value_decimal_limit_uses_commit_message() {
        let result = vetoed_rule_result_for_rule_result_value_failure(
            dummy_evaluated_rule("rule", primitive_number_arc().as_ref()),
            primitive_number_arc().as_ref(),
            None,
            RuleResultValueFailure::DecimalLimit,
            Vec::new(),
        );
        assert_eq!(
            result.veto_reason.as_deref(),
            Some("The calculated result exceeds the decimal value limit.")
        );
    }

    fn test_money_type() -> LemmaType {
        LemmaType::new(
            "money".to_string(),
            TypeSpecification::Measure {
                minimum: None,
                maximum: None,
                units: MeasureUnits::from(vec![
                    MeasureUnit {
                        name: "eur".to_string(),
                        scale: crate::literals::UnitScale::Literal(
                            crate::computation::rational::rational_one(),
                        ),
                        derived_measure_factors: Vec::new(),
                        decomposition: BaseMeasureVector::new(),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    },
                    MeasureUnit {
                        name: "usd".to_string(),
                        scale: crate::literals::UnitScale::Literal(
                            crate::computation::rational::decimal_to_rational(Decimal::new(91, 2))
                                .expect("factor"),
                        ),
                        derived_measure_factors: Vec::new(),
                        decomposition: BaseMeasureVector::new(),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    },
                ]),
                traits: Vec::new(),
                decomposition: Some(BaseMeasureVector::new()),
                unit: None,
                help: String::new(),
            },
            TypeExtends::Primitive,
        )
    }

    #[test]
    fn measure_rule_result_value_uses_rule_type_when_expression_index_empty() {
        let money = test_money_type();
        let family_units = family_catalog_for_money(&money);
        let ten_usd = LiteralValue {
            value: ValueKind::Measure(
                crate::computation::rational::checked_mul(
                    &crate::computation::rational::decimal_to_rational(Decimal::from(10))
                        .expect("ten"),
                    &crate::computation::rational::decimal_to_rational(Decimal::new(91, 2))
                        .expect("usd factor"),
                )
                .expect("canonical usd"),
            ),
        };
        let result = RuleResult::from_operation_result(
            dummy_evaluated_rule("total", &money),
            &OperationResult::from_literal(ten_usd),
            &money,
            &family_units,
            None,
            Vec::new(),
        );
        let measure = result
            .result
            .as_ref()
            .expect("value")
            .measure
            .clone()
            .expect("measure map");
        assert_eq!(measure.get("usd"), Some(&dec("10.00")));
        assert!(measure.contains_key("eur"));
    }

    #[test]
    fn test_measure_rule_result_value_multi_unit() {
        let money = test_money_type();
        let family_units = family_catalog_for_money(&money);
        let ten_eur = LiteralValue {
            value: ValueKind::Measure(
                crate::computation::rational::decimal_to_rational(Decimal::from(10)).expect("ten"),
            ),
        };
        let result = RuleResult::from_operation_result(
            dummy_evaluated_rule("total", &money),
            &OperationResult::from_literal(ten_eur),
            &money,
            &family_units,
            None,
            Vec::new(),
        );
        let measure = result
            .result
            .as_ref()
            .expect("value")
            .measure
            .clone()
            .expect("measure map");
        assert_eq!(measure.get("eur"), Some(&dec("10.00")));
        assert_eq!(
            measure.get("usd"),
            Some(&dec("10.98901098901098901098901098"))
        );
    }

    #[test]
    fn measure_rule_result_value_keeps_unit_conversion_unrounded() {
        let money = LemmaType::new(
            "money".to_string(),
            TypeSpecification::Measure {
                minimum: None,
                maximum: None,
                units: MeasureUnits::from(vec![
                    MeasureUnit {
                        name: "eur".to_string(),
                        scale: crate::literals::UnitScale::Literal(
                            crate::computation::rational::rational_one(),
                        ),
                        derived_measure_factors: Vec::new(),
                        decomposition: BaseMeasureVector::new(),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    },
                    MeasureUnit {
                        name: "usd".to_string(),
                        scale: crate::literals::UnitScale::Literal(
                            crate::computation::rational::decimal_to_rational(Decimal::new(84, 2))
                                .expect("usd factor"),
                        ),
                        derived_measure_factors: Vec::new(),
                        decomposition: BaseMeasureVector::new(),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    },
                ]),
                traits: Vec::new(),
                decomposition: Some(BaseMeasureVector::new()),
                unit: None,
                help: String::new(),
            },
            TypeExtends::Primitive,
        );
        let family_units = family_catalog_for_money(&money);
        let three_twelve_eur = LiteralValue {
            value: ValueKind::Measure(
                crate::computation::rational::decimal_to_rational(Decimal::new(312, 2))
                    .expect("3.12 eur canonical"),
            ),
        };
        let result = RuleResult::from_operation_result(
            dummy_evaluated_rule("delivery_cost", &money),
            &OperationResult::from_literal(three_twelve_eur),
            &money,
            &family_units,
            None,
            Vec::new(),
        );
        let measure = result
            .result
            .as_ref()
            .expect("value")
            .measure
            .clone()
            .expect("measure map");
        assert_eq!(measure.get("eur"), Some(&dec("3.12")));
        assert_eq!(
            measure.get("usd"),
            Some(&dec("3.714285714285714285714285714"))
        );
    }

    #[test]
    fn test_ratio_rule_result_value_multi_unit() {
        let ratio_type = LemmaType::new(
            "rate".to_string(),
            TypeSpecification::Ratio {
                minimum: None,
                maximum: None,
                units: RatioUnits::from(vec![
                    RatioUnit {
                        name: "percent".to_string(),
                        value: crate::computation::rational::decimal_to_rational(Decimal::from(
                            100,
                        ))
                        .expect("percent"),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    },
                    RatioUnit {
                        name: "basis_points".to_string(),
                        value: crate::computation::rational::decimal_to_rational(Decimal::from(
                            10_000,
                        ))
                        .expect("bp"),
                        minimum: None,
                        maximum: None,
                        suggestion_magnitude: None,
                    },
                ]),
                unit: None,
                help: String::new(),
            },
            TypeExtends::Primitive,
        );
        let family_units = family_catalog_for_ratio(&ratio_type);
        let half = crate::computation::rational::rational_new(1, 2);
        let lit = LiteralValue {
            value: ValueKind::Ratio(half),
        };
        let result = RuleResult::from_operation_result(
            dummy_evaluated_rule("rate_out", &ratio_type),
            &OperationResult::from_literal(lit),
            &ratio_type,
            &family_units,
            None,
            Vec::new(),
        );
        let ratio = result
            .result
            .as_ref()
            .expect("value")
            .ratio
            .clone()
            .expect("ratio map");
        assert_eq!(ratio.get("percent"), Some(&dec("50")));
        assert_eq!(ratio.get("basis_points"), Some(&dec("5000")));
    }

    #[test]
    fn test_measure_rule_result_value_cross_spec_import() {
        use crate::parsing::source::SourceType;
        use crate::Engine;

        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(std::sync::Arc::new(std::path::PathBuf::from("t.lemma"))),
                r#"
spec consumer 2025-01-01
uses d: dep 2025-10-01
rule out: d.doubled

spec dep 2025-01-01
uses c: child 2025-06-01
data money: c.money
data p: 5 usd
rule doubled: p * 2

spec child 2025-01-01
data money: measure
 -> unit eur: 1.00

spec child 2025-06-01
data money: measure
 -> unit eur: 1.00
 -> unit usd: 0.91
"#
                .to_string(),
            )])
            .expect("load");
        let effective = crate::literals::DateTimeValue {
            year: 2025,
            month: 3,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            microsecond: 0,
            timezone: None,

            granularity: DateGranularity::Full,
        };
        let response = engine
            .evaluate(
                None,
                "consumer",
                Some(&effective),
                IndexMap::new(),
                None,
                false,
            )
            .expect("evaluate");
        let out = response.results.get("out").expect("out rule");
        assert!(!out.vetoed);
        let measure = out
            .result
            .as_ref()
            .expect("value")
            .measure
            .as_ref()
            .expect("measure map");
        assert!(measure.contains_key("usd"));
        assert!(measure.contains_key("eur"));
    }

    #[test]
    fn to_literal_roundtrips_number() {
        let literal = LiteralValue::number_from_decimal(Decimal::from(42));
        let rule_result = RuleResult::from_operation_result(
            dummy_evaluated_rule("answer", primitive_number_arc().as_ref()),
            &OperationResult::from_literal(literal.clone()),
            primitive_number_arc().as_ref(),
            &empty_family_catalog(),
            None,
            Vec::new(),
        );
        assert_eq!(rule_result.to_literal(), literal);
    }

    #[test]
    fn to_literal_roundtrips_measure() {
        let money = test_money_type();
        let family_units = family_catalog_for_money(&money);
        let literal = LiteralValue::measure(crate::computation::rational::rational_new(60, 1));
        let rule_result = RuleResult::from_operation_result(
            dummy_evaluated_rule("pay", &money),
            &OperationResult::from_literal(literal.clone()),
            &money,
            &family_units,
            None,
            Vec::new(),
        );
        assert_eq!(rule_result.to_literal(), literal);
    }
}
