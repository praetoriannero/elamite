use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::backend::{COptions, emit_c};
use elamite::check::{check_borrows, check_for_target, check_moves, infer_provenance_signatures};
use elamite::config::{SemanticRevision, Target};
use elamite::diagnostics::Category;
use elamite::driver::check_frontend;
use elamite::expansion::ast::{
    ClosureCaptureMode, ExpressionVariant, OWNED_INTERFACE_VERSION, StatementList, TypeSyntaxList,
    TypeSyntaxVariant,
};
use elamite::expansion::expand;
use elamite::formatter::{FormatOptions, format_source_for_revision};
use elamite::ir::{lower_control_flow, lower_typed_ir};
use elamite::lexer::lex;
use elamite::operations::{
    CapabilityState, OwnershipPlace, OwnershipPlaceRoot, OwnershipProjection, OwnershipUseKind,
    PlaceOverlap,
};
use elamite::package::PackageGraph;
use elamite::parsed::parse_package;
use elamite::parser::parse_for_revision;
use elamite::resolution::{resolve, resolve_expanded};
use elamite::source::SourceManager;
use elamite::types::{Mutability, PrimitiveType, TypeKind, ownership_facts, resolve_types};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestPackage {
    root: PathBuf,
}

impl TestPackage {
    fn new(name: &str, source: &str) -> Self {
        Self::with_target(name, source, "lib", "lib.elx")
    }

    fn executable(name: &str, source: &str) -> Self {
        Self::with_target(name, source, "exe", "main.elx")
    }

    fn with_target(name: &str, source: &str, target_kind: &str, source_name: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "elamite-semantic-revision-{}-{name}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("src")).expect("create package source directory");
        fs::write(
            root.join("elamite.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\ntarget_kind = \
                 \"{target_kind}\"\n"
            ),
        )
        .expect("write package manifest");
        fs::write(root.join("src").join(source_name), source).expect("write package source");
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

fn owned_move_diagnostics(name: &str, source: &str) -> Vec<elamite::diagnostics::Diagnostic> {
    owned_move_diagnostics_for_bits(name, source, 64)
}

fn owned_move_diagnostics_for_bits(
    name: &str,
    source: &str,
    pointer_bits: u8,
) -> Vec<elamite::diagnostics::Diagnostic> {
    let package = TestPackage::new(name, source);
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let resolution = resolve(&graph, &mut sources);
    assert!(
        resolution.diagnostics.is_empty(),
        "resolution failed: {:#?}",
        resolution.diagnostics
    );
    let resolved = resolution.program;
    let mut types = resolve_types(&resolved);
    assert!(
        types.diagnostics.is_empty(),
        "type lowering failed: {:#?}",
        types.diagnostics
    );
    let traits = elamite::traits::check_traits(&resolved, &mut types.program);
    assert!(
        traits.diagnostics.is_empty(),
        "trait checking failed: {:#?}",
        traits.diagnostics
    );
    let checked = check_for_target(&resolved, &mut types.program, pointer_bits);
    assert!(
        checked.diagnostics.is_empty(),
        "body checking failed: {:#?}",
        checked.diagnostics
    );
    let lowered = lower_typed_ir(&resolved, &mut types.program, &checked.program);
    assert!(
        lowered.diagnostics.is_empty(),
        "typed lowering failed: {:#?}",
        lowered.diagnostics
    );
    check_moves(&resolved, &mut types.program, &lowered.program)
}

fn owned_borrow_diagnostics(name: &str, source: &str) -> Vec<elamite::diagnostics::Diagnostic> {
    let package = TestPackage::new(name, source);
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let resolution = resolve(&graph, &mut sources);
    assert!(
        resolution.diagnostics.is_empty(),
        "resolution failed: {:#?}",
        resolution.diagnostics
    );
    let resolved = resolution.program;
    let mut types = resolve_types(&resolved);
    assert!(types.diagnostics.is_empty(), "{:#?}", types.diagnostics);
    let traits = elamite::traits::check_traits(&resolved, &mut types.program);
    assert!(traits.diagnostics.is_empty(), "{:#?}", traits.diagnostics);
    let checked = check_for_target(&resolved, &mut types.program, 64);
    assert!(checked.diagnostics.is_empty(), "{:#?}", checked.diagnostics);
    let signature_diagnostics = infer_provenance_signatures(&resolved, &mut types.program);
    if !signature_diagnostics.is_empty() {
        return signature_diagnostics;
    }
    let lowered = lower_typed_ir(&resolved, &mut types.program, &checked.program);
    assert!(lowered.diagnostics.is_empty(), "{:#?}", lowered.diagnostics);
    let moves = check_moves(&resolved, &mut types.program, &lowered.program);
    assert!(moves.is_empty(), "move checking failed: {moves:#?}");
    check_borrows(&resolved, &mut types.program, &lowered.program)
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
fn owned_packages_stop_after_race_safe_concurrency() {
    let package = TestPackage::new(
        "owned_boundary",
        "pub fn identity(value: String) -> String:\n    return value\n",
    );
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let diagnostics = match check_frontend(&graph, &mut sources, Target::X86_64) {
        Ok(_) => panic!("owned-model C interoperability is not enabled yet"),
        Err(diagnostics) => diagnostics,
    };
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(
        diagnostics[0].category,
        Category::SemanticRevision,
        "{diagnostics:#?}"
    );
    assert!(
        diagnostics[0]
            .message
            .contains("owned-model C interoperability")
    );

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

#[test]
fn ownership_facts_places_and_operations_cross_both_ir_levels() {
    let package = TestPackage::new(
        "owned_operations",
        r#"use std.Clone

struct Borrowing:
    value: &i32

struct Recursive:
    next: &Recursive

struct Wrapper[T]:
    value: T

struct Resource:
    value: i32

impl Drop for Resource:
    fn drop(self: &var Self) -> ():
        pass

impl Clone for Resource:
    fn clone(self: &Self) -> Self:
        return Resource { value: self.value }

fn clone_resource(value: Resource) -> Resource:
    return value.clone()

pub fn copy_generic[T: Copy](value: T) -> T:
    return value

pub fn transfer[T: Send + Sync](value: T) -> T:
    return value

fn reborrow_ops(input: &var i32) -> ():
    var slot = input
    let shared = &slot
    let exclusive = &var slot

pub fn ownership_ops(value: String, number: i32) -> String:
    let moved = value
    let copied = number
    let view = &number
    return moved
"#,
    );
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let resolution = resolve(&graph, &mut sources);
    assert!(
        resolution.diagnostics.is_empty(),
        "{:#?}",
        resolution.diagnostics
    );
    let resolved = resolution.program;
    let mut type_output = resolve_types(&resolved);
    assert!(
        type_output.diagnostics.is_empty(),
        "{:#?}",
        type_output.diagnostics
    );
    let trait_output = elamite::traits::check_traits(&resolved, &mut type_output.program);
    assert!(
        trait_output.diagnostics.is_empty(),
        "{:#?}",
        trait_output.diagnostics
    );

    let string = type_output.program.types.primitive(PrimitiveType::String);
    let integer = type_output.program.types.primitive(PrimitiveType::I32);
    let string_facts = ownership_facts(&resolved, &mut type_output.program, string);
    assert_eq!(string_facts.copy, CapabilityState::Absent);
    assert_eq!(string_facts.clone, CapabilityState::Present);
    assert_eq!(string_facts.needs_drop, CapabilityState::Present);
    let integer_facts = ownership_facts(&resolved, &mut type_output.program, integer);
    assert_eq!(integer_facts.copy, CapabilityState::Present);
    assert_eq!(integer_facts.needs_drop, CapabilityState::Absent);
    let named_types = resolved
        .declarations
        .iter()
        .filter_map(|declaration| {
            type_output
                .program
                .declaration_types
                .get(&declaration.id)
                .map(|ty| (resolved.symbol_text(declaration.name).to_string(), *ty))
        })
        .collect::<Vec<_>>();
    let declaration_type = |name: &str| {
        named_types
            .iter()
            .find_map(|(candidate, ty)| (candidate == name).then_some(*ty))
            .unwrap_or_else(|| panic!("missing canonical type for {name}"))
    };
    let borrowing = declaration_type("Borrowing");
    let borrowing_facts = ownership_facts(&resolved, &mut type_output.program, borrowing);
    assert_eq!(borrowing_facts.copy, CapabilityState::Present);
    assert_eq!(borrowing_facts.contains_borrow, CapabilityState::Present);
    let recursive = declaration_type("Recursive");
    assert_eq!(
        ownership_facts(&resolved, &mut type_output.program, recursive).copy,
        CapabilityState::Present,
        "recursive structural queries must terminate deterministically"
    );
    let wrapper = declaration_type("Wrapper");
    assert_eq!(
        ownership_facts(&resolved, &mut type_output.program, wrapper).copy,
        CapabilityState::Conditional
    );
    let resource = declaration_type("Resource");
    let resource_facts = ownership_facts(&resolved, &mut type_output.program, resource);
    assert_eq!(resource_facts.copy, CapabilityState::Absent);
    assert_eq!(resource_facts.clone, CapabilityState::Present);
    assert_eq!(resource_facts.needs_drop, CapabilityState::Present);
    for (name, capability) in [
        ("copy_generic", (true, false, false)),
        ("transfer", (false, true, true)),
    ] {
        let declaration = resolved
            .declarations
            .iter()
            .find(|declaration| resolved.symbol_text(declaration.name) == name)
            .expect("generic function declaration");
        let parameter = type_output.program.function_signatures[&declaration.id].parameters[0].ty;
        let facts = ownership_facts(&resolved, &mut type_output.program, parameter);
        if capability.0 {
            assert_eq!(facts.copy, CapabilityState::Present);
        }
        if capability.1 {
            assert_eq!(facts.send, CapabilityState::Present);
        }
        if capability.2 {
            assert_eq!(facts.sync, CapabilityState::Present);
        }
    }
    let error = type_output.program.types.error();
    assert_eq!(
        ownership_facts(&resolved, &mut type_output.program, error),
        elamite::operations::OwnershipFacts::ERROR,
        "error recovery must not grant ownership capabilities"
    );

    let checked = check_for_target(&resolved, &mut type_output.program, 64);
    assert!(checked.diagnostics.is_empty(), "{:#?}", checked.diagnostics);
    assert_eq!(
        checked.program.expression_types.len(),
        checked.program.ownership_uses.len(),
        "every checked expression must have explicit ownership intent"
    );
    let checked_kinds = checked
        .program
        .ownership_uses
        .values()
        .flatten()
        .map(|operation| operation.kind)
        .collect::<Vec<_>>();
    assert!(checked_kinds.contains(&OwnershipUseKind::Move));
    assert!(checked_kinds.contains(&OwnershipUseKind::Copy));
    assert!(checked_kinds.contains(&OwnershipUseKind::BorrowShared));
    assert!(checked_kinds.contains(&OwnershipUseKind::ReborrowShared));
    assert!(checked_kinds.contains(&OwnershipUseKind::ReborrowExclusive));
    assert!(checked_kinds.contains(&OwnershipUseKind::Clone));
    assert!(!checked_kinds.contains(&OwnershipUseKind::LegacyCopy));

    let typed_ir = lower_typed_ir(&resolved, &mut type_output.program, &checked.program);
    assert!(
        typed_ir.diagnostics.is_empty(),
        "{:#?}",
        typed_ir.diagnostics
    );
    assert_eq!(typed_ir.program.semantic_revision, SemanticRevision::V0_11);
    let typed_function = typed_ir
        .program
        .functions
        .iter()
        .find(|function| function.name == "ownership_ops")
        .expect("ownership_ops typed body");
    assert!(
        typed_function
            .drop_requirements
            .iter()
            .any(|requirement| requirement.operation.kind == OwnershipUseKind::Drop),
        "owned locals retain explicit destruction requirements for cleanup elaboration"
    );
    let control_flow = lower_control_flow(&typed_ir.program, &type_output.program);
    assert_eq!(control_flow.semantic_revision, SemanticRevision::V0_11);
    let control_flow_function = control_flow
        .functions
        .iter()
        .find(|function| function.name == "ownership_ops")
        .expect("ownership_ops control-flow body");
    assert_eq!(
        control_flow_function.drop_requirements,
        typed_function.drop_requirements
    );
    let operations = control_flow_function.ownership_use_inventory();
    let kinds = operations
        .iter()
        .map(|record| record.operation.kind)
        .collect::<Vec<_>>();
    assert!(kinds.contains(&OwnershipUseKind::Move));
    assert!(kinds.contains(&OwnershipUseKind::Copy));
    assert!(kinds.contains(&OwnershipUseKind::BorrowShared));
    assert!(
        operations
            .iter()
            .all(|record| record.operation.legacy_copy.is_none())
    );
    let clone_operations = control_flow
        .functions
        .iter()
        .find(|function| function.name == "clone_resource")
        .expect("clone_resource control-flow body")
        .ownership_use_inventory();
    assert!(
        clone_operations
            .iter()
            .any(|record| record.operation.kind == OwnershipUseKind::Clone)
    );
    let reborrow_operations = control_flow
        .functions
        .iter()
        .find(|function| function.name == "reborrow_ops")
        .expect("reborrow_ops control-flow body")
        .ownership_use_inventory();
    assert!(
        reborrow_operations
            .iter()
            .any(|record| { record.operation.kind == OwnershipUseKind::ReborrowShared })
    );
    assert!(
        reborrow_operations
            .iter()
            .any(|record| { record.operation.kind == OwnershipUseKind::ReborrowExclusive })
    );

    let file = sources.add_text(PathBuf::from("places.elx"), String::new());
    let root = OwnershipPlaceRoot::Expression(elamite::source::Span::new(file, 0, 0));
    let left = OwnershipPlace {
        root,
        projections: vec![OwnershipProjection::TupleField(0)],
    };
    let right = OwnershipPlace {
        root,
        projections: vec![OwnershipProjection::TupleField(1)],
    };
    let dynamic = OwnershipPlace {
        root,
        projections: vec![OwnershipProjection::DynamicIndex],
    };
    assert_eq!(left.overlap(&right), PlaceOverlap::Disjoint);
    assert_eq!(left.overlap(&dynamic), PlaceOverlap::MayOverlap);
}

#[test]
fn move_checking_rejects_second_uses_and_conservative_branch_merges() {
    let diagnostics = owned_move_diagnostics(
        "owned_use_after_move",
        r#"fn consume(value: String) -> ():
    pass

fn direct(value: String) -> ():
    consume(value)
    println(value)

fn conditional(value: String, take: bool) -> ():
    if take:
        consume(value)
    println(value)

fn unreachable(value: String) -> String:
    return value
    println(value)
"#,
    );
    let ownership = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.category == Category::Ownership)
        .collect::<Vec<_>>();
    assert_eq!(ownership.len(), 3, "{diagnostics:#?}");
    assert!(ownership.iter().all(|diagnostic| {
        diagnostic.message.contains("moved") && diagnostic.related.len() == 1
    }));
}

#[test]
fn owned_move_design_fixtures_are_active_at_the_move_layer() {
    let pass = include_str!("fixtures/owned_model_design/pass/ownership.elx");
    let diagnostics = owned_move_diagnostics("owned_move_fixture_pass", pass);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    let fail = include_str!("fixtures/owned_model_design/fail/use_after_move.elx");
    let diagnostics = owned_move_diagnostics("owned_move_fixture_fail", fail);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].category, Category::Ownership);
    assert!(diagnostics[0].message.contains("moved"));
    assert_eq!(diagnostics[0].related.len(), 1);

    let package = TestPackage::new("owned_move_driver_fixture", fail);
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let driver_diagnostics = match check_frontend(&graph, &mut sources, Target::X86_64) {
        Ok(_) => panic!("the invalid move must stop before the provenance boundary"),
        Err(diagnostics) => diagnostics,
    };
    assert_eq!(driver_diagnostics.len(), 1, "{driver_diagnostics:#?}");
    assert_eq!(driver_diagnostics[0].category, Category::Ownership);
}

#[test]
fn move_checking_accepts_copy_reuse_reinitialization_and_disjoint_fields() {
    let diagnostics = owned_move_diagnostics(
        "owned_reinitialization",
        r#"struct Pair:
    left: String
    right: String

fn consume(value: String) -> ():
    pass

fn consume_pair(value: Pair) -> ():
    pass

fn valid(value: String, replacement: String, number: i32) -> ():
    var slot = value
    consume(slot)
    slot = replacement
    consume(slot)
    println(number)
    println(number)

    var pair = Pair {
        left: String.from("left"),
        right: String.from("right"),
    }
    let left = pair.left
    let right_view = &pair.right
    let _ = right_view
    pair.left = left
    consume_pair(pair)
"#,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn move_checking_tracks_loops_and_rejects_indexed_collection_moves() {
    let diagnostics = owned_move_diagnostics(
        "owned_loops_and_indices",
        r#"fn consume(value: String) -> ():
    pass

fn loop_move(value: String, run: bool) -> ():
    while run:
        consume(value)
        break
    println(value)

fn indexed(values: Vec[String]) -> ():
    consume(values[0])

fn static_array(values: [String; 2]) -> ():
    consume(values[0])
    consume(values[1])

fn dynamic_array(values: [String; 2], index: usize) -> ():
    consume(values[index])
"#,
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("moved or partially moved")),
        "{diagnostics:#?}"
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("ownership-taking operation"))
            .count(),
        2,
        "{diagnostics:#?}"
    );
}

#[test]
fn move_checking_tracks_destructuring_and_match_arm_bindings() {
    let diagnostics = owned_move_diagnostics(
        "owned_move_patterns",
        r#"fn consume(value: String) -> ():
    pass

fn consume_option(value: Option[String]) -> ():
    pass

fn destructure(pair: (String, i32)) -> ():
    let (text, number) = pair
    println(pair.1)
    consume(text)
    println(number)

fn match_payload(value: Option[String]) -> ():
    match value:
        Option.Some(text):
            consume(text)
        Option.None:
            pass
    consume_option(value)
"#,
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.category == Category::Ownership)
            .count(),
        2,
        "{diagnostics:#?}"
    );
}

#[test]
fn move_checking_rejects_partial_moves_from_custom_drop_types() {
    let diagnostics = owned_move_diagnostics(
        "owned_drop_partial_move",
        r#"struct Resource:
    name: String
    code: i32

struct Wrapper:
    resource: Resource

impl Drop for Resource:
    fn drop(self: &var Self) -> ():
        pass

fn invalid(value: Resource) -> String:
    return value.name

fn invalid_nested(value: Wrapper) -> String:
    return value.resource.name
"#,
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("custom `Drop` implementation"))
            .count(),
        2,
        "{diagnostics:#?}"
    );
}

#[test]
fn move_checking_is_identical_across_target_widths_and_generic_instances() {
    let source = r#"fn identity[T](value: T) -> T:
    return value

fn valid(value: String) -> ():
    var nested = ((value, 1), 2)
    let moved = nested.0.0
    nested.0.0 = moved
    let whole = nested
    let _ = whole

fn invalid(value: String) -> ():
    let moved = identity(value)
    println(value)
    let _ = moved
"#;
    let x86 = owned_move_diagnostics_for_bits("owned_moves_x86", source, 32);
    let x86_64 = owned_move_diagnostics_for_bits("owned_moves_x86_64", source, 64);
    let signature = |diagnostics: &[elamite::diagnostics::Diagnostic]| {
        diagnostics
            .iter()
            .map(|diagnostic| {
                (
                    diagnostic.category,
                    diagnostic.message.clone(),
                    diagnostic.primary.map(|span| (span.start, span.end)),
                    diagnostic.related.len(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(signature(&x86), signature(&x86_64));
    assert_eq!(x86.len(), 1, "{x86:#?}");
    assert_eq!(x86[0].category, Category::Ownership);
}

#[test]
fn borrow_conflict_design_fixture_is_active() {
    let source = include_str!("fixtures/owned_model_design/fail/borrow_conflict.elx");
    let diagnostics = owned_borrow_diagnostics("owned_borrow_fixture", source);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].category, Category::Borrow);
    assert!(diagnostics[0].message.contains("overlapping live borrow"));
    assert_eq!(diagnostics[0].related.len(), 1);

    let package = TestPackage::new("owned_borrow_driver", source);
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let driver = match check_frontend(&graph, &mut sources, Target::X86_64) {
        Ok(_) => panic!("borrow conflict must stop before the destruction boundary"),
        Err(diagnostics) => diagnostics,
    };
    assert_eq!(driver.len(), 1, "{driver:#?}");
    assert_eq!(driver[0].category, Category::Borrow);
}

#[test]
fn borrow_checking_ends_loans_at_last_use_and_allows_disjoint_places() {
    let diagnostics = owned_borrow_diagnostics(
        "owned_borrow_nll",
        r#"struct Pair:
    left: i32
    right: i32

fn valid() -> ():
    var point = Pair { left: 1, right: 2 }
    let view = &point
    println(view.left)
    let derived = view.right + 1
    point.left = 3
    println(derived)

    let left = &var point.left
    let right = &var point.right
    *left = 4
    *right = 5
"#,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn borrow_checking_rejects_exclusive_aliases_moves_and_aggregate_replacement() {
    let diagnostics = owned_borrow_diagnostics(
        "owned_borrow_conflicts",
        r#"struct Pair:
    name: String
    left: i32
    right: i32

fn consume(value: Pair) -> ():
    pass

fn exclusive_alias() -> ():
    var point = Pair { name: String.from("exclusive"), left: 1, right: 2 }
    let edit = &var point.left
    let alias = &point.left
    println(*edit)
    println(*alias)

fn moved_owner() -> ():
    var point = Pair { name: String.from("moved"), left: 1, right: 2 }
    let view = &point
    consume(point)
    println(view.left)

fn replaced_owner() -> ():
    var point = Pair { name: String.from("replaced"), left: 1, right: 2 }
    let view = &point.left
    point = Pair { name: String.from("new"), left: 3, right: 4 }
    println(*view)

fn relocated_vector() -> ():
    var values = @vec[1, 2]
    let view = &values
    values.append(3)
    println(view.len())
"#,
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.category == Category::Borrow)
            .count(),
        4,
        "{diagnostics:#?}"
    );
}

#[test]
fn borrow_provenance_propagates_structurally_and_rejects_local_escape() {
    let diagnostics = owned_borrow_diagnostics(
        "owned_borrow_structural",
        r#"struct View:
    value: &i32

fn wrap(value: &i32) -> View:
    return View { value: value }

fn identity(value: &i32) -> &i32:
    return value

fn local_escape() -> &i32:
    let local = 1
    return &local

fn retained() -> ():
    var local = 1
    let wrapped = wrap(&local)
    local = 2
    println(*wrapped.value)

fn valid(input: &i32) -> &i32:
    return identity(input)
"#,
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.category == Category::Borrow)
            .count(),
        2,
        "{diagnostics:#?}"
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("local or temporary"))
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("overlapping live borrow"))
    );
}

#[test]
fn borrow_interface_inference_rejects_ambiguous_sources() {
    let diagnostics = owned_borrow_diagnostics(
        "owned_borrow_ambiguous",
        r#"fn choose(left: &i32, right: &i32, first: bool) -> &i32:
    if first:
        return left
    else:
        return right
"#,
    );
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].category, Category::Borrow);
    assert!(diagnostics[0].message.contains("ambiguous"));
}

#[test]
fn borrow_checking_tracks_reborrows_aggregate_fields_and_deferred_uses() {
    let diagnostics = owned_borrow_diagnostics(
        "owned_borrow_reborrows",
        r#"struct Pair:
    left: i32
    right: i32

struct Edits:
    left: &var i32
    right: &var i32

fn disjoint_fields() -> ():
    var pair = Pair { left: 1, right: 2 }
    let edits = Edits { left: &var pair.left, right: &var pair.right }
    *edits.left = 3
    *edits.right = 4

fn shared_reborrow(input: &var i32) -> ():
    var slot = input
    let child = &slot
    *slot = 1
    println(*child)

fn exclusive_reborrow(input: &var i32) -> ():
    var slot = input
    let child = &var slot
    *slot = 1
    println(*child)

fn deferred_use() -> ():
    var value = 1
    let view = &value
    defer println(*view)
    value = 2
"#,
    );
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.category == Category::Borrow)
            .count(),
        3,
        "{diagnostics:#?}"
    );
}

#[test]
fn borrow_interface_prefers_receiver_and_callers_retain_its_provenance() {
    let diagnostics = owned_borrow_diagnostics(
        "owned_borrow_receiver",
        r#"struct View:
    value: &i32

impl View:
    fn select(self: &Self, other: &i32) -> &i32:
        return self.value

fn retained() -> ():
    var source = 1
    let other = 2
    let view = View { value: &source }
    let selected = view.select(&other)
    source = 3
    println(*selected)
"#,
    );
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].category, Category::Borrow);
    assert!(diagnostics[0].message.contains("overlapping live borrow"));
}

#[test]
fn borrow_interfaces_serialize_return_sources_in_package_metadata() {
    let package = TestPackage::new(
        "owned_borrow_metadata",
        r#"pub fn identity(value: &i32) -> &i32:
    return value
"#,
    );
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let resolution = resolve(&graph, &mut sources);
    assert!(
        resolution.diagnostics.is_empty(),
        "{:#?}",
        resolution.diagnostics
    );
    let resolved = resolution.program;
    let mut types = resolve_types(&resolved);
    assert!(types.diagnostics.is_empty(), "{:#?}", types.diagnostics);
    let diagnostics = infer_provenance_signatures(&resolved, &mut types.program);
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");

    let metadata = elamite::artifact::PackageMetadata::collect(
        &graph,
        &resolved,
        &types.program,
        &sources,
        &graph.root,
    );
    let identity = metadata
        .public_api
        .iter()
        .find(|item| item.path == "root.identity")
        .expect("public identity metadata");
    assert_eq!(identity.return_provenance.as_deref(), Some("parameter:0"));
    assert_eq!(metadata.semantic_revision, "0.11.0-draft");

    let path = package.root.join("borrow-metadata.toml");
    metadata.write(&path).expect("serialize borrow metadata");
    let decoded =
        elamite::artifact::PackageMetadata::read(&path).expect("deserialize borrow metadata");
    assert_eq!(decoded, metadata);
}

#[test]
fn owned_reference_formation_accepts_composite_storage_and_rejects_mutable_shared_reborrows() {
    let package = TestPackage::new(
        "owned_borrow_formation",
        r#"struct Value:
    number: i32

fn invalid(input: &i32) -> ():
    var slot = input
    let exclusive = &var slot
    let temporary = &Value { number: 1 }
"#,
    );
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let diagnostics = match check_frontend(&graph, &mut sources, Target::X86_64) {
        Ok(_) => panic!("invalid reference formation must stop during checking"),
        Err(diagnostics) => diagnostics,
    };
    assert_eq!(
        diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.category == Category::Place)
            .count(),
        1,
        "{diagnostics:#?}"
    );
}

#[test]
fn borrow_provenance_flows_through_closure_environments_and_calls() {
    let diagnostics = owned_borrow_diagnostics(
        "owned_borrow_closure",
        r#"fn retained() -> ():
    var source = 1
    let view = fn[&source]() -> &i32:
        return source
    let selected = view()
    source = 2
    println(*selected)
"#,
    );
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].category, Category::Borrow);
    assert!(diagnostics[0].message.contains("overlapping live borrow"));
}

#[test]
fn direct_drop_hook_calls_are_rejected() {
    let package = TestPackage::new(
        "owned_direct_drop",
        r#"struct Resource:
    value: i32

impl Drop for Resource:
    fn drop(self: &var Self) -> ():
        pass

fn invalid(value: Resource) -> ():
    var owned = value
    owned.drop()
"#,
    );
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let diagnostics = match check_frontend(&graph, &mut sources, Target::X86_64) {
        Ok(_) => panic!("a direct custom destruction call must be rejected"),
        Err(diagnostics) => diagnostics,
    };
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0].category, Category::Call);
    assert!(
        diagnostics[0].message.contains("compiler-invoked"),
        "{diagnostics:#?}"
    );
}

#[test]
fn deterministic_cleanup_runs_exactly_once_in_specified_order_in_c99() {
    let package = TestPackage::executable(
        "owned_destruction",
        r#"struct Notice:
    value: i32

struct Pair:
    first: Notice
    second: Notice

impl Drop for Notice:
    fn drop(self: &var Self) -> ():
        println(self.value)

fn main() -> ():
    var slot = Notice { value: 5 }
    let pair = Pair {
        first: Notice { value: 2 },
        second: Notice { value: 3 },
    }
    defer println(4)
    slot = Notice { value: 6 }
    let explicit = Notice { value: 7 }
    drop(explicit)
"#,
    );
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let resolution = resolve(&graph, &mut sources);
    assert!(
        resolution.diagnostics.is_empty(),
        "{:#?}",
        resolution.diagnostics
    );
    let resolved = resolution.program;
    let mut types = resolve_types(&resolved);
    assert!(types.diagnostics.is_empty(), "{:#?}", types.diagnostics);
    let traits = elamite::traits::check_traits(&resolved, &mut types.program);
    assert!(traits.diagnostics.is_empty(), "{:#?}", traits.diagnostics);
    let checked = check_for_target(&resolved, &mut types.program, 64);
    assert!(checked.diagnostics.is_empty(), "{:#?}", checked.diagnostics);
    let provenance = infer_provenance_signatures(&resolved, &mut types.program);
    assert!(provenance.is_empty(), "{provenance:#?}");
    let typed_ir = lower_typed_ir(&resolved, &mut types.program, &checked.program);
    assert!(
        typed_ir.diagnostics.is_empty(),
        "{:#?}",
        typed_ir.diagnostics
    );
    let moves = check_moves(&resolved, &mut types.program, &typed_ir.program);
    assert!(moves.is_empty(), "{moves:#?}");
    let borrows = check_borrows(&resolved, &mut types.program, &typed_ir.program);
    assert!(borrows.is_empty(), "{borrows:#?}");
    let control_flow = lower_control_flow(&typed_ir.program, &types.program);
    let main = control_flow
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("main control-flow body");
    assert_eq!(main.drop_flags.len(), 4);
    assert!(
        main.blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| matches!(
                instruction,
                elamite::ir::Instruction::SetDropFlag {
                    initialized: false,
                    ..
                }
            ))
    );
    assert_eq!(
        main.blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter(|instruction| matches!(instruction, elamite::ir::Instruction::DropValue { .. }))
            .count(),
        5,
        "replacement plus four final cleanup actions must be explicit"
    );

    let entry = resolved
        .declarations
        .iter()
        .find(|declaration| resolved.symbol_text(declaration.name) == "main")
        .map(|declaration| declaration.id)
        .expect("resolved main declaration");
    let output = emit_c(
        &control_flow,
        &resolved,
        &types.program,
        &sources,
        &COptions {
            target: Target::X86_64,
            entry: Some(entry),
            test_entries: None,
        },
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    assert!(output.source.contains("int el_drop_"));
    assert!(output.source.contains("if (el_drop_"));
    assert!(!output.source.contains("_Static_assert"));
    let x86_output = emit_c(
        &control_flow,
        &resolved,
        &types.program,
        &sources,
        &COptions {
            target: Target::X86,
            entry: Some(entry),
            test_entries: None,
        },
    );
    assert!(
        x86_output.diagnostics.is_empty(),
        "{:#?}",
        x86_output.diagnostics
    );
    assert!(x86_output.source.contains("int el_drop_"));
    assert!(!x86_output.source.contains("_Static_assert"));

    let c_path = package.root.join("destruction.c");
    fs::write(&c_path, output.source).expect("write generated C");
    for optimization in ["-O0", "-O2"] {
        let executable = package
            .root
            .join(format!("destruction-{}", &optimization[1..]));
        let compiled = Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()))
            .args([
                "-std=c99",
                "-pedantic-errors",
                "-Wall",
                "-Wextra",
                "-Werror",
                optimization,
            ])
            .arg(&c_path)
            .arg("-o")
            .arg(&executable)
            .output()
            .expect("invoke C99 compiler");
        assert!(
            compiled.status.success(),
            "generated C failed under {optimization}: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let run = Command::new(&executable)
            .output()
            .expect("run generated program");
        assert!(run.status.success(), "program failed under {optimization}");
        assert_eq!(
            String::from_utf8(run.stdout).expect("UTF-8 output"),
            "5\n7\n4\n3\n2\n6\n"
        );
    }
}

#[test]
fn owned_core_values_clone_move_mutate_and_drop_independently() {
    let package = TestPackage::executable(
        "owned_core_values",
        r#"fn main() -> ():
    let original = @vec[String.from("a"), String.from("b")]
    var duplicate = original.clone()
    let moved = original
    duplicate.append(String.from("c"))
    println(moved.len())
    println(duplicate.len())
    duplicate.clear()
    println(duplicate.len())

    var boxed = Box[String].new(String.from("box"))
    println(&*boxed)
    *boxed = String.from("changed")
    println(&*boxed)

    var map = Map[i32, i32].new()
    map.insert(4, 7)
    let map_copy = map.clone()
    println(map_copy.len())
    let map_key = 4
    println(map.contains_key(&map_key))
    match map.get_var(&map_key):
        Option.Some(found):
            *found = 10
        Option.None:
            pass
    match map.get(&map_key):
        Option.Some(found):
            println(*found)
        Option.None:
            pass

    var set = Set[i32].new()
    set.insert(8)
    let set_copy = set.clone()
    println(set_copy.len())
    let member = 8
    println(set.contains(&member))
    println(set.remove(&member))

    var values = @vec[1, 2]
    match values.get_var(0):
        Option.Some(found):
            *found = 6
        Option.None:
            pass
    match values.get(0):
        Option.Some(found):
            println(*found)
        Option.None:
            pass
    match values.pop():
        Option.Some(popped):
            println(popped)
        Option.None:
            pass

    var fixed = [1, 2, 3]
    let view = &fixed
    println(view.len())
    var mutable_view = &var fixed
    mutable_view[1] = 9
    println(fixed[1])
    var sum = 0
    for item in &fixed:
        sum += *item
    println(sum)

    for word in @vec[String.from("first"), String.from("unvisited")]:
        println(&word)
        break
"#,
    );
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let resolution = resolve(&graph, &mut sources);
    assert!(
        resolution.diagnostics.is_empty(),
        "{:#?}",
        resolution.diagnostics
    );
    let resolved = resolution.program;
    let mut types = resolve_types(&resolved);
    assert!(types.diagnostics.is_empty(), "{:#?}", types.diagnostics);
    let traits = elamite::traits::check_traits(&resolved, &mut types.program);
    assert!(traits.diagnostics.is_empty(), "{:#?}", traits.diagnostics);
    let checked = check_for_target(&resolved, &mut types.program, 64);
    assert!(checked.diagnostics.is_empty(), "{:#?}", checked.diagnostics);
    let provenance = infer_provenance_signatures(&resolved, &mut types.program);
    assert!(provenance.is_empty(), "{provenance:#?}");
    let typed_ir = lower_typed_ir(&resolved, &mut types.program, &checked.program);
    assert!(
        typed_ir.diagnostics.is_empty(),
        "{:#?}",
        typed_ir.diagnostics
    );
    let moves = check_moves(&resolved, &mut types.program, &typed_ir.program);
    assert!(moves.is_empty(), "{moves:#?}");
    let borrows = check_borrows(&resolved, &mut types.program, &typed_ir.program);
    assert!(borrows.is_empty(), "{borrows:#?}");
    let control_flow = lower_control_flow(&typed_ir.program, &types.program);
    assert_eq!(control_flow.value_model, elamite::ir::ValueModel::Owned);
    let entry = resolved
        .declarations
        .iter()
        .find(|declaration| resolved.symbol_text(declaration.name) == "main")
        .map(|declaration| declaration.id)
        .expect("resolved main declaration");
    let output = emit_c(
        &control_flow,
        &resolved,
        &types.program,
        &sources,
        &COptions {
            target: Target::X86_64,
            entry: Some(entry),
            test_entries: None,
        },
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    assert!(output.source.contains("#define EL_OWNED_VALUES 1"));
    assert!(output.source.contains("el_clone_t"));
    assert!(!output.source.contains("_Static_assert"));
    let x86_output = emit_c(
        &control_flow,
        &resolved,
        &types.program,
        &sources,
        &COptions {
            target: Target::X86,
            entry: Some(entry),
            test_entries: None,
        },
    );
    assert!(
        x86_output.diagnostics.is_empty(),
        "{:#?}",
        x86_output.diagnostics
    );
    assert!(x86_output.source.contains("#define EL_OWNED_VALUES 1"));
    assert!(!x86_output.source.contains("_Static_assert"));
    let c_path = package.root.join("owned-core.c");
    let executable = package.root.join("owned-core");
    fs::write(&c_path, output.source).expect("write generated C");
    let compiled = Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()))
        .args([
            "-std=c99",
            "-pedantic-errors",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-fsanitize=address",
        ])
        .arg(&c_path)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("invoke C99 compiler");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let run = Command::new(&executable)
        .env("ASAN_OPTIONS", "detect_leaks=0:halt_on_error=1")
        .output()
        .expect("run owned core program");
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8(run.stdout).expect("UTF-8 output"),
        "2\n3\n0\nbox\nchanged\n1\ntrue\n10\n1\ntrue\ntrue\n6\n2\n3\n9\n13\nfirst\n"
    );
}

#[test]
fn owned_closures_are_inline_shared_callable_values() {
    let package = TestPackage::executable(
        "owned_inline_closures",
        r#"fn apply[F: Callable[(i32,), i32]](callback: F, value: i32) -> i32:
    return callback(value)

fn invoke(callback: &Callable[(i32,), i32], value: i32) -> i32:
    return callback(value)

fn main() -> ():
    let label = String.from("owned")
    let show = fn[label](value: i32) -> i32:
        println(&label)
        return value + 1
    let duplicate = show.clone()
    println(show(2))
    println(duplicate(3))
    println(apply(show.clone(), 4))
    println(invoke(&show, 5))
    let stored = @vec[duplicate]
    println(stored.len())

    var total = 0
    let add = fn[&var total as state](value: i32) -> i32:
        *state += value
        return *state
    println(add(2))
    println(add(3))

    let base = 5
    let inner = fn[base](value: i32) -> i32:
        return base + value
    let outer = fn[inner](value: i32) -> i32:
        return inner(value) + 1
    println(outer(6))
    println(outer(7))

    let increment = fn[](value: i32) -> i32:
        return value + 1
    let exact = increment as &fn(i32) -> i32
    println(exact(9))
"#,
    );
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let resolution = resolve(&graph, &mut sources);
    assert!(
        resolution.diagnostics.is_empty(),
        "{:#?}",
        resolution.diagnostics
    );
    let resolved = resolution.program;
    let mut types = resolve_types(&resolved);
    assert!(types.diagnostics.is_empty(), "{:#?}", types.diagnostics);
    let traits = elamite::traits::check_traits(&resolved, &mut types.program);
    assert!(traits.diagnostics.is_empty(), "{:#?}", traits.diagnostics);
    let checked = check_for_target(&resolved, &mut types.program, 64);
    assert!(checked.diagnostics.is_empty(), "{:#?}", checked.diagnostics);
    let provenance = infer_provenance_signatures(&resolved, &mut types.program);
    assert!(provenance.is_empty(), "{provenance:#?}");
    let typed_ir = lower_typed_ir(&resolved, &mut types.program, &checked.program);
    assert!(
        typed_ir.diagnostics.is_empty(),
        "{:#?}",
        typed_ir.diagnostics
    );
    let moves = check_moves(&resolved, &mut types.program, &typed_ir.program);
    assert!(moves.is_empty(), "{moves:#?}");
    let borrows = check_borrows(&resolved, &mut types.program, &typed_ir.program);
    assert!(borrows.is_empty(), "{borrows:#?}");
    let control_flow = lower_control_flow(&typed_ir.program, &types.program);
    assert_eq!(control_flow.value_model, elamite::ir::ValueModel::Owned);
    let entry = resolved
        .declarations
        .iter()
        .find(|declaration| resolved.symbol_text(declaration.name) == "main")
        .map(|declaration| declaration.id)
        .expect("resolved main declaration");
    let output = emit_c(
        &control_flow,
        &resolved,
        &types.program,
        &sources,
        &COptions {
            target: Target::X86_64,
            entry: Some(entry),
            test_entries: None,
        },
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    assert!(output.source.contains("} el_closure_t"));
    assert!(!output.source.contains("} *el_closure_t"));
    assert!(output.source.contains(" *el_env"));
    assert!(output.source.contains("el_clone_t"));
    assert!(!output.source.contains("_Static_assert"));

    let x86_output = emit_c(
        &control_flow,
        &resolved,
        &types.program,
        &sources,
        &COptions {
            target: Target::X86,
            entry: Some(entry),
            test_entries: None,
        },
    );
    assert!(
        x86_output.diagnostics.is_empty(),
        "{:#?}",
        x86_output.diagnostics
    );
    assert!(x86_output.source.contains("} el_closure_t"));
    assert!(!x86_output.source.contains("} *el_closure_t"));
    assert!(!x86_output.source.contains("_Static_assert"));

    let c_path = package.root.join("inline-closures.c");
    fs::write(&c_path, output.source).expect("write generated C");
    for optimization in ["-O0", "-O2"] {
        let executable = package
            .root
            .join(format!("inline-closures-{}", &optimization[1..]));
        let compiled = Command::new(std::env::var_os("CC").unwrap_or_else(|| "cc".into()))
            .args([
                "-std=c99",
                "-pedantic-errors",
                "-Wall",
                "-Wextra",
                "-Werror",
                optimization,
            ])
            .arg(&c_path)
            .arg("-o")
            .arg(&executable)
            .output()
            .expect("invoke C99 compiler");
        assert!(
            compiled.status.success(),
            "generated C failed under {optimization}: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let run = Command::new(&executable)
            .output()
            .expect("run generated program");
        assert!(run.status.success(), "program failed under {optimization}");
        assert_eq!(
            String::from_utf8(run.stdout).expect("UTF-8 output"),
            "owned\n3\nowned\n4\nowned\n5\nowned\n6\n1\n2\n5\n12\n13\n10\n"
        );
    }
}

#[test]
fn owned_reference_temporaries_and_variadic_packs_use_stack_storage() {
    let package = TestPackage::new(
        "owned_stack_storage",
        r#"
struct Point:
    value: i32

fn first(values: ...i32) -> i32:
    return values[0]

pub fn inspect() -> i32:
    let point = &Point { value: 41 }
    return point.value + first(1, 2)
"#,
    );
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let resolution = resolve(&graph, &mut sources);
    assert!(resolution.diagnostics.is_empty());
    let resolved = resolution.program;
    let mut types = resolve_types(&resolved);
    assert!(types.diagnostics.is_empty());
    let traits = elamite::traits::check_traits(&resolved, &mut types.program);
    assert!(traits.diagnostics.is_empty());
    let checked = check_for_target(&resolved, &mut types.program, 64);
    assert!(checked.diagnostics.is_empty(), "{:#?}", checked.diagnostics);
    infer_provenance_signatures(&resolved, &mut types.program);
    let typed_ir = lower_typed_ir(&resolved, &mut types.program, &checked.program);
    assert!(typed_ir.diagnostics.is_empty());
    assert!(check_moves(&resolved, &mut types.program, &typed_ir.program).is_empty());
    assert!(check_borrows(&resolved, &mut types.program, &typed_ir.program).is_empty());
    assert!(
        typed_ir
            .program
            .functions
            .iter()
            .all(|function| function.promoted_locals.is_empty())
    );
    let control = lower_control_flow(&typed_ir.program, &types.program);
    assert!(control.functions.iter().any(|function| {
        function.blocks.iter().any(|block| {
            block.instructions.iter().any(|instruction| {
                matches!(
                    instruction,
                    elamite::ir::Instruction::Assign {
                        value: elamite::ir::Rvalue::AddressOfStableTemporary { .. },
                        ..
                    }
                )
            })
        })
    }));
    assert!(control.functions.iter().any(|function| {
        function.blocks.iter().any(|block| {
            block.instructions.iter().any(|instruction| {
                matches!(
                    instruction,
                    elamite::ir::Instruction::Assign {
                        value: elamite::ir::Rvalue::VariadicSlice { .. },
                        ..
                    }
                )
            })
        })
    }));
    let output = emit_c(
        &control,
        &resolved,
        &types.program,
        &sources,
        &COptions {
            target: Target::X86_64,
            entry: None,
            test_entries: None,
        },
    );
    assert!(output.diagnostics.is_empty(), "{:#?}", output.diagnostics);
    assert!(output.source.contains("el_stack_t"));
    assert!(output.source.contains("el_variadic_t"));
    assert!(!output.source.contains("GC_"));
    assert!(output.native_libraries.is_empty());
}

#[test]
fn owned_closure_construction_is_allocation_free_and_capture_moves_are_rejected() {
    let package = TestPackage::executable(
        "owned_allocation_free_closure",
        r#"fn main() -> ():
    let base = 4
    let add = fn[base](value: i32) -> i32:
        return base + value
    println(add(3))
    var slot = 1
    let bump = fn[&var slot as state](value: i32) -> i32:
        *state += value
        return *state
    println(bump(2))
"#,
    );
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let resolution = resolve(&graph, &mut sources);
    assert!(resolution.diagnostics.is_empty());
    let resolved = resolution.program;
    let mut types = resolve_types(&resolved);
    assert!(types.diagnostics.is_empty());
    let traits = elamite::traits::check_traits(&resolved, &mut types.program);
    assert!(traits.diagnostics.is_empty());
    let checked = check_for_target(&resolved, &mut types.program, 64);
    assert!(checked.diagnostics.is_empty(), "{:#?}", checked.diagnostics);
    let typed_ir = lower_typed_ir(&resolved, &mut types.program, &checked.program);
    assert!(typed_ir.diagnostics.is_empty());
    let main = typed_ir
        .program
        .functions
        .iter()
        .find(|function| function.name == "main")
        .expect("lowered main function");
    assert!(
        main.promoted_locals.is_empty(),
        "owned borrow captures keep their source in the enclosing frame"
    );
    assert!(
        check_moves(&resolved, &mut types.program, &typed_ir.program).is_empty(),
        "simple closure must pass move checking"
    );
    let control_flow = lower_control_flow(&typed_ir.program, &types.program);
    assert_eq!(control_flow.value_model, elamite::ir::ValueModel::Owned);

    let diagnostics = owned_move_diagnostics(
        "owned_move_from_closure_capture",
        r#"fn main() -> ():
    let text = String.from("capture")
    let consume = fn[text]() -> String:
        return text
"#,
    );
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.category == Category::Ownership
                && diagnostic.message.contains("shared receiver")
        }),
        "{diagnostics:#?}"
    );

    let package = TestPackage::new(
        "owned_invalid_closure_conversions",
        r#"fn main() -> ():
    let value = 1
    let capturing = fn[value](input: i32) -> i32:
        return value + input
    let invalid_capture = capturing as &fn(i32) -> i32
    let empty = fn[](input: i32) -> i32:
        return input
    let invalid_signature = empty as &fn(i64) -> i32
"#,
    );
    let mut sources = SourceManager::new();
    let graph = package.graph(&mut sources, SemanticRevision::V0_11);
    let resolution = resolve(&graph, &mut sources);
    assert!(resolution.diagnostics.is_empty());
    let resolved = resolution.program;
    let mut types = resolve_types(&resolved);
    assert!(types.diagnostics.is_empty());
    let traits = elamite::traits::check_traits(&resolved, &mut types.program);
    assert!(traits.diagnostics.is_empty());
    let checked = check_for_target(&resolved, &mut types.program, 64);
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic.category == Category::ExpressionType
                && diagnostic.message.contains("capturing closure")
        }),
        "{:#?}",
        checked.diagnostics
    );
    assert!(
        checked.diagnostics.iter().any(|diagnostic| {
            diagnostic.category == Category::ExpressionType
                && diagnostic.message.contains("exact safe function reference")
        }),
        "{:#?}",
        checked.diagnostics
    );
}
