use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::backend::{COptions, COutput, emit_c};
use elamite::check::{check_borrows, check_for_target, check_moves, infer_provenance_signatures};
use elamite::config::{SemanticRevision, Target};
use elamite::diagnostics::{Category, Diagnostic};
use elamite::ir::{lower_control_flow, lower_typed_ir};
use elamite::package::PackageGraph;
use elamite::resolution::resolve;
use elamite::source::SourceManager;
use elamite::types::resolve_types;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct OwnedPackage {
    root: PathBuf,
}

impl OwnedPackage {
    fn new(name: &str, source: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "elamite-shared-graph-{}-{name}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("src")).expect("create fixture package");
        fs::write(
            root.join("elamite.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\ntarget_kind = \"exe\"\n"),
        )
        .expect("write manifest");
        fs::write(root.join("src/main.elx"), source).expect("write source");
        Self { root }
    }

    fn graph(&self, sources: &mut SourceManager) -> PackageGraph {
        PackageGraph::resolve_with_revision(
            &self.root.join("elamite.toml"),
            sources,
            SemanticRevision::V0_11,
        )
        .expect("resolve package graph")
    }
}

impl Drop for OwnedPackage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn lower_owned(name: &str, source: &str, target: Target) -> (OwnedPackage, COutput) {
    let package = OwnedPackage::new(name, source);
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources);
    let resolution = resolve(&graph, &mut sources);
    assert!(
        resolution.diagnostics.is_empty(),
        "{:#?}",
        resolution.diagnostics
    );
    let resolved = resolution.program;
    let mut types = resolve_types(&resolved);
    assert!(types.diagnostics.is_empty(), "{:#?}", types.diagnostics);
    let traits = elamite::traits::check_traits(&resolved, &mut types.program);
    assert!(traits.diagnostics.is_empty(), "{:#?}", traits.diagnostics);
    let checked = check_for_target(&resolved, &mut types.program, target.pointer_bits());
    assert!(checked.diagnostics.is_empty(), "{:#?}", checked.diagnostics);
    let provenance = infer_provenance_signatures(&resolved, &mut types.program);
    assert!(provenance.is_empty(), "{provenance:#?}");
    let typed_ir = lower_typed_ir(&resolved, &mut types.program, &checked.program);
    assert!(
        typed_ir.diagnostics.is_empty(),
        "{:#?}",
        typed_ir.diagnostics
    );
    let moves = check_moves(&resolved, &mut types.program, &typed_ir.program);
    assert!(moves.is_empty(), "{moves:#?}");
    let borrows = check_borrows(&resolved, &mut types.program, &typed_ir.program);
    assert!(borrows.is_empty(), "{borrows:#?}");
    let control_flow = lower_control_flow(&typed_ir.program, &types.program);
    let entry = resolved
        .declarations
        .iter()
        .find(|declaration| resolved.symbol_text(declaration.name) == "main")
        .map(|declaration| declaration.id)
        .expect("main declaration");
    let output = emit_c(
        &control_flow,
        &resolved,
        &types.program,
        &sources,
        &COptions {
            target,
            entry: Some(entry),
            test_entries: None,
        },
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    (package, output)
}

fn run_owned(name: &str, source: &str) -> std::process::Output {
    let (package, output) = lower_owned(name, source, Target::X86_64);
    let c_path = package.root.join("program.c");
    let executable = package.root.join("program");
    fs::write(&c_path, output.source).expect("write generated C");
    let mut compiler = Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()));
    compiler.args([
        "-std=c99",
        "-pedantic-errors",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-O2",
        "-fsanitize=address,undefined",
        "-fno-omit-frame-pointer",
    ]);
    compiler.arg(&c_path).arg("-o").arg(&executable);
    for library in output.native_libraries {
        compiler.arg(format!("-l{library}"));
    }
    let compiled = compiler.output().expect("run C compiler");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    Command::new(executable)
        // LeakSanitizer cannot enumerate threads under the test runner's
        // ptrace boundary; AddressSanitizer and UBSan remain active here.
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .output()
        .expect("run generated program")
}

fn owned_borrow_diagnostics(name: &str, source: &str) -> Vec<Diagnostic> {
    let package = OwnedPackage::new(name, source);
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources);
    let resolved = resolve(&graph, &mut sources).program;
    let mut types = resolve_types(&resolved).program;
    let traits = elamite::traits::check_traits(&resolved, &mut types);
    assert!(traits.diagnostics.is_empty(), "{:#?}", traits.diagnostics);
    let checked = check_for_target(&resolved, &mut types, 64);
    assert!(checked.diagnostics.is_empty(), "{:#?}", checked.diagnostics);
    let provenance = infer_provenance_signatures(&resolved, &mut types);
    assert!(provenance.is_empty(), "{provenance:#?}");
    let typed = lower_typed_ir(&resolved, &mut types, &checked.program);
    assert!(typed.diagnostics.is_empty(), "{:#?}", typed.diagnostics);
    check_borrows(&resolved, &mut types, &typed.program)
}

#[test]
fn shared_weak_and_generational_store_run_with_exact_cleanup() {
    let output = run_owned(
        "shared_store_run",
        r#"struct Node:
    value: usize

fn expired_weak() -> Weak[String]:
    let owner = Shared[String].new(String.from("temporary"))
    return owner.downgrade()

fn main() -> ():
    let owner = Shared[Node].new(Node { value: 7usize })
    let other = owner.clone()
    let weak = owner.downgrade()
    println(other.get().value)
    println(owner == other)
    match weak.upgrade():
        Option.Some(upgraded):
            println(upgraded.get().value)
        Option.None:
            println("expired")
    match expired_weak().upgrade():
        Option.Some(unexpected):
            println(unexpected.get())
        Option.None:
            println("expired")

    var nodes = Store[Node].new()
    let first = nodes.insert(Node { value: 41usize })
    println(first == first)
    let old = nodes.remove(first)
    println(old.value)
    let second = nodes.insert(Node { value: 42usize })
    println(first == second)
    println(nodes.get(second).value)
    for node in nodes:
        println(node.value)
"#,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "7\ntrue\n7\nexpired\ntrue\n41\nfalse\n42\n42\n"
    );
}

#[test]
fn stale_and_wrong_store_handles_trap_deterministically() {
    let stale = run_owned(
        "stale_handle",
        r#"fn main() -> ():
    var values = Store[i32].new()
    let handle = values.insert(1)
    let _ = values.remove(handle)
    values.compact()
    let _ = values.insert(2)
    let _ = values.get(handle)
"#,
    );
    assert_eq!(stale.status.code(), Some(101));
    assert!(String::from_utf8_lossy(&stale.stderr).contains("E-RUN-STALE"));

    let wrong = run_owned(
        "wrong_store",
        r#"fn main() -> ():
    var left = Store[i32].new()
    var right = Store[i32].new()
    let handle = left.insert(1)
    let _ = right.get(handle)
"#,
    );
    assert_eq!(wrong.status.code(), Some(101));
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("E-RUN-WRONG-STORE"));
}

#[test]
fn live_store_element_borrows_block_relocation_and_removal() {
    let diagnostics = owned_borrow_diagnostics(
        "store_borrow_conflict",
        r#"struct Node:
    value: i32

fn main() -> ():
    var nodes = Store[Node].new()
    let handle = nodes.insert(Node { value: 1 })
    let view = nodes.get(handle)
    let _ = nodes.insert(Node { value: 2 })
    println(view.value)
"#,
    );
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].category, Category::Borrow);
}

#[test]
fn handle_layout_is_pointer_width_neutral_c99() {
    let source = include_str!("fixtures/owned_model_design/target_width/handles.elx");
    let run = run_owned("handles_run", source);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        include_str!("fixtures/owned_model_design/target_width/handles.x86_64.stdout")
    );
    let (_, x64) = lower_owned("handles_x64", source, Target::X86_64);
    let (_, x86) = lower_owned("handles_x86", source, Target::X86);
    for generated in [&x64.source, &x86.source] {
        assert!(generated.contains("uintptr_t store; uintptr_t slot; uintptr_t generation;"));
        assert!(!generated.contains("_Static_assert"));
    }
}
