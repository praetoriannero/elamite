//! Import lookup and provenance resolution.

use super::*;

impl<'a> Resolver<'a> {
    pub(super) fn resolve_all_imports(&mut self) {
        for index in 0..self.program.imports.len() {
            self.resolve_import(ImportId(index as u32));
        }
    }

    pub(super) fn resolve_import(&mut self, import: ImportId) -> Option<LookupResult> {
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
                        "`use` aliases form a cycle without reaching a declaration",
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
                            "a public `use` cannot replace an existing file-backed module with a different target",
                        )
                        .with_primary(span),
                    );
                }
                if visibility == Visibility::Public && !self.item_can_be_reexported(result.item) {
                    let mut diagnostic = Diagnostic::new(
                        Category::Visibility,
                        "a public `use` cannot re-export a package-private declaration",
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

    pub(super) fn resolve_module_path(
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

    pub(super) fn lookup_module_name(
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

    pub(super) fn item_can_be_reexported(&self, item: ItemId) -> bool {
        match item {
            ItemId::Module(_) | ItemId::Builtin(_) => true,
            ItemId::Declaration(declaration) => {
                self.program.declarations[declaration.index()].visibility == Visibility::Public
            }
        }
    }

    pub(super) fn item_display(&self, item: ItemId) -> String {
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
    pub(super) fn standard_package_root(&self, name: Symbol) -> Option<LookupResult> {
        (self.program.symbol_text(name) == "std").then(|| LookupResult {
            item: ItemId::Module(self.program.std_root),
            provenance: Vec::new(),
        })
    }

    pub(super) fn unresolved_name(&mut self, name: Symbol, span: Span) {
        self.diagnostics.push(
            Diagnostic::new(
                Category::NameResolution,
                format!("cannot resolve `{}`", self.program.symbol_text(name)),
            )
            .with_primary(span),
        );
    }
}
