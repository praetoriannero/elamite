use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::check::{check_for_target, check_moves};
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
fn owned_packages_stop_after_move_checking() {
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
    assert!(
        diagnostics[0]
            .message
            .contains("structural borrow provenance")
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
        "owned locals retain explicit destruction requirements without scheduling cleanup yet"
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
