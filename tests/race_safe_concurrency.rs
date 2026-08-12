use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::backend::{COptions, emit_c};
use elamite::check::{check_borrows, check_for_target, check_moves, infer_provenance_signatures};
use elamite::config::{SemanticRevision, Target};
use elamite::diagnostics::Diagnostic;
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
            "elamite-race-safe-{}-{name}-{serial}",
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

fn diagnostics(name: &str, source: &str) -> Vec<Diagnostic> {
    let package = OwnedPackage::new(name, source);
    let mut sources = SourceManager::new();
    let resolved = resolve(&package.graph(&mut sources), &mut sources).program;
    let mut types = resolve_types(&resolved).program;
    let mut diagnostics = elamite::traits::check_traits(&resolved, &mut types).diagnostics;
    diagnostics.extend(check_for_target(&resolved, &mut types, 64).diagnostics);
    diagnostics
}

fn run_owned(name: &str, source: &str) -> std::process::Output {
    let package = OwnedPackage::new(name, source);
    let mut sources = SourceManager::new();
    let resolved = resolve(&package.graph(&mut sources), &mut sources).program;
    let mut types = resolve_types(&resolved).program;
    let traits = elamite::traits::check_traits(&resolved, &mut types);
    assert!(traits.diagnostics.is_empty(), "{:#?}", traits.diagnostics);
    let checked = check_for_target(&resolved, &mut types, 64);
    assert!(checked.diagnostics.is_empty(), "{:#?}", checked.diagnostics);
    let provenance = infer_provenance_signatures(&resolved, &mut types);
    assert!(provenance.is_empty(), "{provenance:#?}");
    let typed = lower_typed_ir(&resolved, &mut types, &checked.program);
    assert!(typed.diagnostics.is_empty(), "{:#?}", typed.diagnostics);
    let moves = check_moves(&resolved, &mut types, &typed.program);
    assert!(moves.is_empty(), "{moves:#?}");
    let borrows = check_borrows(&resolved, &mut types, &typed.program);
    assert!(borrows.is_empty(), "{borrows:#?}");
    let control_flow = lower_control_flow(&typed.program, &types);
    let entry = resolved
        .declarations
        .iter()
        .find(|declaration| resolved.symbol_text(declaration.name) == "main")
        .map(|declaration| declaration.id)
        .expect("main declaration");
    let generated = emit_c(
        &control_flow,
        &resolved,
        &types,
        &sources,
        &COptions {
            target: Target::X86_64,
            entry: Some(entry),
            test_entries: None,
        },
    );
    assert!(
        generated.diagnostics.is_empty(),
        "{:#?}",
        generated.diagnostics
    );
    let c_path = package.root.join("program.c");
    let executable = package.root.join("program");
    fs::write(&c_path, generated.source).expect("write generated C");
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
    for library in generated.native_libraries {
        compiler.arg(format!("-l{library}"));
    }
    let compiled = compiler.output().expect("run C compiler");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    Command::new(executable)
        .env("ASAN_OPTIONS", "detect_leaks=0")
        .output()
        .expect("run generated program")
}

#[test]
fn unscoped_threads_and_channels_reject_non_send_or_borrowed_state() {
    for (name, source, expected) in [
        (
            "raw_spawn",
            "fn main() -> ():\n    let value = 1\n    let pointer = &value as *i32\n    let worker = fn[pointer]() -> ():\n        pass\n    let _ = std.thread.spawn(worker)\n",
            "not `Send`",
        ),
        (
            "borrowed_spawn",
            "fn main() -> ():\n    let value = 1\n    let worker = fn[&value]() -> i32:\n        return value\n    let _ = std.thread.spawn(worker)\n",
            "cannot contain a borrowed value",
        ),
        (
            "raw_channel",
            "fn main() -> ():\n    let _ = std.sync.channel[*i32](1)\n",
            "message type must be `Send`",
        ),
    ] {
        let diagnostics = diagnostics(name, source);
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(expected)),
            "{diagnostics:#?}"
        );
    }
}

#[test]
fn unsafe_thread_capability_assertions_are_explicit() {
    let safe = diagnostics(
        "safe_send_impl",
        "struct Foreign:\n    pointer: *i32\n\nimpl Send for Foreign:\n    pass\n\nfn main() -> ():\n    pass\n",
    );
    assert!(
        safe.iter()
            .any(|diagnostic| diagnostic.message.contains("must be declared `unsafe`"))
    );
    let unsafe_impl = diagnostics(
        "unsafe_send_impl",
        "struct Foreign:\n    pointer: *i32\n\nunsafe impl Send for Foreign:\n    pass\n\nfn main() -> ():\n    let foreign = Foreign { pointer: null }\n    let worker = fn[foreign]() -> ():\n        pass\n    let _ = std.thread.spawn(worker)\n",
    );
    assert!(unsafe_impl.is_empty(), "{unsafe_impl:#?}");
}

#[test]
fn owned_thread_channel_mutex_and_atomic_runtime_is_race_safe() {
    let output = run_owned(
        "owned_concurrency",
        r#"fn main() -> ():
    let (sender, receiver) = std.sync.channel[String](1)
    let worker = fn[sender]() -> ():
        let message = String.from("moved")
        let _ = sender.send(message)
    match std.thread.spawn(worker):
        Result.Ok(thread):
            thread.join()
        Result.Err(_):
            pass
    match receiver.receive():
        Option.Some(received):
            println(received)
        Option.None:
            pass

    let mutex = Shared[std.sync.Mutex[i32]].new(std.sync.Mutex[i32].new(1))
    let child_mutex = mutex.clone()
    let update = fn[child_mutex]() -> ():
        let guard = (*child_mutex.get()).lock()
        *guard.get_var() = 7
    match std.thread.spawn(update):
        Result.Ok(thread):
            thread.join()
        Result.Err(_):
            pass
    let guard = (*mutex.get()).lock()
    println(*guard.get())

    let atomic = Shared[std.sync.AtomicUsize].new(std.sync.AtomicUsize.new(3usize))
    println((*atomic.get()).fetch_add(4usize))
    println((*atomic.get()).load())
"#,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "moved\n7\n3\n7\n");
}

#[test]
fn thread_scope_joins_borrowing_children_before_return() {
    let output = run_owned(
        "thread_scope",
        r#"fn main() -> ():
    var value = 1
    std.thread.scope(fn[&var value](scope: &var std.thread.Scope) -> ():
        let worker = scope.spawn(fn[&var value]() -> ():
            *value = 9
        )
        worker.join()
    )
    println(value)
"#,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "9\n");
}
