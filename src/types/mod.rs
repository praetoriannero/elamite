//! Canonical type-system façade, data, lowering, and literal inference.
//!
//! This module is the Milestone 5 boundary between name resolution and
//! expression checking. Every constructed type is interned in an owned arena;
//! source aliases remain visible in that arena for diagnostics, while exact
//! equivalence recursively compares their expanded targets.

pub mod context;
pub mod lower;
pub mod model;

pub use context::*;
pub use lower::{lower_annotation, resolve_types};
pub use model::*;

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostics::{Category, Diagnostic};
use crate::package::PackageId;
use crate::resolution::{
    BuiltinId, DeclarationId, DeclarationKind, FieldId, ForeignDirection, GenericParameterId,
    ImplId, ItemId, NameTarget, ResolvedProgram,
};
use crate::source::Span;
use crate::syntax::{
    Keyword, NumericSuffix, SyntaxElement, SyntaxKind, SyntaxNode, Token, TokenKind, direct_child,
    direct_child_not, direct_children, direct_path_span, direct_tokens,
};
