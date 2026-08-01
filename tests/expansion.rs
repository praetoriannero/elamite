use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::config::CompilerFeatures;
use elamite::config::Target;
use elamite::driver::{DumpStage, dump_with_features};
use elamite::expansion::namespace::{
    CompileTimeBinding, CompileTimeNamespace, CompileTimeVisibility,
};
use elamite::expansion::scheduler::ExpansionLimits;
use elamite::expansion::{ExpandedUnitIdentity, expand, expand_with_features, expand_with_limits};
use elamite::package::PackageGraph;
use elamite::parsed::{ParsedUnitIdentity, parse_package};
use elamite::resolution::{resolve, resolve_expanded};
use elamite::source::SourceManager;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestWorkspace {
    root: PathBuf,
}

impl TestWorkspace {
    fn new(name: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "elamite-expansion-{}-{name}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create expansion test workspace");
        Self { root }
    }

    fn package(&self, name: &str, dependencies: &[(&str, &str)], source: &str) -> PathBuf {
        let package = self.root.join(name);
        fs::create_dir_all(package.join("src")).expect("create package source directory");
        let mut manifest =
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\ntarget_kind = \"lib\"\n");
        for (alias, path) in dependencies {
            manifest.push_str(&format!("\n[dependencies.{alias}]\npath = \"{path}\"\n"));
        }
        fs::write(package.join("elamite.toml"), manifest).expect("write package manifest");
        fs::write(package.join("src/lib.elx"), source).expect("write package source");
        package
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn expand_package(package: &Path) -> (PackageGraph, elamite::expansion::ExpansionOutput) {
    expand_package_with_features(package, CompilerFeatures::default())
}

fn expand_package_with_features(
    package: &Path,
    features: CompilerFeatures,
) -> (PackageGraph, elamite::expansion::ExpansionOutput) {
    let mut sources = SourceManager::new();
    let graph = PackageGraph::resolve(&package.join("elamite.toml"), &mut sources)
        .expect("package graph resolves");
    let parsed = parse_package(&graph, &mut sources);
    let expanded = expand_with_features(&graph, parsed, features);
    (graph, expanded)
}

fn spec_demo_manifest() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/spec_demo/elamite.toml")
}

fn graph(sources: &mut SourceManager) -> PackageGraph {
    PackageGraph::resolve(&spec_demo_manifest(), sources).expect("spec demo package graph resolves")
}

#[test]
fn expansion_owns_unit_identities_and_preserves_macro_free_inputs() {
    let mut sources = SourceManager::new();
    let graph = graph(&mut sources);
    let parsed = parse_package(&graph, &mut sources);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let expected = parsed
        .package
        .units
        .iter()
        .map(|unit| {
            (
                unit.identity.clone(),
                unit.path.clone(),
                unit.file,
                unit.span,
                unit.tokens.clone(),
                unit.tree.clone(),
            )
        })
        .collect::<Vec<_>>();
    let expanded = expand(&graph, parsed);
    assert!(expanded.diagnostics.is_empty());
    assert_eq!(expanded.package.units.len(), expected.len());

    for (unit, (identity, path, file, span, tokens, tree)) in
        expanded.package.units.iter().zip(expected)
    {
        let expected_identity = match identity {
            ParsedUnitIdentity::Standard(module) => ExpandedUnitIdentity::Standard(module),
            ParsedUnitIdentity::PackageRoot(package) => ExpandedUnitIdentity::PackageRoot(package),
            ParsedUnitIdentity::PackageModule { package, path } => {
                ExpandedUnitIdentity::PackageModule { package, path }
            }
        };
        assert_eq!(unit.identity, expected_identity);
        assert_eq!(unit.path, path);
        assert_eq!(unit.file, file);
        assert_eq!(unit.span, span);
        assert_eq!(unit.tokens, tokens);
        assert_eq!(unit.tree, tree);
        assert_eq!(unit.token_trees.flattened().len(), unit.tokens.len());
    }
}

#[test]
fn explicit_expansion_and_the_normal_resolver_entry_point_are_equivalent() {
    let mut direct_sources = SourceManager::new();
    let direct_graph = graph(&mut direct_sources);
    let direct = resolve(&direct_graph, &mut direct_sources);

    let mut explicit_sources = SourceManager::new();
    let explicit_graph = graph(&mut explicit_sources);
    let parsed = parse_package(&explicit_graph, &mut explicit_sources);
    let explicit = resolve_expanded(&explicit_graph, expand(&explicit_graph, parsed));

    assert_eq!(direct.diagnostics.len(), explicit.diagnostics.len());
    assert_eq!(direct.program.dump(), explicit.program.dump());
}

#[test]
fn every_macro_free_example_preserves_the_explicit_expansion_boundary() {
    for relative in [
        "examples/c_ffi/elamite.toml",
        "examples/closures/elamite.toml",
        "examples/hello/elamite.toml",
        "examples/sdl/elamite.toml",
        "examples/spec_demo/elamite.toml",
    ] {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
        let mut direct_sources = SourceManager::new();
        let direct_graph = PackageGraph::resolve(&manifest, &mut direct_sources)
            .unwrap_or_else(|diagnostics| panic!("{relative}: {diagnostics:?}"));
        let direct = resolve(&direct_graph, &mut direct_sources);

        let mut explicit_sources = SourceManager::new();
        let explicit_graph = PackageGraph::resolve(&manifest, &mut explicit_sources)
            .unwrap_or_else(|diagnostics| panic!("{relative}: {diagnostics:?}"));
        let parsed = parse_package(&explicit_graph, &mut explicit_sources);
        let explicit = resolve_expanded(&explicit_graph, expand(&explicit_graph, parsed));

        let direct_diagnostics = direct
            .diagnostics
            .iter()
            .map(|diagnostic| {
                (
                    diagnostic.category,
                    diagnostic.message.clone(),
                    diagnostic.primary,
                    diagnostic.related.clone(),
                )
            })
            .collect::<Vec<_>>();
        let explicit_diagnostics = explicit
            .diagnostics
            .iter()
            .map(|diagnostic| {
                (
                    diagnostic.category,
                    diagnostic.message.clone(),
                    diagnostic.primary,
                    diagnostic.related.clone(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(direct_diagnostics, explicit_diagnostics, "{relative}");
        assert_eq!(direct.program.dump(), explicit.program.dump(), "{relative}");
    }
}

#[test]
fn compile_time_declarations_use_separate_module_namespaces() {
    let workspace = TestWorkspace::new("separate-namespaces");
    let package = workspace.package(
        "app",
        &[],
        r#"
fn shared() -> ():
    pass

macro shared(value: std.ast.Expression) -> std.ast.Expression:
    pass

attr shared(target: std.ast.StructDefinition) -> std.ast.StructDefinition:
    pass

trait Shared:
    pass

derive Shared(target: std.ast.StructDefinition) -> std.ast.Implementation:
    pass

mod nested:
    pub macro hidden(value: std.ast.Expression) -> std.ast.Expression:
        pass

use macro root.nested.hidden as imported
"#,
    );
    let (graph, output) = expand_package(&package);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let environment = &output.package.compile_time;
    let root = environment
        .module(&graph.root, &[])
        .expect("root compile-time module exists");

    assert!(
        environment
            .lookup(root, CompileTimeNamespace::Macro, "shared")
            .is_some()
    );
    assert!(
        environment
            .lookup(root, CompileTimeNamespace::Attribute, "shared")
            .is_some()
    );
    assert!(
        environment
            .lookup(root, CompileTimeNamespace::Derive, "Shared")
            .is_some()
    );
    let CompileTimeBinding::Import(import) = environment
        .lookup(root, CompileTimeNamespace::Macro, "imported")
        .expect("renamed macro import is bound")
    else {
        panic!("expected an import binding");
    };
    let target = environment.imports[import.index()]
        .target
        .expect("same-package import resolves");
    assert_eq!(environment.declarations[target.index()].name, "hidden");
}

#[test]
fn typed_quotes_validate_roles_only_inside_compile_time_declarations() {
    let workspace = TestWorkspace::new("typed-quotes");
    let valid = workspace.package(
        "valid",
        &[],
        r#"
macro build(value: std.ast.Expression) -> std.ast.Expression:
    let expression: std.ast.Expression = quote:
        ($value, 1)
    let statements: std.ast.StatementList = quote:
        let copied = $value
        return copied
    let members: std.ast.MemberList = quote:
        id: u64

        pub fn identifier(self: &Self) -> u64:
            return self.id
    return quote:
        call($expression)
"#,
    );
    let (_, output) = expand_package(&valid);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);

    let invalid = workspace.package(
        "invalid",
        &[],
        r#"
macro bad() -> std.ast.Expression:
    let ambiguous = quote:
        1
    let wrong: std.ast.Item = quote:
        1 + 2
    return quote:
        0

fn runtime() -> ():
    let forbidden: std.ast.Expression = quote:
        1
"#,
    );
    let (_, output) = expand_package(&invalid);
    let messages = output
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("no expected `std.ast` role"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("expected a declaration"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("only inside a compile-time"))
    );
}

#[test]
fn quotation_remains_compile_time_only_after_stabilization() {
    let workspace = TestWorkspace::new("runtime-quote");
    let package = workspace.package(
        "app",
        &[],
        r#"
fn runtime() -> ():
    let syntax = quote:
        1
"#,
    );
    let (_, output) = expand_package_with_features(&package, CompilerFeatures::default());
    assert!(output.diagnostics.iter().any(|diagnostic| {
        diagnostic.category == elamite::diagnostics::Category::CompileTime
            && diagnostic.message.contains("only inside a compile-time")
    }));
}

#[test]
fn compile_time_imports_resolve_public_cross_package_reexports() {
    let workspace = TestWorkspace::new("cross-package");
    workspace.package(
        "dep",
        &[],
        r#"
pub macro exported(value: std.ast.Expression) -> std.ast.Expression:
    pass

macro private(value: std.ast.Expression) -> std.ast.Expression:
    pass

pub attr exported(target: std.ast.StructDefinition) -> std.ast.StructDefinition:
    pass

pub derive Feature(target: std.ast.StructDefinition) -> std.ast.Implementation:
    pass

pub use macro root.exported as forwarded
use macro root.exported as local_only
"#,
    );
    let app = workspace.package(
        "app",
        &[("dep", "../dep")],
        r#"
use macro dep.forwarded as shared
use attr dep.exported as shared
use derive dep.Feature as shared
use macro dep.private as denied_private
use macro dep.local_only as denied_reexport
"#,
    );
    let (graph, output) = expand_package(&app);
    let environment = &output.package.compile_time;
    let root = environment
        .module(&graph.root, &[])
        .expect("root compile-time module exists");

    for namespace in [
        CompileTimeNamespace::Macro,
        CompileTimeNamespace::Attribute,
        CompileTimeNamespace::Derive,
    ] {
        let CompileTimeBinding::Import(import) = environment
            .lookup(root, namespace, "shared")
            .expect("same alias is legal in each namespace")
        else {
            panic!("expected an import binding");
        };
        assert!(environment.imports[import.index()].target.is_some());
    }
    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.category == elamite::diagnostics::Category::Visibility)
            .count(),
        2,
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn duplicate_compile_time_bindings_conflict_only_within_one_namespace() {
    let workspace = TestWorkspace::new("duplicates");
    let package = workspace.package(
        "app",
        &[],
        r#"
macro duplicate(value: std.ast.Expression) -> std.ast.Expression:
    pass

macro duplicate(value: std.ast.Expression) -> std.ast.Expression:
    pass

attr duplicate(target: std.ast.StructDefinition) -> std.ast.StructDefinition:
    pass

use attr root.duplicate as duplicate
"#,
    );
    let (_, output) = expand_package(&package);
    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.category == elamite::diagnostics::Category::DeclarationConflict
            })
            .count(),
        2,
        "{:?}",
        output.diagnostics
    );
    assert!(output.diagnostics.iter().all(|diagnostic| {
        diagnostic.message.contains("macro `duplicate`")
            || diagnostic.message.contains("attribute `duplicate`")
    }));
    assert!(
        output
            .package
            .compile_time
            .declarations
            .iter()
            .any(|declaration| {
                declaration.namespace == CompileTimeNamespace::Attribute
                    && declaration.visibility == CompileTimeVisibility::Package
            })
    );
}

#[test]
fn user_compile_time_forms_are_stable_without_an_opt_in() {
    let workspace = TestWorkspace::new("stable-macros");
    let package = workspace.package(
        "app",
        &[],
        r#"
macro make(value: std.ast.Expression) -> std.ast.Expression:
    return quote:
        $value

fn invoke() -> i32:
    return @make(42)
"#,
    );
    let (_, output) = expand_package_with_features(&package, CompilerFeatures::default());
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.package.schedule.usage().executions, 1);
}

#[test]
fn compiler_macros_ffi_attributes_and_compact_derives_remain_ungated() {
    let workspace = TestWorkspace::new("stable-builtins");
    let package = workspace.package(
        "app",
        &[],
        r#"
@importc("native_value", "native.h")
fn native_value() -> i32

struct Stable(i32) derives (Default, PartialEq)

@derive(Default, PartialEq)
struct Attached:
    value: i32

fn values() -> ():
    let vector = @vec[1, 2]
    let mapping = @map{1: 2}
    let unique = @set{1, 2}
"#,
    );
    let (_, output) = expand_package_with_features(&package, CompilerFeatures::default());
    assert!(output.diagnostics.iter().all(|diagnostic| {
        diagnostic.category != elamite::diagnostics::Category::ExperimentalFeature
    }));
    let user = output
        .package
        .units
        .iter()
        .find(|unit| !unit.is_standard())
        .expect("user unit");
    assert_eq!(user.tree.count(elamite::syntax::SyntaxKind::DeriveList), 1);
}

#[test]
fn function_macro_executes_quote_interpolation_before_resolution() {
    let workspace = TestWorkspace::new("execute-function-macro");
    let package = workspace.package(
        "app",
        &[],
        r#"
macro pair(left: std.ast.Expression, right: std.ast.Expression) -> std.ast.Expression:
    let result: std.ast.Expression = quote:
        ($left, $right)
    return result

fn value() -> (i32, i32):
    return @pair(20, 22)
"#,
    );
    let (graph, output) = expand_package(&package);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let tree = &output
        .package
        .units
        .iter()
        .find(|unit| !unit.is_standard())
        .expect("user unit")
        .tree;
    assert_eq!(tree.count(elamite::syntax::SyntaxKind::MacroExpression), 0);
    assert_eq!(tree.count(elamite::syntax::SyntaxKind::TupleExpression), 1);
    assert_eq!(output.package.schedule.usage().executions, 1);
    let resolved = resolve_expanded(&graph, output);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
}

#[test]
fn structural_attribute_can_add_members_immutably() {
    let workspace = TestWorkspace::new("execute-attribute");
    let package = workspace.package(
        "app",
        &[],
        r#"
attr identifiable(target: std.ast.StructDefinition) -> std.ast.StructDefinition:
    let additions: std.ast.MemberList = quote:
        id: u64
    return target.with_members(target.members() ++ additions)

@attr(identifiable)
struct Entity:
    name: String
"#,
    );
    let (_, output) = expand_package(&package);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let user = output
        .package
        .units
        .iter()
        .find(|unit| !unit.is_standard())
        .expect("user unit");
    let structure = user
        .tree
        .direct_children(elamite::syntax::SyntaxKind::Struct)[0];
    let fields = structure
        .direct_child(elamite::syntax::SyntaxKind::Block)
        .expect("struct body")
        .direct_children(elamite::syntax::SyntaxKind::Field);
    assert_eq!(fields.len(), 2, "{}", structure.dump());
    assert!(
        structure
            .direct_children(elamite::syntax::SyntaxKind::Attribute)
            .is_empty()
    );
}

#[test]
fn attributes_can_remove_emit_siblings_and_continue_transforming_the_target() {
    let workspace = TestWorkspace::new("attribute-item-lists");
    let package = workspace.package(
        "app",
        &[],
        r#"
attr remove(target: std.ast.StructDefinition) -> std.ast.ItemList:
    return std.ast.ItemList.empty()

attr sibling(target: std.ast.StructDefinition) -> std.ast.ItemList:
    return quote:
        $target

        fn companion() -> i32:
            return 42

attr rename(target: std.ast.StructDefinition) -> std.ast.StructDefinition:
    return target.with_name(std.ast.identifier("Renamed"))

@attr(remove)
struct Removed:
    value: i32

@attr(sibling)
@attr(rename)
struct Original:
    value: i32
"#,
    );
    let (graph, output) = expand_package(&package);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let user = output
        .package
        .units
        .iter()
        .find(|unit| !unit.is_standard())
        .expect("user unit");
    assert_eq!(user.tree.count(elamite::syntax::SyntaxKind::Struct), 1);
    assert_eq!(user.tree.count(elamite::syntax::SyntaxKind::Function), 1);
    assert!(user.tree.dump().contains("Identifier(\"Renamed\")"));
    assert!(!user.tree.dump().contains("Identifier(\"Removed\")"));
    let resolved = resolve_expanded(&graph, output);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
}

#[test]
fn derive_runs_after_attributes_and_emits_an_implementation() {
    let workspace = TestWorkspace::new("execute-derive");
    let package = workspace.package(
        "app",
        &[],
        r#"
trait FieldCount:
    fn field_count() -> usize

derive FieldCount(target: std.ast.StructDefinition) -> std.ast.Implementation:
    let target_type = target.type_syntax()
    let implementation: std.ast.Implementation = quote:
        impl FieldCount for $target_type:
            fn field_count() -> usize:
                return 1
    return implementation

@derive(FieldCount)
struct User:
    name: String
"#,
    );
    let (_, output) = expand_package(&package);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let user = output
        .package
        .units
        .iter()
        .find(|unit| !unit.is_standard())
        .expect("user unit");
    assert_eq!(user.tree.count(elamite::syntax::SyntaxKind::Struct), 1);
    assert_eq!(user.tree.count(elamite::syntax::SyntaxKind::Impl), 1);
    assert_eq!(output.package.schedule.usage().executions, 1);
}

#[test]
fn every_function_macro_role_and_variadic_arguments_expand() {
    let workspace = TestWorkspace::new("all-function-macro-roles");
    let package = workspace.package(
        "app",
        &[],
        r#"
macro make_type() -> std.ast.TypeSyntax:
    return quote:
        i32

macro wildcard() -> std.ast.Pattern:
    return quote:
        _

macro emit(value: std.ast.Expression) -> std.ast.StatementList:
    return quote:
        println($value)

macro make_item() -> std.ast.Item:
    return quote:
        fn generated() -> i32:
            return 42

macro tupled(values: ...std.ast.Expression) -> std.ast.Expression:
    return quote:
        ($values)

@make_item()

fn use_all(value: @make_type()) -> (@make_type(), @make_type()):
    @emit("expanded")
    match value:
        @wildcard():
            pass
    return @tupled(20, 22)
"#,
    );
    let (graph, output) = expand_package(&package);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let user = output
        .package
        .units
        .iter()
        .find(|unit| !unit.is_standard())
        .expect("user unit");
    assert_eq!(
        user.tree
            .count(elamite::syntax::SyntaxKind::MacroExpression),
        0
    );
    assert_eq!(output.package.schedule.usage().executions, 7);
    let resolved = resolve_expanded(&graph, output);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
}

#[test]
fn capability_checks_and_wrong_roles_fail_before_execution() {
    let workspace = TestWorkspace::new("compile-time-checking");
    let package = workspace.package(
        "app",
        &[],
        r#"
macro forbidden() -> std.ast.Expression:
    unsafe:
        pass
    return quote:
        1

macro wrong() -> std.ast.Pattern:
    return quote:
        _

fn value() -> i32:
    return @wrong()
"#,
    );
    let (_, output) = expand_package(&package);
    let messages = output
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("unsafe capability"))
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("Pattern syntax"))
    );
    assert_eq!(output.package.schedule.usage().executions, 0);
}

#[test]
fn failed_execution_recovers_and_independent_macros_continue() {
    let workspace = TestWorkspace::new("execution-recovery");
    let package = workspace.package(
        "app",
        &[],
        r#"
macro bad(value: std.ast.Expression) -> std.ast.Expression:
    std.ast.error(value, "intentional expansion failure")

macro good(value: std.ast.Expression) -> std.ast.Expression:
    return quote:
        $value

fn values() -> i32:
    let ignored = @bad(1)
    return @good(42)
"#,
    );
    let (_, output) = expand_package(&package);
    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("intentional expansion failure"))
            .count(),
        1,
        "{:?}",
        output.diagnostics
    );
    assert_eq!(output.package.schedule.usage().executions, 2);
    assert_eq!(
        output
            .package
            .schedule
            .work()
            .iter()
            .filter(|work| matches!(
                work.state,
                elamite::expansion::scheduler::ExpansionWorkState::Completed
            ))
            .count(),
        1
    );
}

#[test]
fn interpreter_fuel_and_recursive_expansion_cycles_are_contained() {
    let workspace = TestWorkspace::new("limits-and-cycles");
    let package = workspace.package(
        "app",
        &[],
        r#"
macro spins() -> std.ast.Expression:
    while true:
        pass
    return quote:
        0

macro recursive() -> std.ast.Expression:
    return quote:
        @recursive()

fn values() -> (i32, i32):
    return (@spins(), @recursive())
"#,
    );
    let mut sources = SourceManager::new();
    let graph = PackageGraph::resolve(&package.join("elamite.toml"), &mut sources)
        .expect("package graph resolves");
    let parsed = parse_package(&graph, &mut sources);
    let output = expand_with_limits(
        &graph,
        parsed,
        ExpansionLimits {
            interpreter_steps: 32,
            ..ExpansionLimits::default()
        },
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.message.contains("InterpreterSteps") }),
        "{:?}",
        output.diagnostics
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.message.contains("expansion cycle") }),
        "{:?}",
        output.diagnostics
    );
}

#[test]
fn expansion_dump_and_artifact_identities_are_reproducible() {
    let workspace = TestWorkspace::new("reproducible-expansion");
    let package = workspace.package(
        "app",
        &[],
        r#"
macro identity(value: std.ast.Expression) -> std.ast.Expression:
    return quote:
        $value

fn value() -> i32:
    return @identity(42)
"#,
    );
    let (_, first) = expand_package(&package);
    let (_, second) = expand_package(&package);
    assert!(first.diagnostics.is_empty(), "{:?}", first.diagnostics);
    assert!(second.diagnostics.is_empty(), "{:?}", second.diagnostics);
    assert_eq!(first.package.identities, second.package.identities);
    assert_eq!(first.package.dump(), second.package.dump());
}

#[test]
fn compile_time_execution_is_identical_for_x86_and_x86_64_targets() {
    let workspace = TestWorkspace::new("target-independent-expansion");
    let package = workspace.package(
        "app",
        &[],
        r#"
macro identity(value: std.ast.Expression) -> std.ast.Expression:
    return quote:
        $value

fn value() -> i32:
    return @identity(42)
"#,
    );
    let mut x86_sources = SourceManager::new();
    let x86_graph =
        PackageGraph::resolve(&package.join("elamite.toml"), &mut x86_sources).expect("x86 graph");
    let x86 = dump_with_features(
        &x86_graph,
        &mut x86_sources,
        Target::X86,
        DumpStage::Expanded,
        CompilerFeatures::default(),
    )
    .expect("x86 expansion");

    let mut x64_sources = SourceManager::new();
    let x64_graph = PackageGraph::resolve(&package.join("elamite.toml"), &mut x64_sources)
        .expect("x86-64 graph");
    let x64 = dump_with_features(
        &x64_graph,
        &mut x64_sources,
        Target::X86_64,
        DumpStage::Expanded,
        CompilerFeatures::default(),
    )
    .expect("x86-64 expansion");
    assert_eq!(x86, x64);
}

#[test]
fn quote_literals_use_definition_site_module_context_across_packages() {
    let workspace = TestWorkspace::new("definition-site-hygiene");
    workspace.package(
        "dep",
        &[],
        r#"
fn helper() -> i32:
    return 42

pub macro call_helper() -> std.ast.Expression:
    return quote:
        helper()
"#,
    );
    let app = workspace.package(
        "app",
        &[("dep", "../dep")],
        r#"
use macro dep.call_helper

fn value() -> i32:
    return @call_helper()
"#,
    );
    let (graph, output) = expand_package(&app);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    let resolved = resolve_expanded(&graph, output);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
}
