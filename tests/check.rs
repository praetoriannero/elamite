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
        let directory = self.root.join(relative);
        fs::create_dir_all(directory.join("src")).expect("create package source directory");
        let manifest = format!(
            "[package]\nname = \"{relative}\"\nversion = \"0.1.0\"\ntarget_kind = \"executable\"\n"
        );
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
    let (sources, resolved) = resolve_package(&package);
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
