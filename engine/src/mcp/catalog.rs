use serde::Serialize;

use crate::documentation::{example_by_path, GuideTopic, EVALUATE_GUIDE, EXAMPLE_RESOURCES};
use crate::mcp::error::ResourceError;

#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceDefinition {
    pub uri: String,
    pub name: String,
    #[serde(rename = "mimeType")]
    pub mime_type: &'static str,
    pub description: String,
}

fn run_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "spec": {
                "type": "string",
                "description": "Spec set id, e.g. pricing"
            },
            "repository": {
                "type": "string",
                "description": "Optional repository qualifier (e.g. lemma, @org/repo). Omit for the default repository."
            },
            "rules": {
                "description": "Optional: one rule name (string) or several (string array). Omit for all rules.",
                "oneOf": [
                    { "type": "string" },
                    { "type": "array", "items": { "type": "string" }, "minItems": 1 }
                ]
            },
            "data": {
                "type": "object",
                "description": "Optional input bindings. Integers as numbers; decimals as strings; unit maps as {\"eur\": \"84\"}. Partial is fine.",
                "additionalProperties": true
            },
            "effective": {
                "type": "string",
                "description": "Optional: evaluate at a specific effective datetime (e.g. '2026', '2026-03', '2026-03-04', '2026-03-04T10:30:00Z')"
            }
        },
        "required": ["spec"]
    })
}

pub fn list_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "run",
            description: "Evaluate rules; returns ASCII explanation trees (always on) plus a Missing data block when inputs are unbound. Pass `rules` to target one or more rules; omit for all. For human intake: call guide (default = evaluate guide; not topic full), then list, show once, run. Missing data lists unbound input keys only — look up type/help/suggest on the Show already fetched. Primary loop: after each user turn, bind every field that utterance decides (entailments), re-run; ask at most one open topic-question when something remains. Never ask the user what the policy means. Never dispose interpretation as truth; use “should” when a judgment call cannot be answered. When the rule answers, present details+answer in domain language for user verify (no tooling jargon to the user) before treating as done. No questionnaire dumps. No re-call show between asks. Do not dump every show data field into run.",
            input_schema: run_input_schema(),
        },
        ToolDefinition {
            name: "evaluate",
            description: "Deprecated alias of `run`. Same arguments and formatted trees. Prefer `run`.",
            input_schema: run_input_schema(),
        },
        ToolDefinition {
            name: "list",
            description: "List loaded specs by repository (name, effective_from, effective_to). Call this first when you do not already know the exact spec name. Do not invent or guess spec names. For human intake call guide (default evaluate guide), then show once and run. When the workspace is empty and write mode is enabled, use add_spec to load Lemma source.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDefinition {
            name: "show",
            description: "Return JSON Show for a spec: declared data catalog (types, constraints, suggestions, units, help, path; empty needed_by_rules = reuse-only) and this spec's rule graph (local plus reachable imports as ShowRule: type, path, branches, depends_on_rules). Call once after list. Static catalog — not a required-input list, not a questionnaire, not something to re-call between run/ask turns. Human intake: call guide (default = evaluate guide); bind only missing_data keys, not reuse-only slots.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "spec": {
                        "type": "string",
                        "description": "Spec set id, e.g. pricing"
                    },
                    "repository": {
                        "type": "string",
                        "description": "Optional repository qualifier (e.g. lemma, @org/repo). Omit for the default repository."
                    },
                    "effective": {
                        "type": "string",
                        "description": "Optional: show at a specific effective datetime"
                    }
                },
                "required": ["spec"]
            }),
        },
        ToolDefinition {
            name: "source",
            description: "Return formatted Lemma source. Pass `repository` (e.g. `lemma` for embedded units stdlib) for the whole repo, or `spec` for a spec. After add_spec / update_spec, call this and paste the result in chat for user verify; do not present the draft you authored.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "repository": {
                        "type": "string",
                        "description": "Repository qualifier (e.g. lemma). Alone: whole repository. With `spec`: that repo's spec."
                    },
                    "spec": {
                        "type": "string",
                        "description": "Spec set id (default repository when repository omitted)"
                    },
                    "effective": {
                        "type": "string",
                        "description": "Optional: get source at a specific effective datetime"
                    }
                }
            }),
        },
        ToolDefinition {
            name: "check",
            description: "Validate Lemma sources (does not load). On success returns JSON quality recommendations array (empty if none). Call add_spec to load after check passes. On failure returns structured diagnostics (kind, message, suggestion, source line/column). Sources resolve cross-file `uses` within the batch. A leading `@` label loads as a dependency. Lemma has no `#` or `//` comments; commentary is valid only as a docstring immediately after the `spec` line. Before drafting new specs, call guide with topic full (or method then data). Finish Interrogate first: do not call check or add_spec in the same turn as the first policy questions; wait for answers or an explicit acceptance that the source already states the gaps.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "sources": {
                        "type": "array",
                        "items": {
                            "type": "array",
                            "items": { "type": "string" },
                            "minItems": 2,
                            "maxItems": 2
                        },
                        "description": "Array of [label, code] pairs. Label is the source path (e.g. 'pricing.lemma') or a dependency identifier (e.g. '@org/repo')."
                    }
                },
                "required": ["sources"]
            }),
        },
        ToolDefinition {
            name: "guide",
            description: "Return a Lemma guide. Omit topic for the evaluate guide (CS intake with loaded specs; default). That guide forbids writing or redesigning specs. Authoring: pass topic full (complete authoring guide), or method then data. Other authoring sections: syntax, rules, units, veto, composition, anti_patterns; topic evaluate is the same as the default.",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "enum": ["method", "syntax", "data", "rules", "units", "veto", "composition", "anti_patterns", "evaluate", "full"],
                        "description": "Optional. Omit for evaluate guide (do not write specs). Use full when authoring; method then data also fine."
                    }
                }
            }),
        },
    ]
}

pub fn list_resources() -> Vec<ResourceDefinition> {
    let mut resources = vec![ResourceDefinition {
        uri: "lemma://guide".to_string(),
        name: "Lemma evaluate guide".to_string(),
        mime_type: "text/plain",
        description: "Default evaluate guide for CS intake. Same as guide tool with no topic. Use lemma://guide/full only when authoring.".to_string(),
    }];
    for topic in GuideTopic::ALL {
        resources.push(ResourceDefinition {
            uri: format!("lemma://guide/{}", topic.as_str()),
            name: format!("Lemma guide: {}", topic.as_str()),
            mime_type: "text/plain",
            description: format!("Guide section '{}'", topic.as_str()),
        });
    }
    for example in EXAMPLE_RESOURCES {
        resources.push(ResourceDefinition {
            uri: format!("lemma://examples/{}", example.path),
            name: example.path.to_string(),
            mime_type: "text/plain",
            description: format!("Example Lemma source: {}", example.path),
        });
    }
    resources
}

pub fn read_resource(uri: &str) -> Result<&'static str, ResourceError> {
    if uri == "lemma://guide" {
        return Ok(EVALUATE_GUIDE);
    }
    if let Some(topic_name) = uri.strip_prefix("lemma://guide/") {
        let topic = GuideTopic::parse(topic_name).ok_or_else(|| {
            ResourceError::UnknownUri(format!(
                "Unknown guide topic '{topic_name}'. Valid: {}",
                GuideTopic::VALID_LIST
            ))
        })?;
        return Ok(topic.section_text());
    }
    if let Some(path) = uri.strip_prefix("lemma://examples/") {
        return example_by_path(path)
            .ok_or_else(|| ResourceError::UnknownUri(format!("Unknown example resource: {uri}")));
    }
    Err(ResourceError::UnknownUri(format!(
        "Unknown resource URI: {uri}. Use resources/list."
    )))
}
