use std::path::PathBuf;

use lemma_lsp::semantic_tokens::TOKEN_TYPES;

const EXPECTED_TOKEN_COUNT: usize = 13;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("BUG: xtask directory has no parent")
        .to_path_buf()
}

#[test]
fn monaco_js_token_types_sync() {
    let monaco_path = repo_root().join("engine/packages/npm/monaco.js");

    let monaco_content = std::fs::read_to_string(&monaco_path)
        .unwrap_or_else(|e| panic!("Failed to read monaco.js at {:?}: {}", monaco_path, e));

    assert_eq!(
        TOKEN_TYPES.len(),
        EXPECTED_TOKEN_COUNT,
        "TOKEN_TYPES length changed. Update EXPECTED_TOKEN_COUNT and verify monaco.js + vscode package.json."
    );

    let token_names: Vec<String> = TOKEN_TYPES.iter().map(|t| t.as_str().to_string()).collect();

    let mut found_export = false;
    let mut monaco_types = Vec::new();

    for line in monaco_content.lines() {
        if line.contains("export const SEMANTIC_TOKEN_TYPES = [") {
            found_export = true;
            continue;
        }
        if found_export {
            if line.contains("];") {
                break;
            }
            if let Some(stripped) = line.trim().strip_prefix('\'') {
                if let Some(name) = stripped.split('\'').next() {
                    monaco_types.push(name.to_string());
                }
            }
        }
    }

    assert_eq!(
        monaco_types.len(),
        EXPECTED_TOKEN_COUNT,
        "monaco.js SEMANTIC_TOKEN_TYPES array has {} entries, expected {}",
        monaco_types.len(),
        EXPECTED_TOKEN_COUNT
    );

    for (i, (rust_name, monaco_name)) in token_names.iter().zip(monaco_types.iter()).enumerate() {
        assert_eq!(
            rust_name, monaco_name,
            "Token type mismatch at index {}: Rust has '{}', monaco.js has '{}'",
            i, rust_name, monaco_name
        );
    }

    assert!(
        monaco_content.contains("{ token: 'declarationKeyword'"),
        "monaco.js LEMMA_MONACO_RULES missing declarationKeyword entry"
    );

    assert!(
        monaco_content.contains("'declarationKeyword':"),
        "monaco.js LEMMA_SEMANTIC_COLORS missing declarationKeyword entry"
    );

    for name in &token_names {
        assert!(
            monaco_content.contains(&format!("{{ token: '{}'", name))
                || monaco_content.contains(&format!("{{token: '{}'", name))
                || monaco_content.contains(&format!("{{ token: \"{}\"", name))
                || monaco_content.contains(&format!("{{token: \"{}\"", name)),
            "monaco.js LEMMA_MONACO_RULES missing entry for token type '{}'",
            name
        );

        assert!(
            monaco_content.contains(&format!("'{}': ", name))
                || monaco_content.contains(&format!("\"{}\": ", name)),
            "monaco.js LEMMA_SEMANTIC_COLORS missing entry for token type '{}'",
            name
        );
    }
}

#[test]
fn vscode_package_json_sync() {
    let package_json_path = repo_root().join("engine/lsp/editors/vscode/package.json");

    let package_json_content = std::fs::read_to_string(&package_json_path).unwrap_or_else(|e| {
        panic!(
            "Failed to read package.json at {:?}: {}",
            package_json_path, e
        )
    });

    let package_json: serde_json::Value = serde_json::from_str(&package_json_content)
        .unwrap_or_else(|e| panic!("Failed to parse package.json: {}", e));

    assert_eq!(
        TOKEN_TYPES.len(),
        EXPECTED_TOKEN_COUNT,
        "TOKEN_TYPES length changed. Update EXPECTED_TOKEN_COUNT and verify vscode package.json."
    );

    let token_names: Vec<String> = TOKEN_TYPES.iter().map(|t| t.as_str().to_string()).collect();

    let custom_types = vec![
        "controlKeyword",
        "dataBody",
        "punctuation",
        "reference",
        "declarationKeyword",
    ];

    if let Some(semantic_token_types) = package_json["contributes"]["semanticTokenTypes"].as_array()
    {
        for custom_type in &custom_types {
            let found = semantic_token_types
                .iter()
                .any(|entry| entry.get("id").and_then(|id| id.as_str()) == Some(custom_type));
            assert!(
                found,
                "vscode package.json semanticTokenTypes missing custom type '{}'",
                custom_type
            );
        }
    } else {
        panic!("vscode package.json missing contributes.semanticTokenTypes");
    }

    if let Some(scopes_array) = package_json["contributes"]["semanticTokenScopes"].as_array() {
        if let Some(lemma_scopes) = scopes_array.first() {
            if let Some(scopes) = lemma_scopes.get("scopes").and_then(|s| s.as_object()) {
                for name in &token_names {
                    assert!(
                        scopes.contains_key(name),
                        "vscode package.json semanticTokenScopes missing entry for '{}'",
                        name
                    );
                }
            } else {
                panic!("vscode package.json semanticTokenScopes[0] missing scopes object");
            }
        } else {
            panic!("vscode package.json semanticTokenScopes array is empty");
        }
    } else {
        panic!("vscode package.json missing contributes.semanticTokenScopes");
    }

    if let Some(config_defaults) = package_json
        .get("contributes")
        .and_then(|c| c.get("configurationDefaults"))
        .and_then(|cd| cd.get("[lemma]"))
        .and_then(|lemma| lemma.get("editor.semanticTokenColorCustomizations"))
        .and_then(|stcc| stcc.get("rules"))
        .and_then(|rules| rules.as_object())
    {
        for name in &token_names {
            assert!(
                config_defaults.contains_key(name),
                "vscode package.json configurationDefaults color rules missing entry for '{}'",
                name
            );
        }
    } else {
        panic!("vscode package.json missing configurationDefaults color rules");
    }
}
