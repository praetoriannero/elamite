use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use elamite::backend::Target;
use elamite::driver::compile;
use elamite::lexer::lex;
use elamite::package::PackageGraph;
use elamite::parser::{SyntaxKind, parse};
use elamite::source::SourceManager;

fn enum_variants(source: &str, name: &str) -> Vec<String> {
    let declaration = format!("pub enum {name}:");
    let mut lines = source
        .lines()
        .skip_while(|line| line.trim() != declaration)
        .skip(1);
    let mut variants = Vec::new();
    for line in &mut lines {
        if !line.starts_with("    ") || line.trim().is_empty() {
            break;
        }
        variants.push(line.trim().to_string());
    }
    assert!(!variants.is_empty(), "missing or empty enum `{name}`");
    variants
}

fn parse_standard_source(name: &str, source: &str) -> elamite::parser::ParseOutput {
    let mut sources = SourceManager::new();
    let file = sources.add_text(PathBuf::from(name), source.to_string());
    let lexed = lex(file, source);
    assert!(
        lexed.diagnostics.is_empty(),
        "{name}: {:?}",
        lexed.diagnostics
    );
    let output = parse(&lexed.tokens);
    assert!(
        output.diagnostics.is_empty(),
        "{name}: {:?}",
        output.diagnostics
    );
    output
}

static NEXT_PACKAGE: AtomicUsize = AtomicUsize::new(0);

struct TestPackage {
    root: PathBuf,
    owned_runtime_path: PathBuf,
}

impl TestPackage {
    fn system_runtime() -> Self {
        let serial = NEXT_PACKAGE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "elamite-standard-system-package-{}-{serial}",
            std::process::id()
        ));
        let owned_runtime_path = std::env::temp_dir().join(format!(
            "elamite-standard-system-runtime-{}-{serial}",
            std::process::id()
        ));
        assert!(!owned_runtime_path.exists(), "runtime path already exists");
        std::fs::create_dir_all(root.join("src")).expect("create test package");
        std::fs::write(
            root.join("elamite.toml"),
            "[package]\nname = \"standard_system_test\"\nversion = \"0.1.0\"\ntarget_kind = \"exe\"\n",
        )
        .expect("write manifest");
        let source = include_str!("fixtures/standard_system/main.elx").replace(
            "__TEST_ROOT__",
            owned_runtime_path
                .to_str()
                .expect("temporary path is valid UTF-8"),
        );
        std::fs::write(root.join("src/main.elx"), source).expect("write fixture");
        Self {
            root,
            owned_runtime_path,
        }
    }

    fn command(&self, action: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_elamc"))
            .arg(action)
            .arg(&self.root)
            .env("ELAMITE_SYSTEM_TEST_VALUE", "present")
            .env(
                "ELAMITE_SYSTEM_TEST_INVALID",
                OsString::from_vec(vec![0xff]),
            )
            .env_remove("ELAMITE_SYSTEM_TEST_MISSING")
            .output()
            .unwrap_or_else(|error| panic!("run elamc {action}: {error}"))
    }
}

impl Drop for TestPackage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.owned_runtime_path);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn filesystem_surface_has_owned_results_and_explicit_cleanup() {
    let source = include_str!("../stdlib/src/fs.elx");
    let output = parse_standard_source("fs.elx", source);

    assert_eq!(output.tree.count(SyntaxKind::Struct), 3);
    assert_eq!(output.tree.count(SyntaxKind::Enum), 2);
    assert_eq!(output.tree.count(SyntaxKind::Impl), 1);
    for signature in [
        "pub fn open(path: &Path, mode: OpenMode) -> Result[File, io.IoError]",
        "pub fn read_dir(path: &Path) -> Result[Directory, io.IoError]",
        "`File.read_to_end`, `write_all`, `metadata`, and idempotent `close`",
        "`Directory.next` and idempotent `close`",
    ] {
        assert!(source.contains(signature), "missing `{signature}`");
    }
    assert!(
        source.contains(
            "cleanup surface is\n// exactly `pub fn close(self: &Self) -> ()` on each type"
        )
    );
    assert!(source.contains(
        "Closing either handle\n// repeatedly succeeds without another effect and never reports an I/O error"
    ));
}

#[test]
fn io_errors_are_portable_and_exhaustive_at_the_api_boundary() {
    let source = include_str!("../stdlib/src/io.elx");
    let output = parse_standard_source("io.elx", source);

    assert_eq!(output.tree.count(SyntaxKind::Enum), 1);
    assert_eq!(
        enum_variants(source, "IoError"),
        [
            "NotFound",
            "PermissionDenied",
            "AlreadyExists",
            "InvalidInput",
            "IsDirectory",
            "NotDirectory",
            "DirectoryNotEmpty",
            "ReadOnly",
            "BrokenPipe",
            "Interrupted",
            "WouldBlock",
            "TimedOut",
            "StorageFull",
            "ResourceExhausted",
            "Unsupported",
            "Other",
        ]
    );
    assert!(!source.contains("errno"));
}

#[test]
fn environment_surface_distinguishes_absence_from_failure() {
    let source = include_str!("../stdlib/src/env.elx");
    let output = parse_standard_source("env.elx", source);

    assert_eq!(output.tree.count(SyntaxKind::Enum), 1);
    assert!(source.contains("pub fn args() -> Vec[String]"));
    assert!(source.contains("pub fn get(name: str) -> Result[Option[String], EnvError]"));
    assert!(source.contains("pub fn current_dir() -> Result[fs.Path, io.IoError]"));
    assert!(source.contains("InvalidName"));
    assert!(source.contains("InvalidText"));
}

#[test]
fn process_surface_keeps_child_failure_separate_from_launch_failure() {
    let source = include_str!("../stdlib/src/process.elx");
    let output = parse_standard_source("process.elx", source);

    assert_eq!(output.tree.count(SyntaxKind::Struct), 2);
    assert_eq!(output.tree.count(SyntaxKind::Enum), 1);
    assert!(source.contains(
        "pub fn run(program: &fs.Path, arguments: [str]) -> Result[Output, ProcessError]"
    ));
    assert!(source.contains("A nonzero child exit is a successful `Output` value."));
    assert!(source.contains("pub stdout: Vec[u8]"));
    assert!(source.contains("pub stderr: Vec[u8]"));
    assert!(source.contains("pub fn exit(code: i32) -> !"));
}

#[test]
fn system_modules_check_build_and_run_real_operations() {
    let package = TestPackage::system_runtime();

    for action in ["check", "build"] {
        let output = package.command(action);
        assert!(output.status.success(), "{action}: {}", stderr(&output));
    }

    let output = package.command("run");
    assert!(
        output.status.success(),
        "run {:?}: {}",
        output.status,
        stderr(&output)
    );
    assert_eq!(
        String::from_utf8(output.stdout.clone()).expect("fixture stdout is UTF-8"),
        concat!(
            "true\n",
            "true\n",
            "3\n",
            "file\n",
            "3 65 66 67\n",
            "true\n",
            "true\n",
            "true\n",
            "true\n",
            "true\n",
            "false\n",
            "7\n",
            "9 99 116\n",
            "9 99 114\n",
            "launch-not-found\n",
        )
    );
    assert!(output.stderr.is_empty(), "{}", stderr(&output));
    assert!(
        !package.owned_runtime_path.exists(),
        "fixture did not remove its runtime directory"
    );

    let direct = Command::new(package.root.join("build/standard_system_test"))
        .arg(OsString::from_vec(vec![0xff]))
        .env("ELAMITE_SYSTEM_TEST_VALUE", "present")
        .env(
            "ELAMITE_SYSTEM_TEST_INVALID",
            OsString::from_vec(vec![0xff]),
        )
        .env_remove("ELAMITE_SYSTEM_TEST_MISSING")
        .output()
        .expect("run built system fixture with a non-text argument");
    assert!(direct.status.success(), "{}", stderr(&direct));
    let direct_stdout = String::from_utf8(direct.stdout).expect("fixture stdout is UTF-8");
    let lines = direct_stdout.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 16, "{direct_stdout}");
    assert_eq!(lines[7], "true", "invalid argv byte was not replaced");
    assert!(
        !package.owned_runtime_path.exists(),
        "direct fixture run did not remove its runtime directory"
    );
}

#[test]
fn system_runtime_emits_target_width_size_guards() {
    let package = TestPackage::system_runtime();
    for target in [Target::X86, Target::X86_64] {
        let mut sources = SourceManager::new();
        let graph = PackageGraph::resolve(&package.root.join("elamite.toml"), &mut sources)
            .expect("resolve system package");
        let generated = compile(&graph, &mut sources, target)
            .unwrap_or_else(|diagnostics| panic!("{diagnostics:#?}"))
            .generated_c;
        for guard in [
            "if(done>SIZE_MAX-(size_t)n)",
            "if(n>SIZE_MAX-b-(slash?1U:0U))",
            "if ((size_t)el_process_argc > SIZE_MAX / sizeof",
            "arguments.length > (uintptr_t)(SIZE_MAX / sizeof(char *)) - 2U",
        ] {
            assert!(generated.contains(guard), "{target:?}: missing `{guard}`");
        }
    }
}
