//! Load, run, and format-check every ```lemma fence in repo markdown and text files.
//!
//! Consecutive ```lemma fences separated only by whitespace load as one
//! workspace source (so multi-spec examples may stay in separate visual blocks).
//! Format checks each visual fence on its own via `lemma format` / `format_lemma_fences_in_text`.

use indexmap::IndexMap;
use lemma::{
    parse, Context, DateTimeValue, Engine, Registries, Resolve, ResourceLimits, SourceType,
    EMBEDDED_STDLIB_REPOSITORY,
};
use lemma_cli::doc_fences::{
    extract_lemma_fences, format_lemma_fences_in_text, group_whitespace_adjacent_fences, LemmaFence,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn collect_markdown_and_text_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .build()
    {
        let entry = entry.unwrap_or_else(|e| panic!("walk {}: {e}", root.display()));
        let path = entry.path();
        let is_md = path.extension().is_some_and(|e| e == "md");
        let is_txt = path.extension().is_some_and(|e| e == "txt");
        if path.is_file() && (is_md || is_txt) {
            files.push(path.to_path_buf());
        }
    }
    files.sort();
    files
}

fn load_fences(content: &str) -> Vec<LemmaFence> {
    group_whitespace_adjacent_fences(content, extract_lemma_fences(content))
}

fn fence_label(path: &Path, line: usize) -> String {
    format!("{}:{line}", path.display())
}

fn specs_declared_in_fence(content: &str, source: &SourceType) -> Vec<(Option<String>, String)> {
    let limits = ResourceLimits::default();
    let parsed = parse(content, source.clone(), &limits).unwrap_or_else(|e| {
        panic!("parse fence source failed: {e}");
    });
    parsed
        .repositories
        .into_iter()
        .flat_map(|(repo, specs)| {
            let qualifier = repo.name.clone();
            specs
                .into_iter()
                .map(move |spec| (qualifier.clone(), spec.name))
        })
        .collect()
}

fn fence_needs_registry(content: &str) -> bool {
    content.contains('@')
}

fn load_fence_engine(
    path: &Path,
    fence: &LemmaFence,
    registries: &Registries,
    transport: &lemma::__test_support::FixtureTransport,
) -> Engine {
    let label = fence_label(path, fence.line);
    let workspace_source = SourceType::Path(Arc::new(PathBuf::from(&label)));
    let limits = ResourceLimits::default();

    let parsed = parse(&fence.content, workspace_source.clone(), &limits).unwrap_or_else(|e| {
        panic!("parse ```lemma fence at {label} failed: {e}");
    });

    let mut ctx = Context::new();
    for (parsed_repo, specs) in &parsed.repositories {
        for spec in specs {
            ctx.insert_spec(Arc::clone(parsed_repo), spec.clone())
                .unwrap_or_else(|errors| {
                    panic!(
                        "insert spec for fence at {label} failed: {}",
                        errors
                            .iter()
                            .map(|e| e.to_string())
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                });
        }
    }

    let mut sources = HashMap::from([(workspace_source.clone(), fence.content.clone())]);

    if fence_needs_registry(&fence.content) {
        Resolve::run(registries, &mut ctx, &mut sources, &limits, transport).unwrap_or_else(
            |errs| {
                panic!(
                    "resolve registry for ```lemma fence at {label} failed: {}",
                    errs.iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                );
            },
        );
    }

    let mut engine = Engine::new();
    let local: HashSet<_> = HashSet::from([workspace_source.clone()]);

    for (source_id, code) in &sources {
        if local.contains(source_id) {
            continue;
        }
        engine
            .load([(source_id.clone(), code.clone())])
            .unwrap_or_else(|errs| {
                panic!(
                    "load registry dep for ```lemma fence at {label} failed: {}",
                    errs.iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                );
            });
    }

    engine
        .load([(workspace_source, fence.content.clone())])
        .unwrap_or_else(|errs| {
            panic!(
                "load ```lemma fence at {label} failed: {}",
                errs.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        });

    engine
}

fn evaluate_fence(
    path: &Path,
    fence: &LemmaFence,
    registries: &Registries,
    transport: &lemma::__test_support::FixtureTransport,
) {
    let label = fence_label(path, fence.line);
    let workspace_source = SourceType::Path(Arc::new(PathBuf::from(&label)));
    let target_specs = specs_declared_in_fence(&fence.content, &workspace_source);
    let engine = load_fence_engine(path, fence, registries, transport);
    let now = DateTimeValue::now();
    for (repo, spec_name) in target_specs {
        if repo.as_deref() == Some(EMBEDDED_STDLIB_REPOSITORY) {
            continue;
        }
        engine
            .evaluate(
                repo.as_deref(),
                &spec_name,
                Some(&now),
                IndexMap::new(),
                None,
                false,
            )
            .unwrap_or_else(|e| {
                panic!("run ```lemma fence at {label} spec {spec_name} failed: {e}")
            });
    }
}

#[test]
fn all_lemma_fences_load_and_run() {
    let registries = Registries::default();
    let transport = lemma::__test_support::FixtureTransport::bundled();
    let root = workspace_root();
    let files = collect_markdown_and_text_files(&root);
    let mut ran = 0usize;
    for path in files {
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        for fence in load_fences(&content) {
            evaluate_fence(&path, &fence, &registries, &transport);
            ran += 1;
        }
    }
    assert!(ran > 0, "must run at least one ```lemma fence");
}

#[test]
fn all_lemma_fences_are_formatted() {
    let root = workspace_root();
    let files = collect_markdown_and_text_files(&root);
    let mut checked = 0usize;
    for path in files {
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        if !content.contains("```lemma") {
            continue;
        }
        let formatted = format_lemma_fences_in_text(&content).unwrap_or_else(|e| {
            panic!(
                "{}:{}: format ```lemma fence failed: {}",
                path.display(),
                e.fence_line,
                e.error
            )
        });
        assert_eq!(
            content,
            formatted,
            "{} has unformatted ```lemma fences; run `lemma format`",
            path.display()
        );
        checked += extract_lemma_fences(&content).len();
    }
    assert!(checked > 0, "must format-check at least one ```lemma fence");
}

#[test]
fn consecutive_whitespace_separated_fences_group() {
    let file = "\
```lemma
spec a
data x: 1
```

```lemma
spec b
uses a
rule r: a.x
```

prose here

```lemma
spec c
data y: 2
```
";
    let groups = load_fences(file);
    assert_eq!(groups.len(), 2, "got {} groups", groups.len());
    assert!(groups[0].content.contains("spec a") && groups[0].content.contains("spec b"));
    assert!(groups[1].content.contains("spec c"));
    assert!(!groups[1].content.contains("spec a"));
}
