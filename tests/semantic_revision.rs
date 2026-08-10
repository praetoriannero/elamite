use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::config::{SemanticRevision, Target};
use elamite::diagnostics::Category;
use elamite::driver::check_frontend;
use elamite::expansion::ast::{
    ClosureCaptureMode, ExpressionVariant, OWNED_INTERFACE_VERSION, StatementList, TypeSyntaxList,
    TypeSyntaxVariant,
};
use elamite::expansion::expand;
use elamite::formatter::{FormatOptions, format_source_for_revision};
use elamite::lexer::lex;
use elamite::package::PackageGraph;
use elamite::parsed::parse_package;
use elamite::parser::parse_for_revision;
use elamite::resolution::{resolve, resolve_expanded};
use elamite::source::SourceManager;
use elamite::types::{Mutability, TypeKind, resolve_types};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestPackage {
    root: PathBuf,
}

impl TestPackage {
    fn new(name: &str, source: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "elamite-semantic-revision-{}-{name}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("src")).expect("create package source directory");
        fs::write(
            root.join("elamite.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\ntarget_kind = \"lib\"\n"),
        )
        .expect("write package manifest");
        fs::write(root.join("src/lib.elx"), source).expect("write package source");
        Self { root }
    }

    fn graph(&self, sources: &mut SourceManager, revision: SemanticRevision) -> PackageGraph {
        PackageGraph::resolve_with_revision(&self.root.join("elamite.toml"), sources, revision)
            .expect("package graph resolves")
    }
}

impl Drop for TestPackage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/packages")
        .join(name)
}

#[test]
fn owned_surface_parses_closures_slices_arrays_and_borrows_without_legacy_leakage() {
    let source = r#"fn demo(
    shared: [Vec[i32]],
    mutable: [var Vec[i32]],
    fixed: [Vec[i32]; 4],
    slot: &var i32,
) -> ():
    let callback = fn[fixed, &shared as view, &var slot](value: i32) -> i32:
        return value + *slot
    callback(1)
"#;
    let mut sources = SourceManager::new();
    let file = sources.add_text(PathBuf::from("owned.elx"), source.to_string());
    let lexed = lex(file, source);
    assert!(lexed.diagnostics.is_empty(), "{:?}", lexed.diagnostics);

    let owned = parse_for_revision(&lexed.tokens, SemanticRevision::V0_11);
    assert!(owned.diagnostics.is_empty(), "{:?}", owned.diagnostics);
    let dump = owned.tree.dump();
    assert!(dump.contains("Keyword(Var)"));
    assert!(dump.contains("ClosureCaptureList"));
    assert_eq!(
        owned
            .tree
            .count(elamite::syntax::SyntaxKind::ClosureCapture),
        3
    );

    let legacy = parse_for_revision(&lexed.tokens, SemanticRevision::V0_10);
    assert!(
        legacy
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("expected a type")),
        "legacy parsing must not reinterpret `[var T]`"
    );

    for malformed in [
        "fn bad(values: [var i32; 4]) -> ():\n    pass\n",
        "fn bad() -> ():\n    let callback = fn(value: i32) -> i32:\n        return value\n",
    ] {
        let file = sources.add_text(PathBuf::from("malformed.elx"), malformed.to_string());
        let lexed = lex(file, malformed);
        let output = parse_for_revision(&lexed.tokens, SemanticRevision::V0_11);
        assert!(
            output
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("owned-model")
                    || diagnostic.message.contains("owning array")),
            "{:#?}",
            output.diagnostics
        );
    }
}

#[test]
fn authoritative_owned_demo_reaches_the_revision_boundary_cleanly() {
    let source = include_str!("../owned_spec_demo.elx");
    let mut sources = SourceManager::new();
    let file = sources.add_text(PathBuf::from("owned_spec_demo.elx"), source.to_string());
    let lexed = lex(file, source);
    assert!(lexed.diagnostics.is_empty(), "{:?}", lexed.diagnostics);
    let parsed = parse_for_revision(&lexed.tokens, SemanticRevision::V0_11);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    format_source_for_revision(
        file,
        source,
        FormatOptions::default(),
        SemanticRevision::V0_11,
    )
    .expect("the authoritative owned demo is accepted by syntax tooling");
}

#[test]
fn source_type_lowering_distinguishes_shared_mutable_and_owning_sequences() {
    let package = TestPackage::new(
        "owned_types",
        "pub fn inspect(shared: [i32], mutable: [var i32], fixed: [i32; 4]) -> ():\n    pass\n",
    );
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let resolution = resolve(&graph, &mut sources);
    assert!(
        resolution.diagnostics.is_empty(),
        "{:?}",
        resolution.diagnostics
    );
    assert_eq!(
        resolution.program.semantic_revision,
        SemanticRevision::V0_11
    );

    let output = resolve_types(&resolution.program);
    assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
    assert_eq!(output.program.semantic_revision, SemanticRevision::V0_11);
    let declaration = resolution
        .program
        .declarations
        .iter()
        .find(|declaration| resolution.program.symbol_text(declaration.name) == "inspect")
        .expect("inspect declaration");
    let signature = &output.program.function_signatures[&declaration.id];
    assert!(matches!(
        output.program.types.kind(signature.parameters[0].ty),
        TypeKind::Slice {
            mutability: Mutability::Shared,
            ..
        }
    ));
    assert!(matches!(
        output.program.types.kind(signature.parameters[1].ty),
        TypeKind::Slice {
            mutability: Mutability::Mutable,
            ..
        }
    ));
    assert!(matches!(
        output.program.types.kind(signature.parameters[2].ty),
        TypeKind::Array { length: 4, .. }
    ));
}

#[test]
fn owned_ast_interface_and_formatter_preserve_the_new_forms() {
    let source = "fn demo(values: [var i32]) -> ():\n    let callback = fn[]() -> ():\n        pass\n    callback()\n";
    let package = TestPackage::new("owned_ast", source);
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let parsed = parse_package(&graph, &mut sources);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let expanded = expand(&graph, parsed);
    assert!(
        expanded.diagnostics.is_empty(),
        "{:?}",
        expanded.diagnostics
    );
    assert_eq!(expanded.package.semantic_revision, SemanticRevision::V0_11);
    assert_eq!(expanded.package.ast.version(), OWNED_INTERFACE_VERSION);

    let unit = expanded
        .package
        .units
        .iter()
        .find(|unit| !unit.is_standard())
        .expect("user unit");
    let token = unit.token_trees.flattened()[0].clone();
    let builder = expanded.package.ast_builder_for_token(&token);
    let i32_name = builder.identifier("i32").expect("identifier");
    let i32_path = builder.path(vec![i32_name]).expect("path");
    let i32_type = builder.named_type(i32_path, TypeSyntaxList::empty());
    let mutable_slice = builder
        .slice_type(true, i32_type.clone())
        .expect("owned slice AST");
    assert!(matches!(
        mutable_slice.variant(),
        TypeSyntaxVariant::Slice { mutable: true, .. }
    ));

    let value_name = builder.identifier("value").expect("identifier");
    let value = builder.identifier_expression(value_name.clone());
    let borrow = builder
        .borrow_expression(true, value)
        .expect("owned borrow AST");
    assert!(matches!(
        borrow.variant(),
        ExpressionVariant::Borrow { mutable: true, .. }
    ));
    let capture = builder
        .closure_capture(ClosureCaptureMode::MutableBorrow, value_name, None)
        .expect("owned capture AST");
    let closure = builder
        .closure_expression(
            vec![capture],
            elamite::expansion::ast::ParameterList::empty(),
            builder.tuple_type(TypeSyntaxList::empty()),
            StatementList::empty(),
        )
        .expect("owned closure AST");
    assert!(matches!(closure.variant(), ExpressionVariant::Closure(_)));

    let file = sources.add_text(PathBuf::from("format.elx"), source.to_string());
    let formatted = format_source_for_revision(
        file,
        source,
        FormatOptions::default(),
        SemanticRevision::V0_11,
    )
    .expect("owned source formats");
    assert!(formatted.contains("[var i32]"));
    assert!(formatted.contains("fn[]()"));
}

#[test]
fn generated_and_handwritten_owned_types_use_the_same_parser_surface() {
    let package = TestPackage::new(
        "owned_generated_type",
        r#"macro mutable_slice() -> std.ast.TypeSyntax:
    return quote:
        [var i32]

type Handwritten = [var i32]
type Generated = @mutable_slice()
"#,
    );
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let parsed = parse_package(&graph, &mut sources);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let expanded = expand(&graph, parsed);
    assert!(
        expanded.diagnostics.is_empty(),
        "{:?}",
        expanded.diagnostics
    );
    assert_eq!(expanded.package.schedule.usage().executions, 1);
    let first_dump = expanded.package.dump();

    let mut repeated_sources = SourceManager::new();
    let repeated_graph = package.graph(&mut repeated_sources, SemanticRevision::V0_11);
    let repeated_parsed = parse_package(&repeated_graph, &mut repeated_sources);
    let repeated = expand(&repeated_graph, repeated_parsed);
    assert!(
        repeated.diagnostics.is_empty(),
        "{:?}",
        repeated.diagnostics
    );
    assert_eq!(repeated.package.dump(), first_dump);

    let resolved = resolve_expanded(&graph, expanded);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
    let typed = resolve_types(&resolved.program);
    assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
    let alias_targets = ["Handwritten", "Generated"].map(|name| {
        let declaration = resolved
            .program
            .declarations
            .iter()
            .find(|declaration| resolved.program.symbol_text(declaration.name) == name)
            .expect("type alias declaration");
        let alias = typed.program.declaration_types[&declaration.id];
        let TypeKind::Alias { target, .. } = typed.program.types.kind(alias) else {
            panic!("expected alias type")
        };
        *target
    });
    assert_eq!(alias_targets[0], alias_targets[1]);
    assert!(matches!(
        typed.program.types.kind(alias_targets[0]),
        TypeKind::Slice {
            mutability: Mutability::Mutable,
            ..
        }
    ));
}

#[test]
fn revision_is_selected_once_for_the_complete_dependency_graph() {
    let mut sources = SourceManager::new();
    let graph = PackageGraph::resolve_with_revision(
        &fixture("transitive/root").join("elamite.toml"),
        &mut sources,
        SemanticRevision::V0_11,
    )
    .expect("fixture graph resolves");
    assert_eq!(graph.semantic_revision(), SemanticRevision::V0_11);
    assert!(
        graph
            .packages
            .values()
            .all(|package| package.semantic_revision() == SemanticRevision::V0_11)
    );
    graph
        .validate_semantic_revisions()
        .expect("resolved graph is revision-compatible");
}

#[test]
fn owned_packages_stop_once_before_legacy_body_semantics() {
    let package = TestPackage::new(
        "owned_boundary",
        "pub fn identity(value: String) -> String:\n    return value\n",
    );
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let diagnostics = match check_frontend(&graph, &mut sources, Target::X86_64) {
        Ok(_) => panic!("owned body semantics are not enabled yet"),
        Err(diagnostics) => diagnostics,
    };
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].category, Category::SemanticRevision);
    assert!(diagnostics[0].message.contains("source-type lowering"));

    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let parsed = parse_package(&graph, &mut sources);
    let expanded = expand(&graph, parsed);
    let resolved = resolve_expanded(&graph, expanded);
    assert!(
        resolved.diagnostics.is_empty(),
        "{:?}",
        resolved.diagnostics
    );
}
