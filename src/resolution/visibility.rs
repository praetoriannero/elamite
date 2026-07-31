//! External reachability and public-signature visibility validation.

use super::*;

impl<'a> Resolver<'a> {
    pub(super) fn compute_external_reachability(&mut self) {
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

    pub(super) fn check_public_signatures(&mut self) {
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
                DeclarationKind::Closure | DeclarationKind::Test => {}
            }
        }
    }

    pub(super) fn check_signature_node(
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
                        .with_related(provenance, "name was supplied by this `use` or re-export");
                }
                self.diagnostics.push(diagnostic);
            }
        }
    }
}
