//! Stable identities and separate namespaces for physical compile-time declarations.
//!
//! Collection happens after parsing and before ordinary name resolution. It
//! deliberately records declarations and imports only; checking signatures,
//! executing bodies, and expanding attachments or invocations belong to later
//! work packages.

use std::collections::BTreeMap;

use crate::diagnostics::{Category, Diagnostic};
use crate::package::{PackageGraph, PackageId};
use crate::source::Span;
use crate::syntax::{Keyword, SyntaxElement, SyntaxKind, SyntaxNode, TokenKind};

use super::{ExpandedUnit, ExpandedUnitIdentity};

macro_rules! identity {
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

identity!(CompileTimeModuleId);
identity!(CompileTimeDeclarationId);
identity!(CompileTimeImportId);

#[cfg(test)]
impl CompileTimeDeclarationId {
    pub(crate) fn from_index(index: u32) -> Self {
        Self(index)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CompileTimeNamespace {
    Macro,
    Attribute,
    Derive,
}

impl CompileTimeNamespace {
    fn description(self) -> &'static str {
        match self {
            Self::Macro => "macro",
            Self::Attribute => "attribute",
            Self::Derive => "derive",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileTimeVisibility {
    Package,
    Public,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileTimeBinding {
    Declaration(CompileTimeDeclarationId),
    Import(CompileTimeImportId),
}

#[derive(Debug, Clone)]
pub struct CompileTimeModule {
    pub id: CompileTimeModuleId,
    pub package: PackageId,
    pub path: Vec<String>,
    pub parent: Option<CompileTimeModuleId>,
    pub span: Option<Span>,
    macros: BTreeMap<String, CompileTimeBinding>,
    attributes: BTreeMap<String, CompileTimeBinding>,
    derives: BTreeMap<String, CompileTimeBinding>,
}

impl CompileTimeModule {
    #[must_use]
    pub fn bindings(
        &self,
        namespace: CompileTimeNamespace,
    ) -> &BTreeMap<String, CompileTimeBinding> {
        match namespace {
            CompileTimeNamespace::Macro => &self.macros,
            CompileTimeNamespace::Attribute => &self.attributes,
            CompileTimeNamespace::Derive => &self.derives,
        }
    }

    fn bindings_mut(
        &mut self,
        namespace: CompileTimeNamespace,
    ) -> &mut BTreeMap<String, CompileTimeBinding> {
        match namespace {
            CompileTimeNamespace::Macro => &mut self.macros,
            CompileTimeNamespace::Attribute => &mut self.attributes,
            CompileTimeNamespace::Derive => &mut self.derives,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompileTimeDeclaration {
    pub id: CompileTimeDeclarationId,
    pub namespace: CompileTimeNamespace,
    pub module: CompileTimeModuleId,
    pub name: String,
    pub visibility: CompileTimeVisibility,
    pub span: Span,
    pub syntax: SyntaxNode,
    /// Written trait path for a derive; empty for macros and attributes.
    pub trait_path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileTimePathPart {
    Root,
    SelfModule,
    Super,
    Name(String),
}

#[derive(Debug, Clone)]
pub struct CompileTimeImport {
    pub id: CompileTimeImportId,
    pub namespace: CompileTimeNamespace,
    pub module: CompileTimeModuleId,
    pub name: String,
    pub visibility: CompileTimeVisibility,
    pub span: Span,
    pub path: Vec<CompileTimePathPart>,
    pub target: Option<CompileTimeDeclarationId>,
    state: ImportState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportState {
    Unresolved,
    Resolving,
    Resolved,
    Failed,
}

#[derive(Debug, Default)]
pub struct CompileTimeEnvironment {
    pub modules: Vec<CompileTimeModule>,
    pub declarations: Vec<CompileTimeDeclaration>,
    pub imports: Vec<CompileTimeImport>,
    module_keys: BTreeMap<(PackageId, Vec<String>), CompileTimeModuleId>,
}

impl CompileTimeEnvironment {
    #[must_use]
    pub fn module(&self, package: &PackageId, path: &[String]) -> Option<CompileTimeModuleId> {
        self.module_keys
            .get(&(package.clone(), path.to_vec()))
            .copied()
    }

    #[must_use]
    pub fn lookup(
        &self,
        module: CompileTimeModuleId,
        namespace: CompileTimeNamespace,
        name: &str,
    ) -> Option<CompileTimeBinding> {
        self.modules[module.index()]
            .bindings(namespace)
            .get(name)
            .copied()
    }

    fn ensure_module(
        &mut self,
        package: PackageId,
        path: Vec<String>,
        span: Option<Span>,
    ) -> CompileTimeModuleId {
        if let Some(id) = self.module_keys.get(&(package.clone(), path.clone())) {
            return *id;
        }
        let parent = if path.is_empty() {
            None
        } else {
            Some(self.ensure_module(package.clone(), path[..path.len() - 1].to_vec(), None))
        };
        let id = CompileTimeModuleId(self.modules.len() as u32);
        self.modules.push(CompileTimeModule {
            id,
            package: package.clone(),
            path: path.clone(),
            parent,
            span,
            macros: BTreeMap::new(),
            attributes: BTreeMap::new(),
            derives: BTreeMap::new(),
        });
        self.module_keys.insert((package, path), id);
        id
    }

    fn binding_span(&self, binding: CompileTimeBinding) -> Span {
        match binding {
            CompileTimeBinding::Declaration(id) => self.declarations[id.index()].span,
            CompileTimeBinding::Import(id) => self.imports[id.index()].span,
        }
    }

    fn insert_binding(
        &mut self,
        module: CompileTimeModuleId,
        namespace: CompileTimeNamespace,
        name: String,
        binding: CompileTimeBinding,
        span: Span,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        let previous = self.modules[module.index()]
            .bindings(namespace)
            .get(&name)
            .copied();
        if let Some(previous) = previous {
            diagnostics.push(
                Diagnostic::new(
                    Category::DeclarationConflict,
                    format!(
                        "{} `{name}` is bound more than once in this module",
                        namespace.description()
                    ),
                )
                .with_primary(span)
                .with_related(self.binding_span(previous), "first binding is here"),
            );
            return;
        }
        self.modules[module.index()]
            .bindings_mut(namespace)
            .insert(name, binding);
    }
}

pub(super) fn collect(
    graph: &PackageGraph,
    units: &[ExpandedUnit],
    diagnostics: &mut Vec<Diagnostic>,
) -> CompileTimeEnvironment {
    let mut environment = CompileTimeEnvironment::default();

    for unit in units {
        let Some((package, path)) = unit_package_path(&unit.identity) else {
            continue;
        };
        let module = environment.ensure_module(package, path, Some(unit.span));
        discover_inline_modules(&mut environment, module, &unit.tree);
    }

    for unit in units {
        let Some((package, path)) = unit_package_path(&unit.identity) else {
            continue;
        };
        let module = environment
            .module(&package, &path)
            .expect("source unit module was installed");
        collect_container(&mut environment, module, &unit.tree, diagnostics);
    }

    for index in 0..environment.imports.len() {
        resolve_import(
            &mut environment,
            graph,
            CompileTimeImportId(index as u32),
            diagnostics,
        );
    }
    environment
}

fn unit_package_path(identity: &ExpandedUnitIdentity) -> Option<(PackageId, Vec<String>)> {
    match identity {
        ExpandedUnitIdentity::Standard(_) => None,
        ExpandedUnitIdentity::PackageRoot(package) => Some((package.clone(), Vec::new())),
        ExpandedUnitIdentity::PackageModule { package, path } => {
            Some((package.clone(), path.components().to_vec()))
        }
    }
}

fn discover_inline_modules(
    environment: &mut CompileTimeEnvironment,
    parent: CompileTimeModuleId,
    container: &SyntaxNode,
) {
    for item in direct_items(container) {
        if item.kind != SyntaxKind::Module {
            continue;
        }
        let Some((name, _)) = name_after_keyword(item, Keyword::Mod) else {
            continue;
        };
        let parent_module = &environment.modules[parent.index()];
        let mut path = parent_module.path.clone();
        path.push(name);
        let module =
            environment.ensure_module(parent_module.package.clone(), path, Some(item.span));
        discover_inline_modules(environment, module, item);
    }
}

fn collect_container(
    environment: &mut CompileTimeEnvironment,
    module: CompileTimeModuleId,
    container: &SyntaxNode,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for item in direct_items(container) {
        if item.kind == SyntaxKind::Module {
            if let Some((name, _)) = name_after_keyword(item, Keyword::Mod) {
                let parent = &environment.modules[module.index()];
                let mut path = parent.path.clone();
                path.push(name);
                if let Some(child) = environment.module(&parent.package, &path) {
                    collect_container(environment, child, item, diagnostics);
                }
            }
            continue;
        }
        if let Some(namespace) = declaration_namespace(item.kind) {
            collect_declaration(environment, module, namespace, item, diagnostics);
        } else if item.kind == SyntaxKind::Use {
            collect_import(environment, module, item, diagnostics);
        }
    }
}

fn collect_declaration(
    environment: &mut CompileTimeEnvironment,
    module: CompileTimeModuleId,
    namespace: CompileTimeNamespace,
    node: &SyntaxNode,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let keyword = namespace_keyword(namespace);
    let path = path_after_keyword(node, keyword);
    let Some(name) = path.last().cloned() else {
        return;
    };
    let span = last_name_after_keyword(node, keyword).map_or(node.span, |(_, span)| span);
    let id = CompileTimeDeclarationId(environment.declarations.len() as u32);
    let trait_path = if namespace == CompileTimeNamespace::Derive {
        path
    } else {
        Vec::new()
    };
    environment.declarations.push(CompileTimeDeclaration {
        id,
        namespace,
        module,
        name: name.clone(),
        visibility: visibility(node),
        span,
        syntax: node.clone(),
        trait_path,
    });
    environment.insert_binding(
        module,
        namespace,
        name,
        CompileTimeBinding::Declaration(id),
        span,
        diagnostics,
    );
}

fn collect_import(
    environment: &mut CompileTimeEnvironment,
    module: CompileTimeModuleId,
    node: &SyntaxNode,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(namespace) = use_namespace(node) else {
        return;
    };
    let path = use_path(node);
    let Some(default_name) = path.iter().rev().find_map(|part| match part {
        CompileTimePathPart::Name(name) => Some(name.clone()),
        _ => None,
    }) else {
        return;
    };
    let name = use_alias(node).unwrap_or(default_name);
    let id = CompileTimeImportId(environment.imports.len() as u32);
    environment.imports.push(CompileTimeImport {
        id,
        namespace,
        module,
        name: name.clone(),
        visibility: visibility(node),
        span: node.span,
        path,
        target: None,
        state: ImportState::Unresolved,
    });
    environment.insert_binding(
        module,
        namespace,
        name,
        CompileTimeBinding::Import(id),
        node.span,
        diagnostics,
    );
}

fn resolve_import(
    environment: &mut CompileTimeEnvironment,
    graph: &PackageGraph,
    id: CompileTimeImportId,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<CompileTimeDeclarationId> {
    match environment.imports[id.index()].state {
        ImportState::Resolved => return environment.imports[id.index()].target,
        ImportState::Failed => return None,
        ImportState::Resolving => {
            let span = environment.imports[id.index()].span;
            diagnostics.push(
                Diagnostic::new(
                    Category::NameResolution,
                    "compile-time imports form a cycle without reaching a declaration",
                )
                .with_primary(span),
            );
            environment.imports[id.index()].state = ImportState::Failed;
            return None;
        }
        ImportState::Unresolved => {}
    }

    environment.imports[id.index()].state = ImportState::Resolving;
    let import = environment.imports[id.index()].clone();
    let result = resolve_path(environment, graph, &import, diagnostics);
    if let Some(target) = result {
        if import.visibility == CompileTimeVisibility::Public
            && environment.declarations[target.index()].visibility != CompileTimeVisibility::Public
        {
            diagnostics.push(
                Diagnostic::new(
                    Category::Visibility,
                    format!(
                        "a public `use {}` cannot re-export a package-private declaration",
                        import.namespace.description()
                    ),
                )
                .with_primary(import.span)
                .with_related(
                    environment.declarations[target.index()].span,
                    "package-private declaration is here",
                ),
            );
            environment.imports[id.index()].state = ImportState::Failed;
            return None;
        }
        environment.imports[id.index()].target = Some(target);
        environment.imports[id.index()].state = ImportState::Resolved;
        Some(target)
    } else {
        environment.imports[id.index()].state = ImportState::Failed;
        None
    }
}

fn resolve_path(
    environment: &mut CompileTimeEnvironment,
    graph: &PackageGraph,
    import: &CompileTimeImport,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<CompileTimeDeclarationId> {
    let source_module = &environment.modules[import.module.index()];
    let source_package = source_module.package.clone();
    let mut module = import.module;
    let mut index = 0usize;

    match import.path.first()? {
        CompileTimePathPart::Root => {
            module = environment.module(&source_package, &[])?;
            index = 1;
        }
        CompileTimePathPart::SelfModule => index = 1,
        CompileTimePathPart::Super => {
            module = source_module.parent?;
            index = 1;
        }
        CompileTimePathPart::Name(name) => {
            if let Some(package) = graph.dependency(&source_package, name) {
                module = environment.module(package, &[])?;
                index = 1;
            }
        }
    }

    while index + 1 < import.path.len() {
        let CompileTimePathPart::Name(name) = &import.path[index] else {
            diagnostics.push(
                Diagnostic::new(
                    Category::NameResolution,
                    "a path root keyword may appear only as the first component",
                )
                .with_primary(import.span),
            );
            return None;
        };
        let current = &environment.modules[module.index()];
        let mut path = current.path.clone();
        path.push(name.clone());
        let Some(next) = environment.module(&current.package, &path) else {
            unresolved_import(import, diagnostics);
            return None;
        };
        module = next;
        index += 1;
    }

    let CompileTimePathPart::Name(name) = import.path.get(index)? else {
        unresolved_import(import, diagnostics);
        return None;
    };
    let Some(binding) = environment.lookup(module, import.namespace, name) else {
        unresolved_import(import, diagnostics);
        return None;
    };
    let binding_is_external = environment.modules[module.index()].package != source_package;
    if binding_is_external {
        let binding_visibility = match binding {
            CompileTimeBinding::Declaration(target) => {
                environment.declarations[target.index()].visibility
            }
            CompileTimeBinding::Import(other) => environment.imports[other.index()].visibility,
        };
        if binding_visibility != CompileTimeVisibility::Public {
            diagnostics.push(
                Diagnostic::new(
                    Category::Visibility,
                    format!(
                        "{} `{name}` is package-private",
                        import.namespace.description()
                    ),
                )
                .with_primary(import.span)
                .with_related(
                    environment.binding_span(binding),
                    "package-private binding is here",
                ),
            );
            return None;
        }
    }
    let target = match binding {
        CompileTimeBinding::Declaration(target) => target,
        CompileTimeBinding::Import(other) => {
            resolve_import(environment, graph, other, diagnostics)?
        }
    };
    let target_declaration = &environment.declarations[target.index()];
    if target_declaration.module != import.module
        && environment.modules[target_declaration.module.index()].package != source_package
        && target_declaration.visibility != CompileTimeVisibility::Public
    {
        diagnostics.push(
            Diagnostic::new(
                Category::Visibility,
                format!(
                    "{} `{}` is package-private",
                    import.namespace.description(),
                    target_declaration.name
                ),
            )
            .with_primary(import.span)
            .with_related(
                target_declaration.span,
                "package-private declaration is here",
            ),
        );
        return None;
    }
    Some(target)
}

fn unresolved_import(import: &CompileTimeImport, diagnostics: &mut Vec<Diagnostic>) {
    diagnostics.push(
        Diagnostic::new(
            Category::NameResolution,
            format!(
                "cannot resolve {} import `{}`",
                import.namespace.description(),
                import.name
            ),
        )
        .with_primary(import.span),
    );
}

fn declaration_namespace(kind: SyntaxKind) -> Option<CompileTimeNamespace> {
    match kind {
        SyntaxKind::MacroDeclaration => Some(CompileTimeNamespace::Macro),
        SyntaxKind::AttributeDeclaration => Some(CompileTimeNamespace::Attribute),
        SyntaxKind::DeriveDeclaration => Some(CompileTimeNamespace::Derive),
        _ => None,
    }
}

fn namespace_keyword(namespace: CompileTimeNamespace) -> Keyword {
    match namespace {
        CompileTimeNamespace::Macro => Keyword::Macro,
        CompileTimeNamespace::Attribute => Keyword::Attr,
        CompileTimeNamespace::Derive => Keyword::Derive,
    }
}

fn direct_items(node: &SyntaxNode) -> Vec<&SyntaxNode> {
    let container = if matches!(node.kind, SyntaxKind::Module) {
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

fn name_after_keyword(node: &SyntaxNode, keyword: Keyword) -> Option<(String, Span)> {
    let mut saw_keyword = false;
    for child in &node.children {
        match child {
            SyntaxElement::Token(token) if token.kind == TokenKind::Keyword(keyword) => {
                saw_keyword = true;
            }
            SyntaxElement::Token(token) if saw_keyword => {
                if let TokenKind::Identifier(name) = &token.kind {
                    return Some((name.clone(), token.span));
                }
            }
            SyntaxElement::Node(_) if saw_keyword => break,
            _ => {}
        }
    }
    None
}

fn last_name_after_keyword(node: &SyntaxNode, keyword: Keyword) -> Option<(String, Span)> {
    let mut saw_keyword = false;
    let mut found = None;
    for child in &node.children {
        match child {
            SyntaxElement::Token(token) if token.kind == TokenKind::Keyword(keyword) => {
                saw_keyword = true;
            }
            SyntaxElement::Token(token) if saw_keyword => {
                if let TokenKind::Identifier(name) = &token.kind {
                    found = Some((name.clone(), token.span));
                }
            }
            SyntaxElement::Node(_) if saw_keyword => break,
            _ => {}
        }
    }
    found
}

fn path_after_keyword(node: &SyntaxNode, keyword: Keyword) -> Vec<String> {
    let mut saw_keyword = false;
    let mut path = Vec::new();
    for child in &node.children {
        match child {
            SyntaxElement::Token(token) if token.kind == TokenKind::Keyword(keyword) => {
                saw_keyword = true;
            }
            SyntaxElement::Token(token) if saw_keyword => match &token.kind {
                TokenKind::Identifier(name) => path.push(name.clone()),
                TokenKind::Keyword(
                    Keyword::Root | Keyword::SelfValue | Keyword::SelfType | Keyword::Super,
                ) => path.push(token_text(&token.kind)),
                _ => {}
            },
            SyntaxElement::Node(_) if saw_keyword => break,
            _ => {}
        }
    }
    path
}

fn use_namespace(node: &SyntaxNode) -> Option<CompileTimeNamespace> {
    node.children.iter().find_map(|child| match child {
        SyntaxElement::Token(token) => match token.kind {
            TokenKind::Keyword(Keyword::Macro) => Some(CompileTimeNamespace::Macro),
            TokenKind::Keyword(Keyword::Attr) => Some(CompileTimeNamespace::Attribute),
            TokenKind::Keyword(Keyword::Derive) => Some(CompileTimeNamespace::Derive),
            _ => None,
        },
        SyntaxElement::Node(_) => None,
    })
}

fn use_path(node: &SyntaxNode) -> Vec<CompileTimePathPart> {
    let mut saw_namespace = false;
    let mut path = Vec::new();
    for child in &node.children {
        let SyntaxElement::Token(token) = child else {
            continue;
        };
        if !saw_namespace {
            saw_namespace = matches!(
                token.kind,
                TokenKind::Keyword(Keyword::Macro | Keyword::Attr | Keyword::Derive)
            );
            continue;
        }
        match &token.kind {
            TokenKind::Keyword(Keyword::As) => break,
            TokenKind::Keyword(Keyword::Root) => path.push(CompileTimePathPart::Root),
            TokenKind::Keyword(Keyword::SelfValue) => path.push(CompileTimePathPart::SelfModule),
            TokenKind::Keyword(Keyword::Super) => path.push(CompileTimePathPart::Super),
            TokenKind::Identifier(name) => path.push(CompileTimePathPart::Name(name.clone())),
            _ => {}
        }
    }
    path
}

fn use_alias(node: &SyntaxNode) -> Option<String> {
    let mut saw_as = false;
    for child in &node.children {
        let SyntaxElement::Token(token) = child else {
            continue;
        };
        if saw_as {
            if let TokenKind::Identifier(name) = &token.kind {
                return Some(name.clone());
            }
        }
        saw_as = token.kind == TokenKind::Keyword(Keyword::As);
    }
    None
}

fn visibility(node: &SyntaxNode) -> CompileTimeVisibility {
    if node.children.iter().any(|child| {
        matches!(
            child,
            SyntaxElement::Token(token)
                if token.kind == TokenKind::Keyword(Keyword::Pub)
        )
    }) {
        CompileTimeVisibility::Public
    } else {
        CompileTimeVisibility::Package
    }
}

fn token_text(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Keyword(Keyword::Root) => "root",
        TokenKind::Keyword(Keyword::SelfValue) => "self",
        TokenKind::Keyword(Keyword::SelfType) => "Self",
        TokenKind::Keyword(Keyword::Super) => "super",
        _ => "",
    }
    .to_string()
}
