use anyhow::{Context, Result};
use chrono::NaiveDate;
use indexmap::IndexMap;
use inquire::validator::Validation;
use inquire::{Confirm, DateSelect, MultiSelect, Select, Text};
use lemma::{
    numbered_row_origin, row_field_input_key, DateTimeValue, Engine, EvaluateDataValue, LemmaType,
    LiteralValue, Response, Show, ShowData, SpecIdentity, SpecVersion, TypeSpecification,
    ValueKind, VetoType,
};
use rust_decimal::Decimal;
use std::collections::{HashSet, VecDeque};

pub(crate) fn repository_loaded(engine: &Engine, name: &str) -> bool {
    engine
        .specs()
        .iter()
        .any(|r| r.repository.as_deref() == Some(name))
}

/// Repository qualifier, spec name, selected rules, prompted data (stdin/`name=value` already bound).
pub type InteractiveResult = (
    Option<String>,
    String,
    Option<Vec<String>>,
    IndexMap<String, EvaluateDataValue>,
);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Step {
    Field(String),
    Row(usize),
}

#[derive(Clone, Debug)]
enum Slot {
    Prompt {
        lemma_type: Box<LemmaType>,
        suggestion: Option<LiteralValue>,
        location: Vec<Step>,
    },
    DeclaredRow {
        name: String,
        location: Vec<Step>,
        identity: SpecIdentity,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListChoice {
    Keep,
    Clear,
    Enter,
}

fn step_label(location: &[Step]) -> String {
    location
        .iter()
        .map(|step| match step {
            Step::Field(name) => name.clone(),
            Step::Row(index) => format!("item {}", index + 1),
        })
        .collect::<Vec<_>>()
        .join(" › ")
}

fn keep_default_label(count: usize) -> String {
    if count == 1 {
        "Keep the default item".to_string()
    } else {
        format!("Keep the {count} default items")
    }
}

fn list_choice_labels(count: usize) -> [String; 3] {
    [
        keep_default_label(count),
        "No items".to_string(),
        "Enter my own items".to_string(),
    ]
}

fn list_choice_from_index(index: usize) -> ListChoice {
    match index {
        0 => ListChoice::Keep,
        1 => ListChoice::Clear,
        2 => ListChoice::Enter,
        _ => panic!("BUG: list choice index {index} out of range"),
    }
}

fn declared_row_count(declared: &Option<lemma::RuleResultValue>) -> Option<usize> {
    let rows = declared.as_ref()?.list.as_ref()?;
    if rows.is_empty() {
        panic!("BUG: Show declared list value with empty list");
    }
    Some(rows.len())
}

struct PromptSession<'a> {
    engine: &'a Engine,
    repo: Option<&'a str>,
    spec_name: &'a str,
    rules_for_request: Option<&'a [String]>,
    provided_data: &'a IndexMap<String, EvaluateDataValue>,
    now: &'a DateTimeValue,
    collected: &'a mut IndexMap<String, EvaluateDataValue>,
    slots: &'a mut IndexMap<String, Slot>,
    header_printed: &'a mut bool,
    decided: &'a mut HashSet<String>,
}

#[derive(Clone, Debug)]
struct ListPrompt {
    name: String,
    location: Vec<Step>,
    identity: SpecIdentity,
    declared: Option<lemma::RuleResultValue>,
}

fn instance_list_identity(lemma_type: &LemmaType) -> Option<&SpecIdentity> {
    match &lemma_type.specifications {
        TypeSpecification::List { element, .. } => match &element.specifications {
            TypeSpecification::Spec { spec } => Some(spec),
            _ => None,
        },
        _ => None,
    }
}

fn needed_by_selected(needed: &[String], selected: &Option<Vec<String>>) -> bool {
    if needed.is_empty() {
        return false;
    }
    match selected {
        None => true,
        Some(rules) if rules.is_empty() => true,
        Some(rules) => needed.iter().any(|name| rules.iter().any(|r| r == name)),
    }
}

fn list_prompts_from_show(
    show: &Show,
    selected_rules: &Option<Vec<String>>,
    provided_data: &IndexMap<String, EvaluateDataValue>,
    location_prefix: Vec<Step>,
    name_prefix: Option<&str>,
) -> Vec<ListPrompt> {
    let mut lists = Vec::new();
    for (name, entry) in &show.data {
        let Some(identity) = instance_list_identity(&entry.lemma_type) else {
            continue;
        };
        if provided_data.contains_key(name) && name_prefix.is_none() {
            continue;
        }
        if !needed_by_selected(&entry.needed_by_rules, selected_rules) {
            continue;
        }
        let list_name = match name_prefix {
            None => name.clone(),
            Some(prefix) => format!("{prefix}.{name}"),
        };
        let mut location = location_prefix.clone();
        location.push(Step::Field(name.clone()));
        lists.push(ListPrompt {
            name: list_name,
            location,
            identity: identity.clone(),
            declared: entry.value.clone(),
        });
    }
    lists
}

fn slot_from_show_data(entry: &ShowData, location: Vec<Step>) -> Slot {
    let suggestion = entry
        .suggestion
        .as_ref()
        .map(|v| v.to_literal(&entry.lemma_type));
    Slot::Prompt {
        lemma_type: Box::new(entry.lemma_type.clone()),
        suggestion,
        location,
    }
}

fn register_show_slots(
    slots: &mut IndexMap<String, Slot>,
    show: &Show,
    location_prefix: Vec<Step>,
    key_prefix: Option<&str>,
) {
    // Row template specs often have no rules: fields live in Types, not Data.
    for (name, lemma_type) in &show.types {
        if instance_list_identity(lemma_type).is_some() {
            continue;
        }
        let key = match key_prefix {
            None => name.clone(),
            Some(prefix) => format!("{prefix}.{name}"),
        };
        let mut location = location_prefix.clone();
        location.push(Step::Field(name.clone()));
        slots.insert(
            key,
            Slot::Prompt {
                lemma_type: Box::new(lemma_type.clone()),
                suggestion: None,
                location,
            },
        );
    }
    for (name, entry) in &show.data {
        if instance_list_identity(&entry.lemma_type).is_some() {
            continue;
        }
        let key = match key_prefix {
            None => name.clone(),
            Some(prefix) => format!("{prefix}.{name}"),
        };
        let mut location = location_prefix.clone();
        location.push(Step::Field(name.clone()));
        slots.insert(key, slot_from_show_data(entry, location));
    }
}

enum DeclaredRowWork<'a> {
    Slot {
        key: String,
        slot: Slot,
    },
    Nested {
        list_key: String,
        list_location: Vec<Step>,
        identity: SpecIdentity,
        rows: &'a [lemma::RuleResultValue],
    },
}

fn declared_row_work<'a>(
    engine: &Engine,
    list_key: &str,
    list_location: &[Step],
    identity: &SpecIdentity,
    rows: &'a [lemma::RuleResultValue],
) -> Vec<DeclaredRowWork<'a>> {
    let row_show = engine.show_spec(identity);
    let mut work = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let number = index + 1;
        // Row template specs often have no rules: fields live in Types, not Data.
        for (field, lemma_type) in &row_show.types {
            let field_key = row_field_input_key(list_key, number, field);
            if let Some(nested_id) = instance_list_identity(lemma_type) {
                if let Some(nested_rows) = row
                    .fields
                    .as_ref()
                    .and_then(|fields| fields.get(field))
                    .and_then(|value| value.list.as_deref())
                {
                    let mut nested_location = list_location.to_vec();
                    nested_location.push(Step::Row(index));
                    nested_location.push(Step::Field(field.clone()));
                    work.push(DeclaredRowWork::Nested {
                        list_key: field_key,
                        list_location: nested_location,
                        identity: nested_id.clone(),
                        rows: nested_rows,
                    });
                }
                continue;
            }
            work.push(DeclaredRowWork::Slot {
                key: field_key,
                slot: Slot::DeclaredRow {
                    name: list_key.to_string(),
                    location: list_location.to_vec(),
                    identity: identity.clone(),
                },
            });
        }
    }
    work
}

fn register_declared_row_slots(
    slots: &mut IndexMap<String, Slot>,
    engine: &Engine,
    list_key: &str,
    list_location: &[Step],
    identity: &SpecIdentity,
    rows: &[lemma::RuleResultValue],
) {
    let mut pending = vec![DeclaredRowWork::Nested {
        list_key: list_key.to_string(),
        list_location: list_location.to_vec(),
        identity: identity.clone(),
        rows,
    }];
    while let Some(work) = pending.pop() {
        match work {
            DeclaredRowWork::Slot { key, slot } => {
                slots.insert(key, slot);
            }
            DeclaredRowWork::Nested {
                list_key,
                list_location,
                identity,
                rows,
            } => {
                let mut items =
                    declared_row_work(engine, &list_key, &list_location, &identity, rows);
                items.reverse();
                pending.extend(items);
            }
        }
    }
}

fn write_into(node: &mut EvaluateDataValue, location: &[Step], value: EvaluateDataValue) {
    let mut current = node;
    let mut rest = location;
    loop {
        match rest {
            [] => panic!("BUG: write_into with empty location"),
            [Step::Field(name)] => {
                let EvaluateDataValue::Map(map) = current else {
                    panic!("BUG: expected object for field '{name}'");
                };
                map.insert(name.clone(), value);
                return;
            }
            [Step::Row(index)] => {
                let EvaluateDataValue::List(rows) = current else {
                    panic!("BUG: expected list for row {index}");
                };
                if *index >= rows.len() {
                    panic!("BUG: row {index} missing from list of {}", rows.len());
                }
                rows[*index] = value;
                return;
            }
            [Step::Field(name), tail @ ..] => {
                let EvaluateDataValue::Map(map) = current else {
                    panic!("BUG: expected object for field '{name}'");
                };
                current = map
                    .entry(name.clone())
                    .or_insert_with(|| match tail.first() {
                        Some(Step::Row(_)) => EvaluateDataValue::List(Vec::new()),
                        Some(Step::Field(_)) => EvaluateDataValue::Map(IndexMap::new()),
                        None => unreachable!("BUG: rest after Field is non-empty"),
                    });
                rest = tail;
            }
            [Step::Row(index), tail @ ..] => {
                let EvaluateDataValue::List(rows) = current else {
                    panic!("BUG: expected list for row {index}");
                };
                while rows.len() <= *index {
                    rows.push(EvaluateDataValue::Map(IndexMap::new()));
                }
                current = &mut rows[*index];
                rest = tail;
            }
        }
    }
}

fn write_at(
    root: &mut IndexMap<String, EvaluateDataValue>,
    location: &[Step],
    value: EvaluateDataValue,
) {
    match location {
        [Step::Field(name)] => {
            root.insert(name.clone(), value);
        }
        [Step::Field(name), rest @ ..] => {
            let entry = root
                .entry(name.clone())
                .or_insert_with(|| match rest.first() {
                    Some(Step::Row(_)) => EvaluateDataValue::List(Vec::new()),
                    Some(Step::Field(_)) => EvaluateDataValue::Map(IndexMap::new()),
                    None => unreachable!("BUG: rest after Field is non-empty"),
                });
            write_into(entry, rest, value);
        }
        _ => panic!("BUG: run-data tree root must start at a field"),
    }
}

fn list_at<'a>(
    root: &'a mut IndexMap<String, EvaluateDataValue>,
    location: &[Step],
) -> &'a mut Vec<EvaluateDataValue> {
    fn walk<'a>(
        node: &'a mut EvaluateDataValue,
        location: &[Step],
    ) -> &'a mut Vec<EvaluateDataValue> {
        let mut current = node;
        let mut rest = location;
        loop {
            match rest {
                [] => match current {
                    EvaluateDataValue::List(rows) => return rows,
                    _ => panic!("BUG: expected list at location end"),
                },
                [Step::Field(name), tail @ ..] => {
                    let EvaluateDataValue::Map(map) = current else {
                        panic!("BUG: expected object for field '{name}'");
                    };
                    let entry = map
                        .entry(name.clone())
                        .or_insert_with(|| EvaluateDataValue::Map(IndexMap::new()));
                    if tail.is_empty() && !matches!(entry, EvaluateDataValue::List(_)) {
                        *entry = EvaluateDataValue::List(Vec::new());
                    }
                    current = entry;
                    rest = tail;
                }
                [Step::Row(index), tail @ ..] => {
                    let EvaluateDataValue::List(rows) = current else {
                        panic!("BUG: expected list for row {index}");
                    };
                    while rows.len() <= *index {
                        rows.push(EvaluateDataValue::Map(IndexMap::new()));
                    }
                    current = &mut rows[*index];
                    rest = tail;
                }
            }
        }
    }
    match location {
        [Step::Field(name)] => {
            let entry = root
                .entry(name.clone())
                .or_insert_with(|| EvaluateDataValue::List(Vec::new()));
            match entry {
                EvaluateDataValue::List(rows) => rows,
                _ => panic!("BUG: expected list for '{name}'"),
            }
        }
        [Step::Field(name), rest @ ..] => {
            let entry = root.entry(name.clone()).or_insert_with(|| {
                if matches!(rest.first(), Some(Step::Row(_))) {
                    EvaluateDataValue::List(Vec::new())
                } else {
                    EvaluateDataValue::Map(IndexMap::new())
                }
            });
            walk(entry, rest)
        }
        _ => panic!("BUG: list location must start at a field"),
    }
}

fn append_empty_row(root: &mut IndexMap<String, EvaluateDataValue>, location: &[Step]) -> usize {
    let rows = list_at(root, location);
    rows.push(EvaluateDataValue::Map(IndexMap::new()));
    rows.len()
}

#[derive(Clone, Debug)]
struct TextConstraints {
    length: Option<usize>,
    help: String,
}

#[derive(Clone, Debug)]
struct NumericConstraints {
    minimum: Option<Decimal>,
    maximum: Option<Decimal>,
    decimals: Option<u8>,
    help: String,
}

fn load_static_show(
    engine: &Engine,
    repo: Option<&str>,
    name: &str,
    now: &DateTimeValue,
) -> Result<lemma::Show> {
    engine
        .show(repo, name, Some(now))
        .map_err(|e| anyhow::anyhow!("{}", e))
}

pub fn evaluate_interactive(
    engine: &Engine,
    spec_name: Option<String>,
    rule_names: Option<Vec<String>>,
    provided_data: &IndexMap<String, EvaluateDataValue>,
    now: &DateTimeValue,
    cli_repository_qualifier: Option<&str>,
) -> Result<InteractiveResult> {
    let (repository_qualifier, specification_name) = match spec_name {
        Some(name) => {
            load_static_show(engine, cli_repository_qualifier, &name, now)?;
            (cli_repository_qualifier.map(String::from), name)
        }
        None => select_spec(engine, now, cli_repository_qualifier)?,
    };

    let rules = match rule_names {
        Some(names) => Some(names),
        None => select_rules(
            engine,
            repository_qualifier.as_deref(),
            &specification_name,
            now,
        )?,
    };

    let data = prompt_data(
        engine,
        repository_qualifier.as_deref(),
        &specification_name,
        &rules,
        provided_data,
        now,
    )?;

    Ok((repository_qualifier, specification_name, rules, data))
}

fn is_active_at(version: &SpecVersion, now: &DateTimeValue) -> bool {
    let after_start = match &version.effective_from {
        None => true,
        Some(from) => now >= from,
    };
    let before_end = match &version.effective_to {
        None => true,
        Some(to) => now < to,
    };
    after_start && before_end
}

fn select_spec(
    engine: &Engine,
    now: &DateTimeValue,
    cli_repository_qualifier: Option<&str>,
) -> Result<(Option<String>, String)> {
    let mut items: Vec<(Option<String>, SpecVersion)> = engine
        .specs()
        .into_iter()
        .flat_map(|repo| {
            let repo_name = repo.repository.clone();
            repo.specs
                .into_iter()
                .filter(|version| is_active_at(version, now))
                .map(move |version| (repo_name.clone(), version))
        })
        .collect();

    if let Some(q) = cli_repository_qualifier {
        if !repository_loaded(engine, q) {
            anyhow::bail!("Repository '{q}' not loaded");
        }
        items.retain(|(repo_name, _)| repo_name.as_deref() == Some(q));
    }

    if items.is_empty() {
        anyhow::bail!("No specs found. Add .lemma files to get started.");
    }

    let display_options: Vec<String> = items
        .iter()
        .map(|(repo_name, version)| {
            spec_picker_label(engine, repo_name.as_deref(), &version.name, now)
        })
        .collect::<Result<Vec<_>>>()?;

    let selected = Select::new("Select a spec:", display_options.clone())
        .with_help_message("Use arrow keys to navigate, Enter to select")
        .prompt()
        .context("Failed to get spec selection")?;

    let spec_index = display_options
        .iter()
        .position(|d| d == &selected)
        .context("Failed to find selected spec index")?;

    let (repo_name, version) = items
        .into_iter()
        .nth(spec_index)
        .context("Failed to match selected spec")?;

    Ok((repo_name, version.name))
}

fn spec_picker_label(
    engine: &Engine,
    repository: Option<&str>,
    spec_name: &str,
    now: &DateTimeValue,
) -> Result<String> {
    let show = load_static_show(engine, repository, spec_name, now)?;
    let target = match repository {
        None => spec_name.to_string(),
        Some(repo) => format!("{repo} {spec_name}"),
    };
    Ok(format!(
        "{} — {} data, {} rules",
        target,
        show.types.len(),
        show.rules.len()
    ))
}

fn select_rules(
    engine: &Engine,
    repo: Option<&str>,
    spec_name: &str,
    now: &DateTimeValue,
) -> Result<Option<Vec<String>>> {
    let show = load_static_show(engine, repo, spec_name, now)?;
    let rule_names: Vec<String> = show
        .rules
        .iter()
        .filter(|(_, rule)| rule.path.is_empty())
        .map(|(name, _)| name.clone())
        .collect();

    if rule_names.is_empty() {
        return Ok(None);
    }

    let selected = MultiSelect::new("Select rules to evaluate:", rule_names.clone())
        .with_default(&(0..rule_names.len()).collect::<Vec<_>>())
        .prompt()
        .context("Failed to get rule selection")?;

    if selected.is_empty() || selected.len() == rule_names.len() {
        Ok(None)
    } else {
        Ok(Some(selected))
    }
}

fn trial_evaluate_data(
    provided_data: &IndexMap<String, EvaluateDataValue>,
    collected: &IndexMap<String, EvaluateDataValue>,
) -> IndexMap<String, EvaluateDataValue> {
    let mut trial = provided_data.clone();
    trial.extend(collected.clone());
    trial
}

fn prompt_data(
    engine: &Engine,
    repo: Option<&str>,
    spec_name: &str,
    rule_names: &Option<Vec<String>>,
    provided_data: &IndexMap<String, EvaluateDataValue>,
    now: &DateTimeValue,
) -> Result<IndexMap<String, EvaluateDataValue>> {
    let mut collected: IndexMap<String, EvaluateDataValue> = IndexMap::new();
    let mut slots: IndexMap<String, Slot> = IndexMap::new();
    let mut header_printed = false;
    let rules_for_request = rule_names.as_deref().filter(|rules| !rules.is_empty());
    let show = load_static_show(engine, repo, spec_name, now)?;
    register_show_slots(&mut slots, &show, Vec::new(), None);
    let lists = list_prompts_from_show(&show, rule_names, provided_data, Vec::new(), None);
    let mut decided: HashSet<String> = HashSet::new();
    for list in &lists {
        if let Some(rows) = list
            .declared
            .as_ref()
            .and_then(|value| value.list.as_deref())
        {
            register_declared_row_slots(
                &mut slots,
                engine,
                &list.name,
                &list.location,
                &list.identity,
                rows,
            );
        }
    }

    let mut session = PromptSession {
        engine,
        repo,
        spec_name,
        rules_for_request,
        provided_data,
        now,
        collected: &mut collected,
        slots: &mut slots,
        header_printed: &mut header_printed,
        decided: &mut decided,
    };
    run_prompt_tasks(
        &mut session,
        vec![Task::TopLevelLists(lists.into_iter()), Task::FillMissing],
    )?;

    Ok(collected)
}

enum Task {
    FillMissing,
    PromptList(Box<ListPrompt>),
    TopLevelLists(std::vec::IntoIter<ListPrompt>),
    EnterItems(Box<EnterItems>),
}

enum EnterPhase {
    NewRow,
    NextNested,
    AskAnother,
}

struct EnterItems {
    list: ListPrompt,
    label: String,
    row_show: Show,
    pending_nested: VecDeque<ListPrompt>,
    phase: EnterPhase,
}

fn run_prompt_tasks(session: &mut PromptSession<'_>, mut tasks: Vec<Task>) -> Result<()> {
    while let Some(task) = tasks.pop() {
        match task {
            Task::FillMissing => fill_missing_step(session, &mut tasks)?,
            Task::PromptList(list) => prompt_list_step(session, *list, &mut tasks)?,
            Task::TopLevelLists(lists) => top_level_lists_step(session, lists, &mut tasks),
            Task::EnterItems(state) => enter_items_step(session, *state, &mut tasks)?,
        }
    }
    Ok(())
}

fn fill_missing_step(session: &mut PromptSession<'_>, tasks: &mut Vec<Task>) -> Result<()> {
    let response = session
        .engine
        .evaluate(
            session.repo,
            session.spec_name,
            Some(session.now),
            trial_evaluate_data(session.provided_data, session.collected),
            session.rules_for_request,
            false,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let next_name = next_prompt_name_from_results(
        response.results.values(),
        session.provided_data,
        session.collected,
    );

    let name = match next_name {
        Some(name) => name,
        None => return Ok(()),
    };

    tasks.push(Task::FillMissing);

    match session.slots.get(&name).cloned() {
        Some(Slot::Prompt {
            lemma_type,
            suggestion,
            location,
        }) => {
            if !*session.header_printed {
                println!("\nEnter values for data (press Enter to accept suggestion):");
                *session.header_printed = true;
            }
            let display = step_label(&location);
            loop {
                let input_value =
                    prompt_value_for_type(&display, lemma_type.as_ref(), suggestion.as_ref())?;
                write_at(
                    session.collected,
                    &location,
                    EvaluateDataValue::Literal(input_value.clone()),
                );
                match session.engine.evaluate(
                    session.repo,
                    session.spec_name,
                    Some(session.now),
                    trial_evaluate_data(session.provided_data, session.collected),
                    session.rules_for_request,
                    false,
                ) {
                    Ok(response) => {
                        if let Some(reason) = computation_veto_reason_for_trial_input(&response) {
                            eprintln!("  {reason}\n");
                            continue;
                        }
                        break;
                    }
                    Err(e) => {
                        eprintln!("  {}\n", e);
                    }
                }
            }
        }
        Some(Slot::DeclaredRow {
            name: list_name,
            location,
            identity,
        }) => {
            let label = step_label(&location);
            let field = name
                .rsplit('.')
                .next()
                .expect("BUG: str::rsplit yields at least one item");
            let replace = Confirm::new(&format!(
                "The default items of {label} have no {field}. Enter your own items instead?"
            ))
            .with_default(true)
            .prompt()
            .context("Failed to confirm entering items")?;
            if !replace {
                anyhow::bail!("data '{name}' is a field of a default item of {}", label);
            }
            let list = ListPrompt {
                name: list_name,
                location,
                identity,
                declared: None,
            };
            tasks.push(Task::EnterItems(Box::new(begin_enter_items(session, list))));
        }
        None => {
            panic!("BUG: MissingData key '{name}' has no interactive slot");
        }
    }
    Ok(())
}

fn top_level_lists_step(
    session: &mut PromptSession<'_>,
    mut lists: std::vec::IntoIter<ListPrompt>,
    tasks: &mut Vec<Task>,
) {
    let next = lists
        .by_ref()
        .find(|list| !session.decided.contains(&list.name));
    if let Some(list) = next {
        tasks.push(Task::TopLevelLists(lists));
        tasks.push(Task::FillMissing);
        tasks.push(Task::PromptList(Box::new(list)));
    }
}

fn prompt_list_step(
    session: &mut PromptSession<'_>,
    list: ListPrompt,
    tasks: &mut Vec<Task>,
) -> Result<()> {
    session.decided.insert(list.name.clone());
    let label = step_label(&list.location);
    if let Some(count) = declared_row_count(&list.declared) {
        let labels = list_choice_labels(count);
        let options = labels.to_vec();
        let selected = Select::new(&format!("What should {label} contain?"), options.clone())
            .prompt()
            .context("Failed to get list choice")?;
        let index = options
            .iter()
            .position(|option| option == &selected)
            .expect("BUG: selected list choice missing from options");
        match list_choice_from_index(index) {
            ListChoice::Keep => {
                let rows = list
                    .declared
                    .as_ref()
                    .and_then(|value| value.list.as_deref())
                    .expect("BUG: declared_row_count saw rows");
                register_declared_row_slots(
                    session.slots,
                    session.engine,
                    &list.name,
                    &list.location,
                    &list.identity,
                    rows,
                );
            }
            ListChoice::Clear => {
                write_at(
                    session.collected,
                    &list.location,
                    EvaluateDataValue::List(Vec::new()),
                );
            }
            ListChoice::Enter => {
                tasks.push(Task::EnterItems(Box::new(begin_enter_items(session, list))))
            }
        }
        return Ok(());
    }

    let add = Confirm::new(&format!("Add an item to {label}?"))
        .with_default(list.location.len() == 1)
        .prompt()
        .context("Failed to confirm adding an item")?;
    if add {
        tasks.push(Task::EnterItems(Box::new(begin_enter_items(session, list))));
    }
    Ok(())
}

fn clear_declared_row_slots(slots: &mut IndexMap<String, Slot>, list_name: &str) {
    let nested_prefix = format!("{list_name}.");
    slots.retain(|_, slot| match slot {
        Slot::DeclaredRow { name, .. } => name != list_name && !name.starts_with(&nested_prefix),
        Slot::Prompt { .. } => true,
    });
}

fn begin_enter_items(session: &mut PromptSession<'_>, list: ListPrompt) -> EnterItems {
    clear_declared_row_slots(session.slots, &list.name);
    write_at(
        session.collected,
        &list.location,
        EvaluateDataValue::List(Vec::new()),
    );
    let row_show = session.engine.show_spec(&list.identity);
    let label = step_label(&list.location);
    EnterItems {
        list,
        label,
        row_show,
        pending_nested: VecDeque::new(),
        phase: EnterPhase::NewRow,
    }
}

fn enter_items_step(
    session: &mut PromptSession<'_>,
    mut state: EnterItems,
    tasks: &mut Vec<Task>,
) -> Result<()> {
    match state.phase {
        EnterPhase::NewRow => {
            let next_number = match collected_list_len(session.collected, &state.list.location) {
                Some(len) => len + 1,
                None => 1,
            };
            println!("\nItem {next_number} of {}:", state.label);
            let number = append_empty_row(session.collected, &state.list.location);
            let mut row_location = state.list.location.clone();
            row_location.push(Step::Row(number - 1));
            let row_origin = numbered_row_origin(&state.list.name, number);
            register_show_slots(
                session.slots,
                &state.row_show,
                row_location.clone(),
                Some(&row_origin),
            );
            state.pending_nested = list_prompts_from_show(
                &state.row_show,
                &None,
                &IndexMap::new(),
                row_location,
                Some(&row_origin),
            )
            .into();
            state.phase = EnterPhase::NextNested;
            tasks.push(Task::EnterItems(Box::new(state)));
            tasks.push(Task::FillMissing);
        }
        EnterPhase::NextNested => match state.pending_nested.pop_front() {
            Some(nested_list) => {
                tasks.push(Task::EnterItems(Box::new(state)));
                tasks.push(Task::FillMissing);
                tasks.push(Task::PromptList(Box::new(nested_list)));
            }
            None => {
                state.phase = EnterPhase::AskAnother;
                tasks.push(Task::EnterItems(Box::new(state)));
            }
        },
        EnterPhase::AskAnother => {
            let add = Confirm::new(&format!("Add another item to {}?", state.label))
                .with_default(false)
                .prompt()
                .context("Failed to confirm adding another item")?;
            if add {
                state.phase = EnterPhase::NewRow;
                tasks.push(Task::EnterItems(Box::new(state)));
            }
        }
    }
    Ok(())
}

fn collected_list_len(
    collected: &IndexMap<String, EvaluateDataValue>,
    location: &[Step],
) -> Option<usize> {
    fn walk(node: &EvaluateDataValue, location: &[Step]) -> Option<usize> {
        let mut current = node;
        for step in location {
            current = match (step, current) {
                (Step::Field(name), EvaluateDataValue::Map(map)) => map.get(name)?,
                (Step::Row(index), EvaluateDataValue::List(rows)) => rows.get(*index)?,
                _ => return None,
            };
        }
        match current {
            EvaluateDataValue::List(rows) => Some(rows.len()),
            _ => None,
        }
    }
    match location {
        [Step::Field(name)] => match collected.get(name) {
            Some(EvaluateDataValue::List(rows)) => Some(rows.len()),
            _ => None,
        },
        [Step::Field(name), rest @ ..] => walk(collected.get(name)?, rest),
        _ => None,
    }
}

fn next_prompt_name_from_results<'a>(
    results: impl IntoIterator<Item = &'a lemma::RuleResult>,
    provided_data: &IndexMap<String, EvaluateDataValue>,
    collected: &IndexMap<String, EvaluateDataValue>,
) -> Option<String> {
    let mut next = None;
    let mut any_missing_data = false;
    for result in results {
        if !result.is_missing_data() {
            continue;
        }
        any_missing_data = true;
        if let Some(name) = result
            .missing_data()
            .iter()
            .find(|name| !provided_data.contains_key(*name) && !collected.contains_key(*name))
        {
            if next.is_none() {
                next = Some(name.clone());
            }
        }
    }
    if any_missing_data && next.is_none() {
        panic!("BUG: MissingData veto but no unbound key left to prompt");
    }
    next
}

fn computation_veto_reason_for_trial_input(response: &Response) -> Option<String> {
    for rule_result in response.results.values() {
        if !rule_result.vetoed {
            continue;
        }
        if matches!(
            rule_result.veto_detail.as_ref(),
            Some(VetoType::Computation { .. })
        ) {
            return rule_result.veto_reason.clone();
        }
    }
    None
}

fn prompt_value_for_type(
    data_name: &str,
    lemma_type: &LemmaType,
    suggestion: Option<&LiteralValue>,
) -> Result<String> {
    let input_label = lemma_type.label_for_data_input(data_name);

    match &lemma_type.specifications {
        TypeSpecification::Boolean { .. } => {
            prompt_boolean_data(&input_label, data_name, suggestion)
        }
        TypeSpecification::Text {
            options,
            length,
            help,
            ..
        } => {
            if !options.is_empty() {
                if options.len() == 1 {
                    return Ok(options[0].clone());
                }
                let mut prompt =
                    Select::new(&input_label, options.clone()).with_help_message(help.as_str());
                if let Some(lit) = suggestion {
                    if let ValueKind::Text(s) = &lit.value {
                        if let Some(idx) = options.iter().position(|o| o == s) {
                            prompt = prompt.with_starting_cursor(idx);
                        }
                    }
                }
                prompt
                    .prompt()
                    .context(format!("Failed to get option for {}", data_name))
            } else {
                let constraints = TextConstraints {
                    length: *length,
                    help: help.clone(),
                };
                prompt_text_data_with_constraints(&input_label, lemma_type, suggestion, &constraints)
            }
        }
        TypeSpecification::Measure {
            minimum,
            maximum,
            decimals,
            units,
            help,
            traits,
            decomposition,
            ..
        } => {
            let measure_spec = TypeSpecification::Measure {
                minimum: minimum.clone(),
                maximum: maximum.clone(),
                decimals: *decimals,
                units: units.clone(),
                traits: traits.clone(),
                decomposition: decomposition.clone(),
                unit: None,
                help: help.clone(),
            };
            let constraints = NumericConstraints {
                minimum: measure_spec.minimum_decimal(),
                maximum: measure_spec.maximum_decimal(),
                decimals: *decimals,
                help: help.clone(),
            };
            prompt_measure_data(
                data_name,
                &input_label,
                suggestion,
                lemma_type,
                units,
                &constraints,
            )
        }
        TypeSpecification::Number {
            minimum,
            maximum,
            decimals,
            help,
            ..
        } => {
            let number_spec = TypeSpecification::Number {
                minimum: minimum.clone(),
                maximum: maximum.clone(),
                decimals: *decimals,
                help: help.clone(),
            };
            let constraints = NumericConstraints {
                minimum: number_spec.minimum_decimal(),
                maximum: number_spec.maximum_decimal(),
                decimals: *decimals,
                help: help.clone(),
            };
            prompt_number_data(&input_label, suggestion, &constraints)
        }
        TypeSpecification::Ratio {
            minimum,
            maximum,
            decimals,
            units,
            help,
            ..
        } => {
            let ratio_spec = TypeSpecification::Ratio {
                minimum: minimum.clone(),
                maximum: maximum.clone(),
                decimals: *decimals,
                units: units.clone(),
                unit: None,
                help: help.clone(),
            };
            let constraints = NumericConstraints {
                minimum: ratio_spec.minimum_decimal(),
                maximum: ratio_spec.maximum_decimal(),
                decimals: *decimals,
                help: help.clone(),
            };
            prompt_ratio_data(
                data_name,
                &input_label,
                suggestion,
                lemma_type,
                units,
                &constraints,
            )
        }
        TypeSpecification::Date { .. } => prompt_date_data(&input_label, data_name, suggestion),
        TypeSpecification::Time { help, .. } => {
            let def = suggestion
                .filter(|l| matches!(l.value, ValueKind::Time(_)))
                .map(|l| l.to_string());
            prompt_simple_text(&input_label, def.as_deref(), help.as_str(), "12:34:56")
        }
        TypeSpecification::NumberRange { help, .. }
        | TypeSpecification::DateRange { help, .. }
        | TypeSpecification::TimeRange { help, .. }
        | TypeSpecification::MeasureRange { help, .. }
        | TypeSpecification::RatioRange { help, .. } => {
            prompt_range_data(data_name, lemma_type, suggestion, help.as_str())
        }
        TypeSpecification::Veto { .. } => {
            anyhow::bail!("Data '{}' has veto type which is not promptable", data_name)
        }
        TypeSpecification::Undetermined => unreachable!(
            "BUG: prompt_value_for_type called with Error sentinel type; this type must never reach interactive mode"
        ),
        TypeSpecification::Spec { .. } => unreachable!(
            "BUG: prompt_value_for_type called with a spec type; spec instances are not prompted data"
        ),
        TypeSpecification::List { .. } => {
            unreachable!("BUG: list data is never missing; an absent list has no rows")
        }
    }
}

fn prompt_date_data(
    prompt_title: &str,
    data_name: &str,
    suggestion: Option<&LiteralValue>,
) -> Result<String> {
    let help_message = if suggestion.is_some() {
        "Use arrow keys to navigate, Enter to select (or accept suggestion)"
    } else {
        "Use arrow keys to navigate, Enter to select"
    };

    let mut ds = DateSelect::new(prompt_title).with_help_message(help_message);

    if let Some(lit) = suggestion {
        if let ValueKind::Date(d) = &lit.value {
            if let Some(naive) = NaiveDate::from_ymd_opt(d.year, d.month, d.day) {
                ds = ds.with_default(naive);
            }
        }
    }

    let date = ds
        .prompt()
        .context(format!("Failed to get date for {}", data_name))?;

    Ok(format!("{}T00:00:00Z", date.format("%Y-%m-%d")))
}

fn prompt_boolean_data(
    prompt_title: &str,
    data_name: &str,
    suggestion: Option<&LiteralValue>,
) -> Result<String> {
    let options = vec!["true", "false"];

    let default_index = match suggestion.and_then(|lit| match &lit.value {
        ValueKind::Boolean(b) => Some(*b),
        _ => None,
    }) {
        Some(true) => 0,
        Some(false) => 1,
        None => 0,
    };

    let help_message = if suggestion.is_some() {
        format!(
            "Default: {} - Use arrow keys to change, Enter to confirm",
            options[default_index]
        )
    } else {
        "Use arrow keys to select, Enter to confirm".to_string()
    };

    let selected = Select::new(prompt_title, options)
        .with_help_message(&help_message)
        .with_starting_cursor(default_index)
        .prompt()
        .context(format!("Failed to get boolean value for {}", data_name))?;

    Ok(selected.to_string())
}

fn prompt_text_data_with_constraints(
    prompt_title: &str,
    lemma_type: &LemmaType,
    suggestion: Option<&LiteralValue>,
    constraints: &TextConstraints,
) -> Result<String> {
    let default_str = suggestion.map(|l| l.to_string());
    let example = lemma_type.example_value();

    let TextConstraints { length, help } = constraints.clone();

    let validator = move |input: &str| {
        let s = input.trim();
        if s.is_empty() {
            return Ok(Validation::Invalid("Value is required".into()));
        }
        if let Some(len) = length {
            if s.chars().count() != len {
                return Ok(Validation::Invalid(
                    format!("Must be exactly {} characters", len).into(),
                ));
            }
        }
        Ok(Validation::Valid)
    };

    let mut prompt = Text::new(prompt_title).with_validator(validator);
    let help_message = if help.is_empty() {
        format!("Example: {}", example)
    } else {
        help.clone()
    };
    prompt = prompt.with_help_message(&help_message);

    if let Some(default) = default_str.as_deref() {
        prompt = prompt.with_default(default);
    }

    prompt
        .prompt()
        .context(format!("Failed to get value for {}", prompt_title))
}

fn prompt_simple_text(
    prompt_title: &str,
    default_value: Option<&str>,
    help: &str,
    example: &str,
) -> Result<String> {
    let validator = |input: &str| {
        if input.trim().is_empty() {
            Ok(Validation::Invalid("Value is required".into()))
        } else {
            Ok(Validation::Valid)
        }
    };
    let mut prompt = Text::new(prompt_title).with_validator(validator);
    let help_message = if help.is_empty() {
        format!("Example: {}", example)
    } else {
        help.to_string()
    };
    prompt = prompt.with_help_message(&help_message);
    if let Some(default) = default_value {
        prompt = prompt.with_default(default);
    }
    prompt
        .prompt()
        .context(format!("Failed to get value for {}", prompt_title))
}

fn prompt_range_data(
    data_name: &str,
    lemma_type: &LemmaType,
    suggestion: Option<&LiteralValue>,
    help: &str,
) -> Result<String> {
    let (left_default, right_default) = match suggestion {
        Some(LiteralValue {
            value: ValueKind::Range(left, right),
            ..
        }) => (Some(left.display_value()), Some(right.display_value())),
        _ => (None, None),
    };

    let endpoint_example = match &lemma_type.specifications {
        TypeSpecification::DateRange { .. } => "2024-01-01",
        TypeSpecification::TimeRange { .. } => "09:00",
        TypeSpecification::NumberRange { .. } => "0",
        TypeSpecification::MeasureRange { .. } => "30 kilogram",
        TypeSpecification::RatioRange { .. } => "10%",
        _ => unreachable!("BUG: prompt_range_data called with non-range type"),
    };

    let start_title = lemma_type.label_for_data_input(&format!("{data_name}.start"));
    let end_title = lemma_type.label_for_data_input(&format!("{data_name}.end"));

    let left_value = prompt_simple_text(
        &start_title,
        left_default.as_deref(),
        help,
        endpoint_example,
    )?;
    let right_value =
        prompt_simple_text(&end_title, right_default.as_deref(), help, endpoint_example)?;

    Ok(format!("{}...{}", left_value.trim(), right_value.trim()))
}

fn prompt_number_data(
    prompt_title: &str,
    suggestion: Option<&LiteralValue>,
    constraints: &NumericConstraints,
) -> Result<String> {
    let default_str = suggestion.map(|l| l.to_string());
    prompt_decimal_input(prompt_title, default_str.as_deref(), constraints, "10")
}

fn prompt_measure_data(
    data_name: &str,
    prompt_title: &str,
    suggestion: Option<&LiteralValue>,
    lemma_type: &LemmaType,
    units: &lemma::MeasureUnits,
    constraints: &NumericConstraints,
) -> Result<String> {
    let default_unit = suggestion.and_then(|lit| match &lit.value {
        ValueKind::Measure(_) => lemma_type
            .measure_runtime_signature()
            .first()
            .map(|(name, _)| name.clone()),
        _ => None,
    });

    if units.is_empty() {
        let default_str = suggestion
            .and_then(|lit| lit.magnitude_suggestion_for_decimal_prompt(lemma_type))
            .map(|decimal| decimal.to_string());
        return prompt_decimal_input(prompt_title, default_str.as_deref(), constraints, "7.65");
    }

    let unit_names: Vec<String> = units.iter().map(|u| u.name.clone()).collect();
    let unit = if unit_names.len() == 1 {
        unit_names[0].clone()
    } else {
        let prompt_msg = format!("Select unit for {}", data_name);
        let mut select = Select::new(&prompt_msg, unit_names);
        if let Some(def_unit) = default_unit.as_deref() {
            if let Some(idx) = units.iter().position(|u| u.name == def_unit) {
                select = select.with_starting_cursor(idx);
            }
        }
        select
            .prompt()
            .context(format!("Failed to get unit for {}", data_name))?
    };

    let numeric_default = suggestion
        .and_then(|lit| lit.magnitude_in_unit(lemma_type, &unit))
        .map(|decimal| decimal.to_string());

    let value_constraints = NumericConstraints {
        help: if constraints.help.is_empty() {
            format!("Enter numeric value (unit: {})", unit)
        } else {
            constraints.help.clone()
        },
        ..constraints.clone()
    };
    let value = prompt_decimal_input(
        &format!("Enter value for {} ({})", data_name, unit),
        numeric_default.as_deref(),
        &value_constraints,
        "7.65",
    )?;

    Ok(format!("{} {}", value, unit))
}

fn prompt_ratio_data(
    data_name: &str,
    prompt_title: &str,
    suggestion: Option<&LiteralValue>,
    lemma_type: &LemmaType,
    units: &lemma::RatioUnits,
    constraints: &NumericConstraints,
) -> Result<String> {
    let selected_unit = if units.len() == 1 {
        units
            .iter()
            .next()
            .map(|u| u.name.clone())
            .expect("BUG: units.len() == 1 yields one unit")
    } else {
        let mut unit_choices: Vec<String> = vec!["(none)".to_string()];
        unit_choices.extend(units.iter().map(|u| u.name.clone()));
        Select::new(
            &format!("Select ratio unit for {}", data_name),
            unit_choices,
        )
        .prompt()
        .context(format!("Failed to get ratio unit for {}", data_name))?
    };
    let default_decimal = if selected_unit == "(none)" {
        suggestion.and_then(|lit| lit.magnitude_suggestion_for_decimal_prompt(lemma_type))
    } else {
        suggestion.and_then(|lit| lit.magnitude_in_unit(lemma_type, &selected_unit))
    }
    .map(|decimal| decimal.to_string());

    let value = prompt_decimal_input(
        prompt_title,
        default_decimal.as_deref(),
        constraints,
        "0.10",
    )?;

    match selected_unit.as_str() {
        "(none)" => Ok(value),
        "percent" => Ok(format!("{}%", value)),
        "permille" => Ok(format!("{}%%", value)),
        other => Ok(format!("{} {}", value, other)),
    }
}

fn prompt_decimal_input(
    prompt_message: &str,
    default_value: Option<&str>,
    constraints: &NumericConstraints,
    example: &str,
) -> Result<String> {
    let NumericConstraints {
        minimum: min,
        maximum: max,
        decimals: decs,
        help,
    } = constraints.clone();

    let validator = move |input: &str| {
        let raw = input.trim();
        if raw.is_empty() {
            return Ok(Validation::Invalid("Value is required".into()));
        }
        let clean = raw.replace('_', "");
        let provided_decimals = clean
            .split_once('.')
            .map(|(_, frac)| frac.len())
            .unwrap_or(0);
        if let Some(d) = decs {
            if provided_decimals > d as usize {
                return Ok(Validation::Invalid(
                    format!("Too many decimals (max {})", d).into(),
                ));
            }
        }
        let value = match Decimal::from_str_exact(&clean) {
            Ok(v) => v,
            Err(_) => {
                return Ok(Validation::Invalid(
                    format!("Invalid number: '{}'", raw).into(),
                ))
            }
        };
        if let Some(min) = min {
            if value < min {
                return Ok(Validation::Invalid(format!("Must be >= {}", min).into()));
            }
        }
        if let Some(max) = max {
            if value > max {
                return Ok(Validation::Invalid(format!("Must be <= {}", max).into()));
            }
        }
        Ok(Validation::Valid)
    };

    let mut prompt = Text::new(prompt_message).with_validator(validator);
    let help_message = if help.is_empty() {
        format!("Example: {}", example)
    } else {
        help.clone()
    };
    prompt = prompt.with_help_message(&help_message);

    if let Some(default) = default_value {
        prompt = prompt.with_default(default);
    }

    let raw = prompt.prompt().context(format!(
        "Failed to get numeric value for {}",
        prompt_message
    ))?;
    Ok(raw.trim().replace('_', ""))
}

#[cfg(test)]
mod tests {
    use super::{
        load_static_show, next_prompt_name_from_results, prompt_data, select_rules,
        spec_picker_label,
    };
    use indexmap::IndexMap;
    use lemma::{DateTimeValue, Engine, SourceType, VetoType};
    use std::sync::Arc;

    fn colliding_alpha2_engine() -> Engine {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Dependency("@iso/countries".to_string()),
                "repo @iso/countries\nspec alpha2\ndata code: text\n".to_string(),
            )])
            .expect("dep");
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("alpha2.lemma"))),
                "spec alpha2\ndata local: number\nrule out: local\n".to_string(),
            )])
            .expect("default repository");
        engine
    }

    #[test]
    fn picker_label_omits_repo_for_default_when_dep_shares_spec_name() {
        let engine = colliding_alpha2_engine();
        let now = DateTimeValue::now();
        let label = spec_picker_label(&engine, None, "alpha2", &now).expect("default show");
        assert_eq!(label, "alpha2 — 1 data, 1 rules");
    }

    #[test]
    fn picker_label_uses_named_repo_when_spec_name_is_unique() {
        let engine = Engine::new();
        let now = DateTimeValue::now();
        let show = load_static_show(&engine, Some("lemma"), "units", &now).expect("stdlib show");
        assert!(show.rules.is_empty(), "units has no rules");
        assert!(show.data.is_empty(), "units has no rule inputs");
        assert!(!show.types.is_empty(), "units exposes typedef slots");
        let label = spec_picker_label(&engine, Some("lemma"), "units", &now).expect("label");
        assert_eq!(
            label,
            format!("lemma units — {} data, 0 rules", show.types.len())
        );
        spec_picker_label(&engine, None, "units", &now)
            .expect_err("unique named-repo spec must not look up in the default repository");
    }

    #[test]
    fn empty_spec_is_selectable_and_runnable() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("empty.lemma"))),
                "spec empty_lib\n".to_string(),
            )])
            .expect("load");
        let now = DateTimeValue::now();
        let label = spec_picker_label(&engine, None, "empty_lib", &now).expect("show");
        assert_eq!(label, "empty_lib — 0 data, 0 rules");
        assert!(select_rules(&engine, None, "empty_lib", &now)
            .expect("select")
            .is_none());
        prompt_data(&engine, None, "empty_lib", &None, &IndexMap::new(), &now).expect("no prompts");
        engine
            .evaluate(None, "empty_lib", Some(&now), IndexMap::new(), None, false)
            .expect("evaluate empty spec");
    }

    #[test]
    fn prompt_data_skips_keys_already_bound() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("bound.lemma"))),
                r#"
spec bound
data a: number
data b: number
rule r: a + b
"#
                .to_string(),
            )])
            .expect("load");
        let mut provided = IndexMap::new();
        provided.insert(
            "a".to_string(),
            lemma::EvaluateDataValue::Literal("1".into()),
        );
        provided.insert(
            "b".to_string(),
            lemma::EvaluateDataValue::Literal("2".into()),
        );
        let collected = prompt_data(
            &engine,
            None,
            "bound",
            &None,
            &provided,
            &DateTimeValue::now(),
        )
        .expect("no prompts when stdin bound every slot");
        assert!(collected.is_empty(), "{collected:?}");
    }

    #[test]
    fn library_spec_with_data_and_no_rules_skips_rule_prompt() {
        let engine = colliding_alpha2_engine();
        let now = DateTimeValue::now();
        let label =
            spec_picker_label(&engine, Some("@iso/countries"), "alpha2", &now).expect("dep show");
        assert_eq!(label, "@iso/countries alpha2 — 1 data, 0 rules");
        assert!(
            select_rules(&engine, Some("@iso/countries"), "alpha2", &now)
                .expect("select")
                .is_none()
        );
        prompt_data(
            &engine,
            Some("@iso/countries"),
            "alpha2",
            &None,
            &IndexMap::new(),
            &now,
        )
        .expect("no prompts for zero-rule spec");
    }

    #[test]
    fn label_for_data_input_matches_show_types() {
        let code = r#"
spec s
data age: number
data gender_code: text
data gender: gender_code
rule use_age: age
rule use_gender: gender
"#;
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("t.lemma"))),
                code.to_string(),
            )])
            .expect("plan");
        let now = DateTimeValue::now();
        let show = engine.show(None, "s", Some(&now)).expect("show");
        let age = show.data.get("age").expect("age");
        assert_eq!(age.lemma_type.label_for_data_input("age"), "age [number]");
        let gender = show.data.get("gender").expect("gender");
        assert_eq!(
            gender.lemma_type.label_for_data_input("gender"),
            "gender [gender_code]"
        );
    }

    #[test]
    fn list_prompts_skip_provided_and_unneeded() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Volatile,
                r#"
spec line
data qty: number
spec order
uses lines: line list
data extra: number
rule total: lines -> sum qty
rule other: extra
"#
                .to_string(),
            )])
            .expect("load");
        let now = DateTimeValue::now();
        let show = engine.show(None, "order", Some(&now)).expect("show");
        let all = super::list_prompts_from_show(&show, &None, &IndexMap::new(), Vec::new(), None);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "lines");

        let other_only = super::list_prompts_from_show(
            &show,
            &Some(vec!["other".to_string()]),
            &IndexMap::new(),
            Vec::new(),
            None,
        );
        assert!(other_only.is_empty(), "{other_only:?}");

        let mut provided = IndexMap::new();
        provided.insert(
            "lines".to_string(),
            lemma::EvaluateDataValue::List(Vec::new()),
        );
        let skipped = super::list_prompts_from_show(&show, &None, &provided, Vec::new(), None);
        assert!(skipped.is_empty(), "{skipped:?}");
    }

    #[test]
    fn write_at_builds_row_tree() {
        let mut root = IndexMap::new();
        super::write_at(
            &mut root,
            &[
                super::Step::Field("items".into()),
                super::Step::Row(0),
                super::Step::Field("weight".into()),
            ],
            lemma::EvaluateDataValue::Literal("20 kilogram".into()),
        );
        let lemma::EvaluateDataValue::List(rows) = root.get("items").expect("items") else {
            panic!("items must be a list");
        };
        let lemma::EvaluateDataValue::Map(fields) = &rows[0] else {
            panic!("row must be an object");
        };
        assert_eq!(
            fields.get("weight"),
            Some(&lemma::EvaluateDataValue::Literal("20 kilogram".into()))
        );
    }

    #[test]
    fn declared_row_missing_key_resolves_to_declared_slot() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Volatile,
                r#"
spec line
data price: number
data qty: number
spec order
uses lines: line list
  -> with price: 2
rule total: lines -> sum qty
"#
                .to_string(),
            )])
            .expect("load");
        let now = DateTimeValue::now();
        let show = engine.show(None, "order", Some(&now)).expect("show");
        let lists = super::list_prompts_from_show(&show, &None, &IndexMap::new(), Vec::new(), None);
        let mut slots = IndexMap::new();
        let list = &lists[0];
        let rows = list
            .declared
            .as_ref()
            .and_then(|value| value.list.as_deref())
            .expect("declared rows");
        super::register_declared_row_slots(
            &mut slots,
            &engine,
            &list.name,
            &list.location,
            &list.identity,
            rows,
        );
        match slots.get("lines.1.qty") {
            Some(super::Slot::DeclaredRow {
                name,
                location,
                identity,
            }) => {
                assert_eq!(name, "lines");
                assert_eq!(location, &list.location);
                assert_eq!(identity, &list.identity);
            }
            other => panic!("expected DeclaredRow, got {other:?}"),
        }
    }

    #[test]
    fn enter_own_items_replaces_declared_slots_with_prompt() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Volatile,
                r#"
spec item
data score: number
spec cart
uses items: item list
  -> with score: 10
  -> with score: 11
rule average: items -> avg score
"#
                .to_string(),
            )])
            .expect("load");
        let now = DateTimeValue::now();
        let show = engine.show(None, "cart", Some(&now)).expect("show");
        let lists = super::list_prompts_from_show(&show, &None, &IndexMap::new(), Vec::new(), None);
        let list = &lists[0];
        let rows = list
            .declared
            .as_ref()
            .and_then(|value| value.list.as_deref())
            .expect("declared rows");
        let mut slots = IndexMap::new();
        super::register_declared_row_slots(
            &mut slots,
            &engine,
            &list.name,
            &list.location,
            &list.identity,
            rows,
        );
        assert!(
            matches!(
                slots.get("items.1.score"),
                Some(super::Slot::DeclaredRow { .. })
            ),
            "precondition: default rows register DeclaredRow"
        );

        super::clear_declared_row_slots(&mut slots, &list.name);
        let row_show = engine.show_spec(&list.identity);
        let row_origin = lemma::numbered_row_origin(&list.name, 1);
        let mut row_location = list.location.clone();
        row_location.push(super::Step::Row(0));
        super::register_show_slots(&mut slots, &row_show, row_location, Some(&row_origin));

        match slots.get("items.1.score") {
            Some(super::Slot::Prompt { .. }) => {}
            other => panic!("enter-own must prompt items.1.score, got {other:?}"),
        }
        assert!(
            !slots.values().any(|slot| matches!(
                slot,
                super::Slot::DeclaredRow { name, .. } if name == &list.name
                    || name.starts_with(&format!("{}.", list.name))
            )),
            "declared default slots must be cleared for this list"
        );
    }

    #[test]
    fn step_label_builds_breadcrumb() {
        assert_eq!(
            super::step_label(&[super::Step::Field("items".into())]),
            "items"
        );
        assert_eq!(
            super::step_label(&[
                super::Step::Field("items".into()),
                super::Step::Row(0),
                super::Step::Field("processing".into()),
            ]),
            "items › item 1 › processing"
        );
        assert_eq!(
            super::step_label(&[
                super::Step::Field("items".into()),
                super::Step::Row(0),
                super::Step::Field("price".into()),
            ]),
            "items › item 1 › price"
        );
    }

    #[test]
    fn list_choice_labels_singular_and_plural() {
        assert_eq!(
            super::list_choice_labels(1),
            [
                "Keep the default item".to_string(),
                "No items".to_string(),
                "Enter my own items".to_string(),
            ]
        );
        assert_eq!(
            super::list_choice_labels(2),
            [
                "Keep the 2 default items".to_string(),
                "No items".to_string(),
                "Enter my own items".to_string(),
            ]
        );
        assert_eq!(super::list_choice_from_index(0), super::ListChoice::Keep);
        assert_eq!(super::list_choice_from_index(1), super::ListChoice::Clear);
        assert_eq!(super::list_choice_from_index(2), super::ListChoice::Enter);
    }

    #[test]
    fn declared_row_count_none_when_no_rows() {
        assert_eq!(super::declared_row_count(&None), None);
    }

    fn run(
        engine: &Engine,
        spec: &str,
        data: IndexMap<String, String>,
        rules: Option<&[String]>,
    ) -> lemma::Response {
        let now = DateTimeValue::now();
        engine
            .evaluate(
                None,
                spec,
                Some(&now),
                data.into_iter()
                    .map(|(k, v)| (k, lemma::EvaluateDataValue::Literal(v)))
                    .collect(),
                rules,
                false,
            )
            .expect("evaluation must succeed")
    }

    #[test]
    fn is_missing_data_only_for_missing_data_veto() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Volatile,
                r#"
spec demo
data a: number
data age: number
data extra: number
rule need: a
rule base: veto "no rate"
  unless age < 70 then 10
rule premium: base * extra
"#
                .to_string(),
            )])
            .expect("load");

        let missing = run(
            &engine,
            "demo",
            IndexMap::<String, String>::new(),
            Some(&["need".to_string()]),
        );
        assert!(
            missing.results.get("need").expect("need").is_missing_data(),
            "MissingData veto must stay open for prompts"
        );

        let mut age_high = IndexMap::<String, String>::new();
        age_high.insert("age".to_string(), "80".into());
        let user = run(&engine, "demo", age_high, Some(&["base".to_string()]));
        let base = user.results.get("base").expect("base");
        assert!(
            matches!(
                base.veto_detail.as_ref(),
                Some(VetoType::UserDefined { .. })
            ),
            "default veto arm must be UserDefined: {:?}",
            base.veto_detail
        );
        assert!(
            !base.is_missing_data(),
            "UserDefined veto must be settled for prompts"
        );

        let mut engine2 = Engine::new();
        engine2
            .load([(
                SourceType::Volatile,
                r#"
spec demo2
data denom: number
data extra: number
rule premium: (1 / denom) * extra
"#
                .to_string(),
            )])
            .expect("load");
        let mut data = IndexMap::<String, String>::new();
        data.insert("denom".to_string(), "0".into());
        let settled = run(&engine2, "demo2", data, Some(&["premium".to_string()]));
        let premium = settled.results.get("premium").expect("premium");
        assert!(
            matches!(
                premium.veto_detail.as_ref(),
                Some(VetoType::Computation { .. })
            ),
            "div by zero must computation-veto: {:?}",
            premium.veto_detail
        );
        assert!(
            !premium.is_missing_data(),
            "Computation veto must be settled for prompts"
        );
    }

    #[test]
    fn next_prompt_skips_settled_computation_leftover_missing_data() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Volatile,
                r#"
spec demo
data denom: number
data is_smoker: boolean
data is_former_smoker: boolean
data years_since_quit: number
rule loading: 1
  unless is_former_smoker then years_since_quit + 1
  unless is_smoker then 2
rule premium: (1 / denom) * loading
"#
                .to_string(),
            )])
            .expect("load");

        let mut data = IndexMap::<String, String>::new();
        data.insert("denom".to_string(), "0".into());
        let response = run(&engine, "demo", data, Some(&["premium".to_string()]));
        let premium = response.results.get("premium").expect("premium");
        assert!(
            matches!(
                premium.veto_detail.as_ref(),
                Some(VetoType::Computation { .. })
            ),
            "{:?}",
            premium.veto_detail
        );
        assert!(
            premium.missing_data().is_empty(),
            "settled Computation must clear missing_data: {:?}",
            premium.missing_data()
        );

        assert_eq!(
            next_prompt_name_from_results(
                response.results.values(),
                &IndexMap::new(),
                &IndexMap::new(),
            ),
            None,
            "settled Computation must not drive prompts"
        );
    }

    #[test]
    fn next_prompt_uses_only_unsettled_rule_missing_data() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Path(Arc::new(std::path::PathBuf::from("prompt_settle.lemma"))),
                r#"
spec demo
data denom: number
data need: number
data is_smoker: boolean
data is_former_smoker: boolean
data years_since_quit: number
rule loading: 1
  unless is_former_smoker then years_since_quit + 1
  unless is_smoker then 2
rule premium: (1 / denom) * loading
rule other: need
"#
                .to_string(),
            )])
            .expect("load");

        let mut data = IndexMap::<String, String>::new();
        data.insert("denom".to_string(), "0".into());
        let response = run(
            &engine,
            "demo",
            data,
            Some(&["premium".to_string(), "other".to_string()]),
        );

        let premium = response.results.get("premium").expect("premium");
        assert!(!premium.is_missing_data());
        assert!(
            premium.missing_data().is_empty(),
            "settled premium must clear missing_data: {:?}",
            premium.missing_data()
        );

        let other = response.results.get("other").expect("other");
        assert!(other.is_missing_data());
        assert_eq!(other.missing_data(), vec!["need".to_string()]);

        assert_eq!(
            next_prompt_name_from_results(
                response.results.values(),
                &IndexMap::new(),
                &IndexMap::new(),
            ),
            Some("need".to_string()),
            "only unsettled other.need must be next"
        );
    }

    #[test]
    #[should_panic(expected = "BUG: MissingData veto but no unbound key left to prompt")]
    fn next_prompt_panics_when_missing_data_but_all_keys_already_provided() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Volatile,
                r#"
spec demo
data need: number
rule other: need
"#
                .to_string(),
            )])
            .expect("load");

        let response = run(
            &engine,
            "demo",
            IndexMap::<String, String>::new(),
            Some(&["other".to_string()]),
        );
        let other = response.results.get("other").expect("other");
        assert!(other.is_missing_data());
        assert_eq!(other.missing_data(), vec!["need".to_string()]);

        let mut provided = IndexMap::new();
        provided.insert(
            "need".to_string(),
            lemma::EvaluateDataValue::Literal("42".into()),
        );
        let collected = IndexMap::new();
        next_prompt_name_from_results(std::slice::from_ref(other), &provided, &collected);
    }

    #[test]
    fn next_prompt_asks_control_before_gated_body() {
        let mut engine = Engine::new();
        engine
            .load([(
                SourceType::Volatile,
                r#"
spec demo
data expensive: number
data flag: boolean
rule main: expensive unless flag then 0
"#
                .to_string(),
            )])
            .expect("load");

        let response = run(&engine, "demo", IndexMap::<String, String>::new(), None);
        assert_eq!(
            next_prompt_name_from_results(
                response.results.values(),
                &IndexMap::new(),
                &IndexMap::new(),
            ),
            Some("flag".to_string()),
            "interactive must ask flag before gated expensive"
        );
    }
}
