//! Data model for typed high-level IR.

use super::super::*;

#[derive(Debug, Clone)]
pub enum TypedCallee {
    Function(FunctionInstance),
    Indirect(Box<TypedExpression>),
    Closure {
        instance: FunctionInstance,
        value: Box<TypedExpression>,
    },
    /// A call through a trait object's vtable. The object supplies both the
    /// receiver and the function pointer.
    Dynamic {
        trait_declaration: DeclarationId,
        slot: usize,
    },
    Print {
        newline: bool,
    },
}

#[derive(Debug, Clone)]
pub enum FormattedPart {
    Text(String),
    Expression(TypedExpression),
}

#[derive(Debug, Clone)]
pub enum RuntimeFormattedPart {
    Text(String),
    Value { value: TemporaryId, ty: TypeId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CollectionLiteralKind {
    Vec,
    Map,
    Set,
}

#[derive(Debug, Clone)]
pub enum TypedExpressionKind {
    Constant(Constant),
    FunctionReference(FunctionInstance),
    Local(LocalBindingId),
    ClosureCapture(usize),
    Closure {
        instance: FunctionInstance,
        captures: Vec<TypedExpression>,
    },
    Unary {
        operator: UnaryOperator,
        operand: Box<TypedExpression>,
    },
    Binary {
        operator: BinaryOperator,
        left: Box<TypedExpression>,
        right: Box<TypedExpression>,
    },
    Cast {
        value: Box<TypedExpression>,
    },
    Call {
        callee: TypedCallee,
        arguments: Vec<TypedExpression>,
    },
    Field {
        base: Box<TypedExpression>,
        field: FieldId,
    },
    TupleField {
        base: Box<TypedExpression>,
        index: usize,
    },
    Index {
        base: Box<TypedExpression>,
        index: Box<TypedExpression>,
    },
    /// `&place` / `&var place`: the address of an addressable place. The
    /// place's root local is promoted to managed storage by
    /// [`crate::promotion`], so this is always the address of a managed cell
    /// or of a subvalue inside one.
    AddressOf(Box<TypedPlace>),
    /// `&Composite { .. }`: a referenced composite literal, which allocates
    /// its own managed cell because it has no source-level binding.
    AddressOfTemporary(Box<TypedExpression>),
    /// `*reference`: the value the reference names.
    Dereference(Box<TypedExpression>),
    /// `Type.default()` from a `Default` derivation: the type's structural
    /// default value.
    DefaultValue(TypeId),
    /// Postfix `?` on a standard `Result[T, E]` operand (`SPEC.md` 8).
    ///
    /// The operand is evaluated exactly once. An `Ok` payload is copied into
    /// this expression's value; an `Err` payload is copied into an early
    /// `Result.Err` return of the enclosing function's return type, running
    /// every exited scope's deferred registrations after the copy. The
    /// standard `Result` identities are recorded here so control-flow
    /// lowering needs no name lookup.
    Propagate {
        operand: Box<TypedExpression>,
        result_declaration: DeclarationId,
        ok_variant: VariantId,
        ok_field: FieldId,
        err_variant: VariantId,
        err_field: FieldId,
        /// The concrete error payload type `E`, shared by the operand and the
        /// enclosing function's return type.
        error_type: TypeId,
    },
    /// A standard nontrapping numeric conversion (`SPEC.md` 4.1). The operand
    /// is evaluated once; the checked form's range test uses the same
    /// boundaries as the trapping `as` conversion, so the two never disagree
    /// about which values are representable.
    NumericConversion {
        outcome: NumericOutcome,
        value: Box<TypedExpression>,
        target: TypeId,
    },
    /// `value.checked_add(other)` and the other standard alternatives to the
    /// trapping arithmetic operators (`SPEC.md` 4.1). The receiver and any
    /// operand are evaluated left to right, exactly as for the operator.
    NumericAlternative {
        operation: NumericAlternative,
        receiver: Box<TypedExpression>,
        operand: Option<Box<TypedExpression>>,
    },
    /// A compiler-supplied text/array/collection operation. For bound
    /// operations the receiver is the first argument; associated constructors
    /// and `String.from` contain only their explicit source arguments.
    StandardCall {
        operation: StandardCall,
        arguments: Vec<TypedExpression>,
    },
    CollectionLiteral {
        kind: CollectionLiteralKind,
        elements: Vec<TypedExpression>,
    },
    /// `reference as &Trait`: pairs a concrete reference with the vtable of
    /// its implementing type.
    MakeTraitObject {
        value: Box<TypedExpression>,
        trait_declaration: DeclarationId,
        trait_type: TypeId,
        concrete: TypeId,
    },
    Tuple(Vec<TypedExpression>),
    Array(Vec<TypedExpression>),
    Struct {
        declaration: DeclarationId,
        fields: Vec<(FieldId, TypedExpression)>,
    },
    Enum {
        declaration: DeclarationId,
        variant: VariantId,
        fields: Vec<(FieldId, TypedExpression)>,
    },
    /// A homogeneous variadic tail packed into the language slice
    /// representation before the call.
    VariadicSlice(Vec<TypedExpression>),
    FormattedString(Vec<FormattedPart>),
}

#[derive(Debug, Clone)]
pub struct TypedExpression {
    pub ty: TypeId,
    pub place: PlaceKind,
    pub copy: bool,
    pub span: Span,
    pub kind: TypedExpressionKind,
}

#[derive(Debug, Clone)]
pub enum TypedPatternKind {
    Wildcard,
    Binding(LocalBindingId),
    Literal(Constant),
    Alternative(Vec<TypedPattern>),
    Dereference(Box<TypedPattern>),
    Tuple(Vec<TypedPattern>),
    Struct {
        declaration: DeclarationId,
        fields: Vec<(FieldId, TypedPattern)>,
    },
    Variant {
        variant: VariantId,
        fields: Vec<(FieldId, TypedPattern)>,
    },
}

#[derive(Debug, Clone)]
pub struct TypedPattern {
    pub ty: TypeId,
    pub span: Span,
    pub kind: TypedPatternKind,
}

#[derive(Debug, Clone)]
pub struct TypedMatchArm {
    pub pattern: TypedPattern,
    pub guard: Option<TypedExpression>,
    pub body: Vec<TypedStatement>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum TypedPlace {
    Local {
        binding: LocalBindingId,
        ty: TypeId,
        span: Span,
    },
    ClosureCapture {
        index: usize,
        ty: TypeId,
        span: Span,
    },
    Field {
        base: Box<TypedPlace>,
        field: FieldId,
        ty: TypeId,
        span: Span,
    },
    TupleField {
        base: Box<TypedPlace>,
        index: usize,
        ty: TypeId,
        span: Span,
    },
    Index {
        base: Box<TypedPlace>,
        index: TypedExpression,
        ty: TypeId,
        kind: IndexKind,
        span: Span,
    },
    /// `*reference` as an assignable place. Its base is a reference *value*,
    /// not a place, so writes land in the referenced storage.
    Dereference {
        base: Box<TypedExpression>,
        ty: TypeId,
        span: Span,
    },
}

impl TypedPlace {
    #[must_use]
    pub fn ty(&self) -> TypeId {
        match self {
            Self::Local { ty, .. }
            | Self::ClosureCapture { ty, .. }
            | Self::Field { ty, .. }
            | Self::TupleField { ty, .. }
            | Self::Index { ty, .. }
            | Self::Dereference { ty, .. } => *ty,
        }
    }

    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Local { span, .. }
            | Self::ClosureCapture { span, .. }
            | Self::Field { span, .. }
            | Self::TupleField { span, .. }
            | Self::Index { span, .. }
            | Self::Dereference { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone)]
pub enum TypedStatementKind {
    Let {
        binding: LocalBindingId,
        mutable: bool,
        ty: TypeId,
        value: TypedExpression,
    },
    Destructure {
        bindings: Vec<TypedTupleBinding>,
        mutable: bool,
        value: TypedExpression,
    },
    Assign {
        place: TypedPlace,
        operator: AssignmentOperator,
        value: TypedExpression,
    },
    Expression(TypedExpression),
    Return(Option<TypedExpression>),
    If {
        condition: TypedExpression,
        then_body: Vec<TypedStatement>,
        else_body: Vec<TypedStatement>,
    },
    While {
        condition: TypedExpression,
        body: Vec<TypedStatement>,
    },
    For {
        binding: LocalBindingId,
        iterable: TypedExpression,
        kind: IterationKind,
        body: Vec<TypedStatement>,
    },
    Match {
        scrutinee: TypedExpression,
        arms: Vec<TypedMatchArm>,
    },
    Block(Vec<TypedStatement>),
    /// One deferred registration owned by its lexical scope (`SPEC.md` 8).
    ///
    /// The single-call form is a body holding that one call statement; the
    /// `defer:` block form is its statements in source order. Registration
    /// occurs when control reaches the statement and evaluates nothing: the
    /// body is retained as ordinary typed statements over the same lexical
    /// binding identities, so execution at scope exit reads the values those
    /// bindings have *then*. No callable or environment value exists.
    Defer(Vec<TypedStatement>),
    Expect {
        selector: TypedExpression,
        trait_declaration: DeclarationId,
        body: Vec<TypedStatement>,
    },
    Break,
    Continue,
    Pass,
}

#[derive(Debug, Clone)]
pub struct TypedTupleBinding {
    pub binding: LocalBindingId,
    pub ty: TypeId,
    pub indices: Vec<usize>,
}

#[derive(Debug, Clone, Copy)]
pub enum IterationKind {
    Slice {
        collection: TypeId,
        element: TypeId,
    },
    Array {
        length: u128,
        element: TypeId,
    },
    Vec {
        collection: TypeId,
        element: TypeId,
    },
    Map {
        collection: TypeId,
        key: TypeId,
        value: TypeId,
        pair: TypeId,
    },
    Set {
        collection: TypeId,
        element: TypeId,
    },
}

#[derive(Debug, Clone)]
pub struct TypedStatement {
    pub span: Span,
    pub kind: TypedStatementKind,
}

#[derive(Debug, Clone)]
pub struct TypedParameter {
    pub binding: LocalBindingId,
    pub ty: TypeId,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TypedFunction {
    pub declaration: DeclarationId,
    pub instance: FunctionInstance,
    pub name: String,
    pub span: Span,
    pub parameters: Vec<TypedParameter>,
    pub return_type: TypeId,
    pub body: Vec<TypedStatement>,
    pub local_types: BTreeMap<LocalBindingId, TypeId>,
    /// Locals whose address is taken, and which therefore need managed storage
    /// rather than a C stack slot (`ROADMAP.md` Milestone 10). Conservative: every
    /// address-taken local is promoted.
    pub promoted_locals: BTreeSet<LocalBindingId>,
    /// Whether the body allocates a managed cell for a referenced composite
    /// literal. Such a cell has no binding, so promotion alone does not imply
    /// it.
    pub allocates_managed: bool,
    pub closure: Option<TypedClosureBody>,
}

#[derive(Debug, Clone)]
pub struct TypedClosureBody {
    pub ty: TypeId,
    pub captures: BTreeMap<LocalBindingId, usize>,
}

#[derive(Debug, Clone)]
pub struct TypedStruct {
    pub ty: TypeId,
    pub declaration: DeclarationId,
    pub name: String,
    pub fields: Vec<(FieldId, String, TypeId)>,
}

#[derive(Debug, Clone)]
pub struct TypedVariant {
    pub id: VariantId,
    pub name: String,
    pub fields: Vec<(FieldId, String, TypeId)>,
}

#[derive(Debug, Clone)]
pub struct TypedEnum {
    pub ty: TypeId,
    pub declaration: DeclarationId,
    pub name: String,
    pub variants: Vec<TypedVariant>,
}

#[derive(Debug, Default)]
pub struct TypedIrProgram {
    pub functions: Vec<TypedFunction>,
    pub structs: Vec<TypedStruct>,
    pub enums: Vec<TypedEnum>,
    /// One vtable per (trait, implementing type) reachable through a trait
    /// object, in deterministic order.
    pub vtables: Vec<Vtable>,
}

impl TypedIrProgram {
    /// Deterministic high-level IR dump. Maps and sets inside the IR are
    /// ordered, while vectors retain source and lowering order.
    #[must_use]
    pub fn dump(&self) -> String {
        format!("{self:#?}")
    }
}

/// A trait's method table for one implementing type.
#[derive(Debug, Clone)]
pub struct Vtable {
    pub trait_declaration: DeclarationId,
    pub trait_type: TypeId,
    pub concrete: TypeId,
    /// One entry per slot of [`crate::traits::vtable_slots`], in the same
    /// order.
    pub methods: Vec<VtableMethod>,
    pub signatures: Vec<crate::types::FunctionSignature>,
}

#[derive(Debug, Clone)]
pub enum VtableMethod {
    Function(FunctionInstance),
    Closure(FunctionInstance),
    FunctionReference,
}

pub struct TypedIrOutput {
    pub program: TypedIrProgram,
    pub diagnostics: Vec<Diagnostic>,
}
