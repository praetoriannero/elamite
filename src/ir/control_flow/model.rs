//! Data model for explicit control-flow IR.

use super::super::*;

#[derive(Debug, Clone)]
pub enum ControlFlowPlace {
    Local(LocalBindingId),
    ClosureCapture(usize),
    Temporary(TemporaryId),
    Field {
        base: Box<ControlFlowPlace>,
        field: FieldId,
    },
    TupleField {
        base: Box<ControlFlowPlace>,
        index: usize,
    },
    VariantField {
        base: Box<ControlFlowPlace>,
        variant: VariantId,
        field: FieldId,
    },
    Dereference {
        base: Box<ControlFlowPlace>,
    },
    Index {
        base: Box<ControlFlowPlace>,
        index: TemporaryId,
        kind: IndexKind,
        trap: TrapKind,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum IndexKind {
    Array { length: u128 },
    Slice,
    Vec { collection: TypeId },
    Map { collection: TypeId },
}

#[derive(Debug, Clone)]
pub enum AggregateValue {
    Tuple(Vec<TemporaryId>),
    Array(Vec<TemporaryId>),
    Struct {
        declaration: DeclarationId,
        fields: Vec<(FieldId, TemporaryId)>,
    },
    Enum {
        declaration: DeclarationId,
        variant: VariantId,
        fields: Vec<(FieldId, TemporaryId)>,
    },
}

#[derive(Debug, Clone)]
pub enum Rvalue {
    Constant(Constant),
    FunctionReference(FunctionInstance),
    Closure {
        ty: TypeId,
        captures: Vec<TemporaryId>,
    },
    Load(ControlFlowPlace),
    /// The address of a place. Its root local is promoted, so the address is
    /// stable for as long as the reference is reachable.
    AddressOf(ControlFlowPlace),
    /// The structural default of a type, from a `Default` derivation.
    DefaultValue(TypeId),
    /// A standard nontrapping numeric conversion. The destination temporary
    /// carries the result type: `Result[target, NumericError]` for the checked
    /// form, and the target itself otherwise.
    NumericConversion {
        outcome: NumericOutcome,
        value: TemporaryId,
        source_type: TypeId,
        target_type: TypeId,
    },
    /// One standard alternative to a trapping arithmetic operator. The
    /// destination temporary carries the result type, which is `Option[T]` for
    /// a checked operation and `T` otherwise.
    NumericAlternative {
        operation: NumericAlternative,
        receiver: TemporaryId,
        operand: Option<TemporaryId>,
        operand_type: TypeId,
    },
    StandardCall {
        operation: StandardCall,
        arguments: Vec<TemporaryId>,
        /// The evaluated receiver place for operations that mutate an inline
        /// descriptor. When present, the receiver is omitted from
        /// `arguments`.
        receiver_place: Option<ControlFlowPlace>,
    },
    CollectionLiteral {
        kind: CollectionLiteralKind,
        elements: Vec<TemporaryId>,
    },
    CollectionLength {
        collection: TemporaryId,
        kind: IterationKind,
    },
    IterationElement {
        collection: TemporaryId,
        index: TemporaryId,
        kind: IterationKind,
    },
    FormattedString(Vec<RuntimeFormattedPart>),
    /// Pairs a concrete reference with a vtable to form a trait object.
    MakeTraitObject {
        value: TemporaryId,
        trait_declaration: DeclarationId,
        trait_type: TypeId,
        concrete: TypeId,
    },
    /// Calls through a trait object's vtable slot.
    DynamicCall {
        receiver: TemporaryId,
        trait_declaration: DeclarationId,
        slot: usize,
        arguments: Vec<TemporaryId>,
    },
    /// Allocates a managed cell, initializes it from a temporary, and yields
    /// its address. This backs a referenced composite literal.
    AllocateManaged {
        value: TemporaryId,
        value_type: TypeId,
    },
    Copy {
        source: TemporaryId,
        id: LogicalCopyId,
        facts: LogicalCopyFacts,
    },
    /// An owned-model value use. The operation is emitted after its source is
    /// fully evaluated, preserving source order without asking the backend to
    /// infer semantic intent from C expressions.
    OwnershipUse {
        source: TemporaryId,
        id: OwnershipUseId,
        operation: OwnershipUse,
    },
    Discriminant(TemporaryId),
    CompareEqual {
        left: TemporaryId,
        right: TemporaryId,
        operand_type: TypeId,
    },
    /// One structural relational comparison. The backend preserves unordered
    /// floating-point components while comparing aggregate fields
    /// lexicographically.
    CompareOrder {
        operator: BinaryOperator,
        left: TemporaryId,
        right: TemporaryId,
        operand_type: TypeId,
    },
    Unary {
        operator: UnaryOperator,
        operand: TemporaryId,
        trap: Option<TrapKind>,
    },
    Binary {
        operator: BinaryOperator,
        left: TemporaryId,
        right: TemporaryId,
        trap: Option<TrapKind>,
    },
    Cast {
        value: TemporaryId,
        source_type: TypeId,
        trap: Option<TrapKind>,
    },
    Call {
        instance: FunctionInstance,
        arguments: Vec<TemporaryId>,
        argument_modes: Vec<ValuePassingMode>,
    },
    IndirectCall {
        callee: TemporaryId,
        arguments: Vec<TemporaryId>,
    },
    ClosureCall {
        instance: FunctionInstance,
        closure: TemporaryId,
        arguments: Vec<TemporaryId>,
    },
    VariadicSlice {
        elements: Vec<TemporaryId>,
        element_type: TypeId,
    },
    Aggregate(AggregateValue),
    BeginExpectation {
        selector: TemporaryId,
        selector_type: TypeId,
        trait_declaration: DeclarationId,
    },
}

#[derive(Debug, Clone)]
pub enum Instruction {
    Assign {
        destination: TemporaryId,
        value: Rvalue,
        span: Span,
    },
    /// The mandatory null and alignment check before an executed raw
    /// dereference or raw-to-reference conversion (`docs/spec.md` 3.3, Milestone
    /// 16.8). Traps `E-RUN-NULL` or `E-RUN-ALIGN` with this source location.
    CheckPointer {
        pointer: TemporaryId,
        pointee: TypeId,
        span: Span,
    },
    /// A direct call through `*fn`: null is checked before invocation. Other
    /// target validity and signature obligations are asserted by the unsafe
    /// call site.
    CheckFunctionPointer {
        pointer: TemporaryId,
        span: Span,
    },
    Store {
        place: ControlFlowPlace,
        value: TemporaryId,
        span: Span,
    },
    PrintValue {
        value: TemporaryId,
        ty: TypeId,
        span: Span,
        newline: bool,
    },
    CompleteExpectation {
        span: Span,
    },
    WaitExpectation {
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub enum NeverCall {
    Panic {
        message: TemporaryId,
    },
    AssertionFail {
        value: TemporaryId,
        value_type: TypeId,
    },
    TypedTrap {
        reason: TemporaryId,
        reason_type: TypeId,
        trait_declaration: DeclarationId,
    },
    Standard {
        operation: StandardCall,
        arguments: Vec<TemporaryId>,
    },
    Direct {
        instance: FunctionInstance,
        arguments: Vec<TemporaryId>,
        argument_modes: Vec<ValuePassingMode>,
    },
    Dynamic {
        receiver: TemporaryId,
        trait_declaration: DeclarationId,
        slot: usize,
        arguments: Vec<TemporaryId>,
    },
    Indirect {
        callee: TemporaryId,
        arguments: Vec<TemporaryId>,
    },
    Closure {
        instance: FunctionInstance,
        closure: TemporaryId,
        arguments: Vec<TemporaryId>,
    },
}

#[derive(Debug, Clone)]
pub enum Terminator {
    Goto(BlockId),
    Branch {
        condition: TemporaryId,
        then_block: BlockId,
        else_block: BlockId,
    },
    Return(Option<TemporaryId>),
    Trap {
        kind: TrapKind,
        span: Span,
    },
    /// A call whose canonical return type is `Never`. It has no result
    /// storage because control cannot continue after it.
    NeverCall {
        call: NeverCall,
        span: Span,
    },
    Unreachable,
}

#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub id: BlockId,
    pub instructions: Vec<Instruction>,
    pub terminator: Terminator,
}

#[derive(Debug, Clone)]
pub struct ControlFlowFunction {
    pub declaration: DeclarationId,
    pub instance: FunctionInstance,
    pub name: String,
    pub span: Span,
    pub parameters: Vec<TypedParameter>,
    pub return_type: TypeId,
    pub local_types: BTreeMap<LocalBindingId, TypeId>,
    /// Destruction requirements retained for later cleanup-edge elaboration.
    pub drop_requirements: Vec<DropRequirement>,
    /// Locals promoted to managed storage; see [`TypedFunction::promoted_locals`].
    pub promoted_locals: BTreeSet<LocalBindingId>,
    /// See [`TypedFunction::allocates_managed`].
    pub allocates_managed: bool,
    pub closure: Option<TypedClosureBody>,
    pub temporary_types: Vec<TypeId>,
    pub entry: BlockId,
    pub blocks: Vec<BasicBlock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogicalCopyRecord {
    pub id: LogicalCopyId,
    pub facts: LogicalCopyFacts,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipUseRecord {
    pub id: OwnershipUseId,
    pub operation: OwnershipUse,
    pub span: Span,
}

impl ControlFlowFunction {
    /// Every logical copy emitted for this concrete function instance. IDs are
    /// stable within the function and assigned in lowering order.
    #[must_use]
    pub fn logical_copy_inventory(&self) -> Vec<LogicalCopyRecord> {
        self.blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter_map(|instruction| match instruction {
                Instruction::Assign {
                    value: Rvalue::Copy { id, facts, .. },
                    span,
                    ..
                } => Some(LogicalCopyRecord {
                    id: *id,
                    facts: *facts,
                    span: *span,
                }),
                _ => None,
            })
            .collect()
    }

    /// Every owned-model operation in exact control-flow lowering order.
    #[must_use]
    pub fn ownership_use_inventory(&self) -> Vec<OwnershipUseRecord> {
        self.blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .filter_map(|instruction| match instruction {
                Instruction::Assign {
                    value: Rvalue::OwnershipUse { id, operation, .. },
                    span,
                    ..
                } => Some(OwnershipUseRecord {
                    id: *id,
                    operation: operation.clone(),
                    span: *span,
                }),
                _ => None,
            })
            .collect()
    }
}

#[derive(Debug, Default)]
pub struct ControlFlowProgram {
    pub semantic_revision: crate::config::SemanticRevision,
    pub functions: Vec<ControlFlowFunction>,
    pub structs: Vec<TypedStruct>,
    pub enums: Vec<TypedEnum>,
    /// Vtables reachable through trait objects; see [`Vtable`].
    pub vtables: Vec<Vtable>,
    /// Whether lowering produced any managed allocation, promoted storage, or
    /// managed root. The backend engages `ManagedMemoryStrategy` — and the
    /// driver links its native libraries — only when this is set, so programs
    /// that never need the collector keep a dependency-free translation unit.
    /// Milestone 10 promotion analysis is what turns this on.
    pub requires_managed_memory: bool,
}

impl ControlFlowProgram {
    /// Deterministic explicit-control-flow dump.
    #[must_use]
    pub fn dump(&self) -> String {
        format!("{self:#?}")
    }

    /// Concrete function instances selected by monomorphization.
    #[must_use]
    pub fn monomorphization_dump(&self) -> String {
        let mut output = String::new();
        for function in &self.functions {
            output.push_str(&format!(
                "{} declaration={} arguments={:?} self_type={:?} span={:?}\n",
                function.name,
                function.instance.declaration.index(),
                function.instance.arguments,
                function.instance.self_type,
                function.span
            ));
        }
        output
    }
}
