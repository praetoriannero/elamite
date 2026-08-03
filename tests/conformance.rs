use std::ffi::OsString;
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
        self.source_case(
            name,
            "fn main() -> ():\n    println(\"runner\")\n",
            expected_stdout,
        );
    }

    fn source_case(&self, name: &str, source: &str, expected_stdout: &str) {
        let case = self.root.join(name);
        fs::create_dir_all(case.join("src")).expect("create case");
        fs::write(
            case.join("elamite.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\ntarget_kind = \"exe\"\n"),
        )
        .expect("write manifest");
        fs::write(case.join("src/main.elx"), source).expect("write source");
        fs::write(case.join("expected.stdout"), expected_stdout).expect("write expectation");
    }

    fn target_expectation(&self, name: &str, target: &str, expected_stdout: &str) {
        fs::write(
            self.root
                .join(name)
                .join(format!("expected.{target}.stdout")),
            expected_stdout,
        )
        .expect("write target expectation");
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
        features: Default::default(),
        c_flags: Vec::new(),
        runtime_environment: Vec::new(),
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
fn target_specific_expectations_override_the_portable_fallback() {
    let suite = Suite::new();
    suite.case("width", "wrong fallback\n");
    suite.target_expectation("width", "x86_64", "runner\n");
    let report = run_suite(&suite.root, &options(None)).expect("run suite");
    assert!(report.success());
}

#[test]
fn command_line_runner_selects_a_fixture() {
    let suite = Suite::new();
    suite.case("first", "runner\n");
    suite.case("second", "runner\n");
    let output = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .arg("conformance")
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

#[test]
fn authoritative_demo_matches_in_debug_and_release() {
    let suite = Suite::new();
    suite.source_case(
        "spec_demo",
        include_str!("../examples/spec_demo.elx"),
        include_str!("../examples/spec_demo/expected.stdout"),
    );
    let report = run_suite(
        &suite.root,
        &RunnerOptions {
            filter: None,
            targets: vec![Target::X86_64],
            optimizations: vec![Optimization::Debug, Optimization::Release],
            features: Default::default(),
            c_flags: Vec::new(),
            runtime_environment: Vec::new(),
        },
    )
    .expect("run authoritative demo");
    assert!(
        report.success(),
        "{}",
        report
            .cases
            .iter()
            .map(|case| format!("{:?}: {}", case.optimization, case.detail))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn generated_c_is_clean_under_address_and_undefined_behavior_sanitizers() {
    let suite = Suite::new();
    suite.source_case(
        "sanitizers",
        r#"fn main() -> ():
    var values = [1, 2, 3, 4]
    let whole: *var [i32; 4] = (&var values) as *var [i32; 4]
    unsafe:
        let first: *var i32 = whole as *var i32
        first[1] = 5
        println(first[1])
        println(*(first + 2))
        println((first + 4) - first)
        let end = first + 4
        var cursor = first
        var sum = 0
        while cursor < end:
            sum += *cursor
            cursor += 1
        println(sum)
"#,
        "5\n3\n4\n13\n",
    );
    let report = run_suite(
        &suite.root,
        &RunnerOptions {
            filter: None,
            targets: vec![Target::X86_64],
            optimizations: vec![Optimization::Debug],
            features: Default::default(),
            c_flags: vec![
                OsString::from("-fsanitize=address,undefined"),
                OsString::from("-fno-omit-frame-pointer"),
            ],
            runtime_environment: vec![(
                OsString::from("ASAN_OPTIONS"),
                OsString::from("detect_leaks=0"),
            )],
        },
    )
    .expect("run instrumented fixture");
    assert!(
        report.success(),
        "{}",
        report
            .cases
            .iter()
            .map(|case| case.detail.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn concurrency_contract_matches_the_available_target_and_optimization_matrix() {
    let mut targets = vec![Target::X86_64];
    if x86_runtime_available() {
        targets.push(Target::X86);
    } else {
        eprintln!(
            "skipping x86 concurrency execution: the host cannot build and run a 32-bit C/GC/thread probe"
        );
    }
    let report = run_suite(
        Path::new("tests/fixtures/conformance/14_concurrency"),
        &RunnerOptions {
            filter: None,
            targets,
            optimizations: vec![Optimization::Debug, Optimization::Release],
            features: Default::default(),
            c_flags: Vec::new(),
            runtime_environment: Vec::new(),
        },
    )
    .expect("run concurrency contract fixture");
    assert!(
        report.success(),
        "{}",
        report
            .cases
            .iter()
            .map(|case| case.detail.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn concurrency_generated_c_is_clean_under_address_and_undefined_sanitizers() {
    let report = run_suite(
        Path::new("tests/fixtures/conformance"),
        &RunnerOptions {
            filter: Some("concurrency".to_string()),
            targets: vec![Target::X86_64],
            optimizations: vec![Optimization::Debug],
            features: Default::default(),
            c_flags: vec![
                OsString::from("-fsanitize=address,undefined"),
                OsString::from("-fno-omit-frame-pointer"),
            ],
            runtime_environment: vec![(
                OsString::from("ASAN_OPTIONS"),
                OsString::from("detect_leaks=0:halt_on_error=1"),
            )],
        },
    )
    .expect("run instrumented concurrency fixtures");
    assert!(
        report.success(),
        "{}",
        report
            .cases
            .iter()
            .map(|case| case.detail.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn concurrency_stress_is_clean_under_thread_sanitizer() {
    let report = run_suite(
        Path::new("tests/fixtures/conformance/15_concurrency_stress"),
        &RunnerOptions {
            filter: None,
            targets: vec![Target::X86_64],
            optimizations: vec![Optimization::Debug],
            features: Default::default(),
            c_flags: vec![
                OsString::from("-fsanitize=thread"),
                OsString::from("-fno-omit-frame-pointer"),
            ],
            runtime_environment: vec![(
                OsString::from("TSAN_OPTIONS"),
                // Boehm stops collector threads with signals. TSan reports
                // the collector mechanism itself unless this unrelated
                // diagnostic is disabled; data-race reports remain fatal.
                OsString::from("halt_on_error=1:report_signal_unsafe=0"),
            )],
        },
    )
    .expect("run thread-sanitized concurrency fixture");
    assert!(
        report.success(),
        "{}",
        report
            .cases
            .iter()
            .map(|case| case.detail.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn runtime_stress_is_stable_across_repeated_debug_and_release_runs() {
    for _ in 0..3 {
        let report = run_suite(
            Path::new("tests/fixtures/conformance/12_runtime_stress"),
            &RunnerOptions {
                filter: None,
                targets: vec![Target::X86_64],
                optimizations: vec![Optimization::Debug, Optimization::Release],
                features: Default::default(),
                c_flags: Vec::new(),
                runtime_environment: Vec::new(),
            },
        )
        .expect("run stress fixture");
        assert!(
            report.success(),
            "{}",
            report
                .cases
                .iter()
                .map(|case| case.detail.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

#[test]
fn concurrency_stress_is_stable_across_repeated_debug_and_release_runs() {
    for _ in 0..3 {
        let report = run_suite(
            Path::new("tests/fixtures/conformance/15_concurrency_stress"),
            &RunnerOptions {
                filter: None,
                targets: vec![Target::X86_64],
                optimizations: vec![Optimization::Debug, Optimization::Release],
                features: Default::default(),
                c_flags: Vec::new(),
                runtime_environment: Vec::new(),
            },
        )
        .expect("run concurrency stress fixture");
        assert!(
            report.success(),
            "{}",
            report
                .cases
                .iter()
                .map(|case| case.detail.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

fn x86_runtime_available() -> bool {
    let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let source = std::env::temp_dir().join(format!(
        "elamite-x86-conformance-{}-{serial}.c",
        std::process::id()
    ));
    let executable = source.with_extension("bin");
    if fs::write(
        &source,
        "#define GC_THREADS 1\n\
         #include <stdint.h>\n\
         #include <pthread.h>\n\
         #include <gc.h>\n\
         static void *worker(void *unused) { (void)unused; return GC_MALLOC(1U); }\n\
         int main(void) { pthread_t thread; GC_INIT(); GC_allow_register_threads(); \
         if (pthread_create(&thread, NULL, worker, NULL) != 0) return 1; \
         return pthread_join(thread, NULL) == 0 ? 0 : 1; }\n",
    )
    .is_err()
    {
        return false;
    }
    let compiled = Command::new("cc")
        .args(["-m32", "-std=c99"])
        .arg(&source)
        .args(["-lgc", "-lpthread", "-o"])
        .arg(&executable)
        .output()
        .is_ok_and(|output| output.status.success());
    let available = compiled
        && Command::new(&executable)
            .output()
            .is_ok_and(|output| output.status.success());
    let _ = fs::remove_file(source);
    let _ = fs::remove_file(executable);
    available
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
