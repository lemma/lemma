#![recursion_limit = "512"]

mod benchmarks;
mod coverage;
mod hex_standalone;
mod llms;
mod lsp;
mod maven_natives;
mod nuget_natives;
mod recursion;
mod schema;
mod todo_check;
mod versions;
mod versions_diff;
mod warnings;

use std::process::Command;

fn cargo_bin() -> String {
    std::env::var("CARGO").expect("BUG: xtask must run through cargo, which sets CARGO")
}

fn exit_for_child_status(status: std::process::ExitStatus, command: &str) -> ! {
    match status.code() {
        Some(code) => std::process::exit(code),
        None => panic!("{command} terminated by signal: {status}"),
    }
}

fn probe_succeeded(result: std::io::Result<std::process::ExitStatus>, tool: &str) -> bool {
    match result {
        Ok(status) => status.success(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => panic!("failed to run {tool}: {error}"),
    }
}

fn run(args: &[&str]) {
    let status = Command::new(cargo_bin())
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("failed to run {} {:?}: {e}", cargo_bin(), args));
    if !status.success() {
        exit_for_child_status(status, &format!("{} {args:?}", cargo_bin()));
    }
}

fn run_publishable_package_gate() {
    // --list checks the package file set without resolving path+version pins against
    // crates.io. Full package/verify needs the registry; release bottom-up publish owns that.
    for crate_name in ["lemma-engine", "lemma-lsp", "lemma-openapi", "lemma"] {
        let args = ["package", "-p", crate_name, "--list", "--allow-dirty"];
        let output = Command::new(cargo_bin())
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("failed to run {} {args:?}: {e}", cargo_bin()));
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !output.status.success() {
            print!("{combined}");
            exit_for_child_status(output.status, &format!("{} {args:?}", cargo_bin()));
        }
        if let Err(error) =
            warnings::reject_warnings_in_output(&format!("cargo {args:?}"), &combined)
        {
            print!("{combined}");
            eprintln!("{error}");
            std::process::exit(1);
        }
        let file_count = output.stdout.iter().filter(|&&b| b == b'\n').count();
        eprintln!("package --list {crate_name}: ok ({file_count} files)");
    }
}

fn run_versions_verify() {
    let root = versions::workspace_root();
    if let Err(e) = versions::versions_verify(&root) {
        eprintln!("versions-verify: failed:\n{e}");
        std::process::exit(1);
    }
    eprintln!(
        "versions-verify: ok ({})",
        versions::read_workspace_version(&root)
            .expect("BUG: workspace version readable after versions_verify passed")
    );
}

const HEX_PACKAGE_DIR: &str = "engine/packages/hex";
const NPM_WASM_DIR: &str = "engine/packages/npm";
const MAVEN_PACKAGE_DIR: &str = "engine/packages/maven";
const NUGET_PACKAGE_DIR: &str = "engine/packages/nuget";
const FUZZ_PACKAGE_DIR: &str = "engine/fuzz";
const FUZZ_TARGETS: &[&str] = &[
    "fuzz_parser",
    "fuzz_expressions",
    "fuzz_literals",
    "fuzz_deeply_nested",
    "fuzz_data_bindings",
];
/// Wall-clock budget for `--fuzz`, split evenly across [`FUZZ_TARGETS`].
const FUZZ_BUDGET_SECS: u64 = 300;

fn require_command(name: &str, install_hint: &str) {
    let ok = probe_succeeded(
        Command::new(name)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status(),
        name,
    );
    if !ok {
        panic!("{name} not found on PATH. {install_hint}");
    }
}

fn require_wasm_pack() {
    let output = Command::new("wasm-pack")
        .arg("--version")
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "wasm-pack not found on PATH. Install: cargo install wasm-pack --version {} --locked ({e})",
                versions::WASM_PACK_VERSION
            )
        });
    if !output.status.success() {
        panic!(
            "wasm-pack --version failed. Install: cargo install wasm-pack --version {} --locked",
            versions::WASM_PACK_VERSION
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .next()
        .expect("wasm-pack --version printed no output")
        .trim();
    let expected = format!("wasm-pack {}", versions::WASM_PACK_VERSION);
    if line != expected {
        panic!(
            "wasm-pack version mismatch: got {line:?}, expected {expected:?}. Install: cargo install wasm-pack --version {} --locked",
            versions::WASM_PACK_VERSION
        );
    }
}

fn require_nightly() {
    let status = Command::new("rustup")
        .args(["run", "nightly", "rustc", "--version"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap_or_else(|e| panic!("failed to invoke rustup for nightly: {e}"));
    if !status.success() {
        panic!("Rust nightly toolchain required for --fuzz. Install with: rustup install nightly");
    }
}

fn run_npm_wasm_precommit() {
    require_command("node", "Install Node.js (https://nodejs.org/).");
    require_wasm_pack();
    let root = versions::workspace_root();
    let npm_dir = root.join(NPM_WASM_DIR);
    for script in ["build.js", "test.js"] {
        eprintln!("xtask: npm wasm (node {script})");
        let status = Command::new("node")
            .arg(script)
            .current_dir(&npm_dir)
            .status()
            .unwrap_or_else(|e| {
                panic!("failed to run node {script} in {}: {e}", npm_dir.display())
            });
        if !status.success() {
            exit_for_child_status(status, &format!("node {script}"));
        }
    }
    require_command("npm", "Install Node.js (https://nodejs.org/).");
    for args in [&["ci"][..], &["run", "typecheck:fixtures"][..]] {
        eprintln!("xtask: npm wasm (npm {})", args.join(" "));
        if let Err(e) = lsp::run_npm(&npm_dir, args) {
            eprintln!("npm wasm: {e}");
            std::process::exit(1);
        }
    }
}

fn run_mix_precommit() {
    require_command(
        "mix",
        "Install Elixir and Mix (https://elixir-lang.org/install.html).",
    );
    let hex_dir = versions::workspace_root().join(HEX_PACKAGE_DIR);
    let status = Command::new("mix")
        .current_dir(&hex_dir)
        .arg("precommit")
        .status()
        .unwrap_or_else(|e| panic!("failed to run mix precommit in {}: {e}", hex_dir.display()));
    if !status.success() {
        exit_for_child_status(status, "mix precommit");
    }
}

fn run_vscode_precommit() {
    require_command("npm", "Install Node.js (https://nodejs.org/).");
    require_command("npx", "Install Node.js (https://nodejs.org/).");
    let dir = versions::workspace_root().join(lsp::VSCODE_EXTENSION_REL);
    if let Err(e) = lsp::ci_compile_package(&dir) {
        eprintln!("vscode package: {e}");
        std::process::exit(1);
    }
}

fn run_maven_precommit() {
    require_command(
        "java",
        "Install a JDK 21+ (https://adoptium.net/) for the Maven package tests.",
    );
    eprintln!("xtask: cargo build --release -p lemma_jni");
    run(&["build", "--release", "-p", "lemma_jni"]);
    let maven_dir = versions::workspace_root().join(MAVEN_PACKAGE_DIR);
    let mvnw = maven_dir.join("mvnw");
    eprintln!("xtask: maven ./mvnw -B verify");
    let output = Command::new(&mvnw)
        .args(["-B", "verify"])
        .current_dir(&maven_dir)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "failed to run {} in {}: {e}",
                mvnw.display(),
                maven_dir.display()
            )
        });
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{combined}");
    let label = "./mvnw -B verify";
    if !output.status.success() {
        exit_for_child_status(output.status, label);
    }
    if let Err(e) = warnings::reject_warnings_in_output(label, &combined) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn require_uniffi_bindgen_cs() {
    let output = Command::new("uniffi-bindgen-cs")
        .arg("--version")
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "uniffi-bindgen-cs not found on PATH. Install: cargo install uniffi-bindgen-cs --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag {} ({e})",
                nuget_natives::UNIFFI_BINDGEN_CS_TAG
            )
        });
    if !output.status.success() {
        panic!(
            "uniffi-bindgen-cs --version failed. Install: cargo install uniffi-bindgen-cs --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag {}",
            nuget_natives::UNIFFI_BINDGEN_CS_TAG
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next().unwrap_or("").trim();
    if !line.contains("0.11.0") {
        panic!(
            "uniffi-bindgen-cs version mismatch: got {line:?}, expected tag {}. Install: cargo install uniffi-bindgen-cs --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag {}",
            nuget_natives::UNIFFI_BINDGEN_CS_TAG,
            nuget_natives::UNIFFI_BINDGEN_CS_TAG
        );
    }
}

fn run_nuget_precommit() {
    require_command(
        "dotnet",
        "Install the .NET 8 SDK (https://dotnet.microsoft.com/download) for the NuGet package tests.",
    );
    require_uniffi_bindgen_cs();
    let root = versions::workspace_root();
    if let Err(e) = nuget_natives::run(&root) {
        eprintln!("nuget-natives: {e}");
        std::process::exit(1);
    }
    let nuget_dir = root.join(NUGET_PACKAGE_DIR);
    eprintln!("xtask: dotnet test Lemmabase.Lemma.Engine.sln");
    let status = Command::new("dotnet")
        .args([
            "test",
            "Lemmabase.Lemma.Engine.sln",
            "--nologo",
            "--verbosity",
            "minimal",
        ])
        .current_dir(&nuget_dir)
        .status()
        .unwrap_or_else(|e| panic!("failed to run dotnet test in {}: {e}", nuget_dir.display()));
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn run_deny_precommit() {
    let config = versions::workspace_root().join(".cargo/deny.toml");
    eprintln!("xtask: deny ({})", config.display());
    let status = Command::new(cargo_bin())
        .args(["deny", "check", "--config"])
        .arg(&config)
        .status()
        .unwrap_or_else(|e| panic!("failed to run cargo deny: {e}"));
    if !status.success() {
        exit_for_child_status(status, "cargo deny check");
    }
}

fn run_fuzz_precommit() {
    require_command(
        "rustup",
        "Install rustup (https://rustup.rs/) for the nightly toolchain used by --fuzz.",
    );
    require_nightly();
    let fuzz_ok = probe_succeeded(
        Command::new(cargo_bin())
            .args(["fuzz", "--version"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status(),
        "cargo fuzz",
    );
    if !fuzz_ok {
        panic!("cargo fuzz not available. Install cargo-fuzz: cargo install cargo-fuzz");
    }

    let per_target_secs = FUZZ_BUDGET_SECS / FUZZ_TARGETS.len() as u64;
    if per_target_secs == 0 {
        panic!(
            "BUG: FUZZ_BUDGET_SECS ({FUZZ_BUDGET_SECS}) smaller than target count ({})",
            FUZZ_TARGETS.len()
        );
    }
    let max_total_time_arg = format!("-max_total_time={per_target_secs}");
    let fuzz_dir = versions::workspace_root().join(FUZZ_PACKAGE_DIR);
    eprintln!(
        "xtask: fuzz budget {FUZZ_BUDGET_SECS}s across {} targets ({per_target_secs}s each)",
        FUZZ_TARGETS.len()
    );
    for target in FUZZ_TARGETS {
        eprintln!("xtask: fuzz {target} ({max_total_time_arg})");
        let status = Command::new("rustup")
            .args([
                "run",
                "nightly",
                "cargo",
                "fuzz",
                "run",
                target,
                "--",
                max_total_time_arg.as_str(),
            ])
            .current_dir(&fuzz_dir)
            .status()
            .unwrap_or_else(|e| {
                panic!(
                    "failed to run rustup run nightly cargo fuzz run {target} in {}: {e}",
                    fuzz_dir.display()
                )
            });
        if !status.success() {
            exit_for_child_status(status, &format!("cargo fuzz run {target}"));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PrecommitFlags {
    run_coverage: bool,
    run_fuzz: bool,
}

fn parse_precommit_flags(args: impl Iterator<Item = String>) -> Result<PrecommitFlags, String> {
    let mut flags = PrecommitFlags {
        run_coverage: false,
        run_fuzz: false,
    };
    for arg in args {
        match arg.as_str() {
            "--coverage" => flags.run_coverage = true,
            "--fuzz" => flags.run_fuzz = true,
            other => return Err(format!("unknown precommit flag {other:?}")),
        }
    }
    Ok(flags)
}

fn precommit_from_args(args: impl Iterator<Item = String>) {
    match parse_precommit_flags(args) {
        Ok(flags) => precommit(flags),
        Err(message) => {
            eprintln!("xtask: {message}");
            usage();
            std::process::exit(1);
        }
    }
}

fn precommit(flags: PrecommitFlags) {
    eprintln!("xtask: fmt --check");
    run(&["fmt", "--all", "--", "--check"]);
    eprintln!("xtask: todo-check");
    if let Err(e) = todo_check::run(&versions::workspace_root()) {
        eprintln!("{e}");
        std::process::exit(1);
    }
    eprintln!("xtask: recursion-check");
    if let Err(e) = recursion::run(&versions::workspace_root()) {
        eprintln!("{e}");
        std::process::exit(1);
    }
    eprintln!("xtask: clippy");
    run(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--all-features",
        "--",
        "-D",
        "warnings",
    ]);
    eprintln!("xtask: clippy wasm32");
    let installed_targets = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .unwrap_or_else(|e| panic!("failed to run rustup target list: {e}"));
    let wasm_target_ok = String::from_utf8_lossy(&installed_targets.stdout)
        .lines()
        .any(|line| line.trim() == "wasm32-unknown-unknown");
    if !wasm_target_ok {
        panic!(
            "wasm32-unknown-unknown target not installed. Install with: rustup target add wasm32-unknown-unknown"
        );
    }
    run(&[
        "clippy",
        "--target",
        "wasm32-unknown-unknown",
        "-p",
        "lemma-lsp",
        "-p",
        "lemma-engine",
        "--",
        "-D",
        "warnings",
    ]);
    eprintln!("xtask: versions-verify");
    run_versions_verify();
    eprintln!("xtask: mix precommit");
    run_mix_precommit();
    eprintln!("xtask: vscode ci + compile + package");
    run_vscode_precommit();
    eprintln!("xtask: nextest");
    run(&[
        "nextest",
        "run",
        "--workspace",
        "--all-features",
        "--run-ignored",
        "all",
    ]);
    eprintln!("xtask: cargo package --list (publishable crates)");
    run_publishable_package_gate();
    eprintln!("xtask: npm wasm package");
    run_npm_wasm_precommit();
    eprintln!("xtask: maven package");
    run_maven_precommit();
    eprintln!("xtask: nuget package");
    run_nuget_precommit();
    run_deny_precommit();
    if flags.run_coverage {
        eprintln!("xtask: coverage --check");
        let root = versions::workspace_root();
        if let Err(e) = coverage::run(&root, &[String::from("all"), String::from("--check")]) {
            eprintln!("coverage: {e}");
            std::process::exit(1);
        }
    }
    if flags.run_fuzz {
        eprintln!("xtask: fuzz (5 minute budget across targets)");
        run_fuzz_precommit();
    }
    eprintln!("xtask: done");
}

fn usage() {
    eprintln!(
        "usage:\n  cargo precommit [--coverage] [--fuzz] | cargo run -p xtask -- [precommit] [--coverage] [--fuzz]\n  cargo verify   | cargo run -p xtask -- versions-verify\n  cargo bump <version> | cargo run -p xtask -- versions-bump <version>\n  cargo changelog | cargo run -p xtask -- versions-diff [semver]\n  cargo lsp | cargo run -p xtask -- lsp [vsix|prepare|package|publish-marketplace|publish-openvsx|--help]\n  cargo run -p xtask -- hex-standalone\n  cargo benchmarks <engine|logistics|cli|all> | cargo run -p xtask -- benchmarks <engine|logistics|cli|all>\n  cargo coverage <engine|cli|all> [--check] | cargo run -p xtask -- coverage <engine|cli|all> [--check]\n  cargo run -p xtask -- schema [--check]\n  cargo run -p xtask -- llms\n  cargo run -p xtask -- maven-natives\n  cargo run -p xtask -- nuget-natives\n  cargo run -p xtask -- recursion-check\n  cargo run -p xtask -- todo-check\n\n  --coverage  after the gate, run coverage all --check (CI uses this)\n  --fuzz      after the gate, run engine/fuzz for 5 minutes total (split across targets; CI push uses this)"
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let sub = args.next();
    match sub.as_deref() {
        None => precommit_from_args(args),
        Some("precommit") => precommit_from_args(args),
        Some(flag @ ("--coverage" | "--fuzz")) => {
            let mut rest = vec![flag.to_string()];
            rest.extend(args);
            precommit_from_args(rest.into_iter());
        }
        Some("versions-verify") => {
            run_versions_verify();
        }
        Some("versions-bump") => {
            let Some(new_v) = args.next() else {
                eprintln!("versions-bump: missing <version>");
                usage();
                std::process::exit(1);
            };
            if args.next().is_some() {
                eprintln!("versions-bump: too many arguments");
                usage();
                std::process::exit(1);
            }
            let root = versions::workspace_root();
            if let Err(e) = versions::versions_bump(&root, &new_v) {
                eprintln!("versions-bump: {e}");
                std::process::exit(1);
            }
            eprintln!("versions-bump: set to {new_v}");
        }
        Some("hex-standalone") => {
            if args.next().is_some() {
                eprintln!("hex-standalone: takes no arguments");
                usage();
                std::process::exit(1);
            }
            let root = versions::workspace_root();
            if let Err(e) = hex_standalone::run(&root) {
                eprintln!("hex-standalone: {e}");
                std::process::exit(1);
            }
        }
        Some("versions-diff") => {
            let ver = args.next();
            if args.next().is_some() {
                eprintln!("versions-diff: too many arguments");
                usage();
                std::process::exit(1);
            }
            let root = versions::workspace_root();
            if let Err(e) = versions_diff::run_versions_diff(&root, ver.as_deref()) {
                eprintln!("versions-diff: {e}");
                std::process::exit(1);
            }
        }
        Some("lsp") => {
            let root = versions::workspace_root();
            let rest: Vec<String> = args.collect();
            if let Err(e) = lsp::run(&root, &rest) {
                eprintln!("lsp: {e}");
                std::process::exit(1);
            }
        }
        Some("benchmarks") => {
            let rest: Vec<String> = args.collect();
            let root = versions::workspace_root();
            if let Err(e) = benchmarks::run(&root, &rest) {
                eprintln!("benchmarks: {e}");
                usage();
                std::process::exit(1);
            }
        }
        Some("coverage") => {
            let rest: Vec<String> = args.collect();
            let root = versions::workspace_root();
            if let Err(e) = coverage::run(&root, &rest) {
                eprintln!("coverage: {e}");
                usage();
                std::process::exit(1);
            }
        }
        Some("schema") => {
            let check = match args.next().as_deref() {
                None => false,
                Some("--check") => true,
                Some(other) => {
                    eprintln!("schema: unknown argument '{other}'");
                    usage();
                    std::process::exit(1);
                }
            };
            if args.next().is_some() {
                eprintln!("schema: takes at most one argument (--check)");
                usage();
                std::process::exit(1);
            }
            let root = versions::workspace_root();
            let result = if check {
                schema::check(&root)
            } else {
                schema::run(&root)
            };
            if let Err(e) = result {
                eprintln!("schema: {e}");
                std::process::exit(1);
            }
        }
        Some("llms") => {
            if args.next().is_some() {
                eprintln!("llms: takes no arguments");
                usage();
                std::process::exit(1);
            }
            let root = versions::workspace_root();
            if let Err(e) = llms::run(&root) {
                eprintln!("llms: {e}");
                std::process::exit(1);
            }
        }
        Some("maven-natives") => {
            if args.next().is_some() {
                eprintln!("maven-natives: takes no arguments");
                usage();
                std::process::exit(1);
            }
            let root = versions::workspace_root();
            if let Err(e) = maven_natives::run(&root) {
                eprintln!("maven-natives: {e}");
                std::process::exit(1);
            }
        }
        Some("nuget-natives") => {
            if args.next().is_some() {
                eprintln!("nuget-natives: takes no arguments");
                usage();
                std::process::exit(1);
            }
            let root = versions::workspace_root();
            if let Err(e) = nuget_natives::run(&root) {
                eprintln!("nuget-natives: {e}");
                std::process::exit(1);
            }
        }
        Some("recursion-check") => {
            if args.next().is_some() {
                eprintln!("recursion-check: takes no arguments");
                usage();
                std::process::exit(1);
            }
            let root = versions::workspace_root();
            if let Err(e) = recursion::run(&root) {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        Some("todo-check") => {
            if args.next().is_some() {
                eprintln!("todo-check: takes no arguments");
                usage();
                std::process::exit(1);
            }
            let root = versions::workspace_root();
            if let Err(e) = todo_check::run(&root) {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        Some("-h" | "--help" | "help") => {
            usage();
        }
        Some(other) => {
            eprintln!("xtask: unknown subcommand {other:?}");
            usage();
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_precommit_flags, PrecommitFlags};

    fn flags(args: &[&str]) -> PrecommitFlags {
        parse_precommit_flags(args.iter().copied().map(str::to_string)).expect("valid flags")
    }

    #[test]
    fn bare_precommit_skips_coverage_and_fuzz() {
        let parsed = flags(&[]);
        assert!(!parsed.run_coverage);
        assert!(!parsed.run_fuzz);
    }

    #[test]
    fn coverage_and_fuzz_flags_combine_in_either_order() {
        let coverage_first = flags(&["--coverage", "--fuzz"]);
        let fuzz_first = flags(&["--fuzz", "--coverage"]);
        assert!(coverage_first.run_coverage && coverage_first.run_fuzz);
        assert_eq!(coverage_first, fuzz_first);
    }

    #[test]
    fn unknown_precommit_flag_is_rejected() {
        let err = parse_precommit_flags(["--nope".to_string()].into_iter()).unwrap_err();
        assert!(err.contains("--nope"), "{err}");
    }
}
