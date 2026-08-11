//! Compatibility storage-promotion analysis (`docs/roadmap.md` Milestone 10).
//!
//! The 0.10 compatibility model permits a reference to escape its source
//! frame, so address-taken locals retain stable process-lifetime storage.
//! Owned programs prove borrow provenance and keep the same locals on stack.
//!
//! This pass now answers one compatibility-only question per function — which
//! locals have their address taken. Because references point into their container's storage
//! rather than at a boxed subvalue (`docs/spec.md` §3.2), taking a reference through
//! a path promotes the *root* local of that path and nothing else; no
//! whole-program field analysis is involved.

use std::collections::BTreeSet;

use crate::config::SemanticRevision;
use crate::resolution::{LocalBindingId, NameTarget, ResolvedProgram};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, Token, TokenKind};

/// Locals whose address is taken somewhere in `body`, in stable identity
/// order.
///
/// The result is a `BTreeSet` because promotion decides storage layout, and
/// `docs/roadmap.md` §2.2 requires identical source to produce identical output.
#[must_use]
pub fn address_taken_locals(
    resolved: &ResolvedProgram,
    body: &SyntaxNode,
    semantic_revision: SemanticRevision,
) -> BTreeSet<LocalBindingId> {
    if semantic_revision.supports_owned_surface() {
        return BTreeSet::new();
    }
    let mut promoted = BTreeSet::new();
    collect(resolved, body, &mut promoted);
    promoted
}

fn collect(resolved: &ResolvedProgram, node: &SyntaxNode, promoted: &mut BTreeSet<LocalBindingId>) {
    if node.kind == SyntaxKind::UnaryExpression
        && let Some(operand) = reference_operand(node)
        && let Some(binding) = root_local(resolved, operand)
    {
        promoted.insert(binding);
    }
    if node.kind == SyntaxKind::ClosureCapture {
        let mut tokens = node.children.iter().filter_map(|child| match child {
            SyntaxElement::Token(token) => Some(token),
            SyntaxElement::Node(_) => None,
        });
        let first = tokens.next();
        if matches!(first.map(|token| &token.kind), Some(TokenKind::Amp)) {
            let source = tokens.find(|token| matches!(token.kind, TokenKind::Identifier(_)));
            if let Some(source) = source
                && let Some(reference) = resolved.reference_at(source.span)
                && let NameTarget::Local(binding) = reference.target
            {
                promoted.insert(binding);
            }
        }
    }
    for child in &node.children {
        if let SyntaxElement::Node(child) = child {
            collect(resolved, child, promoted);
        }
    }
}

/// The operand of a `&`/`&var` expression, or `None` for any other unary
/// operator.
fn reference_operand(node: &SyntaxNode) -> Option<&SyntaxNode> {
    let operator = node.children.iter().find_map(|child| match child {
        SyntaxElement::Token(token) => Some(token),
        SyntaxElement::Node(_) => None,
    })?;
    if !matches!(operator.kind, TokenKind::Amp) {
        return None;
    }
    node.children.iter().rev().find_map(|child| match child {
        SyntaxElement::Node(node) => Some(node.as_ref()),
        SyntaxElement::Token(_) => None,
    })
}

/// The local binding a place path is rooted at.
///
/// Field and index selection stay within the same storage, so they descend to
/// the same root. Everything else yields `None`, either because no local is
/// involved or because the target already has independently stable storage:
///
/// - a referenced composite literal (`&Point { .. }`) allocates its own cell;
/// - a dereference (`&*handle`) names the referent's storage, not a local;
/// - a call or computed expression is not addressable and is rejected by
///   `docs/spec.md` 3.2 before this analysis matters.
fn root_local(resolved: &ResolvedProgram, node: &SyntaxNode) -> Option<LocalBindingId> {
    match node.kind {
        SyntaxKind::NameExpression => {
            let token = first_token(node)?;
            match resolved.reference_at(token.span)?.target {
                NameTarget::Local(binding) => Some(binding),
                _ => None,
            }
        }
        SyntaxKind::MemberExpression
        | SyntaxKind::TupleFieldExpression
        | SyntaxKind::BracketExpression
        | SyntaxKind::ParenthesizedExpression => {
            let base = node.children.iter().find_map(|child| match child {
                SyntaxElement::Node(node) => Some(node.as_ref()),
                SyntaxElement::Token(_) => None,
            })?;
            root_local(resolved, base)
        }
        _ => None,
    }
}

fn first_token(node: &SyntaxNode) -> Option<&Token> {
    node.children.iter().find_map(|child| match child {
        SyntaxElement::Token(token) => Some(token),
        SyntaxElement::Node(_) => None,
    })
}
