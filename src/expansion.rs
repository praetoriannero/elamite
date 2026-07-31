//! Parsed-to-resolved macro expansion boundary.
//!
//! Compile-time execution remains disabled while **Macro expansion
//! foundations** is implemented. This phase consumes parsed units into its own
//! unit identities and constructs a lossless token-tree view beside each
//! unchanged syntax tree. Resolution consumes only this owned result, so later
//! rewriting can replace unit syntax without reaching back into parsing.

pub mod fragment;
pub mod provenance;
pub mod token_tree;

use std::path::PathBuf;

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
}

pub struct ExpansionOutput {
    pub package: ExpandedPackage,
    pub diagnostics: Vec<crate::diagnostics::Diagnostic>,
}

#[must_use]
pub fn expand(parsed: ParsedPackageOutput) -> ExpansionOutput {
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
        package: ExpandedPackage { units, provenance },
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

        expand(ParsedPackageOutput {
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

        let output = expand(ParsedPackageOutput {
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
