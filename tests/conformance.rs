use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::backend::Target;
use elamite::conformance::{RunnerOptions, run_suite};
use elamite::driver::Optimization;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct Suite {
    root: PathBuf,
}

impl Suite {
    fn new() -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "elamite-runner-test-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create suite");
        Self { root }
    }

    fn case(&self, name: &str, expected_stdout: &str) {
        let case = self.root.join(name);
        fs::create_dir_all(case.join("src")).expect("create case");
        fs::write(
            case.join("elamite.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\ntarget_kind = \"exe\"\n"),
        )
        .expect("write manifest");
        fs::write(
            case.join("src/main.elx"),
            "fn main() -> ():\n    println(\"runner\")\n",
        )
        .expect("write source");
        fs::write(case.join("expected.stdout"), expected_stdout).expect("write expectation");
    }
}

impl Drop for Suite {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn options(filter: Option<&str>) -> RunnerOptions {
    RunnerOptions {
        filter: filter.map(str::to_string),
        targets: vec![Target::X86_64],
        optimizations: vec![Optimization::Debug],
    }
}

#[test]
fn selects_one_fixture_and_removes_successful_artifacts() {
    let suite = Suite::new();
    suite.case("first", "runner\n");
    suite.case("second", "runner\n");
    let report = run_suite(&suite.root, &options(Some("second"))).expect("run suite");
    assert!(report.success());
    assert_eq!(report.cases.len(), 1);
    assert_eq!(report.cases[0].name, "second");
    assert!(report.retained_artifacts.is_none());
}

#[test]
fn retains_generated_artifacts_after_a_failure() {
    let suite = Suite::new();
    suite.case("mismatch", "different\n");
    let report = run_suite(&suite.root, &options(None)).expect("run suite");
    assert!(!report.success());
    let retained = report
        .retained_artifacts
        .expect("failure retains artifacts");
    assert!(retained.is_dir());
    assert!(contains_file_named(&retained, "mismatch.c"));
    fs::remove_dir_all(retained).expect("remove retained test artifacts");
}

#[test]
fn command_line_runner_selects_a_fixture() {
    let suite = Suite::new();
    suite.case("first", "runner\n");
    suite.case("second", "runner\n");
    let output = Command::new(env!("CARGO_BIN_EXE_elamite"))
        .arg("test")
        .arg(&suite.root)
        .arg("--filter=first")
        .output()
        .expect("run conformance command");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pass first"), "{stdout}");
    assert!(!stdout.contains("second"), "{stdout}");
}

fn contains_file_named(directory: &Path, name: &str) -> bool {
    fs::read_dir(directory)
        .expect("read retained directory")
        .filter_map(Result::ok)
        .any(|entry| {
            let path = entry.path();
            if path.is_dir() {
                contains_file_named(&path, name)
            } else {
                path.file_name().and_then(|file| file.to_str()) == Some(name)
            }
        })
}
