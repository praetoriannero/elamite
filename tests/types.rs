use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use elamite::diagnostics::{Category, Diagnostic};
use elamite::lexer::NumericSuffix;
use elamite::package::PackageGraph;
use elamite::resolution::{DeclarationKind, resolve};
use elamite::source::SourceManager;
use elamite::types::{
    Abi, ExpectedType, FunctionParameter, Mutability, PlaceKind, PrimitiveType, Safety,
    TypeContext, TypeKind, resolve_types,
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestTree {
    root: PathBuf,
}

impl TestTree {
    fn new(name: &str) -> Self {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "elamite-types-{}-{name}-{serial}",
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

fn diagnostics(sources: &SourceManager, diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| {
            diagnostic.primary.map_or_else(
                || diagnostic.message.clone(),
                |span| {
                    let position = sources.line_col(span.file, span.start);
                    format!(
                        "{}:{}:{}: {:?}: {}",
                        sources.path(span.file).display(),
                        position.line,
                        position.column,
                        diagnostic.category,
                        diagnostic.message
                    )
                },
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn canonicalizes_aliases_generic_applications_and_unit() {
    let tree = TestTree::new("aliases");
    let package = tree.package(
        "app",
        "executable",
        &[],
        &[(
            "src/main.elx",
            "type Pair[T] = (T, T)\n\
             type IntPair = Pair[i32]\n\
             type Grouped = (i32)\n\
             type Nothing = ()\n\
             struct Holder:\n\
             \x20\x20\x20\x20pair: IntPair\n\
             \x20\x20\x20\x20nothing: Nothing\n\
             fn choose(value: Grouped) -> i32:\n\
             \x20\x20\x20\x20return value\n",
        )],
    );
    let (sources, resolved) = resolve_package(&package);
    assert!(
        resolved.diagnostics.is_empty(),
        "{}",
        diagnostics(&sources, &resolved.diagnostics)
    );
    let typed = resolve_types(&resolved.program);
    assert!(
        typed.diagnostics.is_empty(),
        "{}",
        diagnostics(&sources, &typed.diagnostics)
    );

    let declaration = |name: &str| {
        resolved
            .program
            .declarations
            .iter()
            .find(|declaration| resolved.program.symbol_text(declaration.name) == name)
            .expect("declaration exists")
            .id
    };
    let int_pair = typed.program.declaration_types[&declaration("IntPair")];
    let holder_pair = typed.program.field_types[&resolved
        .program
        .fields
        .iter()
        .find(|field| resolved.program.symbol_text(field.name) == "pair")
        .expect("pair field")
        .id];
    assert!(typed.program.types.exactly_equal(int_pair, holder_pair));
    assert!(typed.program.types.contains_explicit_alias(int_pair));

    let grouped = typed.program.declaration_types[&declaration("Grouped")];
    let i32_type = {
        let signature = &typed.program.function_signatures[&declaration("choose")];
        signature.return_type
    };
    assert!(typed.program.types.exactly_equal(grouped, i32_type));
    let nothing = typed.program.declaration_types[&declaration("Nothing")];
    let unit_target = match typed.program.types.kind(nothing) {
        TypeKind::Alias { target, .. } => *target,
        _ => nothing,
    };
    assert!(typed.program.types.exactly_equal(nothing, unit_target));
    assert!(matches!(
        typed.program.types.kind(unit_target),
        TypeKind::Primitive(PrimitiveType::Unit)
    ));
}

#[test]
fn nominal_identity_includes_the_package_instance() {
    let tree = TestTree::new("package-identity");
    tree.package(
        "left",
        "library",
        &[],
        &[("src/lib.elx", "pub struct Value:\n    pass\n")],
    );
    tree.package(
        "right",
        "library",
        &[],
        &[("src/lib.elx", "pub struct Value:\n    pass\n")],
    );
    let root = tree.package(
        "root",
        "executable",
        &[("left", "../left"), ("right", "../right")],
        &[(
            "src/main.elx",
            "import left.Value as LeftValue\n\
             import right.Value as RightValue\n\
             struct Pair:\n\
             \x20\x20\x20\x20left: LeftValue\n\
             \x20\x20\x20\x20right: RightValue\n",
        )],
    );
    let (sources, resolved) = resolve_package(&root);
    assert!(
        resolved.diagnostics.is_empty(),
        "{}",
        diagnostics(&sources, &resolved.diagnostics)
    );
    let typed = resolve_types(&resolved.program);
    assert!(
        typed.diagnostics.is_empty(),
        "{}",
        diagnostics(&sources, &typed.diagnostics)
    );
    let values = resolved
        .program
        .declarations
        .iter()
        .filter(|declaration| resolved.program.symbol_text(declaration.name) == "Value")
        .map(|declaration| typed.program.declaration_types[&declaration.id])
        .collect::<Vec<_>>();
    assert_eq!(values.len(), 2);
    assert!(!typed.program.types.exactly_equal(values[0], values[1]));
    let packages = values
        .iter()
        .map(|ty| match typed.program.types.kind(*ty) {
            TypeKind::Nominal { identity, .. } => identity.package.clone(),
            kind => panic!("expected nominal type, got {kind:?}"),
        })
        .collect::<Vec<_>>();
    assert_ne!(packages[0], packages[1]);
}

#[test]
fn function_identity_is_invariant_and_preserves_all_markers() {
    let mut types = TypeContext::new();
    let i32_type = types.primitive(PrimitiveType::I32);
    let unit = types.primitive(PrimitiveType::Unit);
    let parameter = FunctionParameter {
        ty: i32_type,
        variadic: false,
    };
    let base = types.intern(TypeKind::Function {
        safety: Safety::Safe,
        abi: Abi::Elamite,
        receiver: None,
        parameters: vec![parameter.clone()],
        return_type: unit,
    });
    assert_eq!(
        base,
        types.intern(TypeKind::Function {
            safety: Safety::Safe,
            abi: Abi::Elamite,
            receiver: None,
            parameters: vec![parameter.clone()],
            return_type: unit,
        })
    );
    let unsafe_type = types.intern(TypeKind::Function {
        safety: Safety::Unsafe,
        abi: Abi::Elamite,
        receiver: None,
        parameters: vec![parameter.clone()],
        return_type: unit,
    });
    let variadic = types.intern(TypeKind::Function {
        safety: Safety::Safe,
        abi: Abi::Elamite,
        receiver: None,
        parameters: vec![FunctionParameter {
            ty: i32_type,
            variadic: true,
        }],
        return_type: unit,
    });
    let receiver = types.intern(TypeKind::Function {
        safety: Safety::Safe,
        abi: Abi::Elamite,
        receiver: Some(i32_type),
        parameters: vec![],
        return_type: unit,
    });
    let c_abi = types.intern(TypeKind::Function {
        safety: Safety::Safe,
        abi: Abi::C,
        receiver: None,
        parameters: vec![parameter],
        return_type: unit,
    });
    for distinct in [unsafe_type, variadic, receiver, c_abi] {
        assert!(!types.exactly_equal(base, distinct));
        assert!(types.unify(base, distinct).is_err());
    }

    let shared = types.intern(TypeKind::Reference {
        mutability: Mutability::Shared,
        target: i32_type,
    });
    let mutable = types.intern(TypeKind::Reference {
        mutability: Mutability::Mutable,
        target: i32_type,
    });
    assert!(!types.exactly_equal(shared, mutable));
    assert!(types.unify(shared, mutable).is_err());

    let i64_type = types.primitive(PrimitiveType::I64);
    assert!(types.unify(i32_type, i64_type).is_err());

    let inferred = types.fresh_inference_variable();
    let tuple_with_variable = types.intern(TypeKind::Tuple(vec![inferred, i32_type]));
    let u8_type = types.primitive(PrimitiveType::U8);
    let tuple_with_u8 = types.intern(TypeKind::Tuple(vec![u8_type, i32_type]));
    types
        .unify(tuple_with_variable, tuple_with_u8)
        .expect("inference variable should bind structurally");
    assert!(matches!(
        types.kind(types.resolve_inference(inferred)),
        TypeKind::Primitive(PrimitiveType::U8)
    ));
}

#[test]
fn materializes_literals_contextually_and_checks_both_pointer_widths() {
    let mut types = TypeContext::new();
    let i32_type = types.primitive(PrimitiveType::I32);
    assert_eq!(
        types
            .materialize_integer("1", 10, None, false, ExpectedType::None, 64)
            .expect("default integer"),
        i32_type
    );
    let i8_type = types.primitive(PrimitiveType::I8);
    assert_eq!(
        types
            .materialize_integer(
                "128i8",
                10,
                Some(NumericSuffix::I8),
                true,
                ExpectedType::None,
                64,
            )
            .expect("signed minimum"),
        i8_type
    );
    assert!(
        types
            .materialize_integer(
                "128i8",
                10,
                Some(NumericSuffix::I8),
                false,
                ExpectedType::None,
                64,
            )
            .is_err()
    );
    let usize_type = types.primitive(PrimitiveType::Usize);
    assert!(
        types
            .materialize_integer(
                "4294967296",
                10,
                None,
                false,
                ExpectedType::Exact(usize_type),
                32,
            )
            .is_err()
    );
    assert_eq!(
        types
            .materialize_integer(
                "4294967296",
                10,
                None,
                false,
                ExpectedType::Exact(usize_type),
                64,
            )
            .expect("64-bit usize"),
        usize_type
    );
    let f32_type = types.primitive(PrimitiveType::F32);
    assert_eq!(
        types
            .materialize_integer("7", 10, None, false, ExpectedType::Exact(f32_type), 64,)
            .expect("integer to contextual float"),
        f32_type
    );
    let default_float = types
        .materialize_float("1.5", None, ExpectedType::None)
        .expect("default float");
    assert!(matches!(
        types.kind(default_float),
        TypeKind::Primitive(PrimitiveType::F64)
    ));
    let string = types.primitive(PrimitiveType::String);
    assert_eq!(
        types
            .materialize_string(ExpectedType::Exact(string))
            .expect("contextual String"),
        string
    );
    let default_string = types
        .materialize_string(ExpectedType::None)
        .expect("default str");
    assert!(matches!(
        types.kind(default_string),
        TypeKind::Primitive(PrimitiveType::Str)
    ));
}

#[test]
fn reports_alias_cycles_wrong_arity_and_non_trait_bounds() {
    let tree = TestTree::new("type-errors");
    let package = tree.package(
        "app",
        "executable",
        &[],
        &[(
            "src/main.elx",
            "type First = Second\n\
             type Second = First\n\
             struct Box[T]:\n\
             \x20\x20\x20\x20value: T\n\
             struct Bad[T: i32]:\n\
             \x20\x20\x20\x20value: Box\n",
        )],
    );
    let (sources, resolved) = resolve_package(&package);
    assert!(
        resolved.diagnostics.is_empty(),
        "{}",
        diagnostics(&sources, &resolved.diagnostics)
    );
    let typed = resolve_types(&resolved.program);
    let text = diagnostics(&sources, &typed.diagnostics);
    assert!(
        typed
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.category == Category::TypeSystem),
        "{text}"
    );
    assert!(text.contains("recursive"), "{text}");
    assert!(text.contains("expects 1 type argument"), "{text}");
    assert!(text.contains("bounds must name traits"), "{text}");
}

#[test]
fn exposes_layout_place_obligation_reference_and_abi_queries() {
    let tree = TestTree::new("queries");
    let package = tree.package(
        "app",
        "executable",
        &[],
        &[(
            "src/main.elx",
            "trait Marker:\n\
             \x20\x20\x20\x20fn mark(self: &Self) -> ()\n\
             type CounterRef = &var i32\n\
             struct Wrapper[T]:\n\
             \x20\x20\x20\x20value: T\n\
             type Concrete = Wrapper[i32]\n\
             fn constrained[T: Marker](value: T) -> ():\n\
             \x20\x20\x20\x20pass\n\
             impl[T: Marker] Marker for Wrapper[T]:\n\
             \x20\x20\x20\x20fn mark(self: &Self) -> ():\n\
             \x20\x20\x20\x20\x20\x20\x20\x20pass\n\
             extern \"C\":\n\
             \x20\x20\x20\x20type Opaque\n\
             \x20\x20\x20\x20struct CRecord:\n\
             \x20\x20\x20\x20\x20\x20\x20\x20number: i32\n\
             \x20\x20\x20\x20\x20\x20\x20\x20handle: *Opaque\n\
             \x20\x20\x20\x20fn accept(record: CRecord) -> i32\n",
        )],
    );
    let (sources, resolved) = resolve_package(&package);
    assert!(
        resolved.diagnostics.is_empty(),
        "{}",
        diagnostics(&sources, &resolved.diagnostics)
    );
    let typed = resolve_types(&resolved.program);
    assert!(
        typed.diagnostics.is_empty(),
        "{}",
        diagnostics(&sources, &typed.diagnostics)
    );
    let declaration = |name: &str| {
        resolved
            .program
            .declarations
            .iter()
            .find(|declaration| resolved.program.symbol_text(declaration.name) == name)
            .expect("declaration exists")
    };

    let alias = typed.program.declaration_types[&declaration("CounterRef").id];
    assert!(typed.program.types.contains_explicit_alias(alias));
    assert!(typed.program.types.contains_managed_reference(alias));
    assert!(typed.program.types.contains_mutable_indirection(alias));
    assert!(typed.program.layout_available(alias, 32));
    assert!(typed.program.layout_available(alias, 64));
    let concrete = typed.program.declaration_types[&declaration("Concrete").id];
    assert!(typed.program.layout_available(concrete, 32));
    let marker = typed.program.declaration_types[&declaration("Marker").id];
    assert!(!typed.program.layout_available(marker, 64));

    let opaque = typed.program.declaration_types[&declaration("Opaque").id];
    assert!(!typed.program.layout_available(opaque, 64));
    assert!(!typed.program.is_abi_safe(opaque));
    let record = typed.program.declaration_types[&declaration("CRecord").id];
    assert!(typed.program.layout_available(record, 32));
    assert!(typed.program.is_abi_safe(record));
    let accept = &typed.program.function_signatures[&declaration("accept").id];
    assert!(typed.program.is_abi_safe(accept.ty));

    let constrained = declaration("constrained");
    let parameter = constrained.generic_parameters[0];
    assert_eq!(typed.program.obligations_for(parameter).count(), 1);
    let implementation_parameter = resolved.program.impls[0].generic_parameters[0];
    assert_eq!(
        typed
            .program
            .obligations_for(implementation_parameter)
            .count(),
        1
    );

    assert!(PlaceKind::Addressable.is_addressable());
    assert!(PlaceKind::Mutable.is_mutable());
    assert!(PlaceKind::Mutable.permits_safe_reference());
    assert!(PlaceKind::CollectionInterior.is_mutable());
    assert!(!PlaceKind::CollectionInterior.permits_safe_reference());
    assert!(!PlaceKind::Value.is_addressable());
}

#[test]
fn canonicalizes_the_authoritative_demonstration() {
    let tree = TestTree::new("spec-demo");
    let package = tree.package(
        "demo",
        "executable",
        &[],
        &[("src/main.elx", include_str!("../examples/spec_demo.elx"))],
    );
    let (sources, resolved) = resolve_package(&package);
    assert!(
        resolved.diagnostics.is_empty(),
        "{}",
        diagnostics(&sources, &resolved.diagnostics)
    );
    let typed = resolve_types(&resolved.program);
    assert!(
        typed.diagnostics.is_empty(),
        "{}",
        diagnostics(&sources, &typed.diagnostics)
    );
    assert!(
        typed.program.function_signatures.len()
            >= resolved
                .program
                .declarations
                .iter()
                .filter(|declaration| matches!(
                    declaration.kind,
                    DeclarationKind::Function | DeclarationKind::ForeignFunction
                ))
                .count()
    );
    assert!(typed.program.types.len() > 30);
}

#[test]
fn a_bare_trait_name_is_a_type_only_where_a_trait_is_expected() {
    // SPEC 6: a trait has no value representation, so it names a type only as
    // a reference target, a bound, or an `impl Trait for Type` trait.
    let accepted = "trait Toggle:\n\
         \x20\x20\x20\x20fn status(self: &Self) -> str\n\
         struct Session:\n\
         \x20\x20\x20\x20active: bool\n\
         impl Toggle for Session:\n\
         \x20\x20\x20\x20fn status(self: &Self) -> str:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20return \"on\"\n\
         fn shared(t: &Toggle) -> ():\n\
         \x20\x20\x20\x20pass\n\
         fn mutable(t: &var Toggle) -> ():\n\
         \x20\x20\x20\x20pass\n\
         fn bounded[T: Toggle](value: &T) -> ():\n\
         \x20\x20\x20\x20pass\n";
    let tree = TestTree::new("trait-position-ok");
    let package = tree.package("app", "executable", &[], &[("src/main.elx", accepted)]);
    let (sources, resolved) = resolve_package(&package);
    let typed = resolve_types(&resolved.program);
    assert!(
        typed.diagnostics.is_empty(),
        "{}",
        diagnostics(&sources, &typed.diagnostics)
    );

    for (index, value_position) in [
        "struct Holder:\n\x20\x20\x20\x20field: Toggle\n",
        "type Alias = Toggle\n",
        "fn parameter(t: Toggle) -> ():\n\x20\x20\x20\x20pass\n",
        "fn returns() -> Toggle:\n\x20\x20\x20\x20pass\n",
        "fn generic_argument(v: Vec[Toggle]) -> ():\n\x20\x20\x20\x20pass\n",
        "fn raw(t: *Toggle) -> ():\n\x20\x20\x20\x20pass\n",
    ]
    .into_iter()
    .enumerate()
    {
        let source = format!(
            "trait Toggle:\n\x20\x20\x20\x20fn status(self: &Self) -> str\n{value_position}"
        );
        let tree = TestTree::new(&format!("trait-position-{index}"));
        let package = tree.package("app", "executable", &[], &[("src/main.elx", &source)]);
        let (sources, resolved) = resolve_package(&package);
        let typed = resolve_types(&resolved.program);
        let text = diagnostics(&sources, &typed.diagnostics);
        assert!(
            typed
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.category == Category::TypeSystem),
            "{text}"
        );
        assert!(
            text.contains("only behind a safe reference")
                || text.contains("raw pointer cannot address a trait object"),
            "expected a bare-trait rejection for `{value_position}`, got:\n{text}"
        );
    }
}
