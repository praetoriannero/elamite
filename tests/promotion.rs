use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::package::PackageGraph;
use elamite::parser::{SyntaxElement, SyntaxKind, SyntaxNode};
use elamite::promotion::address_taken_locals;
use elamite::resolution::{DeclarationKind, ResolvedProgram, resolve};
use elamite::source::SourceManager;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestTree {
    root: PathBuf,
}

impl TestTree {
    fn new(name: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "elamite-promotion-{}-{name}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("src")).expect("create test tree");
        Self { root }
    }

    fn package(&self, source: &str) -> PathBuf {
        fs::write(
            self.root.join("elamite.toml"),
            "[package]\nname = \"promotion_test\"\nversion = \"0.1.0\"\n\
             target_kind = \"executable\"\n",
        )
        .expect("write manifest");
        fs::write(self.root.join("src/main.elx"), source).expect("write source");
        self.root.clone()
    }
}

impl Drop for TestTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn resolve_source(source: &str) -> (SourceManager, ResolvedProgram) {
    let tree = TestTree::new("case");
    let package = tree.package(source);
    let mut sources = SourceManager::new();
    let graph = PackageGraph::resolve(&Path::new(&package).join("elamite.toml"), &mut sources)
        .expect("package graph resolves");
    let output = resolve(&graph, &mut sources);
    assert!(
        output.diagnostics.is_empty(),
        "{:?}",
        output
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect::<Vec<_>>()
    );
    (sources, output.program)
}

fn body_of<'a>(resolved: &'a ResolvedProgram, name: &str) -> &'a SyntaxNode {
    let declaration = resolved
        .declarations
        .iter()
        .find(|declaration| {
            declaration.kind == DeclarationKind::Function
                && resolved.symbol_text(declaration.name) == name
        })
        .unwrap_or_else(|| panic!("function `{name}` is declared"));
    declaration
        .syntax
        .children
        .iter()
        .find_map(|child| match child {
            SyntaxElement::Node(node) if node.kind == SyntaxKind::Block => Some(node.as_ref()),
            _ => None,
        })
        .expect("function has a body")
}

/// Names of the locals promoted in `name`, resolved back to source text so the
/// assertions read like the program rather than like binding indices.
fn promoted_names(source: &str, name: &str) -> Vec<String> {
    let (_sources, resolved) = resolve_source(source);
    let body = body_of(&resolved, name);
    address_taken_locals(&resolved, body)
        .into_iter()
        .map(|binding| {
            let local = resolved
                .local_bindings
                .iter()
                .find(|local| local.id == binding)
                .expect("promoted binding exists");
            resolved.symbol_text(local.name).to_string()
        })
        .collect()
}

#[test]
fn promotes_only_locals_whose_address_is_taken() {
    let source = r#"
struct Address:
    city: i32

struct Account:
    id: i32
    address: Address

fn taken() -> ():
    let plain = 1
    let referenced = 2
    var mutable_target = 3
    let shared = &referenced
    let exclusive = &var mutable_target
    println(f"{plain}{*shared}{*exclusive}")
"#;
    assert_eq!(
        promoted_names(source, "taken"),
        vec!["referenced".to_string(), "mutable_target".to_string()],
        "only address-taken locals are promoted, in stable binding order"
    );
}

#[test]
fn a_reference_through_a_path_promotes_the_root_local() {
    // SPEC 3.2: a reference points into its container's storage, so the whole
    // container is promoted rather than one boxed field.
    let source = r#"
struct Address:
    city: i32

struct Account:
    id: i32
    address: Address

fn nested() -> ():
    let account = Account { id: 1, address: Address { city: 2 } }
    let city = &account.address.city
    println(f"{*city}")
"#;
    assert_eq!(
        promoted_names(source, "nested"),
        vec!["account".to_string()]
    );
}

#[test]
fn a_referenced_composite_literal_promotes_no_local() {
    // A referenced composite literal allocates its own managed cell and has no
    // source-level binding to promote.
    let source = r#"
struct Address:
    city: i32

fn literal() -> ():
    let city = &Address { city: 1 }
    println(f"{city.city}")
"#;
    assert!(promoted_names(source, "literal").is_empty());
}

#[test]
fn promotion_is_per_function() {
    let source = r#"
fn takes_address() -> ():
    let value = 1
    let reference = &value
    println(f"{*reference}")

fn takes_none() -> ():
    let value = 1
    println(f"{value}")
"#;
    assert_eq!(
        promoted_names(source, "takes_address"),
        vec!["value".to_string()]
    );
    assert!(promoted_names(source, "takes_none").is_empty());
}

#[test]
fn nested_scopes_and_repeated_references_promote_once() {
    let source = r#"
fn nested_scopes() -> ():
    var counter = 0
    let first = &var counter
    while counter < 3:
        let inner = &var counter
        *inner += 1
    println(f"{*first}")
"#;
    assert_eq!(
        promoted_names(source, "nested_scopes"),
        vec!["counter".to_string()],
        "a local promoted from several reference sites appears once"
    );
}

/// Runs the frontend far enough to inspect lowered IR. Reference *lowering* is
/// Phase 3 of Milestone 10, so a body containing `&` still produces lowering
/// diagnostics; promotion is computed from syntax and is available regardless.
fn lowered(source: &str) -> (elamite::ir::TypedIrProgram, bool) {
    let (_sources, resolved) = resolve_source(source);
    let mut typed = elamite::types::resolve_types(&resolved);
    assert!(typed.diagnostics.is_empty(), "{:?}", typed.diagnostics);
    let checked = elamite::check::check(&resolved, &mut typed.program);
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    let high_level = elamite::ir::lower_typed_ir(&resolved, &mut typed.program, &checked.program);
    let control_flow = elamite::ir::lower_control_flow(&high_level.program, &typed.program);
    let requires_managed_memory = control_flow.requires_managed_memory;
    for function in &control_flow.functions {
        let matching = high_level
            .program
            .functions
            .iter()
            .find(|typed| typed.declaration == function.declaration)
            .expect("every control-flow function has a typed source");
        assert_eq!(
            function.promoted_locals, matching.promoted_locals,
            "control-flow lowering must carry promotion forward unchanged"
        );
    }
    (high_level.program, requires_managed_memory)
}

#[test]
fn promotion_reaches_the_ir_and_drives_the_managed_memory_flag() {
    let (program, requires_managed_memory) = lowered(
        r#"
fn takes_address() -> ():
    let value = 1
    let reference = &value
    println(f"{*reference}")
"#,
    );
    let function = program
        .functions
        .iter()
        .find(|function| function.name == "takes_address")
        .expect("the function is lowered");
    assert_eq!(function.promoted_locals.len(), 1);
    assert!(
        requires_managed_memory,
        "a promoted local requires managed storage"
    );

    let (program, requires_managed_memory) = lowered(
        r#"
fn plain() -> ():
    let value = 1
    println(value)
"#,
    );
    let function = program
        .functions
        .iter()
        .find(|function| function.name == "plain")
        .expect("the function is lowered");
    assert!(function.promoted_locals.is_empty());
    assert!(
        !requires_managed_memory,
        "a program with no promoted local or allocating value stays collector-free"
    );
}

#[test]
fn a_parameter_whose_address_is_taken_is_promoted() {
    // Parameters are lexical bindings like any other local, and a reference to
    // one may outlive the frame, so they promote on the same rule.
    let source = r#"
fn borrow(value: i32, untouched: i32) -> ():
    let reference = &value
    println(f"{*reference}{untouched}")
"#;
    assert_eq!(promoted_names(source, "borrow"), vec!["value".to_string()]);
}
