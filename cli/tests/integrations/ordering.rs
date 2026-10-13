//! `lemma show --json` output must be byte-identical across separate process invocations.
//!
//! Lives here (not `engine/tests/`) because it needs `CARGO_BIN_EXE_lemma`, which Cargo only
//! injects into test binaries of the crate that owns the `[[bin]]` target — i.e. `cli`.

use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

const META_ORDER_SPEC: &str = r#"
spec ordered
meta zebra: "z"
meta yankee: "y"
meta xray: "x"
meta whiskey: "w"
meta victor: "v"
meta uniform: "u"
data n: number
rule r: n
"#;

#[test]
fn cross_process_show_bytes_identical() {
    let dir = TempDir::new().expect("temp dir");
    let spec_path: PathBuf = dir.path().join("ordered.lemma");
    std::fs::write(&spec_path, META_ORDER_SPEC).expect("write spec");

    let run = || {
        let output = Command::new(env!("CARGO_BIN_EXE_lemma"))
            .args([
                "show",
                "--json",
                "--path",
                dir.path().to_str().expect("utf8 path"),
                "ordered",
            ])
            .output()
            .unwrap_or_else(|e| panic!("spawn lemma show: {e}"));
        assert!(
            output.status.success(),
            "lemma show failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };

    let first = run();
    let second = run();
    assert_eq!(
        first,
        second,
        "cross-process Show JSON must be byte-identical;\nfirst:\n{}\nsecond:\n{}",
        String::from_utf8_lossy(&first),
        String::from_utf8_lossy(&second)
    );
}
