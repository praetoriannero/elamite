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
        argument_modes: Vec<ValuePassingMode>,
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
    /// `&place` / `&var place`: the address of an addressable place. Owned
    /// provenance keeps the place on stack; compatibility lowering may give
    /// the root stable process lifetime.
    AddressOf(Box<TypedPlace>),
    /// `&Composite { .. }`: a referenced composite literal with compiler-owned
    /// stable temporary storage.
    AddressOfTemporary(Box<TypedExpression>),
    /// `*reference`: the value the reference names.
    Dereference(Box<TypedExpression>),
    /// `Type.default()` from a `Default` derivation: the type's structural
    /// default value.
    DefaultValue(TypeId),
    /// Postfix `?` on a standard `Result[T, E]` operand (`docs/spec.md` 8).
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
    /// A standard nontrapping numeric conversion (`docs/spec.md` 4.1). The operand
    /// is evaluated once; the checked form's range test uses the same
    /// boundaries as the trapping `as` conversion, so the two never disagree
    /// about which values are representable.
    NumericConversion {
        outcome: NumericOutcome,
        value: Box<TypedExpression>,
        target: TypeId,
    },
    /// `value.checked_add(other)` and the other standard alternatives to the
    /// trapping arithmetic operators (`docs/spec.md` 4.1). The receiver and any
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
    /// Complete facts for a logical copy materialized after evaluating this
    /// expression. `None` means the expression's value can flow directly.
    pub copy: Option<LogicalCopyFacts>,
    /// Ordered source ownership intents after generic substitution and stable
    /// place projection. Unlike `copy`, this is nonempty for every expression;
    /// a borrow expression consumed by a binding, for example, carries both
    /// borrow and copy/move operations.
    pub ownership: Vec<OwnershipUse>,
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

    #[must_use]
    pub fn ownership_place(&self) -> OwnershipPlace {
        match self {
            Self::Local { binding, .. } => OwnershipPlace {
                root: OwnershipPlaceRoot::Local(*binding),
                projections: Vec::new(),
            },
            Self::ClosureCapture { index, .. } => OwnershipPlace {
                root: OwnershipPlaceRoot::ClosureCapture(*index),
                projections: Vec::new(),
            },
            Self::Field { base, field, .. } => {
                let mut place = base.ownership_place();
                place.projections.push(OwnershipProjection::Field(*field));
                place
            }
            Self::TupleField { base, index, .. } => {
                let mut place = base.ownership_place();
                place
                    .projections
                    .push(OwnershipProjection::TupleField(*index));
                place
            }
            Self::Index { base, index, .. } => {
                let mut place = base.ownership_place();
                let projection = match &index.kind {
                    TypedExpressionKind::Constant(Constant::Integer {
                        magnitude,
                        negative: false,
                    }) => OwnershipProjection::ConstantIndex(*magnitude),
                    _ => OwnershipProjection::DynamicIndex,
                };
                place.projections.push(projection);
                place
            }
            Self::Dereference { base, .. } => {
                let mut place = base.ownership_place().unwrap_or(OwnershipPlace {
                    root: OwnershipPlaceRoot::Expression(base.span),
                    projections: Vec::new(),
                });
                place.projections.push(OwnershipProjection::Dereference);
                place
            }
        }
    }
}

impl TypedExpression {
    #[must_use]
    pub fn ownership_place(&self) -> Option<OwnershipPlace> {
        match &self.kind {
            TypedExpressionKind::Local(binding) => Some(OwnershipPlace {
                root: OwnershipPlaceRoot::Local(*binding),
                projections: Vec::new(),
            }),
            TypedExpressionKind::ClosureCapture(index) => Some(OwnershipPlace {
                root: OwnershipPlaceRoot::ClosureCapture(*index),
                projections: Vec::new(),
            }),
            TypedExpressionKind::Field { base, field } => {
                let mut place = base.ownership_place()?;
                place.projections.push(OwnershipProjection::Field(*field));
                Some(place)
            }
            TypedExpressionKind::TupleField { base, index } => {
                let mut place = base.ownership_place()?;
                place
                    .projections
                    .push(OwnershipProjection::TupleField(*index));
                Some(place)
            }
            TypedExpressionKind::Index { base, index } => {
                let mut place = base.ownership_place()?;
                place.projections.push(match &index.kind {
                    TypedExpressionKind::Constant(Constant::Integer {
                        magnitude,
                        negative: false,
                    }) => OwnershipProjection::ConstantIndex(*magnitude),
                    _ => OwnershipProjection::DynamicIndex,
                });
                Some(place)
            }
            TypedExpressionKind::Dereference(base) => {
                let mut place = base.ownership_place().unwrap_or(OwnershipPlace {
                    root: OwnershipPlaceRoot::Expression(base.span),
                    projections: Vec::new(),
                });
                place.projections.push(OwnershipProjection::Dereference);
                Some(place)
            }
            TypedExpressionKind::AddressOf(place) => Some(place.ownership_place()),
            _ => None,
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
        iterator_drop: Vec<DropAction>,
        body: Vec<TypedStatement>,
    },
    Match {
        scrutinee: TypedExpression,
        arms: Vec<TypedMatchArm>,
    },
    Block(Vec<TypedStatement>),
    /// One deferred registration owned by its lexical scope (`docs/spec.md` 8).
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

#[derive(Debug, Clone)]
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
    Store {
        collection: TypeId,
        element: TypeId,
    },
    /// A source type implementing the ordinary `std.Iterator[Element]`
    /// protocol. Control-flow lowering keeps the hidden mutable state in a
    /// function-local cell for the duration proven by borrow checking.
    User {
        state: TypeId,
        element: TypeId,
        receiver: TypeId,
        option: TypeId,
        some_variant: VariantId,
        some_field: FieldId,
        next: FunctionInstance,
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
    pub passing: ValuePassingMode,
}

/// A local whose initialized value requires destruction. Control-flow lowering
/// consumes this analysis requirement after move-state checking and elaborates
/// one conditional flag per ordered action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropRequirement {
    pub binding: LocalBindingId,
    pub ty: TypeId,
    pub span: Span,
    pub operation: OwnershipUse,
    /// Custom hooks and structural leaves in their required execution order:
    /// custom hook first, then fields/elements in reverse declaration order.
    pub actions: Vec<DropAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum DropProjection {
    Field(FieldId),
    TupleField(usize),
    ArrayIndex(usize),
    VariantField { variant: VariantId, field: FieldId },
    ClosureCapture(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropActionKind {
    /// A leaf whose owned representation supplies its release hook in the
    /// owning representation milestone. The flag transition is already exact.
    StructuralLeaf,
    Custom(FunctionInstance),
    OwnedString,
    OwnedVec {
        elements: Vec<DropAction>,
    },
    OwnedMap {
        keys: Vec<DropAction>,
        values: Vec<DropAction>,
    },
    OwnedSet {
        elements: Vec<DropAction>,
    },
    OwnedBox {
        value: Vec<DropAction>,
    },
    OwnedShared {
        owner: TypeId,
    },
    OwnedWeak {
        owner: TypeId,
    },
    OwnedStore {
        owner: TypeId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropAction {
    pub ty: TypeId,
    pub projections: Vec<DropProjection>,
    pub kind: DropActionKind,
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
    pub drop_requirements: Vec<DropRequirement>,
    /// Compatibility locals whose address is taken and therefore need stable
    /// process-lifetime storage. Always empty for the owned value model.
    pub promoted_locals: BTreeSet<LocalBindingId>,
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
    pub semantic_revision: crate::config::SemanticRevision,
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
