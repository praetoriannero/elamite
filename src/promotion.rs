//! Storage-promotion analysis (`docs/ROADMAP.md` Milestone 10).
//!
//! A safe reference names storage, so any local whose address is taken must
//! live in managed storage rather than on the C stack: the reference may
//! outlive the frame, and the collector must be able to reach the target.
//!
//! This pass answers one question per function — which locals have their
//! address taken — and is deliberately conservative, per `docs/ROADMAP.md` Milestone
//! 10: every such local is promoted, with precise escape analysis left to
//! **Post-conformance optimization**. Because references point into their container's storage
//! rather than at a boxed subvalue (`docs/SPEC.md` §3.2), taking a reference through
//! a path promotes the *root* local of that path and nothing else; no
//! whole-program field analysis is involved.

use std::collections::BTreeSet;

use crate::resolution::{LocalBindingId, NameTarget, ResolvedProgram};
use crate::syntax::{SyntaxElement, SyntaxKind, SyntaxNode, Token, TokenKind};

/// Locals whose address is taken somewhere in `body`, in stable identity
/// order.
///
/// The result is a `BTreeSet` because promotion decides storage layout, and
/// `docs/ROADMAP.md` §2.2 requires identical source to produce identical output.
#[must_use]
pub fn address_taken_locals(
    resolved: &ResolvedProgram,
    body: &SyntaxNode,
) -> BTreeSet<LocalBindingId> {
    let mut promoted = BTreeSet::new();
    collect(resolved, body, &mut promoted);
    promoted
}

/// Whether `body` forms a referenced composite literal, which allocates a
/// managed cell of its own even when no local is promoted.
#[must_use]
pub fn allocates_managed_temporary(body: &SyntaxNode) -> bool {
    if body.kind == SyntaxKind::UnaryExpression
        && let Some(operand) = reference_operand(body)
        && operand.kind == SyntaxKind::RecordExpression
    {
        return true;
    }
    body.children.iter().any(|child| match child {
        SyntaxElement::Node(child) => allocates_managed_temporary(child),
        SyntaxElement::Token(_) => false,
    })
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
/// involved or because the target is already managed:
///
/// - a referenced composite literal (`&Point { .. }`) allocates its own cell;
/// - a dereference (`&*handle`) names the referent's storage, not a local;
/// - a call or computed expression is not addressable and is rejected by
///   `docs/SPEC.md` 3.2 before this analysis matters.
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
