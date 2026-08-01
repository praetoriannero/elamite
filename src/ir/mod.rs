//! Typed high-level IR and explicit control-flow IR façade (`ROADMAP.md` Milestones
//! 8-11).
//!
//! The high-level form owns selected declaration and type identities while
//! retaining source spans, place classifications, and explicit logical-copy
//! markers from [`crate::check`]. The control-flow form then makes evaluation
//! order, temporaries, branches, loops, calls, returns, and potentially
//! trapping operations explicit before any C is emitted. Milestone 9 adds the
//! canonical logical-copy contract and source-ordered pattern-match lowering.

pub mod control_flow;
mod copy;
mod syntax_helpers;
mod traps;
pub mod typed;

pub use control_flow::*;
pub use typed::*;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::check::{CheckedCall, CheckedProgram, TraitObjectCoercion};
use crate::diagnostics::{Category, Diagnostic};
use crate::operations::{NumericAlternative, NumericOutcome, ReceiverAdjustment, StandardCall};
use crate::resolution::{
    DeclarationId, DeclarationKind, FieldId, ItemId, LocalBindingId, LocalBindingKind, MemberId,
    NameTarget, ResolvedProgram, VariantId,
};
use crate::source::Span;
use crate::syntax::{
    FormattedSegmentKind, Keyword, SyntaxElement, SyntaxKind, SyntaxNode, Token, TokenKind,
    child_nodes,
};
use crate::types::{
    FunctionInstance, PlaceKind, PrimitiveType, Substitution, TypeContext, TypeId, TypeKind,
    TypedProgram, parse_integer_magnitude,
};
pub use copy::{LogicalCopyStrategy, logical_copy_strategy};
use syntax_helpers::*;
pub use traps::TrapKind;

macro_rules! id_type {
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

id_type!(BlockId);
id_type!(TemporaryId);

#[derive(Debug, Clone, PartialEq)]
pub enum Constant {
    Unit,
    Bool(bool),
    Integer { magnitude: u128, negative: bool },
    Float(String),
    Character(char),
    String(String),
    Null,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    Positive,
    Negative,
    LogicalNot,
    BitwiseNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Add,
    Concatenate,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    BitAnd,
    BitOr,
    BitXor,
    ShiftLeft,
    ShiftRight,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    LogicalAnd,
    LogicalOr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentOperator {
    Assign,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    BitAnd,
    BitOr,
    BitXor,
    ShiftLeft,
    ShiftRight,
}
