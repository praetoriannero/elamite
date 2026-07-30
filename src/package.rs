//! Package identity, file-backed module-path derivation, and the resolved
//! package dependency graph, per `ROADMAP.md` Milestone 1 and `SPEC.md` §2.3.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::diagnostics::{Category, Diagnostic};
use crate::ident::is_valid_identifier;
use crate::manifest::Manifest;
use crate::source::SourceManager;

/// A dot-joined module path, rooted at the package's `root` module.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModulePath(Vec<String>);

impl ModulePath {
    #[must_use]
    pub fn components(&self) -> &[String] {
        &self.0
    }

    /// The `root.a.b` display form used in `SPEC.md` §2.3's own examples.
    #[must_use]
    pub fn display(&self) -> String {
        if self.0.is_empty() {
            "root".to_string()
        } else {
            format!("root.{}", self.0.join("."))
        }
    }
}

/// A package's opaque identity. Distinct even for two dependency instances
/// that display the same manifest `name`.
///
/// Under the initial local-path resolver specified by `SPEC.md` §2.3, the
/// canonicalized manifest-directory path is the instance discriminator.
/// Future resolvers may construct a different opaque key without changing how
/// the rest of the compiler consumes a [`PackageId`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageId(PathBuf);

impl PackageId {
    #[must_use]
    pub fn display(&self) -> std::path::Display<'_> {
        self.0.display()
    }
}

fn canonical_dir(dir: &Path) -> PathBuf {
    dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf())
}

/// One resolved package: its manifest, source directory, and discovered
/// file-backed module map.
#[derive(Debug)]
pub struct Package {
    pub id: PackageId,
    pub manifest: Manifest,
    /// Directory containing the manifest.
    pub manifest_dir: PathBuf,
    /// Directory containing the root source file (`SPEC.md` §2.3: "the
    /// directory containing the selected root file is the package's source
    /// directory").
    pub source_dir: PathBuf,
    /// Every file-backed module discovered beneath `source_dir` other than
    /// the root file itself, keyed by its derived module path in
    /// deterministic (sorted) order.
    pub modules: BTreeMap<ModulePath, PathBuf>,
}

impl Package {
    /// Loads and validates the package rooted at `manifest_path` (an
    /// `elamite.toml` file), discovering every file-backed module beneath its
    /// source directory. Manifest source text is registered with `sources` so
    /// callers can render any returned diagnostics with real spans.
    pub fn load(
        manifest_path: &Path,
        sources: &mut SourceManager,
    ) -> Result<Package, Vec<Diagnostic>> {
        let manifest = Manifest::load(manifest_path, sources)?;
        let manifest_dir = manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let root_file = manifest_dir.join(&manifest.root);

        if !root_file.is_file() {
            return Err(vec![Diagnostic::new(
                Category::ManifestInvalid,
                format!("root source file {} does not exist", root_file.display()),
            )]);
        }

        let source_dir = root_file
            .parent()
            .expect("root_file was joined from manifest_dir, so it has a parent")
            .to_path_buf();

        let modules = discover_modules(&source_dir, &root_file)?;
        let id = PackageId(canonical_dir(&manifest_dir));

        Ok(Package {
            id,
            manifest,
            manifest_dir,
            source_dir,
            modules,
        })
    }
}

fn invalid_component_diagnostic(relative: &Path) -> Diagnostic {
    Diagnostic::new(
        Category::ModuleDiscoveryInvalid,
        format!(
            "{} contains a path component that is not a valid identifier",
            relative.display()
        ),
    )
}

/// Converts one `.elx` file's path, relative to the package source
/// directory, into a module path, validating that every component (each
/// directory segment, plus the file stem) is a legal identifier.
fn module_path_for(relative: &Path) -> Result<ModulePath, Diagnostic> {
    let mut components = Vec::new();

    if let Some(parent) = relative.parent() {
        for component in parent.components() {
            let std::path::Component::Normal(part) = component else {
                return Err(invalid_component_diagnostic(relative));
            };
            let part = part
                .to_str()
                .ok_or_else(|| invalid_component_diagnostic(relative))?;
            if !is_valid_identifier(part) {
                return Err(invalid_component_diagnostic(relative));
            }
            components.push(part.to_string());
        }
    }

    let stem = relative
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| invalid_component_diagnostic(relative))?;
    if !is_valid_identifier(stem) {
        return Err(invalid_component_diagnostic(relative));
    }
    components.push(stem.to_string());

    Ok(ModulePath(components))
}

fn collect_elx_files(dir: &Path, out: &mut Vec<PathBuf>, diagnostics: &mut Vec<Diagnostic>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            diagnostics.push(Diagnostic::new(
                Category::ModuleDiscoveryInvalid,
                format!("cannot read directory {}: {error}", dir.display()),
            ));
            return;
        }
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.is_dir() {
            collect_elx_files(&path, out, diagnostics);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("elx") {
            out.push(path);
        }
    }
}

/// Discovers every `.elx` file beneath `source_dir` other than `root_file`
/// and derives its module path. `root_file` is excluded because it defines
/// the package's `root` module, not a file-backed module (`SPEC.md` §2.3:
/// "Every *other* `.elx` file beneath the source directory...").
///
/// Discovery order is deterministic: entries are sorted before they can
/// affect output, per `ROADMAP.md` §2.2.
pub fn discover_modules(
    source_dir: &Path,
    root_file: &Path,
) -> Result<BTreeMap<ModulePath, PathBuf>, Vec<Diagnostic>> {
    let mut files = Vec::new();
    let mut diagnostics = Vec::new();
    collect_elx_files(source_dir, &mut files, &mut diagnostics);
    files.sort();

    let mut modules = BTreeMap::new();
    for file in files {
        if file == root_file {
            continue;
        }
        let relative = file
            .strip_prefix(source_dir)
            .expect("file was discovered beneath source_dir");
        match module_path_for(relative) {
            Ok(path) => {
                if let Some(existing) = modules.insert(path.clone(), file.clone()) {
                    diagnostics.push(Diagnostic::new(
                        Category::ModuleDiscoveryInvalid,
                        format!(
                            "{} and {} both define module `{}`",
                            existing.display(),
                            file.display(),
                            path.display()
                        ),
                    ));
                }
            }
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }

    if diagnostics.is_empty() {
        Ok(modules)
    } else {
        Err(diagnostics)
    }
}

/// A resolved graph of packages: the root package under compilation plus
/// every (transitive) dependency, keyed by [`PackageId`].
///
/// `ROADMAP.md` Milestone 1: "Separate dependency resolution from compilation.
/// The compiler pipeline should consume a resolved package graph so that a
/// future resolver can be replaced without changing semantic analysis." This
/// graph is that consumed artifact; [`PackageGraph::resolve`] implements the
/// initial local-path resolver defined by `SPEC.md` §2.3.
#[derive(Debug)]
pub struct PackageGraph {
    pub root: PackageId,
    pub packages: BTreeMap<PackageId, Package>,
    /// Resolved outgoing edges for every package, keyed first by the depending
    /// package and then by the dependency alias written in its manifest.
    pub dependency_edges: BTreeMap<PackageId, BTreeMap<String, PackageId>>,
}

impl PackageGraph {
    /// Resolves the package at `manifest_path` and every dependency it
    /// declares, transitively, detecting cycles in the dependency graph.
    /// `SPEC.md` §2.3: "The package dependency graph must be acyclic" — this
    /// is a stronger requirement than the import-cycle rule within one
    /// package, which is explicitly permitted and is not this function's
    /// concern.
    /// `sources` accumulates the source text of every manifest visited during
    /// resolution (root and every transitive dependency), so the caller can
    /// still render span-carrying diagnostics on the error path — `sources`
    /// stays owned by the caller and is populated regardless of whether
    /// resolution ultimately succeeds.
    pub fn resolve(
        manifest_path: &Path,
        sources: &mut SourceManager,
    ) -> Result<PackageGraph, Vec<Diagnostic>> {
        let mut packages = BTreeMap::new();
        let mut dependency_edges = BTreeMap::new();
        let mut stack = Vec::new();
        let root = resolve_recursive(
            manifest_path,
            &mut packages,
            &mut dependency_edges,
            &mut stack,
            sources,
        )?;
        Ok(PackageGraph {
            root,
            packages,
            dependency_edges,
        })
    }

    /// Returns the package selected by `alias` from `package`.
    #[must_use]
    pub fn dependency(&self, package: &PackageId, alias: &str) -> Option<&PackageId> {
        self.dependency_edges.get(package)?.get(alias)
    }

    /// Packages in deterministic dependency-first order. Each package occurs
    /// exactly once even when several aliases share it.
    #[must_use]
    pub fn dependency_order(&self) -> Vec<&PackageId> {
        fn visit<'a>(
            graph: &'a PackageGraph,
            package: &'a PackageId,
            visited: &mut std::collections::BTreeSet<PackageId>,
            output: &mut Vec<&'a PackageId>,
        ) {
            if !visited.insert(package.clone()) {
                return;
            }
            if let Some(edges) = graph.dependency_edges.get(package) {
                for dependency in edges.values() {
                    visit(graph, dependency, visited, output);
                }
            }
            output.push(package);
        }

        let mut visited = std::collections::BTreeSet::new();
        let mut output = Vec::new();
        visit(self, &self.root, &mut visited, &mut output);
        output
    }
}

fn resolve_recursive(
    manifest_path: &Path,
    packages: &mut BTreeMap<PackageId, Package>,
    dependency_edges: &mut BTreeMap<PackageId, BTreeMap<String, PackageId>>,
    stack: &mut Vec<PackageId>,
    sources: &mut SourceManager,
) -> Result<PackageId, Vec<Diagnostic>> {
    let manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let probe_id = PackageId(canonical_dir(manifest_dir));

    if stack.contains(&probe_id) {
        let mut cycle = stack
            .iter()
            .map(|id| id.display().to_string())
            .collect::<Vec<_>>();
        cycle.push(probe_id.display().to_string());
        return Err(vec![Diagnostic::new(
            Category::PackageGraphInvalid,
            format!("cyclic package dependency: {}", cycle.join(" -> ")),
        )]);
    }
    if packages.contains_key(&probe_id) {
        return Ok(probe_id);
    }

    let package = Package::load(manifest_path, sources)?;
    let id = package.id.clone();
    let dependency_decls = package.manifest.dependencies.clone();

    stack.push(id.clone());
    packages.insert(id.clone(), package);

    let mut resolved_dependencies = BTreeMap::new();
    for dependency in dependency_decls {
        let dependency_manifest = dependency.path.join("elamite.toml");
        let dependency_id = resolve_recursive(
            &dependency_manifest,
            packages,
            dependency_edges,
            stack,
            sources,
        )?;
        resolved_dependencies.insert(dependency.alias, dependency_id);
    }

    stack.pop();
    dependency_edges.insert(id.clone(), resolved_dependencies);
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_a_simple_module_path() {
        let path = module_path_for(Path::new("models.elx")).expect("should derive");
        assert_eq!(path.display(), "root.models");
    }

    #[test]
    fn derives_a_nested_module_path() {
        let path = module_path_for(Path::new("codec/json.elx")).expect("should derive");
        assert_eq!(path.display(), "root.codec.json");
    }

    #[test]
    fn rejects_invalid_file_component() {
        assert!(module_path_for(Path::new("2bad.elx")).is_err());
    }

    #[test]
    fn rejects_invalid_directory_component() {
        assert!(module_path_for(Path::new("2bad/models.elx")).is_err());
    }
}
