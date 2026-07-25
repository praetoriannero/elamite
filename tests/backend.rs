use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::backend::Target;
use elamite::diagnostics::{Category, Diagnostic};
use elamite::driver::{BuildOptions, Optimization, build, compile, run};
use elamite::package::PackageGraph;
use elamite::source::SourceManager;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestTree {
    root: PathBuf,
}

impl TestTree {
    fn new(name: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "elamite-backend-{}-{name}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("src")).expect("create test tree");
        Self { root }
    }

    fn executable(&self, source: &str) {
        fs::write(
            self.root.join("elamite.toml"),
            "[package]\nname = \"backend_test\"\nversion = \"0.1.0\"\ntarget_kind = \"executable\"\n",
        )
        .expect("write manifest");
        fs::write(self.root.join("src/main.elx"), source).expect("write source");
    }

    fn library(&self, source: &str) {
        fs::write(
            self.root.join("elamite.toml"),
            "[package]\nname = \"backend_test\"\nversion = \"0.1.0\"\ntarget_kind = \"library\"\n",
        )
        .expect("write manifest");
        fs::write(self.root.join("src/lib.elx"), source).expect("write source");
    }

    fn graph(&self, sources: &mut SourceManager) -> PackageGraph {
        PackageGraph::resolve(&self.root.join("elamite.toml"), sources)
            .expect("package graph resolves")
    }
}

impl Drop for TestTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn render(sources: &SourceManager, diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| {
            diagnostic.primary.map_or_else(
                || format!("{:?}: {}", diagnostic.category, diagnostic.message),
                |span| {
                    let position = sources.line_col(span.file, span.start);
                    format!(
                        "{}:{}:{}: {:?}: {}",
                        sources.path(span.file).display(),
                        position.line,
                        position.column,
                        diagnostic.category,
                        diagnostic.message
                    )
                },
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_and_run(source: &str, optimization: Optimization) -> (String, String, i32) {
    let tree = TestTree::new("run");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let artifact = build(
        &graph,
        &mut sources,
        &BuildOptions {
            target: Target::X86_64,
            optimization,
            output_directory: tree.root.join("out"),
            keep_generated_c: true,
            c_compiler: None,
        },
    )
    .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    assert!(
        artifact
            .generated_c_path
            .as_ref()
            .is_some_and(|path| path.is_file())
    );
    let result = run(&artifact).expect("run generated executable");
    (
        String::from_utf8(result.stdout).expect("UTF-8 stdout"),
        String::from_utf8(result.stderr).expect("UTF-8 stderr"),
        result.status.code().unwrap_or(-1),
    )
}

#[test]
fn builds_and_runs_calls_branches_loops_and_output() {
    let source = r#"
fn add(left: i32, right: i32) -> i32:
    return left + right

fn main() -> ():
    var total = 0
    var index = 0
    while index < 4:
        if index == 2:
            total += add(index, 1)
        else:
            total += index
        index += 1
    println(f"total={total}")
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(stdout, "total=7\n");
        assert_eq!(stderr, "");
        assert_eq!(status, 0);
    }
}

#[test]
fn preserves_left_to_right_evaluation_order() {
    let (stdout, stderr, status) = build_and_run(
        r#"
fn first() -> i32:
    print("A")
    return 1

fn second() -> i32:
    print("B")
    return 2

fn main() -> ():
    let sum = first() + second()
    println(f"={sum}")
"#,
        Optimization::Release,
    );
    assert_eq!(stdout, "AB=3\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn compound_assignment_evaluates_its_destination_once() {
    let (stdout, stderr, status) = build_and_run(
        r#"
fn selected_index() -> usize:
    print("I")
    return 0

fn main() -> ():
    var values = [1]
    values[selected_index()] += 4
    println(values[0])
"#,
        Optimization::Release,
    );
    assert_eq!(stdout, "I5\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn short_circuit_operators_do_not_evaluate_the_skipped_operand() {
    let (stdout, stderr, status) = build_and_run(
        r#"
fn observed() -> bool:
    print("unexpected")
    return true

fn main() -> ():
    if false && observed():
        print("also unexpected")
    if true || observed():
        println("ok")
"#,
        Optimization::Release,
    );
    assert_eq!(stdout, "ok\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn runtime_traps_have_stable_codes_and_source_locations() {
    let (stdout, stderr, status) = build_and_run(
        r#"
fn main() -> ():
    var zero = 0
    println(10 / zero)
"#,
        Optimization::Debug,
    );
    assert_eq!(stdout, "");
    assert_eq!(status, 101);
    assert!(stderr.contains("E-RUN-DIVZERO"), "{stderr}");
    assert!(stderr.contains("main.elx:4:"), "{stderr}");
}

#[test]
fn checked_integer_index_and_conversion_operations_trap() {
    let cases = [
        (
            "fn main() -> ():\n    var value: i8 = 127i8\n    value += 1i8\n",
            "E-RUN-OVERFLOW",
        ),
        (
            "fn main() -> ():\n    let values = [1, 2]\n    println(values[2])\n",
            "E-RUN-INDEX",
        ),
        (
            "fn main() -> ():\n    let value = 300 as i8\n    println(value)\n",
            "E-RUN-CAST",
        ),
        (
            "fn main() -> ():\n    let value = 1 << 32\n    println(value)\n",
            "E-RUN-SHIFT",
        ),
    ];
    for (source, code) in cases {
        let (_, stderr, status) = build_and_run(source, Optimization::Debug);
        assert_eq!(status, 101, "{stderr}");
        assert!(stderr.contains(code), "{stderr}");
    }
}

#[test]
fn source_integer_bases_are_normalized_to_portable_c99_constants() {
    let (stdout, stderr, status) = build_and_run(
        "fn main() -> ():\n    println(0b1010 + 0o7 + 0xff)\n    println(0xf32)\n    println(-9223372036854775808i64)\n    println(18446744073709551615u64)\n",
        Optimization::Debug,
    );
    assert_eq!(
        stdout,
        "272\n3890\n-9223372036854775808\n18446744073709551615\n"
    );
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn selected_pointer_width_controls_usize_literal_materialization() {
    let tree = TestTree::new("pointer-width");
    tree.executable("fn main() -> ():\n    let value: usize = 4294967296\n    println(value)\n");
    let mut x86_sources = SourceManager::new();
    let x86_graph = tree.graph(&mut x86_sources);
    let diagnostics = compile(&x86_graph, &mut x86_sources, Target::X86)
        .err()
        .expect("value does not fit 32-bit usize");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.category == Category::LiteralType)
    );

    let mut x64_sources = SourceManager::new();
    let x64_graph = tree.graph(&mut x64_sources);
    let compilation = compile(&x64_graph, &mut x64_sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&x64_sources, &diagnostics)));
    assert!(compilation.generated_c.contains("uintptr_t"));
}

#[test]
fn selected_x86_target_reaches_the_native_driver() {
    let tree = TestTree::new("x86-driver");
    tree.executable("fn main() -> ():\n    println(\"x86\")\n");
    let compiler = tree.root.join("fake-cc");
    let argument_log = tree.root.join("fake-cc.args");
    let script = format!(
        "#!/bin/sh\n\
         printf '%s\\n' \"$@\" > '{}'\n\
         output=''\n\
         while [ \"$#\" -gt 0 ]; do\n\
         \x20\x20if [ \"$1\" = '-o' ]; then shift; output=\"$1\"; fi\n\
         \x20\x20shift\n\
         done\n\
         : > \"$output\"\n",
        argument_log.display()
    );
    fs::write(&compiler, script).expect("write fake C compiler");
    let mut permissions = fs::metadata(&compiler)
        .expect("stat fake C compiler")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&compiler, permissions).expect("make fake C compiler executable");

    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let artifact = build(
        &graph,
        &mut sources,
        &BuildOptions {
            target: Target::X86,
            optimization: Optimization::Debug,
            output_directory: tree.root.join("x86-out"),
            keep_generated_c: true,
            c_compiler: Some(compiler.into_os_string()),
        },
    )
    .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    assert!(artifact.path.is_file());
    let arguments = fs::read_to_string(argument_log).expect("read fake compiler arguments");
    assert!(arguments.lines().any(|argument| argument == "-std=c99"));
    assert!(arguments.lines().any(|argument| argument == "-m32"));
}

#[test]
fn lowers_initial_struct_and_fixed_array_representations() {
    let (stdout, stderr, status) = build_and_run(
        r#"
struct Pair:
    left: i32
    right: i32

fn main() -> ():
    let pair = Pair { left: 2, right: 5 }
    var values = [pair.left, pair.right]
    values[0] += 3
    println(f"{values[0]},{values[1]}")
"#,
        Optimization::Debug,
    );
    assert_eq!(stdout, "5,5\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn generated_c_is_deterministic_c99_and_has_explicit_control_flow() {
    let tree = TestTree::new("emit");
    tree.executable(
        r#"
fn main() -> ():
    var value = 1
    if value == 1:
        value += 2
    println(value)
"#,
    );
    let mut first_sources = SourceManager::new();
    let first_graph = tree.graph(&mut first_sources);
    let first = compile(&first_graph, &mut first_sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&first_sources, &diagnostics)));
    let mut second_sources = SourceManager::new();
    let second_graph = tree.graph(&mut second_sources);
    let second = compile(&second_graph, &mut second_sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&second_sources, &diagnostics)));
    assert_eq!(first.generated_c, second.generated_c);
    assert!(first.generated_c.contains("-*-") || first.generated_c.contains("C99"));
    assert!(!first.generated_c.contains("_Static_assert"));
    assert!(first.generated_c.contains("goto b"));
    assert!(
        first
            .control_flow_ir
            .functions
            .iter()
            .any(|function| function.blocks.len() >= 4)
    );
    assert!(
        first
            .high_level_ir
            .functions
            .iter()
            .any(|function| !function.body.is_empty())
    );
}

#[test]
fn executable_subset_rejects_match_and_for_at_the_lowering_boundary() {
    let cases = [
        (
            "fn main() -> ():\n    match true:\n        true:\n            pass\n        false:\n            pass\n",
            "`match` lowering",
        ),
        (
            "fn main() -> ():\n    for value in [1, 2]:\n        println(value)\n",
            "`for` lowering",
        ),
    ];
    for (source, expected) in cases {
        let tree = TestTree::new("unsupported-lowering");
        tree.executable(source);
        let mut sources = SourceManager::new();
        let graph = tree.graph(&mut sources);
        let diagnostics = compile(&graph, &mut sources, Target::X86_64)
            .err()
            .expect("construct must remain outside the Milestone 8 executable subset");
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.category == Category::Lowering
                    && diagnostic.message.contains(expected)
                    && diagnostic.primary.is_some()
            }),
            "{}",
            render(&sources, &diagnostics)
        );
    }
}

#[test]
fn library_packages_produce_relocatable_objects_without_entry_shims() {
    let tree = TestTree::new("library");
    tree.library("pub fn answer() -> i32:\n    return 42\n");
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let artifact = build(
        &graph,
        &mut sources,
        &BuildOptions {
            target: Target::X86_64,
            optimization: Optimization::Debug,
            output_directory: tree.root.join("out"),
            keep_generated_c: true,
            c_compiler: None,
        },
    )
    .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    assert_eq!(
        artifact.path.extension().and_then(|value| value.to_str()),
        Some("o")
    );
    assert!(artifact.path.is_file());
    let c_path = artifact.generated_c_path.expect("retained generated C");
    let generated = fs::read_to_string(c_path).expect("read generated C");
    assert!(!generated.contains("int main("));
}

#[test]
fn command_line_run_builds_and_executes_a_package() {
    let tree = TestTree::new("cli");
    tree.executable("fn main() -> ():\n    println(\"from cli\")\n");
    let output = Command::new(env!("CARGO_BIN_EXE_elamite"))
        .arg("run")
        .arg(&tree.root)
        .arg(format!("--out-dir={}", tree.root.join("cli-out").display()))
        .output()
        .expect("run Elamite command");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "from cli\n");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
}

#[test]
fn command_line_check_honors_the_selected_pointer_width() {
    let tree = TestTree::new("cli-target");
    tree.executable("fn main() -> ():\n    let value: usize = 4294967296\n    println(value)\n");
    let output = Command::new(env!("CARGO_BIN_EXE_elamite"))
        .arg("check")
        .arg(&tree.root)
        .arg("--target=x86")
        .output()
        .expect("run Elamite check command");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("out of range for `usize`"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn reports_missing_toolchains_without_losing_generated_c() {
    let tree = TestTree::new("tool-failure");
    tree.executable("fn main() -> ():\n    pass\n");
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let output = tree.root.join("out");
    let diagnostics = build(
        &graph,
        &mut sources,
        &BuildOptions {
            target: Target::X86_64,
            optimization: Optimization::Debug,
            output_directory: output.clone(),
            keep_generated_c: false,
            c_compiler: Some("elamite-no-such-c-compiler".into()),
        },
    )
    .err()
    .expect("missing toolchain should fail");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.category == Category::Toolchain)
    );
    assert!(output.join("backend_test.c").is_file());
}

#[test]
fn reports_a_toolchain_that_omits_its_promised_artifact() {
    let tree = TestTree::new("missing-artifact");
    tree.executable("fn main() -> ():\n    pass\n");
    let compiler = tree.root.join("empty-cc");
    fs::write(&compiler, "#!/bin/sh\nexit 0\n").expect("write empty C compiler");
    let mut permissions = fs::metadata(&compiler)
        .expect("stat empty C compiler")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&compiler, permissions).expect("make empty C compiler executable");

    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let output = tree.root.join("out");
    let diagnostics = build(
        &graph,
        &mut sources,
        &BuildOptions {
            target: Target::X86_64,
            optimization: Optimization::Debug,
            output_directory: output.clone(),
            keep_generated_c: false,
            c_compiler: Some(compiler.into_os_string()),
        },
    )
    .err()
    .expect("a missing output artifact must fail the build");
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.category == Category::Toolchain && diagnostic.message.contains("did not create")
    }));
    assert!(output.join("backend_test.c").is_file());
}
