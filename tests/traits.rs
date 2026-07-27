use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::backend::Target;
use elamite::diagnostics::{Category, Diagnostic};
use elamite::driver::check_frontend;
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
            "elamite-traits-{}-{name}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("src")).expect("create test tree");
        Self { root }
    }
}

impl Drop for TestTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn frontend_diagnostics(source: &str) -> (SourceManager, Vec<Diagnostic>) {
    let tree = TestTree::new("case");
    fs::write(
        tree.root.join("elamite.toml"),
        "[package]\nname = \"traits_test\"\nversion = \"0.1.0\"\ntarget_kind = \"executable\"\n",
    )
    .expect("write manifest");
    fs::write(tree.root.join("src/main.elx"), source).expect("write source");
    let mut sources = SourceManager::new();
    let graph = PackageGraph::resolve(&tree.root.join("elamite.toml"), &mut sources)
        .expect("package graph resolves");
    let diagnostics = match check_frontend(&graph, &mut sources, Target::X86_64) {
        Ok(_) => Vec::new(),
        Err(diagnostics) => diagnostics,
    };
    (sources, diagnostics)
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
                        "{}:{}: {:?}: {}",
                        position.line, position.column, diagnostic.category, diagnostic.message
                    )
                },
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_clean(source: &str) {
    let (sources, diagnostics) = frontend_diagnostics(source);
    assert!(
        diagnostics.is_empty(),
        "expected no diagnostics, got:\n{}",
        render(&sources, &diagnostics)
    );
}

fn assert_reports(source: &str, expected: &str) {
    let (sources, diagnostics) = frontend_diagnostics(source);
    let text = render(&sources, &diagnostics);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.category == Category::TypeSystem
                && diagnostic.message.contains(expected)),
        "expected a TypeSystem diagnostic containing {expected:?}, got:\n{text}"
    );
}

const TRAIT: &str = r#"
trait Toggle:
    fn status(self: &Self) -> str
    fn label(self: &Self) -> str

    fn category(self: &Self) -> str:
        return "toggle"

struct Session:
    active: bool
"#;

#[test]
fn accepts_an_implementation_that_matches_its_trait() {
    // A default method may be omitted or overridden; required methods must be
    // present with the trait's exact signature.
    assert_clean(&format!(
        r#"{TRAIT}
impl Toggle for Session:
    fn status(self: &Self) -> str:
        return "on"
    fn label(self: &Self) -> str:
        return "session"

fn main() -> ():
    pass
"#
    ));
    assert_clean(&format!(
        r#"{TRAIT}
impl Toggle for Session:
    fn status(self: &Self) -> str:
        return "on"
    fn label(self: &Self) -> str:
        return "session"
    fn category(self: &Self) -> str:
        return "overridden"

fn main() -> ():
    pass
"#
    ));
}

#[test]
fn rejects_a_missing_required_method() {
    assert_reports(
        &format!(
            r#"{TRAIT}
impl Toggle for Session:
    fn status(self: &Self) -> str:
        return "on"

fn main() -> ():
    pass
"#
        ),
        "missing required method `label`",
    );
}

#[test]
fn rejects_a_method_the_trait_does_not_declare() {
    assert_reports(
        &format!(
            r#"{TRAIT}
impl Toggle for Session:
    fn status(self: &Self) -> str:
        return "on"
    fn label(self: &Self) -> str:
        return "session"
    fn surprise(self: &Self) -> str:
        return "no"

fn main() -> ():
    pass
"#
        ),
        "has no method named `surprise`",
    );
}

#[test]
fn rejects_signature_mismatches_in_each_position() {
    // Return type.
    assert_reports(
        &format!(
            r#"{TRAIT}
impl Toggle for Session:
    fn status(self: &Self) -> i32:
        return 1
    fn label(self: &Self) -> str:
        return "session"

fn main() -> ():
    pass
"#
        ),
        "the return type differs",
    );
    // Receiver form.
    assert_reports(
        &format!(
            r#"{TRAIT}
impl Toggle for Session:
    fn status(self: &var Self) -> str:
        return "on"
    fn label(self: &Self) -> str:
        return "session"

fn main() -> ():
    pass
"#
        ),
        "the receiver form differs",
    );
    // Parameter count.
    assert_reports(
        &format!(
            r#"{TRAIT}
impl Toggle for Session:
    fn status(self: &Self, extra: i32) -> str:
        return "on"
    fn label(self: &Self) -> str:
        return "session"

fn main() -> ():
    pass
"#
        ),
        "the parameter count differs",
    );
}

#[test]
fn rejects_overlapping_implementations() {
    assert_reports(
        &format!(
            r#"{TRAIT}
impl Toggle for Session:
    fn status(self: &Self) -> str:
        return "a"
    fn label(self: &Self) -> str:
        return "a"

impl Toggle for Session:
    fn status(self: &Self) -> str:
        return "b"
    fn label(self: &Self) -> str:
        return "b"

fn main() -> ():
    pass
"#
        ),
        "is already implemented for this type",
    );
}

#[test]
fn a_self_returning_trait_method_matches_the_implementing_type() {
    // `Self` in the trait declaration must compare equal to the implementing
    // type, not to some distinct nominal type.
    assert_clean(
        r#"
trait Clone2:
    fn duplicate(self: &Self) -> Self

struct Point:
    x: i32

impl Clone2 for Point:
    fn duplicate(self: &Self) -> Point:
        return Point { x: self.x }

fn main() -> ():
    pass
"#,
    );
    assert_reports(
        r#"
trait Clone2:
    fn duplicate(self: &Self) -> Self

struct Point:
    x: i32

struct Other:
    y: i32

impl Clone2 for Point:
    fn duplicate(self: &Self) -> Other:
        return Other { y: 1 }

fn main() -> ():
    pass
"#,
        "the return type differs",
    );
}

#[test]
fn a_trait_object_requires_an_implementation_and_object_safety() {
    // A concrete reference becomes a trait-object reference only when its
    // target implements the trait.
    assert_reports(
        &format!(
            r#"{TRAIT}
struct Stranger:
    value: i32

impl Toggle for Session:
    fn status(self: &Self) -> str:
        return "on"
    fn label(self: &Self) -> str:
        return "session"

fn main() -> ():
    let stranger = Stranger {{ value: 1 }}
    let reference = &stranger
    let object = reference as &Toggle
    pass
"#
        ),
        "does not implement trait `Toggle`",
    );
}

#[test]
fn object_safety_rejects_self_returning_and_generic_methods() {
    // SPEC 6: a method returning `Self`, taking `Self` by value, or carrying
    // its own generic parameters keeps the trait usable for static dispatch
    // but bars it from forming an object.
    assert_reports(
        r#"
trait Duplicate:
    fn duplicate(self: &Self) -> Self

struct Point:
    x: i32

impl Duplicate for Point:
    fn duplicate(self: &Self) -> Point:
        return Point { x: self.x }

fn main() -> ():
    let point = Point { x: 1 }
    let reference = &point
    let object = reference as &Duplicate
    pass
"#,
        "cannot form a trait object because method `duplicate` returns `Self`",
    );
    assert_reports(
        r#"
trait ByValue:
    fn consume(self: Self) -> i32

struct Point:
    x: i32

impl ByValue for Point:
    fn consume(self: Self) -> i32:
        return self.x

fn main() -> ():
    let point = Point { x: 1 }
    let reference = &point
    let object = reference as &ByValue
    pass
"#,
        "does not take an `&Self` or `&var Self` receiver",
    );
}

#[test]
fn a_trait_that_is_not_object_safe_still_works_for_static_dispatch() {
    // The trait itself remains legal; only object formation is barred.
    assert_clean(
        r#"
trait Duplicate:
    fn duplicate(self: &Self) -> Self

struct Point:
    x: i32

impl Duplicate for Point:
    fn duplicate(self: &Self) -> Point:
        return Point { x: self.x }

fn main() -> ():
    pass
"#,
    );
}

#[test]
fn selects_trait_methods_with_inherent_preference() {
    // An impl-provided method and a trait default are both callable; an
    // inherent method of the same name wins over the trait's.
    assert_clean(
        r#"
trait Toggle:
    fn status(self: &Self) -> str
    fn category(self: &Self) -> str:
        return "toggle"

struct Session:
    active: bool

    fn label(self: &Self) -> str:
        return "inherent"

impl Toggle for Session:
    fn status(self: &Self) -> str:
        return "on"

fn main() -> ():
    let session = Session { active: true }
    println(session.status())
    println(session.category())
    println(session.label())
"#,
    );
}

#[test]
fn an_absent_method_is_reported_rather_than_accepted() {
    let (sources, diagnostics) = frontend_diagnostics(
        r#"
struct Session:
    active: bool

fn main() -> ():
    let session = Session { active: true }
    let value = session.nonexistent()
"#,
    );
    let text = render(&sources, &diagnostics);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("no method named `nonexistent`")),
        "{text}"
    );
}

#[test]
fn a_method_supplied_by_two_traits_is_ambiguous() {
    let (sources, diagnostics) = frontend_diagnostics(
        r#"
trait Left:
    fn shared(self: &Self) -> i32

trait Right:
    fn shared(self: &Self) -> i32

struct Both:
    value: i32

impl Left for Both:
    fn shared(self: &Self) -> i32:
        return 1

impl Right for Both:
    fn shared(self: &Self) -> i32:
        return 2

fn main() -> ():
    let both = Both { value: 0 }
    let value = both.shared()
"#,
    );
    let text = render(&sources, &diagnostics);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("more than one trait")),
        "{text}"
    );
}

#[test]
fn a_builtin_trait_implementation_is_selectable() {
    // `Close` is compiler-known rather than a user declaration, so selection
    // must search builtin-trait implementations too.
    assert_clean(
        r#"
struct State:
    closed: bool

struct Resource:
    state: &var State

impl Close for Resource:
    fn close(self: &Self) -> ():
        self.state.closed = true

fn main() -> ():
    var state = State { closed: false }
    let resource = Resource { state: &var state }
    resource.close()
"#,
    );
}

#[test]
fn equality_requires_a_partial_eq_implementation() {
    // SPEC 4.3: comparing a nominal value needs a derivation or an
    // implementation; without one the operator is rejected rather than
    // compiled into a machine comparison.
    assert_clean(
        r#"
struct Point(PartialEq):
    x: i32

fn main() -> ():
    let a = Point { x: 1 }
    let b = Point { x: 1 }
    println(f"{a == b}")
"#,
    );
    assert_reports(
        r#"
struct Point:
    x: i32

fn main() -> ():
    let a = Point { x: 1 }
    let b = Point { x: 1 }
    println(f"{a == b}")
"#,
        "does not implement `PartialEq`",
    );
}

#[test]
fn default_is_available_only_where_derived() {
    let (sources, diagnostics) = frontend_diagnostics(
        r#"
struct Point:
    x: i32

fn main() -> ():
    let origin = Point.default()
    println(f"{origin.x}")
"#,
    );
    let text = render(&sources, &diagnostics);
    assert!(
        diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("no associated function named `default`")),
        "{text}"
    );
}
