//! Checked facts consumed by typed-IR lowering.

use super::*;

/// Stable identities minted by structural borrow analysis. They are separate
/// from source type identity because Elamite has no written lifetime syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LoanId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProvenanceVariableId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProvenanceConstraintId(pub u32);

/// Milestones 6-7 output: per-expression classification and copied pattern
/// bindings consumed by later lowering passes. Expression facts are keyed by
/// source span since the syntax tree has no per-node stable identity.
#[derive(Debug, Default)]
pub struct CheckedProgram {
    pub expression_types: BTreeMap<Span, TypeId>,
    pub expression_places: BTreeMap<Span, PlaceKind>,
    /// One explicit ownership intent for every checked value expression.
    /// Place projections are attached during typed lowering, after member and
    /// receiver selection has stable identities.
    pub ownership_uses: BTreeMap<Span, Vec<OwnershipUse>>,
    /// Validated implicit conversions from concrete safe references to
    /// trait-object references. Lowering consumes these facts to construct the
    /// target-plus-vtable fat reference at the contextual conversion point.
    pub trait_object_coercions: BTreeMap<Span, TraitObjectCoercion>,
    /// Expressions that produce an explicit logical-value copy, including
    /// the source-level purpose and lifetime boundary selected by checking.
    /// Typed lowering adds allocation behavior from the concrete value type.
    pub copies: BTreeMap<Span, LogicalCopyContext>,
    /// Match-pattern and local tuple-destructuring bindings receive
    /// independent logical-value copies. Lowering consumes these stable
    /// identities when it materializes component extraction.
    pub copied_pattern_bindings: BTreeSet<LocalBindingId>,
    /// Canonical component type selected for each copied binding.
    pub pattern_binding_types: BTreeMap<LocalBindingId, TypeId>,
    /// Named functions and type-selected methods used as first-class function
    /// references. The key is the complete expression span, not merely its
    /// final identifier, so lowering can distinguish a type selection from a
    /// bound member expression.
    pub function_references: BTreeMap<Span, FunctionInstance>,
    /// Callable resolution selected for each call expression.
    pub calls: BTreeMap<Span, CheckedCall>,
    /// User-defined `for` loops selected through the standard
    /// `Iterator[Element]` trait. Built-in collections retain their dedicated
    /// typed-IR kinds and therefore need no entry here.
    pub iterations: BTreeMap<Span, CheckedIteration>,
    pub closures: BTreeMap<DeclarationId, CheckedClosure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckedIteration {
    pub trait_declaration: DeclarationId,
    pub element_type: TypeId,
}

#[derive(Debug, Clone)]
pub struct CheckedClosureCapture {
    pub source: LocalBindingId,
    pub binding: LocalBindingId,
    pub kind: ClosureCaptureKind,
    pub source_ty: TypeId,
    pub ty: TypeId,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct CheckedClosure {
    pub declaration: DeclarationId,
    pub ty: TypeId,
    pub captures: Vec<CheckedClosureCapture>,
}

/// One validated contextual conversion from `&Concrete`/`&var Concrete` to a
/// matching trait-object reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraitObjectCoercion {
    pub source: TypeId,
    pub target: TypeId,
    pub trait_declaration: DeclarationId,
    pub concrete: TypeId,
}

/// The callable selected for one checked call expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedCall {
    Direct(FunctionInstance),
    BoundMethod {
        instance: FunctionInstance,
        adjustment: ReceiverAdjustment,
    },
    Indirect,
    Closure {
        declaration: DeclarationId,
    },
    /// Call syntax justified by a `Callable[Arguments, Return]` bound. The
    /// concrete monomorphized type decides whether lowering emits a closure
    /// environment call, an indirect named-function call, or a user impl.
    CallableBound {
        trait_declaration: DeclarationId,
        receiver_type: TypeId,
        parameters: Vec<TypeId>,
    },
    CallableDynamic {
        trait_declaration: DeclarationId,
        slot: usize,
        parameters: Vec<TypeId>,
    },
    /// `Type.default()` supplied by a `Default` derivation, which has no
    /// declaration to call.
    DerivedDefault {
        ty: TypeId,
    },
    /// `Target.try_from(value)`, `Target.wrapping_from(value)`, or
    /// `Target.saturating_from(value)` (`docs/spec.md` 4.1). A primitive has no
    /// declaration, so this records the selection instead. `result` is
    /// `Result[Target, NumericError]` for the checked form and `Target`
    /// otherwise.
    NumericConversion {
        outcome: NumericOutcome,
        source: TypeId,
        target: TypeId,
        result: TypeId,
    },
    /// `value.checked_add(other)` and the other standard alternatives to the
    /// trapping arithmetic operators (`docs/spec.md` 4.1).
    NumericAlternative {
        operation: NumericAlternative,
        operand_type: TypeId,
        result: TypeId,
    },
    /// A compiler-supplied operation on text, arrays, or standard
    /// collections. These types intentionally have no source declarations;
    /// recording the exact operation here keeps their surface in semantic
    /// checking rather than teaching name resolution about synthetic methods.
    Standard(StandardCall),
    /// A call on `self` inside a trait's default body. `Self` is unknown when
    /// the body is checked, so the concrete implementation is selected when the
    /// body is specialized for an implementing type.
    TraitSelfMethod {
        trait_declaration: DeclarationId,
        method: DeclarationId,
    },
    /// A statically dispatched call through a user-declared trait bound on a
    /// generic parameter. The generic body is checked against the trait
    /// declaration; monomorphization selects the concrete implementation.
    GenericBoundMethod {
        trait_declaration: DeclarationId,
        method: DeclarationId,
        receiver_type: TypeId,
        adjustment: ReceiverAdjustment,
    },
    /// A call through a trait object's vtable. The receiver is the object
    /// itself; `slot` is the method's index in the trait's vtable, ordered by
    /// method name so the layout is deterministic.
    DynamicMethod {
        trait_declaration: DeclarationId,
        method: DeclarationId,
        slot: usize,
    },
    Print {
        newline: bool,
    },
}

pub struct CheckOutput {
    pub program: CheckedProgram,
    pub diagnostics: Vec<Diagnostic>,
}
