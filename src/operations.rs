//! Phase-neutral descriptions of compiler-selected standard operations.
//!
//! The checker selects these operations, IR preserves them, and the backend
//! lowers them. No one phase owns their shared vocabulary.

use crate::resolution::DeclarationId;
use crate::types::TypeId;

/// Why the compiler materializes one source-level value copy.
///
/// This vocabulary is deliberately phase-neutral: checking records the
/// source-level reason, typed IR adds the allocation class, and control-flow
/// IR assigns an emitted-copy identity. Optimization passes can therefore
/// select copies without reconstructing intent from backend shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogicalCopyKind {
    Binding,
    Assignment,
    Return,
    Argument,
    AggregateElement,
    PatternBinding,
    Receiver,
    ClosureCapture,
    IterationSnapshot,
    IterationElement,
    Formatting,
    CollectionLookup,
    ControlFlowMerge,
    Propagation,
}

/// Coarse lifetime regions relevant to logical-copy selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogicalCopyLifetime {
    Unknown,
    Temporary,
    LexicalScope,
    Aggregate,
    Callee,
    Caller,
    Closure,
    Loop,
    Thread,
    SynchronizedStorage,
}

/// Facts known while checking, before the concrete value type is available to
/// classify the copy's allocation behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LogicalCopyContext {
    pub kind: LogicalCopyKind,
    pub source_lifetime: LogicalCopyLifetime,
    pub destination_lifetime: LogicalCopyLifetime,
}

impl LogicalCopyContext {
    #[must_use]
    pub const fn ordinary(
        kind: LogicalCopyKind,
        source_lifetime: LogicalCopyLifetime,
        destination_lifetime: LogicalCopyLifetime,
    ) -> Self {
        Self {
            kind,
            source_lifetime,
            destination_lifetime,
        }
    }
}

/// The broad allocation behavior selected from a copied value's canonical
/// type. This is a planning fact, not a promise that an allocation survives a
/// later optimization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogicalCopyAllocation {
    None,
    PreserveIdentity,
    /// Copies one complete inline C representation without recursively
    /// duplicating managed backing reached through its fields.
    Shallow,
    SharedBacking,
    RuntimeManaged,
}

/// Complete typed-IR copy facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LogicalCopyFacts {
    pub context: LogicalCopyContext,
    pub allocation: LogicalCopyAllocation,
    /// The selected physical implementation of this semantic copy.
    pub mode: LogicalCopyMode,
}

/// How the compiler realizes a source-level value copy.
///
/// `ReuseSource` is valid only when the source storage is dead after the
/// operation. It is explicit in IR so the backend never guesses liveness from
/// generated C expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogicalCopyMode {
    Materialize,
    ReuseSource,
}

/// How one source-level value parameter crosses an internal call boundary.
/// Borrowing is a compiler-only ABI choice and never changes the source type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValuePassingMode {
    Owned,
    ReadOnlyBorrowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StandardCall {
    Panic,
    Assert,
    Fail {
        value_type: TypeId,
    },
    Trap {
        reason_type: TypeId,
        trait_declaration: DeclarationId,
    },
    StringFrom,
    IdentityFrom {
        wrapper: TypeId,
    },
    ForeignRootRetain {
        handle: TypeId,
        mutable: bool,
    },
    ForeignRootPointer {
        handle: TypeId,
        mutable: bool,
    },
    ForeignRootClose {
        handle: TypeId,
    },
    ThreadSpawn {
        thread: TypeId,
        callable: TypeId,
        entry: DeclarationId,
        closure_entry: bool,
        return_type: TypeId,
    },
    ThreadJoin {
        thread: TypeId,
        return_type: TypeId,
    },
    ThreadIsFinished {
        thread: TypeId,
    },
    ChannelCreate {
        sender: TypeId,
        receiver: TypeId,
        element: TypeId,
        bounded: bool,
    },
    ChannelSend {
        sender: TypeId,
        element: TypeId,
        nonblocking: bool,
    },
    ChannelReceive {
        receiver: TypeId,
        element: TypeId,
        nonblocking: bool,
    },
    ChannelClose {
        handle: TypeId,
        sender: bool,
    },
    MutexNew {
        mutex: TypeId,
        value_type: TypeId,
    },
    MutexRead {
        mutex: TypeId,
        value_type: TypeId,
    },
    MutexReplace {
        mutex: TypeId,
        value_type: TypeId,
    },
    MutexUpdate {
        mutex: TypeId,
        value_type: TypeId,
        callable: TypeId,
        entry: DeclarationId,
        closure_entry: bool,
    },
    AtomicNew {
        atomic: TypeId,
        value_type: TypeId,
    },
    AtomicLoad {
        atomic: TypeId,
        value_type: TypeId,
    },
    AtomicStore {
        atomic: TypeId,
        value_type: TypeId,
    },
    AtomicExchange {
        atomic: TypeId,
        value_type: TypeId,
    },
    AtomicCompareExchange {
        atomic: TypeId,
        value_type: TypeId,
    },
    AtomicFetchAdd {
        atomic: TypeId,
        value_type: TypeId,
        subtract: bool,
    },
    FormatterWrite {
        formatter: TypeId,
    },
    ArrayLen {
        collection: TypeId,
    },
    ArrayGet {
        collection: TypeId,
    },
    SliceLen {
        collection: TypeId,
    },
    VecNew {
        collection: TypeId,
    },
    VecLen {
        collection: TypeId,
    },
    VecIsEmpty {
        collection: TypeId,
    },
    VecGet {
        collection: TypeId,
    },
    VecAppend {
        collection: TypeId,
    },
    VecInsert {
        collection: TypeId,
    },
    VecRemove {
        collection: TypeId,
    },
    VecClear {
        collection: TypeId,
    },
    MapNew {
        collection: TypeId,
    },
    MapLen {
        collection: TypeId,
    },
    MapIsEmpty {
        collection: TypeId,
    },
    MapContainsKey {
        collection: TypeId,
    },
    MapGet {
        collection: TypeId,
    },
    MapInsert {
        collection: TypeId,
    },
    MapRemove {
        collection: TypeId,
    },
    MapClear {
        collection: TypeId,
    },
    SetNew {
        collection: TypeId,
    },
    SetLen {
        collection: TypeId,
    },
    SetIsEmpty {
        collection: TypeId,
    },
    SetContains {
        collection: TypeId,
    },
    SetInsert {
        collection: TypeId,
    },
    SetRemove {
        collection: TypeId,
    },
    SetClear {
        collection: TypeId,
    },
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
