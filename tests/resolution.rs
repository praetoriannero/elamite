use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::diagnostics::{Category, Diagnostic};
use elamite::package::PackageGraph;
use elamite::resolution::{LocalBindingKind, NameTarget, Visibility, resolve};
use elamite::source::SourceManager;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestTree {
    root: PathBuf,
}

impl TestTree {
    fn new(name: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "elamite-resolution-{}-{name}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create test tree");
        Self { root }
    }

    fn package(
        &self,
        relative: &str,
        target_kind: &str,
        dependencies: &[(&str, &str)],
        files: &[(&str, &str)],
    ) -> PathBuf {
        let directory = self.root.join(relative);
        fs::create_dir_all(directory.join("src")).expect("create package source directory");
        let mut manifest = format!(
            "[package]\nname = \"{relative}\"\nversion = \"0.1.0\"\ntarget_kind = \"{target_kind}\"\n"
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

fn resolve_package(package: &Path) -> (SourceManager, elamite::resolution::ResolutionOutput) {
    let mut sources = SourceManager::new();
    let graph = PackageGraph::resolve(&package.join("elamite.toml"), &mut sources)
        .expect("package graph should resolve");
    let output = resolve(&graph, &mut sources);
    (sources, output)
}

/// The declarations collected from the package under test, excluding the
/// compiler-supplied standard declarations (`Option`, …) that live in the
/// package-less `std` module tree. Counting only package declarations keeps a
/// test independent of how many standard types exist.
fn package_declarations(
    program: &elamite::resolution::ResolvedProgram,
) -> impl Iterator<Item = &elamite::resolution::Declaration> {
    program.declarations.iter().filter(|declaration| {
        program.modules[declaration.module.index()]
            .package
            .is_some()
    })
}

/// The enum variants declared by the package under test, on the same basis as
/// [`package_declarations`].
fn package_variants(
    program: &elamite::resolution::ResolvedProgram,
) -> impl Iterator<Item = &elamite::resolution::Variant> {
    program.variants.iter().filter(|variant| {
        let declaration = &program.declarations[variant.parent.index()];
        program.modules[declaration.module.index()]
            .package
            .is_some()
    })
}

fn package_imports(
    program: &elamite::resolution::ResolvedProgram,
) -> impl Iterator<Item = &elamite::resolution::Import> {
    program
        .imports
        .iter()
        .filter(|import| program.modules[import.module.index()].package.is_some())
}

fn package_local_bindings<'a>(
    program: &'a elamite::resolution::ResolvedProgram,
    sources: &'a SourceManager,
) -> impl Iterator<Item = &'a elamite::resolution::LocalBinding> {
    program.local_bindings.iter().filter(|binding| {
        !sources
            .path(binding.span.file)
            .to_string_lossy()
            .starts_with("<std>")
    })
}

fn diagnostic_text(sources: &SourceManager, diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| {
            let position = diagnostic.primary.map_or_else(
                || "<no span>".to_string(),
                |span| {
                    let position = sources.line_col(span.file, span.start);
                    format!(
                        "{}:{}:{}",
                        sources.path(span.file).display(),
                        position.line,
                        position.column
                    )
                },
            );
            format!(
                "{position}: {:?}: {}",
                diagnostic.category, diagnostic.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn predeclares_functions_and_allows_local_shadowing() {
    let tree = TestTree::new("predeclare");
    let package = tree.package(
        "app",
        "exe",
        &[],
        &[(
            "src/main.elx",
            "fn first() -> i32:\n\
                 \x20\x20\x20\x20return second()\n\
             fn second() -> i32:\n\
                 \x20\x20\x20\x20return first()\n\
             fn shadow() -> i32:\n\
                 \x20\x20\x20\x20let first = 1\n\
                 \x20\x20\x20\x20return first\n",
        )],
    );
    let (sources, output) = resolve_package(&package);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        diagnostic_text(&sources, &output.diagnostics)
    );
    assert_eq!(
        package_declarations(&output.program)
            .filter(|declaration| declaration.parent_declaration.is_none())
            .count(),
        3
    );
    assert_eq!(
        package_local_bindings(&output.program, &sources)
            .filter(|binding| binding.kind == LocalBindingKind::Local)
            .count(),
        1
    );
    assert!(
        output
            .program
            .references
            .iter()
            .any(|reference| matches!(reference.target, NameTarget::Local(_)))
    );
    let (_, repeated) = resolve_package(&package);
    assert_eq!(output.program.dump(), repeated.program.dump());
}

#[test]
fn resolves_the_authoritative_demonstration() {
    let tree = TestTree::new("spec-demo");
    let package = tree.package(
        "demo",
        "exe",
        &[],
        &[("src/main.elx", include_str!("../examples/spec_demo.elx"))],
    );
    let (sources, output) = resolve_package(&package);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        diagnostic_text(&sources, &output.diagnostics)
    );
    assert!(output.program.declarations.len() >= 30);
    assert!(output.program.local_bindings.len() >= 50);
    assert!(output.program.references.len() >= 100);
}

#[test]
fn resolves_circular_imports_after_collecting_declarations() {
    let tree = TestTree::new("import-cycle");
    let package = tree.package(
        "library",
        "lib",
        &[],
        &[(
            "src/lib.elx",
            "pub mod left:\n\
                 \x20\x20\x20\x20use super.right.Right\n\
                 \x20\x20\x20\x20pub struct Left:\n\
                 \x20\x20\x20\x20\x20\x20\x20\x20pass\n\
             pub mod right:\n\
                 \x20\x20\x20\x20use super.left.Left\n\
                 \x20\x20\x20\x20pub struct Right:\n\
                 \x20\x20\x20\x20\x20\x20\x20\x20pass\n",
        )],
    );
    let (sources, output) = resolve_package(&package);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        diagnostic_text(&sources, &output.diagnostics)
    );
    assert_eq!(package_imports(&output.program).count(), 2);
    assert!(package_imports(&output.program).all(|import| import.target.is_some()));
}

#[test]
fn duplicate_imports_conflict_even_when_their_targets_are_identical() {
    let tree = TestTree::new("duplicate-import");
    let package = tree.package(
        "library",
        "lib",
        &[],
        &[(
            "src/lib.elx",
            "pub struct Value:\n\
                 \x20\x20\x20\x20pass\n\
             use self.Value as Alias\n\
             use self.Value as Alias\n",
        )],
    );
    let (sources, output) = resolve_package(&package);
    assert!(
        output
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.category == Category::DeclarationConflict
                    && !diagnostic.related.is_empty()
            ),
        "{}",
        diagnostic_text(&sources, &output.diagnostics)
    );
    let imports = package_imports(&output.program).collect::<Vec<_>>();
    assert_eq!(imports[0].target, imports[1].target);
}

#[test]
fn resolves_parameters_locals_patterns_loops_and_record_shorthand() {
    let tree = TestTree::new("lexical-scopes");
    let package = tree.package(
        "app",
        "exe",
        &[],
        &[(
            "src/main.elx",
            "struct Point:\n\
                 \x20\x20\x20\x20x: i32\n\
             fn scopes(point: Point, values: [i32]) -> i32:\n\
                 \x20\x20\x20\x20let x = 1\n\
                 \x20\x20\x20\x20let built = Point{x}\n\
                 \x20\x20\x20\x20match point:\n\
                 \x20\x20\x20\x20\x20\x20\x20\x20Point { x } if x > 0:\n\
                 \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20return x\n\
                 \x20\x20\x20\x20\x20\x20\x20\x20_:\n\
                 \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20for item in values:\n\
                 \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20let Point = item\n\
                 \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20println(Point)\n\
                 \x20\x20\x20\x20return x\n",
        )],
    );
    let (sources, output) = resolve_package(&package);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        diagnostic_text(&sources, &output.diagnostics)
    );
    assert!(output.program.local_bindings.len() >= 7);
    assert!(
        output
            .program
            .references
            .iter()
            .filter(|reference| matches!(reference.target, NameTarget::Local(_)))
            .count()
            >= 8
    );
}

#[test]
fn tuple_binding_names_enter_scope_together_after_the_initializer() {
    let tree = TestTree::new("tuple-binding-scope");
    let package = tree.package(
        "app",
        "exe",
        &[],
        &[(
            "src/main.elx",
            "fn main() -> ():\n\
             \x20\x20\x20\x20let source = (1, 2)\n\
             \x20\x20\x20\x20let (left, right) = source\n\
             \x20\x20\x20\x20println(left + right)\n",
        )],
    );
    let (sources, output) = resolve_package(&package);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        diagnostic_text(&sources, &output.diagnostics)
    );
    assert_eq!(
        package_local_bindings(&output.program, &sources)
            .filter(|binding| binding.kind == LocalBindingKind::Local)
            .count(),
        3
    );

    let invalid = tree.package(
        "invalid",
        "exe",
        &[],
        &[(
            "src/main.elx",
            "fn main() -> ():\n\
             \x20\x20\x20\x20let (left, right) = (left, 2)\n\
             \x20\x20\x20\x20println(right)\n",
        )],
    );
    let (sources, output) = resolve_package(&invalid);
    let text = diagnostic_text(&sources, &output.diagnostics);
    assert!(text.contains("cannot resolve `left`"), "{text}");

    let duplicate = tree.package(
        "duplicate",
        "exe",
        &[],
        &[(
            "src/main.elx",
            "fn main() -> ():\n\
             \x20\x20\x20\x20let (same, same) = (1, 2)\n\
             \x20\x20\x20\x20println(same)\n",
        )],
    );
    let (sources, output) = resolve_package(&duplicate);
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.category == Category::DeclarationConflict),
        "{}",
        diagnostic_text(&sources, &output.diagnostics)
    );
}

#[test]
fn lookup_does_not_search_other_modules_or_inherit_imports() {
    let tree = TestTree::new("lexical-boundaries");
    let package = tree.package(
        "app",
        "exe",
        &[],
        &[
            (
                "src/main.elx",
                "use root.other.Hidden\n\
                 mod nested:\n\
                 \x20\x20\x20\x20fn bad(value: Hidden):\n\
                 \x20\x20\x20\x20\x20\x20\x20\x20pass\n",
            ),
            ("src/other.elx", "pub struct Hidden:\n    pass\n"),
        ],
    );
    let (sources, output) = resolve_package(&package);
    let rendered = diagnostic_text(&sources, &output.diagnostics);
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.category == Category::NameResolution
                && diagnostic.message.contains("Hidden")),
        "{rendered}"
    );
}

#[test]
fn closure_resolution_requires_explicit_unique_capture_bindings() {
    let tree = TestTree::new("closure-captures");
    let package = tree.package(
        "app",
        "exe",
        &[],
        &[(
            "src/main.elx",
            r#"
fn main() -> ():
    let outer: i32 = 1
    let missing = fn():
        println(outer)
    let duplicate = fn[outer as first, outer as second]():
        println(first)
        println(second)
    let collision = fn[outer as value](value: i32):
        println(value)
"#,
        )],
    );
    let (sources, output) = resolve_package(&package);
    let rendered = diagnostic_text(&sources, &output.diagnostics);
    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.category == Category::NameResolution
                && diagnostic.message.contains("cannot resolve `outer`")
        }),
        "{rendered}"
    );
    assert!(
        output.diagnostics.iter().any(|diagnostic| {
            diagnostic.category == Category::DeclarationConflict
                && diagnostic.message.contains("captured only once")
        }),
        "{rendered}"
    );
    assert!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.category == Category::DeclarationConflict)
            .count()
            >= 2,
        "{rendered}"
    );
}

#[test]
fn public_reexport_chain_preserves_the_original_identity() {
    let tree = TestTree::new("reexport");
    tree.package(
        "dep",
        "lib",
        &[],
        &[
            ("src/lib.elx", "pub use root.hidden.Visible as Exported\n"),
            (
                "src/hidden.elx",
                "pub struct Visible:\n    pass\nstruct Secret:\n    pass\n",
            ),
        ],
    );
    let app = tree.package(
        "app",
        "exe",
        &[("dep", "../dep")],
        &[(
            "src/main.elx",
            "use dep.Exported\nfn consume(value: Exported):\n    pass\n",
        )],
    );
    let (sources, output) = resolve_package(&app);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        diagnostic_text(&sources, &output.diagnostics)
    );
    let imports = package_imports(&output.program).collect::<Vec<_>>();
    assert_eq!(imports.len(), 2);
    assert_eq!(imports[0].target, imports[1].target);
    assert!(
        imports
            .iter()
            .any(|import| import.visibility == Visibility::Public)
    );
    assert!(
        output
            .program
            .references
            .iter()
            .any(|reference| !reference.provenance.is_empty())
    );
}

#[test]
fn public_api_checks_only_externally_visible_members() {
    let tree = TestTree::new("public-api");
    let package = tree.package(
        "library",
        "lib",
        &[],
        &[(
            "src/lib.elx",
            "struct Hidden:\n\
                 \x20\x20\x20\x20pass\n\
             pub struct Allowed:\n\
                 \x20\x20\x20\x20private_value: Hidden\n\
             pub struct Rejected:\n\
                 \x20\x20\x20\x20pub exposed: Hidden\n\
             pub enum AlsoRejected:\n\
                 \x20\x20\x20\x20Value(Hidden)\n",
        )],
    );
    let (sources, output) = resolve_package(&package);
    let rendered = diagnostic_text(&sources, &output.diagnostics);
    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.category == Category::Visibility)
            .count(),
        2,
        "{rendered}"
    );
}

#[test]
fn retains_member_namespaces_and_reports_member_conflicts() {
    let tree = TestTree::new("members");
    let package = tree.package(
        "app",
        "exe",
        &[],
        &[(
            "src/main.elx",
            "struct Broken:\n\
                 \x20\x20\x20\x20value: i32\n\
                 \x20\x20\x20\x20value: i32\n\
                 \x20\x20\x20\x20late: i32\n\
             impl Broken:\n\
                 \x20\x20\x20\x20fn method():\n\
                 \x20\x20\x20\x20\x20\x20\x20\x20pass\n\
                 \x20\x20\x20\x20fn method():\n\
                 \x20\x20\x20\x20\x20\x20\x20\x20pass\n\
             enum Choice:\n\
                 \x20\x20\x20\x20One(i32)\n\
                 \x20\x20\x20\x20Two { value: i32 }\n",
        )],
    );
    let (sources, output) = resolve_package(&package);
    let rendered = diagnostic_text(&sources, &output.diagnostics);
    assert!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.category == Category::DeclarationConflict)
            .count()
            >= 2,
        "{rendered}"
    );
    assert!(
        output
            .program
            .declaration_members
            .values()
            .any(|members| members.len() >= 2)
    );
    assert_eq!(package_variants(&output.program).count(), 2);
    assert_eq!(
        package_variants(&output.program)
            .map(|variant| variant.fields.len())
            .sum::<usize>(),
        2
    );
}

#[test]
fn self_type_is_scoped_to_structs_traits_and_trait_impls() {
    let tree = TestTree::new("self-type");
    let package = tree.package(
        "app",
        "exe",
        &[],
        &[(
            "src/main.elx",
            "struct Valid:\n\
                 \x20\x20\x20\x20pass\n\
             impl Valid:\n\
                 \x20\x20\x20\x20fn copy(self: &Self) -> Self:\n\
                 \x20\x20\x20\x20\x20\x20\x20\x20return *self\n\
             trait Make:\n\
                 \x20\x20\x20\x20fn make() -> Self\n\
             impl Make for Valid:\n\
                 \x20\x20\x20\x20fn make() -> Self:\n\
                 \x20\x20\x20\x20\x20\x20\x20\x20return Valid{}\n\
             enum Invalid:\n\
                 \x20\x20\x20\x20Value(Self)\n\
             fn invalid() -> Self:\n\
                 \x20\x20\x20\x20pass\n",
        )],
    );
    let (sources, output) = resolve_package(&package);
    let rendered = diagnostic_text(&sources, &output.diagnostics);
    assert_eq!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic.category == Category::NameResolution
                    && diagnostic.message.contains("`Self`")
            })
            .count(),
        2,
        "{rendered}"
    );
}

#[test]
fn a_file_backed_module_can_be_reexported_under_its_existing_name() {
    let tree = TestTree::new("module-reexport");
    tree.package(
        "dep",
        "lib",
        &[],
        &[
            ("src/lib.elx", "pub use root.hidden as hidden\n"),
            (
                "src/hidden.elx",
                "pub struct Exposed:\n    pass\nstruct Private:\n    pass\n",
            ),
        ],
    );
    let app = tree.package(
        "app",
        "exe",
        &[("dep", "../dep")],
        &[(
            "src/main.elx",
            "use dep.hidden.Exposed\nfn consume(value: Exposed):\n    pass\n",
        )],
    );
    let (sources, output) = resolve_package(&app);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        diagnostic_text(&sources, &output.diagnostics)
    );

    let bad_app = tree.package(
        "bad_app",
        "exe",
        &[("dep", "../dep")],
        &[("src/main.elx", "use dep.hidden.Private\n")],
    );
    let (bad_sources, bad_output) = resolve_package(&bad_app);
    assert!(
        bad_output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.category == Category::Visibility),
        "{}",
        diagnostic_text(&bad_sources, &bad_output.diagnostics)
    );
}

#[test]
fn reports_milestone_four_visibility_and_namespace_failures() {
    let tree = TestTree::new("failures");
    tree.package(
        "dep",
        "lib",
        &[],
        &[
            ("src/lib.elx", ""),
            (
                "src/hidden.elx",
                "pub struct PublicButHidden:\n    pass\nstruct Private:\n    pass\n",
            ),
        ],
    );
    let app = tree.package(
        "app",
        "exe",
        &[("dep", "../dep")],
        &[
            (
                "src/main.elx",
                "mod collision:\n\
                     \x20\x20\x20\x20pass\n\
                 struct Duplicate:\n\
                     \x20\x20\x20\x20pass\n\
                 struct Duplicate:\n\
                     \x20\x20\x20\x20pass\n\
                 use dep.hidden.PublicButHidden\n\
                 use super.nope\n\
                 pub fn expose(value: root.private_api.Hidden):\n\
                     \x20\x20\x20\x20pass\n",
            ),
            ("src/collision.elx", ""),
            ("src/private_api.elx", "pub struct Hidden:\n    pass\n"),
        ],
    );
    let (sources, output) = resolve_package(&app);
    let rendered = diagnostic_text(&sources, &output.diagnostics);
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.category == Category::DeclarationConflict),
        "{rendered}"
    );
    assert!(
        output
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.category == Category::NameResolution),
        "{rendered}"
    );
    assert!(
        output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.category == Category::Visibility)
            .count()
            >= 2,
        "{rendered}"
    );
    assert!(
        output
            .diagnostics
            .iter()
            .filter_map(|diagnostic| diagnostic.primary)
            .all(|span| span.start <= span.end)
    );
}

#[test]
fn std_resolves_as_an_ordinary_name_and_can_be_shadowed() {
    // `std` is not a keyword: it names the standard-library package through
    // ordinary lookup, so a module declaring its own `std` shadows it.
    let tree = TestTree::new("std-name");
    let package = tree.package(
        "demo",
        "exe",
        &[],
        &[(
            "src/main.elx",
            "use std.io\n\nfn main() -> ():\n    io.println(\"ok\")\n",
        )],
    );
    let (sources, output) = resolve_package(&package);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        diagnostic_text(&sources, &output.diagnostics)
    );

    // `std` is also usable as an ordinary identifier, which a keyword could
    // never be.
    let shadow = tree.package(
        "shadow",
        "exe",
        &[],
        &[(
            "src/main.elx",
            "fn main() -> ():\n    let std = 1\n    println(f\"{std}\")\n",
        )],
    );
    let (sources, output) = resolve_package(&shadow);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        diagnostic_text(&sources, &output.diagnostics)
    );
}

#[test]
fn a_deferred_block_binding_is_local_to_that_block() {
    // SPEC 8: a `defer:` body is an ordinary lexical scope.
    let tree = TestTree::new("defer-scope");
    let package = tree.package(
        "demo",
        "exe",
        &[],
        &[(
            "src/main.elx",
            "fn log(value: i32) -> ():\n\
             \x20\x20\x20\x20pass\n\
             fn main() -> ():\n\
             \x20\x20\x20\x20defer:\n\
             \x20\x20\x20\x20\x20\x20\x20\x20let inner = 1\n\
             \x20\x20\x20\x20\x20\x20\x20\x20log(inner)\n\
             \x20\x20\x20\x20log(inner)\n",
        )],
    );
    let (sources, output) = resolve_package(&package);
    let text = diagnostic_text(&sources, &output.diagnostics);
    assert!(text.contains("cannot resolve `inner`"), "{text}");
}

#[test]
fn option_is_a_standard_declaration_that_a_user_type_can_shadow() {
    // Milestone 14.1: the standard `Option[T]` is collected from
    // compiler-supplied source into the `std` root module, so it carries real
    // declaration, variant, and generic-parameter identities.
    let tree = TestTree::new("standard-option");
    let package = tree.package(
        "app",
        "exe",
        &[],
        &[(
            "src/main.elx",
            "fn main() -> ():\n\
                 \x20\x20\x20\x20let value: Option[i32] = Option.None\n\
                 \x20\x20\x20\x20pass\n",
        )],
    );
    let (sources, output) = resolve_package(&package);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        diagnostic_text(&sources, &output.diagnostics)
    );
    let option = output
        .program
        .standard_declaration("Option")
        .expect("the standard `Option` declaration exists");
    let declaration = &output.program.declarations[option.index()];
    assert_eq!(declaration.kind, elamite::resolution::DeclarationKind::Enum);
    assert_eq!(declaration.generic_parameters.len(), 1);
    assert!(
        output.program.modules[declaration.module.index()]
            .package
            .is_none(),
        "standard declarations live outside every package instance"
    );
    let variants = output
        .program
        .variants
        .iter()
        .filter(|variant| variant.parent == option)
        .map(|variant| output.program.symbol_text(variant.name))
        .collect::<Vec<_>>();
    // `docs/spec.md` 4.4 declares `Some` before `None`; the order is observable
    // through derived comparison, so it is part of the declaration.
    assert_eq!(variants, vec!["Some", "None"]);
    assert!(output.program.standard_variant("Option", "None").is_some());

    // A user declaration shadows the prelude name and is a different identity.
    let shadowing = tree.package(
        "shadow",
        "exe",
        &[],
        &[(
            "src/main.elx",
            "enum Option[T]:\n\
                 \x20\x20\x20\x20Some(T)\n\
                 \x20\x20\x20\x20None\n\
             fn main() -> ():\n\
                 \x20\x20\x20\x20let value: Option[i32] = Option.None\n\
                 \x20\x20\x20\x20pass\n",
        )],
    );
    let (_, shadowed) = resolve_package(&shadowing);
    let user = shadowed
        .program
        .declarations
        .iter()
        .find(|declaration| {
            shadowed.program.modules[declaration.module.index()]
                .package
                .is_some()
                && shadowed.program.symbol_text(declaration.name) == "Option"
        })
        .expect("the user declaration is collected");
    assert!(!shadowed.program.is_standard_declaration(user.id, "Option"));
}

#[test]
fn standard_intrinsic_inventory_is_exact_and_unique() {
    let tree = TestTree::new("intrinsic-inventory");
    let package = tree.package(
        "app",
        "exe",
        &[],
        &[("src/main.elx", "fn main() -> ():\n    pass\n")],
    );
    let (sources, output) = resolve_package(&package);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        diagnostic_text(&sources, &output.diagnostics)
    );

    let actual = output.program.builtin_names();
    let expected = elamite::standard::intrinsic_leaf_names();
    assert_eq!(actual, expected);

    let unique = actual
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        unique.len(),
        actual.len(),
        "a compiler-known spelling was registered more than once"
    );

    let source_declarations = elamite::standard::SOURCE_DECLARATIONS
        .iter()
        .map(|path| {
            path.rsplit_once('.')
                .expect("source declaration path is qualified")
                .1
        })
        .collect::<std::collections::BTreeSet<_>>();
    for name in source_declarations {
        assert!(
            !unique.contains(name),
            "source-backed `{name}` must not also have a builtin identity"
        );
    }
}

#[test]
fn prelude_surface_is_exact_and_standard_modules_are_source_backed() {
    let tree = TestTree::new("prelude-surface");
    let package = tree.package(
        "app",
        "exe",
        &[],
        &[("src/main.elx", "fn main() -> ():\n    pass\n")],
    );
    let (sources, output) = resolve_package(&package);
    assert!(
        output.diagnostics.is_empty(),
        "{}",
        diagnostic_text(&sources, &output.diagnostics)
    );

    assert_eq!(
        output.program.prelude_names(),
        vec![
            "Callable",
            "Clone",
            "Copy",
            "Default",
            "Display",
            "Drop",
            "Eq",
            "Formatter",
            "Hash",
            "Identity",
            "Iterator",
            "Map",
            "NumericError",
            "Option",
            "Ord",
            "PartialEq",
            "PartialOrd",
            "Result",
            "Send",
            "Set",
            "StableHash",
            "String",
            "Sync",
            "Vec",
            "bool",
            "char",
            "f32",
            "f64",
            "i128",
            "i16",
            "i32",
            "i64",
            "i8",
            "isize",
            "panic",
            "print",
            "println",
            "str",
            "trap",
            "u128",
            "u16",
            "u32",
            "u64",
            "u8",
            "usize",
        ]
    );

    let standard_modules = output
        .program
        .modules
        .iter()
        .filter(|module| module.origin == elamite::resolution::ModuleOrigin::Standard)
        .collect::<Vec<_>>();
    assert_eq!(standard_modules.len(), 13);
    for module in standard_modules {
        assert!(
            module.source_file.is_some(),
            "standard module {:?} is not backed by shipped source",
            module.path
        );
    }
}

#[test]
fn native_declaration_inventory_matches_shipped_sources_exactly() {
    let modules = [
        ("std", elamite::standard::ROOT_SOURCE),
        ("std.io", elamite::standard::IO_SOURCE),
        ("std.fs", elamite::standard::FS_SOURCE),
        ("std.env", elamite::standard::ENV_SOURCE),
        ("std.process", elamite::standard::PROCESS_SOURCE),
        ("std.time", elamite::standard::TIME_SOURCE),
        ("std.random", elamite::standard::RANDOM_SOURCE),
        ("std.ordering", elamite::standard::ORDERING_SOURCE),
        ("std.text", elamite::standard::TEXT_SOURCE),
        ("std.ffi", elamite::standard::FFI_SOURCE),
        ("std.testing", elamite::standard::TESTING_SOURCE),
        ("std.thread", elamite::standard::THREAD_SOURCE),
        ("std.sync", elamite::standard::SYNC_SOURCE),
    ];
    let mut actual = Vec::new();
    for (module, source) in modules {
        let mut intrinsic = false;
        for line in source.lines().map(str::trim) {
            if line == "@intrinsic()" {
                assert!(!intrinsic, "nested intrinsic marker in {module}");
                intrinsic = true;
                continue;
            }
            if intrinsic
                && let Some(rest) = line
                    .strip_prefix("fn ")
                    .or_else(|| line.strip_prefix("pub fn "))
            {
                let name = rest.split_once(['[', '(']).map_or(rest, |(name, _)| name);
                actual.push(format!("{module}.{name}"));
                intrinsic = false;
            }
        }
        assert!(!intrinsic, "orphan intrinsic marker in {module}");
    }
    actual.sort();
    let mut expected = elamite::standard::NATIVE_DECLARATIONS
        .iter()
        .map(|intrinsic| intrinsic.path.to_string())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(actual, expected);
    assert!(
        elamite::standard::NATIVE_DECLARATIONS
            .iter()
            .all(|intrinsic| !intrinsic.reason.trim().is_empty())
    );
}

#[test]
fn user_packages_cannot_declare_compiler_intrinsics() {
    let tree = TestTree::new("reserved-intrinsic");
    let package = tree.package(
        "app",
        "exe",
        &[],
        &[(
            "src/main.elx",
            "@intrinsic()\nfn steal() -> i32\n\nfn main() -> ():\n    pass\n",
        )],
    );
    let (sources, output) = resolve_package(&package);
    let text = diagnostic_text(&sources, &output.diagnostics);
    assert!(
        text.contains("`@intrinsic` is reserved for exact compiler-owned standard declarations"),
        "{text}"
    );
}
