mod data_json;
mod error_formatter;
mod formatter;
mod interactive;
mod mcp;
pub(crate) mod server;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use formatter::{Formatter, RepositorySpecGroup};
use lemma::DateTimeValue;
use lemma::Engine;
use lemma_cli::install;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use walkdir::WalkDir;

#[derive(Parser)]
#[command(name = "lemma")]
#[command(about = "A pure, declarative language for business rules.")]
#[command(
    long_about = "Lemma is a declarative programming language for business logic, expressed simply and clearly.\nThe CLI lets you evaluate rules from .lemma files, run Lemma as an HTTP server, or integrate with AI tools via MCP."
)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Evaluate rules and display results
    ///
    /// Load a workspace (see `--prefix`), evaluate the specified spec, and display results.
    ///
    /// Examples:
    ///   lemma run calculator income=85000
    ///   lemma run --prefix tax.lemma calculator income=85000
    ///   lemma run --prefix ./project calculator income=85000
    ///   lemma run @iso/countries alpha2
    Run {
        /// [repo] [spec] [name=value ...] — optional repository qualifier (e.g. `@org/pkg`), then spec name
        args: Vec<String>,
        /// Workspace directory or `.lemma` file (default: current directory)
        #[arg(long, value_name = "PATH")]
        prefix: Option<PathBuf>,
        /// Rules to evaluate (comma-separated); omit to evaluate all rules
        #[arg(long, value_name = "RULES")]
        rules: Option<String>,
        /// Include data and explanation trees (table) or explanation objects (json)
        #[arg(short = 'x', long)]
        explain: bool,
        /// Output results as JSON
        #[arg(long)]
        json: bool,
        /// Enable interactive mode for spec/rule/data selection
        #[arg(short = 'i', long)]
        interactive: bool,
        /// Effective datetime for evaluation (e.g. 2026, 2026-03, 2026-03-04, 2026-03-04T10:30:00Z)
        #[arg(long)]
        effective: Option<String>,
    },
    /// Spec interface (data types, constraints, and rules)
    ///
    /// Examples:
    ///   lemma show --prefix tax.lemma
    ///   lemma show calculator
    ///   lemma show '@iso/countries' alpha2
    Show {
        /// Repository qualifier (e.g. `@iso/countries`)
        repo: Option<String>,
        /// Spec name
        spec: Option<String>,
        /// Workspace directory or `.lemma` file (default: current directory)
        #[arg(long, value_name = "PATH")]
        prefix: Option<PathBuf>,
        /// Effective datetime (e.g. 2026, 2026-03-04)
        #[arg(long)]
        effective: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// List loaded specs grouped by repository.
    ///
    /// Examples:
    ///   lemma list
    ///   lemma list --prefix ./project
    List {
        /// Workspace directory or `.lemma` file (default: current directory)
        #[arg(long, value_name = "PATH")]
        prefix: Option<PathBuf>,
        /// Output listing as JSON
        #[arg(long)]
        json: bool,
    },
    /// Start HTTP REST API server with auto-generated typed endpoints (default: localhost:8012)
    ///
    /// SIGINT/SIGTERM stop accepting new connections, drain in-flight requests
    /// up to `--eval-timeout`, then exit 0.
    ///
    /// Routes:
    ///   GET  /{spec}              — show spec interface (data, rules, versions)
    ///   POST /{spec}              — evaluate all rules (data as JSON or form body)
    ///   GET  /{spec}/{rules}      — evaluate specific rules (comma-separated)
    ///   POST /{spec}/{rules}      — evaluate specific rules (JSON or form body)
    ///   GET  /                   — list all specs
    ///   GET  /docs               — interactive API documentation
    ///   GET  /openapi.json       — OpenAPI 3.1 specification
    ///   GET  /health             — health check
    Server {
        /// Workspace directory or `.lemma` file (default: current directory)
        #[arg(long, value_name = "PATH")]
        prefix: Option<PathBuf>,
        /// Host address to bind to
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port number to listen on
        #[arg(short, long, default_value = "8012")]
        port: u16,
        /// Watch workspace for .lemma file changes and reload automatically
        #[arg(short, long)]
        watch: bool,
        /// Enable explanation generation
        #[arg(long)]
        explain: bool,
        /// Wall-clock timeout for a single evaluation request, in second
        #[arg(long, default_value = "10", value_name = "SECONDS")]
        eval_timeout: u64,
        /// Allow cross-origin browser requests from any origin (permissive CORS).
        /// Off by default: cross-origin requests are denied.
        #[arg(long)]
        cors: bool,
    },
    /// Start Language Server Protocol server (stdio)
    Lsp {
        /// Accepted for vscode-languageclient compatibility (stdio is the only transport)
        #[arg(long, hide = true)]
        stdio: bool,
    },
    /// Start MCP server for AI assistant integration (stdio by default; `--http` for Streamable HTTP)
    Mcp {
        /// Workspace directory or `.lemma` file (default: current directory)
        #[arg(long, value_name = "PATH")]
        prefix: Option<PathBuf>,
        /// Enable write tools: add_spec, update_spec, remove_spec, clear, install (read-only by default)
        #[arg(long)]
        write: bool,
        /// Wall-clock timeout for a single request, in second
        #[arg(long, default_value = "10", value_name = "SECONDS")]
        request_timeout: u64,
        /// Serve Streamable HTTP (POST /mcp) instead of stdio
        #[arg(long)]
        http: bool,
        /// Host address to bind to (requires `--http`)
        #[arg(long, default_value = "127.0.0.1", requires = "http")]
        host: String,
        /// Port to listen on (requires `--http`; default 8013, not 8012 used by `lemma server`)
        #[arg(long, default_value = "8013", requires = "http")]
        port: u16,
        /// Allow cross-origin browser requests from any origin (requires `--http`)
        #[arg(long, requires = "http")]
        cors: bool,
    },
    /// Install repositories from LemmaBase into lemma_deps/
    Install {
        /// Repository to install (e.g. `@user/repo`)
        repository: Option<String>,
        /// Workspace directory or `.lemma` file (default: current directory)
        #[arg(long, value_name = "PATH")]
        prefix: Option<PathBuf>,
        /// Install all @... references in the workspace
        #[arg(short = 'a', long)]
        all: bool,
        /// Overwrite existing LemmaBase repositories when content has changed
        #[arg(short = 'f', long)]
        force: bool,
    },
    /// Format .lemma files to canonical style
    Format {
        /// Files or directories to format (default: current directory)
        #[arg(default_value = ".")]
        paths: Vec<PathBuf>,
        /// Check formatting without modifying files (exit 1 if any file would change)
        #[arg(long)]
        check: bool,
        /// Write formatted output to stdout instead of modifying files
        #[arg(long)]
        stdout: bool,
    },
}

/// Positional args for `lemma run`: optional `[repo]`, optional `[spec]`, then `name=value` data.
/// Workspace root is always `--prefix` (default `.`), never a positional path.
struct RunArgs {
    positionals: Vec<String>,
    data: Vec<String>,
}

fn parse_run_args(arguments: &[String]) -> Result<RunArgs> {
    let mut data = Vec::new();
    let mut positionals = Vec::new();
    for argument in arguments {
        if argument.contains('=') {
            data.push(argument.clone());
        } else {
            if argument == "-" {
                anyhow::bail!(
                    "`-` is not a valid path (stdin is not supported); use `--prefix` for workspace files"
                );
            }
            positionals.push(argument.to_string());
        }
    }
    if positionals.len() > 2 {
        anyhow::bail!(
            "Too many positional arguments; expected [repo] [spec], [spec], or [repo] with --interactive (use `--prefix` for workspace path)"
        );
    }
    for pos in &positionals {
        if Path::new(pos).exists() {
            anyhow::bail!(
                "Workspace path must be passed with --prefix {}, not as a bare positional",
                pos
            );
        }
    }
    Ok(RunArgs { positionals, data })
}

fn workspace_dir(prefix: Option<&PathBuf>) -> &Path {
    prefix
        .map(|p| p.as_path())
        .unwrap_or_else(|| Path::new("."))
}

/// Resolve spec name: explicit name wins; single-spec workspaces auto-resolve;
/// interactive mode yields empty placeholder for multi-spec; otherwise error.
fn resolve_spec(engine: &Engine, spec: Option<&str>, interactive: bool) -> Result<String> {
    if let Some(name) = spec {
        return Ok(name.to_string());
    }
    let workspace = engine
        .list()
        .into_iter()
        .find(|repository_group| repository_group.repository.is_none())
        .expect("BUG: default repository must exist after Engine::new")
        .specs;
    let unique_names: std::collections::BTreeSet<&str> =
        workspace.iter().map(|ls| ls.name.as_str()).collect();
    match unique_names.len() {
        0 => anyhow::bail!("No specs found in source"),
        1 => Ok(unique_names
            .into_iter()
            .next()
            .expect("BUG: len was 1")
            .to_string()),
        _ if interactive => Ok(String::new()),
        _ => {
            let names: Vec<&str> = unique_names.into_iter().collect();
            anyhow::bail!(
                "Workspace contains multiple specs: {}\n\nUsage: lemma run [repo] <spec> [--prefix PATH] [name=value ...]",
                names.join(", ")
            );
        }
    }
}

fn resolve_effective(cli_effective: Option<&String>) -> Result<DateTimeValue> {
    lemma::resolve_effective(cli_effective.map(String::as_str))
        .map_err(|e| anyhow::anyhow!("{}", e.message()))
}

fn main() {
    let cli = Cli::parse();

    let result: Result<()> = (|| match &cli.command {
        Commands::Run {
            args,
            prefix,
            rules,
            explain,
            interactive,
            effective,
            json,
        } => {
            let parsed_run = parse_run_args(args)?;
            let workdir = prefix.as_deref().unwrap_or_else(|| Path::new("."));
            run_command(RunOptions {
                source: workdir,
                positionals: &parsed_run.positionals,
                rules: rules.as_ref(),
                data: &parsed_run.data,
                explain: *explain,
                interactive: *interactive,
                effective: effective.as_ref(),
                json: *json,
            })
        }
        Commands::Show {
            repo,
            spec,
            prefix,
            effective,
            json,
        } => show_command(
            workspace_dir(prefix.as_ref()),
            repo.as_deref(),
            spec.as_deref(),
            effective.as_ref(),
            *json,
        ),
        Commands::List { prefix, json } => list_command(workspace_dir(prefix.as_ref()), *json),
        Commands::Server {
            prefix,
            host,
            port,
            watch,
            explain,
            eval_timeout,
            cors,
        } => server_command(
            workspace_dir(prefix.as_ref()),
            host,
            *port,
            *watch,
            *explain,
            *eval_timeout,
            *cors,
        ),
        Commands::Lsp { stdio: _ } => lsp_command(),
        Commands::Mcp {
            prefix,
            write,
            request_timeout,
            http,
            host,
            port,
            cors,
        } => mcp_command(
            workspace_dir(prefix.as_ref()),
            *write,
            *request_timeout,
            *http,
            host,
            *port,
            *cors,
        ),
        Commands::Install {
            repository,
            prefix,
            all,
            force,
        } => {
            if repository.is_some() && *all {
                anyhow::bail!("Cannot specify both a repository and --all");
            }
            if repository.is_none() && !*all {
                let mut cmd = Cli::command();
                cmd.build();
                let install_cmd = cmd
                    .find_subcommand_mut("install")
                    .expect("BUG: Cli must define install subcommand");
                let _ = install_cmd.print_help();
                std::process::exit(1);
            }
            install_command(
                workspace_dir(prefix.as_ref()),
                repository.as_deref(),
                *force,
            )
        }
        Commands::Format {
            paths,
            check,
            stdout,
        } => format_command(paths, *check, *stdout),
    })();

    if let Err(e) = result {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}

struct RunOptions<'a> {
    source: &'a Path,
    positionals: &'a [String],
    rules: Option<&'a String>,
    data: &'a [String],
    explain: bool,
    interactive: bool,
    effective: Option<&'a String>,
    json: bool,
}

fn run_command(options: RunOptions<'_>) -> Result<()> {
    let now = resolve_effective(options.effective)?;
    let mut engine = Engine::new();
    load_workspace(&mut engine, options.source)?;

    let (repository_qualifier_optional, spec_name_optional) = match options.positionals {
        [] => (None, None),
        [one] => {
            let is_repo = interactive::repository_loaded(&engine, one);
            let is_spec = engine
                .list()
                .into_iter()
                .find(|repository_group| repository_group.repository.is_none())
                .expect("BUG: default repository must exist after Engine::new")
                .specs
                .iter()
                .any(|ls| ls.name == *one);

            if is_repo && !is_spec {
                (Some(one.to_string()), None)
            } else if is_spec && !is_repo {
                (None, Some(one.to_string()))
            } else if is_repo && is_spec {
                if options.interactive {
                    anyhow::bail!(
                        "'{}' resolves to both a repository and a specification. Please specify both [repo] [spec] to disambiguate.",
                        one
                    );
                } else {
                    (None, Some(one.to_string()))
                }
            } else {
                (None, Some(one.to_string()))
            }
        }
        [repo, spec] => (Some(repo.to_string()), Some(spec.to_string())),
        _ => unreachable!("Parser ensures <= 2 positionals"),
    };

    if repository_qualifier_optional.is_some()
        && spec_name_optional.is_none()
        && !options.interactive
    {
        anyhow::bail!(
            "Repository positional requires a specification name (second argument), or use --interactive"
        );
    }

    let resolved_spec_name =
        resolve_spec(&engine, spec_name_optional.as_deref(), options.interactive)?;

    let (repository_qualifier_for_run, spec_set_identifier, rule_names, evaluation_inputs) =
        if options.interactive {
            let (interactive_spec_preset, interactive_rules_preset) =
                if resolved_spec_name.is_empty() {
                    (None, None)
                } else {
                    let preset_identifier = lemma::parse_spec_set_id(&resolved_spec_name)
                        .map_err(|e| anyhow::anyhow!("{}", e))?;
                    (
                        Some(preset_identifier),
                        options
                            .rules
                            .map(|rules_fragment| parse_rule_names(rules_fragment.as_str())),
                    )
                };

            let command_line_data: HashMap<String, String> = parse_data_strings(options.data);

            let interactive_outcome = interactive::run_interactive(
                &engine,
                interactive_spec_preset,
                interactive_rules_preset,
                &command_line_data,
                &now,
                repository_qualifier_optional.as_deref(),
            )?;

            let (
                chosen_repository_qualifier,
                chosen_specification_name,
                interactive_rules_selection,
                prompted_data,
            ) = interactive_outcome;

            println!();

            let mut merged_inputs = command_line_data;
            merged_inputs.extend(prompted_data);
            let interactive_spec_id = lemma::parse_spec_set_id(&chosen_specification_name)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            (
                chosen_repository_qualifier,
                interactive_spec_id,
                interactive_rules_selection.unwrap_or_default(),
                merged_inputs,
            )
        } else {
            let non_interactive_spec_id = lemma::parse_spec_set_id(&resolved_spec_name)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            let rule_names: Vec<String> = options
                .rules
                .map(|rules_fragment| parse_rule_names(rules_fragment.as_str()))
                .unwrap_or_default();
            let evaluation_inputs = parse_data_strings(options.data);
            (
                repository_qualifier_optional,
                non_interactive_spec_id,
                rule_names,
                evaluation_inputs,
            )
        };

    let rules = if rule_names.is_empty() {
        None
    } else {
        Some(rule_names.as_slice())
    };
    let response = engine
        .run(
            repository_qualifier_for_run.as_deref(),
            &spec_set_identifier,
            Some(&now),
            evaluation_inputs,
            rules,
            options.explain,
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let formatter = Formatter;

    if options.json {
        let json_document = if options.explain {
            serde_json::to_string_pretty(&lemma::api::Response::from(&response))
                .expect("BUG: failed to serialize response JSON")
        } else {
            formatter.serialize_response_json(&response, false)
        };
        println!("{}", json_document);
    } else {
        print!("{}", formatter.format_response(&response, options.explain));
    }

    Ok(())
}

/// Parse data value strings in "key=value" format into a HashMap
fn parse_data_strings(data: &[String]) -> HashMap<String, String> {
    data.iter()
        .filter_map(|s| {
            s.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect()
}

fn resolve_show_target(
    engine: &Engine,
    repository_qualifier: Option<&str>,
    specification_name: Option<&str>,
) -> Result<(Option<String>, String)> {
    match (repository_qualifier, specification_name) {
        (None, Some(spec)) => Ok((None, spec.to_string())),
        (Some(repo), Some(spec)) => Ok((Some(repo.to_string()), spec.to_string())),
        (Some(one), None) => {
            let is_repository = interactive::repository_loaded(engine, one);
            let is_spec = engine
                .list()
                .into_iter()
                .find(|repository_group| repository_group.repository.is_none())
                .expect("BUG: default repository must exist after Engine::new")
                .specs
                .iter()
                .any(|ls| ls.name == *one);

            if is_repository && !is_spec {
                anyhow::bail!(
                    "Repository positional requires a specification name (second argument)"
                );
            }
            Ok((None, one.to_string()))
        }
        (None, None) => {
            let chosen = resolve_spec(engine, None, false)?;
            Ok((None, chosen))
        }
    }
}

fn show_command(
    source_path: &Path,
    repository_qualifier: Option<&str>,
    specification_name: Option<&str>,
    effective: Option<&String>,
    json: bool,
) -> Result<()> {
    let now = resolve_effective(effective)?;
    let mut engine = Engine::new();
    load_workspace(&mut engine, source_path)?;

    let (repository_for_show, chosen_specification) =
        resolve_show_target(&engine, repository_qualifier, specification_name)?;
    let show = engine
        .show(
            repository_for_show.as_deref(),
            &chosen_specification,
            Some(&now),
        )
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if json {
        let json_document = serde_json::to_string_pretty(&lemma::api::Show::from(&show))
            .expect("BUG: failed to serialize show JSON");
        println!("{}", json_document);
    } else {
        print!("{show}");
    }
    Ok(())
}

fn spec_set_names_in_repository(repo: &lemma::ResolvedRepository) -> Vec<String> {
    let unique_names: std::collections::BTreeSet<String> =
        repo.specs.iter().map(|ls| ls.name.clone()).collect();
    unique_names.into_iter().collect()
}

fn repository_spec_groups(engine: &Engine) -> Vec<(Option<String>, Vec<String>)> {
    let mut groups: Vec<(Option<String>, Vec<String>)> = Vec::new();
    for resolved in engine.list() {
        let names = spec_set_names_in_repository(&resolved);
        if names.is_empty() {
            continue;
        }
        match resolved.repository.as_deref() {
            None => groups.push((None, names)),
            Some(repository) => groups.push((Some(repository.to_string()), names)),
        }
    }
    groups
}

fn list_command(source_path: &Path, json: bool) -> Result<()> {
    let mut engine = Engine::new();
    load_workspace(&mut engine, source_path)?;

    if json {
        let payload = engine.list();
        let json_document =
            serde_json::to_string_pretty(&payload).expect("BUG: failed to serialize list JSON");
        print!("{}", json_document);
        return Ok(());
    }

    let groups = repository_spec_groups(&engine);
    let formatter = Formatter;
    let view_groups: Vec<RepositorySpecGroup<'_>> = groups
        .iter()
        .map(|(repository, specs)| RepositorySpecGroup {
            repository: repository.as_deref(),
            specs: specs.as_slice(),
        })
        .collect();
    print!("{}", formatter.format_repository_spec_list(&view_groups));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn server_command(
    source: &Path,
    host: &str,
    port: u16,
    watch: bool,
    explain: bool,
    eval_timeout_secs: u64,
    cors: bool,
) -> Result<()> {
    use tokio::runtime::Runtime;
    let rt = Runtime::new()?;
    rt.block_on(async {
        let mut engine = Engine::new();
        load_workspace(&mut engine, source)?;

        let workspace = engine
            .list()
            .into_iter()
            .find(|repository_group| repository_group.repository.is_none())
            .expect("BUG: default repository must exist after Engine::new")
            .specs;
        let unique_specs: std::collections::BTreeSet<&str> =
            workspace.iter().map(|ls| ls.name.as_str()).collect();
        println!(
            "Starting HTTP server with {} spec(s) loaded...",
            unique_specs.len()
        );
        server::http::start_server(
            engine,
            host,
            port,
            watch,
            explain,
            source.to_path_buf(),
            eval_timeout_secs,
            cors,
        )
        .await
    })?;
    Ok(())
}

fn lsp_command() -> Result<()> {
    let workspace_files: Arc<dyn lemma_lsp::workspace_files::WorkspaceFiles> =
        Arc::new(lemma_cli::workspace::CliWorkspaceFiles);
    lemma_lsp::stdio::run_stdio(Some(workspace_files)).map_err(anyhow::Error::from)
}

fn mcp_command(
    workdir: &Path,
    write: bool,
    request_timeout_secs: u64,
    http: bool,
    host: &str,
    port: u16,
    cors: bool,
) -> Result<()> {
    let mut engine = Engine::new();
    load_workspace(&mut engine, workdir)?;

    let config = mcp::McpConfig {
        write,
        request_timeout: std::time::Duration::from_secs(request_timeout_secs),
    };

    let workspace_specs = engine
        .list()
        .into_iter()
        .find(|repository_group| repository_group.repository.is_none())
        .expect("BUG: default repository must exist after Engine::new")
        .specs;
    let unique_specs: std::collections::BTreeSet<&str> =
        workspace_specs.iter().map(|ls| ls.name.as_str()).collect();

    if http {
        eprintln!(
            "Starting MCP HTTP server with {} spec(s) loaded",
            unique_specs.len()
        );
        use tokio::runtime::Runtime;
        let rt = Runtime::new()?;
        rt.block_on(mcp::http::start_http_server(
            engine,
            config,
            workdir,
            mcp::http::McpHttpBind {
                host: host.to_string(),
                port,
                cors,
            },
        ))?;
    } else {
        eprintln!(
            "Starting MCP server with {} spec(s) loaded",
            unique_specs.len()
        );
        mcp::server::start_server(engine, config, workdir)?;
    }
    Ok(())
}

fn install_command(source: &Path, spec_name: Option<&str>, force: bool) -> Result<()> {
    let registries = lemma::Registries::default();
    let transport = install::ReqwestTransport::new();

    match spec_name {
        Some(id) => install_repo(source, id, &registries, &transport, force),
        None => install_workspace_deps(source, &registries, &transport, force),
    }
}

fn install_repo(
    workdir: &Path,
    raw_id: &str,
    registries: &lemma::Registries,
    transport: &install::ReqwestTransport,
    force: bool,
) -> Result<()> {
    match install::install_registry_dependency(workdir, raw_id, force, registries, transport) {
        Ok(install::InstallOutcome::AlreadyUpToDate { .. }) => {
            eprintln!("Already up to date: {}.", raw_id);
            Ok(())
        }
        Ok(install::InstallOutcome::Written { relative_path, .. }) => {
            eprintln!("  installed: {} -> {}", raw_id, relative_path.display());
            Ok(())
        }
        Err(ref e @ install::InstallError::Plan(ref load_err)) => {
            for err in load_err.iter() {
                eprintln!("{}", error_formatter::format_error(err, &load_err.sources));
            }
            Err(anyhow::anyhow!("{e}"))
        }
        Err(error) => Err(anyhow::anyhow!("{error}")),
    }
}

fn install_workspace_deps(
    workdir: &Path,
    registries: &lemma::Registries,
    transport: &install::ReqwestTransport,
    force: bool,
) -> Result<()> {
    let mut ctx = lemma::Context::new();
    let mut sources: HashMap<lemma::SourceType, String> = HashMap::new();
    let limits = lemma::ResourceLimits::default();

    let discovered = lemma_cli::workspace::discover_lemma_paths(workdir)
        .map_err(|error| anyhow::anyhow!("{error}"))?;

    for path in &discovered.workspace_paths {
        let code = fs::read_to_string(path)?;
        let source_type =
            lemma::SourceType::Path(lemma_cli::workspace::workspace_path_label(workdir, path));
        match lemma::parse(&code, source_type.clone(), &limits) {
            Ok(result) => {
                for (parsed_repo, specs) in &result.repositories {
                    let repository_arc = std::sync::Arc::clone(parsed_repo);
                    for spec in specs {
                        ctx.insert_spec(std::sync::Arc::clone(&repository_arc), spec.clone())
                            .map_err(|errors| {
                                anyhow::anyhow!(
                                    "{}",
                                    errors
                                        .iter()
                                        .map(|error| error.to_string())
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                )
                            })?;
                    }
                }
                sources.insert(source_type, code);
            }
            Err(e) => {
                sources.insert(source_type.clone(), code.clone());
                eprintln!("{}", error_formatter::format_error(&e, &sources));
                anyhow::bail!("Parse error in {}", path.display());
            }
        }
    }

    let local_workspace_sources: std::collections::HashSet<lemma::SourceType> =
        sources.keys().cloned().collect();

    if let Err(errs) = lemma::Resolve::run(registries, &mut ctx, &mut sources, &limits, transport) {
        for e in &errs {
            eprintln!("{}", error_formatter::format_error(e, &sources));
        }
        anyhow::bail!("Registry resolution failed ({} error(s))", errs.len());
    }

    let mut installed_count = 0u32;
    let mut skipped_count = 0u32;

    for (attribute, source_text) in &sources {
        if local_workspace_sources.contains(attribute) {
            continue;
        }

        let id = match attribute {
            lemma::SourceType::Dependency(id) => id.as_str(),
            other => {
                panic!("BUG: Resolve inserted non-Dependency source: {other}")
            }
        };

        match install::persist_registry_dependency(workdir, id, source_text, force) {
            Ok(install::InstallOutcome::AlreadyUpToDate { .. }) => {
                skipped_count += 1;
            }
            Ok(install::InstallOutcome::Written { relative_path, .. }) => {
                installed_count += 1;
                eprintln!("  installed: {} -> {}", id, relative_path.display());
            }
            Err(ref e @ install::InstallError::Plan(ref load_err)) => {
                for err in load_err.iter() {
                    eprintln!("{}", error_formatter::format_error(err, &load_err.sources));
                }
                return Err(anyhow::anyhow!("{e}"));
            }
            Err(error) => return Err(anyhow::anyhow!("{error}")),
        }
    }

    let plural = if installed_count == 1 {
        "repository"
    } else {
        "repositories"
    };

    if installed_count == 0 && skipped_count == 0 {
        eprintln!("No repositories found.");
    } else if installed_count == 0 {
        eprintln!("All repositories are up to date. Use --force to overwrite.");
    } else if skipped_count > 0 {
        eprintln!(
            "Installed {} {} ({} already up to date).",
            installed_count, plural, skipped_count
        );
    } else {
        eprintln!("Installed {} {}.", installed_count, plural);
    }

    Ok(())
}

/// Load specs from a workspace directory (recursive walk) or a single `.lemma` path.
/// `lemma_deps/` `.lemma` paths load as dependencies with identifiers derived from paths under
/// `lemma_deps/` (e.g. `lemma_deps/@org/repo.lemma` -> `"@org/repo"`).
///
/// Returns the loaded source batch (empty when the workspace has no `.lemma` files).
fn load_workspace(engine: &mut Engine, workdir: &std::path::Path) -> Result<()> {
    match lemma_cli::workspace::load_workspace(engine, workdir) {
        Ok(()) => Ok(()),
        Err(lemma_cli::workspace::WorkspaceDiskError::EngineLoad(load_failures)) => {
            bail_workspace_load_errors(&load_failures)
        }
        Err(error) => Err(anyhow::Error::msg(error.to_string())),
    }
}

/// Always fails after printing diagnostics.
fn bail_workspace_load_errors(load_errors: &lemma::Errors) -> anyhow::Result<()> {
    let mut emitted_message_keys = std::collections::HashSet::new();
    let unique_errors: Vec<_> = load_errors
        .iter()
        .filter(|report| emitted_message_keys.insert(report.message().to_string()))
        .collect();
    for report in &unique_errors {
        eprintln!(
            "{}",
            error_formatter::format_error(report, &load_errors.sources)
        );
    }
    anyhow::bail!("Workspace load failed ({} error(s))", unique_errors.len());
}

fn parse_rule_names(comma_separated_rules: &str) -> Vec<String> {
    comma_separated_rules
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Collect all .lemma file paths from the given paths (each may be a file or directory).
/// Collect every `.lemma` filesystem path rooted at the given files or directories.
fn collect_lemma_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for path in paths {
        if path.is_file() {
            if path.extension().and_then(|e| e.to_str()) == Some("lemma") {
                let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                if seen.insert(canonical.clone()) {
                    result.push(path.clone());
                }
            }
        } else if path.is_dir() {
            for entry_result in WalkDir::new(path) {
                let entry = match entry_result {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("warning: ignoring directory entry: {}", e);
                        continue;
                    }
                };
                let p = entry.path();
                if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("lemma") {
                    if let Ok(canonical) = p.canonicalize() {
                        if seen.insert(canonical) {
                            result.push(p.to_path_buf());
                        }
                    } else if seen.insert(p.to_path_buf()) {
                        result.push(p.to_path_buf());
                    }
                }
            }
        }
    }
    Ok(result)
}

fn format_command(paths: &[PathBuf], check: bool, stdout: bool) -> Result<()> {
    let files = collect_lemma_paths(paths)?;
    let mut any_changed = false;
    let mut parse_errors = 0u32;

    for file_path in &files {
        let source = match fs::read_to_string(file_path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading {}: {}", file_path.display(), e);
                parse_errors += 1;
                continue;
            }
        };
        let formatted = match lemma::format_source(
            &source,
            lemma::SourceType::Path(std::sync::Arc::new(file_path.clone())),
        ) {
            Ok(s) => s,
            Err(e) => {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    lemma::SourceType::Path(std::sync::Arc::new(file_path.clone())),
                    source.clone(),
                );
                eprintln!("{}", error_formatter::format_error(&e, &m));
                parse_errors += 1;
                continue;
            }
        };

        if stdout {
            print!("{}", formatted);
            continue;
        }

        if source == formatted {
            continue;
        }
        any_changed = true;

        if check {
            eprintln!("Would reformat: {}", file_path.display());
        } else if let Err(e) = fs::write(file_path, &formatted) {
            eprintln!("Error writing {}: {}", file_path.display(), e);
            parse_errors += 1;
        } else {
            eprintln!("Formatted: {}", file_path.display());
        }
    }

    if parse_errors > 0 {
        std::process::exit(1);
    }
    if check && any_changed {
        std::process::exit(1);
    }
    Ok(())
}
