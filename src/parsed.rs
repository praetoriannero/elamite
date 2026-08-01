//! Package-wide source loading, lexing, and parsing.
//!
//! This phase owns the complete token-preserving syntax input consumed by
//! expansion and resolution. Resolution deliberately does not read files or
//! invoke the lexer/parser itself.

use std::path::PathBuf;

use crate::diagnostics::{Category, Diagnostic};
use crate::lexer::lex;
use crate::package::{ModulePath, PackageGraph, PackageId};
use crate::parser::parse;
use crate::source::{FileId, SourceManager, Span};
use crate::syntax::{SyntaxNode, Token};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandardModule {
    Root,
    Io,
    Ffi,
    Testing,
    Thread,
    Sync,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedUnitIdentity {
    Standard(StandardModule),
    PackageRoot(PackageId),
    PackageModule {
        package: PackageId,
        path: ModulePath,
    },
}

#[derive(Debug)]
pub struct ParsedUnit {
    pub identity: ParsedUnitIdentity,
    pub path: PathBuf,
    pub file: FileId,
    pub span: Span,
    /// Complete physical source retained for lossless expansion token trees.
    pub source: String,
    pub tokens: Vec<Token>,
    pub tree: SyntaxNode,
}

impl ParsedUnit {
    #[must_use]
    pub fn is_standard(&self) -> bool {
        matches!(self.identity, ParsedUnitIdentity::Standard(_))
    }
}

#[derive(Debug, Default)]
pub struct ParsedPackage {
    pub units: Vec<ParsedUnit>,
}

pub struct ParsedPackageOutput {
    pub package: ParsedPackage,
    pub diagnostics: Vec<Diagnostic>,
}

/// Loads and parses all shipped and package sources in deterministic order.
#[must_use]
pub fn parse_package(graph: &PackageGraph, sources: &mut SourceManager) -> ParsedPackageOutput {
    parse_package_inner(graph, sources, true)
}

/// Parses only user package sources for token/syntax inspection. The same
/// phase implementation is used, but shipped sources are omitted so these
/// source-only dumps retain their historical file identities and scope.
#[must_use]
pub fn parse_user_package(
    graph: &PackageGraph,
    sources: &mut SourceManager,
) -> ParsedPackageOutput {
    parse_package_inner(graph, sources, false)
}

fn parse_package_inner(
    graph: &PackageGraph,
    sources: &mut SourceManager,
    include_standard: bool,
) -> ParsedPackageOutput {
    let mut units = Vec::new();
    let mut diagnostics = Vec::new();

    if include_standard {
        for (identity, path, source) in [
            (
                ParsedUnitIdentity::Standard(StandardModule::Root),
                PathBuf::from("<std>/lib.elx"),
                crate::standard::ROOT_SOURCE,
            ),
            (
                ParsedUnitIdentity::Standard(StandardModule::Io),
                PathBuf::from("<std>/io.elx"),
                crate::standard::IO_SOURCE,
            ),
            (
                ParsedUnitIdentity::Standard(StandardModule::Ffi),
                PathBuf::from("<std>/ffi.elx"),
                crate::standard::FFI_SOURCE,
            ),
            (
                ParsedUnitIdentity::Standard(StandardModule::Testing),
                PathBuf::from("<std>/testing.elx"),
                crate::standard::TESTING_SOURCE,
            ),
            (
                ParsedUnitIdentity::Standard(StandardModule::Thread),
                PathBuf::from("<std>/thread.elx"),
                crate::standard::THREAD_SOURCE,
            ),
            (
                ParsedUnitIdentity::Standard(StandardModule::Sync),
                PathBuf::from("<std>/sync.elx"),
                crate::standard::SYNC_SOURCE,
            ),
        ] {
            let file = sources.add_text(path.clone(), source.to_string());
            parse_unit(identity, path, file, sources, &mut units, &mut diagnostics);
        }
    }

    let mut files = Vec::new();
    for (package_id, package) in &graph.packages {
        files.push((
            ParsedUnitIdentity::PackageRoot(package_id.clone()),
            package.manifest_dir.join(&package.manifest.root),
        ));
        for (path, file) in &package.modules {
            files.push((
                ParsedUnitIdentity::PackageModule {
                    package: package_id.clone(),
                    path: path.clone(),
                },
                file.clone(),
            ));
        }
    }
    files.sort_by(|left, right| left.1.cmp(&right.1));

    for (identity, path) in files {
        match sources.load_file(&path) {
            Ok(file) => parse_unit(identity, path, file, sources, &mut units, &mut diagnostics),
            Err(error) => {
                diagnostics.push(Diagnostic::new(Category::NameResolution, error.to_string()))
            }
        }
    }

    ParsedPackageOutput {
        package: ParsedPackage { units },
        diagnostics,
    }
}

fn parse_unit(
    identity: ParsedUnitIdentity,
    path: PathBuf,
    file: FileId,
    sources: &SourceManager,
    units: &mut Vec<ParsedUnit>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let source = sources.text(file);
    let span = Span::new(file, 0, u32::try_from(source.len()).unwrap_or(u32::MAX));
    let lexed = lex(file, source);
    diagnostics.extend(lexed.diagnostics);
    let parsed = parse(&lexed.tokens);
    diagnostics.extend(parsed.diagnostics);
    units.push(ParsedUnit {
        identity,
        path,
        file,
        span,
        source: source.to_string(),
        tokens: lexed.tokens,
        tree: parsed.tree,
    });
}
