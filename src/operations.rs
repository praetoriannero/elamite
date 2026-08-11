//! Phase-neutral descriptions of compiler-selected standard operations.
//!
//! The checker selects these operations, IR preserves them, and the backend
//! lowers them. No one phase owns their shared vocabulary.

use crate::resolution::{DeclarationId, FieldId, LocalBindingId};
use crate::source::Span;
use crate::types::TypeId;

/// The result of one structural ownership-capability query.
///
/// `Conditional` is retained for generic and erased types whose concrete
/// substitutions decide the answer. `Error` is deliberately distinct from
/// `Present`: recovery types must never accidentally acquire a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CapabilityState {
    Absent,
    Present,
    Conditional,
    Error,
}

impl CapabilityState {
    #[must_use]
    pub const fn is_present(self) -> bool {
        matches!(self, Self::Present)
    }
}

/// Canonical facts used by move, borrow, cleanup, and concurrency analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct OwnershipFacts {
    pub copy: CapabilityState,
    pub clone: CapabilityState,
    pub needs_drop: CapabilityState,
    pub contains_borrow: CapabilityState,
    pub send: CapabilityState,
    pub sync: CapabilityState,
}

impl OwnershipFacts {
    pub const ERROR: Self = Self {
        copy: CapabilityState::Error,
        clone: CapabilityState::Error,
        needs_drop: CapabilityState::Error,
        contains_borrow: CapabilityState::Error,
        send: CapabilityState::Error,
        sync: CapabilityState::Error,
    };
}

/// A stable ownership-analysis root. Expression roots are used only when an
/// indirection prevents the compiler from naming unique source storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OwnershipPlaceRoot {
    Local(LocalBindingId),
    ClosureCapture(usize),
    Expression(Span),
}

/// One source-independent projection from an ownership place root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OwnershipProjection {
    Field(FieldId),
    TupleField(usize),
    ConstantIndex(u128),
    DynamicIndex,
    Dereference,
    ReceiverAdaptation,
}

/// A place description retained by typed and control-flow IR so later passes
/// never need to reconstruct aliasing paths from syntax.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct OwnershipPlace {
    pub root: OwnershipPlaceRoot,
    pub projections: Vec<OwnershipProjection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceOverlap {
    Equal,
    Disjoint,
    MayOverlap,
}

impl OwnershipPlace {
    #[must_use]
    pub fn overlap(&self, other: &Self) -> PlaceOverlap {
        if self.root != other.root {
            return if matches!(self.root, OwnershipPlaceRoot::Expression(_))
                || matches!(other.root, OwnershipPlaceRoot::Expression(_))
                || self.projections.contains(&OwnershipProjection::Dereference)
                || other
                    .projections
                    .contains(&OwnershipProjection::Dereference)
            {
                PlaceOverlap::MayOverlap
            } else {
                PlaceOverlap::Disjoint
            };
        }
        if self.projections == other.projections {
            return PlaceOverlap::Equal;
        }
        for (left, right) in self.projections.iter().zip(&other.projections) {
            if left == right {
                continue;
            }
            return match (left, right) {
                (OwnershipProjection::Field(left), OwnershipProjection::Field(right))
                    if left != right =>
                {
                    PlaceOverlap::Disjoint
                }
                (OwnershipProjection::TupleField(left), OwnershipProjection::TupleField(right))
                    if left != right =>
                {
                    PlaceOverlap::Disjoint
                }
                (
                    OwnershipProjection::ConstantIndex(left),
                    OwnershipProjection::ConstantIndex(right),
                ) if left != right => PlaceOverlap::Disjoint,
                _ => PlaceOverlap::MayOverlap,
            };
        }
        // One path is a projection of the other, so replacing the shorter
        // place overlaps the longer path.
        PlaceOverlap::MayOverlap
    }
}

/// Semantic intent of one source value use. `LegacyCopy` is the only variant
/// that carries the 0.10 shallow-copy contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OwnershipUseKind {
    Produce,
    Move,
    Copy,
    Clone,
    BorrowShared,
    BorrowExclusive,
    ReborrowShared,
    ReborrowExclusive,
    Drop,
    LegacyCopy,
}

/// Complete phase-neutral facts for one value-producing or consuming use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipUse {
    pub kind: OwnershipUseKind,
    pub facts: OwnershipFacts,
    pub place: Option<OwnershipPlace>,
    pub legacy_copy: Option<LogicalCopyFacts>,
}

impl Default for OwnershipUse {
    fn default() -> Self {
        Self {
            kind: OwnershipUseKind::Produce,
            facts: OwnershipFacts::ERROR,
            place: None,
            legacy_copy: None,
        }
    }
}

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
pub enum TextOperation {
    ByteLen,
    NextScalar,
    SliceBytes,
    StringView,
    FromChars,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SystemOperation {
    PathView,
    Open,
    ReadDir,
    Metadata,
    CreateDir,
    RemoveDir,
    RemoveFile,
    Rename,
    FileReadToEnd,
    FileWriteAll,
    FileMetadata,
    FileClose,
    DirectoryNext,
    DirectoryClose,
    Args,
    EnvGet,
    CurrentDir,
    ProcessRun,
    ProcessExit,
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
    ClockNow {
        clock_type: TypeId,
        monotonic: bool,
    },
    StringFrom,
    /// Explicit content clone for compiler-represented owned values.
    Clone {
        value: TypeId,
    },
    BoxNew {
        boxed: TypeId,
        value: TypeId,
    },
    Text {
        operation: TextOperation,
        result_type: TypeId,
        input_type: TypeId,
    },
    System {
        operation: SystemOperation,
        result_type: TypeId,
    },
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
    VecGetVar {
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
    VecPop {
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
    MapGetVar {
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

impl StandardCall {
    /// Rewrites every type carried by a standard operation. Typed lowering
    /// uses this when specializing generic source-backed standard functions.
    pub fn map_types(mut self, mut map: impl FnMut(TypeId) -> TypeId) -> Self {
        match &mut self {
            Self::Panic | Self::Assert | Self::StringFrom => {}
            Self::Clone { value } => *value = map(*value),
            Self::BoxNew { boxed, value } => {
                *boxed = map(*boxed);
                *value = map(*value);
            }
            Self::Text {
                result_type,
                input_type,
                ..
            } => {
                *result_type = map(*result_type);
                *input_type = map(*input_type);
            }
            Self::System { result_type, .. } => *result_type = map(*result_type),
            Self::Fail { value_type } => *value_type = map(*value_type),
            Self::Trap { reason_type, .. } => *reason_type = map(*reason_type),
            Self::ClockNow { clock_type, .. } => *clock_type = map(*clock_type),
            Self::IdentityFrom { wrapper } => *wrapper = map(*wrapper),
            Self::ForeignRootRetain { handle, .. }
            | Self::ForeignRootPointer { handle, .. }
            | Self::ForeignRootClose { handle }
            | Self::ChannelClose { handle, .. } => *handle = map(*handle),
            Self::ThreadSpawn {
                thread,
                callable,
                return_type,
                ..
            } => {
                *thread = map(*thread);
                *callable = map(*callable);
                *return_type = map(*return_type);
            }
            Self::ThreadJoin {
                thread,
                return_type,
            } => {
                *thread = map(*thread);
                *return_type = map(*return_type);
            }
            Self::ThreadIsFinished { thread } => *thread = map(*thread),
            Self::ChannelCreate {
                sender,
                receiver,
                element,
                ..
            } => {
                *sender = map(*sender);
                *receiver = map(*receiver);
                *element = map(*element);
            }
            Self::ChannelSend {
                sender, element, ..
            } => {
                *sender = map(*sender);
                *element = map(*element);
            }
            Self::ChannelReceive {
                receiver, element, ..
            } => {
                *receiver = map(*receiver);
                *element = map(*element);
            }
            Self::MutexNew { mutex, value_type }
            | Self::MutexRead { mutex, value_type }
            | Self::MutexReplace { mutex, value_type } => {
                *mutex = map(*mutex);
                *value_type = map(*value_type);
            }
            Self::MutexUpdate {
                mutex,
                value_type,
                callable,
                ..
            } => {
                *mutex = map(*mutex);
                *value_type = map(*value_type);
                *callable = map(*callable);
            }
            Self::AtomicNew { atomic, value_type }
            | Self::AtomicLoad { atomic, value_type }
            | Self::AtomicStore { atomic, value_type }
            | Self::AtomicExchange { atomic, value_type }
            | Self::AtomicCompareExchange { atomic, value_type }
            | Self::AtomicFetchAdd {
                atomic, value_type, ..
            } => {
                *atomic = map(*atomic);
                *value_type = map(*value_type);
            }
            Self::FormatterWrite { formatter } => *formatter = map(*formatter),
            Self::ArrayLen { collection }
            | Self::ArrayGet { collection }
            | Self::SliceLen { collection }
            | Self::VecNew { collection }
            | Self::VecLen { collection }
            | Self::VecIsEmpty { collection }
            | Self::VecGet { collection }
            | Self::VecGetVar { collection }
            | Self::VecAppend { collection }
            | Self::VecInsert { collection }
            | Self::VecRemove { collection }
            | Self::VecPop { collection }
            | Self::VecClear { collection }
            | Self::MapNew { collection }
            | Self::MapLen { collection }
            | Self::MapIsEmpty { collection }
            | Self::MapContainsKey { collection }
            | Self::MapGet { collection }
            | Self::MapGetVar { collection }
            | Self::MapInsert { collection }
            | Self::MapRemove { collection }
            | Self::MapClear { collection }
            | Self::SetNew { collection }
            | Self::SetLen { collection }
            | Self::SetIsEmpty { collection }
            | Self::SetContains { collection }
            | Self::SetInsert { collection }
            | Self::SetRemove { collection }
            | Self::SetClear { collection } => *collection = map(*collection),
        }
        self
    }
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
