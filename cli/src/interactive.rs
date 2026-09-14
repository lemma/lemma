use anyhow::{Context, Result};
use chrono::NaiveDate;
use inquire::validator::Validation;
use inquire::{DateSelect, MultiSelect, Select, Text};
use lemma::{
    DateTimeValue, Engine, LemmaType, ListedSpec, LiteralValue, Response, TypeSpecification,
    ValueKind, VetoType,
};
use rust_decimal::Decimal;
use std::collections::HashMap;

pub(crate) fn repository_loaded(engine: &Engine, name: &str) -> bool {
    engine
        .list()
        .iter()
        .any(|r| r.repository.as_deref() == Some(name))
}

/// Repository qualifier, spec name, selected rules, merged data (CLI + prompts).
pub type InteractiveResult = (
    Option<String>,
    String,
    Option<Vec<String>>,
    HashMap<String, String>,
);

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

pub fn run_interactive(
    engine: &Engine,
    spec_name: Option<String>,
    rule_names: Option<Vec<String>>,
    provided_data: &HashMap<String, String>,
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

fn is_active_at(ls: &ListedSpec, now: &DateTimeValue) -> bool {
    let after_start = match &ls.effective_from {
        None => true,
        Some(from) => now >= from,
    };
    let before_end = match &ls.effective_to {
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
    let mut items: Vec<(Option<String>, ListedSpec)> = engine
        .list()
        .into_iter()
        .flat_map(|repo| {
            let repo_name = repo.repository.clone();
            repo.specs
                .into_iter()
                .filter(|ls| is_active_at(ls, now))
                .map(move |ls| (repo_name.clone(), ls))
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
        .map(|(repo_name, ls)| spec_picker_label(engine, repo_name.as_deref(), &ls.name, now))
        .collect::<Result<Vec<_>>>()?;

    let selected = Select::new("Select a spec:", display_options.clone())
        .with_help_message("Use arrow keys to navigate, Enter to select")
        .prompt()
        .context("Failed to get spec selection")?;

    let spec_index = display_options
        .iter()
        .position(|d| d == &selected)
        .context("Failed to find selected spec index")?;

    let (repo_name, ls) = items
        .into_iter()
        .nth(spec_index)
        .context("Failed to match selected spec")?;

    Ok((repo_name, ls.name))
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
        show.data.len(),
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
    let rule_names: Vec<String> = show.rules.keys().cloned().collect();

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

fn prompt_data(
    engine: &Engine,
    repo: Option<&str>,
    spec_name: &str,
    rule_names: &Option<Vec<String>>,
    provided_data: &HashMap<String, String>,
    now: &DateTimeValue,
) -> Result<HashMap<String, String>> {
    let mut collected: HashMap<String, String> = HashMap::new();
    let mut header_printed = false;
    let rules_for_request = rule_names.as_deref().filter(|rules| !rules.is_empty());
    let show = load_static_show(engine, repo, spec_name, now)?;

    loop {
        let mut trial = provided_data.clone();
        trial.extend(collected.clone());
        let response = engine
            .run(repo, spec_name, Some(now), trial, rules_for_request, false)
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let next_name =
            next_prompt_name_from_results(response.results.values(), provided_data, &collected);

        let name = match next_name {
            Some(name) => name,
            None => break,
        };

        let entry = show
            .data
            .get(&name)
            .unwrap_or_else(|| panic!("BUG: missing_data key {name:?} must exist in show.data"));
        let lemma_type = entry.lemma_type.clone();
        // `ShowData.suggestion` is the API-facing per-unit map (`RuleResultValue`); prompts
        // need canonical `LiteralValue` methods (`magnitude_in_unit`, unit signatures, etc.),
        // so reconstruct once here and thread `LiteralValue` through the existing prompt code.
        let suggestion = entry.suggestion.as_ref().map(|v| v.to_literal(&lemma_type));
        let suggestion = suggestion.as_ref();

        if !header_printed {
            println!("\nEnter values for data (press Enter to accept suggestion):");
            header_printed = true;
        }

        loop {
            let input_value = prompt_value_for_type(&name, &lemma_type, suggestion)?;

            let mut validation_trial = provided_data.clone();
            validation_trial.extend(collected.clone());
            validation_trial.insert(name.clone(), input_value.clone());
            match engine.run(
                repo,
                spec_name,
                Some(now),
                validation_trial,
                rules_for_request,
                false,
            ) {
                Ok(response) => {
                    if let Some(reason) = computation_veto_reason_for_trial_input(&response) {
                        eprintln!("  {reason}\n");
                        continue;
                    }
                    collected.insert(name.clone(), input_value);
                    break;
                }
                Err(e) => {
                    eprintln!("  {}\n", e);
                }
            }
        }
    }

    Ok(collected)
}

fn next_prompt_name_from_results<'a>(
    results: impl IntoIterator<Item = &'a lemma::RuleResult>,
    provided_data: &HashMap<String, String>,
    collected: &HashMap<String, String>,
) -> Option<String> {
    let mut next = None;
    let mut any_awaiting = false;
    for result in results {
        if !result.awaits_missing_data() {
            continue;
        }
        any_awaiting = true;
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
    if any_awaiting && next.is_none() {
        panic!("BUG: MissingData awaits but no unbound key left to prompt");
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
        let default_str =
            suggestion.and_then(|lit| lit.magnitude_suggestion_for_decimal_prompt(lemma_type));
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

    let numeric_default = suggestion.and_then(|lit| lit.magnitude_in_unit(lemma_type, &unit));

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
            .unwrap_or_else(|| "(none)".to_string())
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
    };

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
        let clean = raw.replace(['_', ','], "");
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
    Ok(raw.trim().replace(['_', ','], ""))
}

#[cfg(test)]
mod tests {
    use super::{
        load_static_show, next_prompt_name_from_results, prompt_data, select_rules,
        spec_picker_label,
    };
    use lemma::{DateTimeValue, Engine, SourceType, VetoType};
    use std::collections::HashMap;
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
        assert!(!show.data.is_empty(), "units exposes typedef slots");
        let label = spec_picker_label(&engine, Some("lemma"), "units", &now).expect("label");
        assert_eq!(
            label,
            format!("lemma units — {} data, 0 rules", show.data.len())
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
        prompt_data(&engine, None, "empty_lib", &None, &HashMap::new(), &now).expect("no prompts");
        engine
            .run(None, "empty_lib", Some(&now), HashMap::new(), None, false)
            .expect("run empty spec");
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
            &HashMap::new(),
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

    fn run(
        engine: &Engine,
        spec: &str,
        data: HashMap<String, String>,
        rules: Option<&[String]>,
    ) -> lemma::Response {
        let now = DateTimeValue::now();
        engine
            .run(None, spec, Some(&now), data, rules, false)
            .expect("evaluation must succeed")
    }

    #[test]
    fn awaits_missing_data_only_for_missing_data_veto() {
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

        let missing = run(&engine, "demo", HashMap::new(), Some(&["need".to_string()]));
        assert!(
            missing
                .results
                .get("need")
                .expect("need")
                .awaits_missing_data(),
            "MissingData veto must stay open for prompts"
        );

        let mut age_high = HashMap::new();
        age_high.insert("age".to_string(), "80".to_string());
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
            !base.awaits_missing_data(),
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
        let mut data = HashMap::new();
        data.insert("denom".to_string(), "0".to_string());
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
            !premium.awaits_missing_data(),
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

        let mut data = HashMap::new();
        data.insert("denom".to_string(), "0".to_string());
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

        let empty = HashMap::new();
        assert_eq!(
            next_prompt_name_from_results(response.results.values(), &empty, &empty),
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

        let mut data = HashMap::new();
        data.insert("denom".to_string(), "0".to_string());
        let response = run(
            &engine,
            "demo",
            data,
            Some(&["premium".to_string(), "other".to_string()]),
        );

        let premium = response.results.get("premium").expect("premium");
        assert!(!premium.awaits_missing_data());
        assert!(
            premium.missing_data().is_empty(),
            "settled premium must clear missing_data: {:?}",
            premium.missing_data()
        );

        let other = response.results.get("other").expect("other");
        assert!(other.awaits_missing_data());
        assert_eq!(other.missing_data(), vec!["need".to_string()]);

        let empty = HashMap::new();
        assert_eq!(
            next_prompt_name_from_results(response.results.values(), &empty, &empty),
            Some("need".to_string()),
            "only unsettled other.need must be next"
        );
    }

    #[test]
    #[should_panic(expected = "BUG: MissingData awaits but no unbound key left to prompt")]
    fn next_prompt_panics_when_awaiting_but_all_keys_already_provided() {
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
            HashMap::new(),
            Some(&["other".to_string()]),
        );
        let other = response.results.get("other").expect("other");
        assert!(other.awaits_missing_data());
        assert_eq!(other.missing_data(), vec!["need".to_string()]);

        let mut provided = HashMap::new();
        provided.insert("need".to_string(), "42".to_string());
        let collected = HashMap::new();
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

        let response = run(&engine, "demo", HashMap::new(), None);
        let empty = HashMap::new();
        assert_eq!(
            next_prompt_name_from_results(response.results.values(), &empty, &empty),
            Some("flag".to_string()),
            "interactive must ask flag before gated expensive"
        );
    }
}
