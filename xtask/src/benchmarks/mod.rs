//! Workspace benchmark orchestration.
//!
//! ## How to run
//!
//! From the workspace root:
//!
//! ```text
//! cargo benchmarks engine      # engine evaluation + Python comparison
//! cargo benchmarks logistics   # logistics ladder (embedded + external catalog)
//! cargo benchmarks cli         # HTTP evaluate + engine profile
//! cargo benchmarks all         # all suites, sequentially
//! ```
//!
//! Writes reports under `cli/documentation/reference/benchmarks/`. Any subprocess failure
//! aborts the run; no partial reports.

mod cli;
pub mod common;
mod engine;
mod logistics;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suite {
    Engine,
    Logistics,
    Cli,
    All,
}

pub fn parse_suite(args: &[String]) -> Result<Suite, String> {
    match args {
        [] => Err(
            "missing suite: expected one of engine, logistics, cli, all (e.g. cargo benchmarks engine)"
                .into(),
        ),
        [suite] => match suite.as_str() {
            "engine" => Ok(Suite::Engine),
            "logistics" => Ok(Suite::Logistics),
            "cli" => Ok(Suite::Cli),
            "all" => Ok(Suite::All),
            other => Err(format!(
                "unknown suite '{other}': expected one of engine, logistics, cli, all"
            )),
        },
        _ => Err(
            "too many arguments: expected exactly one suite (engine, logistics, cli, or all)".into(),
        ),
    }
}

pub fn run(root: &std::path::Path, args: &[String]) -> Result<(), String> {
    let suite = parse_suite(args)?;
    match suite {
        Suite::Engine => engine::run(root).map_err(|e| format!("engine: {e}")),
        Suite::Logistics => logistics::run(root).map_err(|e| format!("logistics: {e}")),
        Suite::Cli => cli::run(root).map_err(|e| format!("cli: {e}")),
        Suite::All => {
            engine::run(root).map_err(|e| format!("engine: {e}"))?;
            logistics::run(root).map_err(|e| format!("logistics: {e}"))?;
            cli::run(root).map_err(|e| format!("cli: {e}"))?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_suite_requires_argument() {
        assert!(parse_suite(&[]).is_err());
    }

    #[test]
    fn parse_suite_accepts_known_suites() {
        assert_eq!(parse_suite(&["engine".into()]).unwrap(), Suite::Engine);
        assert_eq!(
            parse_suite(&["logistics".into()]).unwrap(),
            Suite::Logistics
        );
        assert_eq!(parse_suite(&["cli".into()]).unwrap(), Suite::Cli);
        assert_eq!(parse_suite(&["all".into()]).unwrap(), Suite::All);
    }

    #[test]
    fn parse_suite_rejects_unknown_and_extra_args() {
        assert!(parse_suite(&["wasm".into()]).is_err());
        assert!(parse_suite(&["engine".into(), "cli".into()]).is_err());
    }
}
