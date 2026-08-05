use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::backend::{COptions, Target, emit_c, mangle_function_instance};
use elamite::diagnostics::{Category, Diagnostic};
use elamite::driver::{BuildOptions, DumpStage, Optimization, build, compile, dump, run};
use elamite::ir::{TypedExpressionKind, TypedStatementKind};
use elamite::operations::{
    LogicalCopyAllocation, LogicalCopyKind, LogicalCopyLifetime, LogicalCopyMode, ValuePassingMode,
};
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
            "[package]\nname = \"backend_test\"\nversion = \"0.1.0\"\ntarget_kind = \"exe\"\n",
        )
        .expect("write manifest");
        fs::write(self.root.join("src/main.elx"), source).expect("write source");
    }

    fn library(&self, source: &str) {
        fs::write(
            self.root.join("elamite.toml"),
            "[package]\nname = \"backend_test\"\nversion = \"0.1.0\"\ntarget_kind = \"lib\"\n",
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

#[test]
fn production_c_is_unchanged_when_tests_are_appended() {
    let tree = TestTree::new("test-artifact-equivalence");
    let production = "pub fn answer() -> i32:\n    return 42\n";
    tree.library(production);

    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let without_tests = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)))
        .generated_c;

    tree.library(&format!(
        "{production}\ntest ignored_by_production_build:\n    missing_test_only_name()\n"
    ));
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let with_tests = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)))
        .generated_c;

    assert_eq!(without_tests, with_tests);
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
            features: Default::default(),
            output_directory: tree.root.join("out"),
            keep_generated_c: true,
            c_compiler: None,
            c_flags: Vec::new(),
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
fn adversarial_regressions_build_and_run_with_specified_behavior() {
    let cases = [
        (
            include_str!(
                "../tests/fixtures/regression/adversarial/regressions/variant_literal_arm.elx"
            ),
            "nonzero\n",
        ),
        (
            include_str!(
                "../tests/fixtures/regression/adversarial/regressions/trait_impl_for_primitive.elx"
            ),
            "bound call: i32\nqualified: i32\ngeneric bound: i32\n",
        ),
        (
            include_str!(
                "../tests/fixtures/regression/adversarial/regressions/self_expression_path.elx"
            ),
            "1\n",
        ),
        (
            include_str!(
                "../tests/fixtures/regression/adversarial/regressions/alias_collection.elx"
            ),
            "2\n",
        ),
        (
            include_str!(
                "../tests/fixtures/regression/adversarial/regressions/raw_pointer_field_access.elx"
            ),
            "7\n",
        ),
        (
            include_str!("../tests/fixtures/regression/adversarial/regressions/unit_equality.elx"),
            "true\n",
        ),
        (
            include_str!(
                "../tests/fixtures/regression/adversarial/regressions/trait_object_equality.elx"
            ),
            "true\n",
        ),
        (
            include_str!(
                "../tests/fixtures/regression/adversarial/regressions/field_less_struct.elx"
            ),
            "constructed\n",
        ),
        (
            include_str!(
                "../tests/fixtures/regression/adversarial/regressions/zero_length_array.elx"
            ),
            "0\n",
        ),
        (
            include_str!(
                "../tests/fixtures/regression/adversarial/regressions/shift_operand_typing.elx"
            ),
            "unsigned shift count: 8\n",
        ),
        (
            include_str!(
                "../tests/fixtures/regression/adversarial/regressions/variadic_iteration.elx"
            ),
            "length: 2\n7: one\n7: two\n",
        ),
        (
            include_str!(
                "../tests/fixtures/regression/adversarial/regressions/non_final_variadic.elx"
            ),
            "3\n",
        ),
    ];
    for (source, expected) in cases {
        let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, expected);
    }
}

#[test]
fn variadic_slices_use_managed_backing_storage_even_for_scalar_elements() {
    let tree = TestTree::new("managed-variadic-slice");
    tree.executable(
        r#"
fn collect(values: ...i32) -> [i32]:
    return values

fn main():
    let saved = collect(1, 2)
    println(saved.len())
"#,
    );
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    assert!(compilation.control_flow_ir.requires_managed_memory);
    assert!(compilation.generated_c.contains("static el_slice_t"));
    assert!(compilation.generated_c.contains("el_variadic_t"));
    assert!(compilation.generated_c.contains("result.values"));
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
fn native_threads_shallow_copy_closures_and_join_results() {
    let source = r#"
fn main() -> ():
    var values = @vec[1]
    let worker_values = values
    let worker = fn[worker_values]() -> Vec[i32]:
        return worker_values
    let started = std.thread.spawn(worker)
    values[0] = 7
    match started:
        Result.Ok(thread):
            let copy = thread
            var first = thread.join()
            println(first[0])
            first[0] = 9
            let second = copy.join()
            println(second[0])
            println(copy.is_finished())
        Result.Err(_):
            println("spawn failed")
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "7\n9\ntrue\n");
}

#[test]
fn threads_and_channels_publish_reference_pointer_and_trait_object_aliases() {
    let source = r#"
trait Read:
    fn read(self: &Self) -> i32

struct Value:
    number: i32

impl Read for Value:
    fn read(self: &Self) -> i32:
        return self.number

fn launch_slice(values: ...i32) -> Result[std.thread.Thread[i32], std.thread.SpawnError]:
    let worker = fn[values]() -> i32:
        return values[1]
    return std.thread.spawn(worker)

fn main() -> ():
    let number = 7
    let borrowed = &number
    let pointer = borrowed as *i32
    let concrete = Value { number: 11 }
    let erased: &Read = &concrete
    let (sender, receiver) = std.sync.channel[&i32](1)
    let _ = sender.send(borrowed)

    let worker = fn[receiver, *pointer as raw, erased]() -> i32:
        match receiver.receive():
            Option.Some(shared):
                unsafe:
                    return *shared + *raw + erased.read()
            Option.None:
                return 0

    match std.thread.spawn(worker):
        Result.Ok(thread):
            println(thread.join())
        Result.Err(_):
            println("spawn failed")

    match launch_slice(3, 4):
        Result.Ok(thread):
            println(thread.join())
        Result.Err(_):
            println("spawn failed")
"#;

    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "25\n4\n");
    }
}

#[test]
fn discarded_and_nested_threads_are_waited_for_at_shutdown() {
    let source = r#"
fn child() -> ():
    println("child")

fn parent() -> ():
    let _ = std.thread.spawn(child)
    println("parent")

fn main() -> ():
    let _ = std.thread.spawn(parent)
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    let lines = stdout.lines().collect::<BTreeSet<_>>();
    assert_eq!(lines, BTreeSet::from(["child", "parent"]));
}

#[test]
fn joining_the_current_thread_traps_the_process() {
    let source = r#"
fn main() -> ():
    let (sender, receiver) = std.sync.unbounded_channel[std.thread.Thread[()]]()
    let body = fn[receiver]() -> ():
        match receiver.receive():
            Option.Some(thread):
                thread.join()
            Option.None:
                pass
    let started = std.thread.spawn(body)
    match started:
        Result.Ok(thread):
            let _ = sender.send(thread)
            thread.join()
        Result.Err(_):
            pass
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 101, "{stdout}\n{stderr}");
    assert!(stderr.contains("E-RUN-SELF-JOIN"), "{stderr}");
}

#[test]
fn a_worker_panic_terminates_the_complete_process() {
    let (stdout, stderr, status) = build_and_run(
        r#"
fn fail() -> !:
    panic("worker failure")

fn main() -> ():
    let started = std.thread.spawn(fail)
    match started:
        Result.Ok(thread):
            thread.join()
        Result.Err(_):
            println("spawn unavailable")
"#,
        Optimization::Debug,
    );
    assert_eq!(stdout, "");
    assert_eq!(status, 101, "{stderr}");
    assert!(stderr.contains("E-RUN-PANIC"), "{stderr}");
    assert!(stderr.contains("worker failure"), "{stderr}");
}

#[test]
fn operating_system_spawn_failure_is_recoverable() {
    let tree = TestTree::new("spawn-failure");
    fs::create_dir_all(tree.root.join("native/include")).expect("create include directory");
    fs::write(
        tree.root.join("native/include/fail_spawn.h"),
        "#ifndef ELAMITE_FAIL_SPAWN_H\n\
         #define ELAMITE_FAIL_SPAWN_H\n\
         #include <errno.h>\n\
         static void elamite_force_spawn_failure(void) {}\n\
         #ifdef pthread_create\n\
         #undef pthread_create\n\
         #endif\n\
         static int elamite_failed_pthread_create(\n\
         \x20\x20\x20\x20pthread_t *thread, const pthread_attr_t *attributes,\n\
         \x20\x20\x20\x20void *(*entry)(void *), void *argument\n\
         ) {\n\
         \x20\x20\x20\x20(void)thread; (void)attributes; (void)entry; (void)argument;\n\
         \x20\x20\x20\x20return EAGAIN;\n\
         }\n\
         #define pthread_create elamite_failed_pthread_create\n\
         #endif\n",
    )
    .expect("write failure-injection header");
    fs::write(
        tree.root.join("elamite.toml"),
        "[package]\n\
         name = \"backend_test\"\n\
         version = \"0.1.0\"\n\
         target_kind = \"exe\"\n\
         \n\
         [native]\n\
         include_paths = [\"native/include\"]\n",
    )
    .expect("write manifest");
    fs::write(
        tree.root.join("src/main.elx"),
        r#"
use std.thread.SpawnError

@importc("elamite_force_spawn_failure", "fail_spawn.h")
fn force_spawn_failure()

fn worker() -> i32:
    return 42

fn main() -> ():
    unsafe:
        force_spawn_failure()
    match std.thread.spawn(worker):
        Result.Ok(_):
            println("unexpected success")
        Result.Err(reason):
            match reason:
                SpawnError.Unavailable:
                    println("unavailable")
"#,
    )
    .expect("write Elamite source");
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let artifact = build(
        &graph,
        &mut sources,
        &BuildOptions {
            target: Target::X86_64,
            optimization: Optimization::Debug,
            features: Default::default(),
            output_directory: tree.root.join("out"),
            keep_generated_c: true,
            c_compiler: None,
            c_flags: Vec::new(),
        },
    )
    .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    let result = run(&artifact).expect("run spawn-failure fixture");
    assert_eq!(result.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&result.stdout), "unavailable\n");
    assert_eq!(String::from_utf8_lossy(&result.stderr), "");
}

#[test]
fn never_returning_thread_bodies_and_joins_reach_c99_lowering() {
    let tree = TestTree::new("never-thread");
    tree.executable(
        r#"
fn diverge() -> !:
    panic("worker stopped")

fn main() -> ():
    let started = std.thread.spawn(diverge)
    match started:
        Result.Ok(thread):
            thread.join()
        Result.Err(_):
            pass
"#,
    );
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let artifact = build(
        &graph,
        &mut sources,
        &BuildOptions {
            target: Target::X86_64,
            optimization: Optimization::Debug,
            features: Default::default(),
            output_directory: tree.root.join("out"),
            keep_generated_c: true,
            c_compiler: None,
            c_flags: Vec::new(),
        },
    )
    .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    assert!(artifact.path.is_file());
}

#[test]
fn channels_mutexes_and_atomics_preserve_synchronized_handle_identity() {
    let source = r#"
use std.sync.TrySendError

fn increment(value: i32) -> i32:
    return value + 1

fn main() -> ():
    let (sender, receiver) = std.sync.channel[i32](1)
    match sender.try_send(7):
        Result.Ok(_):
            println("sent")
        Result.Err(_):
            println("unexpected send error")
    match sender.try_send(8):
        Result.Ok(_):
            println("unexpected capacity")
        Result.Err(reason):
            match reason:
                TrySendError.Full:
                    println("full")
                TrySendError.Closed:
                    println("unexpected closed")
    match receiver.receive():
        Option.Some(value):
            println(value)
        Option.None:
            println("unexpected close")
    sender.close()
    match receiver.receive():
        Option.Some(_):
            println("unexpected value")
        Option.None:
            println("drained")

    let (rendezvous_sender, rendezvous_receiver) = std.sync.channel[i32](0)
    let handoff = fn[rendezvous_sender]() -> ():
        let _ = rendezvous_sender.send(9)
        rendezvous_sender.close()
    let handoff_thread = std.thread.spawn(handoff)
    match rendezvous_receiver.receive():
        Option.Some(value):
            println(value)
        Option.None:
            println("unexpected rendezvous close")
    match handoff_thread:
        Result.Ok(thread):
            thread.join()
        Result.Err(_):
            println("spawn failed")

    let mutex = std.sync.Mutex[i32].new(0)
    let update = fn[mutex]() -> i32:
        var index = 0
        while index < 100:
            let _ = mutex.update(increment)
            index += 1
        return mutex.read()
    let left = std.thread.spawn(update)
    let right = std.thread.spawn(update)
    match left:
        Result.Ok(thread):
            let _ = thread.join()
        Result.Err(_):
            println("spawn failed")
    match right:
        Result.Ok(thread):
            let _ = thread.join()
        Result.Err(_):
            println("spawn failed")
    println(mutex.read())

    let atomic = std.sync.AtomicI32.new(0)
    let add = fn[atomic]() -> ():
        var index = 0
        while index < 100:
            let _ = atomic.fetch_add(1)
            index += 1
    let first = std.thread.spawn(add)
    let second = std.thread.spawn(add)
    match first:
        Result.Ok(thread):
            thread.join()
        Result.Err(_):
            println("spawn failed")
    match second:
        Result.Ok(thread):
            thread.join()
        Result.Err(_):
            println("spawn failed")
    println(atomic.load())
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "sent\nfull\n7\ndrained\n9\n200\n200\n");
    }
}

#[test]
fn synchronization_without_spawn_shallow_copies_messages() {
    let source = r#"
fn main() -> ():
    let (sender, receiver) = std.sync.unbounded_channel[Vec[i32]]()
    var original = @vec[1]
    let _ = sender.send(original)
    original[0] = 7
    original.append(2)
    sender.close()
    match receiver.receive():
        Option.Some(message):
            println(message.len())
            println(message[0])
            println(original.len())
        Option.None:
            println("unexpected close")
    let atomic = std.sync.AtomicUsize.new(3)
    println(atomic.exchange(4))
    println(atomic.compare_exchange(4, 5))
    println(atomic.fetch_sub(2))
    atomic.store(9)
    println(atomic.load())
    let flag = std.sync.AtomicBool.new(false)
    println(flag.compare_exchange(false, true))
    println(flag.exchange(false))

    let mutex = std.sync.Mutex[i32].new(1)
    let offset = 2
    let add_offset = fn[offset](value: i32) -> i32:
        return value + offset
    println(mutex.update(add_offset))
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "1\n7\n2\n3\ntrue\n5\n9\ntrue\ntrue\n3\n");
}

#[test]
fn concurrency_runtime_remains_c99_and_target_width_neutral() {
    let tree = TestTree::new("concurrency-target-width");
    tree.executable(
        r#"
fn worker() -> usize:
    let value = std.sync.AtomicUsize.new(1)
    let _ = value.fetch_add(2)
    return value.load()

fn main() -> ():
    let mutex = std.sync.Mutex[i32].new(7)
    let _ = mutex.read()
    let (sender, receiver) = std.sync.channel[i32](1)
    let _ = sender.send(9)
    let _ = receiver.receive()
    match std.thread.spawn(worker):
        Result.Ok(thread):
            println(thread.join())
        Result.Err(_):
            pass
"#,
    );
    for target in [Target::X86, Target::X86_64] {
        let mut sources = SourceManager::new();
        let graph = tree.graph(&mut sources);
        let compilation = compile(&graph, &mut sources, target)
            .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
        assert!(compilation.generated_c.contains("uintptr_t value;"));
        assert!(compilation.generated_c.contains("pthread_mutex_t lock;"));
        assert!(compilation.generated_c.contains("pthread_create("));
        assert!(compilation.generated_c.contains("pthread_join("));
        assert!(compilation.generated_c.contains("pthread_mutex_lock("));
        assert!(compilation.generated_c.contains("pthread_mutex_unlock("));
        assert!(compilation.generated_c.contains("pthread_cond_wait("));
        assert!(!compilation.generated_c.contains("_Atomic"));
        assert!(!compilation.generated_c.contains("_Static_assert"));
    }
}

#[test]
fn bounded_channels_sustain_mpmc_contention() {
    let source = r#"
fn increment(value: i32) -> i32:
    return value + 1

fn main() -> ():
    let (sender, receiver) = std.sync.channel[i32](8)
    let total = std.sync.Mutex[i32].new(0)
    let received = std.sync.AtomicUsize.new(0)
    let produce = fn[sender]() -> ():
        var index = 0
        while index < 1000:
            let _ = sender.send(1)
            index += 1
    let consume = fn[receiver, total, received]() -> ():
        var open = true
        while open:
            match receiver.receive():
                Option.Some(_):
                    let _ = total.update(increment)
                    let _ = received.fetch_add(1)
                Option.None:
                    open = false
    let first_consumer = std.thread.spawn(consume)
    let second_consumer = std.thread.spawn(consume)
    let first_producer = std.thread.spawn(produce)
    let second_producer = std.thread.spawn(produce)
    match first_producer:
        Result.Ok(thread):
            thread.join()
        Result.Err(_):
            println("spawn failed")
    match second_producer:
        Result.Ok(thread):
            thread.join()
        Result.Err(_):
            println("spawn failed")
    sender.close()
    match first_consumer:
        Result.Ok(thread):
            thread.join()
        Result.Err(_):
            println("spawn failed")
    match second_consumer:
        Result.Ok(thread):
            thread.join()
        Result.Err(_):
            println("spawn failed")
    receiver.close()
    println(total.read())
    println(received.load())
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Release);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "2000\n2000\n");
}

#[test]
fn closing_channels_wakes_blocked_senders_and_receivers() {
    let source = r#"
fn main() -> ():
    let (sender, receiver) = std.sync.channel[i32](1)
    let _ = sender.send(1)
    let blocked_send = fn[sender]() -> Result[(), std.sync.SendError]:
        return sender.send(2)
    let send_thread = std.thread.spawn(blocked_send)
    receiver.close()
    match send_thread:
        Result.Ok(thread):
            match thread.join():
                Result.Ok(_):
                    println("unexpected send")
                Result.Err(_):
                    println("sender woke")
        Result.Err(_):
            println("spawn unavailable")

    let (second_sender, second_receiver) = std.sync.unbounded_channel[i32]()
    let blocked_receive = fn[second_receiver]() -> Option[i32]:
        return second_receiver.receive()
    let receive_thread = std.thread.spawn(blocked_receive)
    second_sender.close()
    match receive_thread:
        Result.Ok(thread):
            match thread.join():
                Option.Some(_):
                    println("unexpected receive")
                Option.None:
                    println("receiver woke")
        Result.Err(_):
            println("spawn unavailable")
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "sender woke\nreceiver woke\n");
    }
}

#[test]
fn concurrent_output_preserves_complete_calls() {
    let source = r#"
fn main() -> ():
    let write_left = fn() -> ():
        var index = 0
        while index < 100:
            println("left")
            index += 1
    let write_right = fn() -> ():
        var index = 0
        while index < 100:
            println("right")
            index += 1
    let left = std.thread.spawn(write_left)
    let right = std.thread.spawn(write_right)
    match left:
        Result.Ok(thread):
            thread.join()
        Result.Err(_):
            pass
    match right:
        Result.Ok(thread):
            thread.join()
        Result.Err(_):
            pass
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout.lines().filter(|line| *line == "left").count(), 100);
    assert_eq!(stdout.lines().filter(|line| *line == "right").count(), 100);
    assert_eq!(stdout.lines().count(), 200, "{stdout}");
}

#[test]
fn explicit_closures_capture_copy_alias_and_call_through_generic_bounds() {
    let source = r#"
fn apply[F: Callable[(i32,), i32]](callback: F, value: i32) -> i32:
    return callback(value)

fn invoke(callback: &Callable[(i32,), i32], value: i32) -> i32:
    return callback(value)

fn invoke_zero(callback: &Callable[(), i32]) -> i32:
    return callback()

fn make(offset: i32) -> &Callable[(i32,), i32]:
    let add = fn[offset](value: i32) -> i32:
        return value + offset
    return &add

fn echo_with_closure[T](value: T) -> T:
    let echo = fn[value](ignored: ()) -> T:
        return value
    return echo(())

fn increment(value: i32) -> i32:
    return value + 1

struct Multiplier:
    factor: i32

impl Callable[(i32,), i32] for Multiplier:
    fn call(self: &Self, arguments: (i32,)) -> i32:
        return arguments.0 * self.factor

fn main() -> ():
    let offset: i32 = 4
    let add = fn[offset](value: i32):
        return value + offset
    let copied = add
    let composed = fn[add as callback](value: i32):
        return callback(value)
    var total: i32 = 10
    let increase = fn[&var total as state](value: i32):
        *state += value
        return *state
    let observe = fn[&total as state]() -> i32:
        return *state
    let read_pointer: *i32 = &total as *i32
    let write_pointer: *var i32 = &var total as *var i32
    let read_raw = fn[*read_pointer as pointer]() -> i32:
        unsafe:
            return *pointer
    let write_raw = fn[*var write_pointer as pointer](value: i32) -> i32:
        unsafe:
            *pointer += value
            return *pointer
    let constant = fn() -> i32:
        return 9
    let never = fn() -> !:
        panic("unreachable closure")
    println(add(3))
    println(copied(5))
    println(composed(2))
    println(increase(2))
    println(observe())
    println(read_raw())
    println(write_raw(3))
    println(total)
    println(constant())
    println(invoke_zero(&constant))
    println(apply(add, 6))
    println(apply(increment, 8))
    let callbacks = @vec[add, add]
    println(callbacks[0](1))
    println(invoke(&add, 7))
    let named = increment
    println(invoke(&named, 10))
    let multiplier = Multiplier { factor: 3 }
    println(multiplier(4))
    println(invoke(&multiplier, 5))
    let escaped = make(20)
    println(escaped(2))
    println(echo_with_closure(31))
    if false:
        never()
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(
            stdout,
            "7\n9\n6\n12\n12\n12\n15\n15\n9\n9\n10\n9\n5\n11\n11\n12\n15\n22\n31\n"
        );
        assert_eq!(stderr, "");
        assert_eq!(status, 0);
    }
}

#[test]
fn panic_is_a_never_returning_runtime_terminator() {
    let (stdout, stderr, status) = build_and_run(
        r#"
fn panic_message() -> str:
    print("message:")
    return "the value was missing"

fn cleanup() -> ():
    print("cleanup")

fn stop(message: str) -> !:
    std.panic(message)

fn main() -> ():
    println("before")
    defer cleanup()
    stop(panic_message())
    println("after")
"#,
        Optimization::Debug,
    );
    assert_eq!(stdout, "before\nmessage:");
    assert_ne!(status, 0);
    assert!(stderr.contains("E-RUN-PANIC"), "{stderr}");
    assert!(stderr.contains("the value was missing"), "{stderr}");
    assert!(stderr.contains("main.elx:10:"), "{stderr}");
}

#[test]
fn unreachable_panic_code_does_not_retain_an_unused_runtime_helper() {
    let (stdout, stderr, status) = build_and_run(
        r#"
fn dead_code() -> ():
    return
    panic("unreachable")

fn main() -> ():
    println("ok")
"#,
        Optimization::Debug,
    );
    assert_eq!(stdout, "ok\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn never_returns_survive_function_references_generics_and_trait_dispatch() {
    let source = r#"
fn halt() -> !:
    panic("halt")

fn indirect(callback: &fn() -> !) -> !:
    callback()

fn generic_halt[T](value: T) -> !:
    panic("generic halt")

trait Stopper:
    fn stop(self: &Self) -> !

struct Switch:
    armed: bool

impl Stopper for Switch:
    fn stop(self: &Self) -> !:
        panic("dynamic halt")

fn dynamic_halt(value: &Stopper) -> !:
    value.stop()

fn main() -> ():
    let raw = halt as *fn() -> !
    if false:
        indirect(halt)
    if false:
        unsafe:
            raw()
    if false:
        generic_halt(1)
    if false:
        let switch = Switch { armed: true }
        dynamic_halt(&switch)
    println("ok")
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(stdout, "ok\n");
        assert_eq!(stderr, "");
        assert_eq!(status, 0);
    }

    let tree = TestTree::new("never-x86");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    assert!(compilation.generated_c.contains("el_panic("));
    assert!(!compilation.generated_c.contains("_Noreturn"));
}

#[test]
fn imports_c_symbols_through_attributes() {
    let source = r#"
@importc("abs", "stdlib.h")
fn c_abs(value: i32) -> i32

fn main() -> ():
    unsafe:
        println(c_abs(-42))
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "42\n");
    assert_eq!(stderr, "");
}

#[test]
fn imported_c_structs_use_header_layout_and_field_names() {
    let source = r#"
@importc("div_t", "stdlib.h")
struct CDiv:
    quot: i32
    rem: i32

@importc("div", "stdlib.h")
fn c_div(numerator: i32, denominator: i32) -> CDiv

fn main() -> ():
    unsafe:
        let result = c_div(17, 5)
        println(result.quot)
        println(result.rem)
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "3\n2\n");
    assert_eq!(stderr, "");
}

#[test]
fn exports_unmangled_c_symbols() {
    let tree = TestTree::new("exportc");
    tree.library(
        r#"
@exportc("elamite_add_one")
fn add_one(value: i32) -> i32:
    return value + 1
"#,
    );
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    assert!(
        compilation
            .generated_c
            .contains("int32_t elamite_add_one(int32_t")
    );
    assert!(!compilation.generated_c.contains("elamite_add_one_t"));
}

#[test]
fn a_c_harness_calls_an_exported_elamite_function() {
    let tree = TestTree::new("exportc-harness");
    tree.library(
        r#"
@exportc("elamite_add_one")
fn add_one(value: i32) -> i32:
    return value + 1
"#,
    );
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let artifact = build(
        &graph,
        &mut sources,
        &BuildOptions {
            target: Target::X86_64,
            optimization: Optimization::Debug,
            features: Default::default(),
            output_directory: tree.root.join("out"),
            keep_generated_c: true,
            c_compiler: None,
            c_flags: Vec::new(),
        },
    )
    .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    let harness = tree.root.join("harness.c");
    fs::write(
        &harness,
        "extern int elamite_add_one(int);\n\
         int main(void) { return elamite_add_one(41) == 42 ? 0 : 1; }\n",
    )
    .expect("write C harness");
    let executable = tree.root.join("harness");
    let output = Command::new("cc")
        .arg("-std=c99")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-Werror")
        .arg(&harness)
        .arg(&artifact.path)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("run C compiler");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        Command::new(executable)
            .status()
            .expect("run harness")
            .success()
    );
}

#[test]
fn raw_function_pointers_are_general_and_directly_callable() {
    let source = r#"
fn increment(value: i32) -> i32:
    return value + 1

fn main() -> ():
    let reference: &fn(i32) -> i32 = increment
    let pointer: *fn(i32) -> i32 = reference as *fn(i32) -> i32
    unsafe:
        println(pointer(41))
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "42\n");
    assert_eq!(stderr, "");
}

#[test]
fn a_null_raw_function_pointer_traps_before_the_call() {
    let (_, stderr, status) = build_and_run(
        r#"
fn main() -> ():
    let pointer: *fn() -> () = null
    unsafe:
        pointer()
"#,
        Optimization::Debug,
    );
    assert_ne!(status, 0);
    assert!(stderr.contains("E-RUN-NULL"), "{stderr}");
}

#[test]
fn foreign_root_copies_share_one_idempotent_registration() {
    let source = r#"
@importc("GC_gcollect", "gc.h")
fn collect()

fn main() -> ():
    let value = 42
    let registration = std.ffi.ForeignRoot[i32].retain(&value)
    let copy = registration
    unsafe:
        collect()
        println(*copy.pointer())
    registration.close()
    copy.close()
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "42\n");
    assert_eq!(stderr, "");
}

#[test]
fn a_mutable_foreign_root_exposes_a_mutable_raw_pointer() {
    let source = r#"
fn main() -> ():
    var value = 1
    let registration = std.ffi.ForeignRootMut[i32].retain(&var value)
    unsafe:
        *registration.pointer() = 42
    println(value)
    registration.close()
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "42\n");
    assert_eq!(stderr, "");
}

#[test]
fn c_can_invoke_an_elamite_raw_function_pointer() {
    let source = r#"
@importc("atexit", "stdlib.h")
fn register_exit(callback: *fn() -> ()) -> i32

fn goodbye() -> ():
    println("callback")

fn main() -> ():
    let callback: &fn() -> () = goodbye
    unsafe:
        let status = register_exit(callback as *fn() -> ())
        println(status)
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "0\ncallback\n");
    assert_eq!(stderr, "");
}

#[test]
fn a_foreign_callback_can_use_registered_managed_context() {
    let tree = TestTree::new("callback-context");
    fs::create_dir_all(tree.root.join("native/include")).expect("create include directory");
    fs::write(
        tree.root.join("native/include/callback.h"),
        "#ifndef ELAMITE_CALLBACK_H\n\
         #define ELAMITE_CALLBACK_H\n\
         static int run_callback(int (*callback)(void *), void *context) {\n\
         \x20\x20\x20\x20return callback(context);\n\
         }\n\
         #endif\n",
    )
    .expect("write callback header");
    fs::write(
        tree.root.join("elamite.toml"),
        "[package]\n\
         name = \"backend_test\"\n\
         version = \"0.1.0\"\n\
         target_kind = \"exe\"\n\
         \n\
         [native]\n\
         include_paths = [\"native/include\"]\n",
    )
    .expect("write manifest");
    fs::write(
        tree.root.join("src/main.elx"),
        r#"
@importc("run_callback", "callback.h")
fn run_callback(callback: *fn(*std.ffi.CVoid) -> i32, context: *std.ffi.CVoid) -> i32

fn read_context(context: *std.ffi.CVoid) -> i32:
    unsafe:
        let value = context as *i32
        return *value

fn main() -> ():
    let value = 42
    let registration = std.ffi.ForeignRoot[i32].retain(&value)
    let handler: &fn(*std.ffi.CVoid) -> i32 = read_context
    unsafe:
        let context = registration.pointer() as *std.ffi.CVoid
        println(run_callback(handler as *fn(*std.ffi.CVoid) -> i32, context))
    registration.close()
"#,
    )
    .expect("write Elamite source");
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let artifact = build(
        &graph,
        &mut sources,
        &BuildOptions {
            target: Target::X86_64,
            optimization: Optimization::Debug,
            features: Default::default(),
            output_directory: tree.root.join("out"),
            keep_generated_c: true,
            c_compiler: None,
            c_flags: Vec::new(),
        },
    )
    .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    let generated = fs::read_to_string(
        artifact
            .generated_c_path
            .as_ref()
            .expect("generated C is retained"),
    )
    .expect("read generated C");
    assert!(generated.contains("GC_reachable_here("));
    let result = run(&artifact).expect("run callback fixture");
    assert_eq!(result.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&result.stdout), "42\n");
    assert_eq!(String::from_utf8_lossy(&result.stderr), "");
}

#[test]
fn a_closed_foreign_root_reports_a_stable_runtime_error() {
    let (_, stderr, status) = build_and_run(
        r#"
fn main() -> ():
    let value = 42
    let registration = std.ffi.ForeignRoot[i32].retain(&value)
    registration.close()
    registration.pointer()
"#,
        Optimization::Debug,
    );
    assert_ne!(status, 0);
    assert!(stderr.contains("E-RUN-CLOSED"), "{stderr}");
}

#[test]
fn rejects_invalid_c_contracts_and_unsafe_calls() {
    let cases = [
        (
            "generic import",
            "@importc(\"identity\", \"foreign.h\")\nfn identity[T](value: T) -> T\n",
            "cannot be generic",
        ),
        (
            "non ABI-safe parameter",
            "@importc(\"accept\", \"foreign.h\")\nfn accept(value: bool) -> i32\n",
            "ABI-safe",
        ),
        (
            "C variadic",
            "@importc(\"accept\", \"foreign.h\")\nfn accept(values: ...i32) -> i32\n",
            "variadic",
        ),
        (
            "derived foreign struct",
            "@importc(\"foreign_value\", \"foreign.h\")\n\
             struct ForeignValue(PartialEq):\n\
             \x20\x20\x20\x20value: i32\n",
            "cannot derive",
        ),
        (
            "unknown item macro",
            "@unknown(\"value\")\nfn main() -> ():\n    pass\n",
            "cannot resolve macro",
        ),
        (
            "safe imported call",
            "@importc(\"abs\", \"stdlib.h\")\nfn c_abs(value: i32) -> i32\n\
             fn main() -> ():\n    c_abs(-1)\n",
            "unsafe",
        ),
        (
            "deferred imported call",
            "@importc(\"close\", \"foreign.h\")\nfn close() -> ()\n\
             fn main() -> ():\n    defer close()\n",
            "unsafe",
        ),
        (
            "safe raw function-pointer call",
            "fn increment(value: i32) -> i32:\n    return value + 1\n\
             fn main() -> ():\n\
             \x20\x20\x20\x20let pointer = increment as *fn(i32) -> i32\n\
             \x20\x20\x20\x20pointer(1)\n",
            "unsafe",
        ),
        (
            "function-to-data pointer cast",
            "fn increment(value: i32) -> i32:\n    return value + 1\n\
             fn main() -> ():\n\
             \x20\x20\x20\x20let pointer = increment as *i32\n",
            "exactly its pointee type",
        ),
        (
            "mutable function pointer",
            "type Bad = *var fn(i32) -> i32\nfn main() -> ():\n    pass\n",
            "function pointers and references cannot be mutable",
        ),
        (
            "duplicate exported symbol",
            "@exportc(\"same_symbol\")\nfn first() -> ():\n    pass\n\
             @exportc(\"same_symbol\")\nfn second() -> ():\n    pass\n",
            "exported more than once",
        ),
    ];
    for (name, source, expected) in cases {
        let tree = TestTree::new(name);
        tree.executable(source);
        let mut sources = SourceManager::new();
        let graph = tree.graph(&mut sources);
        let diagnostics = compile(&graph, &mut sources, Target::X86_64)
            .err()
            .unwrap_or_else(|| panic!("{name} unexpectedly compiled"));
        let rendered = render(&sources, &diagnostics);
        assert!(
            rendered.contains(expected),
            "{name} did not report `{expected}`:\n{rendered}"
        );
    }
}

#[test]
fn monomorphizes_explicit_and_inferred_generic_calls() {
    let source = r#"
fn identity[T](value: T) -> T:
    return value

fn selector[T]() -> &fn(T) -> T:
    return identity[T]

fn main() -> ():
    println(identity[i32](40))
    println(identity(41))
    println(identity(42u32))
    let selected: &fn(i32) -> i32 = selector()
    println(selected(43))
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "40\n41\n42\n43\n");
}

#[test]
fn monomorphizes_explicit_and_inferred_generic_structs() {
    let source = r#"
struct Box[T]:
    value: T

fn main() -> ():
    let inferred = Box { value: 40 }
    let explicit = Box[i32] { value: 41 }
    println(inferred.value)
    println(explicit.value)
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "40\n41\n");
}

#[test]
fn infers_enclosing_and_method_generic_arguments_from_bound_calls() {
    let source = r#"
struct Box[T]:
    value: T

impl[T] Box[T]:
    fn get(self: &Self) -> T:
        return self.value

    fn replace[U](self: &Self, value: U) -> U:
        return value

    fn create(value: T) -> Self:
        return Self { value: value }

fn main() -> ():
    let boxed = Box { value: 40 }
    let explicit = Box[i32].create(42)
    let inferred = Box.create(43)
    println(boxed.get())
    println(boxed.replace(41))
    println(explicit.get())
    println(inferred.get())
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "40\n41\n42\n43\n");
}

#[test]
fn monomorphizes_inferred_and_expected_generic_enums() {
    let source = r#"
enum Maybe[T]:
    None
    Some(T)

fn main() -> ():
    let some = Maybe.Some(42)
    let explicit = Maybe[i32].Some(41)
    let none: Maybe[i32] = Maybe.None
    println(42)
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "42\n");
}

#[test]
fn permits_finite_recursive_generic_instantiation_sets() {
    let source = r#"
fn left[T](value: T, remaining: i32) -> T:
    if remaining == 0:
        return value
    return right(value, remaining - 1)

fn right[T](value: T, remaining: i32) -> T:
    if remaining == 0:
        return value
    return left(value, remaining - 1)

fn main() -> ():
    println(left(42, 4))
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "42\n");
}

#[test]
fn emits_recursive_generic_types_through_explicit_indirection() {
    let source = r#"
struct Node[T]:
    value: T
    next: *Node[T]

fn main() -> ():
    let node = Node { value: 42, next: null }
    println(node.value)
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "42\n");
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
fn builds_and_runs_methods_function_references_and_variadic_packing() {
    let source = r#"
struct Counter:
    value: i32

impl Counter:
    fn new(value: i32) -> Self:
        return Self{value: value}

    fn plus(self: Self, amount: i32) -> i32:
        return self.value + amount

    fn read(self: &Self) -> i32:
        return self.value

    fn add(self: &var Self, amount: i32) -> ():
        self.value += amount

    fn raw_read(self: *Self) -> i32:
        unsafe:
            return (*self).value

fn increment(value: i32) -> i32:
    return value + 1

fn choose() -> &fn(i32) -> i32:
    return increment

fn apply(callback: &fn(i32) -> i32, value: i32) -> i32:
    return callback(value)

fn variadic(first: i32, rest: ...i32) -> i32:
    return first + rest[1]

struct Transform:
    apply: &fn(i32) -> i32

fn main() -> ():
    var counter = Counter.new(10)
    counter.add(2)
    let shared = &counter
    let pointer: *Counter = shared as *Counter
    let unbound: &fn(&var Counter, i32) -> () = Counter.add
    unbound(&var counter, 3)
    let callback = choose()
    let transform = Transform{apply: callback}
    println(counter.read())
    println(shared.read())
    println(Counter.new(20).plus(2))
    println(pointer.raw_read())
    println(apply(callback, 40))
    println(transform.apply(4))
    println(callback == increment)
    println(variadic(7, 8, 9))
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(stdout, "15\n15\n22\n15\n41\n5\ntrue\n16\n");
        assert_eq!(stderr, "");
        assert_eq!(status, 0);
    }
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
fn lowers_source_ordered_matches_and_copied_payload_bindings() {
    let (stdout, stderr, status) = build_and_run(
        r#"
enum State:
    Count(i32)
    Positioned { x: i32, y: i32 }
    Disabled

fn describe(state: State) -> i32:
    match state:
        State.Count(value) if value > 10:
            return value + 100
        State.Count(value):
            return value
        State.Positioned { x, y }:
            return x + y
        State.Disabled:
            return 0

fn bool_value(value: bool) -> i32:
    match value:
        true:
            return 1
        false:
            return 0

fn main() -> ():
    println(describe(State.Count(4)))
    println(describe(State.Count(12)))
    println(describe(State.Positioned { x: 2, y: 3 }))
    println(describe(State.Disabled))
    println(bool_value(true))
"#,
        Optimization::Release,
    );
    assert_eq!(stdout, "4\n112\n5\n0\n1\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn enum_representation_uses_an_explicit_c99_discriminant_and_named_payload() {
    let tree = TestTree::new("enum-c99");
    tree.executable(
        "enum Value:\n    Integer(i32)\n    Empty\n\nfn main() -> ():\n    let value = Value.Integer(1)\n    match value:\n        Value.Integer(inner):\n            println(inner)\n        Value.Empty:\n            pass\n",
    );
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    assert!(compilation.generated_c.contains("uint32_t tag;"));
    assert!(compilation.generated_c.contains("} payload;"));
    assert!(!compilation.generated_c.contains("_Static_assert"));
}

#[test]
fn logical_copies_are_independent_across_assignment_arguments_and_returns() {
    let source = r#"
struct Inner:
    values: [i32; 2]

struct Outer:
    inner: Inner

fn changed(input: Outer) -> Outer:
    var result = input
    result.inner.values[0] = 9
    return result

fn main() -> ():
    var original = Outer { inner: Inner { values: [1, 2] } }
    var assigned = original
    assigned.inner.values[1] = 8
    let returned = changed(original)
    println(f"{original.inner.values[0]},{original.inner.values[1]}")
    println(f"{assigned.inner.values[0]},{assigned.inner.values[1]}")
    println(f"{returned.inner.values[0]},{returned.inner.values[1]}")
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(stdout, "1,2\n1,8\n9,2\n");
        assert_eq!(stderr, "");
        assert_eq!(status, 0);
    }
}

#[test]
fn shallow_aggregate_copies_preserve_nested_backing_across_value_flows() {
    let source = r#"
struct Payload:
    values: Vec[i32]

fn pass_through(value: Payload) -> Payload:
    return value

fn through_question(value: Payload) -> Result[Payload, String]:
    let ready: Result[Payload, String] = Result.Ok(value)
    let propagated = ready?
    return Result.Ok(propagated)

fn main() -> ():
    var original = Payload { values: @vec[1] }
    var assigned = original
    assigned.values[0] = 2

    var reassigned = Payload { values: @vec[99] }
    reassigned = original
    reassigned.values[0] = 3

    let indirect: &fn(Payload) -> Payload = pass_through
    var returned = indirect(reassigned)
    returned.values[0] = 4

    let tuple = (returned,)
    var (destructured,) = tuple
    destructured.values[0] = 5

    let wrapped: Option[Payload] = Option.Some(destructured)
    match wrapped:
        Option.Some(bound):
            var patterned = bound
            patterned.values[0] = 6
        Option.None:
            panic("missing payload")

    var holders = @vec[returned]
    var indexed = holders[0]
    indexed.values[0] = 7

    match through_question(returned):
        Result.Ok(value):
            var propagated = value
            propagated.values[0] = 8
        Result.Err(message):
            panic(f"{message}")

    let captured = returned
    let callable = fn[captured]() -> Payload:
        return captured
    let copied_callable = callable
    var from_closure = copied_callable()
    from_closure.values[0] = 9

    println(
        f"{original.values[0]}:{assigned.values[0]}:{reassigned.values[0]}"
        ++ f":{returned.values[0]}:{destructured.values[0]}:{holders[0].values[0]}"
        ++ f":{indexed.values[0]}:{from_closure.values[0]}"
    )
"#;
    let tree = TestTree::new("shallow-ordinary-copies");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));

    let copies = compilation
        .control_flow_ir
        .functions
        .iter()
        .flat_map(|function| function.logical_copy_inventory())
        .collect::<Vec<_>>();
    for kind in [
        LogicalCopyKind::Binding,
        LogicalCopyKind::Assignment,
        LogicalCopyKind::Return,
        LogicalCopyKind::Argument,
        LogicalCopyKind::AggregateElement,
        LogicalCopyKind::PatternBinding,
        LogicalCopyKind::ClosureCapture,
        LogicalCopyKind::Propagation,
    ] {
        assert!(
            copies.iter().any(|copy| {
                copy.facts.context.kind == kind
                    && copy.facts.allocation == LogicalCopyAllocation::Shallow
            }),
            "{kind:?} has an explicit shallow aggregate copy"
        );
    }
    assert!(copies.iter().any(|copy| {
        copy.facts.context.kind == LogicalCopyKind::Binding
            && copy.facts.allocation == LogicalCopyAllocation::PreserveIdentity
    }));

    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(stdout, "9:9:9:9:9:9:9:9\n");
        assert_eq!(stderr, "");
        assert_eq!(status, 0);
    }
}

#[test]
fn mutex_values_are_shallow_and_external_aliases_remain_visible() {
    let source = r#"
fn bump(values: Vec[i32]) -> Vec[i32]:
    var result = values
    result[0] += 1
    return result

fn main() -> ():
    var original = @vec[1]
    let mutex = std.sync.Mutex[Vec[i32]].new(original)
    original[0] = 7
    var read = mutex.read()
    println(read[0])
    read[0] = 8
    println(mutex.read()[0])
    read.append(2)
    println(f"{read.len()}:{mutex.read().len()}")

    let updated = mutex.update(bump)
    println(f"{updated[0]}:{original[0]}")

    var replacement = @vec[3]
    var previous = mutex.replace(replacement)
    replacement[0] = 4
    println(mutex.read()[0])
    previous[0] = 10
    println(original[0])
"#;
    let tree = TestTree::new("mutex-shallow-values");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    assert!(!compilation.generated_c.contains("el_copy_string"));
    assert!(!compilation.generated_c.contains("el_copy_t"));
    assert!(compilation.generated_c.contains("state->value = value;"));
    assert!(compilation.generated_c.contains("value = state->value;"));
    assert!(
        compilation
            .generated_c
            .contains("previous = state->value; state->value = replacement;")
    );

    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(stdout, "7\n8\n2:1\n9:9\n4\n10\n");
        assert_eq!(stderr, "");
        assert_eq!(status, 0);
    }
}

#[test]
fn mutex_does_not_claim_external_backing_aliases_are_synchronized() {
    let source = r#"
fn bump(values: Vec[i32]) -> Vec[i32]:
    var result = values
    result[0] += 1
    return result

fn main() -> ():
    var outside = @vec[0]
    let mutex = std.sync.Mutex[Vec[i32]].new(outside)
    let worker = fn[mutex]() -> ():
        let _ = mutex.update(bump)
    let started = std.thread.spawn(worker)
    outside[0] = 2
    match started:
        Result.Ok(thread):
            thread.join()
        Result.Err(_):
            pass
"#;
    let tree = TestTree::new("mutex-external-race-compiles");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
}

#[test]
fn strings_remain_shallow_across_thread_and_channel_publication() {
    let source = r#"
fn identity(value: String) -> String:
    return value

fn main() -> ():
    var original = String.from("alpha")
    let place = &original
    let snapshot = original
    let called = identity(snapshot)
    original = String.from("beta")

    let (sender, receiver) = std.sync.channel[String](1)
    let _ = sender.send(snapshot)
    let worker_value = snapshot
    let worker = fn[worker_value]() -> String:
        return worker_value
    let started = std.thread.spawn(worker)
    original = String.from("gamma")

    match receiver.receive():
        Option.Some(message):
            match started:
                Result.Ok(thread):
                    let result = thread.join()
                    let joined = snapshot ++ String.from("!")
                    println(f"{snapshot}:{called}:{message}:{result}:{place}:{joined}")
                Result.Err(_):
                    println("spawn failed")
        Option.None:
            println("channel closed")
"#;
    let tree = TestTree::new("string-shallow");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));

    assert!(
        compilation
            .generated_c
            .contains("typedef struct { const char *bytes; size_t length; } el_string;")
    );
    assert!(!compilation.generated_c.contains("el_string_backing"));
    assert!(compilation.generated_c.contains("char *el_string_make_mut"));
    assert!(compilation.generated_c.contains("el_runtime_alloc_atomic"));

    let identity = compilation
        .high_level_ir
        .functions
        .iter()
        .find(|function| function.name == "identity")
        .expect("identity is lowered");
    assert_eq!(identity.parameters[0].passing, ValuePassingMode::Owned);
    let publication_copies = compilation
        .control_flow_ir
        .functions
        .iter()
        .flat_map(|function| function.logical_copy_inventory())
        .filter(|copy| copy.facts.context.destination_lifetime == LogicalCopyLifetime::Thread)
        .collect::<Vec<_>>();
    assert!(!publication_copies.is_empty());
    assert!(publication_copies.iter().all(|copy| {
        matches!(
            copy.facts.allocation,
            LogicalCopyAllocation::PreserveIdentity
                | LogicalCopyAllocation::SharedBacking
                | LogicalCopyAllocation::Shallow
        )
    }));
    assert!(compilation.generated_c.contains("context->body = body;"));
    assert!(compilation.generated_c.contains("message->value = value;"));
    assert!(compilation.generated_c.contains("return state->result;"));

    let (stdout, stderr, status) = build_and_run(source, Optimization::Release);
    assert_eq!(stdout, "alpha:alpha:alpha:alpha:gamma:alpha!\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn string_mutation_aliases_ordinary_descriptors_without_detaching() {
    let tree = TestTree::new("string-shallow-mutation");
    tree.library("pub fn seed() -> String:\n    return String.from(\"alpha\")\n");
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let artifact = build(
        &graph,
        &mut sources,
        &BuildOptions {
            target: Target::X86_64,
            optimization: Optimization::Release,
            features: Default::default(),
            output_directory: tree.root.join("out"),
            keep_generated_c: true,
            c_compiler: None,
            c_flags: Vec::new(),
        },
    )
    .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    let generated = artifact
        .generated_c_path
        .as_ref()
        .expect("library C source is retained");
    let generated_source = fs::read_to_string(generated).expect("read generated C");
    assert!(!generated_source.contains("__sync_val_compare_and_swap"));
    assert!(!generated_source.contains("el_string_backing"));

    if !libgc_available() {
        eprintln!("skipping String alias execution: no gc.h or link archive available");
        return;
    }

    let harness = tree.root.join("out/string_alias_harness.c");
    fs::write(
        &harness,
        "#include \"backend_test.c\"\n\
         int main(void) {\n\
         \x20\x20\x20\x20el_string original;\n\
         \x20\x20\x20\x20el_string snapshot;\n\
         \x20\x20\x20\x20const char *shared;\n\
         \x20\x20\x20\x20char *first;\n\
         \x20\x20\x20\x20char *second;\n\
         \x20\x20\x20\x20GC_set_all_interior_pointers(1);\n\
         \x20\x20\x20\x20GC_INIT();\n\
         \x20\x20\x20\x20original = el_string_from((el_str){\"alpha\", (size_t)5U});\n\
         \x20\x20\x20\x20snapshot = original;\n\
         \x20\x20\x20\x20shared = original.bytes;\n\
         \x20\x20\x20\x20first = el_string_make_mut(&original);\n\
         \x20\x20\x20\x20if (first != shared) return 1;\n\
         \x20\x20\x20\x20first[0] = 'A';\n\
         \x20\x20\x20\x20if (memcmp(snapshot.bytes, \"Alpha\", 5U) != 0) return 2;\n\
         \x20\x20\x20\x20second = el_string_make_mut(&original);\n\
         \x20\x20\x20\x20if (second != first) return 3;\n\
         \x20\x20\x20\x20second[1] = 'L';\n\
         \x20\x20\x20\x20if (memcmp(original.bytes, \"ALpha\", 5U) != 0) return 4;\n\
         \x20\x20\x20\x20if (memcmp(snapshot.bytes, \"ALpha\", 5U) != 0) return 5;\n\
         \x20\x20\x20\x20return 0;\n\
         }\n",
    )
    .expect("write String alias harness");
    let executable = tree.root.join("string_alias_harness");
    let output = Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()))
        .arg("-std=c99")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-Werror")
        .arg("-m64")
        .arg(&harness)
        .arg("-o")
        .arg(&executable)
        .arg("-lgc")
        .output()
        .expect("compile String alias harness");
    assert!(
        output.status.success(),
        "String alias harness must compile:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = Command::new(&executable)
        .output()
        .expect("run String alias harness");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn lowers_alternative_tuple_struct_and_string_patterns() {
    let (stdout, stderr, status) = build_and_run(
        r#"
enum Number:
    Left(i32)
    Right(i32)
    Missing

struct Point:
    x: i32
    y: i32

fn number(value: Number) -> i32:
    match value:
        Number.Left(inner) | Number.Right(inner):
            return inner
        Number.Missing:
            return 0

fn pair(value: (i32, bool)) -> i32:
    match value:
        (0, false):
            return 1
        _:
            return 2

fn point(value: Point) -> i32:
    match value:
        Point { x: 1, y }:
            return y
        _:
            return 0

fn text(value: str) -> i32:
    match value:
        "yes":
            return 1
        _:
            return 0

fn main() -> ():
    println(number(Number.Left(3)))
    println(number(Number.Right(4)))
    println(pair((0, false)))
    println(pair((1, false)))
    println(point(Point { x: 1, y: 7 }))
    println(text("yes"))
"#,
        Optimization::Release,
    );
    assert_eq!(stdout, "3\n4\n1\n2\n7\n1\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn copied_match_payload_does_not_mutate_the_scrutinee() {
    let (stdout, stderr, status) = build_and_run(
        r#"
struct Pair:
    left: i32
    right: i32

enum Boxed:
    Value(Pair)

fn main() -> ():
    let boxed = Boxed.Value(Pair { left: 1, right: 2 })
    match boxed:
        Boxed.Value(pair):
            var changed = pair
            changed.left = 9
            println(changed.left)
    match boxed:
        Boxed.Value(pair):
            println(pair.left)
"#,
        Optimization::Release,
    );
    assert_eq!(stdout, "9\n1\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn aggregate_reads_produce_independent_values() {
    let (stdout, stderr, status) = build_and_run(
        r#"
struct Pair:
    left: i32
    right: i32

fn main() -> ():
    var values = [Pair { left: 1, right: 2 }]
    var selected = values[0]
    selected.left = 9
    println(selected.left)
    println(values[0].left)
"#,
        Optimization::Release,
    );
    assert_eq!(stdout, "9\n1\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn tuple_bindings_and_positional_fields_preserve_value_and_place_semantics() {
    let source = r#"
fn make(calls: &var i32) -> ((String, i32), (i32,)):
    *calls += 1
    return ((String.from("alpha"), 9), (3,))

fn select(calls: &var i32) -> (i32, i32):
    *calls += 1
    return (4, 5)

fn first[T](pair: (T, T)) -> T:
    return pair.0

fn main() -> ():
    var calls = 0
    let ((name, _), (right,)) = make(&var calls)
    println(calls)
    println(name)
    println(right)
    println(select(&var calls).1)
    println(calls)
    println(first[i32]((6, 7)))

    let () = ()
    let (single,) = (7,)
    println(single)

    var pair = (10, 20)
    var (independent, _) = pair
    independent = 99
    println(independent)
    println(pair.0)

    pair.0 = 11
    let alias = &var pair.1
    *alias = 21
    let shared: &(i32, i32) = &pair
    println(shared.0)
    println(pair.1)

    let pointer: *var (i32, i32) = (&var pair) as *var (i32, i32)
    unsafe:
        pointer.0 = 12
    println(pair.0)
    println(((1, 2), (3, 4)).1.0)

    var target = 1
    let (link,) = (&var target,)
    *link = 2
    println(target)
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stderr, "");
        assert_eq!(
            stdout,
            "1\nalpha\n3\n5\n2\n6\n7\n99\n10\n11\n21\n12\n3\n2\n"
        );
    }

    let tree = TestTree::new("tuple-x86");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    assert!(compilation.generated_c.contains(".v0"));
    assert!(compilation.generated_c.contains("el_check_ptr"));
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
            "fn main() -> ():\n    let values = [1, 2]\n    let index: usize = 2\n    println(values[index])\n",
            "E-RUN-INDEX",
        ),
        (
            "fn main() -> ():\n    let value = 300 as i8\n    println(value)\n",
            "E-RUN-CAST",
        ),
        (
            "fn main() -> ():\n    let count = 32u32\n    let value = 1 << count\n    println(value)\n",
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
fn collection_traps_have_stable_codes_and_source_locations() {
    let cases = [
        (
            "fn main() -> ():\n    var values = @vec[1, 2]\n    values.insert(3, 9)\n",
            "E-RUN-INDEX",
        ),
        (
            "fn main() -> ():\n    var values = @vec[1, 2]\n    println(values.remove(2))\n",
            "E-RUN-INDEX",
        ),
        (
            "fn main() -> ():\n    let values = @map{\"present\": 1}\n    println(values[\"missing\"])\n",
            "E-RUN-KEY",
        ),
    ];
    for (source, code) in cases {
        let (_, stderr, status) = build_and_run(source, Optimization::Debug);
        assert_eq!(status, 101, "{stderr}");
        assert!(stderr.contains(code), "{stderr}");
        assert!(stderr.contains("main.elx:3:"), "{stderr}");
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
    fs::write(
        tree.root.join("elamite.toml"),
        "[package]\n\
         name = \"backend_test\"\n\
         version = \"0.1.0\"\n\
         target_kind = \"exe\"\n\
         \n\
         [native]\n\
         include_paths = [\"native/include\"]\n\
         library_paths = [\"native/lib\"]\n\
         libraries = [\"fixture\"]\n\
         link_options = [\"-pthread\"]\n",
    )
    .expect("write native manifest");
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
            features: Default::default(),
            output_directory: tree.root.join("x86-out"),
            keep_generated_c: true,
            c_compiler: Some(compiler.into_os_string()),
            c_flags: Vec::new(),
        },
    )
    .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    assert!(artifact.path.is_file());
    let arguments = fs::read_to_string(argument_log).expect("read fake compiler arguments");
    assert!(arguments.lines().any(|argument| argument == "-std=c99"));
    assert!(arguments.lines().any(|argument| argument == "-m32"));
    assert!(arguments.lines().any(|argument| argument == "-I"));
    assert!(
        arguments
            .lines()
            .any(|argument| argument == tree.root.join("native/include").to_string_lossy())
    );
    assert!(arguments.lines().any(|argument| argument == "-L"));
    assert!(
        arguments
            .lines()
            .any(|argument| argument == tree.root.join("native/lib").to_string_lossy())
    );
    assert!(arguments.lines().any(|argument| argument == "-lfixture"));
    assert!(arguments.lines().any(|argument| argument == "-pthread"));
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
fn logical_copy_facts_survive_both_ir_levels_and_are_accounted_once() {
    let tree = TestTree::new("logical-copy-facts");
    tree.executable(
        r#"
fn worker() -> i32:
    return 7

fn main() -> ():
    let text = String.from("hello")
    let copied = text
    let started = std.thread.spawn(worker)
    let values = @vec[String.from("world")]
    for value in values:
        println(value)
    println(copied)
"#,
    );
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));

    let main = compilation
        .high_level_ir
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main is lowered");
    let copied_binding = main
        .body
        .iter()
        .filter_map(|statement| match &statement.kind {
            TypedStatementKind::Let { value, .. } => Some(value),
            _ => None,
        })
        .find(|value| {
            matches!(value.kind, TypedExpressionKind::Local(_))
                && value.copy.is_some_and(|copy| {
                    copy.context.kind == LogicalCopyKind::Binding
                        && copy.allocation == LogicalCopyAllocation::SharedBacking
                })
        })
        .expect("the String binding carries its shared-backing copy facts");
    let binding_context = copied_binding.copy.expect("binding has copy facts").context;
    assert_eq!(
        (
            binding_context.source_lifetime,
            binding_context.destination_lifetime,
        ),
        (
            LogicalCopyLifetime::LexicalScope,
            LogicalCopyLifetime::LexicalScope,
        )
    );

    let spawn_argument = main
        .body
        .iter()
        .filter_map(|statement| match &statement.kind {
            TypedStatementKind::Let {
                value:
                    elamite::ir::TypedExpression {
                        kind: TypedExpressionKind::StandardCall { arguments, .. },
                        ..
                    },
                ..
            } => arguments.first(),
            _ => None,
        })
        .find(|argument| {
            argument.copy.is_some_and(|copy| {
                copy.context.destination_lifetime == LogicalCopyLifetime::Thread
            })
        })
        .expect("thread spawn carries an ordinary shallow-copy argument");
    assert_eq!(
        spawn_argument
            .copy
            .expect("spawn argument has copy facts")
            .allocation,
        LogicalCopyAllocation::PreserveIdentity
    );
    assert_eq!(
        spawn_argument
            .copy
            .expect("spawn argument has copy facts")
            .mode,
        LogicalCopyMode::ReuseSource,
        "shallow publication reuses the immediate callable representation"
    );

    for function in &compilation.control_flow_ir.functions {
        let inventory = function.logical_copy_inventory();
        let mut ids = inventory
            .iter()
            .map(|record| record.id.index())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert_eq!(ids, (0..inventory.len()).collect::<Vec<_>>());
    }
    let emitted = compilation
        .control_flow_ir
        .functions
        .iter()
        .flat_map(|function| function.logical_copy_inventory())
        .collect::<Vec<_>>();
    assert!(emitted.iter().any(|copy| {
        copy.facts.context.kind == LogicalCopyKind::Binding
            && copy.facts.allocation == LogicalCopyAllocation::SharedBacking
    }));
    assert!(
        emitted
            .iter()
            .any(|copy| { copy.facts.context.destination_lifetime == LogicalCopyLifetime::Thread })
    );
    assert!(emitted.iter().any(|copy| {
        copy.facts.context.kind == LogicalCopyKind::IterationElement
            && copy.facts.allocation == LogicalCopyAllocation::SharedBacking
    }));
}

#[test]
fn proven_read_only_direct_calls_preserve_shallow_value_semantics() {
    let source = r#"
fn read_len(values: Vec[i32]) -> usize:
    return values.len()

fn recursive_len(values: Vec[i32], remaining: i32) -> usize:
    if remaining == 0:
        return values.len()
    return recursive_len(values, remaining - 1)

fn identity[T](value: T) -> T:
    return value

fn exposed(value: Vec[i32]) -> &Vec[i32]:
    return &value

fn indirect_len(values: Vec[i32]) -> usize:
    return values.len()

fn main() -> ():
    var values = @vec[1, 2, 3]
    let copied = identity(values)
    values.append(4)
    let direct = read_len(values)
    let recursive = recursive_len(values, 2)
    let callback: &fn(Vec[i32]) -> usize = indirect_len
    let indirect = callback(values)
    println(f"{direct}:{recursive}:{indirect}:{copied.len()}")
"#;
    let tree = TestTree::new("read-only-borrowing");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));

    let function = |name: &str| {
        compilation
            .high_level_ir
            .functions
            .iter()
            .find(|function| function.name == name)
            .unwrap_or_else(|| panic!("{name} is lowered"))
    };
    for name in ["read_len", "recursive_len", "identity"] {
        assert_eq!(
            function(name).parameters[0].passing,
            ValuePassingMode::ReadOnlyBorrowed,
            "{name}'s costly read-only parameter should use hidden borrowing"
        );
    }
    for name in ["exposed", "indirect_len"] {
        assert_eq!(
            function(name).parameters[0].passing,
            ValuePassingMode::Owned,
            "{name}'s escaping or callable ABI must retain eager copying"
        );
    }
    assert!(
        compilation
            .control_flow_ir
            .functions
            .iter()
            .find(|function| function.name == "identity")
            .expect("identity has control-flow IR")
            .logical_copy_inventory()
            .iter()
            .any(|copy| {
                copy.facts.context.kind == LogicalCopyKind::Return
                    && copy.facts.mode == LogicalCopyMode::Materialize
            }),
        "a borrowed parameter retains its explicit return-copy record"
    );

    let read_len = function("read_len");
    let symbol = mangle_function_instance(&compilation.resolved, &read_len.instance);
    let prototype = compilation
        .generated_c
        .lines()
        .find(|line| line.contains(&symbol) && line.ends_with(';'))
        .expect("read_len has a prototype");
    assert!(prototype.contains("const ") && prototype.contains(" *l"));
    assert!(
        compilation
            .control_flow_ir
            .functions
            .iter()
            .flat_map(|function| &function.blocks)
            .flat_map(|block| &block.instructions)
            .any(|instruction| matches!(
                instruction,
                elamite::ir::Instruction::Assign {
                    value: elamite::ir::Rvalue::Call { argument_modes, .. },
                    ..
                } if argument_modes.contains(&ValuePassingMode::ReadOnlyBorrowed)
            ))
    );

    let (stdout, stderr, status) = build_and_run(source, Optimization::Release);
    assert_eq!(stdout, "4:4:4:3\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn fresh_temporaries_and_dead_local_returns_reuse_storage_conservatively() {
    let source = r#"
struct Payload:
    text: String
    values: Vec[i32]

fn make_payload(label: str) -> Payload:
    return Payload {
        text: String.from(label),
        values: @vec[1, 2],
    }

fn return_local() -> Payload:
    let value = make_payload("local")
    return value

fn return_promoted() -> Payload:
    let value = make_payload("promoted")
    let alias = &value
    if alias.values.len() != 2:
        panic("promotion failed")
    return value

fn return_deferred() -> Payload:
    var value = make_payload("deferred")
    defer:
        value.values.append(3)
    return value

fn repeated() -> (Payload, Payload):
    let value = make_payload("repeated")
    return (value, value)

fn main() -> ():
    let fresh = make_payload("fresh")
    let read = fn[fresh]() -> usize:
        return fresh.values.len()
    let local = return_local()
    let promoted = return_promoted()
    let deferred = return_deferred()
    var pair = repeated()
    pair.0.values.append(9)
    println(
        f"{read()}:{local.values.len()}:{promoted.values.len()}:{deferred.values.len()}"
        ++ f":{pair.0.values.len()}:{pair.1.values.len()}"
    )
"#;
    let tree = TestTree::new("temporary-return-reuse");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));

    let inventory = |name: &str| {
        compilation
            .control_flow_ir
            .functions
            .iter()
            .find(|function| function.name == name)
            .unwrap_or_else(|| panic!("{name} is lowered"))
            .logical_copy_inventory()
    };
    assert!(inventory("make_payload").iter().any(|copy| {
        copy.facts.context.kind == LogicalCopyKind::Return
            && copy.facts.mode == LogicalCopyMode::ReuseSource
    }));
    assert!(inventory("return_local").iter().any(|copy| {
        copy.facts.context.kind == LogicalCopyKind::Return
            && copy.facts.context.source_lifetime == LogicalCopyLifetime::LexicalScope
            && copy.facts.mode == LogicalCopyMode::ReuseSource
    }));
    for name in ["return_promoted", "return_deferred"] {
        assert!(inventory(name).iter().any(|copy| {
            copy.facts.context.kind == LogicalCopyKind::Return
                && copy.facts.mode == LogicalCopyMode::Materialize
        }));
    }
    assert_eq!(
        inventory("repeated")
            .iter()
            .filter(|copy| {
                copy.facts.context.kind == LogicalCopyKind::AggregateElement
                    && copy.facts.mode == LogicalCopyMode::Materialize
            })
            .count(),
        2,
        "repeated local aggregate inputs retain two shallow-copy records"
    );
    assert!(
        !compilation.generated_c.contains(" = el_copy_"),
        "ordinary materialization remains a shallow C assignment"
    );

    let (stdout, stderr, status) = build_and_run(source, Optimization::Release);
    assert_eq!(stdout, "2:2:2:2:3:2\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn every_intermediate_dump_is_deterministic_and_source_identified() {
    let tree = TestTree::new("dumps");
    tree.executable("fn main() -> ():\n    println(42)\n");
    for stage in [
        DumpStage::Tokens,
        DumpStage::Syntax,
        DumpStage::Resolution,
        DumpStage::Types,
        DumpStage::TypedIr,
        DumpStage::ControlFlow,
        DumpStage::Monomorphized,
        DumpStage::GeneratedC,
    ] {
        let mut first_sources = SourceManager::new();
        let first_graph = tree.graph(&mut first_sources);
        let first = dump(&first_graph, &mut first_sources, Target::X86_64, stage)
            .unwrap_or_else(|diagnostics| panic!("{}", render(&first_sources, &diagnostics)));

        let mut second_sources = SourceManager::new();
        let second_graph = tree.graph(&mut second_sources);
        let second = dump(&second_graph, &mut second_sources, Target::X86_64, stage)
            .unwrap_or_else(|diagnostics| panic!("{}", render(&second_sources, &diagnostics)));

        assert_eq!(first, second, "{stage:?} dump changed between runs");
        assert!(
            first.contains("main.elx"),
            "{stage:?} dump lacks source identity:\n{first}"
        );
        assert!(!first.is_empty());
    }
}

#[test]
fn executable_subset_lowers_for_after_milestone_fourteen() {
    let (stdout, stderr, status) = build_and_run(
        "fn main() -> ():\n    for value in [1, 2]:\n        println(value)\n",
        Optimization::Debug,
    );
    assert_eq!(stdout, "1\n2\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn rejects_unbounded_recursive_generic_instantiation() {
    let tree = TestTree::new("unbounded-generic");
    tree.executable(
        r#"
fn expand[T](value: T) -> T:
    expand([value])
    return value

fn main() -> ():
    println(expand(1))
"#,
    );
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let diagnostics = compile(&graph, &mut sources, Target::X86_64)
        .err()
        .expect("unbounded generic expansion must be rejected");
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.category == Category::TypeSystem
                && diagnostic.message.contains("expands without bound")
                && diagnostic.primary.is_some()
        }),
        "{}",
        render(&sources, &diagnostics)
    );
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
            features: Default::default(),
            output_directory: tree.root.join("out"),
            keep_generated_c: true,
            c_compiler: None,
            c_flags: Vec::new(),
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
    assert!(artifact.metadata_path.is_file());
    let metadata = elamite::artifact::PackageMetadata::read(&artifact.metadata_path)
        .expect("read library metadata");
    assert_eq!(metadata.package_name, "backend_test");
    assert!(
        metadata
            .public_api
            .iter()
            .any(|item| item.path == "root.answer")
    );
}

#[test]
fn a_multi_package_build_consumes_reexported_generic_metadata_once() {
    let tree = TestTree::new("dependency-artifacts");
    let leaf = tree.root.join("leaf");
    let middle = tree.root.join("middle");
    let app = tree.root.join("app");
    for package in [&leaf, &middle, &app] {
        fs::create_dir_all(package.join("src")).expect("create package source");
    }
    fs::write(
        leaf.join("elamite.toml"),
        "[package]\nname = \"leaf\"\nversion = \"0.1.0\"\ntarget_kind = \"lib\"\n",
    )
    .expect("write leaf manifest");
    fs::write(
        leaf.join("src/lib.elx"),
        "pub fn identity[T](value: T) -> T:\n    return value\n",
    )
    .expect("write leaf source");
    fs::write(
        middle.join("elamite.toml"),
        "[package]\nname = \"middle\"\nversion = \"0.1.0\"\ntarget_kind = \"lib\"\n\
         \n[dependencies.leaf]\npath = \"../leaf\"\n",
    )
    .expect("write middle manifest");
    fs::write(
        middle.join("src/lib.elx"),
        "pub use leaf.identity as identity\n",
    )
    .expect("write middle source");
    fs::write(
        app.join("elamite.toml"),
        "[package]\nname = \"artifact_app\"\nversion = \"0.1.0\"\ntarget_kind = \"exe\"\n\
         \n[dependencies.middle]\npath = \"../middle\"\n",
    )
    .expect("write app manifest");
    fs::write(
        app.join("src/main.elx"),
        "use middle.identity\nfn main() -> ():\n    println(identity(42))\n",
    )
    .expect("write app source");

    let mut sources = SourceManager::new();
    let graph = PackageGraph::resolve(&app.join("elamite.toml"), &mut sources)
        .expect("resolve package graph");
    let artifact = build(
        &graph,
        &mut sources,
        &BuildOptions {
            target: Target::X86_64,
            optimization: Optimization::Debug,
            features: Default::default(),
            output_directory: tree.root.join("out"),
            keep_generated_c: false,
            c_compiler: None,
            c_flags: Vec::new(),
        },
    )
    .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    assert_eq!(artifact.dependency_metadata_paths.len(), 2);
    let unique = artifact
        .dependency_metadata_paths
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), 2);
    let metadata = artifact
        .dependency_metadata_paths
        .iter()
        .map(|path| elamite::artifact::PackageMetadata::read(path).expect("read metadata"))
        .collect::<Vec<_>>();
    let middle_metadata = metadata
        .iter()
        .find(|metadata| metadata.package_name == "middle")
        .expect("middle metadata");
    assert!(middle_metadata.public_api.iter().any(|item| {
        item.path == "root.identity" && item.reexport && item.signature.contains("identity[T]")
    }));
    let result = run(&artifact).expect("run multi-package executable");
    assert!(result.status.success());
    assert_eq!(result.stdout, b"42\n");
}

#[test]
fn command_line_run_builds_and_executes_a_package() {
    let tree = TestTree::new("cli");
    tree.executable("fn main() -> ():\n    println(\"from cli\")\n");
    let output = Command::new(env!("CARGO_BIN_EXE_elamc"))
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
fn command_line_compiles_one_source_file_to_the_exact_output_path() {
    let tree = TestTree::new("cli-single-file");
    let source = tree.root.join("standalone.elx");
    let output_path = tree.root.join("bin/custom-name");
    fs::write(
        &source,
        "fn main() -> ():\n    println(\"from one file\")\n",
    )
    .expect("write standalone source");
    fs::write(
        tree.root.join("broken.elx"),
        "this sibling must not be discovered",
    )
    .expect("write ignored sibling");

    let built = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .arg(&source)
        .arg("-o")
        .arg(&output_path)
        .output()
        .expect("compile standalone Elamite source");
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    assert_eq!(built.stdout, b"");
    assert_eq!(built.stderr, b"");
    assert!(output_path.is_file());

    let executed = Command::new(&output_path)
        .output()
        .expect("run standalone executable");
    assert!(executed.status.success());
    assert_eq!(executed.stdout, b"from one file\n");
}

#[test]
fn command_line_build_subcommand_accepts_a_source_file_and_output_path() {
    let tree = TestTree::new("cli-single-file-build");
    let source = tree.root.join("tool.elx");
    let output_path = tree.root.join("tool-bin");
    fs::write(&source, "fn main() -> ():\n    pass\n").expect("write standalone source");

    let built = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .arg("build")
        .arg(&source)
        .arg(format!("-o={}", output_path.display()))
        .output()
        .expect("build standalone Elamite source");
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    assert_eq!(built.stdout, b"");
    assert_eq!(built.stderr, b"");
    assert!(output_path.is_file());
}

#[test]
fn command_line_check_honors_the_selected_pointer_width() {
    let tree = TestTree::new("cli-target");
    tree.executable("fn main() -> ():\n    let value: usize = 4294967296\n    println(value)\n");
    let output = Command::new(env!("CARGO_BIN_EXE_elamc"))
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
fn command_line_help_lists_the_supported_workflows() {
    let output = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .arg("--help")
        .output()
        .expect("run Elamite help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for command in [
        "check",
        "build",
        "run",
        "init",
        "dump",
        "fmt",
        "doc",
        "test",
        "conformance",
    ] {
        assert!(
            stdout.contains(command),
            "missing `{command}` in:\n{stdout}"
        );
    }
}

#[test]
fn command_line_accepts_stable_user_defined_macros() {
    let tree = TestTree::new("stable-macros");
    tree.executable(
        r#"
macro inert(value: std.ast.Expression) -> std.ast.Expression:
    pass

fn main() -> ():
    pass
"#,
    );

    let checked = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .arg("check")
        .arg(&tree.root)
        .output()
        .expect("run check");
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );

    let obsolete = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .arg("check")
        .arg(&tree.root)
        .arg("--unstable-macros")
        .output()
        .expect("run obsolete option check");
    assert!(!obsolete.status.success());
}

#[test]
fn command_line_version_reports_the_specification_revision() {
    let output = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .arg("--version")
        .output()
        .expect("run compiler version command");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!(
            "elamc {} (SPEC {})\n",
            elamite::version(),
            elamite::spec_revision()
        )
    );
}

#[test]
fn command_line_rejects_an_unknown_target_with_clap_guidance() {
    let output = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .args(["check", "--target=arm64"])
        .output()
        .expect("run Elamite with an invalid target");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid value 'arm64'"), "{stderr}");
    assert!(stderr.contains("x86_64"), "{stderr}");
}

#[test]
fn command_line_init_creates_a_runnable_hello_world_package() {
    let tree = TestTree::new("cli-init");
    let package = tree.root.join("hello");
    let initialized = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .arg("init")
        .arg(&package)
        .output()
        .expect("run Elamite init command");
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    assert!(package.join("elamite.toml").is_file());
    assert!(package.join("src/main.elx").is_file());

    let executed = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .arg("run")
        .arg(&package)
        .output()
        .expect("run initialized package");
    assert!(
        executed.status.success(),
        "{}",
        String::from_utf8_lossy(&executed.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&executed.stdout),
        "Hello, Elamite!\n"
    );
}

#[test]
fn command_line_init_can_create_a_library_package() {
    let tree = TestTree::new("cli-init-lib");
    let package = tree.root.join("hello_lib");
    let initialized = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .arg("init")
        .arg(&package)
        .arg("--lib")
        .output()
        .expect("run Elamite library init command");
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    assert!(package.join("elamite.toml").is_file());
    assert!(package.join("src/lib.elx").is_file());
    assert!(!package.join("src/main.elx").exists());

    let built = Command::new(env!("CARGO_BIN_EXE_elamc"))
        .arg("build")
        .arg(&package)
        .output()
        .expect("build initialized library package");
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    assert!(package.join("build/hello_lib.o").is_file());
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
            features: Default::default(),
            output_directory: output.clone(),
            keep_generated_c: false,
            c_compiler: Some("elamite-no-such-c-compiler".into()),
            c_flags: Vec::new(),
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
            features: Default::default(),
            output_directory: output.clone(),
            keep_generated_c: false,
            c_compiler: Some(compiler.into_os_string()),
            c_flags: Vec::new(),
        },
    )
    .err()
    .expect("a missing output artifact must fail the build");
    assert!(diagnostics.iter().any(|diagnostic| {
        diagnostic.category == Category::Toolchain && diagnostic.message.contains("did not create")
    }));
    assert!(output.join("backend_test.c").is_file());
}

/// Whether a C toolchain can compile and link against the Boehm collector.
/// Only the runtime shared object is commonly installed; the headers and the
/// link archive come from a separate development package, so this probes the
/// real toolchain instead of guessing from a file path.
fn libgc_available() -> bool {
    let tree = TestTree::new("libgc-probe");
    let probe = tree.root.join("probe.c");
    let binary = tree.root.join("probe");
    if fs::write(
        &probe,
        "#include <gc.h>\nint main(void) { GC_INIT(); return 0; }\n",
    )
    .is_err()
    {
        return false;
    }
    Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()))
        .arg("-std=c99")
        .arg(&probe)
        .arg("-o")
        .arg(&binary)
        .arg("-lgc")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn programs_without_managed_storage_carry_no_collector_dependency() {
    let tree = TestTree::new("no-managed-memory");
    tree.executable("fn main() -> ():\n    println(\"ok\")\n");
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));

    assert!(!compilation.control_flow_ir.requires_managed_memory);
    assert!(
        compilation.native_libraries.is_empty(),
        "a program with no managed storage must not request runtime libraries"
    );
    assert!(!compilation.generated_c.contains("gc.h"));
    assert!(!compilation.generated_c.contains("GC_INIT"));
}

#[test]
fn managed_storage_engages_the_collector_prelude_and_link_inputs() {
    let tree = TestTree::new("managed-memory");
    tree.executable("fn main() -> ():\n    println(\"ok\")\n");
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));

    // Promotion analysis sets this flag from Phase 1 onward; forcing it here
    // exercises the prelude, entry-shim initialization, and link inputs before
    // any lowering produces managed storage.
    let mut program = compilation.control_flow_ir;
    program.requires_managed_memory = true;
    let emitted = emit_c(
        &program,
        &compilation.resolved,
        &compilation.typed,
        &sources,
        &COptions {
            target: Target::X86_64,
            entry: compilation.entry,
            test_entries: None,
        },
    );

    assert!(
        emitted.diagnostics.is_empty(),
        "{}",
        render(&sources, &emitted.diagnostics)
    );
    assert_eq!(emitted.native_libraries, vec!["gc".to_string()]);
    assert!(emitted.source.contains("#include <gc.h>"));
    let initialization = emitted
        .source
        .find("GC_INIT();")
        .expect("the entry shim initializes the collector");
    let entry = emitted
        .source
        .find("int main(void) {")
        .expect("an executable emits an entry shim");
    assert!(
        initialization > entry,
        "the collector must be initialized inside the entry shim"
    );
    let scanned_allocator = emitted
        .source
        .split("void *el_runtime_alloc(size_t byte_count) {")
        .nth(1)
        .and_then(|source| source.split("void *el_runtime_alloc_atomic").next())
        .expect("generated scanned allocation wrapper");
    let first_attempt = scanned_allocator
        .find("GC_MALLOC(byte_count)")
        .expect("first allocation attempt");
    let collection = scanned_allocator
        .find("GC_gcollect()")
        .expect("full collection after failure");
    let retry = scanned_allocator[first_attempt + 1..]
        .find("GC_MALLOC(byte_count)")
        .map(|offset| offset + first_attempt + 1)
        .expect("allocation retry");
    let terminal = scanned_allocator
        .find("el_out_of_memory()")
        .expect("terminal OOM path");
    assert!(
        first_attempt < collection && collection < retry && retry < terminal,
        "OOM handling must collect, retry, and only then terminate"
    );

    if !libgc_available() {
        eprintln!("skipping libgc link verification: no gc.h or link archive available");
        return;
    }
    let c_path = tree.root.join("managed.c");
    let binary = tree.root.join("managed");
    fs::write(&c_path, &emitted.source).expect("write generated C");
    let output = Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()))
        .arg("-std=c99")
        .arg("-Wall")
        .arg("-Wextra")
        .arg("-Werror")
        .arg("-m64")
        .arg(&c_path)
        .arg("-o")
        .arg(&binary)
        .args(
            emitted
                .native_libraries
                .iter()
                .map(|library| format!("-l{library}")),
        )
        .output()
        .expect("invoke the C compiler");
    assert!(
        output.status.success(),
        "collector-enabled C must compile and link:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let executed = Command::new(&binary).output().expect("run linked program");
    assert!(executed.status.success());
    assert_eq!(String::from_utf8_lossy(&executed.stdout), "ok\n");
}

#[test]
fn cost_instrumentation_is_opt_in_thread_safe_and_machine_readable() {
    let source = r#"
fn main() -> ():
    let text = String.from("instrumented")
    let values = @vec[text]
    let worker = fn[values]() -> usize:
        return values.len()
    match std.thread.spawn(worker):
        Result.Ok(thread):
            println(thread.join())
        Result.Err(_):
            pass
"#;
    let (_, ordinary_stderr, ordinary_status) = build_and_run(source, Optimization::Release);
    assert_eq!(ordinary_status, 0, "{ordinary_stderr}");
    assert_eq!(ordinary_stderr, "");

    let tree = TestTree::new("cost-instrumentation");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let artifact = build(
        &graph,
        &mut sources,
        &BuildOptions {
            target: Target::X86_64,
            optimization: Optimization::Release,
            features: Default::default(),
            output_directory: tree.root.join("out"),
            keep_generated_c: true,
            c_compiler: None,
            c_flags: vec!["-DELAMITE_COST_INSTRUMENTATION=1".into()],
        },
    )
    .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    let output = run(&artifact).expect("run cost-instrumented executable");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "1\n");
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 cost report");
    let fields = stderr.trim().split('\t').collect::<Vec<_>>();
    assert_eq!(fields.first(), Some(&"elamite-cost-v1"), "{stderr}");
    assert_eq!(fields.len(), 7, "{stderr}");
    for field in &fields[1..] {
        let (name, value) = field.split_once('=').expect("named numeric cost field");
        assert!(
            matches!(
                name,
                "allocations"
                    | "allocated_bytes"
                    | "scanned_allocations"
                    | "scanned_bytes"
                    | "memcpy_calls"
                    | "memcpy_bytes"
            ),
            "{stderr}"
        );
        assert!(value.parse::<u64>().is_ok(), "{stderr}");
    }
}

#[test]
fn cost_instrumentation_compiles_for_both_target_widths() {
    let tree = TestTree::new("cost-instrumentation-targets");
    tree.executable(
        r#"
fn main() -> ():
    let values = @vec[String.from("target width")]
    println(values.len())
"#,
    );
    for target in [Target::X86, Target::X86_64] {
        let mut sources = SourceManager::new();
        let graph = tree.graph(&mut sources);
        let artifact = build(
            &graph,
            &mut sources,
            &BuildOptions {
                target,
                optimization: Optimization::Release,
                features: Default::default(),
                output_directory: tree.root.join(format!("out-{}", target.pointer_bits())),
                keep_generated_c: true,
                c_compiler: None,
                c_flags: vec!["-DELAMITE_COST_INSTRUMENTATION=1".into()],
            },
        )
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
        assert!(artifact.path.is_file());
    }
}

#[test]
fn references_observe_storage_through_binding_and_path() {
    // SPEC 3.2: a reference names storage. A binding reference sees
    // reassignment, and a reference into an aggregate sees replacement of its
    // container, exactly as in C and Go.
    let source = r#"
struct Address:
    city: i32

struct Account:
    id: i32
    address: Address

fn main() -> ():
    var point = Address { city: 0 }
    let view = &point
    point = Address { city: 1 }
    println(f"binding={view.city}")

    var account = Account { id: 1, address: Address { city: 10 } }
    let city = &var account.address.city
    println(f"before={*city}")
    account = Account { id: 2, address: Address { city: 20 } }
    println(f"after={*city}")
    *city = 30
    println(f"container={account.address.city}")
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(
            stdout, "binding=1\nbefore=10\nafter=20\ncontainer=30\n",
            "optimization must not change reference behavior"
        );
        assert_eq!(stderr, "");
        assert_eq!(status, 0);
    }
}

#[test]
fn mutable_aliases_apply_sequential_writes() {
    // SPEC 3.2: references are not exclusive; later writes replace earlier
    // ones. This is the specification's own `count == 2` example.
    let (stdout, stderr, status) = build_and_run(
        r#"
fn main() -> ():
    var count = 0
    let first = &var count
    let second = &var count
    *first = 1
    *second = *second + 1
    println(f"{count}")
"#,
        Optimization::Debug,
    );
    assert_eq!(stdout, "2\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn a_reference_to_a_local_survives_its_frame() {
    // SPEC 3.2: returning a reference to a local is valid because the local is
    // promoted to managed storage when its address escapes.
    let (stdout, stderr, status) = build_and_run(
        r#"
fn answer() -> &i32:
    let value = 42
    return &value

struct Boxed:
    value: i32

fn literal() -> &Boxed:
    // Only a *composite* literal is addressable; `&7` is correctly rejected.
    let boxed = &Boxed { value: 7 }
    return boxed

fn main() -> ():
    let escaped = answer()
    let composite = literal()
    println(f"{*escaped},{composite.value}")
"#,
        Optimization::Release,
    );
    assert_eq!(stdout, "42,7\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn an_interior_reference_keeps_its_whole_container_reachable() {
    // LEDGER 19: a reference into an aggregate points inside a managed
    // allocation, so the collector must trace interior pointers and keep the
    // whole container alive. Allocation churn between the reference and its
    // use forces real collection cycles in between.
    if !libgc_available() {
        eprintln!("skipping: libgc unavailable");
        return;
    }
    let (stdout, stderr, status) = build_and_run(
        r#"
struct Address:
    city: i32

struct Account:
    id: i32
    address: Address

fn churn(rounds: i32) -> i32:
    var index = 0
    var total = 0
    while index < rounds:
        let garbage = &Account { id: index, address: Address { city: index } }
        total += garbage.id
        index += 1
    return total

fn main() -> ():
    var account = Account { id: 1, address: Address { city: 4242 } }
    let city = &account.address.city
    let ignored = churn(2000)
    println(f"{*city},{ignored}")
"#,
        Optimization::Release,
    );
    assert_eq!(
        stdout, "4242,1999000\n",
        "the container must survive collection while only its interior is referenced"
    );
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn sustained_allocation_churn_is_reclaimed() {
    // docs/roadmap.md Milestone 10: collection is best-effort and its *timing* is not a
    // conformance requirement, so this asserts only that a program allocating
    // far more than it retains completes normally rather than exhausting
    // memory.
    if !libgc_available() {
        eprintln!("skipping: libgc unavailable");
        return;
    }
    let (stdout, stderr, status) = build_and_run(
        r#"
struct Node:
    value: i32

fn main() -> ():
    var index = 0
    var live = 0
    while index < 300000:
        let node = &Node { value: index }
        live = node.value
        index += 1
    println(f"{live}")
"#,
        Optimization::Release,
    );
    assert_eq!(stdout, "299999\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn static_trait_dispatch_prefers_inherent_and_honors_qualified_selection() {
    // SPEC 6: a bound call prefers an inherent method; `Type.Trait.method`
    // selects the implementation's member unconditionally. A trait default
    // body is specialized for the implementing type.
    let (stdout, stderr, status) = build_and_run(
        r#"
trait Toggle:
    fn status(self: &Self) -> i32
    fn doubled(self: &Self) -> i32:
        return self.status() + self.status()

struct Session:
    active: bool

impl Session:
    fn status(self: &Self) -> i32:
        return 1

impl Toggle for Session:
    fn status(self: &Self) -> i32:
        return 2

fn main() -> ():
    let session = Session { active: true }
    println(f"bound={session.status()}")
    println(f"qualified={Session.Toggle.status(&session)}")
"#,
        Optimization::Release,
    );
    assert_eq!(stdout, "bound=1\nqualified=2\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn dynamic_dispatch_selects_by_concrete_type_through_one_trait() {
    // docs/roadmap.md Milestone 13: vtable dispatch with several concrete types behind one
    // trait object. Default methods participate in the vtable and an
    // implementation may override them.
    let source = r#"
trait Shape:
    fn area(self: &Self) -> i32
    fn describe(self: &Self) -> i32:
        return self.area() + 1000

struct Square:
    side: i32

struct Rect:
    width: i32
    height: i32

impl Shape for Square:
    fn area(self: &Self) -> i32:
        return self.side * self.side

impl Shape for Rect:
    fn area(self: &Self) -> i32:
        return self.width * self.height
    fn describe(self: &Self) -> i32:
        return self.area() + 2000

fn measure(shape: &Shape) -> i32:
    return shape.area()

fn describe(shape: &Shape) -> i32:
    return shape.describe()

fn main() -> ():
    let square = Square { side: 3 }
    let rect = Rect { width: 2, height: 5 }
    let sq = &square
    let rc = &rect
    println(f"{measure(&square)},{measure(&rect)}")
    println(f"{describe(sq)},{describe(rc)}")
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(
            stdout, "9,10\n1009,2010\n",
            "each object must dispatch to its own implementation"
        );
        assert_eq!(stderr, "");
        assert_eq!(status, 0);
    }
}

#[test]
fn a_trait_object_uses_a_fat_reference_and_thunked_vtable() {
    let tree = TestTree::new("vtable-shape");
    tree.executable(
        "trait Shape:\n    fn area(self: &Self) -> i32\n\nstruct Square:\n    side: i32\n\n\
         impl Shape for Square:\n    fn area(self: &Self) -> i32:\n        return self.side\n\n\
         fn measure(shape: &Shape) -> i32:\n    return shape.area()\n\n\
         fn main() -> ():\n    let square = Square { side: 2 }\n    let reference = &square\n\
         \x20\x20\x20\x20println(measure(reference))\n",
    );
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    // A fat reference: data plus vtable, dispatched indirectly.
    assert!(compilation.generated_c.contains("void *data;"));
    assert!(compilation.generated_c.contains("vtable->m0("));
    // Thunks adapt the receiver so no function pointer is cast between
    // incompatible types.
    assert!(compilation.generated_c.contains("el_thunk"));
    assert!(!compilation.generated_c.contains("_Static_assert"));
}

#[test]
fn derived_default_and_equality_are_structural() {
    // SPEC 4.3: `Default` supplies `Self.default()` fieldwise, and derived
    // equality compares components rather than machine representations.
    let (stdout, stderr, status) = build_and_run(
        r#"
struct Inner(Default, PartialEq):
    value: i32

struct Point(Default, PartialEq):
    x: i32
    y: i32
    inner: Inner

fn main() -> ():
    let origin = Point.default()
    let same = Point { x: 0, y: 0, inner: Inner { value: 0 } }
    let different = Point { x: 1, y: 0, inner: Inner { value: 0 } }
    let nested = Point { x: 0, y: 0, inner: Inner { value: 7 } }
    println(f"{origin.x},{origin.y},{origin.inner.value}")
    println(f"{origin == same},{origin == different},{origin == nested}")
    println(f"{origin != different}")
"#,
        Optimization::Release,
    );
    assert_eq!(stdout, "0,0,0\ntrue,false,false\ntrue\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn a_manual_default_implementation_supplies_the_associated_function() {
    let (stdout, stderr, status) = build_and_run(
        r#"
enum Choice:
    None
    Value(i32)

impl Default for Choice:
    fn default() -> Self:
        return Choice.Value(42)

struct Outer(Default):
    choice: Choice
    choices: [Choice; 2]

fn main() -> ():
    let choice = Choice.default()
    match choice:
        Choice.None:
            println("none")
        Choice.Value(value):
            println(value)
    let outer = Outer.default()
    match outer.choice:
        Choice.None:
            println("none")
        Choice.Value(value):
            println(value)
    match outer.choices[1usize]:
        Choice.None:
            println("none")
        Choice.Value(value):
            println(value)
"#,
        Optimization::Release,
    );
    assert_eq!(stdout, "42\n42\n42\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn derived_ordering_is_lexicographic_and_preserves_unordered_floats() {
    let (stdout, stderr, status) = build_and_run(
        r#"
struct Pair(PartialEq, PartialOrd):
    first: i32
    second: i32

enum Rank(PartialEq, PartialOrd):
    Low(i32)
    High(i32)

struct Measure(PartialEq, PartialOrd):
    value: f64

fn main() -> ():
    let a = Pair { first: 1, second: 9 }
    let b = Pair { first: 2, second: 0 }
    let low = Rank.Low(99)
    let high = Rank.High(-99)
    let nan = 0.0f64 / 0.0f64
    let unordered = Measure { value: nan }
    println(f"{a < b},{a <= b},{b > a},{b >= a}")
    println(f"{low < high},{Rank.Low(1) < Rank.Low(2)}")
    println(f"{unordered < unordered},{unordered <= unordered},{unordered > unordered},{unordered >= unordered}")
"#,
        Optimization::Release,
    );
    assert_eq!(
        stdout,
        "true,true,true,true\ntrue,true\nfalse,false,false,false\n"
    );
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn generic_trait_implementations_dispatch_statically_and_through_vtables() {
    let (stdout, stderr, status) = build_and_run(
        r#"
trait Label:
    fn label(self: &Self) -> str

struct Wrapper[T]:
    value: T

impl[T] Label for Wrapper[T]:
    fn label(self: &Self) -> str:
        return "generic"

fn main() -> ():
    let wrapped = Wrapper { value: 7i32 }
    println(wrapped.label())
    println(Wrapper[i32].Label.label(&wrapped))
    let reference = &wrapped
    let object: &Label = reference
    println(object.label())
"#,
        Optimization::Release,
    );
    assert_eq!(stdout, "generic\ngeneric\ngeneric\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn executes_option_construction_matching_and_payload_copies() {
    // Milestone 14.1: `Option[T]` reaches C through the same generic-enum
    // path as a user enum, so its payload copies independently of its source.
    let source = r#"
struct Counter:
    value: i32

fn describe(value: Option[i32]) -> i32:
    match value:
        Option.Some(inner):
            return inner
        Option.None:
            return -1

fn main() -> ():
    let some: Option[i32] = Option.Some(7)
    let none: Option[i32] = Option.None
    println(describe(some))
    println(describe(none))
    println(describe(Option.Some(8)))

    var source = Counter { value: 1 }
    let wrapped = Option.Some(source)
    source.value = 2
    match wrapped:
        Option.Some(copied):
            println(copied.value)
        Option.None:
            println(0)
    println(source.value)
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "7\n-1\n8\n1\n2\n");
    }
}

#[test]
fn option_defaults_to_none_through_an_explicit_discriminant() {
    // The enum discriminant is the variant's identity rather than its ordinal,
    // so a zero-initialized value is not `Option.None` and the default helper
    // must write the tag explicitly (`docs/spec.md` 4.3).
    let source = r#"
struct Undefaultable:
    value: i32

struct Holder(Default):
    counted: Option[i32]
    referenced: Option[&i32]
    nested: Option[Undefaultable]

fn main() -> ():
    let holder = Holder.default()
    match holder.counted:
        Option.Some(inner):
            println(inner)
        Option.None:
            println(1)
    match holder.referenced:
        Option.Some(target):
            println(*target)
        Option.None:
            println(2)
    match holder.nested:
        Option.Some(inner):
            println(inner.value)
        Option.None:
            println(3)
"#;
    let tree = TestTree::new("option-default");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    assert!(
        compilation
            .generated_c
            .lines()
            .any(|line| line.trim_start().starts_with("value.tag = UINT32_C(")),
        "the default helper must select `None` explicitly:\n{}",
        compilation.generated_c
    );

    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "1\n2\n3\n");
    }
}

#[test]
fn option_of_a_safe_reference_keeps_a_recursive_graph_reachable() {
    // `Option[&T]` is the nullable safe reference (`docs/spec.md` 4.2). Its payload
    // retains identity rather than copying, and the referenced links stay
    // reachable through the collector's interior-pointer scan.
    let source = r#"
struct Chain:
    value: i32
    next: Option[&Chain]

fn total(node: &Chain) -> i32:
    match node.next:
        Option.Some(following):
            return node.value + total(following)
        Option.None:
            return node.value

fn main() -> ():
    let leaf = Chain { value: 1, next: Option.None }
    let middle = Chain { value: 2, next: Option.Some(&leaf) }
    let head = Chain { value: 4, next: Option.Some(&middle) }
    println(total(&head))

    var mutated = Chain { value: 8, next: Option.None }
    let alias: Option[&var Chain] = Option.Some(&var mutated)
    match alias:
        Option.Some(target):
            target.value = 9
        Option.None:
            pass
    println(mutated.value)
"#;
    let tree = TestTree::new("option-reference");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    assert!(
        compilation.generated_c.contains("GC_MALLOC"),
        "a referenced link must live in managed storage:\n{}",
        compilation.generated_c
    );

    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "7\n9\n");
    }
}

#[test]
fn executes_result_construction_matching_and_payload_copies() {
    // Milestone 14.2: standard `Result[T, E]` values construct, match, and copy
    // through the same generic-enum path as `Option`. Milestone 15 still owns
    // its postfix `?` propagation role.
    let source = r#"
struct Payload:
    value: i32

fn parse(flag: bool) -> Result[i32, str]:
    if flag:
        return Result.Ok(7)
    return Result.Err("failed")

fn main() -> ():
    match parse(true):
        Result.Ok(value):
            println(value)
        Result.Err(message):
            println(message)
    match parse(false):
        Result.Ok(value):
            println(value)
        Result.Err(message):
            println(message)

    var source = Payload { value: 1 }
    let wrapped: Result[Payload, str] = Result.Ok(source)
    source.value = 2
    match wrapped:
        Result.Ok(copied):
            println(copied.value)
        Result.Err(message):
            println(message)
    println(source.value)
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "7\nfailed\n1\n2\n");
    }
}

#[test]
fn checked_numeric_conversion_reports_instead_of_trapping() {
    // Milestone 14.3: `Target.try_from(value)` is the nontrapping counterpart
    // of `value as Target` (`docs/spec.md` 4.1). It reuses the same range
    // boundaries, so the two never disagree about representability.
    let source = r#"
fn tag_i8(value: Result[i8, NumericError]) -> str:
    match value:
        Result.Ok(inner):
            return "ok"
        Result.Err(reason):
            match reason:
                NumericError.OutOfRange:
                    return "range"
                NumericError.NotANumber:
                    return "nan"

fn tag_u16(value: Result[u16, NumericError]) -> str:
    match value:
        Result.Ok(inner):
            return "ok"
        Result.Err(reason):
            return "err"

fn tag_f32(value: Result[f32, NumericError]) -> str:
    match value:
        Result.Ok(inner):
            return "ok"
        Result.Err(reason):
            return "err"

fn tag_usize(value: Result[usize, NumericError]) -> str:
    match value:
        Result.Ok(inner):
            return "ok"
        Result.Err(reason):
            return "err"

fn main() -> ():
    // signed integer source
    println(tag_i8(i8.try_from(127)))
    println(tag_i8(i8.try_from(128)))
    println(tag_i8(i8.try_from(-129)))
    // unsigned integer source
    println(tag_i8(i8.try_from(200u32)))
    println(tag_u16(u16.try_from(65535u32)))
    println(tag_u16(u16.try_from(65536u32)))
    // an unsigned target rejects a negative source rather than wrapping
    println(tag_u16(u16.try_from(-1)))
    // floating source: truncation, range, and NaN are distinguished
    println(tag_i8(i8.try_from(12.75)))
    println(tag_i8(i8.try_from(1.0e10)))
    println(tag_i8(i8.try_from(0.0 / 0.0)))
    // floating target: IEEE rounding never fails
    println(tag_f32(f32.try_from(9u64)))
    println(tag_f32(f32.try_from(1.5)))
    // pointer-width target
    println(tag_usize(usize.try_from(8)))
    println(tag_usize(usize.try_from(-8)))
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(
            stdout,
            "ok\nrange\nrange\nrange\nok\nerr\nerr\n\
             ok\nrange\nnan\nok\nok\nok\nerr\n"
        );
    }

    // The conversion must not reach the trapping cast path at all.
    let tree = TestTree::new("try-from-nontrapping");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    for helper in compilation
        .generated_c
        .split("static ")
        .filter(|chunk| chunk.contains("el_try_from_"))
    {
        let body = &helper[..helper.find("\n}").unwrap_or(helper.len())];
        assert!(
            !body.contains("el_trap") && !body.contains("el_cast_integer"),
            "a checked conversion must not trap:\n{body}"
        );
    }
}

#[test]
fn checked_conversion_bounds_follow_the_selected_pointer_width() {
    let source = "fn main() -> ():\n\
                  \x20\x20\x20\x20let converted = isize.try_from(1u64)\n\
                  \x20\x20\x20\x20pass\n";
    let tree = TestTree::new("try-from-width");
    tree.executable(source);

    // The shared prelude mentions every width, so inspect the conversion
    // helper itself rather than the whole translation unit.
    let helper = |generated: &str| {
        let start = generated
            .find("el_try_from_")
            .expect("the conversion emits a helper");
        let body = &generated[start..];
        body[..body.find("\n}").unwrap_or(body.len())].to_string()
    };

    let mut x86_sources = SourceManager::new();
    let x86_graph = tree.graph(&mut x86_sources);
    let x86 = compile(&x86_graph, &mut x86_sources, Target::X86)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&x86_sources, &diagnostics)));
    let x86_helper = helper(&x86.generated_c);
    assert!(x86_helper.contains("INT32_MAX"), "{x86_helper}");
    assert!(!x86_helper.contains("INT64_MAX"), "{x86_helper}");

    let mut x64_sources = SourceManager::new();
    let x64_graph = tree.graph(&mut x64_sources);
    let x64 = compile(&x64_graph, &mut x64_sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&x64_sources, &diagnostics)));
    let x64_helper = helper(&x64.generated_c);
    assert!(x64_helper.contains("INT64_MAX"), "{x64_helper}");
    assert!(!x64_helper.contains("INT32_MAX"), "{x64_helper}");
}

#[test]
fn an_unreachable_block_emits_neither_a_label_nor_a_goto() {
    // A `match` whose every arm returns leaves its join block with no live
    // predecessor. Emitting that block's label anyway leaves a label no `goto`
    // names, which C rejects under `-Werror=unused-label`.
    let source = r#"
enum Inner:
    A
    B

enum Outer:
    One(i32)
    Two(Inner)

fn tag(value: Outer) -> str:
    match value:
        Outer.One(count):
            return "one"
        Outer.Two(inner):
            match inner:
                Inner.A:
                    return "a"
                Inner.B:
                    return "b"

fn main() -> ():
    println(tag(Outer.One(1)))
    println(tag(Outer.Two(Inner.A)))
    println(tag(Outer.Two(Inner.B)))
"#;
    let tree = TestTree::new("unreachable-block");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    for label in compilation
        .generated_c
        .lines()
        .filter_map(|line| line.strip_suffix(':'))
        .filter(|label| label.starts_with('b'))
    {
        assert!(
            compilation.generated_c.contains(&format!("goto {label};")),
            "label `{label}` has no `goto` naming it:\n{}",
            compilation.generated_c
        );
    }

    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "one\na\nb\n");
}

#[test]
fn numeric_alternatives_replace_the_trapping_operators_at_the_width_boundary() {
    // Milestone 14.4: the trapping operators stay the default; these are the
    // explicit opt-outs (`docs/spec.md` 4.1). Checked reports `Option.None`,
    // wrapping wraps modulo the range, saturating clamps.
    let source = r#"
fn show(value: Option[i8]) -> ():
    match value:
        Option.Some(inner):
            println(inner)
        Option.None:
            println("none")

fn show_u8(value: Option[u8]) -> ():
    match value:
        Option.Some(inner):
            println(inner)
        Option.None:
            println("none")

fn main() -> ():
    // Postfix binds tighter than unary minus, so a negative receiver needs a
    // binding (or parentheses) for the literal to reach its signed minimum.
    let low: i8 = -128
    let high: i8 = 127
    let zero: u8 = 0

    // checked: the exact boundary succeeds, one past it reports
    show(100i8.checked_add(27i8))
    show(100i8.checked_add(28i8))
    show(low.checked_sub(1i8))
    show(63i8.checked_mul(2i8))
    show(64i8.checked_mul(2i8))
    show(1i8.checked_div(0i8))
    show(low.checked_div(-1i8))
    show(low.checked_rem(-1i8))
    show(low.checked_neg())
    show(1i8.checked_shl(6i8))
    show(1i8.checked_shl(7i8))
    show(1i8.checked_shr(8i8))
    show_u8(zero.checked_sub(1u8))
    show_u8(zero.checked_neg())

    // wrapping
    println(100i8.wrapping_add(28i8))
    println(low.wrapping_sub(1i8))
    println(64i8.wrapping_mul(2i8))
    println(low.wrapping_div(-1i8))
    println(low.wrapping_rem(-1i8))
    println(low.wrapping_neg())
    println(1i8.wrapping_shl(7i8))
    println(low.wrapping_shr(1i8))
    println(zero.wrapping_sub(1u8))

    // saturating
    println(high.saturating_add(28i8))
    println(low.saturating_sub(1i8))
    println(64i8.saturating_mul(2i8))
    println((0i8 - 64i8).saturating_mul(4i8))
    println(zero.saturating_sub(1u8))
    println(200u8.saturating_mul(2u8))
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(
            stdout,
            "127\nnone\nnone\n126\nnone\nnone\nnone\n0\nnone\n64\nnone\nnone\nnone\n0\n\
             -128\n127\n-128\n-128\n0\n-128\n-128\n-64\n255\n\
             127\n-128\n127\n-128\n0\n255\n"
        );
    }
}

#[test]
fn standard_collections_construct_shallow_copy_mutate_and_query() {
    let source = r#"
fn main() -> ():
    var values = @vec[1, 2, 3]
    values.append(4)
    values.insert(1, 9)
    let removed = values.remove(2)
    var copied = values
    copied[0] = 7
    println(f"{values.len()} {values[0]} {copied[0]} {removed}")

    var names = @map{"one": 1, "two": 2, "one": 3}
    let replaced = names.insert("two", 8)
    let absent = names.remove("missing")
    names["one"] += 1
    println(f"{names.len()} {names["one"]} {names.contains_key("two")}")
    match replaced:
        Option.Some(value):
            println(f"{value}")
        Option.None:
            println("none")
    match absent:
        Option.Some(value):
            println(f"{value}")
        Option.None:
            println("none")

    var tags = @set{"a", "b", "a"}
    println(f"{tags.len()} {tags.insert("c")} {tags.insert("c")} {tags.remove("a")}")
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(stdout, "4 7 7 2\n2 4 true\n2\nnone\n2 true false true\n");
        assert_eq!(stderr, "");
        assert_eq!(status, 0);
    }
}

#[test]
fn vector_copies_keep_descriptor_state_local_and_share_backing_until_growth() {
    let source = r#"
fn main() -> ():
    var original: Vec[i32] = Vec.new()
    original.append(1)
    var shared = original
    shared.append(2)
    shared[0] = 9
    println(f"{original.len()} {shared.len()} {original[0]} {shared[0]}")

    var tight = @vec[3]
    var grown = tight
    grown.append(4)
    grown[0] = 8
    println(f"{tight.len()} {grown.len()} {tight[0]} {grown[0]}")
    tight[0] = 7
    println(f"{tight[0]} {grown[0]}")
"#;

    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(stdout, "1 2 9 9\n1 2 3 8\n7 8\n");
        assert_eq!(stderr, "");
        assert_eq!(status, 0);
    }
}

#[test]
fn collection_iteration_copies_the_iterable_and_yielded_values() {
    let source = r#"
fn main() -> ():
    var values = @vec[1, 2, 3]
    var total = 0
    for value in values:
        total += value
        values[0] = 9
    println(f"{total} {values[0]}")

    let entries = @map{"a": 1, "b": 2}
    var map_total = 0
    for entry in entries:
        match entry:
            (key, value):
                map_total += value
    println(f"{map_total}")

    let tags = @set{1, 2, 2}
    var set_total = 0
    for tag in tags:
        set_total += tag
    println(f"{set_total}")
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(stdout, "6 9\n3\n3\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn iteration_snapshots_once_and_observes_nonstructural_alias_mutation() {
    let source = r#"
fn make_values() -> Vec[i32]:
    println("iterable evaluated")
    return @vec[1, 2, 3]

fn main() -> ():
    var values = make_values()
    var total = 0
    for value in values:
        if value == 1:
            values[1] = 20
        total += value
    println(total)

    var scores = @map{ "a": 1, "b": 2 }
    var map_total = 0
    for entry in scores:
        scores["a"] = 10
        scores["b"] = 10
        match entry:
            (_, value):
                map_total += value
    println(map_total == 11 || map_total == 12)
"#;

    let tree = TestTree::new("iteration-snapshot");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    let main = compilation
        .control_flow_ir
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main is lowered");
    assert!(
        main.logical_copy_inventory()
            .iter()
            .any(|copy| { copy.facts.context.kind == LogicalCopyKind::IterationSnapshot })
    );
    assert_eq!(
        main.blocks[0]
            .instructions
            .iter()
            .filter(|instruction| matches!(
                instruction,
                elamite::ir::Instruction::Assign {
                    value: elamite::ir::Rvalue::CollectionLength { .. },
                    ..
                }
            ))
            .count(),
        1,
        "the first loop's hidden length is captured before its condition block"
    );

    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(stdout, "iterable evaluated\n24\ntrue\n");
        assert_eq!(stderr, "");
        assert_eq!(status, 0);
    }
}

#[test]
fn structural_collection_mutation_during_iteration_compiles_as_documented_ub() {
    let source = r#"
fn main() -> ():
    var values = @vec[1]
    for value in values:
        values.append(value)

    var scores = @map{ "a": 1 }
    for entry in scores:
        let _ = scores.insert("b", entry.1)

    var tags = @set{1}
    for tag in tags:
        let _ = tags.remove(tag)
"#;
    let tree = TestTree::new("iteration-structural-mutation-ub");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    assert!(compilation.generated_c.contains("el_vec_append"));
    assert!(compilation.generated_c.contains("el_map_insert"));
    assert!(compilation.generated_c.contains("el_set_remove"));
}

#[test]
fn collection_comparison_and_copying_follow_shallow_value_semantics() {
    let source = r#"
fn copy_map(values: Map[str, i32]) -> Map[str, i32]:
    var result = values
    result["a"] = 9
    return result

fn main() -> ():
    let ascending = @vec[1, 2, 3]
    let later = @vec[1, 2, 4]
    println(f"{ascending == @vec[1, 2, 3]} {ascending < later}")

    var original_map = @map{"a": 1, "b": 2}
    let reordered_map = @map{"b": 2, "a": 1}
    let changed_map = copy_map(original_map)
    println(f"{original_map == reordered_map} {original_map["a"]} {changed_map["a"]}")

    var original_set = @set{"a", "b"}
    var copied_set = original_set
    copied_set.remove("a")
    println(f"{original_set == @set{"b", "a"}} {original_set.contains("a")} {copied_set.contains("a")}")
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(stdout, "true true\nfalse 9 9\nfalse false false\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn collection_iteration_honors_break_and_continue() {
    let source = r#"
fn main() -> ():
    var values = @vec[1, 2, 3, 4, 5]
    var total = 0
    for value in values:
        if value == 2:
            continue
        if value == 4:
            break
        total += value
        values[0] = 9
    println(f"{total} {values[0]}")
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(stdout, "4 9\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn user_iterators_drive_for_through_next_and_preserve_control_flow() {
    let source = r#"
struct Countdown:
    current: i32

impl Iterator[i32] for Countdown:
    fn next(self: &var Self) -> Option[i32]:
        if self.current <= 0:
            return Option.None
        let value = self.current
        self.current -= 1
        return Option.Some(value)

fn make_countdown() -> Countdown:
    println("iterator evaluated")
    return Countdown { current: 5 }

fn total[I: Iterator[i32]](iterator: I) -> i32:
    var result = 0
    for value in iterator:
        if value == 4:
            continue
        if value == 2:
            break
        result += value
    return result

fn first[E, I: Iterator[E]](iterator: I, fallback: E) -> E:
    for value in iterator:
        return value
    return fallback

fn main() -> ():
    println(total(make_countdown()))
    println(first(Countdown { current: 5 }, 0))
    var original = Countdown { current: 2 }
    var copied_total = 0
    for value in original:
        copied_total += value
    println(f"{copied_total} {original.current}")
"#;

    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(stdout, "iterator evaluated\n8\n5\n3 2\n");
        assert_eq!(stderr, "");
        assert_eq!(status, 0);
    }
}

#[test]
fn references_yielded_from_hidden_iterator_state_remain_valid() {
    let source = r#"
struct Once:
    value: i32
    yielded: bool

impl Iterator[&i32] for Once:
    fn next(self: &var Self) -> Option[&i32]:
        if self.yielded:
            return Option.None
        self.yielded = true
        return Option.Some(&self.value)

fn main() -> ():
    var saved: Option[&i32] = Option.None
    for value in Once { value: 42, yielded: false }:
        saved = Option.Some(value)
    match saved:
        Option.Some(value):
            println(*value)
        Option.None:
            println(0)
"#;

    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(stdout, "42\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn generic_iterators_can_yield_shallow_copied_closures() {
    let source = r#"
struct Once[T]:
    value: T
    yielded: bool

impl[T] Iterator[T] for Once[T]:
    fn next(self: &var Self) -> Option[T]:
        if self.yielded:
            return Option.None
        self.yielded = true
        return Option.Some(self.value)

fn main() -> ():
    let offset = 40
    let callback = fn[offset]() -> i32:
        return offset + 2
    var calls = 0
    for yielded in Once { value: callback, yielded: false }:
        calls += 1
        println(yielded())
    println(calls)
"#;

    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(stdout, "42\n1\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn user_iterator_ir_preserves_copy_facts_and_both_target_widths() {
    let source = r#"
struct Once:
    value: String
    yielded: bool

impl Iterator[String] for Once:
    fn next(self: &var Self) -> Option[String]:
        if self.yielded:
            return Option.None
        self.yielded = true
        return Option.Some(self.value)

fn main() -> ():
    for value in Once { value: String.from("value"), yielded: false }:
        println(value)
"#;
    let tree = TestTree::new("user-iterator-ir");
    tree.executable(source);

    for target in [Target::X86, Target::X86_64] {
        let mut sources = SourceManager::new();
        let graph = tree.graph(&mut sources);
        let compilation = compile(&graph, &mut sources, target)
            .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
        let main = compilation
            .control_flow_ir
            .functions
            .iter()
            .find(|function| function.name == "main")
            .expect("main is lowered");
        let copies = main.logical_copy_inventory();
        assert!(
            copies
                .iter()
                .any(|copy| { copy.facts.context.kind == LogicalCopyKind::IterationSnapshot })
        );
        assert!(
            copies
                .iter()
                .any(|copy| { copy.facts.context.kind == LogicalCopyKind::IterationElement })
        );
        assert!(
            main.blocks.iter().any(
                |block| block.instructions.iter().any(|instruction| matches!(
                    instruction,
                    elamite::ir::Instruction::Assign {
                        value: elamite::ir::Rvalue::AllocateManaged { .. },
                        ..
                    }
                ))
            )
        );
        assert!(compilation.control_flow_ir.requires_managed_memory);
    }
}

#[test]
fn formatted_strings_are_values_and_display_builtin_collections() {
    let source = r#"
struct Counter:
    value: i32

impl Counter:
    fn observe(self: &var Self, value: i32) -> i32:
        self.value += 1
        print(value)
        return value

fn make(value: i32) -> str:
    return f"value={value}, ok={true}"

fn main() -> ():
    let text = make(7)
    println(text)
    let values = @vec[1, 2, 3]
    let rendered = f"values={values}"
    println(rendered)
    let nested = @vec[@vec[1, 2], @vec[3]]
    println(f"{nested}")
    var counter = Counter { value: 0 }
    println(f"={counter.observe(4)}:{counter.observe(5)}:{counter.value}")
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(
        stdout,
        "value=7, ok=true\nvalues=[1, 2, 3]\n[[1, 2], [3]]\n45=4:5:2\n"
    );
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn identity_wrappers_are_stable_collection_keys() {
    let source = r#"
fn main() -> ():
    var first = 1
    var second = 1
    let first_ref = &var first
    let alias = &var first
    let second_ref = &var second
    let first_key = Identity[&var i32].from(first_ref)
    let alias_key = Identity[&var i32].from(alias)
    let second_key = Identity[&var i32].from(second_ref)
    var values = @map{first_key: 7}
    first = 9
    println(f"{values[alias_key]} {values.contains_key(second_key)}")
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(stdout, "7 false\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn user_display_implementations_write_through_formatter() {
    let source = r#"
struct Point:
    x: i32
    y: i32

impl Display for Point:
    fn fmt(self: &Self, formatter: &var Formatter) -> ():
        formatter.write(f"Point({self.x}, {self.y})")

fn main() -> ():
    let point = Point { x: 3, y: 4 }
    println(point)
    let text = f"wrapped {point}"
    println(text)
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(stdout, "Point(3, 4)\nwrapped Point(3, 4)\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn display_trait_objects_dispatch_through_the_formatter() {
    let source = r#"
struct Label:
    value: i32

impl Display for Label:
    fn fmt(self: &Self, formatter: &var Formatter) -> ():
        formatter.write(f"label={self.value}")

fn render(value: &Display) -> str:
    return f"<{value}>"

fn main() -> ():
    let label = Label { value: 8 }
    println(render(&label))
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(stdout, "<label=8>\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn text_preserves_unicode_and_embedded_nul_bytes() {
    let source = r#"
fn main() -> ():
    let first = "α\0β"
    let same = "α\0β"
    let later = "α\0γ"
    let owned = String.from(first)
    println(f"{first == same} {first < later} {owned == String.from(same)}")
    print(first)
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(stdout.as_bytes(), "true true true\nα\0β".as_bytes());
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn concatenation_preserves_order_and_creates_independent_values() {
    let source = r#"
fn observed(label: str, value: str) -> str:
    print(label)
    return value

fn main() -> ():
    let borrowed = observed("L", "hello ") ++ observed("R", "world")
    let owned = String.from("owned ") ++ String.from("text")
    var left = @vec[1, 2]
    var right = @vec[3, 4]
    let joined = left ++ right
    left.append(9)
    right.append(8)
    println(f"={borrowed}:{owned}:{left.len()}:{right.len()}:{joined.len()}")
    for value in joined:
        print(value)
    println("")
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(stdout, "LR=hello world:owned text:3:3:4\n1234\n");
        assert_eq!(stderr, "");
        assert_eq!(status, 0);
    }
}

#[test]
fn array_and_collection_empty_apis_return_typed_values() {
    let source = r#"
fn main() -> ():
    let array = [4, 5]
    println(f"{array.len()}")
    match array.get(1):
        Option.Some(value):
            println(f"{value}")
        Option.None:
            println("none")
    match array.get(9):
        Option.Some(value):
            println(f"{value}")
        Option.None:
            println("none")

    let values: Vec[i32] = Vec.new()
    let names: Map[str, i32] = Map.default()
    let tags: Set[str] = Set.new()
    println(f"{values.is_empty()} {names.is_empty()} {tags.is_empty()}")
    println(f"{i32.default()}:{bool.default()}:{str.default()}:{String.default()}")
"#;
    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(stdout, "2\n5\nnone\ntrue true true\n0:false::\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
}

#[test]
fn numeric_alternative_conversions_wrap_and_saturate() {
    let source = r#"
fn main() -> ():
    println(u8.wrapping_from(300))
    println(u8.wrapping_from(-1))
    println(i8.wrapping_from(200))
    println(u8.saturating_from(300))
    println(u8.saturating_from(-1))
    println(i8.saturating_from(200))
    println(i8.saturating_from(-200))
    println(i64.wrapping_from(7u8))
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "44\n255\n-56\n255\n0\n127\n-128\n7\n");
    }
}

#[test]
fn only_wrapping_division_can_still_trap() {
    // Every alternative avoids the trapping path except division and
    // remainder by zero, which have no wrapped answer.
    let tree = TestTree::new("alternative-traps");
    tree.executable(
        "fn main() -> ():\n\
         \x20\x20\x20\x20println(1i32.wrapping_add(2i32))\n\
         \x20\x20\x20\x20println(1i32.saturating_mul(2i32))\n\
         \x20\x20\x20\x20println(1i32.wrapping_shl(2i32))\n",
    );
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    for helper in compilation.generated_c.split("\nstatic ").filter(|chunk| {
        chunk.starts_with("int32_t el_wrapping_") || chunk.contains("el_saturating_")
    }) {
        let body = &helper[..helper.find("\n}").unwrap_or(helper.len())];
        assert!(!body.contains("el_trap"), "must not trap:\n{body}");
    }
}

#[test]
fn propagation_branches_evaluate_once_and_copy_payloads() {
    // M15.3: the `?` operand is evaluated exactly once; an `Ok` payload is
    // copied into the expression's value and an `Err` payload is copied into
    // an early `Result.Err` return (`docs/spec.md` 8).
    let source = r#"
struct Counter:
    value: i32

fn tick(label: str, fail: bool) -> Result[Counter, str]:
    println(f"eval {label}")
    if fail:
        return Result.Err("failed")
    return Result.Ok(Counter { value: 1 })

fn probe(fail: bool) -> Result[i32, str]:
    let source = tick("operand", fail)
    var copied = source?
    copied.value = 99
    match source:
        Result.Ok(counter):
            println(f"original {counter.value}")
        Result.Err(message):
            println(f"original err {message}")
    return Result.Ok(copied.value)

fn describe(value: Result[i32, str]) -> ():
    match value:
        Result.Ok(inner):
            println(f"ok {inner}")
        Result.Err(message):
            println(f"err {message}")

fn main() -> ():
    describe(probe(false))
    describe(probe(true))
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        // The mutated `?` copy never leaks back into the source `Result`,
        // and each operand evaluates exactly once per call.
        assert_eq!(
            stdout,
            "eval operand\noriginal 1\nok 99\neval operand\nerr failed\n"
        );
    }
}

#[test]
fn deferred_registrations_run_in_reverse_on_every_exit_edge() {
    // M15.6-15.8: registrations execute in reverse order at scope exit, a
    // `defer:` body runs forward as one unit, inner scopes run before outer
    // ones, and `break`/`continue` exit exactly the loop-body scopes.
    let source = r#"
fn fallthrough_and_blocks() -> ():
    defer println("outer call")
    defer:
        println("block first")
        println("block second")
    if true:
        defer println("inner scope")
        println("inner body")
    println("outer body")

fn return_edge(early: bool) -> i32:
    defer println("return cleanup")
    if early:
        defer println("branch cleanup")
        return 1
    return 2

fn loop_edges() -> ():
    var index = 0
    while index < 3:
        index += 1
        defer println(f"iter {index} cleanup")
        if index == 2:
            continue
        if index == 3:
            break
        println(f"iter {index} body")
    println("after loop")

fn main() -> ():
    fallthrough_and_blocks()
    println(f"returned {return_edge(true)}")
    println(f"returned {return_edge(false)}")
    loop_edges()
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(
            stdout,
            "inner body\ninner scope\nouter body\nblock first\nblock second\nouter call\n\
             branch cleanup\nreturn cleanup\nreturned 1\n\
             return cleanup\nreturned 2\n\
             iter 1 body\niter 1 cleanup\niter 2 cleanup\niter 3 cleanup\nafter loop\n"
        );
    }
}

#[test]
fn a_returned_shared_handle_observes_its_deferred_close() {
    // M15.7: the return value is copied before cleanup begins, so
    // unconditionally deferring `close()` on a returned shared handle closes
    // the returned copy too (`docs/spec.md` 8): the copy shares the handle's
    // explicit alias even though the copy itself happened first.
    let source = r#"
struct Handle:
    state: &var i32

impl Handle:
    fn close(self: &Self) -> ():
        *self.state = 0

fn open_and_return() -> Handle:
    var cell = 1
    let handle = Handle { state: &var cell }
    defer handle.close()
    return handle

fn main() -> ():
    let handle = open_and_return()
    println(*handle.state)
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "0\n");
    }
}

#[test]
fn propagation_runs_cleanup_for_every_exited_scope() {
    // M15.9: an `Err` propagation copies its error, then runs every exited
    // scope's registrations inner-to-outer before returning.
    let source = r#"
fn source(fail: bool) -> Result[i32, str]:
    if fail:
        return Result.Err("inner failure")
    return Result.Ok(1)

fn run(fail: bool) -> Result[i32, str]:
    defer println("outer cleanup")
    if true:
        defer println("inner cleanup")
        let value = source(fail)?
        println(f"inner value {value}")
    println("after inner scope")
    return Result.Ok(0)

fn main() -> ():
    match run(false):
        Result.Ok(value):
            println(f"ok {value}")
        Result.Err(message):
            println(f"err {message}")
    match run(true):
        Result.Ok(value):
            println(f"ok {value}")
        Result.Err(message):
            println(f"err {message}")
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(
            stdout,
            "inner value 1\ninner cleanup\nafter inner scope\nouter cleanup\nok 0\n\
             inner cleanup\nouter cleanup\nerr inner failure\n"
        );
    }
}

#[test]
fn deferred_bodies_read_execution_time_values() {
    // M15.10: a deferred expression reads the values its bindings have when
    // the registration executes. Rebinding a `var` after registration changes
    // the later call; a `let` continues to identify the original value, and a
    // managed target named by deferred syntax stays alive until it runs.
    let source = r#"
fn report(label: str, value: i32) -> ():
    println(f"{label} {value}")

fn main() -> ():
    var counter = 1
    let snapshot = counter
    defer report("var", counter)
    defer report("let", snapshot)
    counter = 2
    var cell = 10
    let alias = &var cell
    defer report("managed", *alias)
    cell = 11
    println("body done")
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "body done\nmanaged 11\nlet 1\nvar 2\n");
    }
}

#[test]
fn deferred_execution_is_static_and_constructs_no_callable() {
    // M15.4: registration is a static cleanup plan, not a runtime list. Each
    // exit edge re-lowers the registered bodies in place, so the deferred
    // call's C appears once per exit edge and no callable or environment
    // value is ever constructed.
    let source = r#"
fn helper() -> ():
    println("cleanup")

fn probe(flag: bool) -> ():
    defer helper()
    if flag:
        return
    println("body")

fn main() -> ():
    probe(true)
    probe(false)
"#;
    let tree = TestTree::new("static-defer");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    let helper_symbol = compilation
        .generated_c
        .lines()
        .find_map(|line| {
            let start = line.find("el_p")?;
            let rest = &line[start..];
            let end = rest.find('(')?;
            rest[..end]
                .contains("helper")
                .then(|| rest[..end].to_string())
        })
        .expect("the deferred helper is emitted");
    let calls = compilation
        .generated_c
        .matches(&format!("{helper_symbol}()"))
        .count();
    // Two exit edges (the early return and the fallthrough), no more: nothing
    // is emitted at the registration point itself.
    assert_eq!(calls, 2, "{}", compilation.generated_c);
    // No function pointer to the helper is ever stored, so no callable value
    // backs the registration.
    assert!(
        !compilation
            .generated_c
            .contains(&format!("= {helper_symbol};")),
        "{}",
        compilation.generated_c
    );

    let (stdout, stderr, status) = build_and_run(source, Optimization::Debug);
    assert_eq!(status, 0, "{stderr}");
    assert_eq!(stdout, "cleanup\nbody\ncleanup\n");
}

#[test]
fn traps_terminate_without_promising_remaining_cleanup() {
    // M15.11: a trap terminates through the existing runtime path. Deferred
    // registrations are not an unwinding mechanism, so this asserts only
    // termination and the output that provably preceded the trap.
    let (stdout, stderr, status) = build_and_run(
        r#"
fn main() -> ():
    defer println("unreached cleanup promise")
    println("before trap")
    var zero = 0
    println(10 / zero)
"#,
        Optimization::Debug,
    );
    assert_eq!(status, 101);
    assert!(stderr.contains("E-RUN-DIVZERO"), "{stderr}");
    assert!(stdout.starts_with("before trap\n"), "{stdout}");

    // A trap *during* deferred execution also terminates; the remaining
    // registration's execution is deliberately not asserted either way.
    let (stdout, stderr, status) = build_and_run(
        r#"
fn main() -> ():
    defer println("earlier registration")
    defer:
        println("later registration runs first")
        var zero = 0
        println(10 / zero)
    println("body")
"#,
        Optimization::Debug,
    );
    assert_eq!(status, 101);
    assert!(stderr.contains("E-RUN-DIVZERO"), "{stderr}");
    assert!(
        stdout.starts_with("body\nlater registration runs first\n"),
        "{stdout}"
    );
}

#[test]
fn spec_demo_error_and_cleanup_regions_build_and_run() {
    // M15.12: the error and resource-cleanup regions of the authoritative
    // demonstration, together with methods, shared handles, formatting, and
    // `io.IoError` propagation. The full demonstration remains Milestone
    // 19.1's fixture.
    let source = r#"
use std.io

struct DemoResourceState:
    closed: bool

struct DemoResource:
    // Copies share state because the handle explicitly contains a reference.
    state: &var DemoResourceState

impl DemoResource:
    fn is_closed(self: &Self) -> bool:
        return self.state.closed

    fn close(self: &Self) -> ():
        self.state.closed = true

fn make_demo_resource() -> DemoResource:
    var state = DemoResourceState{closed: false}
    return DemoResource{state: &var state}

fn use_demo_resource(resource: DemoResource):
    defer resource.close()
    println(f"resource closed: {resource.is_closed()}") // false

fn use_demo_resource_block(resource: DemoResource):
    defer:
        let closing = "closing demo resource"
        println(closing)
        resource.close()
    println(f"resource closed: {resource.is_closed()}") // false

fn propagate_io(result: Result[i32, io.IoError]) -> Result[i32, io.IoError]:
    // `?` propagates only the exact enclosing error type.
    let value = result?
    return Result.Ok(value)

fn run() -> Result[(), io.IoError]:
    let resource = make_demo_resource()
    use_demo_resource(resource)
    println(f"after call closed: {resource.is_closed()}")

    let second = make_demo_resource()
    use_demo_resource_block(second)
    println(f"after block closed: {second.is_closed()}")

    let propagated = propagate_io(Result.Ok(42))?
    println(f"propagated value: {propagated}")
    let failed = propagate_io(Result.Err(io.IoError.Other))?
    println(f"unreached: {failed}")
    return Result.Ok(())

fn main() -> ():
    match run():
        Result.Ok(unit):
            println("demo ok")
        Result.Err(error):
            println("demo err propagated")
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(
            stdout,
            "resource closed: false\nafter call closed: true\n\
             resource closed: false\nclosing demo resource\nafter block closed: true\n\
             propagated value: 42\ndemo err propagated\n"
        );
    }
}

#[test]
fn raw_pointers_read_write_and_compare_by_address() {
    // M16.1/M16.6: the authoritative demonstration's pointer region — null
    // comparison, safe conversions, unsafe recovery, and writes through
    // `*var T` observable in the original storage.
    let source = r#"
fn main() -> ():
    var value = 41
    let edit: &var i32 = &var value
    let pointer: *var i32 = edit as *var i32

    if pointer != null:
        unsafe:
            let recovered: &var i32 = pointer as &var i32
            *recovered = 42
    println(value)

    unsafe:
        *pointer = 43
        *pointer += 1
        println(*pointer)
    println(value)

    // Copies and downgrades preserve the address, so equality holds.
    let copied = pointer
    let shared: *i32 = pointer as *i32
    println(copied == pointer)
    println(shared == (pointer as *i32))

    var other = 1
    let elsewhere: *i32 = (&other) as *i32
    println(elsewhere == shared)

    let absent: *i32 = null
    println(absent == null)
    println(shared == null)
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "42\n44\n44\ntrue\ntrue\nfalse\ntrue\nfalse\n");
    }
}

#[test]
fn raw_pointer_null_and_alignment_checks_trap() {
    // M16.8: every executed raw dereference and raw-to-reference conversion
    // checks null and alignment first, with stable codes and locations. The
    // null pointers hide behind a call so the expression-local rule (M16.7)
    // does not reject the program statically.
    let (stdout, stderr, status) = build_and_run(
        r#"
fn conceal(pointer: *i32) -> *i32:
    return pointer

fn main() -> ():
    println("before")
    let hidden = conceal(null)
    unsafe:
        println(*hidden)
"#,
        Optimization::Debug,
    );
    assert_eq!(status, 101);
    assert_eq!(stdout, "before\n");
    assert!(stderr.contains("E-RUN-NULL"), "{stderr}");
    assert!(stderr.contains("main.elx:9:"), "{stderr}");

    // Positional selection through a raw tuple pointer has the same mandatory
    // pointer check as explicit dereference.
    let (stdout, stderr, status) = build_and_run(
        r#"
fn conceal(pointer: *(i32, i32)) -> *(i32, i32):
    return pointer

fn main() -> ():
    println("before")
    let hidden = conceal(null)
    unsafe:
        println(hidden.0)
"#,
        Optimization::Debug,
    );
    assert_eq!(status, 101);
    assert_eq!(stdout, "before\n");
    assert!(stderr.contains("E-RUN-NULL"), "{stderr}");
    assert!(stderr.contains("main.elx:9:"), "{stderr}");

    let (stdout, stderr, status) = build_and_run(
        r#"
fn conceal(pointer: *i32) -> *i32:
    return pointer

fn main() -> ():
    println("before")
    let hidden = conceal(null)
    unsafe:
        let bad = hidden as &i32
        println(*bad)
"#,
        Optimization::Debug,
    );
    assert_eq!(status, 101);
    assert_eq!(stdout, "before\n");
    assert!(stderr.contains("E-RUN-NULL"), "{stderr}");

    // A pointee-changing cast preserves the address, so a `*u8` into the
    // second byte of a managed cell is misaligned for `u16`. The cell comes
    // from the managed allocator, whose alignment exceeds two.
    let (stdout, stderr, status) = build_and_run(
        r#"
struct Bytes:
    lead: u8
    trail: u8

fn main() -> ():
    println("before")
    var bytes = Bytes { lead: 1, trail: 2 }
    let trail: *var u8 = (&var bytes.trail) as *var u8
    unsafe:
        let wide: *var u16 = trail as *var u16
        println(*wide)
"#,
        Optimization::Debug,
    );
    assert_eq!(status, 101);
    assert_eq!(stdout, "before\n");
    assert!(stderr.contains("E-RUN-ALIGN"), "{stderr}");
}

#[test]
fn raw_pointer_arithmetic_indexing_and_distance_lower_to_c99() {
    let source = r#"
fn select(pointer: *var i32, calls: &var i32) -> *var i32:
    *calls += 1
    return pointer

fn select_index(calls: &var i32) -> isize:
    *calls += 1
    return 1

fn main() -> ():
    var values = [10, 20, 30, 40]
    var pointer_calls = 0
    var index_calls = 0
    let whole: *var [i32; 4] = (&var values) as *var [i32; 4]
    unsafe:
        let first: *var i32 = whole as *var i32
        select(first, &var pointer_calls)[select_index(&var index_calls)] = 25
        println(f"{values[1]}:{pointer_calls}:{index_calls}")

        let third = first + 2
        println(*third)
        let one_past = first + 4
        println(one_past - first)

        var cursor = first
        cursor += 3
        cursor -= 1
        println(cursor[0])
        println(cursor[-1])

        let shared_one_past: *i32 = one_past as *i32
        println(shared_one_past - first)
"#;

    let tree = TestTree::new("raw-pointer-arithmetic");
    tree.executable(source);
    for target in [Target::X86, Target::X86_64] {
        let mut sources = SourceManager::new();
        let graph = tree.graph(&mut sources);
        let compilation = compile(&graph, &mut sources, target)
            .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
        assert!(compilation.generated_c.contains("intptr_t"));
        assert!(compilation.generated_c.contains("el_check_ptr_t"));
        assert!(compilation.generated_c.contains(" - "));
    }

    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "25:1:1\n30\n4\n30\n25\n4\n");
        assert_eq!(stderr, "");
    }
}

#[test]
fn raw_pointer_relational_ordering_handles_null_without_ordering_it_in_c() {
    let source = r#"
fn main() -> ():
    var values = [1, 2, 3, 4]
    let whole: *var [i32; 4] = (&var values) as *var [i32; 4]
    unsafe:
        let first: *var i32 = whole as *var i32
        let end = first + 4
        var cursor = first
        var sum = 0
        while cursor < end:
            sum += *cursor
            cursor += 1
        println(sum)

        let middle = first + 2
        let shared_middle: *i32 = middle as *i32
        println(f"{first < middle}:{first <= middle}:{middle > first}:{middle >= shared_middle}")

        let none: *i32 = null
        println(f"{none < first}:{none <= first}:{first > none}:{first >= none}")
        println(f"{none < null}:{none <= null}:{none > null}:{none >= null}")
        println(f"{first < none}:{first <= none}:{none > first}:{none >= first}")
"#;

    let tree = TestTree::new("raw-pointer-ordering");
    tree.executable(source);
    for target in [Target::X86, Target::X86_64] {
        let mut sources = SourceManager::new();
        let graph = tree.graph(&mut sources);
        let compilation = compile(&graph, &mut sources, target)
            .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
        assert!(compilation.generated_c.contains("const unsigned char *"));
        assert!(compilation.generated_c.contains("== NULL"));
        assert!(compilation.generated_c.contains("!= NULL"));
        for invalid in [" < NULL", " <= NULL", " > NULL", " >= NULL"] {
            assert!(!compilation.generated_c.contains(invalid), "{invalid}");
        }
    }

    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(
            stdout,
            "10\ntrue:true:true:true\ntrue:true:true:true\nfalse:true:false:true\nfalse:false:false:false\n"
        );
        assert_eq!(stderr, "");
    }
}

#[test]
fn raw_pointer_indexing_uses_existing_null_and_alignment_traps() {
    let (stdout, stderr, status) = build_and_run(
        r#"
fn conceal(pointer: *i32) -> *i32:
    return pointer

fn main() -> ():
    println("before")
    let hidden = conceal(null)
    unsafe:
        println(hidden[0])
"#,
        Optimization::Debug,
    );
    assert_eq!(status, 101);
    assert_eq!(stdout, "before\n");
    assert!(stderr.contains("E-RUN-NULL"), "{stderr}");

    let (stdout, stderr, status) = build_and_run(
        r#"
fn main() -> ():
    println("before")
    var bytes = [1u8, 2u8, 3u8, 4u8, 5u8, 6u8, 7u8, 8u8]
    let whole: *var [u8; 8] = (&var bytes) as *var [u8; 8]
    unsafe:
        let first: *var u8 = whole as *var u8
        let misaligned: *u32 = (first + 1) as *u32
        println(misaligned[0])
"#,
        Optimization::Debug,
    );
    assert_eq!(status, 101);
    assert_eq!(stdout, "before\n");
    assert!(stderr.contains("E-RUN-ALIGN"), "{stderr}");
}

#[test]
fn raw_to_reference_conversion_restores_a_strong_managed_path() {
    // M16.9/M16.10: a raw pointer is not a root, but a validly recovered
    // reference is. The target storage was promoted when its address was
    // taken, and reading through the recovered reference after the source
    // frame exits observes the value written through the pointer.
    let source = r#"
struct Cell:
    value: i32

unsafe fn recover(pointer: *var Cell) -> &var Cell:
    unsafe:
        return pointer as &var Cell

fn produce() -> &var Cell:
    var cell = Cell { value: 1 }
    let pointer: *var Cell = (&var cell) as *var Cell
    unsafe:
        let recovered = recover(pointer)
        recovered.value = 2
        return recovered

fn main() -> ():
    let alias = produce()
    println(alias.value)
    alias.value += 1
    println(alias.value)
"#;
    let tree = TestTree::new("raw-recover");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    // The address-taken local is promoted to managed storage, and the
    // conversion site checks the pointer before forming the reference.
    assert!(compilation.generated_c.contains("GC_MALLOC"));
    assert!(
        compilation.generated_c.contains("el_check_ptr_t"),
        "{}",
        compilation.generated_c
    );

    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "2\n3\n");
    }
}

#[test]
fn unsafe_methods_and_references_follow_the_demo_region() {
    // M16.3/M16.4/M16.6: raw-pointer receivers accept only an exactly
    // matching pointer with no adaptation; selecting an unsafe method
    // unbound preserves its safety qualifier; both invocation forms need
    // `unsafe:`; function-reference identity compares by target.
    let source = r#"
struct Session:
    active: bool

impl Session:
    pub fn get_const_self_ptr(self: &Self) -> *Self:
        return self as *Self

    unsafe pub fn get_self_ptr_unsafe(self: *Self) -> &Self:
        unsafe:
            return self as &Self

    fn status(self: &Self) -> str:
        if self.active:
            return "active"
        return "inactive"

fn main() -> ():
    let observer = Session { active: true }
    let observer_ptr: *Session = observer.get_const_self_ptr()
    let recover_observer: &unsafe fn(*Session) -> &Session =
        Session.get_self_ptr_unsafe
    unsafe:
        let recovered_bound = observer_ptr.get_self_ptr_unsafe()
        let recovered_unbound = recover_observer(observer_ptr)
        println(f"bound: {recovered_bound.status()}")
        println(f"unbound: {recovered_unbound.status()}")
    println(recover_observer == Session.get_self_ptr_unsafe)
"#;
    for optimization in [Optimization::Debug, Optimization::Release] {
        let (stdout, stderr, status) = build_and_run(source, optimization);
        assert_eq!(status, 0, "{stderr}");
        assert_eq!(stdout, "bound: active\nunbound: active\ntrue\n");
    }
}
