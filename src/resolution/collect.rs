//! Module-graph and declaration collection.

use super::*;

impl<'a> Resolver<'a> {
    pub(super) fn push_module(
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

    pub(super) fn install_standard_library_names(&mut self) -> (ModuleId, ModuleId) {
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
            if matches!(name, "Formatter" | "str") {
                self.insert_namespace(
                    self.program.std_root,
                    symbol,
                    NamespaceTarget::Item(ItemId::Builtin(id)),
                    if name == "Formatter" {
                        Visibility::Public
                    } else {
                        Visibility::Package
                    },
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

    pub(super) fn push_builtin(&mut self, name: Symbol) -> BuiltinId {
        let id = BuiltinId(self.program.builtins.len() as u32);
        self.program.builtins.push(Builtin { name });
        id
    }

    /// Records the declarations collected from [`crate::standard::ROOT_SOURCE`] as
    /// both the compiler-known standard identities and prelude names, so an
    /// unqualified `Option` resolves after lexical, module, import, and
    /// dependency-alias lookup exactly like the builtin names beside it.
    pub(super) fn register_standard_declarations(&mut self) {
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

    pub(super) fn create_file_module_graph(&mut self) {
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

    pub(super) fn install_parsed_units(&mut self, io_module: ModuleId, ffi_module: ModuleId) {
        for unit in std::mem::take(&mut self.expanded.units) {
            let module = match unit.identity {
                ParsedUnitIdentity::Standard(StandardModule::Root) => self.program.std_root,
                ParsedUnitIdentity::Standard(StandardModule::Io) => io_module,
                ParsedUnitIdentity::Standard(StandardModule::Ffi) => ffi_module,
                ParsedUnitIdentity::PackageRoot(package) => self.program.package_roots[&package],
                ParsedUnitIdentity::PackageModule { package, path } => {
                    self.program.module_keys[&(package, path.components().to_vec())]
                }
            };
            self.program.modules[module.index()].source_file = Some(unit.file);
            self.program.modules[module.index()].span = Some(unit.span);
            self.parsed_units.push(ParsedUnit {
                module,
                tree: unit.tree,
            });
        }
    }

    pub(super) fn discover_inline_modules(&mut self) {
        let units = self
            .parsed_units
            .iter()
            .map(|unit| (unit.module, unit.tree.clone()))
            .collect::<Vec<_>>();
        for (module, tree) in units {
            self.discover_inline_children(module, &tree);
        }
    }

    pub(super) fn discover_inline_children(&mut self, parent: ModuleId, container: &SyntaxNode) {
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

    pub(super) fn collect_all_declarations(&mut self) {
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

    pub(super) fn check_exported_c_symbol_conflicts(&mut self) {
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

    pub(super) fn collect_container(&mut self, module: ModuleId, container: &SyntaxNode) {
        for item in direct_item_nodes(container) {
            match item.kind {
                SyntaxKind::Module | SyntaxKind::PassStatement | SyntaxKind::Error => {}
                SyntaxKind::Use => self.collect_import(module, item),
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

    pub(super) fn collect_import(&mut self, module: ModuleId, node: &SyntaxNode) {
        let Some((path, bound_name, bound_span)) = use_path(node, &mut self.program.symbols) else {
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

    pub(super) fn collect_named_declaration(
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

    pub(super) fn foreign_binding(
        &mut self,
        node: &SyntaxNode,
        parent_declaration: Option<DeclarationId>,
        parent_impl: Option<ImplId>,
    ) -> Option<ForeignBinding> {
        let attributes = crate::syntax::direct_children(node, SyntaxKind::Attribute);
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

    pub(super) fn collect_generic_parameters(
        &mut self,
        declaration: DeclarationId,
        node: &SyntaxNode,
    ) {
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

    pub(super) fn collect_struct_members(&mut self, parent: DeclarationId, foreign: bool) {
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

    pub(super) fn collect_enum_variants(&mut self, parent: DeclarationId) {
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

    pub(super) fn collect_trait_methods(&mut self, parent: DeclarationId) {
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

    pub(super) fn collect_impl(&mut self, module: ModuleId, node: &SyntaxNode) {
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

    pub(super) fn insert_namespace(
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

    pub(super) fn duplicate_diagnostic(&mut self, what: &str, span: Span, previous: Option<Span>) {
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
}
