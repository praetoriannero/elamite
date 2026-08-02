//! Stable resolution identities and owned result tables.

use super::*;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub(super) u32);

        impl $name {
            #[must_use]
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

id_type!(ModuleId);
id_type!(DeclarationId);
id_type!(ImportId);
id_type!(ImplId);
id_type!(FieldId);
id_type!(VariantId);
id_type!(GenericParameterId);
id_type!(LocalBindingId);
id_type!(BuiltinId);

/// Interned source spelling. It is an identity only within one
/// [`ResolvedProgram`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Symbol(pub(super) Spur);

/// Source-level visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Package,
    Public,
}

/// How a module entered the module graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleOrigin {
    Standard,
    RootFile,
    FileBacked,
    DirectoryNamespace,
    Inline,
}

/// A module, declaration, or compiler-provided entity in a module namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemId {
    Module(ModuleId),
    Declaration(DeclarationId),
    Builtin(BuiltinId),
}

/// The stable identity selected by one resolved source name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameTarget {
    Item(ItemId),
    GenericParameter(GenericParameterId),
    Local(LocalBindingId),
    SelfType,
}

/// A stable entry in a type member namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberId {
    Field(FieldId),
    Method(DeclarationId),
    Variant(VariantId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NamespaceTarget {
    Item(ItemId),
    Import(ImportId),
}

/// One resolved source occurrence. `provenance` contains import/re-export
/// spans followed to obtain the target.
#[derive(Debug, Clone)]
pub struct ResolvedReference {
    pub span: Span,
    pub target: NameTarget,
    pub provenance: Vec<Span>,
}

/// A module-level namespace entry.
#[derive(Debug, Clone)]
pub struct NamespaceEntry {
    pub name: Symbol,
    pub(super) target: NamespaceTarget,
    pub visibility: Visibility,
    pub span: Option<Span>,
}

/// One module identity.
#[derive(Debug)]
pub struct Module {
    pub id: ModuleId,
    pub package: Option<PackageId>,
    pub path: Vec<Symbol>,
    pub parent: Option<ModuleId>,
    pub origin: ModuleOrigin,
    pub source_file: Option<FileId>,
    pub span: Option<Span>,
    pub externally_reachable: bool,
    pub(super) namespace: BTreeMap<Symbol, NamespaceEntry>,
}

/// The declaration families available before type checking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclarationKind {
    TypeAlias,
    Struct,
    Enum,
    Trait,
    Function,
    Closure,
    Test,
    ForeignType,
    ForeignStruct,
    ForeignFunction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForeignDirection {
    Import,
    Export,
}

/// Compiler-validated external naming metadata supplied by `@importc` or
/// `@exportc`. The header is present only for imports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignBinding {
    pub direction: ForeignDirection,
    pub c_name: String,
    pub header: Option<String>,
}

/// One named declaration, including methods.
#[derive(Debug)]
pub struct Declaration {
    pub id: DeclarationId,
    pub module: ModuleId,
    pub name: Symbol,
    pub kind: DeclarationKind,
    pub visibility: Visibility,
    pub span: Span,
    pub syntax: SyntaxNode,
    pub parent_declaration: Option<DeclarationId>,
    pub parent_impl: Option<ImplId>,
    pub generic_parameters: Vec<GenericParameterId>,
    pub externally_reachable: bool,
    pub foreign_binding: Option<ForeignBinding>,
    /// True only for tests owned by the package selected by
    /// [`crate::resolution::resolve_for_tests`].
    pub test_selected: bool,
}

/// One `impl Trait for Type` block.
#[derive(Debug)]
pub struct ImplBlock {
    pub id: ImplId,
    pub module: ModuleId,
    pub span: Span,
    pub syntax: SyntaxNode,
    pub generic_parameters: Vec<GenericParameterId>,
    pub methods: Vec<DeclarationId>,
}

/// One struct or variant field.
#[derive(Debug)]
pub struct Field {
    pub id: FieldId,
    pub name: Symbol,
    pub visibility: Visibility,
    pub span: Span,
    pub parent_declaration: DeclarationId,
    pub parent_variant: Option<VariantId>,
    pub syntax: SyntaxNode,
}

/// One enum variant.
#[derive(Debug)]
pub struct Variant {
    pub id: VariantId,
    pub name: Symbol,
    pub span: Span,
    pub parent: DeclarationId,
    pub syntax: SyntaxNode,
    pub fields: Vec<FieldId>,
}

/// One declared generic parameter.
#[derive(Debug)]
pub struct GenericParameter {
    pub id: GenericParameterId,
    pub name: Symbol,
    pub span: Span,
}

/// One parameter, local binding, loop binding, or pattern binding.
#[derive(Debug)]
pub struct LocalBinding {
    pub id: LocalBindingId,
    pub name: Symbol,
    pub span: Span,
    pub kind: LocalBindingKind,
}

/// Why a lexical binding exists. An unqualified identifier pattern remains a
/// candidate until Milestone 7 distinguishes a binding from a unit variant
/// using the scrutinee type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalBindingKind {
    Parameter,
    ClosureCapture,
    Local,
    Loop,
    PatternCandidate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosureCaptureKind {
    Value,
    SharedReference,
    MutableReference,
    SharedRawPointer,
    MutableRawPointer,
}

#[derive(Debug, Clone)]
pub struct ClosureCapture {
    pub source: LocalBindingId,
    pub binding: LocalBindingId,
    pub kind: ClosureCaptureKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ResolvedClosure {
    pub declaration: DeclarationId,
    pub span: Span,
    pub captures: Vec<ClosureCapture>,
}

/// One import or public re-export.
#[derive(Debug)]
pub struct Import {
    pub id: ImportId,
    pub module: ModuleId,
    pub name: Symbol,
    pub visibility: Visibility,
    pub span: Span,
    pub(super) path: Vec<PathPart>,
    pub target: Option<ItemId>,
    pub provenance: Vec<Span>,
    pub(super) state: ImportState,
    pub(super) replaced_file_module: Option<ModuleId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ImportState {
    Unresolved,
    Resolving,
    Resolved,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PathPart {
    Root,
    SelfModule,
    Super,
    Name(Symbol),
}

#[derive(Debug)]
pub(super) struct Builtin {
    pub(super) name: Symbol,
}

/// Milestone 4 output. A partially resolved database is returned with
/// diagnostics so later tooling can inspect independent declarations.
pub struct ResolutionOutput {
    pub program: ResolvedProgram,
    pub diagnostics: Vec<Diagnostic>,
}

/// All identities and resolution results for one package graph.
pub struct ResolvedProgram {
    pub(super) symbols: Rodeo<Spur>,
    pub modules: Vec<Module>,
    pub declarations: Vec<Declaration>,
    pub imports: Vec<Import>,
    pub impls: Vec<ImplBlock>,
    pub fields: Vec<Field>,
    pub variants: Vec<Variant>,
    pub generic_parameters: Vec<GenericParameter>,
    pub local_bindings: Vec<LocalBinding>,
    pub closures: Vec<ResolvedClosure>,
    pub references: Vec<ResolvedReference>,
    pub declaration_members: BTreeMap<DeclarationId, BTreeMap<Symbol, MemberId>>,
    pub impl_members: BTreeMap<ImplId, BTreeMap<Symbol, DeclarationId>>,
    pub(super) builtins: Vec<Builtin>,
    pub(super) prelude: BTreeMap<Symbol, ItemId>,
    pub(super) standard_declarations: BTreeMap<Symbol, DeclarationId>,
    pub(super) package_roots: BTreeMap<PackageId, ModuleId>,
    pub(super) module_keys: BTreeMap<(PackageId, Vec<String>), ModuleId>,
    pub(super) std_root: ModuleId,
}

impl ResolvedProgram {
    #[must_use]
    pub fn symbol_text(&self, symbol: Symbol) -> &str {
        self.symbols.resolve(&symbol.0)
    }

    /// Whether a declaration is directly nameable in a module through that
    /// module's declarations or imports.
    #[must_use]
    pub fn declaration_in_scope(&self, module: ModuleId, declaration: DeclarationId) -> bool {
        let name = self.declarations[declaration.index()].name;
        self.modules[module.index()]
            .namespace
            .get(&name)
            .is_some_and(|entry| match entry.target {
                NamespaceTarget::Item(ItemId::Declaration(candidate)) => candidate == declaration,
                NamespaceTarget::Import(import) => {
                    self.imports[import.index()].target == Some(ItemId::Declaration(declaration))
                }
                _ => false,
            })
    }

    /// Returns the source spelling of a compiler-provided name.
    #[must_use]
    pub fn builtin_name(&self, builtin: BuiltinId) -> &str {
        self.symbol_text(self.builtins[builtin.index()].name)
    }

    /// Finds the compiler-known prelude entity with this exact spelling, for
    /// later passes that construct a builtin type without an existing source
    /// occurrence to resolve against (Milestone 6, e.g. `@vec`/`@map`/`@set`).
    #[must_use]
    pub fn builtin_named(&self, name: &str) -> Option<BuiltinId> {
        self.builtins
            .iter()
            .position(|builtin| self.symbol_text(builtin.name) == name)
            .map(|index| BuiltinId(index as u32))
    }

    /// Exact compiler-known leaf spellings, in identity order.
    ///
    /// This is intentionally public for the standard-intrinsic inventory
    /// audit and developer dumps. Language behavior should use stable IDs,
    /// not search this list by spelling.
    #[must_use]
    pub fn builtin_names(&self) -> Vec<&str> {
        self.builtins
            .iter()
            .map(|builtin| self.symbol_text(builtin.name))
            .collect()
    }

    /// Exact names visible through unqualified prelude lookup.
    #[must_use]
    pub fn prelude_names(&self) -> Vec<&str> {
        let mut names = self
            .prelude
            .keys()
            .map(|name| self.symbol_text(*name))
            .collect::<Vec<_>>();
        names.sort_unstable();
        names
    }

    /// Finds the standard-library declaration with this exact spelling.
    ///
    /// Standard types that the specification writes as ordinary declarations
    /// (`enum Option[T]`, `docs/SPEC.md` 4.4) are collected from compiler-supplied
    /// source into the `std` root module, so they carry real declaration,
    /// variant, field, and generic-parameter identities and travel the same
    /// generic-enum path as user code. A pass needs this lookup only where the
    /// specification gives that exact type extra behavior, such as `Option[T]`
    /// defaulting to `Option.None` without a `T: Default` obligation
    /// (`docs/SPEC.md` 4.3).
    #[must_use]
    pub fn standard_declaration(&self, name: &str) -> Option<DeclarationId> {
        self.standard_declarations
            .iter()
            .find(|(symbol, _)| self.symbol_text(**symbol) == name)
            .map(|(_, declaration)| *declaration)
    }

    /// Whether `declaration` is the standard declaration spelled `name`. A
    /// user type that merely shares the spelling is not the standard one.
    #[must_use]
    pub fn is_standard_declaration(&self, declaration: DeclarationId, name: &str) -> bool {
        self.standard_declaration(name) == Some(declaration)
    }

    /// The variant `name` of the standard declaration `owner`.
    #[must_use]
    pub fn standard_variant(&self, owner: &str, name: &str) -> Option<VariantId> {
        let declaration = self.standard_declaration(owner)?;
        self.variants
            .iter()
            .find(|variant| variant.parent == declaration && self.symbol_text(variant.name) == name)
            .map(|variant| variant.id)
    }

    /// Finds the identity previously recorded for an exact source path.
    #[must_use]
    pub fn reference_at(&self, span: Span) -> Option<&ResolvedReference> {
        self.references
            .iter()
            .find(|reference| reference.span == span)
    }

    #[must_use]
    pub fn module_namespace(&self, module: ModuleId) -> &BTreeMap<Symbol, NamespaceEntry> {
        &self.modules[module.index()].namespace
    }

    #[must_use]
    pub fn namespace_target(&self, entry: &NamespaceEntry) -> Option<ItemId> {
        match entry.target {
            NamespaceTarget::Item(item) => Some(item),
            NamespaceTarget::Import(import) => self.imports[import.index()].target,
        }
    }

    #[must_use]
    pub fn root_module(&self, package: &PackageId) -> Option<ModuleId> {
        self.package_roots.get(package).copied()
    }

    #[must_use]
    pub fn module_by_path(&self, package: &PackageId, components: &[&str]) -> Option<ModuleId> {
        self.module_keys
            .get(&(
                package.clone(),
                components.iter().map(|part| (*part).to_string()).collect(),
            ))
            .copied()
    }

    /// Produces a deterministic identity and namespace dump for debugging and
    /// regression tests.
    #[must_use]
    pub fn dump(&self) -> String {
        let mut output = String::new();
        for module in &self.modules {
            let path = if module.path.is_empty() {
                "root".to_string()
            } else {
                module
                    .path
                    .iter()
                    .map(|part| self.symbol_text(*part))
                    .collect::<Vec<_>>()
                    .join(".")
            };
            output.push_str(&format!(
                "module {} {path} {:?} reachable={}\n",
                module.id.0, module.origin, module.externally_reachable
            ));
            for entry in module.namespace.values() {
                output.push_str(&format!(
                    "  {} {:?} {:?}\n",
                    self.symbol_text(entry.name),
                    entry.visibility,
                    self.namespace_target(entry)
                ));
            }
        }
        for declaration in &self.declarations {
            output.push_str(&format!(
                "decl {} {} {:?} module={} reachable={}\n",
                declaration.id.0,
                self.symbol_text(declaration.name),
                declaration.kind,
                declaration.module.0,
                declaration.externally_reachable
            ));
        }
        for import in &self.imports {
            output.push_str(&format!(
                "import {} {} module={} {:?} target={:?}\n",
                import.id.0,
                self.symbol_text(import.name),
                import.module.0,
                import.visibility,
                import.target
            ));
        }
        output
    }
}
