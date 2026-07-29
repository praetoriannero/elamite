//! Public API documentation extraction.
//!
//! Extraction reads attached syntax and declaration visibility from the
//! resolver's owned database. It deliberately does not require canonical
//! types, checked bodies, lowering, or code generation, so an unrelated
//! private-body error cannot erase otherwise valid public documentation.

use crate::lexer::TokenKind;
use crate::package::PackageId;
use crate::parser::{SyntaxElement, SyntaxKind, SyntaxNode};
use crate::resolution::{DeclarationKind, ResolvedProgram};
use crate::source::{SourceManager, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiItem {
    pub path: String,
    pub kind: DeclarationKind,
    pub signature: String,
    pub documentation: String,
    pub source_path: String,
    pub source_line: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApiDocumentation {
    pub items: Vec<ApiItem>,
}

impl ApiDocumentation {
    /// Deterministic Markdown suitable for the CLI and snapshot tests.
    #[must_use]
    pub fn markdown(&self) -> String {
        let mut output = String::from("# Public API\n\n");
        for item in &self.items {
            output.push_str(&format!("## `{}`\n\n", item.path));
            if !item.documentation.is_empty() {
                output.push_str(&item.documentation);
                output.push_str("\n\n");
            }
            output.push_str("```elamite\n");
            output.push_str(&item.signature);
            output.push_str("\n```\n\n");
            output.push_str(&format!(
                "Source: `{}`:{}\n\n",
                item.source_path, item.source_line
            ));
        }
        output
    }
}

/// Extracts externally reachable declarations belonging to `package`.
#[must_use]
pub fn extract(
    resolved: &ResolvedProgram,
    sources: &SourceManager,
    package: &PackageId,
) -> ApiDocumentation {
    let mut items = resolved
        .declarations
        .iter()
        .filter(|declaration| {
            declaration.externally_reachable
                && resolved.modules[declaration.module.index()]
                    .package
                    .as_ref()
                    == Some(package)
        })
        .map(|declaration| {
            let module = &resolved.modules[declaration.module.index()];
            let mut components = if module.path.is_empty() {
                vec!["root".to_string()]
            } else {
                std::iter::once("root".to_string())
                    .chain(
                        module
                            .path
                            .iter()
                            .map(|part| resolved.symbol_text(*part).to_string()),
                    )
                    .collect()
            };
            if let Some(parent) = declaration.parent_declaration {
                components.push(
                    resolved
                        .symbol_text(resolved.declarations[parent.index()].name)
                        .to_string(),
                );
            }
            components.push(resolved.symbol_text(declaration.name).to_string());
            let position = sources.line_col(declaration.span.file, declaration.span.start);
            ApiItem {
                path: components.join("."),
                kind: declaration.kind,
                signature: declaration_signature(&declaration.syntax, sources),
                documentation: documentation(&declaration.syntax),
                source_path: sources.path(declaration.span.file).display().to_string(),
                source_line: position.line,
            }
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| left.path.cmp(&right.path));
    ApiDocumentation { items }
}

fn documentation(node: &SyntaxNode) -> String {
    let mut lines = Vec::new();
    for child in &node.children {
        let SyntaxElement::Node(child) = child else {
            continue;
        };
        if child.kind != SyntaxKind::Documentation {
            continue;
        }
        for element in &child.children {
            if let SyntaxElement::Token(token) = element
                && let TokenKind::DocComment(text) = &token.kind
            {
                lines.push(text.trim().to_string());
            }
        }
    }
    lines.join("\n")
}

pub(crate) fn declaration_signature(node: &SyntaxNode, sources: &SourceManager) -> String {
    fn bounds(node: &SyntaxNode, first: &mut Option<Span>, last: &mut Option<Span>) -> bool {
        for child in &node.children {
            match child {
                SyntaxElement::Node(child) if child.kind == SyntaxKind::Documentation => {}
                SyntaxElement::Node(child) => {
                    if bounds(child, first, last) {
                        return true;
                    }
                }
                SyntaxElement::Token(token) => {
                    if matches!(token.kind, TokenKind::Indent) {
                        return true;
                    }
                    if matches!(
                        token.kind,
                        TokenKind::Newline | TokenKind::Dedent | TokenKind::Eof
                    ) {
                        continue;
                    }
                    first.get_or_insert(token.span);
                    *last = Some(token.span);
                }
            }
        }
        false
    }

    let mut first = None;
    let mut last = None;
    bounds(node, &mut first, &mut last);
    match (first, last) {
        (Some(first), Some(last)) if first.file == last.file => sources
            .snippet(Span::new(first.file, first.start, last.end))
            .trim()
            .to_string(),
        _ => String::new(),
    }
}
