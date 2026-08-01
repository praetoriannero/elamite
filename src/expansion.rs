//! Parsed-to-resolved macro expansion boundary.
//!
//! Compile-time execution remains disabled while the interpreter layers are
//! implemented. This phase consumes parsed units into its own unit identities,
//! constructs a lossless token-tree view beside each unchanged syntax tree,
//! and owns the detached versioned [`ast`] façade. Resolution consumes only
//! this owned result, so later rewriting can replace unit syntax without
//! reaching back into parsing.

pub mod ast;
mod engine;
pub mod fragment;
pub mod identity;
pub mod interpreter;
pub mod namespace;
pub mod provenance;
pub mod quote;
pub mod scheduler;
pub mod token_tree;

use std::path::PathBuf;

use crate::config::CompilerFeatures;
use crate::package::PackageGraph;
use crate::package::{ModulePath, PackageId};
use crate::parsed::StandardModule;
use crate::parsed::{ParsedPackage, ParsedPackageOutput, ParsedUnit, ParsedUnitIdentity};
use crate::source::{FileId, Span};
use crate::syntax::{SyntaxNode, Token};
use provenance::ProvenanceTable;
use token_tree::{TokenTreeUnit, build_token_trees};

/// Stable identity of one source unit after it crosses the expansion boundary.
///
/// This deliberately does not reuse [`ParsedUnitIdentity`]: later expansion
/// work may add generated units or other expansion-owned identity data without
/// making resolution depend on the parsing phase's package representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpandedUnitIdentity {
    Standard(StandardModule),
    PackageRoot(PackageId),
    PackageModule {
        package: PackageId,
        path: ModulePath,
    },
}

impl From<ParsedUnitIdentity> for ExpandedUnitIdentity {
    fn from(identity: ParsedUnitIdentity) -> Self {
        match identity {
            ParsedUnitIdentity::Standard(module) => Self::Standard(module),
            ParsedUnitIdentity::PackageRoot(package) => Self::PackageRoot(package),
            ParsedUnitIdentity::PackageModule { package, path } => {
                Self::PackageModule { package, path }
            }
        }
    }
}

#[derive(Debug)]
pub struct ExpandedUnit {
    pub identity: ExpandedUnitIdentity,
    pub path: PathBuf,
    pub file: FileId,
    pub span: Span,
    pub tokens: Vec<Token>,
    pub token_trees: TokenTreeUnit,
    pub tree: SyntaxNode,
}

impl ExpandedUnit {
    #[must_use]
    pub fn is_standard(&self) -> bool {
        matches!(self.identity, ExpandedUnitIdentity::Standard(_))
    }
}

#[derive(Debug)]
pub struct ExpandedPackage {
    pub units: Vec<ExpandedUnit>,
    pub provenance: ProvenanceTable,
    /// Versioned, compile-time-only structural syntax interface. The value is
    /// deliberately independent of the compiler's parsed and resolved trees.
    pub ast: ast::AstInterface,
    pub compile_time: namespace::CompileTimeEnvironment,
    pub schedule: scheduler::ExpansionScheduler,
    /// Stable compiler/spec/interface/source identity used by compile-time
    /// metadata and future persistent caches.
    pub identities: identity::ExpansionIdentities,
}

impl ExpandedPackage {
    /// Creates a `std.ast` builder whose products inherit one represented
    /// token's origin. Interpreter and quote lowering use the equivalent
    /// expansion-local path for generated inputs.
    #[must_use]
    pub fn ast_builder_for_token(&self, token: &token_tree::TokenTreeToken) -> ast::AstBuilder {
        // Looking the identity up catches an origin from an unrelated table at
        // this compiler-owned boundary before it can enter an AST value.
        let _ = self.provenance.origin(token.origin);
        self.ast
            .builder(ast::OriginHandle::from_range(provenance::OriginRange::new(
                token.origin,
                token.origin,
            )))
    }

    /// Deterministic expanded-syntax and execution inspection.
    #[must_use]
    pub fn dump(&self) -> String {
        let mut output = format!(
            "expansion package={} interface={}\n",
            self.identities.package, self.identities.interface
        );
        for (index, identity) in self.identities.declarations.iter().enumerate() {
            output.push_str(&format!("declaration {index} {identity}\n"));
        }
        for unit in &self.units {
            output.push_str(&format!(
                "file {} {}\n",
                unit.file.index(),
                unit.path.display()
            ));
            for line in unit.tree.dump().lines() {
                output.push_str("  ");
                output.push_str(line);
                output.push('\n');
            }
        }
        for work in self.schedule.work() {
            output.push_str(&format!(
                "execution {} declaration={} role={:?} state={:?} depth={} location={:?} resources={:?}\n",
                work.id.index(),
                work.request.declaration.index(),
                work.request.role,
                work.state,
                work.depth,
                work.request.location,
                work.resources
            ));
        }
        output.push_str(&format!(
            "provenance origins={} expansions={}\n",
            self.provenance.origins().len(),
            self.provenance.expansions().len()
        ));
        output
    }
}

pub struct ExpansionOutput {
    pub package: ExpandedPackage,
    pub diagnostics: Vec<crate::diagnostics::Diagnostic>,
}

#[must_use]
pub fn expand(graph: &PackageGraph, parsed: ParsedPackageOutput) -> ExpansionOutput {
    expand_with_features(graph, parsed, CompilerFeatures::default())
}

#[must_use]
pub fn expand_with_features(
    graph: &PackageGraph,
    parsed: ParsedPackageOutput,
    _features: CompilerFeatures,
) -> ExpansionOutput {
    expand_with_limits(graph, parsed, scheduler::ExpansionLimits::default())
}

/// Expands with explicit deterministic budgets. Compiler entry points use the
/// normative defaults; tests and embedding tools may choose smaller budgets
/// without changing language semantics below those limits.
#[must_use]
pub fn expand_with_limits(
    graph: &PackageGraph,
    parsed: ParsedPackageOutput,
    limits: scheduler::ExpansionLimits,
) -> ExpansionOutput {
    let mut output = expand_units(parsed);
    output.package.schedule = scheduler::ExpansionScheduler::with_limits(limits);
    output.package.compile_time =
        namespace::collect(graph, &output.package.units, &mut output.diagnostics);
    quote::validate(&output.package.units, &mut output.diagnostics);
    engine::execute_package(
        graph,
        &mut output.package.units,
        &output.package.compile_time,
        &mut output.package.schedule,
        &mut output.package.provenance,
        &mut output.diagnostics,
    );
    output.package.identities =
        identity::calculate(&output.package.units, &output.package.compile_time);
    output
}

fn expand_units(parsed: ParsedPackageOutput) -> ExpansionOutput {
    let ParsedPackageOutput {
        package: ParsedPackage { units },
        diagnostics,
    } = parsed;
    let mut provenance = ProvenanceTable::new();
    let units = units
        .into_iter()
        .map(|unit| expand_unit(unit, &mut provenance))
        .collect();
    ExpansionOutput {
        package: ExpandedPackage {
            units,
            provenance,
            ast: ast::AstInterface::current(),
            compile_time: namespace::CompileTimeEnvironment::default(),
            schedule: scheduler::ExpansionScheduler::new(),
            identities: identity::ExpansionIdentities::default(),
        },
        diagnostics,
    }
}

fn expand_unit(unit: ParsedUnit, provenance: &mut ProvenanceTable) -> ExpandedUnit {
    let ParsedUnit {
        identity,
        path,
        file,
        span,
        source,
        tokens,
        tree,
    } = unit;
    let token_trees = build_token_trees(span, source, &tokens, provenance);
    ExpandedUnit {
        identity: identity.into(),
        path,
        file,
        span,
        tokens,
        token_trees,
        tree,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expansion::ast::{HasOrigin, INTERFACE_VERSION};
    use crate::lexer::lex;
    use crate::parsed::StandardModule;
    use crate::parser::parse;
    use crate::source::SourceManager;

    fn expand_source(source: &str) -> ExpansionOutput {
        let path = PathBuf::from("main.elx");
        let mut sources = SourceManager::new();
        let file = sources.add_text(path.clone(), source.to_string());
        let span = Span::new(file, 0, source.len() as u32);
        let lexed = lex(file, source);
        assert!(lexed.diagnostics.is_empty(), "{:?}", lexed.diagnostics);
        let parsed = parse(&lexed.tokens);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

        expand_units(ParsedPackageOutput {
            package: ParsedPackage {
                units: vec![ParsedUnit {
                    identity: ParsedUnitIdentity::Standard(StandardModule::Root),
                    path,
                    file,
                    span,
                    source: source.to_string(),
                    tokens: lexed.tokens,
                    tree: parsed.tree,
                }],
            },
            diagnostics: Vec::new(),
        })
    }

    #[test]
    fn expansion_adds_token_trees_without_changing_parsed_syntax() {
        let source = "fn main() -> ():\n    println((\"hello\", [1, 2]))\n";
        let path = PathBuf::from("main.elx");
        let mut sources = SourceManager::new();
        let file = sources.add_text(path.clone(), source.to_string());
        let span = Span::new(file, 0, source.len() as u32);
        let lexed = lex(file, source);
        assert!(lexed.diagnostics.is_empty(), "{:?}", lexed.diagnostics);
        let parsed = parse(&lexed.tokens);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let expected_tree = parsed.tree.clone();
        let expected_tokens = lexed.tokens.clone();

        let output = expand_units(ParsedPackageOutput {
            package: ParsedPackage {
                units: vec![ParsedUnit {
                    identity: ParsedUnitIdentity::Standard(StandardModule::Root),
                    path,
                    file,
                    span,
                    source: source.to_string(),
                    tokens: lexed.tokens,
                    tree: parsed.tree,
                }],
            },
            diagnostics: Vec::new(),
        });

        assert!(output.diagnostics.is_empty());
        let unit = &output.package.units[0];
        assert_eq!(unit.tree, expected_tree);
        assert_eq!(unit.tokens, expected_tokens);
        assert_eq!(
            unit.token_trees
                .flattened()
                .into_iter()
                .map(|token| (
                    &token.kind,
                    output.package.provenance.physical_span(token.origin)
                ))
                .collect::<Vec<_>>(),
            expected_tokens
                .iter()
                .map(|token| (&token.kind, Some(token.span)))
                .collect::<Vec<_>>()
        );
        assert_eq!(output.package.ast.version(), INTERFACE_VERSION);
        let token = unit.token_trees.flattened()[0].clone();
        let builder = output.package.ast_builder_for_token(&token);
        let identifier = builder.identifier("generated_name").unwrap();
        let expression = builder.identifier_expression(identifier.clone());
        assert_eq!(expression.origin(), identifier.origin());
        assert_eq!(
            output.package.provenance.physical_span(token.origin),
            output
                .package
                .provenance
                .physical_range(expression.origin().range())
        );
    }

    #[test]
    fn repeated_expansion_assigns_identical_origins() {
        let source = "fn main() -> ():\n    println((\"hello\", [1, 2]))\n";
        let first = expand_source(source);
        let second = expand_source(source);

        assert_eq!(first.package.provenance, second.package.provenance);
        assert_eq!(
            first.package.units[0].token_trees,
            second.package.units[0].token_trees
        );
        assert_eq!(
            first.package.units[0]
                .token_trees
                .flattened()
                .into_iter()
                .map(|token| token.origin.index())
                .collect::<Vec<_>>(),
            (0..first.package.provenance.origins().len()).collect::<Vec<_>>()
        );
    }
}
