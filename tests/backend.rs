use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::backend::{COptions, Target, emit_c};
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
fn recursive_copy_helpers_include_owned_strings() {
    let source = r#"
struct Label:
    text: String

fn echo(value: Label) -> Label:
    return value

fn main() -> ():
    let original = Label { text: "copied" }
    let result = echo(original)
    println(result.text)
"#;
    let tree = TestTree::new("string-copy");
    tree.executable(source);
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let compilation = compile(&graph, &mut sources, Target::X86_64)
        .unwrap_or_else(|diagnostics| panic!("{}", render(&sources, &diagnostics)));
    assert!(
        compilation
            .generated_c
            .contains("return el_copy_string(value);")
    );
    assert!(compilation.generated_c.contains("result.f0 = el_copy_t"));

    let (stdout, stderr, status) = build_and_run(source, Optimization::Release);
    assert_eq!(stdout, "copied\n");
    assert_eq!(stderr, "");
    assert_eq!(status, 0);
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
fn executable_subset_rejects_for_at_the_lowering_boundary() {
    let tree = TestTree::new("unsupported-lowering");
    tree.executable("fn main() -> ():\n    for value in [1, 2]:\n        println(value)\n");
    let mut sources = SourceManager::new();
    let graph = tree.graph(&mut sources);
    let diagnostics = compile(&graph, &mut sources, Target::X86_64)
        .err()
        .expect("`for` must remain outside the executable subset until Milestone 14");
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.category == Category::Lowering
                && diagnostic.message.contains("`for` lowering")
                && diagnostic.primary.is_some()
        }),
        "{}",
        render(&sources, &diagnostics)
    );
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
fn references_observe_storage_through_binding_and_path() {
    // SPEC 3.2 / I-018: a reference names storage. A binding reference sees
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
    // IMPL Milestone 10: collection is best-effort and its *timing* is not a
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
    // IMPL Milestone 13: vtable dispatch with several concrete types behind one
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
    println(f"{measure(sq as &Shape)},{measure(rc as &Shape)}")
    println(f"{describe(sq as &Shape)},{describe(rc as &Shape)}")
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
         \x20\x20\x20\x20println(measure(reference as &Shape))\n",
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
