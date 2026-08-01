use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::config::CompilerFeatures;
use elamite::expansion::namespace::{
    CompileTimeBinding, CompileTimeNamespace, CompileTimeVisibility,
};
use elamite::expansion::{ExpandedUnitIdentity, expand, expand_with_features};
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
    expand_package_with_features(
        package,
        CompilerFeatures {
            unstable_macros: true,
        },
    )
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
fn user_compile_time_forms_require_the_explicit_unstable_gate() {
    let workspace = TestWorkspace::new("unstable-gate");
    let package = workspace.package(
        "app",
        &[],
        r#"
macro make(value: std.ast.Expression) -> std.ast.Expression:
    pass

attr mark(target: std.ast.StructDefinition) -> std.ast.StructDefinition:
    pass

trait Feature:
    pass

derive Feature(target: std.ast.StructDefinition) -> std.ast.Implementation:
    pass

use macro root.make as imported_make
use attr root.mark as imported_mark
use derive root.Feature as ImportedFeature

@attr(mark)
@derive(Feature)
struct Marked:
    value: i32

fn invoke() -> ():
    let value = @custom(1)
"#,
    );
    let (_, stable) = expand_package_with_features(&package, CompilerFeatures::default());
    let gated = stable
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.category == elamite::diagnostics::Category::ExperimentalFeature
        })
        .collect::<Vec<_>>();
    assert_eq!(gated.len(), 9, "{:?}", stable.diagnostics);
    assert!(
        gated
            .iter()
            .all(|diagnostic| diagnostic.message.contains("--unstable-macros"))
    );

    let (_, experimental) = expand_package_with_features(
        &package,
        CompilerFeatures {
            unstable_macros: true,
        },
    );
    assert!(experimental.diagnostics.iter().all(|diagnostic| {
        diagnostic.category != elamite::diagnostics::Category::ExperimentalFeature
    }));
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
}
