//! Integration tests for `elamite::package` and `elamite::manifest`, backed
//! by fixture packages under `tests/fixtures/packages/`. Covers `ROADMAP.md`
//! Milestone 1's validation list: single-file executable/library packages,
//! nested file-backed modules, custom root paths, missing roots, malformed
//! manifests, invalid module components, cyclic package dependencies, resolved
//! alias edges, and distinct identity for same-named dependencies.

use std::path::{Path, PathBuf};

use elamite::diagnostics::Category;
use elamite::manifest::TargetKind;
use elamite::package::{Package, PackageGraph};
use elamite::source::SourceManager;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/packages")
        .join(name)
}

fn manifest_at(dir: &Path) -> PathBuf {
    dir.join("elamite.toml")
}

#[test]
fn single_file_executable_has_no_extra_modules() {
    let mut sources = SourceManager::new();
    let package = Package::load(
        &manifest_at(&fixture("single_file_executable")),
        &mut sources,
    )
    .expect("package should load");
    assert_eq!(package.manifest.name, "single_file_executable");
    assert!(package.modules.is_empty());
}

#[test]
fn single_file_library_has_no_extra_modules() {
    let mut sources = SourceManager::new();
    let package = Package::load(&manifest_at(&fixture("single_file_library")), &mut sources)
        .expect("package should load");
    assert!(package.modules.is_empty());
}

#[test]
fn standalone_source_file_becomes_an_implicit_executable_package() {
    let source = fixture("single_file_executable").join("src/main.elx");
    let graph = PackageGraph::single_file(&source).expect("single source file should load");
    let package = &graph.packages[&graph.root];

    assert_eq!(package.manifest.name, "main");
    assert_eq!(package.manifest.version, "0.0.0");
    assert_eq!(package.manifest.target_kind, TargetKind::Executable);
    assert_eq!(package.manifest.root, Path::new("main.elx"));
    assert!(package.manifest.dependencies.is_empty());
    assert!(package.modules.is_empty());
    assert_eq!(graph.dependency_order(), vec![&graph.root]);
}

#[test]
fn standalone_source_file_requires_an_elx_extension() {
    let diagnostics = PackageGraph::single_file(&manifest_at(&fixture("single_file_executable")))
        .expect_err("manifest should not be accepted as a source file");
    assert_eq!(diagnostics[0].category, Category::PackageGraphInvalid);
    assert!(diagnostics[0].message.contains("`.elx` extension"));
}

#[test]
fn nested_file_backed_modules_derive_dotted_paths() {
    let mut sources = SourceManager::new();
    let package = Package::load(&manifest_at(&fixture("nested_modules")), &mut sources)
        .expect("package should load");
    let names: Vec<String> = package.modules.keys().map(|path| path.display()).collect();
    assert_eq!(names, vec!["root.codec", "root.codec.json", "root.models"]);
}

#[test]
fn custom_root_path_is_honored() {
    let mut sources = SourceManager::new();
    let package = Package::load(&manifest_at(&fixture("custom_root")), &mut sources)
        .expect("package should load");
    assert!(package.source_dir.ends_with("custom_root"));
    let names: Vec<String> = package.modules.keys().map(|path| path.display()).collect();
    assert_eq!(names, vec!["root.helper"]);
}

#[test]
fn missing_root_file_is_reported() {
    let mut sources = SourceManager::new();
    let diagnostics = Package::load(&manifest_at(&fixture("missing_root")), &mut sources)
        .expect_err("missing root file should fail to load");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.message.contains("does not exist"))
    );
}

#[test]
fn malformed_manifest_is_reported() {
    let mut sources = SourceManager::new();
    let diagnostics = Package::load(&manifest_at(&fixture("malformed_manifest")), &mut sources)
        .expect_err("malformed manifest should fail to load");
    assert!(
        diagnostics
            .iter()
            .all(|d| d.category == Category::ManifestInvalid)
    );
    assert!(
        diagnostics.iter().any(|d| d.primary.is_some()),
        "malformed TOML should carry a real span"
    );
}

#[test]
fn invalid_module_component_is_reported() {
    let mut sources = SourceManager::new();
    let diagnostics = Package::load(&manifest_at(&fixture("invalid_component")), &mut sources)
        .expect_err("invalid module component should fail to load");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.category == Category::ModuleDiscoveryInvalid)
    );
}

#[test]
fn cyclic_package_dependency_is_rejected() {
    let mut sources = SourceManager::new();
    let diagnostics =
        PackageGraph::resolve(&manifest_at(&fixture("cyclic_dependency/a")), &mut sources)
            .expect_err("cyclic dependency should be rejected");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.category == Category::PackageGraphInvalid)
    );
}

#[test]
fn two_dependencies_with_same_display_name_have_distinct_identity() {
    let mut sources = SourceManager::new();
    let graph = PackageGraph::resolve(
        &manifest_at(&fixture("distinct_identity/root")),
        &mut sources,
    )
    .expect("graph should resolve");
    assert_eq!(graph.packages.len(), 3);

    let shared: Vec<_> = graph
        .packages
        .values()
        .filter(|package| package.manifest.name == "shared")
        .collect();
    assert_eq!(shared.len(), 2, "both dependencies display as `shared`");
    assert_ne!(
        shared[0].id, shared[1].id,
        "same-named dependencies at different paths must have distinct identity"
    );

    let x = graph
        .dependency(&graph.root, "x")
        .expect("root dependency alias `x` should resolve");
    let y = graph
        .dependency(&graph.root, "y")
        .expect("root dependency alias `y` should resolve");
    assert_ne!(x, y);
    assert_eq!(graph.packages[x].manifest.name, "shared");
    assert_eq!(graph.packages[y].manifest.name, "shared");
}

#[test]
fn preserves_transitive_dependency_alias_edges() {
    let mut sources = SourceManager::new();
    let graph = PackageGraph::resolve(&manifest_at(&fixture("transitive/root")), &mut sources)
        .expect("graph should resolve");
    assert_eq!(graph.packages.len(), 3);

    let middle = graph
        .dependency(&graph.root, "middle")
        .expect("root alias `middle` should resolve");
    assert_eq!(graph.packages[middle].manifest.name, "middle");

    let leaf = graph
        .dependency(middle, "leaf")
        .expect("middle alias `leaf` should resolve");
    assert_eq!(graph.packages[leaf].manifest.name, "leaf");
    assert!(graph.dependency(leaf, "anything").is_none());
}
