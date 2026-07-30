//! Data model for explicit control-flow IR.

use super::super::*;

#[derive(Debug, Clone)]
pub enum ControlFlowPlace {
    Local(LocalBindingId),
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
    Copy(TemporaryId),
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
    },
    IndirectCall {
        callee: TemporaryId,
        arguments: Vec<TemporaryId>,
    },
    VariadicSlice {
        elements: Vec<TemporaryId>,
        element_type: TypeId,
    },
    Aggregate(AggregateValue),
}

#[derive(Debug, Clone)]
pub enum Instruction {
    Assign {
        destination: TemporaryId,
        value: Rvalue,
        span: Span,
    },
    /// The mandatory null and alignment check before an executed raw
    /// dereference or raw-to-reference conversion (`SPEC.md` 3.3, Milestone
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
    },
    PrintNewline {
        span: Span,
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
    /// Locals promoted to managed storage; see [`TypedFunction::promoted_locals`].
    pub promoted_locals: BTreeSet<LocalBindingId>,
    /// See [`TypedFunction::allocates_managed`].
    pub allocates_managed: bool,
    pub temporary_types: Vec<TypeId>,
    pub entry: BlockId,
    pub blocks: Vec<BasicBlock>,
}

#[derive(Debug, Default)]
pub struct ControlFlowProgram {
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
