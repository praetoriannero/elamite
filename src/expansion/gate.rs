//! Experimental feature gating at the physical expansion boundary.

use crate::config::CompilerFeatures;
use crate::diagnostics::{Category, Diagnostic};
use crate::syntax::{Keyword, SyntaxElement, SyntaxKind, SyntaxNode, TokenKind};

use super::ExpandedUnit;

pub(super) fn validate(
    units: &[ExpandedUnit],
    features: CompilerFeatures,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if features.unstable_macros {
        return;
    }
    for unit in units.iter().filter(|unit| !unit.is_standard()) {
        validate_node(&unit.tree, diagnostics);
    }
}

fn validate_node(node: &SyntaxNode, diagnostics: &mut Vec<Diagnostic>) {
    let feature = match node.kind {
        SyntaxKind::MacroDeclaration => Some("user-defined macro declarations"),
        SyntaxKind::AttributeDeclaration => Some("user-defined attribute declarations"),
        SyntaxKind::DeriveDeclaration => Some("user-defined derive declarations"),
        SyntaxKind::Use if is_compile_time_import(node) => Some("compile-time namespace imports"),
        SyntaxKind::Attribute => gated_attachment(node),
        SyntaxKind::MacroExpression if is_user_macro_invocation(node) => {
            Some("user-defined macro invocations")
        }
        SyntaxKind::QuoteExpression => Some("compile-time quotation"),
        _ => None,
    };
    if let Some(feature) = feature {
        diagnostics.push(
            Diagnostic::new(
                Category::ExperimentalFeature,
                format!("{feature} require `--unstable-macros`"),
            )
            .with_primary(node.span),
        );
    }

    // One declaration-level diagnostic is sufficient while the entire body
    // is gated. This also avoids noisy diagnostics for helper invocations in
    // compile-time code that cannot run yet.
    if matches!(
        node.kind,
        SyntaxKind::MacroDeclaration
            | SyntaxKind::AttributeDeclaration
            | SyntaxKind::DeriveDeclaration
    ) {
        return;
    }
    for child in &node.children {
        if let SyntaxElement::Node(child) = child {
            validate_node(child, diagnostics);
        }
    }
}

fn is_compile_time_import(node: &SyntaxNode) -> bool {
    node.children.iter().any(|child| {
        matches!(
            child,
            SyntaxElement::Token(token)
                if matches!(
                    token.kind,
                    TokenKind::Keyword(Keyword::Macro | Keyword::Attr | Keyword::Derive)
                )
        )
    })
}

fn gated_attachment(node: &SyntaxNode) -> Option<&'static str> {
    node.children.iter().find_map(|child| match child {
        SyntaxElement::Token(token) => match token.kind {
            TokenKind::Keyword(Keyword::Attr) => Some("user-defined attributes"),
            TokenKind::Keyword(Keyword::Derive) => Some("user-defined derives"),
            _ => None,
        },
        SyntaxElement::Node(_) => None,
    })
}

fn is_user_macro_invocation(node: &SyntaxNode) -> bool {
    node.children
        .iter()
        .find_map(|child| match child {
            SyntaxElement::Token(token) => match &token.kind {
                TokenKind::Identifier(name) => Some(name.as_str()),
                _ => None,
            },
            SyntaxElement::Node(_) => None,
        })
        .is_some_and(|name| !matches!(name, "vec" | "map" | "set"))
}
