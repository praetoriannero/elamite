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
        "[package]\nname = \"traits_test\"\nversion = \"0.1.0\"\ntarget_kind = \"exe\"\n",
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

fn assert_reports_any(source: &str, expected: &str) {
    let (sources, diagnostics) = frontend_diagnostics(source);
    let text = render(&sources, &diagnostics);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains(expected)),
        "expected a diagnostic containing {expected:?}, got:\n{text}"
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
    let object: &Toggle = reference
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
    let object: &Duplicate = reference
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
    let object: &ByValue = reference
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

impl Session:
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
fn close_is_not_builtin_but_can_be_user_declared() {
    // Cleanup protocols are ordinary library abstractions when a program wants
    // generic cleanup polymorphism; `defer` does not require this trait.
    assert_clean(
        r#"
trait Close:
    fn close(self: &Self) -> ()

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

    let (sources, diagnostics) = frontend_diagnostics(
        r#"
struct Resource:
    pass

impl Resource:
    fn close(self: &Self) -> ():
        pass

impl Close for Resource:
    fn close(self: &Self) -> ():
        pass
"#,
    );
    let text = render(&sources, &diagnostics);
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("cannot resolve `Close`")),
        "expected undeclared `Close` to be absent from the prelude, got:\n{text}"
    );
}

#[test]
fn inherent_blocks_apply_generic_bounds_and_disjoint_exact_targets() {
    assert_clean(
        r#"
trait Mark:
    fn mark(self: &Self) -> i32

struct Tagged:
    value: i32

impl Mark for Tagged:
    fn mark(self: &Self) -> i32:
        return self.value

struct Wrapper[T]:
    value: T

impl[T: Mark] Wrapper[T]:
    fn marked(self: &Self) -> i32:
        return self.value.mark()

impl Wrapper[i32]:
    fn kind(self: &Self) -> str:
        return "integer"

impl Wrapper[str]:
    fn kind(self: &Self) -> str:
        return "text"

fn main() -> ():
    let tagged = Wrapper { value: Tagged { value: 7 } }
    let integer = Wrapper { value: 1 }
    let text: Wrapper[str] = Wrapper { value: "x" }
    println(tagged.marked())
    println(integer.kind())
    println(text.kind())
"#,
    );
}

#[test]
fn inherent_blocks_reject_unconstrained_and_overlapping_methods() {
    assert_reports_any(
        r#"
struct Wrapper[T]:
    value: T

impl[T, U] Wrapper[T]:
    fn unused(self: &Self) -> ():
        pass

fn main() -> ():
    pass
"#,
        "must occur in its target type",
    );
    assert_reports_any(
        r#"
struct Wrapper[T]:
    value: T

impl[T] Wrapper[T]:
    fn label(self: &Self) -> str:
        return "generic"

impl Wrapper[i32]:
    fn label(self: &Self) -> str:
        return "exact"

fn main() -> ():
    pass
"#,
        "overlapping implementation targets",
    );
}

#[test]
fn inherent_blocks_compare_aliases_canonically_and_share_the_field_namespace() {
    assert_reports_any(
        r#"
struct Record:
    value: i32

type Alias = Record

impl Record:
    fn name(self: &Self) -> str:
        return "record"

impl Alias:
    fn name(self: &Self) -> str:
        return "alias"

fn main() -> ():
    pass
"#,
        "overlapping implementation targets",
    );
    assert_reports_any(
        r#"
struct Record:
    value: i32

impl Record:
    fn value(self: &Self) -> i32:
        return 1

fn main() -> ():
    pass
"#,
        "conflicts with a field",
    );

    assert_clean(
        r#"
struct Record:
    value: i32

type Alias = Record

impl Alias:
    fn new(value: i32) -> Self:
        return Self { value: value }

fn main() -> ():
    let record = Alias.new(7)
    println(record.value)
"#,
    );
}

#[test]
fn inherent_blocks_are_confined_to_the_target_declaration_module() {
    assert_reports_any(
        r#"
struct Record:
    value: i32

mod extensions:
    impl root.Record:
        fn extra(self: &Self) -> i32:
            return self.value

fn main() -> ():
    pass
"#,
        "must be in the module that declares its target type",
    );
    assert_reports_any(
        r#"
@importc("Foreign", "foreign.h")
struct Foreign:
    value: i32

impl Foreign:
    fn method(self: &Self) -> ():
        pass

fn main() -> ():
    pass
"#,
        "must be a nominal struct or enum type",
    );
}

#[test]
fn inherent_method_visibility_is_declared_on_each_method() {
    let tree = TestTree::new("inherent-visibility");
    let dependency = tree.root.join("dep");
    let application = tree.root.join("app");
    fs::create_dir_all(dependency.join("src")).expect("create dependency source");
    fs::create_dir_all(application.join("src")).expect("create application source");
    fs::write(
        dependency.join("elamite.toml"),
        "[package]\nname = \"dep\"\nversion = \"0.1.0\"\ntarget_kind = \"lib\"\n",
    )
    .expect("write dependency manifest");
    fs::write(
        dependency.join("src/lib.elx"),
        r#"
pub struct Record:
    pub value: i32

impl Record:
    fn hidden(self: &Self) -> i32:
        return self.value

    pub fn visible(self: &Self) -> i32:
        return self.value
"#,
    )
    .expect("write dependency source");
    fs::write(
        application.join("elamite.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\ntarget_kind = \"exe\"\n\n\
         [dependencies.dep]\npath = \"../dep\"\n",
    )
    .expect("write application manifest");
    fs::write(
        application.join("src/main.elx"),
        r#"
use dep.Record

fn main() -> ():
    let record = Record { value: 7 }
    println(record.visible())
    println(record.hidden())
"#,
    )
    .expect("write application source");

    let mut sources = SourceManager::new();
    let graph = PackageGraph::resolve(&application.join("elamite.toml"), &mut sources)
        .expect("package graph resolves");
    let diagnostics = match check_frontend(&graph, &mut sources, Target::X86_64) {
        Ok(_) => panic!("the private method must be rejected"),
        Err(diagnostics) => diagnostics,
    };
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.category == Category::Visibility
                && diagnostic
                    .message
                    .contains("method `hidden` is package-private")
        }),
        "{}",
        render(&sources, &diagnostics)
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

#[test]
fn generic_derivations_are_conditional_on_instantiated_fields() {
    assert_clean(
        r#"
struct Wrapper[T](Default, PartialEq):
    value: T

fn main() -> ():
    let first: Wrapper[i32] = Wrapper.default()
    let second = Wrapper[i32].default()
    println(f"{first == second}")
"#,
    );
    assert_reports(
        r#"
struct Wrapper[T](Default):
    value: T

fn main() -> ():
    let invalid = Wrapper[&i32].default()
"#,
        "field type does not provide it",
    );
}

#[test]
fn derivation_lists_and_component_requirements_are_validated() {
    assert_reports(
        r#"
struct Bad(Default):
    callback: &fn() -> ()

fn main() -> ():
    pass
"#,
        "at least one field does not provide `Default`",
    );
    assert_reports(
        r#"
enum Choice(Default):
    None

fn main() -> ():
    pass
"#,
        "`Default` cannot be derived for an enum",
    );
    assert_reports(
        r#"
struct Duplicate(PartialEq, PartialEq):
    value: i32

fn main() -> ():
    pass
"#,
        "listed more than once",
    );
    assert_reports(
        r#"
struct Custom(Display):
    value: i32

fn main() -> ():
    pass
"#,
        "not a compiler-supported derivable trait",
    );
}

#[test]
fn ordering_requires_partial_ord_and_generic_impl_overlap_is_rejected() {
    assert_reports(
        r#"
struct Plain:
    value: i32

fn main() -> ():
    let left = Plain { value: 1 }
    let right = Plain { value: 2 }
    println(f"{left < right}")
"#,
        "does not implement `PartialOrd`",
    );
    assert_reports(
        r#"
trait Label:
    fn label(self: &Self) -> str

struct Wrapper[T]:
    value: T

impl[T] Label for Wrapper[T]:
    fn label(self: &Self) -> str:
        return "generic"

impl Label for Wrapper[i32]:
    fn label(self: &Self) -> str:
        return "specific"

fn main() -> ():
    pass
"#,
        "already implemented for this type",
    );
}

#[test]
fn stable_hash_is_structural_and_cannot_be_claimed_manually() {
    assert_clean(
        r#"
struct Key[T](PartialEq, Eq, Hash):
    value: T

fn accept[T: StableHash](value: &T) -> ():
    pass

fn main() -> ():
    let key = Key { value: 1i32 }
    accept(&key)
"#,
    );
    assert_reports(
        r#"
struct Key[T](PartialEq, Eq, Hash):
    value: T

fn accept[T: StableHash](value: &T) -> ():
    pass

fn main() -> ():
    let text: String = "mutable"
    let key = Key { value: text }
    accept(&key)
"#,
        "does not satisfy required `StableHash` capability",
    );
    assert_reports(
        r#"
struct Manual:
    value: i32

impl StableHash for Manual:
    pass

fn accept[T: StableHash](value: &T) -> ():
    pass

fn main() -> ():
    let value = Manual { value: 1 }
    accept(&value)
"#,
        "does not satisfy required `StableHash` capability",
    );
}

#[test]
fn bound_lookup_considers_only_traits_in_the_calling_modules_scope() {
    let hidden = r#"
struct Session:
    value: i32

mod hidden:
    trait Secret:
        fn secret(self: &Self) -> str

    impl Secret for root.Session:
        fn secret(self: &Self) -> str:
            return "secret"

fn main() -> ():
    let session = Session { value: 1 }
    println(session.secret())
"#;
    let (sources, diagnostics) = frontend_diagnostics(hidden);
    let text = render(&sources, &diagnostics);
    assert!(
        diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("no method named `secret` is available")),
        "expected hidden trait to be excluded from bound lookup, got:\n{text}"
    );

    assert_clean(
        r#"
struct Session:
    value: i32

mod hidden:
    pub trait Secret:
        fn secret(self: &Self) -> str

    impl Secret for root.Session:
        fn secret(self: &Self) -> str:
            return "secret"

use self.hidden.Secret

fn main() -> ():
    let session = Session { value: 1 }
    println(session.secret())
"#,
    );
}

#[test]
fn option_defaults_to_none_without_a_payload_default() {
    // `docs/spec.md` 4.3: `Option[T]` defaults to `Option.None` without requiring
    // `T` to implement `Default`, so neither a safe reference nor an
    // underivable struct blocks the enclosing derivation.
    assert_clean(
        r#"
struct Undefaultable:
    value: i32

struct Holder(Default):
    referenced: Option[&i32]
    nested: Option[Undefaultable]

fn main() -> ():
    let holder = Holder.default()
    match holder.nested:
        Option.Some(inner):
            println(inner.value)
        Option.None:
            println(0)
"#,
    );
    // A direct safe-reference field still has no default, so the rule is about
    // `Option` and not about references generally.
    assert_reports(
        r#"
struct Direct(Default):
    value: &i32

fn main() -> ():
    pass
"#,
        "cannot be derived",
    );
    // The rule names the standard declaration, not the spelling: a user enum
    // shadowing `Option` receives no intrinsic capability.
    assert_reports(
        r#"
enum Option[T]:
    Some(T)
    None

struct Holder(Default):
    slot: Option[i32]

fn main() -> ():
    pass
"#,
        "cannot be derived",
    );
}

#[test]
fn transfer_is_not_a_compiler_capability() {
    assert_clean(
        r#"
trait Transfer:
    pass

struct Packet:
    value: i32

impl Transfer for Packet:
    pass

fn cross[T: Transfer](value: T) -> ():
    pass

fn main() -> ():
    let packet = Packet { value: 7 }
    cross(packet)
"#,
    );

    assert_reports(
        r#"
trait Transfer:
    pass

fn cross[T: Transfer](value: T) -> ():
    pass

fn main() -> ():
    let value = 7
    cross(&value)
"#,
        "does not satisfy required `Transfer` capability",
    );
}

#[test]
fn unsafe_impl_has_no_reserved_capability() {
    assert_reports(
        r#"
trait Marker:
    pass

struct SharedForeign:
    pointer: *var i32

unsafe impl Marker for SharedForeign:
    pass

fn main() -> ():
    pass
"#,
        "`unsafe impl` is not part of the language",
    );
}
