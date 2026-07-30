//! Phase-neutral descriptions of compiler-selected standard operations.
//!
//! The checker selects these operations, IR preserves them, and the backend
//! lowers them. No one phase owns their shared vocabulary.

use crate::types::TypeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StandardCall {
    Panic,
    StringFrom,
    IdentityFrom { wrapper: TypeId },
    ForeignRootRetain { handle: TypeId, mutable: bool },
    ForeignRootPointer { handle: TypeId, mutable: bool },
    ForeignRootClose { handle: TypeId },
    FormatterWrite { formatter: TypeId },
    ArrayLen { collection: TypeId },
    ArrayGet { collection: TypeId },
    VecNew { collection: TypeId },
    VecLen { collection: TypeId },
    VecIsEmpty { collection: TypeId },
    VecGet { collection: TypeId },
    VecAppend { collection: TypeId },
    VecInsert { collection: TypeId },
    VecRemove { collection: TypeId },
    VecClear { collection: TypeId },
    MapNew { collection: TypeId },
    MapLen { collection: TypeId },
    MapIsEmpty { collection: TypeId },
    MapContainsKey { collection: TypeId },
    MapGet { collection: TypeId },
    MapInsert { collection: TypeId },
    MapRemove { collection: TypeId },
    MapClear { collection: TypeId },
    SetNew { collection: TypeId },
    SetLen { collection: TypeId },
    SetIsEmpty { collection: TypeId },
    SetContains { collection: TypeId },
    SetInsert { collection: TypeId },
    SetRemove { collection: TypeId },
    SetClear { collection: TypeId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NumericAlternative {
    pub name: &'static str,
    pub outcome: NumericOutcome,
    pub operator: NumericOperator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NumericOutcome {
    Checked,
    Wrapping,
    Saturating,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NumericOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Negate,
    ShiftLeft,
    ShiftRight,
}

impl NumericOperator {
    #[must_use]
    pub fn is_shift(self) -> bool {
        matches!(self, Self::ShiftLeft | Self::ShiftRight)
    }
}

impl NumericAlternative {
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        use NumericOperator::{
            Add, Divide, Multiply, Negate, Remainder, ShiftLeft, ShiftRight, Subtract,
        };
        use NumericOutcome::{Checked, Saturating, Wrapping};
        let (outcome, operator) = match name {
            "checked_add" => (Checked, Add),
            "checked_sub" => (Checked, Subtract),
            "checked_mul" => (Checked, Multiply),
            "checked_div" => (Checked, Divide),
            "checked_rem" => (Checked, Remainder),
            "checked_neg" => (Checked, Negate),
            "checked_shl" => (Checked, ShiftLeft),
            "checked_shr" => (Checked, ShiftRight),
            "wrapping_add" => (Wrapping, Add),
            "wrapping_sub" => (Wrapping, Subtract),
            "wrapping_mul" => (Wrapping, Multiply),
            "wrapping_div" => (Wrapping, Divide),
            "wrapping_rem" => (Wrapping, Remainder),
            "wrapping_neg" => (Wrapping, Negate),
            "wrapping_shl" => (Wrapping, ShiftLeft),
            "wrapping_shr" => (Wrapping, ShiftRight),
            "saturating_add" => (Saturating, Add),
            "saturating_sub" => (Saturating, Subtract),
            "saturating_mul" => (Saturating, Multiply),
            _ => return None,
        };
        let name = match outcome {
            Checked => match operator {
                Add => "checked_add",
                Subtract => "checked_sub",
                Multiply => "checked_mul",
                Divide => "checked_div",
                Remainder => "checked_rem",
                Negate => "checked_neg",
                ShiftLeft => "checked_shl",
                ShiftRight => "checked_shr",
            },
            Wrapping => match operator {
                Add => "wrapping_add",
                Subtract => "wrapping_sub",
                Multiply => "wrapping_mul",
                Divide => "wrapping_div",
                Remainder => "wrapping_rem",
                Negate => "wrapping_neg",
                ShiftLeft => "wrapping_shl",
                ShiftRight => "wrapping_shr",
            },
            Saturating => match operator {
                Add => "saturating_add",
                Subtract => "saturating_sub",
                _ => "saturating_mul",
            },
        };
        Some(Self {
            name,
            outcome,
            operator,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiverAdjustment {
    Pass,
    CopyValue,
    DereferenceAndCopy,
    BorrowShared,
    BorrowMutable,
}
