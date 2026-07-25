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
    // Milestone 6 deliberately checks only the non-generic, non-method,
    // non-trait subset (methods, generics, and traits are Milestones
    // 11-13); this asserts the checker never *misdiagnoses* the parts of
    // the demonstration it does look at, not that it fully checks it.
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
