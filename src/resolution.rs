//! Declaration collection, imports, visibility, and lexical name resolution.
//!
//! This is the owned identity database for `IMPL.md` Milestone 4. It consumes
//! the package graph and Milestone 3 syntax trees, predeclares every module
//! item, then resolves imports and bodies without depending on source order.

use lasso::{Rodeo, Spur};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::diagnostics::{Category, Diagnostic};
use crate::ident::is_valid_identifier;
use crate::lexer::{Keyword, Token, TokenKind, lex};
use crate::package::{PackageGraph, PackageId};
use crate::parser::{SyntaxElement, SyntaxKind, SyntaxNode, parse};
use crate::source::{FileId, SourceManager, Span};

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u32);

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
pub struct Symbol(Spur);

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
enum NamespaceTarget {
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
    target: NamespaceTarget,
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
    namespace: BTreeMap<Symbol, NamespaceEntry>,
}

/// The declaration families available before type checking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclarationKind {
    TypeAlias,
    Struct,
    Enum,
    Trait,
    Function,
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
    Local,
    Loop,
    PatternCandidate,
}

/// One import or public re-export.
#[derive(Debug)]
pub struct Import {
    pub id: ImportId,
    pub module: ModuleId,
    pub name: Symbol,
    pub visibility: Visibility,
    pub span: Span,
    path: Vec<PathPart>,
    pub target: Option<ItemId>,
    pub provenance: Vec<Span>,
    state: ImportState,
    replaced_file_module: Option<ModuleId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportState {
    Unresolved,
    Resolving,
    Resolved,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathPart {
    Root,
    SelfModule,
    Super,
    Name(Symbol),
}

#[derive(Debug)]
struct Builtin {
    name: Symbol,
}

/// Milestone 4 output. A partially resolved database is returned with
/// diagnostics so later tooling can inspect independent declarations.
pub struct ResolutionOutput {
    pub program: ResolvedProgram,
    pub diagnostics: Vec<Diagnostic>,
}

/// All identities and resolution results for one package graph.
pub struct ResolvedProgram {
    symbols: Rodeo<Spur>,
    pub modules: Vec<Module>,
    pub declarations: Vec<Declaration>,
    pub imports: Vec<Import>,
    pub impls: Vec<ImplBlock>,
    pub fields: Vec<Field>,
    pub variants: Vec<Variant>,
    pub generic_parameters: Vec<GenericParameter>,
    pub local_bindings: Vec<LocalBinding>,
    pub references: Vec<ResolvedReference>,
    pub declaration_members: BTreeMap<DeclarationId, BTreeMap<Symbol, MemberId>>,
    pub impl_members: BTreeMap<ImplId, BTreeMap<Symbol, DeclarationId>>,
    builtins: Vec<Builtin>,
    prelude: BTreeMap<Symbol, ItemId>,
    standard_declarations: BTreeMap<Symbol, DeclarationId>,
    package_roots: BTreeMap<PackageId, ModuleId>,
    module_keys: BTreeMap<(PackageId, Vec<String>), ModuleId>,
    std_root: ModuleId,
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
    /// (`enum Option[T]`, `SPEC.md` 4.4) are collected from compiler-supplied
    /// source into the `std` root module, so they carry real declaration,
    /// variant, field, and generic-parameter identities and travel the same
    /// generic-enum path as user code. A pass needs this lookup only where the
    /// specification gives that exact type extra behavior, such as `Option[T]`
    /// defaulting to `Option.None` without a `T: Default` obligation
    /// (`SPEC.md` 4.3).
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

#[derive(Debug)]
struct ParsedUnit {
    module: ModuleId,
    tree: SyntaxNode,
}

#[derive(Debug, Clone)]
struct LookupResult {
    item: ItemId,
    provenance: Vec<Span>,
}

#[derive(Default)]
struct LexicalScopes {
    scopes: Vec<BTreeMap<Symbol, LocalBindingId>>,
}

impl LexicalScopes {
    fn push(&mut self) {
        self.scopes.push(BTreeMap::new());
    }

    fn pop(&mut self) {
        self.scopes.pop().expect("lexical scope stack is balanced");
    }

    fn lookup(&self, name: Symbol) -> Option<LocalBindingId> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&name).copied())
    }
}

/// Loads, lexes, parses, collects, and resolves every package source file.
#[must_use]
pub fn resolve(graph: &PackageGraph, sources: &mut SourceManager) -> ResolutionOutput {
    Resolver::new(graph, sources).run()
}

struct Resolver<'a> {
    graph: &'a PackageGraph,
    sources: &'a mut SourceManager,
    program: ResolvedProgram,
    diagnostics: Vec<Diagnostic>,
    parsed_units: Vec<ParsedUnit>,
    inline_units: Vec<ParsedUnit>,
}

impl<'a> Resolver<'a> {
    fn new(graph: &'a PackageGraph, sources: &'a mut SourceManager) -> Self {
        let mut symbols = Rodeo::default();
        let std_symbol = Symbol(symbols.get_or_intern("std"));
        let std_root = ModuleId(0);
        let program = ResolvedProgram {
            symbols,
            modules: vec![Module {
                id: std_root,
                package: None,
                path: vec![std_symbol],
                parent: None,
                origin: ModuleOrigin::Standard,
                source_file: None,
                span: None,
                externally_reachable: true,
                namespace: BTreeMap::new(),
            }],
            declarations: Vec::new(),
            imports: Vec::new(),
            impls: Vec::new(),
            fields: Vec::new(),
            variants: Vec::new(),
            generic_parameters: Vec::new(),
            local_bindings: Vec::new(),
            references: Vec::new(),
            declaration_members: BTreeMap::new(),
            impl_members: BTreeMap::new(),
            builtins: Vec::new(),
            prelude: BTreeMap::new(),
            standard_declarations: BTreeMap::new(),
            package_roots: BTreeMap::new(),
            module_keys: BTreeMap::new(),
            std_root,
        };
        Self {
            graph,
            sources,
            program,
            diagnostics: Vec::new(),
            parsed_units: Vec::new(),
            inline_units: Vec::new(),
        }
    }

    fn run(mut self) -> ResolutionOutput {
        let (io_module, ffi_module) = self.install_standard_library_names();
        self.parse_standard_library_source(io_module, ffi_module);
        self.create_file_module_graph();
        self.parse_source_files();
        self.discover_inline_modules();
        self.collect_all_declarations();
        self.check_exported_c_symbol_conflicts();
        self.register_standard_declarations();
        self.resolve_all_imports();
        self.resolve_all_declaration_contents();
        self.compute_external_reachability();
        self.check_public_signatures();
        ResolutionOutput {
            program: self.program,
            diagnostics: self.diagnostics,
        }
    }

    fn intern(&mut self, text: &str) -> Symbol {
        Symbol(self.program.symbols.get_or_intern(text))
    }

    fn push_module(
        &mut self,
        package: Option<PackageId>,
        path: Vec<Symbol>,
        parent: Option<ModuleId>,
        origin: ModuleOrigin,
    ) -> ModuleId {
        let id = ModuleId(self.program.modules.len() as u32);
        self.program.modules.push(Module {
            id,
            package,
            path,
            parent,
            origin,
            source_file: None,
            span: None,
            externally_reachable: false,
            namespace: BTreeMap::new(),
        });
        id
    }

    fn install_standard_library_names(&mut self) -> (ModuleId, ModuleId) {
        let io = self.intern("io");
        let ffi = self.intern("ffi");
        let std = self.intern("std");
        let io_module = self.push_module(
            None,
            vec![std, io],
            Some(self.program.std_root),
            ModuleOrigin::Standard,
        );
        let ffi_module = self.push_module(
            None,
            vec![std, ffi],
            Some(self.program.std_root),
            ModuleOrigin::Standard,
        );
        self.insert_namespace(
            self.program.std_root,
            io,
            NamespaceTarget::Item(ItemId::Module(io_module)),
            Visibility::Public,
            None,
        );
        self.insert_namespace(
            self.program.std_root,
            ffi,
            NamespaceTarget::Item(ItemId::Module(ffi_module)),
            Visibility::Public,
            None,
        );

        for name in [
            "bool",
            "char",
            "i8",
            "i16",
            "i32",
            "i64",
            "i128",
            "isize",
            "u8",
            "u16",
            "u32",
            "u64",
            "u128",
            "usize",
            "f32",
            "f64",
            "str",
            "String",
            "Vec",
            "Map",
            "Set",
            "Default",
            "PartialEq",
            "Eq",
            "PartialOrd",
            "Ord",
            "Hash",
            "StableHash",
            "Formatter",
            "Identity",
            "print",
            "println",
        ] {
            let symbol = self.intern(name);
            let id = self.push_builtin(symbol);
            self.program.prelude.insert(symbol, ItemId::Builtin(id));
            if name == "Formatter" {
                self.insert_namespace(
                    self.program.std_root,
                    symbol,
                    NamespaceTarget::Item(ItemId::Builtin(id)),
                    Visibility::Public,
                    None,
                );
            }
        }
        for (module, names) in [
            (io_module, &["print", "println"][..]),
            (ffi_module, &["ForeignRoot", "ForeignRootMut", "CVoid"][..]),
        ] {
            for name in names {
                let symbol = self.intern(name);
                let id = self
                    .program
                    .builtins
                    .iter()
                    .position(|builtin| builtin.name == symbol)
                    .map_or_else(
                        || self.push_builtin(symbol),
                        |index| BuiltinId(index as u32),
                    );
                self.insert_namespace(
                    module,
                    symbol,
                    NamespaceTarget::Item(ItemId::Builtin(id)),
                    Visibility::Public,
                    None,
                );
            }
        }
        (io_module, ffi_module)
    }

    fn push_builtin(&mut self, name: Symbol) -> BuiltinId {
        let id = BuiltinId(self.program.builtins.len() as u32);
        self.program.builtins.push(Builtin { name });
        id
    }

    /// Lexes and parses the compiler-supplied source of the standard types the
    /// specification writes as ordinary declarations, into the `std` root
    /// module. These are not a `std` *package* — Milestone 18 owns that — but
    /// they are ordinary declarations from here on, so every later pass sees
    /// `Option[T]` as the generic enum `SPEC.md` 4.4 declares it to be.
    ///
    /// This source is compiler input, not user input: a diagnostic in it is an
    /// internal defect, so it is reported through the ordinary diagnostic list
    /// rather than silently dropped.
    fn parse_standard_library_source(&mut self, io_module: ModuleId, ffi_module: ModuleId) {
        let root = self.program.std_root;
        for (module, path, source) in [
            (root, "<std>/lib.elx", crate::standard::ROOT_SOURCE),
            (io_module, "<std>/io.elx", crate::standard::IO_SOURCE),
            (ffi_module, "<std>/ffi.elx", crate::standard::FFI_SOURCE),
        ] {
            let file = self
                .sources
                .add_text(std::path::PathBuf::from(path), source.to_string());
            self.program.modules[module.index()].source_file = Some(file);
            self.program.modules[module.index()].span = Some(Span::new(
                file,
                0,
                u32::try_from(self.sources.text(file).len()).unwrap_or(u32::MAX),
            ));
            let lexed = lex(file, self.sources.text(file));
            self.diagnostics.extend(lexed.diagnostics);
            let parsed = parse(&lexed.tokens);
            self.diagnostics.extend(parsed.diagnostics);
            self.parsed_units.push(ParsedUnit {
                module,
                tree: parsed.tree,
            });
        }
    }

    /// Records the declarations collected from [`crate::standard::ROOT_SOURCE`] as
    /// both the compiler-known standard identities and prelude names, so an
    /// unqualified `Option` resolves after lexical, module, import, and
    /// dependency-alias lookup exactly like the builtin names beside it.
    fn register_standard_declarations(&mut self) {
        let entries = self.program.modules[self.program.std_root.index()]
            .namespace
            .iter()
            .filter_map(|(name, entry)| match entry.target {
                NamespaceTarget::Item(ItemId::Declaration(declaration)) => {
                    Some((*name, declaration))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        for (name, declaration) in entries {
            self.program.standard_declarations.insert(name, declaration);
            self.program
                .prelude
                .insert(name, ItemId::Declaration(declaration));
        }
    }

    fn create_file_module_graph(&mut self) {
        for (package_id, package) in &self.graph.packages {
            let root = self.push_module(
                Some(package_id.clone()),
                Vec::new(),
                None,
                ModuleOrigin::RootFile,
            );
            self.program.package_roots.insert(package_id.clone(), root);
            self.program
                .module_keys
                .insert((package_id.clone(), Vec::new()), root);

            for module_path in package.modules.keys() {
                let mut path = Vec::<String>::new();
                for component in module_path.components() {
                    path.push(component.clone());
                    if self
                        .program
                        .module_keys
                        .contains_key(&(package_id.clone(), path.clone()))
                    {
                        continue;
                    }
                    let parent_path = &path[..path.len() - 1];
                    let parent =
                        self.program.module_keys[&(package_id.clone(), parent_path.to_vec())];
                    let symbols = path
                        .iter()
                        .map(|part| self.intern(part))
                        .collect::<Vec<_>>();
                    let origin = if path.len() == module_path.components().len() {
                        ModuleOrigin::FileBacked
                    } else {
                        ModuleOrigin::DirectoryNamespace
                    };
                    let module =
                        self.push_module(Some(package_id.clone()), symbols, Some(parent), origin);
                    self.program
                        .module_keys
                        .insert((package_id.clone(), path.clone()), module);
                    let name = *self.program.modules[module.index()]
                        .path
                        .last()
                        .expect("non-root module has a name");
                    self.insert_namespace(
                        parent,
                        name,
                        NamespaceTarget::Item(ItemId::Module(module)),
                        Visibility::Package,
                        None,
                    );
                }
            }
        }
    }

    fn parse_source_files(&mut self) {
        let mut files = Vec::<(ModuleId, PathBuf)>::new();
        for (package_id, package) in &self.graph.packages {
            let root = self.program.package_roots[package_id];
            files.push((root, package.manifest_dir.join(&package.manifest.root)));
            for (path, file) in &package.modules {
                let module =
                    self.program.module_keys[&(package_id.clone(), path.components().to_vec())];
                files.push((module, file.clone()));
            }
        }
        files.sort_by(|left, right| left.1.cmp(&right.1));

        for (module, path) in files {
            let file = match self.sources.load_file(&path) {
                Ok(file) => file,
                Err(error) => {
                    self.diagnostics
                        .push(Diagnostic::new(Category::NameResolution, error.to_string()));
                    continue;
                }
            };
            self.program.modules[module.index()].source_file = Some(file);
            self.program.modules[module.index()].span = Some(Span::new(
                file,
                0,
                u32::try_from(self.sources.text(file).len()).unwrap_or(u32::MAX),
            ));
            let lexed = lex(file, self.sources.text(file));
            self.diagnostics.extend(lexed.diagnostics);
            let parsed = parse(&lexed.tokens);
            self.diagnostics.extend(parsed.diagnostics);
            self.parsed_units.push(ParsedUnit {
                module,
                tree: parsed.tree,
            });
        }
    }

    fn discover_inline_modules(&mut self) {
        let units = self
            .parsed_units
            .iter()
            .map(|unit| (unit.module, unit.tree.clone()))
            .collect::<Vec<_>>();
        for (module, tree) in units {
            self.discover_inline_children(module, &tree);
        }
    }

    fn discover_inline_children(&mut self, parent: ModuleId, container: &SyntaxNode) {
        for item in direct_item_nodes(container) {
            if item.kind != SyntaxKind::Module {
                continue;
            }
            let Some((name_text, span)) = declaration_name(item, Keyword::Mod) else {
                continue;
            };
            let name = self.intern(&name_text);
            let package = self.program.modules[parent.index()]
                .package
                .clone()
                .expect("inline modules exist only in packages");
            let mut path_text = self.program.modules[parent.index()]
                .path
                .iter()
                .map(|symbol| self.program.symbol_text(*symbol).to_string())
                .collect::<Vec<_>>();
            path_text.push(name_text);
            if let Some(existing) = self
                .program
                .module_keys
                .get(&(package.clone(), path_text.clone()))
                .copied()
            {
                let existing_module = &self.program.modules[existing.index()];
                let message = if existing_module.origin == ModuleOrigin::Inline {
                    format!(
                        "inline module `{}` is declared more than once",
                        path_text.join(".")
                    )
                } else {
                    format!(
                        "inline module `{}` collides with a file-backed module path",
                        path_text.join(".")
                    )
                };
                self.diagnostics.push(
                    Diagnostic::new(Category::DeclarationConflict, message).with_primary(span),
                );
                continue;
            }

            let mut path = self.program.modules[parent.index()].path.clone();
            path.push(name);
            let module = self.push_module(
                Some(package.clone()),
                path,
                Some(parent),
                ModuleOrigin::Inline,
            );
            self.program.modules[module.index()].span = Some(item.span);
            self.program
                .module_keys
                .insert((package, path_text), module);
            let visibility = node_visibility(item);
            self.insert_namespace(
                parent,
                name,
                NamespaceTarget::Item(ItemId::Module(module)),
                visibility,
                Some(span),
            );
            self.inline_units.push(ParsedUnit {
                module,
                tree: item.clone(),
            });
            self.discover_inline_children(module, item);
        }
    }

    fn collect_all_declarations(&mut self) {
        let units = self
            .parsed_units
            .iter()
            .chain(&self.inline_units)
            .map(|unit| (unit.module, unit.tree.clone()))
            .collect::<Vec<_>>();
        for (module, tree) in units {
            self.collect_container(module, &tree);
        }
    }

    fn check_exported_c_symbol_conflicts(&mut self) {
        let exports = self
            .program
            .declarations
            .iter()
            .filter_map(|declaration| {
                declaration
                    .foreign_binding
                    .as_ref()
                    .filter(|binding| binding.direction == ForeignDirection::Export)
                    .map(|binding| (binding.c_name.clone(), declaration.span))
            })
            .collect::<Vec<_>>();
        let mut first_export = BTreeMap::new();
        for (symbol, span) in exports {
            if let Some(previous) = first_export.insert(symbol.clone(), span) {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::DeclarationConflict,
                        format!("C symbol `{symbol}` is exported more than once"),
                    )
                    .with_primary(span)
                    .with_related(previous, "first export is here"),
                );
            }
        }
    }

    fn collect_container(&mut self, module: ModuleId, container: &SyntaxNode) {
        for item in direct_item_nodes(container) {
            match item.kind {
                SyntaxKind::Module | SyntaxKind::PassStatement | SyntaxKind::Error => {}
                SyntaxKind::Import => self.collect_import(module, item),
                SyntaxKind::Impl => self.collect_impl(module, item),
                SyntaxKind::TypeAlias
                | SyntaxKind::Struct
                | SyntaxKind::Enum
                | SyntaxKind::Trait
                | SyntaxKind::Function
                | SyntaxKind::ForeignType => {
                    self.collect_named_declaration(module, item, false, None, None);
                }
                _ => {}
            }
        }
    }

    fn collect_import(&mut self, module: ModuleId, node: &SyntaxNode) {
        let Some((path, bound_name, bound_span)) = import_path(node, &mut self.program.symbols)
        else {
            return;
        };
        let id = ImportId(self.program.imports.len() as u32);
        let visibility = node_visibility(node);
        let replaced_file_module = self.program.modules[module.index()]
            .namespace
            .get(&bound_name)
            .and_then(|entry| match entry.target {
                NamespaceTarget::Item(ItemId::Module(existing))
                    if visibility == Visibility::Public
                        && matches!(
                            self.program.modules[existing.index()].origin,
                            ModuleOrigin::FileBacked | ModuleOrigin::DirectoryNamespace
                        ) =>
                {
                    Some(existing)
                }
                _ => None,
            });
        self.program.imports.push(Import {
            id,
            module,
            name: bound_name,
            visibility,
            span: node.span,
            path,
            target: None,
            provenance: Vec::new(),
            state: ImportState::Unresolved,
            replaced_file_module,
        });
        if replaced_file_module.is_some() {
            self.program.modules[module.index()].namespace.insert(
                bound_name,
                NamespaceEntry {
                    name: bound_name,
                    target: NamespaceTarget::Import(id),
                    visibility,
                    span: Some(bound_span),
                },
            );
        } else {
            self.insert_namespace(
                module,
                bound_name,
                NamespaceTarget::Import(id),
                visibility,
                Some(bound_span),
            );
        }
    }

    fn collect_named_declaration(
        &mut self,
        module: ModuleId,
        node: &SyntaxNode,
        foreign: bool,
        parent_declaration: Option<DeclarationId>,
        parent_impl: Option<ImplId>,
    ) -> Option<DeclarationId> {
        let foreign_binding = self.foreign_binding(node, parent_declaration, parent_impl);
        let imported = foreign_binding
            .as_ref()
            .is_some_and(|binding| binding.direction == ForeignDirection::Import);
        let keyword = match node.kind {
            SyntaxKind::TypeAlias | SyntaxKind::ForeignType => Keyword::Type,
            SyntaxKind::Struct => Keyword::Struct,
            SyntaxKind::Enum => Keyword::Enum,
            SyntaxKind::Trait => Keyword::Trait,
            SyntaxKind::Function => Keyword::Fn,
            _ => return None,
        };
        let (name_text, span) = declaration_name(node, keyword)?;
        let name = self.intern(&name_text);
        let foreign = foreign || imported;
        let kind = match (node.kind, foreign) {
            (SyntaxKind::TypeAlias, _) => DeclarationKind::TypeAlias,
            (SyntaxKind::ForeignType, _) => DeclarationKind::ForeignType,
            (SyntaxKind::Struct, true) => DeclarationKind::ForeignStruct,
            (SyntaxKind::Struct, false) => DeclarationKind::Struct,
            (SyntaxKind::Enum, _) => DeclarationKind::Enum,
            (SyntaxKind::Trait, _) => DeclarationKind::Trait,
            (SyntaxKind::Function, true) => DeclarationKind::ForeignFunction,
            (SyntaxKind::Function, false) => DeclarationKind::Function,
            _ => return None,
        };
        let id = DeclarationId(self.program.declarations.len() as u32);
        self.program.declarations.push(Declaration {
            id,
            module,
            name,
            kind,
            visibility: node_visibility(node),
            span,
            syntax: node.clone(),
            parent_declaration,
            parent_impl,
            generic_parameters: Vec::new(),
            externally_reachable: false,
            foreign_binding,
        });
        self.program.declaration_members.entry(id).or_default();
        if parent_declaration.is_none() && parent_impl.is_none() {
            self.insert_namespace(
                module,
                name,
                NamespaceTarget::Item(ItemId::Declaration(id)),
                node_visibility(node),
                Some(span),
            );
        }
        self.collect_generic_parameters(id, node);
        match kind {
            DeclarationKind::Struct | DeclarationKind::ForeignStruct => {
                self.collect_struct_members(id, kind == DeclarationKind::ForeignStruct)
            }
            DeclarationKind::Enum => self.collect_enum_variants(id),
            DeclarationKind::Trait => self.collect_trait_methods(id),
            _ => {}
        }
        Some(id)
    }

    fn foreign_binding(
        &mut self,
        node: &SyntaxNode,
        parent_declaration: Option<DeclarationId>,
        parent_impl: Option<ImplId>,
    ) -> Option<ForeignBinding> {
        let attributes = direct_children_of_kind(node, SyntaxKind::Attribute);
        let mut binding = None;
        for attribute in attributes {
            let tokens = attribute
                .children
                .iter()
                .filter_map(|child| match child {
                    SyntaxElement::Token(token) => Some(token),
                    SyntaxElement::Node(_) => None,
                })
                .collect::<Vec<_>>();
            let name = tokens.iter().find_map(|token| match &token.kind {
                TokenKind::Identifier(name) => Some(name.as_str()),
                _ => None,
            });
            let arguments = tokens
                .iter()
                .filter_map(|token| match &token.kind {
                    TokenKind::StringLiteral(value) => Some(value.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let (direction, expected) = match name {
                Some("importc") => (ForeignDirection::Import, 2usize),
                Some("exportc") => (ForeignDirection::Export, 1usize),
                Some(other) => {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::DeclarationConflict,
                            format!("unknown compiler attribute `@{other}`"),
                        )
                        .with_primary(attribute.span),
                    );
                    continue;
                }
                None => continue,
            };
            if binding.is_some() {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::DeclarationConflict,
                        "a declaration can have only one FFI attribute",
                    )
                    .with_primary(attribute.span),
                );
                continue;
            }
            if arguments.len() != expected {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::DeclarationConflict,
                        if direction == ForeignDirection::Import {
                            "`@importc` requires a C name and header"
                        } else {
                            "`@exportc` requires one C symbol name"
                        },
                    )
                    .with_primary(attribute.span),
                );
                continue;
            }
            if parent_declaration.is_some() || parent_impl.is_some() {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::DeclarationConflict,
                        "FFI attributes are permitted only on module-level declarations",
                    )
                    .with_primary(attribute.span),
                );
                continue;
            }
            let item_allowed = match direction {
                ForeignDirection::Import => matches!(
                    node.kind,
                    SyntaxKind::ForeignType | SyntaxKind::Struct | SyntaxKind::Function
                ),
                ForeignDirection::Export => node.kind == SyntaxKind::Function,
            };
            if !item_allowed {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::DeclarationConflict,
                        match direction {
                            ForeignDirection::Import => {
                                "`@importc` applies only to a type, struct, or bodyless function"
                            }
                            ForeignDirection::Export => {
                                "`@exportc` applies only to a function definition"
                            }
                        },
                    )
                    .with_primary(attribute.span),
                );
                continue;
            }
            let c_name = &arguments[0];
            let valid_c_name = if node.kind == SyntaxKind::Function {
                is_valid_identifier(c_name)
            } else {
                is_valid_identifier(c_name)
                    || c_name
                        .strip_prefix("struct ")
                        .is_some_and(is_valid_identifier)
            };
            if !valid_c_name {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::DeclarationConflict,
                        "the imported C spelling is not a supported identifier or struct tag",
                    )
                    .with_primary(attribute.span),
                );
                continue;
            }
            let header = (direction == ForeignDirection::Import).then(|| arguments[1].clone());
            if header.as_ref().is_some_and(|header| {
                header.is_empty()
                    || !header.chars().all(|character| {
                        character.is_ascii_alphanumeric()
                            || matches!(character, '_' | '-' | '.' | '/')
                    })
            }) {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::DeclarationConflict,
                        "a C header name may contain only ASCII path characters",
                    )
                    .with_primary(attribute.span),
                );
                continue;
            }
            binding = Some(ForeignBinding {
                direction,
                c_name: c_name.clone(),
                header,
            });
        }
        binding
    }

    fn collect_generic_parameters(&mut self, declaration: DeclarationId, node: &SyntaxNode) {
        let generic_nodes = generic_parameter_nodes(node);
        let mut seen = BTreeMap::new();
        for generic in generic_nodes {
            let Some(token) = first_identifier(generic) else {
                continue;
            };
            let name = self.intern(token_text(token));
            let id = GenericParameterId(self.program.generic_parameters.len() as u32);
            if let Some(previous) = seen.insert(name, token.span) {
                self.duplicate_diagnostic("generic parameter", token.span, Some(previous));
            }
            self.program.generic_parameters.push(GenericParameter {
                id,
                name,
                span: token.span,
            });
            self.program.declarations[declaration.index()]
                .generic_parameters
                .push(id);
        }
    }

    fn collect_struct_members(&mut self, parent: DeclarationId, foreign: bool) {
        let syntax = self.program.declarations[parent.index()].syntax.clone();
        let module = self.program.declarations[parent.index()].module;
        let mut namespace = BTreeMap::<Symbol, Span>::new();
        let mut saw_method = false;
        for member in direct_block_nodes(&syntax) {
            match member.kind {
                SyntaxKind::Field => {
                    if saw_method && !foreign {
                        self.diagnostics.push(
                            Diagnostic::new(
                                Category::DeclarationConflict,
                                "struct fields must be declared before methods",
                            )
                            .with_primary(member.span),
                        );
                    }
                    if let Some(token) = first_identifier(member) {
                        let name = self.intern(token_text(token));
                        if let Some(previous) = namespace.insert(name, token.span) {
                            self.duplicate_diagnostic("struct member", token.span, Some(previous));
                        }
                        let id = FieldId(self.program.fields.len() as u32);
                        self.program.fields.push(Field {
                            id,
                            name,
                            visibility: node_visibility(member),
                            span: token.span,
                            parent_declaration: parent,
                            parent_variant: None,
                            syntax: member.clone(),
                        });
                        self.program
                            .declaration_members
                            .entry(parent)
                            .or_default()
                            .entry(name)
                            .or_insert(MemberId::Field(id));
                    }
                }
                SyntaxKind::Function => {
                    saw_method = true;
                    if foreign {
                        self.diagnostics.push(
                            Diagnostic::new(
                                Category::DeclarationConflict,
                                "an imported C struct cannot contain methods",
                            )
                            .with_primary(member.span),
                        );
                        continue;
                    }
                    if let Some(id) =
                        self.collect_named_declaration(module, member, false, Some(parent), None)
                    {
                        let name = self.program.declarations[id.index()].name;
                        let span = self.program.declarations[id.index()].span;
                        if let Some(previous) = namespace.insert(name, span) {
                            self.duplicate_diagnostic("struct member", span, Some(previous));
                        }
                        self.program
                            .declaration_members
                            .entry(parent)
                            .or_default()
                            .entry(name)
                            .or_insert(MemberId::Method(id));
                    }
                }
                _ => {}
            }
        }
    }

    fn collect_enum_variants(&mut self, parent: DeclarationId) {
        let syntax = self.program.declarations[parent.index()].syntax.clone();
        let mut seen = BTreeMap::<Symbol, Span>::new();
        for variant_node in direct_block_nodes(&syntax)
            .into_iter()
            .filter(|node| node.kind == SyntaxKind::EnumVariant)
        {
            let Some(token) = first_identifier(variant_node) else {
                continue;
            };
            let name = self.intern(token_text(token));
            if let Some(previous) = seen.insert(name, token.span) {
                self.duplicate_diagnostic("enum variant", token.span, Some(previous));
            }
            let variant = VariantId(self.program.variants.len() as u32);
            self.program.variants.push(Variant {
                id: variant,
                name,
                span: token.span,
                parent,
                syntax: variant_node.clone(),
                fields: Vec::new(),
            });
            self.program
                .declaration_members
                .entry(parent)
                .or_default()
                .entry(name)
                .or_insert(MemberId::Variant(variant));
            let mut field_names = BTreeMap::<Symbol, Span>::new();
            for field_node in child_nodes_of_kind(variant_node, SyntaxKind::Field) {
                let Some(field_token) = first_identifier(field_node) else {
                    continue;
                };
                let field_name = self.intern(token_text(field_token));
                if let Some(previous) = field_names.insert(field_name, field_token.span) {
                    self.duplicate_diagnostic("variant field", field_token.span, Some(previous));
                }
                let field = FieldId(self.program.fields.len() as u32);
                self.program.fields.push(Field {
                    id: field,
                    name: field_name,
                    visibility: self.program.declarations[parent.index()].visibility,
                    span: field_token.span,
                    parent_declaration: parent,
                    parent_variant: Some(variant),
                    syntax: field_node.clone(),
                });
                self.program.variants[variant.index()].fields.push(field);
            }
            let tuple_types = child_nodes(variant_node)
                .into_iter()
                .filter(|child| child.kind == SyntaxKind::Type)
                .cloned()
                .collect::<Vec<_>>();
            for (position, ty) in tuple_types.into_iter().enumerate() {
                let name = self.intern(&position.to_string());
                let field = FieldId(self.program.fields.len() as u32);
                self.program.fields.push(Field {
                    id: field,
                    name,
                    visibility: self.program.declarations[parent.index()].visibility,
                    span: ty.span,
                    parent_declaration: parent,
                    parent_variant: Some(variant),
                    syntax: ty,
                });
                self.program.variants[variant.index()].fields.push(field);
            }
        }
    }

    fn collect_trait_methods(&mut self, parent: DeclarationId) {
        let syntax = self.program.declarations[parent.index()].syntax.clone();
        let module = self.program.declarations[parent.index()].module;
        let mut seen = BTreeMap::<Symbol, Span>::new();
        for method in direct_block_nodes(&syntax)
            .into_iter()
            .filter(|node| node.kind == SyntaxKind::Function)
        {
            if has_keyword(method, Keyword::Pub) {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::DeclarationConflict,
                        "trait methods cannot carry a separate `pub` modifier",
                    )
                    .with_primary(method.span),
                );
            }
            if let Some(id) =
                self.collect_named_declaration(module, method, false, Some(parent), None)
            {
                let name = self.program.declarations[id.index()].name;
                let span = self.program.declarations[id.index()].span;
                if let Some(previous) = seen.insert(name, span) {
                    self.duplicate_diagnostic("trait method", span, Some(previous));
                }
                self.program
                    .declaration_members
                    .entry(parent)
                    .or_default()
                    .entry(name)
                    .or_insert(MemberId::Method(id));
            }
        }
    }

    fn collect_impl(&mut self, module: ModuleId, node: &SyntaxNode) {
        let id = ImplId(self.program.impls.len() as u32);
        self.program.impls.push(ImplBlock {
            id,
            module,
            span: node.span,
            syntax: node.clone(),
            generic_parameters: Vec::new(),
            methods: Vec::new(),
        });
        self.program.impl_members.entry(id).or_default();
        let generic_nodes = generic_parameter_nodes(node);
        let mut seen = BTreeMap::new();
        for generic in generic_nodes {
            let Some(token) = first_identifier(generic) else {
                continue;
            };
            let name = self.intern(token_text(token));
            let parameter = GenericParameterId(self.program.generic_parameters.len() as u32);
            if let Some(previous) = seen.insert(name, token.span) {
                self.duplicate_diagnostic("generic parameter", token.span, Some(previous));
            }
            self.program.generic_parameters.push(GenericParameter {
                id: parameter,
                name,
                span: token.span,
            });
            self.program.impls[id.index()]
                .generic_parameters
                .push(parameter);
        }
        let methods = direct_block_nodes(node)
            .into_iter()
            .filter(|child| child.kind == SyntaxKind::Function)
            .cloned()
            .collect::<Vec<_>>();
        let mut seen_methods = BTreeMap::new();
        for method in methods {
            if let Some(declaration) =
                self.collect_named_declaration(module, &method, false, None, Some(id))
            {
                let name = self.program.declarations[declaration.index()].name;
                let span = self.program.declarations[declaration.index()].span;
                if let Some(previous) = seen_methods.insert(name, span) {
                    self.duplicate_diagnostic("implementation method", span, Some(previous));
                }
                self.program.impls[id.index()].methods.push(declaration);
                self.program
                    .impl_members
                    .entry(id)
                    .or_default()
                    .entry(name)
                    .or_insert(declaration);
            }
        }
    }

    fn insert_namespace(
        &mut self,
        module: ModuleId,
        name: Symbol,
        target: NamespaceTarget,
        visibility: Visibility,
        span: Option<Span>,
    ) {
        if let Some(previous) = self.program.modules[module.index()].namespace.get(&name) {
            if let Some(span) = span {
                self.duplicate_diagnostic("module item", span, previous.span);
            } else {
                self.diagnostics.push(Diagnostic::new(
                    Category::DeclarationConflict,
                    format!(
                        "module item `{}` is defined more than once",
                        self.program.symbol_text(name)
                    ),
                ));
            }
            return;
        }
        self.program.modules[module.index()].namespace.insert(
            name,
            NamespaceEntry {
                name,
                target,
                visibility,
                span,
            },
        );
    }

    fn duplicate_diagnostic(&mut self, what: &str, span: Span, previous: Option<Span>) {
        let mut diagnostic = Diagnostic::new(
            Category::DeclarationConflict,
            format!("{what} is defined more than once"),
        )
        .with_primary(span);
        if let Some(previous) = previous {
            diagnostic = diagnostic.with_related(previous, "first definition is here");
        }
        self.diagnostics.push(diagnostic);
    }

    fn resolve_all_imports(&mut self) {
        for index in 0..self.program.imports.len() {
            self.resolve_import(ImportId(index as u32));
        }
    }

    fn resolve_import(&mut self, import: ImportId) -> Option<LookupResult> {
        match self.program.imports[import.index()].state {
            ImportState::Resolved => {
                return self.program.imports[import.index()]
                    .target
                    .map(|item| LookupResult {
                        item,
                        provenance: self.program.imports[import.index()].provenance.clone(),
                    });
            }
            ImportState::Failed => return None,
            ImportState::Resolving => {
                let span = self.program.imports[import.index()].span;
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::NameResolution,
                        "import aliases form a cycle without reaching a declaration",
                    )
                    .with_primary(span),
                );
                self.program.imports[import.index()].state = ImportState::Failed;
                return None;
            }
            ImportState::Unresolved => {}
        }

        self.program.imports[import.index()].state = ImportState::Resolving;
        let module = self.program.imports[import.index()].module;
        let span = self.program.imports[import.index()].span;
        let path = self.program.imports[import.index()].path.clone();
        let visibility = self.program.imports[import.index()].visibility;
        let replaced_file_module = self.program.imports[import.index()].replaced_file_module;
        let result = self.resolve_module_path(module, &path, span);
        match result {
            Some(mut result) => {
                if replaced_file_module.is_some()
                    && result.item != ItemId::Module(replaced_file_module.expect("checked above"))
                {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::DeclarationConflict,
                            "a public import cannot replace an existing file-backed module with a different target",
                        )
                        .with_primary(span),
                    );
                }
                if visibility == Visibility::Public && !self.item_can_be_reexported(result.item) {
                    let mut diagnostic = Diagnostic::new(
                        Category::Visibility,
                        "a public import cannot re-export a package-private declaration",
                    )
                    .with_primary(span);
                    if let ItemId::Declaration(declaration) = result.item {
                        diagnostic = diagnostic.with_related(
                            self.program.declarations[declaration.index()].span,
                            "package-private declaration is here",
                        );
                    }
                    self.diagnostics.push(diagnostic);
                }
                result.provenance.insert(0, span);
                self.program.imports[import.index()].target = Some(result.item);
                self.program.imports[import.index()].provenance = result.provenance.clone();
                self.program.imports[import.index()].state = ImportState::Resolved;
                Some(result)
            }
            None => {
                self.program.imports[import.index()].state = ImportState::Failed;
                None
            }
        }
    }

    fn resolve_module_path(
        &mut self,
        from_module: ModuleId,
        parts: &[PathPart],
        use_span: Span,
    ) -> Option<LookupResult> {
        let first = parts.first().copied()?;
        let from_package = self.program.modules[from_module.index()].package.clone();
        let (mut result, mut index, mut external) = match first {
            PathPart::Root => {
                let package = from_package.as_ref()?;
                (
                    LookupResult {
                        item: ItemId::Module(self.program.package_roots[package]),
                        provenance: Vec::new(),
                    },
                    1,
                    false,
                )
            }
            PathPart::SelfModule => (
                LookupResult {
                    item: ItemId::Module(from_module),
                    provenance: Vec::new(),
                },
                1,
                false,
            ),
            PathPart::Super => {
                let Some(parent) = self.program.modules[from_module.index()].parent else {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::NameResolution,
                            "`super` cannot be used in a package root module",
                        )
                        .with_primary(use_span),
                    );
                    return None;
                };
                (
                    LookupResult {
                        item: ItemId::Module(parent),
                        provenance: Vec::new(),
                    },
                    1,
                    false,
                )
            }
            PathPart::Name(name) => {
                if let Some(package) = &from_package {
                    if let Some(dependency) = self
                        .graph
                        .dependency(package, self.program.symbol_text(name))
                    {
                        (
                            LookupResult {
                                item: ItemId::Module(self.program.package_roots[dependency]),
                                provenance: Vec::new(),
                            },
                            1,
                            true,
                        )
                    } else if let Some(found) =
                        self.lookup_module_name(from_module, name, false, use_span)
                    {
                        (found, 1, false)
                    } else if let Some(item) = self.program.prelude.get(&name).copied() {
                        (
                            LookupResult {
                                item,
                                provenance: Vec::new(),
                            },
                            1,
                            false,
                        )
                    } else if let Some(found) = self.standard_package_root(name) {
                        (found, 1, true)
                    } else {
                        self.unresolved_name(name, use_span);
                        return None;
                    }
                } else if let Some(found) =
                    self.lookup_module_name(from_module, name, false, use_span)
                {
                    (found, 1, true)
                } else if let Some(found) = self.standard_package_root(name) {
                    (found, 1, true)
                } else {
                    self.unresolved_name(name, use_span);
                    return None;
                }
            }
        };

        while index < parts.len() {
            let PathPart::Name(name) = parts[index] else {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::NameResolution,
                        "a path root keyword may appear only as the first component",
                    )
                    .with_primary(use_span),
                );
                return None;
            };
            let ItemId::Module(module) = result.item else {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::NameResolution,
                        format!(
                            "`{}` is not a module, so the path cannot continue",
                            self.item_display(result.item)
                        ),
                    )
                    .with_primary(use_span),
                );
                return None;
            };
            let target_package = self.program.modules[module.index()].package.clone();
            external = external
                || match (&from_package, &target_package) {
                    (Some(from), Some(target)) => from != target,
                    (_, None) => true,
                    _ => false,
                };
            let name_exists = self.program.modules[module.index()]
                .namespace
                .contains_key(&name);
            let Some(next) = self.lookup_module_name(module, name, external, use_span) else {
                if !name_exists {
                    self.unresolved_name(name, use_span);
                }
                return None;
            };
            result.provenance.extend(next.provenance);
            result.item = next.item;
            index += 1;
        }
        Some(result)
    }

    fn lookup_module_name(
        &mut self,
        module: ModuleId,
        name: Symbol,
        require_public: bool,
        use_span: Span,
    ) -> Option<LookupResult> {
        let entry = self.program.modules[module.index()]
            .namespace
            .get(&name)
            .cloned()?;
        if require_public && entry.visibility != Visibility::Public {
            let mut diagnostic = Diagnostic::new(
                Category::Visibility,
                format!(
                    "`{}` is package-private and cannot be accessed from another package",
                    self.program.symbol_text(name)
                ),
            )
            .with_primary(use_span);
            if let Some(span) = entry.span {
                diagnostic = diagnostic.with_related(span, "package-private item is here");
            }
            self.diagnostics.push(diagnostic);
            return None;
        }
        match entry.target {
            NamespaceTarget::Item(item) => Some(LookupResult {
                item,
                provenance: Vec::new(),
            }),
            NamespaceTarget::Import(import)
                if self.program.imports[import.index()].state == ImportState::Resolving
                    && self.program.imports[import.index()]
                        .replaced_file_module
                        .is_some() =>
            {
                Some(LookupResult {
                    item: ItemId::Module(
                        self.program.imports[import.index()]
                            .replaced_file_module
                            .expect("guard checked"),
                    ),
                    provenance: Vec::new(),
                })
            }
            NamespaceTarget::Import(import) => self.resolve_import(import),
        }
    }

    fn item_can_be_reexported(&self, item: ItemId) -> bool {
        match item {
            ItemId::Module(_) | ItemId::Builtin(_) => true,
            ItemId::Declaration(declaration) => {
                self.program.declarations[declaration.index()].visibility == Visibility::Public
            }
        }
    }

    fn item_display(&self, item: ItemId) -> String {
        match item {
            ItemId::Module(module) => {
                let module = &self.program.modules[module.index()];
                if module.package.is_none() {
                    module
                        .path
                        .iter()
                        .map(|part| self.program.symbol_text(*part))
                        .collect::<Vec<_>>()
                        .join(".")
                } else if module.path.is_empty() {
                    "root".to_string()
                } else {
                    format!(
                        "root.{}",
                        module
                            .path
                            .iter()
                            .map(|part| self.program.symbol_text(*part))
                            .collect::<Vec<_>>()
                            .join(".")
                    )
                }
            }
            ItemId::Declaration(declaration) => self
                .program
                .symbol_text(self.program.declarations[declaration.index()].name)
                .to_string(),
            ItemId::Builtin(builtin) => self
                .program
                .symbol_text(self.program.builtins[builtin.index()].name)
                .to_string(),
        }
    }

    /// Resolves the name `std` to the standard-library package root.
    ///
    /// `std` is an ordinary name rather than a keyword (SPEC 2.2), so this is
    /// consulted only after lexical bindings, module declarations, imports,
    /// dependency aliases, and prelude names have all failed. A module that
    /// declares or imports its own `std` therefore shadows this.
    fn standard_package_root(&self, name: Symbol) -> Option<LookupResult> {
        (self.program.symbol_text(name) == "std").then(|| LookupResult {
            item: ItemId::Module(self.program.std_root),
            provenance: Vec::new(),
        })
    }

    fn unresolved_name(&mut self, name: Symbol, span: Span) {
        self.diagnostics.push(
            Diagnostic::new(
                Category::NameResolution,
                format!("cannot resolve `{}`", self.program.symbol_text(name)),
            )
            .with_primary(span),
        );
    }

    fn resolve_all_declaration_contents(&mut self) {
        for index in 0..self.program.impls.len() {
            let implementation = ImplId(index as u32);
            let syntax = self.program.impls[index].syntax.clone();
            let module = self.program.impls[index].module;
            let generics = self.generic_scope_for_impl(implementation);
            self.resolve_non_body_syntax(&syntax, module, &generics, false, true);
        }
        for index in 0..self.program.declarations.len() {
            self.resolve_declaration(DeclarationId(index as u32));
        }
    }

    fn resolve_declaration(&mut self, declaration: DeclarationId) {
        let item = &self.program.declarations[declaration.index()];
        let syntax = item.syntax.clone();
        let module = item.module;
        let kind = item.kind;
        let generics = self.generic_scope_for_declaration(declaration);
        let self_allowed = item.parent_declaration.is_some()
            || item.parent_impl.is_some()
            || matches!(kind, DeclarationKind::Struct | DeclarationKind::Trait);
        self.resolve_non_body_syntax(&syntax, module, &generics, self_allowed, false);
        if matches!(
            kind,
            DeclarationKind::Struct | DeclarationKind::ForeignStruct
        ) {
            let fields = self
                .program
                .fields
                .iter()
                .filter(|field| {
                    field.parent_declaration == declaration && field.parent_variant.is_none()
                })
                .map(|field| field.syntax.clone())
                .collect::<Vec<_>>();
            for field in fields {
                self.resolve_non_body_syntax(&field, module, &generics, true, false);
            }
        }
        if kind == DeclarationKind::Enum {
            let variants = self
                .program
                .variants
                .iter()
                .filter(|variant| variant.parent == declaration)
                .map(|variant| variant.syntax.clone())
                .collect::<Vec<_>>();
            for variant in variants {
                self.resolve_non_body_syntax(&variant, module, &generics, false, false);
            }
        }
        if kind == DeclarationKind::Function {
            self.resolve_function_body(declaration, &syntax, module, &generics, self_allowed);
        }
    }

    fn generic_scope_for_impl(
        &self,
        implementation: ImplId,
    ) -> BTreeMap<Symbol, GenericParameterId> {
        self.program.impls[implementation.index()]
            .generic_parameters
            .iter()
            .map(|parameter| {
                let parameter = &self.program.generic_parameters[parameter.index()];
                (parameter.name, parameter.id)
            })
            .collect()
    }

    fn generic_scope_for_declaration(
        &self,
        declaration: DeclarationId,
    ) -> BTreeMap<Symbol, GenericParameterId> {
        let mut chain = Vec::new();
        let mut current = Some(declaration);
        while let Some(id) = current {
            chain.push(id);
            current = self.program.declarations[id.index()].parent_declaration;
        }
        chain.reverse();
        let mut scope = BTreeMap::new();
        if let Some(implementation) = self.program.declarations[declaration.index()].parent_impl {
            for parameter in &self.program.impls[implementation.index()].generic_parameters {
                let parameter = &self.program.generic_parameters[parameter.index()];
                scope.insert(parameter.name, parameter.id);
            }
        }
        for id in chain {
            for parameter in &self.program.declarations[id.index()].generic_parameters {
                let parameter = &self.program.generic_parameters[parameter.index()];
                scope.insert(parameter.name, parameter.id);
            }
        }
        scope
    }

    fn resolve_non_body_syntax(
        &mut self,
        node: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        self_allowed: bool,
        implementation_root: bool,
    ) {
        for child in &node.children {
            let SyntaxElement::Node(child) = child else {
                continue;
            };
            if child.kind == SyntaxKind::Block {
                continue;
            }
            if child.kind == SyntaxKind::Function && node.kind != SyntaxKind::Function {
                continue;
            }
            match child.kind {
                SyntaxKind::Type => {
                    self.resolve_type(child, module, generics, self_allowed);
                }
                SyntaxKind::DeriveList => {
                    self.resolve_derive_list(child, module, generics, self_allowed);
                }
                SyntaxKind::GenericParameter | SyntaxKind::GenericParameters => {
                    self.resolve_non_body_syntax(
                        child,
                        module,
                        generics,
                        self_allowed,
                        implementation_root,
                    );
                }
                _ => self.resolve_non_body_syntax(
                    child,
                    module,
                    generics,
                    self_allowed,
                    implementation_root,
                ),
            }
        }
        if implementation_root && node.kind == SyntaxKind::Impl {
            // The two direct `Type` children above are the trait and target;
            // method blocks are deliberately handled as declarations.
        }
    }

    fn resolve_derive_list(
        &mut self,
        node: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        self_allowed: bool,
    ) {
        let mut current = Vec::new();
        for child in &node.children {
            match child {
                SyntaxElement::Token(Token {
                    kind: TokenKind::Comma,
                    ..
                }) => {
                    self.resolve_token_path(module, &current, generics, self_allowed, node.span);
                    current.clear();
                }
                SyntaxElement::Token(token)
                    if matches!(
                        token.kind,
                        TokenKind::Identifier(_)
                            | TokenKind::Keyword(
                                Keyword::Root
                                    | Keyword::SelfValue
                                    | Keyword::SelfType
                                    | Keyword::Super
                            )
                            | TokenKind::Dot
                    ) =>
                {
                    current.push(token.clone());
                }
                _ => {}
            }
        }
        self.resolve_token_path(module, &current, generics, self_allowed, node.span);
    }

    fn resolve_type(
        &mut self,
        node: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        self_allowed: bool,
    ) {
        let direct_tokens = node
            .children
            .iter()
            .filter_map(|child| match child {
                SyntaxElement::Token(token)
                    if matches!(
                        token.kind,
                        TokenKind::Identifier(_)
                            | TokenKind::Keyword(
                                Keyword::Root
                                    | Keyword::SelfValue
                                    | Keyword::SelfType
                                    | Keyword::Super
                            )
                            | TokenKind::Dot
                    ) =>
                {
                    Some(token.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        self.resolve_token_path(module, &direct_tokens, generics, self_allowed, node.span);
        for child in &node.children {
            let SyntaxElement::Node(child) = child else {
                continue;
            };
            match child.kind {
                SyntaxKind::Type => self.resolve_type(child, module, generics, self_allowed),
                SyntaxKind::TypeArguments => {
                    for argument in child_nodes(child) {
                        if argument.kind == SyntaxKind::Type {
                            self.resolve_type(argument, module, generics, self_allowed);
                        }
                    }
                }
                _ => {
                    let mut scopes = LexicalScopes::default();
                    scopes.push();
                    self.resolve_expression(child, module, generics, &scopes, self_allowed);
                }
            }
        }
    }

    fn resolve_token_path(
        &mut self,
        module: ModuleId,
        tokens: &[Token],
        generics: &BTreeMap<Symbol, GenericParameterId>,
        self_allowed: bool,
        _fallback: Span,
    ) -> Option<NameTarget> {
        let meaningful = tokens
            .iter()
            .filter(|token| !matches!(token.kind, TokenKind::Dot))
            .collect::<Vec<_>>();
        let first = meaningful.first().copied()?;
        let span = Span::new(
            first.span.file,
            first.span.start,
            meaningful
                .last()
                .map_or(first.span.end, |last| last.span.end),
        );
        if meaningful.len() == 1 {
            match &first.kind {
                TokenKind::Identifier(name) => {
                    let symbol = self.intern(name);
                    if let Some(parameter) = generics.get(&symbol).copied() {
                        let target = NameTarget::GenericParameter(parameter);
                        self.record_reference(span, target, Vec::new());
                        return Some(target);
                    }
                }
                TokenKind::Keyword(Keyword::SelfType) => {
                    if self_allowed {
                        self.record_reference(span, NameTarget::SelfType, Vec::new());
                        return Some(NameTarget::SelfType);
                    }
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::NameResolution,
                            "`Self` is valid only in a struct, trait, or trait implementation",
                        )
                        .with_primary(span),
                    );
                    return None;
                }
                _ => {}
            }
        }
        let parts = path_parts_from_tokens(tokens, &mut self.program.symbols);
        let result = self.resolve_module_path(module, &parts, span)?;
        let target = NameTarget::Item(result.item);
        self.record_reference(span, target, result.provenance);
        Some(target)
    }

    fn resolve_function_body(
        &mut self,
        _declaration: DeclarationId,
        syntax: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        self_allowed: bool,
    ) {
        let mut scopes = LexicalScopes::default();
        scopes.push();
        if let Some(parameters) = child_nodes(syntax)
            .into_iter()
            .find(|node| node.kind == SyntaxKind::Parameters)
        {
            for parameter in child_nodes(parameters)
                .into_iter()
                .filter(|node| node.kind == SyntaxKind::Parameter)
            {
                if let Some(token) = parameter_name_token(parameter) {
                    self.declare_local(&mut scopes, token, LocalBindingKind::Parameter);
                }
            }
        }
        if let Some(block) = child_nodes(syntax)
            .into_iter()
            .find(|node| node.kind == SyntaxKind::Block)
        {
            self.resolve_block(block, module, generics, &mut scopes, self_allowed, false);
        }
    }

    fn resolve_block(
        &mut self,
        block: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        scopes: &mut LexicalScopes,
        self_allowed: bool,
        nested: bool,
    ) {
        if nested {
            scopes.push();
        }
        for statement in child_nodes(block) {
            self.resolve_statement(statement, module, generics, scopes, self_allowed);
        }
        if nested {
            scopes.pop();
        }
    }

    fn resolve_statement(
        &mut self,
        node: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        scopes: &mut LexicalScopes,
        self_allowed: bool,
    ) {
        match node.kind {
            SyntaxKind::LetStatement => {
                for child in child_nodes(node) {
                    if child.kind == SyntaxKind::Type {
                        self.resolve_type(child, module, generics, self_allowed);
                    } else {
                        self.resolve_expression(child, module, generics, scopes, self_allowed);
                    }
                }
                if let Some(token) = binding_name_token(node) {
                    self.declare_local(scopes, token, LocalBindingKind::Local);
                }
            }
            SyntaxKind::IfStatement => {
                for child in child_nodes(node) {
                    match child.kind {
                        SyntaxKind::Block => {
                            self.resolve_block(child, module, generics, scopes, self_allowed, true)
                        }
                        SyntaxKind::ElseClause => {
                            if let Some(block) = child_nodes(child)
                                .into_iter()
                                .find(|node| node.kind == SyntaxKind::Block)
                            {
                                self.resolve_block(
                                    block,
                                    module,
                                    generics,
                                    scopes,
                                    self_allowed,
                                    true,
                                );
                            }
                        }
                        _ => {
                            self.resolve_expression(child, module, generics, scopes, self_allowed);
                        }
                    }
                }
            }
            SyntaxKind::MatchStatement => {
                for child in child_nodes(node) {
                    if child.kind == SyntaxKind::Block {
                        for arm in child_nodes(child)
                            .into_iter()
                            .filter(|node| node.kind == SyntaxKind::MatchArm)
                        {
                            self.resolve_match_arm(arm, module, generics, scopes, self_allowed);
                        }
                    } else {
                        self.resolve_expression(child, module, generics, scopes, self_allowed);
                    }
                }
            }
            SyntaxKind::ForStatement => {
                let children = child_nodes(node);
                for child in &children {
                    if child.kind != SyntaxKind::Block {
                        self.resolve_expression(child, module, generics, scopes, self_allowed);
                    }
                }
                scopes.push();
                if let Some(token) = for_binding_token(node) {
                    self.declare_local(scopes, token, LocalBindingKind::Loop);
                }
                if let Some(block) = children
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)
                {
                    self.resolve_block(block, module, generics, scopes, self_allowed, false);
                }
                scopes.pop();
            }
            SyntaxKind::WhileStatement | SyntaxKind::UnsafeBlock => {
                for child in child_nodes(node) {
                    if child.kind == SyntaxKind::Block {
                        self.resolve_block(child, module, generics, scopes, self_allowed, true);
                    } else {
                        self.resolve_expression(child, module, generics, scopes, self_allowed);
                    }
                }
            }
            SyntaxKind::AssignmentStatement
            | SyntaxKind::ExpressionStatement
            | SyntaxKind::ReturnStatement => {
                for child in child_nodes(node) {
                    self.resolve_expression(child, module, generics, scopes, self_allowed);
                }
            }
            // `defer:` defers a block, whose body is its own lexical scope;
            // `defer call` defers one call expression.
            SyntaxKind::DeferStatement => {
                for child in child_nodes(node) {
                    if child.kind == SyntaxKind::Block {
                        self.resolve_block(child, module, generics, scopes, self_allowed, true);
                    } else {
                        self.resolve_expression(child, module, generics, scopes, self_allowed);
                    }
                }
            }
            SyntaxKind::BreakStatement
            | SyntaxKind::ContinueStatement
            | SyntaxKind::PassStatement => {}
            _ => {
                for child in child_nodes(node) {
                    self.resolve_expression(child, module, generics, scopes, self_allowed);
                }
            }
        }
    }

    fn resolve_match_arm(
        &mut self,
        arm: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        scopes: &mut LexicalScopes,
        self_allowed: bool,
    ) {
        scopes.push();
        if let Some(pattern) = child_nodes(arm).into_iter().find(|node| {
            matches!(
                node.kind,
                SyntaxKind::Pattern
                    | SyntaxKind::AlternativePattern
                    | SyntaxKind::DereferencePattern
                    | SyntaxKind::TuplePattern
                    | SyntaxKind::RecordPattern
                    | SyntaxKind::VariantPattern
            )
        }) {
            let mut bindings = BTreeMap::<Symbol, Span>::new();
            self.collect_pattern_bindings(pattern, module, generics, self_allowed, &mut bindings);
            for (name, span) in bindings {
                self.declare_local_symbol(scopes, name, span, LocalBindingKind::PatternCandidate);
            }
        }
        for child in child_nodes(arm) {
            match child.kind {
                SyntaxKind::Guard => {
                    for expression in child_nodes(child) {
                        self.resolve_expression(expression, module, generics, scopes, self_allowed);
                    }
                }
                SyntaxKind::Block => {
                    self.resolve_block(child, module, generics, scopes, self_allowed, false)
                }
                _ => {}
            }
        }
        scopes.pop();
    }

    fn collect_pattern_bindings(
        &mut self,
        pattern: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        self_allowed: bool,
        bindings: &mut BTreeMap<Symbol, Span>,
    ) {
        match pattern.kind {
            SyntaxKind::RecordPattern | SyntaxKind::VariantPattern => {
                let path_tokens = leading_pattern_path(pattern);
                self.resolve_pattern_path(
                    module,
                    &path_tokens,
                    generics,
                    self_allowed,
                    pattern.span,
                );
                for child in child_nodes(pattern) {
                    if child.kind == SyntaxKind::PatternField {
                        let nested = child_nodes(child);
                        if nested.is_empty() {
                            if let Some(token) = first_identifier(child) {
                                let name = self.intern(token_text(token));
                                bindings.entry(name).or_insert(token.span);
                            }
                        } else {
                            for nested in nested {
                                self.collect_pattern_bindings(
                                    nested,
                                    module,
                                    generics,
                                    self_allowed,
                                    bindings,
                                );
                            }
                        }
                    } else if matches!(
                        child.kind,
                        SyntaxKind::Pattern
                            | SyntaxKind::AlternativePattern
                            | SyntaxKind::DereferencePattern
                            | SyntaxKind::TuplePattern
                            | SyntaxKind::RecordPattern
                            | SyntaxKind::VariantPattern
                    ) {
                        self.collect_pattern_bindings(
                            child,
                            module,
                            generics,
                            self_allowed,
                            bindings,
                        );
                    }
                }
            }
            SyntaxKind::TuplePattern
            | SyntaxKind::AlternativePattern
            | SyntaxKind::DereferencePattern => {
                for child in child_nodes(pattern) {
                    self.collect_pattern_bindings(child, module, generics, self_allowed, bindings);
                }
            }
            SyntaxKind::Pattern => {
                let nested = child_nodes(pattern);
                if !nested.is_empty() {
                    for child in nested {
                        self.collect_pattern_bindings(
                            child,
                            module,
                            generics,
                            self_allowed,
                            bindings,
                        );
                    }
                    return;
                }
                let identifiers = pattern
                    .children
                    .iter()
                    .filter_map(|child| match child {
                        SyntaxElement::Token(token)
                            if matches!(token.kind, TokenKind::Identifier(_)) =>
                        {
                            Some(token)
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                let has_dot = pattern.children.iter().any(|child| {
                    matches!(
                        child,
                        SyntaxElement::Token(Token {
                            kind: TokenKind::Dot,
                            ..
                        })
                    )
                });
                if has_dot {
                    let tokens = pattern
                        .children
                        .iter()
                        .filter_map(|child| match child {
                            SyntaxElement::Token(token) => Some(token.clone()),
                            SyntaxElement::Node(_) => None,
                        })
                        .collect::<Vec<_>>();
                    self.resolve_pattern_path(
                        module,
                        &tokens,
                        generics,
                        self_allowed,
                        pattern.span,
                    );
                } else if let [token] = identifiers.as_slice() {
                    let name = self.intern(token_text(token));
                    if self.program.symbol_text(name) != "_" {
                        bindings.entry(name).or_insert(token.span);
                    }
                }
            }
            _ => {}
        }
    }

    fn resolve_pattern_path(
        &mut self,
        module: ModuleId,
        tokens: &[Token],
        generics: &BTreeMap<Symbol, GenericParameterId>,
        self_allowed: bool,
        fallback: Span,
    ) -> Option<NameTarget> {
        let meaningful = tokens
            .iter()
            .filter(|token| !matches!(token.kind, TokenKind::Dot))
            .collect::<Vec<_>>();
        let first = meaningful.first().copied()?;
        if meaningful.len() == 1 {
            return self.resolve_token_path(module, tokens, generics, self_allowed, fallback);
        }
        let parts = path_parts_from_tokens(tokens, &mut self.program.symbols);
        let mut latest = None;
        for end in 1..=parts.len() {
            let span = Span::new(
                first.span.file,
                first.span.start,
                meaningful
                    .get(end.saturating_sub(1))
                    .map_or(first.span.end, |token| token.span.end),
            );
            let result = self.resolve_module_path(module, &parts[..end], span)?;
            let is_module = matches!(result.item, ItemId::Module(_));
            latest = Some((span, result));
            if !is_module {
                break;
            }
        }
        let (span, result) = latest?;
        let target = NameTarget::Item(result.item);
        self.record_reference(span, target, result.provenance);
        Some(target)
    }

    fn resolve_expression(
        &mut self,
        node: &SyntaxNode,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        scopes: &LexicalScopes,
        self_allowed: bool,
    ) -> Option<NameTarget> {
        match node.kind {
            SyntaxKind::NameExpression => {
                let token = node.children.iter().find_map(|child| match child {
                    SyntaxElement::Token(token) => Some(token),
                    SyntaxElement::Node(_) => None,
                })?;
                self.resolve_unqualified_name(token, module, generics, scopes, self_allowed)
            }
            SyntaxKind::MemberExpression => {
                let base = child_nodes(node).first().copied().and_then(|child| {
                    self.resolve_expression(child, module, generics, scopes, self_allowed)
                });
                let member = node.children.iter().rev().find_map(|child| match child {
                    SyntaxElement::Token(token)
                        if matches!(token.kind, TokenKind::Identifier(_)) =>
                    {
                        Some(token)
                    }
                    _ => None,
                });
                if let (Some(NameTarget::Item(ItemId::Module(base))), Some(member)) = (base, member)
                {
                    let name = self.intern(token_text(member));
                    let require_public = self.module_access_is_external(module, base);
                    let result =
                        self.lookup_module_name(base, name, require_public, member.span)?;
                    let target = NameTarget::Item(result.item);
                    self.record_reference(member.span, target, result.provenance);
                    Some(target)
                } else {
                    None
                }
            }
            SyntaxKind::Type => {
                self.resolve_type(node, module, generics, self_allowed);
                None
            }
            SyntaxKind::RecordField if child_nodes(node).is_empty() => {
                let token = first_identifier(node)?;
                self.resolve_unqualified_name(token, module, generics, scopes, self_allowed)
            }
            _ => {
                let mut result = None;
                for child in child_nodes(node) {
                    result = self
                        .resolve_expression(child, module, generics, scopes, self_allowed)
                        .or(result);
                }
                if node.kind == SyntaxKind::ParenthesizedExpression {
                    result
                } else {
                    None
                }
            }
        }
    }

    fn resolve_unqualified_name(
        &mut self,
        token: &Token,
        module: ModuleId,
        generics: &BTreeMap<Symbol, GenericParameterId>,
        scopes: &LexicalScopes,
        self_allowed: bool,
    ) -> Option<NameTarget> {
        let (name, special) = match &token.kind {
            TokenKind::Identifier(name) => (self.intern(name), None),
            TokenKind::Keyword(Keyword::SelfValue) => {
                (self.intern("self"), Some(Keyword::SelfValue))
            }
            TokenKind::Keyword(Keyword::SelfType) => (self.intern("Self"), Some(Keyword::SelfType)),
            TokenKind::Keyword(Keyword::Root) => (self.intern("root"), Some(Keyword::Root)),
            TokenKind::Keyword(Keyword::Super) => (self.intern("super"), Some(Keyword::Super)),
            _ => return None,
        };
        if special == Some(Keyword::SelfValue) {
            if let Some(local) = scopes.lookup(name) {
                let target = NameTarget::Local(local);
                self.record_reference(token.span, target, Vec::new());
                return Some(target);
            }
            self.unresolved_name(name, token.span);
            return None;
        }
        if special == Some(Keyword::SelfType) {
            if self_allowed {
                self.record_reference(token.span, NameTarget::SelfType, Vec::new());
                return Some(NameTarget::SelfType);
            }
            self.diagnostics.push(
                Diagnostic::new(
                    Category::NameResolution,
                    "`Self` is valid only in a struct, trait, or trait implementation",
                )
                .with_primary(token.span),
            );
            return None;
        }
        if let Some(local) = scopes.lookup(name) {
            let target = NameTarget::Local(local);
            self.record_reference(token.span, target, Vec::new());
            return Some(target);
        }
        if let Some(parameter) = generics.get(&name).copied() {
            let target = NameTarget::GenericParameter(parameter);
            self.record_reference(token.span, target, Vec::new());
            return Some(target);
        }
        let result = match special {
            Some(Keyword::Root) => self.resolve_module_path(module, &[PathPart::Root], token.span),
            Some(Keyword::Super) => {
                self.resolve_module_path(module, &[PathPart::Super], token.span)
            }
            _ => {
                let package = self.program.modules[module.index()].package.clone();
                if let Some(package) = package {
                    if let Some(dependency) = self
                        .graph
                        .dependency(&package, self.program.symbol_text(name))
                    {
                        Some(LookupResult {
                            item: ItemId::Module(self.program.package_roots[dependency]),
                            provenance: Vec::new(),
                        })
                    } else if let Some(found) =
                        self.lookup_module_name(module, name, false, token.span)
                    {
                        Some(found)
                    } else {
                        self.program
                            .prelude
                            .get(&name)
                            .copied()
                            .map(|item| LookupResult {
                                item,
                                provenance: Vec::new(),
                            })
                            .or_else(|| self.standard_package_root(name))
                    }
                } else {
                    self.lookup_module_name(module, name, false, token.span)
                        .or_else(|| self.standard_package_root(name))
                }
            }
        };
        let Some(result) = result else {
            self.unresolved_name(name, token.span);
            return None;
        };
        let target = NameTarget::Item(result.item);
        self.record_reference(token.span, target, result.provenance);
        Some(target)
    }

    fn module_access_is_external(&self, from: ModuleId, target: ModuleId) -> bool {
        match (
            &self.program.modules[from.index()].package,
            &self.program.modules[target.index()].package,
        ) {
            (Some(from), Some(target)) => from != target,
            (_, None) => true,
            _ => false,
        }
    }

    fn declare_local(
        &mut self,
        scopes: &mut LexicalScopes,
        token: &Token,
        kind: LocalBindingKind,
    ) -> LocalBindingId {
        let name = match &token.kind {
            TokenKind::Identifier(name) => self.intern(name),
            TokenKind::Keyword(Keyword::SelfValue) => self.intern("self"),
            _ => unreachable!("binding token is an identifier or `self`"),
        };
        self.declare_local_symbol(scopes, name, token.span, kind)
    }

    fn declare_local_symbol(
        &mut self,
        scopes: &mut LexicalScopes,
        name: Symbol,
        span: Span,
        kind: LocalBindingKind,
    ) -> LocalBindingId {
        let id = LocalBindingId(self.program.local_bindings.len() as u32);
        let current = scopes
            .scopes
            .last_mut()
            .expect("function resolution has a lexical scope");
        if let Some(previous) = current.get(&name).copied() {
            self.duplicate_diagnostic(
                "lexical binding",
                span,
                Some(self.program.local_bindings[previous.index()].span),
            );
        } else {
            current.insert(name, id);
        }
        self.program.local_bindings.push(LocalBinding {
            id,
            name,
            span,
            kind,
        });
        id
    }

    fn record_reference(&mut self, span: Span, target: NameTarget, provenance: Vec<Span>) {
        self.program.references.push(ResolvedReference {
            span,
            target,
            provenance,
        });
    }

    fn compute_external_reachability(&mut self) {
        let mut queue = Vec::new();
        for root in self.program.package_roots.values().copied() {
            self.program.modules[root.index()].externally_reachable = true;
            queue.push(root);
        }
        queue.push(self.program.std_root);
        let mut visited = BTreeSet::new();
        while let Some(module) = queue.pop() {
            if !visited.insert(module) {
                continue;
            }
            let entries = self.program.modules[module.index()]
                .namespace
                .values()
                .filter(|entry| entry.visibility == Visibility::Public)
                .cloned()
                .collect::<Vec<_>>();
            for entry in entries {
                let item = match entry.target {
                    NamespaceTarget::Item(item) => Some(item),
                    NamespaceTarget::Import(import) => self.program.imports[import.index()].target,
                };
                match item {
                    Some(ItemId::Module(target)) => {
                        if !self.program.modules[target.index()].externally_reachable {
                            self.program.modules[target.index()].externally_reachable = true;
                        }
                        queue.push(target);
                    }
                    Some(ItemId::Declaration(declaration)) => {
                        self.program.declarations[declaration.index()].externally_reachable = true;
                    }
                    Some(ItemId::Builtin(_)) | None => {}
                }
            }
        }

        let reachability = self
            .program
            .declarations
            .iter()
            .map(|declaration| declaration.externally_reachable)
            .collect::<Vec<_>>();
        for index in 0..self.program.declarations.len() {
            let parent = self.program.declarations[index].parent_declaration;
            let Some(parent) = parent else { continue };
            if !reachability[parent.index()] {
                continue;
            }
            let parent_kind = self.program.declarations[parent.index()].kind;
            let visible = parent_kind == DeclarationKind::Trait
                || self.program.declarations[index].visibility == Visibility::Public;
            if visible {
                self.program.declarations[index].externally_reachable = true;
            }
        }
    }

    fn check_public_signatures(&mut self) {
        let mut checked = BTreeSet::<(FileId, u32, u32)>::new();
        for index in 0..self.program.declarations.len() {
            let declaration = DeclarationId(index as u32);
            if !self.program.declarations[index].externally_reachable {
                continue;
            }
            let item = &self.program.declarations[index];
            let syntax = item.syntax.clone();
            match item.kind {
                DeclarationKind::Struct | DeclarationKind::ForeignStruct => {
                    self.check_signature_node(&syntax, &mut checked);
                    let fields = self
                        .program
                        .fields
                        .iter()
                        .filter(|field| {
                            field.parent_declaration == declaration
                                && field.parent_variant.is_none()
                                && field.visibility == Visibility::Public
                        })
                        .map(|field| field.syntax.clone())
                        .collect::<Vec<_>>();
                    for field in fields {
                        self.check_signature_node(&field, &mut checked);
                    }
                }
                DeclarationKind::Enum => {
                    self.check_signature_node(&syntax, &mut checked);
                    let variants = self
                        .program
                        .variants
                        .iter()
                        .filter(|variant| variant.parent == declaration)
                        .map(|variant| variant.syntax.clone())
                        .collect::<Vec<_>>();
                    for variant in variants {
                        self.check_signature_node(&variant, &mut checked);
                    }
                }
                DeclarationKind::Trait => {
                    self.check_signature_node(&syntax, &mut checked);
                }
                DeclarationKind::Function
                | DeclarationKind::ForeignFunction
                | DeclarationKind::TypeAlias
                | DeclarationKind::ForeignType => {
                    self.check_signature_node(&syntax, &mut checked);
                }
            }
        }
    }

    fn check_signature_node(
        &mut self,
        node: &SyntaxNode,
        checked: &mut BTreeSet<(FileId, u32, u32)>,
    ) {
        let types = signature_type_nodes(node);
        for ty in types {
            let references = self
                .program
                .references
                .iter()
                .filter(|reference| {
                    reference.span.file == ty.span.file
                        && reference.span.start >= ty.span.start
                        && reference.span.end <= ty.span.end
                })
                .cloned()
                .collect::<Vec<_>>();
            for reference in references {
                let NameTarget::Item(ItemId::Declaration(target)) = reference.target else {
                    continue;
                };
                if self.program.declarations[target.index()].externally_reachable {
                    continue;
                }
                let key = (
                    reference.span.file,
                    reference.span.start,
                    reference.span.end,
                );
                if !checked.insert(key) {
                    continue;
                }
                let mut diagnostic = Diagnostic::new(
                    Category::Visibility,
                    format!(
                        "public signature exposes `{}`, which is not externally reachable",
                        self.program
                            .symbol_text(self.program.declarations[target.index()].name)
                    ),
                )
                .with_primary(reference.span)
                .with_related(
                    self.program.declarations[target.index()].span,
                    "less-visible declaration is here",
                );
                for provenance in reference.provenance {
                    diagnostic = diagnostic
                        .with_related(provenance, "name was supplied by this import or re-export");
                }
                self.diagnostics.push(diagnostic);
            }
        }
    }
}

fn direct_item_nodes(node: &SyntaxNode) -> Vec<&SyntaxNode> {
    let container = if node.kind == SyntaxKind::Module {
        node.children.iter().find_map(|child| match child {
            SyntaxElement::Node(child) if child.kind == SyntaxKind::Block => Some(child.as_ref()),
            _ => None,
        })
    } else {
        Some(node)
    };
    container
        .into_iter()
        .flat_map(|container| &container.children)
        .filter_map(|child| match child {
            SyntaxElement::Node(node) => Some(node.as_ref()),
            SyntaxElement::Token(_) => None,
        })
        .collect()
}

fn direct_block_nodes(node: &SyntaxNode) -> Vec<&SyntaxNode> {
    node.children
        .iter()
        .find_map(|child| match child {
            SyntaxElement::Node(child) if child.kind == SyntaxKind::Block => Some(
                child
                    .children
                    .iter()
                    .filter_map(|element| match element {
                        SyntaxElement::Node(node) => Some(node.as_ref()),
                        SyntaxElement::Token(_) => None,
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

fn child_nodes_of_kind(node: &SyntaxNode, kind: SyntaxKind) -> Vec<&SyntaxNode> {
    fn walk<'a>(node: &'a SyntaxNode, kind: SyntaxKind, output: &mut Vec<&'a SyntaxNode>) {
        for child in &node.children {
            if let SyntaxElement::Node(child) = child {
                if child.kind == kind {
                    output.push(child);
                } else {
                    walk(child, kind, output);
                }
            }
        }
    }
    let mut output = Vec::new();
    walk(node, kind, &mut output);
    output
}

fn direct_children_of_kind(node: &SyntaxNode, kind: SyntaxKind) -> Vec<&SyntaxNode> {
    node.children
        .iter()
        .filter_map(|child| match child {
            SyntaxElement::Node(child) if child.kind == kind => Some(child.as_ref()),
            _ => None,
        })
        .collect()
}

fn generic_parameter_nodes(node: &SyntaxNode) -> Vec<&SyntaxNode> {
    child_nodes(node)
        .into_iter()
        .find(|child| child.kind == SyntaxKind::GenericParameters)
        .map(|parameters| {
            child_nodes(parameters)
                .into_iter()
                .filter(|parameter| parameter.kind == SyntaxKind::GenericParameter)
                .collect()
        })
        .unwrap_or_default()
}

fn child_nodes(node: &SyntaxNode) -> Vec<&SyntaxNode> {
    node.children
        .iter()
        .filter_map(|child| match child {
            SyntaxElement::Node(node) => Some(node.as_ref()),
            SyntaxElement::Token(_) => None,
        })
        .collect()
}

fn first_identifier(node: &SyntaxNode) -> Option<&Token> {
    node.children.iter().find_map(|child| match child {
        SyntaxElement::Token(token) if matches!(token.kind, TokenKind::Identifier(_)) => {
            Some(token)
        }
        _ => None,
    })
}

fn parameter_name_token(node: &SyntaxNode) -> Option<&Token> {
    node.children.iter().find_map(|child| match child {
        SyntaxElement::Token(token)
            if matches!(
                token.kind,
                TokenKind::Identifier(_) | TokenKind::Keyword(Keyword::SelfValue)
            ) =>
        {
            Some(token)
        }
        _ => None,
    })
}

fn binding_name_token(node: &SyntaxNode) -> Option<&Token> {
    let mut saw_binding = false;
    node.children.iter().find_map(|child| {
        let SyntaxElement::Token(token) = child else {
            return None;
        };
        match &token.kind {
            TokenKind::Keyword(Keyword::Let | Keyword::Var) => {
                saw_binding = true;
                None
            }
            TokenKind::Identifier(_) if saw_binding => Some(token),
            _ => None,
        }
    })
}

fn for_binding_token(node: &SyntaxNode) -> Option<&Token> {
    let mut saw_for = false;
    node.children.iter().find_map(|child| {
        let SyntaxElement::Token(token) = child else {
            return None;
        };
        match &token.kind {
            TokenKind::Keyword(Keyword::For) => {
                saw_for = true;
                None
            }
            TokenKind::Identifier(_) if saw_for => Some(token),
            _ => None,
        }
    })
}

fn leading_pattern_path(node: &SyntaxNode) -> Vec<Token> {
    node.children
        .iter()
        .take_while(|child| {
            !matches!(
                child,
                SyntaxElement::Token(Token {
                    kind: TokenKind::LParen | TokenKind::LBrace,
                    ..
                })
            )
        })
        .filter_map(|child| match child {
            SyntaxElement::Token(token) => Some(token.clone()),
            SyntaxElement::Node(_) => None,
        })
        .collect()
}

fn path_parts_from_tokens(tokens: &[Token], symbols: &mut Rodeo<Spur>) -> Vec<PathPart> {
    tokens
        .iter()
        .filter_map(|token| match &token.kind {
            TokenKind::Identifier(name) => {
                Some(PathPart::Name(Symbol(symbols.get_or_intern(name))))
            }
            TokenKind::Keyword(Keyword::Root) => Some(PathPart::Root),
            TokenKind::Keyword(Keyword::SelfValue) => Some(PathPart::SelfModule),
            TokenKind::Keyword(Keyword::Super) => Some(PathPart::Super),
            TokenKind::Keyword(Keyword::SelfType) => {
                Some(PathPart::Name(Symbol(symbols.get_or_intern("Self"))))
            }
            _ => None,
        })
        .collect()
}

fn signature_type_nodes(node: &SyntaxNode) -> Vec<&SyntaxNode> {
    fn walk<'a>(node: &'a SyntaxNode, output: &mut Vec<&'a SyntaxNode>) {
        for child in &node.children {
            let SyntaxElement::Node(child) = child else {
                continue;
            };
            if child.kind == SyntaxKind::Block {
                continue;
            }
            if child.kind == SyntaxKind::Type {
                output.push(child);
            }
            walk(child, output);
        }
    }
    let mut output = Vec::new();
    walk(node, &mut output);
    output
}

fn declaration_name(node: &SyntaxNode, keyword: Keyword) -> Option<(String, Span)> {
    let mut saw_keyword = false;
    for child in &node.children {
        let SyntaxElement::Token(token) = child else {
            continue;
        };
        if matches!(token.kind, TokenKind::Keyword(actual) if actual == keyword) {
            saw_keyword = true;
            continue;
        }
        if saw_keyword {
            if let TokenKind::Identifier(name) = &token.kind {
                return Some((name.clone(), token.span));
            }
        }
    }
    None
}

fn token_text(token: &Token) -> &str {
    match &token.kind {
        TokenKind::Identifier(text) => text,
        _ => unreachable!("caller selected an identifier token"),
    }
}

fn node_visibility(node: &SyntaxNode) -> Visibility {
    if has_keyword(node, Keyword::Pub) {
        Visibility::Public
    } else {
        Visibility::Package
    }
}

fn has_keyword(node: &SyntaxNode, keyword: Keyword) -> bool {
    node.children.iter().any(
        |child| matches!(child, SyntaxElement::Token(Token { kind: TokenKind::Keyword(actual), .. }) if *actual == keyword),
    )
}

fn import_path(
    node: &SyntaxNode,
    symbols: &mut Rodeo<Spur>,
) -> Option<(Vec<PathPart>, Symbol, Span)> {
    let mut saw_import = false;
    let mut saw_as = false;
    let mut parts = Vec::new();
    let mut last = None;
    let mut alias = None;
    for child in &node.children {
        let SyntaxElement::Token(token) = child else {
            continue;
        };
        match &token.kind {
            TokenKind::Keyword(Keyword::Import) => saw_import = true,
            TokenKind::Keyword(Keyword::As) if saw_import => saw_as = true,
            TokenKind::Identifier(name) if saw_import && saw_as => {
                alias = Some((Symbol(symbols.get_or_intern(name)), token.span));
            }
            TokenKind::Identifier(name) if saw_import => {
                let symbol = Symbol(symbols.get_or_intern(name));
                parts.push(PathPart::Name(symbol));
                last = Some((symbol, token.span));
            }
            TokenKind::Keyword(keyword) if saw_import => {
                let part = match keyword {
                    Keyword::Root => PathPart::Root,
                    Keyword::SelfValue => PathPart::SelfModule,
                    Keyword::Super => PathPart::Super,
                    _ => continue,
                };
                parts.push(part);
                let text = match keyword {
                    Keyword::Root => "root",
                    Keyword::SelfValue => "self",
                    Keyword::Super => "super",
                    _ => unreachable!(),
                };
                last = Some((Symbol(symbols.get_or_intern(text)), token.span));
            }
            _ => {}
        }
    }
    let (name, span) = alias.or(last)?;
    Some((parts, name, span))
}
