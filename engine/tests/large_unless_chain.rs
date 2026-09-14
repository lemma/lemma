//! Regression: temporal inheritance chains with large unless lookup tables (alpha2.lemma shape).

use lemma::{DateGranularity, DateTimeValue, Engine, SourceType};
use std::path::PathBuf;
use std::sync::Arc;

fn date(y: i32, m: u32, d: u32) -> DateTimeValue {
    DateTimeValue {
        year: y,
        month: m,
        day: d,
        hour: 0,
        minute: 0,
        second: 0,
        microsecond: 0,
        timezone: None,
        granularity: DateGranularity::Full,
    }
}

fn alpha2_analog_source(branch_count: usize, layer_count: usize) -> String {
    let mut src = String::from(
        "spec lookup 1974-01-01\n\
         data code: text\n",
    );
    for i in 0..branch_count {
        src.push_str(&format!("  -> option \"C{i:03}\"\n"));
    }
    src.push_str("rule name: veto \"invalid\"\n");
    for i in 0..branch_count {
        src.push_str(&format!(
            "  unless code is \"C{i:03}\" then \"Name {i:03}\"\n"
        ));
    }
    src.push('\n');

    let mut prev_alias = "from1974".to_string();
    let mut prev_spec = "lookup 1974-01-01".to_string();
    for layer in 1..layer_count {
        let year = 1974 + layer * 3;
        src.push_str(&format!(
            "spec lookup {year}-01-01\n\nuses {prev_alias}: {prev_spec}\n  -> with code: code\n\n"
        ));
        src.push_str(&format!(
            "data code: {prev_alias}.code\n  -> option \"L{layer:02}\"\n\n"
        ));
        src.push_str(&format!("rule name: {prev_alias}.name\n"));
        src.push_str(&format!(
            "  unless code is \"L{layer:02}\" then \"Layer {layer}\"\n\n"
        ));
        prev_alias = format!("from{year}");
        prev_spec = format!("lookup {year}-01-01");
    }
    src
}

#[test]
fn alpha2_analog_199_branches_compiles_and_evaluates() {
    let src = alpha2_analog_source(199, 18);
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("alpha2_analog.lemma"))),
            &src.to_string(),
        )])
        .expect("alpha2 analog must load");

    let effective = date(2025, 1, 1);
    engine
        .show(None, "lookup", Some(&effective))
        .expect("plan at latest slice");

    let mut data = std::collections::HashMap::new();
    data.insert("code".to_string(), "C042".to_string());
    let response = engine
        .run(
            None,
            "lookup",
            Some(&effective),
            data,
            Some(&["name".to_string()]),
            false,
        )
        .expect("evaluate base code");
    let result = response.results.get("name").expect("name rule");
    assert_eq!(
        result.display().map(|d| d.to_string()),
        Some("Name 042".to_string())
    );

    let mut data = std::collections::HashMap::new();
    data.insert("code".to_string(), "L17".to_string());
    let response = engine
        .run(
            None,
            "lookup",
            Some(&effective),
            data,
            Some(&["name".to_string()]),
            false,
        )
        .expect("evaluate layer code");
    let result = response.results.get("name").expect("name rule");
    assert_eq!(
        result.display().map(|d| d.to_string()),
        Some("Layer 17".to_string())
    );
}

#[test]
fn alpha2_analog_explanation_states_evaluated_conditions() {
    let src = alpha2_analog_source(199, 18);
    let mut engine = Engine::new();
    engine
        .load([(
            SourceType::Path(Arc::new(PathBuf::from("alpha2_analog.lemma"))),
            &src.to_string(),
        )])
        .expect("alpha2 analog must load");

    let effective = date(2025, 1, 1);

    let mut data = std::collections::HashMap::new();
    data.insert("code".to_string(), "L17".to_string());
    let response = engine
        .run(
            None,
            "lookup",
            Some(&effective),
            data,
            Some(&["name".to_string()]),
            true,
        )
        .expect("evaluate layer code with explanation");
    let explanation = response
        .results
        .get("name")
        .expect("name rule")
        .explanation
        .as_ref()
        .expect("explanation built");
    assert_eq!(
        explanation.causes.len(),
        1,
        "only the matching condition should be a cause, got {:?}",
        explanation.causes
    );
    assert!(
        explanation
            .causes
            .iter()
            .any(|c| c.condition == "code is L17" && c.value == "true"),
        "expected the matched condition stated as a fact, got {:?}",
        explanation.causes
    );

    let mut data = std::collections::HashMap::new();
    data.insert("code".to_string(), "C042".to_string());
    let response = engine
        .run(
            None,
            "lookup",
            Some(&effective),
            data,
            Some(&["name".to_string()]),
            true,
        )
        .expect("evaluate base code with explanation");
    let explanation = response
        .results
        .get("name")
        .expect("name rule")
        .explanation
        .as_ref()
        .expect("explanation built");
    assert_eq!(
        explanation.causes.len(),
        0,
        "exclusive default must not dump is-not causes, got {:?}",
        explanation.causes
    );
    assert!(
        explanation.children.iter().any(|child| {
            matches!(
                child,
                lemma::ExplanationNode::Data { name, display }
                    if name.input_key() == "code" && display == "C042"
            ) || matches!(
                child,
                lemma::ExplanationNode::Compose { operands, .. }
                    if operands.iter().any(|op| {
                        matches!(
                            op,
                            lemma::ExplanationNode::Data { name, display }
                                if name.input_key() == "code" && display == "C042"
                        )
                    })
            )
        }),
        "exclusive default must attach scrutinee data, got {:?}",
        explanation.children
    );
}
