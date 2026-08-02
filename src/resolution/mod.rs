//! Declaration collection, imports, visibility, and lexical name-resolution façade.
//!
//! This is the owned identity database for `docs/ROADMAP.md` Milestone 4. It consumes
//! the package graph and Milestone 3 syntax trees, predeclares every module
//! item, then resolves imports and bodies without depending on source order.
//!
//! Naming convention: the surface declaration is spelled `use`, and the binding
//! it establishes is an *import*. Names that describe syntax (`Keyword::Use`,
//! `SyntaxKind::Use`) therefore say "use", while the semantic entities here
//! (`ImportId`, `Import`, `imports`) say "import" — `use` is a Rust keyword and
//! cannot name a binding or field.

mod bodies;
mod collect;
mod imports;
mod model;
mod visibility;

pub use model::*;

use lasso::{Rodeo, Spur};
use std::collections::{BTreeMap, BTreeSet};

use crate::config::CompilerFeatures;
use crate::diagnostics::{Category, Diagnostic};
use crate::expansion::{
    ExpandedPackage, ExpandedUnitIdentity, ExpansionOutput, expand, expand_with_features,
};
use crate::ident::is_valid_identifier;
use crate::package::{PackageGraph, PackageId};
use crate::parsed::{StandardModule, parse_package};
use crate::source::{FileId, SourceManager, Span};
use crate::syntax::{
    Keyword, SyntaxElement, SyntaxKind, SyntaxNode, Token, TokenKind, child_nodes,
};

use model::{Builtin, ImportState, NamespaceTarget, PathPart};

#[derive(Debug)]
struct ResolutionUnit {
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

/// Parses, expands, collects, and resolves every package source file.
#[must_use]
pub fn resolve(graph: &PackageGraph, sources: &mut SourceManager) -> ResolutionOutput {
    resolve_expanded(graph, expand(graph, parse_package(graph, sources)))
}

/// Resolves with explicit unstable-language feature opt-ins.
#[must_use]
pub fn resolve_with_features(
    graph: &PackageGraph,
    sources: &mut SourceManager,
    features: CompilerFeatures,
) -> ResolutionOutput {
    resolve_expanded(
        graph,
        expand_with_features(graph, parse_package(graph, sources), features),
    )
}

/// Resolves production declarations plus test bodies owned by the selected
/// root package. Dependency test bodies remain deliberately unresolved.
#[must_use]
pub fn resolve_for_tests(graph: &PackageGraph, sources: &mut SourceManager) -> ResolutionOutput {
    Resolver::new(graph, expand(graph, parse_package(graph, sources)), true).run()
}

/// Resolves selected test bodies with explicit unstable feature opt-ins.
#[must_use]
pub fn resolve_for_tests_with_features(
    graph: &PackageGraph,
    sources: &mut SourceManager,
    features: CompilerFeatures,
) -> ResolutionOutput {
    Resolver::new(
        graph,
        expand_with_features(graph, parse_package(graph, sources), features),
        true,
    )
    .run()
}

/// Collects and resolves one already expanded package.
#[must_use]
pub fn resolve_expanded(graph: &PackageGraph, expanded: ExpansionOutput) -> ResolutionOutput {
    Resolver::new(graph, expanded, false).run()
}

struct Resolver<'a> {
    graph: &'a PackageGraph,
    program: ResolvedProgram,
    diagnostics: Vec<Diagnostic>,
    expanded: ExpandedPackage,
    expanded_units: Vec<ResolutionUnit>,
    inline_units: Vec<ResolutionUnit>,
    resolve_test_bodies: bool,
}

impl<'a> Resolver<'a> {
    fn new(graph: &'a PackageGraph, expanded: ExpansionOutput, resolve_test_bodies: bool) -> Self {
        let ExpansionOutput {
            package: expanded,
            diagnostics,
        } = expanded;
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
            closures: Vec::new(),
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
            program,
            diagnostics,
            expanded,
            expanded_units: Vec::new(),
            inline_units: Vec::new(),
            resolve_test_bodies,
        }
    }

    fn run(mut self) -> ResolutionOutput {
        let (io_module, ffi_module, testing_module, thread_module, sync_module) =
            self.install_standard_library_names();
        self.create_file_module_graph();
        self.install_expanded_units(
            io_module,
            ffi_module,
            testing_module,
            thread_module,
            sync_module,
        );
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

    /// Returns the physical module context carried by one expanded token.
    /// Quote literals keep definition spans and interpolations keep invocation
    /// spans, so this implements hygiene without fabricating source offsets.
    fn syntax_context_module(&self, span: Span, fallback: ModuleId) -> ModuleId {
        self.program
            .modules
            .iter()
            .filter(|module| {
                module.span.is_some_and(|owner| {
                    owner.file == span.file && owner.start <= span.start && span.end <= owner.end
                })
            })
            .max_by_key(|module| {
                (
                    module.path.len(),
                    std::cmp::Reverse(module.span.map_or(u32::MAX, |span| span.end - span.start)),
                )
            })
            .map_or(fallback, |module| module.id)
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

fn binding_name_tokens(node: &SyntaxNode) -> Vec<&Token> {
    if let Some(pattern) = child_nodes(node)
        .into_iter()
        .find(|child| child.kind == SyntaxKind::TuplePattern)
    {
        let mut tokens = Vec::new();
        collect_binding_name_tokens(pattern, &mut tokens);
        return tokens;
    }
    node.children
        .iter()
        .filter_map(|child| match child {
            SyntaxElement::Token(
                token @ Token {
                    kind: TokenKind::Identifier(name),
                    ..
                },
            ) if name != "_" => Some(token),
            _ => None,
        })
        .take(1)
        .collect()
}

fn collect_binding_name_tokens<'a>(node: &'a SyntaxNode, tokens: &mut Vec<&'a Token>) {
    for child in &node.children {
        match child {
            SyntaxElement::Token(
                token @ Token {
                    kind: TokenKind::Identifier(name),
                    ..
                },
            ) if name != "_" => tokens.push(token),
            SyntaxElement::Node(child)
                if matches!(child.kind, SyntaxKind::TuplePattern | SyntaxKind::Pattern) =>
            {
                collect_binding_name_tokens(child, tokens);
            }
            _ => {}
        }
    }
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

fn use_path(node: &SyntaxNode, symbols: &mut Rodeo<Spur>) -> Option<(Vec<PathPart>, Symbol, Span)> {
    let mut saw_use = false;
    let mut saw_as = false;
    let mut parts = Vec::new();
    let mut last = None;
    let mut alias = None;
    for child in &node.children {
        let SyntaxElement::Token(token) = child else {
            continue;
        };
        match &token.kind {
            TokenKind::Keyword(Keyword::Use) => saw_use = true,
            TokenKind::Keyword(Keyword::As) if saw_use => saw_as = true,
            TokenKind::Identifier(name) if saw_use && saw_as => {
                alias = Some((Symbol(symbols.get_or_intern(name)), token.span));
            }
            TokenKind::Identifier(name) if saw_use => {
                let symbol = Symbol(symbols.get_or_intern(name));
                parts.push(PathPart::Name(symbol));
                last = Some((symbol, token.span));
            }
            TokenKind::Keyword(keyword) if saw_use => {
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
