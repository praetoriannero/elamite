use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::check::{CheckOutput, check};
use elamite::diagnostics::{Category, Diagnostic};
use elamite::package::PackageGraph;
use elamite::resolution::{ResolvedProgram, resolve};
use elamite::source::SourceManager;
use elamite::types::resolve_types;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestTree {
    root: PathBuf,
}

impl TestTree {
    fn new(name: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "elamite-check-{}-{name}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create test tree");
        Self { root }
    }

    fn package(&self, relative: &str, files: &[(&str, &str)]) -> PathBuf {
        self.package_with_dependencies(relative, "exe", &[], files)
    }

    fn package_with_dependencies(
        &self,
        relative: &str,
        target_kind: &str,
        dependencies: &[(&str, &str)],
        files: &[(&str, &str)],
    ) -> PathBuf {
        let directory = self.root.join(relative);
        fs::create_dir_all(directory.join("src")).expect("create package source directory");
        let mut manifest = format!(
            "[package]\nname = \"{relative}\"\nversion = \"0.1.0\"\ntarget_kind = \
             \"{target_kind}\"\n"
        );
        for (alias, path) in dependencies {
            manifest.push_str(&format!("\n[dependencies.{alias}]\npath = \"{path}\"\n"));
        }
        fs::write(directory.join("elamite.toml"), manifest).expect("write manifest");
        for (path, source) in files {
            let path = directory.join(path);
            fs::create_dir_all(path.parent().expect("source file has parent"))
                .expect("create source parent");
            fs::write(path, source).expect("write source");
        }
        directory
    }
}

impl Drop for TestTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn resolve_package(package: &Path) -> (SourceManager, ResolvedProgram) {
    let mut sources = SourceManager::new();
    let graph = PackageGraph::resolve(&package.join("elamite.toml"), &mut sources)
        .expect("package graph should resolve");
    let output = resolve(&graph, &mut sources);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        render(&sources, &output.diagnostics)
    );
    (sources, output.program)
}

fn render(sources: &SourceManager, diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| {
            diagnostic.primary.map_or_else(
                || diagnostic.message.clone(),
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

/// Runs a source file through Milestones 4-6 and returns its Milestone 6
/// diagnostics. Panics (with rendered diagnostics) if an earlier milestone
/// already rejects the source, since these fixtures are meant to isolate
/// Milestone 6 behavior.
fn check_output(source: &str) -> (SourceManager, CheckOutput) {
    let tree = TestTree::new("case");
    let package = tree.package("demo", &[("src/main.elx", source)]);
    check_package_output(&package)
}

fn check_package_output(package: &Path) -> (SourceManager, CheckOutput) {
    let (sources, resolved) = resolve_package(package);
    let mut typed = resolve_types(&resolved);
    assert!(
        typed.diagnostics.is_empty(),
        "{}",
        render(&sources, &typed.diagnostics)
    );
    let checked = check(&resolved, &mut typed.program);
    (sources, checked)
}

fn check_diagnostics(source: &str) -> (SourceManager, Vec<Diagnostic>) {
    let (sources, checked) = check_output(source);
    (sources, checked.diagnostics)
}

fn assert_no_diagnostics(source: &str) {
    let (sources, diagnostics) = check_diagnostics(source);
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics, got:\n{}",
        render(&sources, &diagnostics)
    );
}

fn assert_has_category(source: &str, category: Category) {
    let (sources, diagnostics) = check_diagnostics(source);
    let diagnostic = diagnostics.iter().find(|d| d.category == category);
    assert!(
        diagnostic.is_some(),
        "expected a {category:?} diagnostic, got:\n{}",
        render(&sources, &diagnostics)
    );
    let span = diagnostic
        .and_then(|diagnostic| diagnostic.primary)
        .expect("checked diagnostics must carry a primary source span");
    assert!(
        span.start <= span.end && span.end <= source.len() as u32,
        "expected a meaningful in-file span, got {span:?}"
    );
}

#[test]
fn the_authoritative_demonstration_produces_no_false_positive_diagnostics() {
    // Milestone 11 checks non-generic inherent methods and function
    // references. Generics and traits remain Milestones 12-13, so this still
    // asserts that deferred constructs are not misdiagnosed.
    assert_no_diagnostics(include_str!("../examples/spec_demo.elx"));
}

#[test]
fn checks_arity_and_argument_types_of_a_direct_named_call() {
    assert_no_diagnostics(
        r#"
fn add(left: i32, right: i32) -> i32:
    return left + right

fn main() -> ():
    let sum = add(1, 2)
    println(f"{sum}")
"#,
    );
    assert_has_category(
        r#"
fn add(left: i32, right: i32) -> i32:
    return left + right

fn main() -> ():
    let sum = add(1)
    println(f"{sum}")
"#,
        Category::Call,
    );
    assert_has_category(
        r#"
fn add(left: i32, right: i32) -> i32:
    return left + right

fn main() -> ():
    let sum = add(1, true)
    println(f"{sum}")
"#,
        Category::ExpressionType,
    );
}

#[test]
fn checks_variadic_call_arity() {
    assert_no_diagnostics(
        r#"
fn variadic(x: i32, y: ...String) -> ():
    pass

fn main() -> ():
    variadic(1)
    variadic(1, String.from("a"), String.from("b"))
"#,
    );
}

#[test]
fn checks_closure_captures_return_inference_and_callable_bounds() {
    assert_no_diagnostics(
        r#"
fn apply[F: Callable[(i32,), i32]](callback: F, value: i32) -> i32:
    return callback(value)

fn invoke(callback: &Callable[(i32,), i32], value: i32) -> i32:
    return callback(value)

fn increment(value: i32) -> i32:
    return value + 1

struct Multiplier:
    factor: i32

impl Callable[(i32,), i32] for Multiplier:
    fn call(self: &Self, arguments: (i32,)) -> i32:
        return arguments.0 * self.factor

fn main() -> ():
    let offset: i32 = 4
    var total: i32 = 0
    let add = fn[offset, &var total as state](value: i32):
        *state += value
        return value + offset
    println(apply(add, 3))
    println(apply(increment, 4))
    println(invoke(&add, 5))
    let multiplier = Multiplier { factor: 2 }
    println(multiplier(6))
    println(invoke(&multiplier, 7))
"#,
    );

    assert_has_category(
        r#"
fn main() -> ():
    let pointer: *i32 = null
    let invalid = fn[pointer]():
        pass
"#,
        Category::ExpressionType,
    );

    assert_has_category(
        r#"
fn main() -> ():
    let value: i32 = 1
    let invalid = fn[&var value]():
        pass
"#,
        Category::Place,
    );

    assert_has_category(
        r#"
fn apply[F: Callable[(i32,), i32]](callback: F, value: i32) -> i32:
    return callback(value)

fn main() -> ():
    let wrong = fn(value: bool) -> bool:
        return value
    println(apply(wrong, 1))
"#,
        Category::TypeSystem,
    );

    assert_has_category(
        r#"
fn main() -> ():
    let value: i32 = 1
    let invalid = fn[value]() -> i32:
        value = 2
        return value
"#,
        Category::Place,
    );

    assert_has_category(
        r#"
fn main() -> ():
    let pointer: *i32 = null
    unsafe:
        let invalid = fn[*pointer]():
            println(*pointer)
"#,
        Category::UnsafeContext,
    );

    assert_has_category(
        r#"
fn main() -> ():
    let invalid = fn() -> i32:
        pass
"#,
        Category::ControlFlow,
    );
}

#[test]
fn checks_methods_receiver_adaptation_and_function_references() {
    assert_no_diagnostics(
        r#"
struct Counter:
    value: i32

    fn new(value: i32) -> Self:
        return Self{value: value}

    fn copied(self: Self) -> i32:
        return self.value

    fn shared(self: &Self) -> i32:
        return self.value

    fn mutable(self: &var Self, value: i32) -> ():
        self.value = value

    fn raw(self: *Self) -> ():
        pass

    fn raw_mut(self: *var Self) -> ():
        pass

fn increment(value: i32) -> i32:
    return value + 1

fn apply(callback: &fn(i32) -> i32, value: i32) -> i32:
    return callback(value)

enum Callback:
    One(&fn(i32) -> i32)

fn main() -> ():
    var counter = Counter.new(1)
    let shared = &counter
    let mutable = &var counter
    let raw: *Counter = shared as *Counter
    let raw_mut: *var Counter = mutable as *var Counter
    let from_value = counter.copied()
    let from_computed = Counter.new(2).copied()
    let from_shared = counter.shared()
    let from_reference = shared.shared()
    counter.mutable(3)
    mutable.mutable(4)
    raw.raw()
    raw_mut.raw_mut()
    let unbound: &fn(&var Counter, i32) -> () = Counter.mutable
    unbound(&var counter, 5)
    let callback: &fn(i32) -> i32 = increment
    let wrapped = Callback.One(increment)
    let callbacks = [increment, increment]
    let result = apply(callback, from_value + from_computed + from_shared + from_reference)
    println(result)
"#,
    );
}

#[test]
fn rejects_invalid_receivers_bound_method_values_and_function_signatures() {
    assert_has_category(
        r#"
struct Counter:
    value: i32

    fn update(self: &var Self) -> ():
        pass

fn main() -> ():
    let counter = Counter{value: 1}
    counter.update()
"#,
        Category::Call,
    );
    assert_has_category(
        r#"
struct Counter:
    value: i32

    fn read(self: &Self) -> i32:
        return self.value

fn main() -> ():
    let counter = Counter{value: 1}
    let method = counter.read
"#,
        Category::Call,
    );
    assert_has_category(
        r#"
unsafe fn dangerous(value: i32) -> i32:
    return value

fn main() -> ():
    let callback: &fn(i32) -> i32 = dangerous
"#,
        Category::ExpressionType,
    );
    assert_has_category(
        r#"
fn increment(value: i32) -> i32:
    return value + 1

fn main() -> ():
    let callback: fn(i32) -> i32 = increment
"#,
        Category::TypeSystem,
    );
    assert_has_category(
        r#"
struct Counter:
    value: i32

    fn raw(self: *Self) -> ():
        pass

fn main() -> ():
    let pointer: *var Counter = null
    pointer.raw()
"#,
        Category::Call,
    );
    assert_has_category(
        r#"
fn variadic(values: ...i32) -> ():
    pass

fn main() -> ():
    let callback: &fn(i32) -> () = variadic
"#,
        Category::ExpressionType,
    );
}

#[test]
fn checks_return_type_and_unit_fallthrough_rules() {
    assert_no_diagnostics(
        r#"
fn answer() -> i32:
    return 42

fn main() -> ():
    let value = answer()
    println(f"{value}")
"#,
    );
    assert_has_category(
        r#"
fn answer() -> i32:
    return true
"#,
        Category::ExpressionType,
    );
    assert_has_category(
        r#"
fn answer() -> i32:
    return
"#,
        Category::ExpressionType,
    );
    assert_has_category(
        r#"
fn answer() -> ():
    return 1
"#,
        Category::ExpressionType,
    );
}

#[test]
fn checks_never_functions_and_bottom_compatibility() {
    assert_no_diagnostics(
        r#"
fn stop(message: str) -> !:
    panic(message)

fn require_value(ready: bool) -> i32:
    if ready:
        return 42
    stop("missing value")

fn main() -> ():
    println(f"{require_value(true)}")
"#,
    );
    assert_has_category(
        r#"
fn bad() -> !:
    pass
"#,
        Category::ControlFlow,
    );
    assert_has_category(
        r#"
fn bad() -> !:
    return
"#,
        Category::ControlFlow,
    );
    assert_has_category(
        r#"
fn bad() -> !:
    return panic("nested")
"#,
        Category::ControlFlow,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let impossible: ! = panic("stop")
"#,
        Category::TypeSystem,
    );
    assert_has_category(
        r#"
fn stop() -> !:
    panic("stop")

fn value() -> i32:
    return 1

fn main() -> ():
    let callback: &fn() -> i32 = stop
"#,
        Category::ExpressionType,
    );
}

#[test]
fn checks_struct_construction_field_rules() {
    assert_no_diagnostics(
        r#"
struct Point:
    x: f64
    y: f64

fn main() -> ():
    let origin = Point { x: 0.0, y: 0.0 }
    println(f"{origin.x}")
"#,
    );
    assert_has_category(
        r#"
struct Point:
    x: f64
    y: f64

fn main() -> ():
    let origin = Point { x: 0.0 }
    println(f"{origin.x}")
"#,
        Category::Construction,
    );
    assert_has_category(
        r#"
struct Point:
    x: f64
    y: f64

fn main() -> ():
    let origin = Point { x: 0.0, y: 0.0, z: 0.0 }
    println(f"{origin.x}")
"#,
        Category::Construction,
    );
    assert_has_category(
        r#"
struct Point:
    x: f64
    y: f64

fn main() -> ():
    let origin = Point { x: 0.0, x: 1.0, y: 0.0 }
    println(f"{origin.x}")
"#,
        Category::Construction,
    );
}

#[test]
fn checks_field_shorthand_and_field_selection() {
    assert_no_diagnostics(
        r#"
struct Point:
    x: f64
    y: f64

fn main() -> ():
    let x = 1.0
    let y = 2.0
    let point = Point { x, y }
    println(f"{point.x}, {point.y}")
"#,
    );
}

#[test]
fn enforces_struct_field_visibility_across_packages() {
    let tree = TestTree::new("field-visibility");
    tree.package_with_dependencies(
        "records",
        "lib",
        &[],
        &[(
            "src/lib.elx",
            r#"
fn increment(value: i32) -> i32:
    return value + 1

pub struct PublicRecord:
    pub value: i32
    pub callback: &fn(i32) -> i32

pub struct GuardedRecord:
    pub visible: i32
    hidden: i32
    hidden_callback: &fn(i32) -> i32

pub fn public_record() -> PublicRecord:
    return PublicRecord{value: 1, callback: increment}

pub fn guarded_record() -> GuardedRecord:
    return GuardedRecord{
        visible: 1,
        hidden: 2,
        hidden_callback: increment,
    }
"#,
        )],
    );
    let public_consumer = tree.package_with_dependencies(
        "public_consumer",
        "exe",
        &[("records", "../records")],
        &[(
            "src/main.elx",
            r#"
use records.PublicRecord
use records.GuardedRecord
use records.guarded_record
use records.public_record

fn increment(value: i32) -> i32:
    return value + 1

fn read(record: PublicRecord) -> i32:
    match record:
        PublicRecord { value, .. }:
            return value

fn read_visible(record: GuardedRecord) -> i32:
    match record:
        GuardedRecord { visible, .. }:
            return visible

fn main():
    var record = public_record()
    println(record.value)
    record.value = 2
    let value = &record.value
    println(record.callback(40))
    let constructed = PublicRecord{value: 3, callback: increment}
    println(read(constructed))
    println(read_visible(guarded_record()))
"#,
        )],
    );
    let (sources, public_output) = check_package_output(&public_consumer);
    assert!(
        public_output.diagnostics.is_empty(),
        "{}",
        render(&sources, &public_output.diagnostics)
    );

    let private_consumer = tree.package_with_dependencies(
        "private_consumer",
        "exe",
        &[("records", "../records")],
        &[(
            "src/main.elx",
            r#"
use records.GuardedRecord
use records.guarded_record

fn increment(value: i32) -> i32:
    return value + 1

fn main():
    var record = guarded_record()
    println(record.hidden)
    record.hidden = 3
    let hidden_reference = &record.hidden
    println(record.hidden_callback(40))

    let hidden = 4
    let forged = GuardedRecord{
        visible: 1,
        hidden,
        hidden_callback: increment,
    }
    let omitted = GuardedRecord{visible: 1}

    match record:
        GuardedRecord { hidden, .. }:
            println(hidden)
"#,
        )],
    );
    let (sources, private_output) = check_package_output(&private_consumer);
    let visibility = private_output
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.category == Category::Visibility)
        .collect::<Vec<_>>();
    assert_eq!(
        visibility.len(),
        9,
        "{}",
        render(&sources, &private_output.diagnostics)
    );
    assert!(
        visibility.iter().all(|diagnostic| {
            diagnostic.message.contains("package-private")
                && diagnostic.primary.is_some()
                && diagnostic.related.len() == 1
        }),
        "{}",
        render(&sources, &private_output.diagnostics)
    );
    assert!(
        private_output
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.category == Category::Visibility),
        "{}",
        render(&sources, &private_output.diagnostics)
    );
}

#[test]
fn checks_enum_variant_construction() {
    assert_no_diagnostics(
        r#"
enum State:
    Count(i32)
    Positioned { x: i32, y: i32 }
    Disabled

fn main() -> ():
    let a = State.Count(1)
    let b = State.Positioned { x: 1, y: 2 }
    let c = State.Disabled
    match a:
        State.Count(value):
            println(f"{value}")
        _:
            pass
    match b:
        _:
            pass
    match c:
        _:
            pass
"#,
    );
    assert_has_category(
        r#"
enum State:
    Count(i32)
    Disabled

fn main() -> ():
    let a = State.Count(1, 2)
"#,
        Category::Construction,
    );
}

#[test]
fn checks_assignment_place_mutability() {
    assert_no_diagnostics(
        r#"
fn main() -> ():
    var counter = 0
    counter = 1
    counter += 1
    println(f"{counter}")
"#,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let counter = 0
    counter = 1
    println(f"{counter}")
"#,
        Category::Place,
    );
}

#[test]
fn checks_reference_formation_addressability() {
    assert_no_diagnostics(
        r#"
fn main() -> ():
    let value = 1
    let shared = &value
    var mutable_value = 2
    let mutable_ref = &var mutable_value
    println(f"{*shared}, {*mutable_ref}")
"#,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let value = 1
    let mutable_ref = &var value
    println(f"{*mutable_ref}")
"#,
        Category::Place,
    );
}

#[test]
fn rejects_safe_references_to_collection_interiors() {
    // SPEC 3.2: a collection interior is an assignable place but is never a
    // safe-reference target, for either reference form. A mutable collection
    // path keeps element assignment valid.
    assert_no_diagnostics(
        r#"
fn main() -> ():
    var numbers = @vec[1, 2, 3]
    numbers[0] = 4
    println(f"{numbers[0]}")
"#,
    );
    assert_has_category(
        r#"
fn main() -> ():
    var numbers = @vec[1, 2, 3]
    let element = &numbers[0]
    println(f"{*element}")
"#,
        Category::Place,
    );
    assert_has_category(
        r#"
fn main() -> ():
    var numbers = @vec[1, 2, 3]
    let element = &var numbers[0]
    println(f"{*element}")
"#,
        Category::Place,
    );
    assert_has_category(
        r#"
fn main() -> ():
    var array = [1, 2, 3]
    let element = &var array[0]
    println(f"{*element}")
"#,
        Category::Place,
    );
}

#[test]
fn checks_struct_containment_cycles() {
    assert_no_diagnostics(
        r#"
struct Chain:
    value: i32
    next: Option[&Chain]

fn main() -> ():
    let leaf = Chain { value: 1, next: Option.None }
    println(f"{leaf.value}")
"#,
    );
    assert_has_category(
        r#"
struct Node:
    next: Option[Node]

fn main() -> ():
    pass
"#,
        Category::Containment,
    );
}

#[test]
fn checks_primitive_operators_and_conditions() {
    assert_no_diagnostics(
        r#"
fn main() -> ():
    let sum = 1 + 2
    let ratio = 1.0 / 2.0
    let flag = true && false
    if flag:
        println(f"{sum}, {ratio}")
    else:
        pass
"#,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let bad = true + 1
    println(f"{bad}")
"#,
        Category::ExpressionType,
    );
    assert_has_category(
        r#"
fn main() -> ():
    if 1:
        pass
"#,
        Category::ExpressionType,
    );
}

#[test]
fn checks_explicit_numeric_casts() {
    assert_no_diagnostics(
        r#"
fn main() -> ():
    let byte: u8 = 255u8
    let widened = byte as i32
    println(f"{widened}")
"#,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let flag = true as i32
    println(f"{flag}")
"#,
        Category::ExpressionType,
    );
}

#[test]
fn checks_array_and_collection_indexing() {
    assert_no_diagnostics(
        r#"
fn main() -> ():
    var numbers = @vec[1, 2, 3]
    numbers[0] = 4
    let first = numbers[0]
    println(f"{first}")
"#,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let numbers = @set{1, 2, 3}
    let missing = numbers[0]
    println(f"{missing}")
"#,
        Category::ExpressionType,
    );
}

#[test]
fn checks_break_and_continue_placement() {
    assert_no_diagnostics(
        r#"
fn main() -> ():
    var i = 0
    while i < 10:
        if i == 5:
            break
        else:
            continue
        i = i + 1
"#,
    );
    assert_has_category(
        r#"
fn main() -> ():
    break
"#,
        Category::ControlFlow,
    );
    assert_has_category(
        r#"
fn main() -> ():
    continue
"#,
        Category::ControlFlow,
    );
}

#[test]
fn checks_every_reachable_path_returns_a_value() {
    assert_no_diagnostics(
        r#"
fn classify(value: i32) -> str:
    if value < 0:
        return "negative"
    else:
        return "non-negative"
"#,
    );
    assert_has_category(
        r#"
fn classify(value: i32) -> str:
    if value < 0:
        return "negative"
"#,
        Category::ControlFlow,
    );
    assert_has_category(
        r#"
fn classify(value: i32) -> str:
    println(f"{value}")
"#,
        Category::ControlFlow,
    );
    // A `while`/`for` body is never assumed to run, so it cannot by itself
    // satisfy a non-unit function's return requirement.
    assert_has_category(
        r#"
fn classify() -> i32:
    while true:
        return 1
"#,
        Category::ControlFlow,
    );
}

#[test]
fn checks_bool_match_exhaustiveness() {
    assert_no_diagnostics(
        r#"
fn describe(flag: bool) -> str:
    match flag:
        true:
            return "yes"
        false:
            return "no"
"#,
    );
    assert_no_diagnostics(
        r#"
fn describe(flag: bool) -> str:
    match flag:
        true:
            return "yes"
        _:
            return "no"
"#,
    );
    assert_has_category(
        r#"
fn describe(flag: bool) -> str:
    match flag:
        true:
            return "yes"
"#,
        Category::Pattern,
    );
}

#[test]
fn checks_guards_without_counting_them_toward_exhaustiveness() {
    assert_no_diagnostics(
        r#"
fn describe(flag: bool) -> str:
    match flag:
        true if flag:
            return "yes"
        _:
            return "no"
"#,
    );
    assert_has_category(
        r#"
fn describe(flag: bool) -> str:
    match flag:
        true if flag:
            return "yes"
        false:
            return "no"
"#,
        Category::Pattern,
    );
}

#[test]
fn checks_explicit_reference_dereference_patterns_and_copy_bindings() {
    let source = r#"
enum State:
    Enabled
    Disabled

fn describe(flag: &bool) -> str:
    match flag:
        *true:
            return "yes"
        *false:
            return "no"

fn copy_value(value: &i32) -> i32:
    match value:
        *inner:
            return inner

fn enum_value(state: &State) -> i32:
    match state:
        *State.Enabled:
            return 1
        *State.Disabled:
            return 0
"#;
    let (sources, checked) = check_output(source);
    assert!(
        checked.diagnostics.is_empty(),
        "{}",
        render(&sources, &checked.diagnostics)
    );
    assert_eq!(
        checked.program.copied_pattern_bindings.len(),
        1,
        "the `inner` payload binding must be marked as an independent copy"
    );

    assert_has_category(
        r#"
fn describe(flag: bool) -> str:
    match flag:
        *inner:
            return "invalid"
"#,
        Category::Pattern,
    );
    assert_has_category(
        r#"
fn describe(flag: &bool) -> str:
    match flag:
        true:
            return "yes"
        _:
            return "no"
"#,
        Category::Pattern,
    );
}

#[test]
fn checks_enum_match_exhaustiveness_and_bindings() {
    assert_no_diagnostics(
        r#"
enum State:
    Count(i32)
    Positioned { x: i32, y: i32 }
    Disabled

fn describe(state: State) -> i32:
    match state:
        State.Count(value):
            return value
        State.Positioned { x, y }:
            return x + y
        State.Disabled:
            return 0
"#,
    );
    assert_has_category(
        r#"
enum State:
    Count(i32)
    Disabled

fn describe(state: State) -> i32:
    match state:
        State.Count(value):
            return value
"#,
        Category::Pattern,
    );
    assert_no_diagnostics(
        r#"
enum State:
    Count(i32)
    Disabled

fn describe(state: State) -> i32:
    match state:
        State.Count(value):
            return value
        _:
            return 0
"#,
    );
}

#[test]
fn checks_unreachable_match_arms() {
    assert_has_category(
        r#"
enum State:
    Count(i32)
    Disabled

fn describe(state: State) -> i32:
    match state:
        _:
            return 0
        State.Count(value):
            return value
"#,
        Category::Pattern,
    );
    assert_has_category(
        r#"
enum State:
    Count(i32)
    Disabled

fn describe(state: State) -> i32:
    match state:
        State.Count(value):
            return value
        State.Count(other):
            return other
        State.Disabled:
            return 0
"#,
        Category::Pattern,
    );
}

#[test]
fn checks_pattern_type_matches_scrutinee() {
    assert_no_diagnostics(
        r#"
fn describe(value: i32) -> str:
    match value:
        0:
            return "zero"
        _:
            return "other"
"#,
    );
    assert_has_category(
        r#"
fn describe(value: i32) -> str:
    match value:
        true:
            return "no"
        _:
            return "other"
"#,
        Category::Pattern,
    );
}

#[test]
fn checks_struct_pattern_field_completeness() {
    assert_no_diagnostics(
        r#"
struct Point:
    x: f64
    y: f64

fn sum(point: Point) -> f64:
    match point:
        Point { x, y }:
            return x + y
"#,
    );
    assert_no_diagnostics(
        r#"
struct Point:
    x: f64
    y: f64

fn only_x(point: Point) -> f64:
    match point:
        Point { x, .. }:
            return x
"#,
    );
    assert_has_category(
        r#"
struct Point:
    x: f64
    y: f64

fn only_x(point: Point) -> f64:
    match point:
        Point { x }:
            return x
"#,
        Category::Pattern,
    );
}

#[test]
fn checks_variant_pattern_arity() {
    assert_has_category(
        r#"
enum State:
    Count(i32)
    Disabled

fn describe(state: State) -> i32:
    match state:
        State.Count(a, b):
            return a
        State.Disabled:
            return 0
"#,
        Category::Pattern,
    );
}

#[test]
fn checks_alternative_pattern_binding_consistency() {
    assert_no_diagnostics(
        r#"
enum State:
    Count(i32)
    Amount(i32)
    Disabled

fn describe(state: State) -> i32:
    match state:
        State.Count(value) | State.Amount(value):
            return value
        State.Disabled:
            return 0
"#,
    );
    assert_has_category(
        r#"
enum State:
    Count(i32)
    Amount(i32)
    Disabled

fn describe(state: State) -> i32:
    match state:
        State.Count(value) | State.Amount(other):
            return value
        State.Disabled:
            return 0
"#,
        Category::Pattern,
    );
}

#[test]
fn checks_tuple_pattern_exhaustiveness_and_bindings() {
    assert_no_diagnostics(
        r#"
fn sum(pair: (i32, i32)) -> i32:
    match pair:
        (a, b):
            return a + b
"#,
    );
    assert_has_category(
        r#"
fn describe(pair: (i32, i32)) -> str:
    match pair:
        (0, 0):
            return "origin"
"#,
        Category::Pattern,
    );
}

#[test]
fn checks_local_tuple_bindings_and_positional_places() {
    assert_no_diagnostics(
        r#"
fn main() -> ():
    var tuple = ((1, 2), (3,))
    var ((left, _), (right,)): ((i32, i32), (i32,)) = tuple
    left += right
    tuple.0.1 = left
    let shared: &((i32, i32), (i32,)) = &tuple
    let observed = shared.0.1
    let pointer: *var ((i32, i32), (i32,)) = (&var tuple) as *var ((i32, i32), (i32,))
    unsafe:
        pointer.1.0 = observed
"#,
    );

    assert_has_category(
        r#"
fn main() -> ():
    let (left, right) = (1, 2, 3)
"#,
        Category::Pattern,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let pair = (1, 2)
    println(pair.2)
"#,
        Category::ExpressionType,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let pair = (1, 2)
    println(pair.01)
"#,
        Category::Syntax,
    );
    assert_has_category(
        r#"
fn main() -> ():
    var pair = (1, 2)
    let pointer: *var (i32, i32) = (&var pair) as *var (i32, i32)
    pointer.0 = 3
"#,
        Category::UnsafeContext,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let pair = (1, 2)
    pair.0 = 3
"#,
        Category::Place,
    );
}

#[test]
fn coerces_concrete_references_to_implemented_trait_objects() {
    // SPEC 6: an exact expected `&Trait` type context forms a trait object
    // automatically when the concrete reference target implements the trait.
    assert_no_diagnostics(
        r#"
trait Toggle:
    fn status(self: &Self) -> str

struct Session:
    active: bool

impl Toggle for Session:
    fn status(self: &Self) -> str:
        return "on"

fn describe(toggle: &Toggle) -> str:
    return toggle.status()

fn as_toggle(session: &Session) -> &Toggle:
    return session

fn main() -> ():
    let session = Session { active: true }
    let session_ref = &session
    let toggle: &Toggle = session_ref
    var assigned: &Toggle = session_ref
    assigned = session_ref
    println(describe(session_ref))
    println(as_toggle(session_ref).status())
    println(toggle.status())
    println(assigned.status())
    let explicit = session_ref as &Toggle
    println(explicit.status())
"#,
    );
    // Mutability must be preserved across implicit and explicit conversions.
    assert_has_category(
        r#"
trait Toggle:
    fn status(self: &Self) -> str

struct Session:
    active: bool

impl Toggle for Session:
    fn status(self: &Self) -> str:
        return "on"

fn main() -> ():
    var session = Session { active: true }
    let session_ref = &session
    let toggle: &var Toggle = session_ref
    println(toggle.status())
"#,
        Category::ExpressionType,
    );
    // An implicit trait object is formed only from a safe reference.
    assert_has_category(
        r#"
trait Toggle:
    fn status(self: &Self) -> str

fn describe(toggle: &Toggle) -> str:
    return toggle.status()

fn main() -> ():
    let value = 1
    println(describe(value))
    println(f"{value}")
"#,
        Category::ExpressionType,
    );
    // The concrete target must implement the expected trait.
    assert_has_category(
        r#"
trait Toggle:
    fn status(self: &Self) -> str

struct Stone:
    value: i32

fn describe(toggle: &Toggle) -> str:
    return toggle.status()

fn main() -> ():
    let stone = Stone { value: 1 }
    println(describe(&stone))
"#,
        Category::TypeSystem,
    );
}

#[test]
fn defers_a_block_of_statements_under_control_flow_restrictions() {
    // SPEC 8: `defer:` defers several statements as one registration. Its body
    // is a lexical scope and cannot redirect control, because it runs while the
    // enclosing scope is already exiting.
    assert_no_diagnostics(
        r#"
fn release() -> ():
    pass

fn log(value: i32) -> ():
    pass

fn main() -> ():
    var index = 0
    defer:
        let count = 2
        if count > 1:
            log(count)
        release()
    defer release()
    while index < 3:
        defer release()
        index += 1
    unsafe:
        release()
"#,
    );
    for rejected in [
        "            break\n",
        "            continue\n",
        "            return\n",
        "            defer release()\n",
        "            unsafe:\n                release()\n",
    ] {
        let source = format!(
            "fn release() -> ():\n    pass\n\nfn main() -> ():\n    var index = 0\n\
             \x20\x20\x20\x20while index < 3:\n        defer:\n{rejected}\
             \x20\x20\x20\x20\x20\x20\x20\x20index += 1\n"
        );
        assert_has_category(&source, Category::ControlFlow);
    }
    // A `defer` statement may not appear inside an `unsafe` block.
    assert_has_category(
        r#"
fn release() -> ():
    pass

fn main() -> ():
    unsafe:
        defer release()
"#,
        Category::ControlFlow,
    );
}

#[test]
fn generic_inference_requires_a_unique_complete_solution() {
    assert_no_diagnostics(
        r#"
fn select[T, U](left: T, right: U) -> T:
    return left

struct Pair[T, U]:
    left: T
    right: U

fn main() -> ():
    let inferred = select(1, 2u32)
    let explicit = select[i32, u32](1, 2u32)
    let pair = Pair { left: 1, right: 2u32 }
    println(inferred)
    println(explicit)
    println(pair.left)
"#,
    );
    assert_has_category(
        r#"
fn unused[T](value: i32) -> i32:
    return value

fn main() -> ():
    println(unused(1))
"#,
        Category::Call,
    );
    assert_has_category(
        r#"
fn select[T, U](left: T, right: U) -> T:
    return left

fn main() -> ():
    println(select[i32](1, 2u32))
"#,
        Category::Call,
    );
    assert_has_category(
        r#"
struct Pair[T, U]:
    left: T
    right: U

fn main() -> ():
    let pair = Pair[i32] { left: 1, right: 2u32 }
    println(pair.left)
"#,
        Category::Construction,
    );
}

#[test]
fn generic_bodies_use_declared_comparison_bounds() {
    assert_no_diagnostics(
        r#"
fn equivalent[T: PartialEq](left: &T, right: &T) -> bool:
    return *left == *right

fn ordered[T: PartialOrd](left: &T, right: &T) -> bool:
    return *left < *right
"#,
    );
    assert_has_category(
        r#"
fn invalid[T](left: &T, right: &T) -> bool:
    return *left == *right
"#,
        Category::TypeSystem,
    );
}

#[test]
fn checks_standard_option_as_an_ordinary_generic_enum() {
    // Milestone 14.1: `Option[T]` is the generic enum `SPEC.md` 4.4 declares,
    // so construction, inference, and matching are the ordinary enum rules
    // rather than a parallel builtin path.
    assert_no_diagnostics(
        r#"
fn describe(value: Option[i32]) -> i32:
    match value:
        Option.Some(inner):
            return inner
        Option.None:
            return 0

fn main() -> ():
    let some: Option[i32] = Option.Some(7)
    let none: Option[i32] = Option.None
    let inferred = Option.Some(1u32)
    println(describe(some) + describe(none))
    match inferred:
        Option.Some(inner):
            println(inner)
        Option.None:
            pass
"#,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let mismatched: Option[i32] = Option.Some(true)
    println(1)
"#,
        Category::ExpressionType,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let extra: Option[i32] = Option.Some(1, 2)
    println(1)
"#,
        Category::Construction,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let value: Option[i32] = Option.Some(1)
    match value:
        Option.Some(inner):
            println(inner)
"#,
        Category::Pattern,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let value: Option[i32] = Option.Some(1)
    match value:
        Option.Some:
            println(0)
        Option.None:
            println(1)
"#,
        Category::Pattern,
    );
}

#[test]
fn checks_standard_result_as_an_ordinary_generic_enum() {
    // Milestone 14.2: `Result[T, E]` is the generic enum `SPEC.md` 4.4
    // declares. Its propagation role stays with Milestone 15.
    assert_no_diagnostics(
        r#"
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
"#,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let mismatched: Result[i32, str] = Result.Ok(true)
    println(1)
"#,
        Category::ExpressionType,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let value: Result[i32, str] = Result.Ok(1)
    match value:
        Result.Ok(inner):
            println(inner)
"#,
        Category::Pattern,
    );
}

#[test]
fn checks_checked_numeric_conversion_selection() {
    // Milestone 14.3: `Target.try_from(value)` is an associated function on a
    // concrete numeric type (`SPEC.md` 4.1).
    assert_no_diagnostics(
        r#"
fn main() -> ():
    let narrowed: Result[i8, NumericError] = i8.try_from(300)
    let widened: Result[i64, NumericError] = i64.try_from(1u8)
    let rounded: Result[f32, NumericError] = f32.try_from(1.5)
    match narrowed:
        Result.Ok(value):
            println(value)
        Result.Err(reason):
            println(1)
"#,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let missing = i8.try_from()
    println(1)
"#,
        Category::Call,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let extra = i8.try_from(1, 2)
    println(1)
"#,
        Category::Call,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let text = i8.try_from("nope")
    println(1)
"#,
        Category::Call,
    );
    // A numeric type's associated-function surface is complete, so an
    // unrecognized member on one is reported rather than silently typed.
    assert_has_category(
        r#"
fn main() -> ():
    let nonsense = i8.absent(1)
    println(1)
"#,
        Category::Call,
    );
}

#[test]
fn checks_numeric_alternative_selection() {
    // Milestone 14.4: the standard alternatives take the receiver's exact
    // operand type and are integer-only (`SPEC.md` 4.1).
    assert_no_diagnostics(
        r#"
fn main() -> ():
    let checked: Option[i32] = 1.checked_add(2)
    let wrapped: i32 = 1.wrapping_sub(2)
    let clamped: i32 = 1.saturating_mul(2)
    let negated: Option[i32] = 1.checked_neg()
    let shifted: i32 = 1.wrapping_shl(2)
    let converted: u8 = u8.wrapping_from(300)
    println(wrapped + clamped + shifted)
"#,
    );
    // An operand takes the receiver's exact type, exactly as the operator does.
    assert_has_category(
        r#"
fn main() -> ():
    let mixed = 1i32.checked_add(2u8)
    println(1)
"#,
        Category::Call,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let extra = 1i32.checked_neg(2i32)
    println(1)
"#,
        Category::Call,
    );
    // Wrapping and saturating conversions are defined by an integer range.
    assert_has_category(
        r#"
fn main() -> ():
    let rounded = i32.wrapping_from(1.5)
    println(1)
"#,
        Category::Call,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let float_target = f32.saturating_from(1)
    println(1)
"#,
        Category::Call,
    );
}

#[test]
fn checks_standard_collection_and_display_surfaces() {
    assert_no_diagnostics(
        r#"
fn main() -> ():
    let empty_vec: Vec[i32] = @vec[]
    let empty_map: Map[str, i32] = @map{}
    let empty_set: Set[str] = @set{}
    let values = @vec[1, 2, 3]
    let names = @map{"one": 1}
    let tags = @set{"a", "b"}
    let text = f"{values} {names} {tags}"
    println(text)
"#,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let values = @vec[1, true]
    println(values.len())
"#,
        Category::ExpressionType,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let values = @vec[]
    println(1)
"#,
        Category::ExpressionType,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let values = @set{1.0}
    println(values.len())
"#,
        Category::TypeSystem,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let key: String = "mutable"
    let values = @map{key: 1}
    println(values.len())
"#,
        Category::TypeSystem,
    );
    assert_has_category(
        r#"
struct Hidden:
    value: i32

fn main() -> ():
    let hidden = Hidden { value: 1 }
    println(f"{hidden}")
"#,
        Category::ExpressionType,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let values = [1, 2]
    println(values[2])
"#,
        Category::ExpressionType,
    );
}

#[test]
fn postfix_propagation_requires_matching_standard_result_types() {
    // M15.2: `?` accepts only the standard `Result[T, E]` inside a function
    // returning `Result[U, E]` with exactly the same `E` (`SPEC.md` 8).
    assert_no_diagnostics(
        r#"
fn source(flag: bool) -> Result[i32, str]:
    if flag:
        return Result.Ok(1)
    return Result.Err("failed")

fn widen(flag: bool) -> Result[u32, str]:
    let value = source(flag)?
    return Result.Ok(2u32)

fn increment_result[E](result: Result[i32, E]) -> Result[i32, E]:
    let value = result?
    return Result.Ok(value + 1)

fn main() -> ():
    pass
"#,
    );
    // The standard identity is recognized through a transparent alias.
    assert_no_diagnostics(
        r#"
type Fallible[T] = Result[T, str]

fn source() -> Fallible[i32]:
    return Result.Ok(1)

fn chain() -> Fallible[i32]:
    let value = source()?
    return Result.Ok(value)

fn main() -> ():
    pass
"#,
    );
    // `Option` is handled with `match`, never `?`.
    assert_has_category(
        r#"
fn reject() -> Result[i32, str]:
    let value: Option[i32] = Option.Some(1)
    let bad = value?
    return Result.Ok(1)

fn main() -> ():
    pass
"#,
        Category::ExpressionType,
    );
    // A non-`Result` operand cannot propagate.
    assert_has_category(
        r#"
fn reject() -> Result[i32, str]:
    let bad = 7?
    return Result.Ok(bad)

fn main() -> ():
    pass
"#,
        Category::ExpressionType,
    );
    // Differing error types receive no implicit conversion.
    assert_has_category(
        r#"
fn reject(source: Result[i32, u32]) -> Result[i32, str]:
    let bad = source?
    return Result.Ok(bad)

fn main() -> ():
    pass
"#,
        Category::ExpressionType,
    );
    // The enclosing function must itself return the standard `Result`.
    assert_has_category(
        r#"
fn reject(source: Result[i32, str]) -> i32:
    return source?

fn main() -> ():
    pass
"#,
        Category::ExpressionType,
    );
    // A user enum that merely shares the spelling receives no propagation
    // role (M15.1), on either side of the rule.
    assert_has_category(
        r#"
enum Result[T, E]:
    Ok(T)
    Err(E)

fn reject(source: Result[i32, str]) -> Result[i32, str]:
    let bad = source?
    return Result.Ok(bad)

fn main() -> ():
    pass
"#,
        Category::ExpressionType,
    );
}

#[test]
fn deferred_calls_must_be_safe_and_unit_returning() {
    // M15.5: the single-call form defers one safe unit-returning call; a
    // fallible or unsafe operation is handled before scope exit instead.
    assert_no_diagnostics(
        r#"
struct Resource:
    open: bool

    fn close(self: &Self) -> ():
        pass

fn release() -> ():
    pass

fn main() -> ():
    let resource = Resource { open: true }
    defer resource.close()
    defer release()
"#,
    );
    assert_has_category(
        r#"
fn value() -> i32:
    return 7

fn main() -> ():
    defer value()
"#,
        Category::ControlFlow,
    );
    assert_has_category(
        r#"
unsafe fn danger() -> ():
    pass

fn main() -> ():
    defer danger()
"#,
        Category::ControlFlow,
    );
    // A fallible call's `Result` must be handled, not deferred.
    assert_has_category(
        r#"
fn flush() -> Result[i32, str]:
    return Result.Ok(1)

fn main() -> ():
    defer flush()
"#,
        Category::ControlFlow,
    );
    // Postfix `?` cannot appear anywhere inside deferred syntax.
    assert_has_category(
        r#"
fn flush() -> Result[i32, str]:
    return Result.Ok(1)

fn cleanup() -> Result[i32, str]:
    defer:
        let value = flush()?
    return Result.Ok(1)

fn main() -> ():
    pass
"#,
        Category::ControlFlow,
    );
}

#[test]
fn raw_pointer_conversions_follow_the_exact_matrix() {
    // M16.2/M16.5: `&T as *T`, `&var T as *var T`/`*T`, and `*var T as *T`
    // are safe; a pointee-changing raw cast requires `unsafe:`; nothing
    // upgrades mutability, and pointers never convert to or from integers.
    assert_no_diagnostics(
        r#"
fn main() -> ():
    var value = 41
    let shared: *i32 = (&value) as *i32
    let edit: &var i32 = &var value
    let mutable: *var i32 = edit as *var i32
    let downgraded_reference: *i32 = edit as *i32
    let downgraded_pointer: *i32 = mutable as *i32
    unsafe:
        let repointed: *u8 = shared as *u8
        let repointed_mutable: *var u8 = mutable as *var u8
        let repointed_downgrade: *u8 = mutable as *u8
    println(value)
"#,
    );
    // A shared reference or pointer never upgrades to a mutable pointer.
    assert_has_category(
        r#"
fn main() -> ():
    let value = 1
    let bad = (&value) as *var i32
"#,
        Category::ExpressionType,
    );
    assert_has_category(
        r#"
fn main() -> ():
    var value = 1
    let shared: *i32 = (&value) as *i32
    unsafe:
        let bad = shared as *var i32
"#,
        Category::ExpressionType,
    );
    assert_has_category(
        r#"
fn main() -> ():
    var value = 1
    let shared: *i32 = (&value) as *i32
    unsafe:
        let bad = shared as *var u8
"#,
        Category::ExpressionType,
    );
    // Changing the pointee type requires an `unsafe:` block.
    assert_has_category(
        r#"
fn main() -> ():
    var value = 1
    let shared: *i32 = (&value) as *i32
    let bad = shared as *u8
"#,
        Category::UnsafeContext,
    );
    // No pointer/integer conversion in either direction.
    assert_has_category(
        r#"
fn main() -> ():
    var value = 1
    let shared: *i32 = (&value) as *i32
    let bad = shared as usize
"#,
        Category::ExpressionType,
    );
    assert_has_category(
        r#"
fn main() -> ():
    let bad = 4096 as *i32
"#,
        Category::ExpressionType,
    );
    // A raw pointer converts only to a reference with exactly its pointee
    // type and mutability, and only in an `unsafe:` block.
    assert_has_category(
        r#"
fn main() -> ():
    var value = 1
    let mutable: *var i32 = (&var value) as *var i32
    unsafe:
        let bad = mutable as &i32
"#,
        Category::ExpressionType,
    );
    assert_has_category(
        r#"
fn main() -> ():
    var value = 1
    let shared: *i32 = (&value) as *i32
    let bad = shared as &i32
"#,
        Category::UnsafeContext,
    );
}

#[test]
fn unsafe_only_operations_require_a_lexical_unsafe_block() {
    // M16.3: the lexical `unsafe:` block is the only unsafe context; an
    // `unsafe` function's body is deliberately not one.
    assert_no_diagnostics(
        r#"
unsafe fn danger() -> ():
    pass

fn wrapper() -> ():
    unsafe:
        danger()

unsafe fn layered() -> ():
    unsafe:
        danger()

fn main() -> ():
    wrapper()
"#,
    );
    // Raw dereference, read or write, requires `unsafe:`.
    assert_has_category(
        r#"
fn main() -> ():
    var value = 1
    let pointer: *i32 = (&value) as *i32
    println(*pointer)
"#,
        Category::UnsafeContext,
    );
    assert_has_category(
        r#"
fn main() -> ():
    var value = 1
    let pointer: *var i32 = (&var value) as *var i32
    *pointer = 2
"#,
        Category::UnsafeContext,
    );
    // Calling an unsafe function requires `unsafe:` at the call site, even
    // inside an unsafe function's own body (M16.3's independence rule).
    assert_has_category(
        r#"
unsafe fn danger() -> ():
    pass

fn main() -> ():
    danger()
"#,
        Category::UnsafeContext,
    );
    assert_has_category(
        r#"
unsafe fn danger() -> ():
    pass

unsafe fn caller() -> ():
    danger()

fn main() -> ():
    pass
"#,
        Category::UnsafeContext,
    );
    // M16.4: taking and storing an `&unsafe fn` reference is safe; invoking
    // it requires `unsafe:`, and it never converts to `&fn`.
    assert_no_diagnostics(
        r#"
unsafe fn danger() -> ():
    pass

fn main() -> ():
    let reference: &unsafe fn() -> () = danger
    let same = reference == danger
    unsafe:
        reference()
    println(same)
"#,
    );
    assert_has_category(
        r#"
unsafe fn danger() -> ():
    pass

fn main() -> ():
    let reference: &unsafe fn() -> () = danger
    reference()
"#,
        Category::UnsafeContext,
    );
    assert_has_category(
        r#"
unsafe fn danger() -> ():
    pass

fn main() -> ():
    let bad: &fn() -> () = danger
"#,
        Category::ExpressionType,
    );
}

#[test]
fn pointer_validity_is_expression_local() {
    // M16.7: only an expression-local constant null operand is a compile
    // error; facts never propagate through bindings, assignments, branch
    // conditions, reachability, or calls.
    assert_no_diagnostics(
        r#"
fn conceal(pointer: *i32) -> *i32:
    return pointer

fn main() -> ():
    let through_binding: *i32 = null
    let through_call = conceal(null)
    if through_binding == null:
        println("null checked")
    else:
        unsafe:
            println(*through_binding)
    unsafe:
        println(*through_call)
"#,
    );
    assert_has_category(
        r#"
fn main() -> ():
    unsafe:
        println(*null)
"#,
        Category::PointerValidity,
    );
    // Grouping and casts stay within the operand expression.
    assert_has_category(
        r#"
fn main() -> ():
    unsafe:
        println(*(null))
"#,
        Category::PointerValidity,
    );
    assert_has_category(
        r#"
fn main() -> ():
    unsafe:
        let bad = null as &i32
"#,
        Category::PointerValidity,
    );
}

#[test]
fn raw_dereference_places_permit_writes_but_never_safe_references() {
    // M16.6: a `*var T` target is an assignable place, a `*T` target is
    // read-only even in unsafe code, and neither forms a safe reference —
    // that path is the explicit, asserted `as` conversion.
    assert_no_diagnostics(
        r#"
struct Cell:
    value: i32

fn main() -> ():
    var cell = Cell { value: 1 }
    let pointer: *var Cell = (&var cell) as *var Cell
    unsafe:
        *pointer = Cell { value: 2 }
        (*pointer).value = 3
        (*pointer).value += 1
    println(cell.value)
"#,
    );
    assert_has_category(
        r#"
fn main() -> ():
    var value = 1
    let shared: *i32 = (&value) as *i32
    unsafe:
        *shared = 2
"#,
        Category::Place,
    );
    assert_has_category(
        r#"
fn main() -> ():
    var value = 1
    let pointer: *var i32 = (&var value) as *var i32
    unsafe:
        let bad = &var *pointer
"#,
        Category::Place,
    );
    assert_has_category(
        r#"
fn main() -> ():
    var value = 1
    let pointer: *i32 = (&value) as *i32
    unsafe:
        let bad = &*pointer
"#,
        Category::Place,
    );
}
