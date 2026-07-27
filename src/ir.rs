//! Typed high-level IR and explicit control-flow IR (`IMPL.md` Milestones
//! 8-11).
//!
//! The high-level form owns selected declaration and type identities while
//! retaining source spans, place classifications, and explicit logical-copy
//! markers from [`crate::check`]. The control-flow form then makes evaluation
//! order, temporaries, branches, loops, calls, returns, and potentially
//! trapping operations explicit before any C is emitted. Milestone 9 adds the
//! canonical logical-copy contract and source-ordered pattern-match lowering.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::check::{CheckedCall, CheckedProgram, ReceiverAdjustment};
use crate::diagnostics::{Category, Diagnostic};
use crate::lexer::{FormattedSegmentKind, Keyword, Token, TokenKind};
use crate::parser::{SyntaxElement, SyntaxKind, SyntaxNode};
use crate::resolution::{
    DeclarationId, DeclarationKind, FieldId, ItemId, LocalBindingId, LocalBindingKind, MemberId,
    NameTarget, ResolvedProgram, VariantId,
};
use crate::source::Span;
use crate::types::{
    FunctionInstance, PlaceKind, PrimitiveType, Substitution, TypeContext, TypeId, TypeKind,
    TypedProgram, parse_integer_magnitude,
};

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

/// Backend-level copy contract for a canonical type. Later representations
/// must select a strategy here before C lowering can accept them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalCopyStrategy {
    /// Plain immutable scalars and views can use an ordinary C value copy.
    Trivial,
    /// Explicit aliases copy their identity rather than the pointee.
    PreserveIdentity,
    /// Inline aggregates recursively copy each ordinary field or element.
    Recursive,
    /// Mutable `String` owns a buffer that must be duplicated eagerly until a
    /// copy-on-write representation is introduced.
    OwnedString,
    /// A later runtime-owned representation must supply its copy operation
    /// before code generation is enabled.
    RuntimeManaged,
}

#[must_use]
pub fn logical_copy_strategy(types: &TypeContext, mut ty: TypeId) -> LogicalCopyStrategy {
    loop {
        return match types.kind(ty) {
            TypeKind::Alias { target, .. } => {
                ty = *target;
                continue;
            }
            TypeKind::Primitive(PrimitiveType::String) => LogicalCopyStrategy::OwnedString,
            TypeKind::Primitive(_) => LogicalCopyStrategy::Trivial,
            TypeKind::Tuple(_) | TypeKind::Array { .. } | TypeKind::Nominal { .. } => {
                LogicalCopyStrategy::Recursive
            }
            TypeKind::Reference { .. }
            | TypeKind::RawPointer { .. }
            | TypeKind::Function { .. }
            | TypeKind::TraitObject { .. }
            | TypeKind::Slice(_) => LogicalCopyStrategy::PreserveIdentity,
            TypeKind::Builtin { .. }
            | TypeKind::Foreign { .. }
            | TypeKind::GenericParameter(_)
            | TypeKind::SelfType(_)
            | TypeKind::InferenceVariable(_)
            | TypeKind::Error => LogicalCopyStrategy::RuntimeManaged,
        };
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrapKind {
    IntegerOverflow,
    DivisionByZero,
    InvalidShift,
    IndexOutOfBounds,
    InvalidNumericConversion,
}

impl TrapKind {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::IntegerOverflow => "E-RUN-OVERFLOW",
            Self::DivisionByZero => "E-RUN-DIVZERO",
            Self::InvalidShift => "E-RUN-SHIFT",
            Self::IndexOutOfBounds => "E-RUN-INDEX",
            Self::InvalidNumericConversion => "E-RUN-CAST",
        }
    }
}

#[derive(Debug, Clone)]
pub enum TypedCallee {
    Function(FunctionInstance),
    Indirect(Box<TypedExpression>),
    Print { newline: bool },
}

#[derive(Debug, Clone)]
pub enum FormattedPart {
    Text(String),
    Expression(TypedExpression),
}

#[derive(Debug, Clone)]
pub enum TypedExpressionKind {
    Constant(Constant),
    FunctionReference(FunctionInstance),
    Local(LocalBindingId),
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
    Field {
        base: Box<TypedPlace>,
        field: FieldId,
        ty: TypeId,
        span: Span,
    },
    Index {
        base: Box<TypedPlace>,
        index: TypedExpression,
        ty: TypeId,
        length: u128,
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
            | Self::Field { ty, .. }
            | Self::Index { ty, .. }
            | Self::Dereference { ty, .. } => *ty,
        }
    }

    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Local { span, .. }
            | Self::Field { span, .. }
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
    Match {
        scrutinee: TypedExpression,
        arms: Vec<TypedMatchArm>,
    },
    Block(Vec<TypedStatement>),
    Break,
    Continue,
    Pass,
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
    /// rather than a C stack slot (`IMPL.md` Milestone 10). Conservative: every
    /// address-taken local is promoted.
    pub promoted_locals: BTreeSet<LocalBindingId>,
    /// Whether the body allocates a managed cell for a referenced composite
    /// literal. Such a cell has no binding, so promotion alone does not imply
    /// it.
    pub allocates_managed: bool,
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
}

pub struct TypedIrOutput {
    pub program: TypedIrProgram,
    pub diagnostics: Vec<Diagnostic>,
}

/// Monomorphizes and lowers the pre-trait executable subset into typed
/// high-level IR. Features assigned to later milestones are not selected.
#[must_use]
pub fn lower_typed_ir(
    resolved: &ResolvedProgram,
    typed: &mut TypedProgram,
    checked: &CheckedProgram,
) -> TypedIrOutput {
    TypedLowerer::new(resolved, typed, checked).run()
}

#[derive(Clone)]
struct PendingInstance {
    instance: FunctionInstance,
    ancestry: Vec<FunctionInstance>,
}

struct TypedLowerer<'a> {
    resolved: &'a ResolvedProgram,
    typed: &'a mut TypedProgram,
    checked: &'a CheckedProgram,
    binding_by_span: BTreeMap<Span, LocalBindingId>,
    diagnostics: Vec<Diagnostic>,
    substitution: Substitution,
    current_instance: Option<FunctionInstance>,
    ancestry: Vec<FunctionInstance>,
    pending: VecDeque<PendingInstance>,
    queued: BTreeSet<FunctionInstance>,
}

impl<'a> TypedLowerer<'a> {
    fn new(
        resolved: &'a ResolvedProgram,
        typed: &'a mut TypedProgram,
        checked: &'a CheckedProgram,
    ) -> Self {
        let binding_by_span = resolved
            .local_bindings
            .iter()
            .map(|binding| (binding.span, binding.id))
            .collect();
        Self {
            resolved,
            typed,
            checked,
            binding_by_span,
            diagnostics: Vec::new(),
            substitution: Substitution::new(),
            current_instance: None,
            ancestry: Vec::new(),
            pending: VecDeque::new(),
            queued: BTreeSet::new(),
        }
    }

    fn run(mut self) -> TypedIrOutput {
        let mut program = TypedIrProgram::default();
        let roots = self
            .resolved
            .declarations
            .iter()
            .filter(|declaration| {
                declaration.kind == DeclarationKind::Function
                    && declaration.parent_impl.is_none()
                    && self
                        .typed
                        .callable_generic_parameters(self.resolved, declaration.id)
                        .is_empty()
                    && !declaration.parent_declaration.is_some_and(|parent| {
                        self.resolved.declarations[parent.index()].kind == DeclarationKind::Trait
                    })
            })
            .map(|declaration| FunctionInstance {
                declaration: declaration.id,
                arguments: Vec::new(),
            })
            .collect::<Vec<_>>();
        for instance in roots {
            let span = self.resolved.declarations[instance.declaration.index()].span;
            self.enqueue(instance, Vec::new(), span);
        }
        while let Some(pending) = self.pending.pop_front() {
            self.current_instance = Some(pending.instance.clone());
            self.ancestry = pending.ancestry;
            self.substitution = self
                .typed
                .instance_substitution(self.resolved, &pending.instance);
            if let Some(function) = self.lower_function(&pending.instance) {
                program.functions.push(function);
            }
        }
        self.collect_concrete_nominals(&mut program);
        TypedIrOutput {
            program,
            diagnostics: self.diagnostics,
        }
    }

    fn concrete_type(&mut self, ty: TypeId) -> TypeId {
        self.typed.types.substitute(ty, &self.substitution)
    }

    fn concrete_instance(&mut self, instance: &FunctionInstance) -> FunctionInstance {
        FunctionInstance {
            declaration: instance.declaration,
            arguments: instance
                .arguments
                .iter()
                .map(|argument| self.concrete_type(*argument))
                .collect(),
        }
    }

    fn enqueue(&mut self, instance: FunctionInstance, ancestry: Vec<FunctionInstance>, span: Span) {
        if ancestry.iter().any(|ancestor| {
            ancestor.declaration == instance.declaration
                && ancestor.arguments != instance.arguments
                && ancestor
                    .arguments
                    .iter()
                    .zip(&instance.arguments)
                    .any(|(old, new)| old != new && self.typed.types.contains_type(*new, *old))
        }) {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::TypeSystem,
                    "generic instantiation expands without bound",
                )
                .with_primary(span),
            );
            return;
        }
        if self.queued.insert(instance.clone()) {
            self.pending
                .push_back(PendingInstance { instance, ancestry });
        }
    }

    fn enqueue_reachable(&mut self, instance: FunctionInstance, span: Span) {
        if self.resolved.declarations[instance.declaration.index()].kind
            != DeclarationKind::Function
        {
            return;
        }
        let mut ancestry = self.ancestry.clone();
        if let Some(current) = &self.current_instance {
            ancestry.push(current.clone());
        }
        self.enqueue(instance, ancestry, span);
    }

    fn collect_concrete_nominals(&mut self, program: &mut TypedIrProgram) {
        let concrete = (0..self.typed.types.len())
            .filter_map(|index| {
                let ty = self.typed.types.id_at(index)?;
                (!self.typed.types.contains_generic_parameter(ty)).then_some(ty)
            })
            .collect::<Vec<_>>();
        for ty in concrete {
            let TypeKind::Nominal {
                identity,
                arguments,
            } = self.typed.types.kind(ty).clone()
            else {
                continue;
            };
            let declaration = &self.resolved.declarations[identity.declaration.index()];
            if declaration.kind == DeclarationKind::Struct {
                let fields = self
                    .resolved
                    .fields
                    .iter()
                    .filter(|field| {
                        field.parent_declaration == declaration.id && field.parent_variant.is_none()
                    })
                    .filter_map(|field| {
                        self.typed
                            .instantiate_field_type(self.resolved, field.id, &arguments)
                            .map(|field_type| {
                                (
                                    field.id,
                                    self.resolved.symbol_text(field.name).to_string(),
                                    field_type,
                                )
                            })
                    })
                    .collect();
                program.structs.push(TypedStruct {
                    ty,
                    declaration: declaration.id,
                    name: self.resolved.symbol_text(declaration.name).to_string(),
                    fields,
                });
            } else if declaration.kind == DeclarationKind::Enum {
                let variants = self
                    .resolved
                    .variants
                    .iter()
                    .filter(|variant| variant.parent == declaration.id)
                    .map(|variant| TypedVariant {
                        id: variant.id,
                        name: self.resolved.symbol_text(variant.name).to_string(),
                        fields: variant
                            .fields
                            .iter()
                            .filter_map(|field_id| {
                                let field = &self.resolved.fields[field_id.index()];
                                self.typed
                                    .instantiate_field_type(self.resolved, *field_id, &arguments)
                                    .map(|field_type| {
                                        (
                                            *field_id,
                                            self.resolved.symbol_text(field.name).to_string(),
                                            field_type,
                                        )
                                    })
                            })
                            .collect(),
                    })
                    .collect();
                program.enums.push(TypedEnum {
                    ty,
                    declaration: declaration.id,
                    name: self.resolved.symbol_text(declaration.name).to_string(),
                    variants,
                });
            }
        }
    }

    fn lower_function(&mut self, instance: &FunctionInstance) -> Option<TypedFunction> {
        let declaration = instance.declaration;
        let data = &self.resolved.declarations[declaration.index()];
        let signature = self.typed.instantiate_signature(self.resolved, instance)?;
        let mut parameters = Vec::new();
        let mut local_types = BTreeMap::new();
        if let Some(parameter_list) =
            crate::types::direct_child(&data.syntax, SyntaxKind::Parameters)
        {
            let nodes = crate::types::direct_children(parameter_list, SyntaxKind::Parameter);
            let mut ordinary_parameters = signature.parameters.iter();
            for node in nodes {
                let is_receiver = crate::types::direct_tokens(node)
                    .iter()
                    .any(|token| matches!(token.kind, TokenKind::Keyword(Keyword::SelfValue)));
                let parameter_type = if is_receiver {
                    signature.receiver
                } else {
                    ordinary_parameters.next().map(|parameter| {
                        if parameter.variadic {
                            self.typed
                                .types
                                .id_for_kind(&TypeKind::Slice(parameter.ty))
                                .expect("checking interns every variadic binding's slice type")
                        } else {
                            parameter.ty
                        }
                    })
                };
                let Some(parameter_type) = parameter_type else {
                    continue;
                };
                let Some(token) = parameter_name_token(node) else {
                    continue;
                };
                let Some(binding) = self.binding_by_span.get(&token.span).copied() else {
                    continue;
                };
                parameters.push(TypedParameter {
                    binding,
                    ty: parameter_type,
                    span: token.span,
                });
                local_types.insert(binding, parameter_type);
            }
        }
        // Promotion is computed from the function's syntax rather than from
        // lowered statements, so it is available even for bodies whose
        // reference lowering is not yet represented.
        let body_block = crate::types::direct_child(&data.syntax, SyntaxKind::Block);
        let promoted_locals = body_block
            .map(|block| crate::promotion::address_taken_locals(self.resolved, block))
            .unwrap_or_default();
        let allocates_managed =
            body_block.is_some_and(crate::promotion::allocates_managed_temporary);
        let body = crate::types::direct_child(&data.syntax, SyntaxKind::Block)
            .map(|block| self.lower_block(block, &mut local_types))
            .unwrap_or_default();
        Some(TypedFunction {
            declaration,
            instance: instance.clone(),
            name: self.resolved.symbol_text(data.name).to_string(),
            span: data.span,
            parameters,
            return_type: signature.return_type,
            body,
            local_types,
            promoted_locals,
            allocates_managed,
        })
    }

    fn lower_block(
        &mut self,
        block: &SyntaxNode,
        local_types: &mut BTreeMap<LocalBindingId, TypeId>,
    ) -> Vec<TypedStatement> {
        let mut statements = Vec::new();
        for node in child_nodes(block) {
            let before = self.diagnostics.len();
            match self.lower_statement(node, local_types) {
                Some(statement) => statements.push(statement),
                // A statement that cannot be lowered must be reported. Dropping
                // it silently would generate a program missing an effect the
                // source asked for.
                None if self.diagnostics.len() == before => {
                    self.unsupported(node.span, "this statement");
                }
                None => {}
            }
        }
        statements
    }

    fn lower_statement(
        &mut self,
        node: &SyntaxNode,
        local_types: &mut BTreeMap<LocalBindingId, TypeId>,
    ) -> Option<TypedStatement> {
        let kind = match node.kind {
            SyntaxKind::LetStatement => {
                let value_node = child_nodes(node).into_iter().next_back()?;
                let value = self.lower_expression(value_node)?;
                let token = let_name_token(node)?;
                let binding = self.binding_by_span.get(&token.span).copied()?;
                let ty = self
                    .checked
                    .expression_types
                    .get(&value_node.span)
                    .copied()
                    .unwrap_or(value.ty);
                let ty = self.concrete_type(ty);
                local_types.insert(binding, ty);
                let mutable = node.children.iter().any(|child| {
                    matches!(
                        child,
                        SyntaxElement::Token(Token {
                            kind: TokenKind::Keyword(Keyword::Var),
                            ..
                        })
                    )
                });
                TypedStatementKind::Let {
                    binding,
                    mutable,
                    ty,
                    value,
                }
            }
            SyntaxKind::AssignmentStatement => {
                let nodes = child_nodes(node);
                let target = *nodes.first()?;
                let Some(place) = self.lower_place(target) else {
                    self.unsupported(target.span, "an assignment to this place");
                    return None;
                };
                let value = self.lower_expression(*nodes.get(1)?)?;
                let operator = node.children.iter().find_map(|child| match child {
                    SyntaxElement::Token(token) => assignment_operator(&token.kind),
                    SyntaxElement::Node(_) => None,
                })?;
                TypedStatementKind::Assign {
                    place,
                    operator,
                    value,
                }
            }
            SyntaxKind::ExpressionStatement => TypedStatementKind::Expression(
                self.lower_expression(child_nodes(node).into_iter().next()?)?,
            ),
            SyntaxKind::ReturnStatement => TypedStatementKind::Return(
                child_nodes(node)
                    .into_iter()
                    .next()
                    .and_then(|expression| self.lower_expression(expression)),
            ),
            SyntaxKind::IfStatement => {
                let condition_node = child_nodes(node).into_iter().find(|child| {
                    child.kind != SyntaxKind::Block && child.kind != SyntaxKind::ElseClause
                })?;
                let condition = self.lower_expression(condition_node)?;
                let then_body = child_nodes(node)
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)
                    .map(|block| self.lower_block(block, local_types))
                    .unwrap_or_default();
                let else_body = child_nodes(node)
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::ElseClause)
                    .and_then(|clause| {
                        child_nodes(clause)
                            .into_iter()
                            .find(|child| child.kind == SyntaxKind::Block)
                    })
                    .map(|block| self.lower_block(block, local_types))
                    .unwrap_or_default();
                TypedStatementKind::If {
                    condition,
                    then_body,
                    else_body,
                }
            }
            SyntaxKind::WhileStatement => {
                let condition_node = child_nodes(node)
                    .into_iter()
                    .find(|child| child.kind != SyntaxKind::Block)?;
                let condition = self.lower_expression(condition_node)?;
                let body = child_nodes(node)
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)
                    .map(|block| self.lower_block(block, local_types))
                    .unwrap_or_default();
                TypedStatementKind::While { condition, body }
            }
            SyntaxKind::MatchStatement => {
                let nodes = child_nodes(node);
                let scrutinee_node = nodes
                    .iter()
                    .copied()
                    .find(|child| child.kind != SyntaxKind::Block)?;
                let scrutinee = self.lower_expression(scrutinee_node)?;
                let arm_block = nodes
                    .iter()
                    .copied()
                    .find(|child| child.kind == SyntaxKind::Block)?;
                let mut arms = Vec::new();
                for arm in child_nodes(arm_block)
                    .into_iter()
                    .filter(|child| child.kind == SyntaxKind::MatchArm)
                {
                    let pattern_node = child_nodes(arm).into_iter().find(|child| {
                        matches!(
                            child.kind,
                            SyntaxKind::Pattern | SyntaxKind::AlternativePattern
                        )
                    })?;
                    let bindings = self.pattern_bindings(arm);
                    let pattern =
                        self.lower_pattern(pattern_node, scrutinee.ty, &bindings, local_types)?;
                    let guard = child_nodes(arm)
                        .into_iter()
                        .find(|child| child.kind == SyntaxKind::Guard)
                        .and_then(|guard| child_nodes(guard).into_iter().next())
                        .and_then(|expression| self.lower_expression(expression));
                    let body = child_nodes(arm)
                        .into_iter()
                        .find(|child| child.kind == SyntaxKind::Block)
                        .map(|block| self.lower_block(block, local_types))
                        .unwrap_or_default();
                    arms.push(TypedMatchArm {
                        pattern,
                        guard,
                        body,
                        span: arm.span,
                    });
                }
                TypedStatementKind::Match { scrutinee, arms }
            }
            SyntaxKind::UnsafeBlock => {
                let block = child_nodes(node)
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)?;
                TypedStatementKind::Block(self.lower_block(block, local_types))
            }
            SyntaxKind::BreakStatement => TypedStatementKind::Break,
            SyntaxKind::ContinueStatement => TypedStatementKind::Continue,
            SyntaxKind::PassStatement => TypedStatementKind::Pass,
            SyntaxKind::ForStatement | SyntaxKind::DeferStatement => {
                self.unsupported(
                    node.span,
                    match node.kind {
                        SyntaxKind::ForStatement => "`for` lowering",
                        SyntaxKind::DeferStatement => "`defer` lowering",
                        _ => unreachable!(),
                    },
                );
                return None;
            }
            _ => return None,
        };
        Some(TypedStatement {
            span: node.span,
            kind,
        })
    }

    fn pattern_bindings(&self, arm: &SyntaxNode) -> BTreeMap<String, LocalBindingId> {
        self.resolved
            .local_bindings
            .iter()
            .filter(|binding| {
                binding.kind == LocalBindingKind::PatternCandidate
                    && binding.span.file == arm.span.file
                    && binding.span.start >= arm.span.start
                    && binding.span.end <= arm.span.end
            })
            .map(|binding| {
                (
                    self.resolved.symbol_text(binding.name).to_string(),
                    binding.id,
                )
            })
            .collect()
    }

    fn lower_pattern(
        &mut self,
        node: &SyntaxNode,
        expected: TypeId,
        bindings: &BTreeMap<String, LocalBindingId>,
        local_types: &mut BTreeMap<LocalBindingId, TypeId>,
    ) -> Option<TypedPattern> {
        let kind = match node.kind {
            SyntaxKind::AlternativePattern => TypedPatternKind::Alternative(
                child_nodes(node)
                    .into_iter()
                    .map(|child| self.lower_pattern(child, expected, bindings, local_types))
                    .collect::<Option<Vec<_>>>()?,
            ),
            SyntaxKind::DereferencePattern => {
                let target = match self.expanded_kind(expected) {
                    TypeKind::Reference { target, .. } => *target,
                    TypeKind::Error => self.typed.types.error(),
                    _ => {
                        self.unsupported(node.span, "an invalid dereference pattern");
                        return None;
                    }
                };
                let inner = child_nodes(node).into_iter().next()?;
                TypedPatternKind::Dereference(Box::new(self.lower_pattern(
                    inner,
                    target,
                    bindings,
                    local_types,
                )?))
            }
            SyntaxKind::TuplePattern => {
                let elements = child_nodes(node);
                let member_types = match self.expanded_kind(expected) {
                    TypeKind::Tuple(members) => members.clone(),
                    TypeKind::Error => vec![self.typed.types.error(); elements.len()],
                    _ => return None,
                };
                TypedPatternKind::Tuple(
                    elements
                        .into_iter()
                        .zip(member_types)
                        .map(|(element, ty)| self.lower_pattern(element, ty, bindings, local_types))
                        .collect::<Option<Vec<_>>>()?,
                )
            }
            SyntaxKind::RecordPattern | SyntaxKind::VariantPattern => {
                return self.lower_aggregate_pattern(node, expected, bindings, local_types);
            }
            SyntaxKind::Pattern => {
                if let Some(inner) = child_nodes(node).into_iter().next() {
                    return self.lower_pattern(inner, expected, bindings, local_types);
                }
                if let Some(constant) = lower_constant(node, false) {
                    TypedPatternKind::Literal(constant)
                } else if let Some((_, variant)) = self.resolve_pattern_variant(node) {
                    TypedPatternKind::Variant {
                        variant,
                        fields: Vec::new(),
                    }
                } else {
                    let token = first_identifier(node)?;
                    let name = identifier_text(token);
                    if name == "_" {
                        TypedPatternKind::Wildcard
                    } else {
                        let binding = *bindings.get(name)?;
                        let ty = self
                            .checked
                            .pattern_binding_types
                            .get(&binding)
                            .copied()
                            .unwrap_or(expected);
                        let ty = self.concrete_type(ty);
                        local_types.insert(binding, ty);
                        TypedPatternKind::Binding(binding)
                    }
                }
            }
            _ => return None,
        };
        Some(TypedPattern {
            ty: expected,
            span: node.span,
            kind,
        })
    }

    fn lower_aggregate_pattern(
        &mut self,
        node: &SyntaxNode,
        expected: TypeId,
        bindings: &BTreeMap<String, LocalBindingId>,
        local_types: &mut BTreeMap<LocalBindingId, TypeId>,
    ) -> Option<TypedPattern> {
        if let Some((_, variant)) = self.resolve_pattern_variant(node) {
            let required = self.resolved.variants[variant.index()].fields.clone();
            let fields =
                self.lower_pattern_fields(node, &required, bindings, local_types, expected)?;
            return Some(TypedPattern {
                ty: expected,
                span: node.span,
                kind: TypedPatternKind::Variant { variant, fields },
            });
        }
        let declaration = match self.expanded_kind(expected) {
            TypeKind::Nominal { identity, .. } => identity.declaration,
            _ => return None,
        };
        let required = self
            .resolved
            .fields
            .iter()
            .filter(|field| {
                field.parent_declaration == declaration && field.parent_variant.is_none()
            })
            .map(|field| field.id)
            .collect::<Vec<_>>();
        let fields = self.lower_pattern_fields(node, &required, bindings, local_types, expected)?;
        Some(TypedPattern {
            ty: expected,
            span: node.span,
            kind: TypedPatternKind::Struct {
                declaration,
                fields,
            },
        })
    }

    fn lower_pattern_fields(
        &mut self,
        node: &SyntaxNode,
        required: &[FieldId],
        bindings: &BTreeMap<String, LocalBindingId>,
        local_types: &mut BTreeMap<LocalBindingId, TypeId>,
        expected: TypeId,
    ) -> Option<Vec<(FieldId, TypedPattern)>> {
        if node.kind == SyntaxKind::VariantPattern {
            return child_nodes(node)
                .into_iter()
                .zip(required.iter().copied())
                .map(|(pattern, field)| {
                    let ty = self.instantiated_pattern_field_type(field, expected)?;
                    Some((
                        field,
                        self.lower_pattern(pattern, ty, bindings, local_types)?,
                    ))
                })
                .collect();
        }
        let mut fields = Vec::new();
        for field_node in child_nodes(node)
            .into_iter()
            .filter(|child| child.kind == SyntaxKind::PatternField)
        {
            if matches!(
                field_node.children.first(),
                Some(SyntaxElement::Token(Token {
                    kind: TokenKind::DotDot,
                    ..
                }))
            ) {
                continue;
            }
            let token = first_identifier(field_node)?;
            let name = identifier_text(token);
            let field = required.iter().copied().find(|field| {
                self.resolved
                    .symbol_text(self.resolved.fields[field.index()].name)
                    == name
            })?;
            let ty = self.instantiated_pattern_field_type(field, expected)?;
            let pattern = if let Some(nested) = child_nodes(field_node).into_iter().next() {
                self.lower_pattern(nested, ty, bindings, local_types)?
            } else {
                let binding = *bindings.get(name)?;
                local_types.insert(binding, ty);
                TypedPattern {
                    ty,
                    span: token.span,
                    kind: TypedPatternKind::Binding(binding),
                }
            };
            fields.push((field, pattern));
        }
        Some(fields)
    }

    fn instantiated_pattern_field_type(
        &mut self,
        field: FieldId,
        expected: TypeId,
    ) -> Option<TypeId> {
        let expected = self.concrete_type(expected);
        let arguments = match self.expanded_kind(expected) {
            TypeKind::Nominal { arguments, .. } => arguments.clone(),
            _ => Vec::new(),
        };
        self.typed
            .instantiate_field_type(self.resolved, field, &arguments)
            .map(|ty| self.concrete_type(ty))
    }

    fn resolve_pattern_variant(&self, node: &SyntaxNode) -> Option<(DeclarationId, VariantId)> {
        let tokens = pattern_path_tokens(node);
        let first = tokens.first()?;
        let last = tokens
            .iter()
            .rev()
            .find(|token| matches!(token.kind, TokenKind::Identifier(_)))?;
        if first.span == last.span {
            return None;
        }
        let NameTarget::Item(ItemId::Declaration(declaration)) =
            self.resolved.reference_at(first.span)?.target
        else {
            return None;
        };
        if self.resolved.declarations[declaration.index()].kind != DeclarationKind::Enum {
            return None;
        }
        match self.find_member(declaration, identifier_text(last)) {
            Some(MemberId::Variant(variant)) => Some((declaration, variant)),
            _ => None,
        }
    }

    fn lower_expression(&mut self, node: &SyntaxNode) -> Option<TypedExpression> {
        let template = self
            .checked
            .expression_types
            .get(&node.span)
            .copied()
            .unwrap_or_else(|| self.typed.types.error());
        let ty = self.concrete_type(template);
        if ty == self.typed.types.error() {
            self.unsupported(
                node.span,
                "an expression whose type belongs to a later milestone",
            );
            return None;
        }
        let place = self
            .checked
            .expression_places
            .get(&node.span)
            .copied()
            .unwrap_or(PlaceKind::Value);
        let kind = match node.kind {
            SyntaxKind::LiteralExpression => {
                TypedExpressionKind::Constant(lower_constant(node, false)?)
            }
            SyntaxKind::NameExpression => {
                if let Some(instance) = self.checked.function_references.get(&node.span).cloned() {
                    let instance = self.concrete_instance(&instance);
                    self.enqueue_reachable(instance.clone(), node.span);
                    TypedExpressionKind::FunctionReference(instance)
                } else {
                    let token = first_token(node)?;
                    let target = self.resolved.reference_at(token.span)?.target;
                    match target {
                        NameTarget::Local(binding) => TypedExpressionKind::Local(binding),
                        _ => {
                            self.unsupported(node.span, "a non-local value reference");
                            return None;
                        }
                    }
                }
            }
            SyntaxKind::UnaryExpression => {
                let operand_node = child_nodes(node).into_iter().next_back()?;
                let token = first_token(node)?;
                if matches!(token.kind, TokenKind::Amp) {
                    // A referenced composite literal has no place to address;
                    // it allocates a managed cell of its own (SPEC 3.2).
                    if operand_node.kind == SyntaxKind::RecordExpression {
                        let value = self.lower_expression(operand_node)?;
                        return Some(TypedExpression {
                            kind: TypedExpressionKind::AddressOfTemporary(Box::new(value)),
                            ty,
                            place,
                            copy: false,
                            span: node.span,
                        });
                    }
                    let Some(target) = self.lower_place(operand_node) else {
                        self.unsupported(node.span, "a reference to this place");
                        return None;
                    };
                    return Some(TypedExpression {
                        kind: TypedExpressionKind::AddressOf(Box::new(target)),
                        ty,
                        place,
                        copy: false,
                        span: node.span,
                    });
                }
                if matches!(token.kind, TokenKind::Star) {
                    let operand = self.lower_expression(operand_node)?;
                    return Some(TypedExpression {
                        kind: TypedExpressionKind::Dereference(Box::new(operand)),
                        ty,
                        place,
                        copy: self.checked.copies.contains(&node.span),
                        span: node.span,
                    });
                }
                let operator = unary_operator(&token.kind)?;
                let negative_literal = operator == UnaryOperator::Negative
                    && operand_node.kind == SyntaxKind::LiteralExpression;
                if negative_literal {
                    if let Some(constant) = lower_constant(operand_node, true) {
                        TypedExpressionKind::Constant(constant)
                    } else {
                        TypedExpressionKind::Unary {
                            operator,
                            operand: Box::new(self.lower_expression(operand_node)?),
                        }
                    }
                } else {
                    TypedExpressionKind::Unary {
                        operator,
                        operand: Box::new(self.lower_expression(operand_node)?),
                    }
                }
            }
            SyntaxKind::BinaryExpression => {
                let nodes = child_nodes(node);
                let operator = node.children.iter().find_map(|child| match child {
                    SyntaxElement::Token(token) => binary_operator(&token.kind),
                    SyntaxElement::Node(_) => None,
                })?;
                TypedExpressionKind::Binary {
                    operator,
                    left: Box::new(self.lower_expression(*nodes.first()?)?),
                    right: Box::new(self.lower_expression(*nodes.get(1)?)?),
                }
            }
            SyntaxKind::CastExpression => TypedExpressionKind::Cast {
                value: Box::new(self.lower_expression(child_nodes(node).into_iter().next()?)?),
            },
            SyntaxKind::CallExpression => self.lower_call(node)?,
            SyntaxKind::MemberExpression => {
                if let Some(instance) = self.checked.function_references.get(&node.span).cloned() {
                    let instance = self.concrete_instance(&instance);
                    self.enqueue_reachable(instance.clone(), node.span);
                    TypedExpressionKind::FunctionReference(instance)
                } else if let Some((declaration, variant)) =
                    self.resolve_enum_variant_expression(node)
                {
                    TypedExpressionKind::Enum {
                        declaration,
                        variant,
                        fields: Vec::new(),
                    }
                } else {
                    let base_node = child_nodes(node).into_iter().next()?;
                    let field = self.resolve_field(node, base_node)?;
                    TypedExpressionKind::Field {
                        base: Box::new(self.lower_receiver(base_node)?),
                        field,
                    }
                }
            }
            SyntaxKind::BracketExpression => {
                if let Some(instance) = self.checked.function_references.get(&node.span).cloned() {
                    let instance = self.concrete_instance(&instance);
                    self.enqueue_reachable(instance.clone(), node.span);
                    TypedExpressionKind::FunctionReference(instance)
                } else {
                    let nodes = child_nodes(node);
                    TypedExpressionKind::Index {
                        base: Box::new(self.lower_expression(*nodes.first()?)?),
                        index: Box::new(self.lower_expression(*nodes.get(1)?)?),
                    }
                }
            }
            SyntaxKind::ParenthesizedExpression => {
                return self.lower_expression(child_nodes(node).into_iter().next()?);
            }
            SyntaxKind::TupleExpression => TypedExpressionKind::Tuple(
                child_nodes(node)
                    .into_iter()
                    .map(|child| self.lower_expression(child))
                    .collect::<Option<Vec<_>>>()?,
            ),
            SyntaxKind::ArrayExpression => TypedExpressionKind::Array(
                child_nodes(node)
                    .into_iter()
                    .map(|child| self.lower_expression(child))
                    .collect::<Option<Vec<_>>>()?,
            ),
            SyntaxKind::RecordExpression => self.lower_record(node)?,
            SyntaxKind::FormattedStringExpression => {
                self.unsupported(node.span, "a formatted string outside `print` or `println`");
                return None;
            }
            SyntaxKind::TryExpression | SyntaxKind::MacroExpression => {
                self.unsupported(
                    node.span,
                    if node.kind == SyntaxKind::TryExpression {
                        "postfix `?` lowering"
                    } else {
                        "collection macro lowering"
                    },
                );
                return None;
            }
            _ => return None,
        };
        Some(TypedExpression {
            ty,
            place,
            copy: self.checked.copies.contains(&node.span),
            span: node.span,
            kind,
        })
    }

    fn lower_call(&mut self, node: &SyntaxNode) -> Option<TypedExpressionKind> {
        let nodes = child_nodes(node);
        let callee_node = *nodes.first()?;
        if let Some((declaration, variant)) = self.resolve_enum_variant_expression(callee_node) {
            let fields = self.resolved.variants[variant.index()]
                .fields
                .iter()
                .copied()
                .zip(nodes[1..].iter().copied())
                .map(|(field, argument)| Some((field, self.lower_expression(argument)?)))
                .collect::<Option<Vec<_>>>()?;
            return Some(TypedExpressionKind::Enum {
                declaration,
                variant,
                fields,
            });
        }
        let checked_call = self.checked.calls.get(&node.span).cloned()?;
        let mut source_arguments = nodes[1..].to_vec();
        let (callee, parameters, receiver) = match checked_call {
            CheckedCall::Direct(instance) => {
                let instance = self.concrete_instance(&instance);
                let signature = self.typed.instantiate_signature(self.resolved, &instance)?;
                self.enqueue_reachable(instance.clone(), node.span);
                let mut parameters = signature.parameters.clone();
                if let Some(receiver) = signature.receiver {
                    parameters.insert(
                        0,
                        crate::types::FunctionParameter {
                            ty: receiver,
                            variadic: false,
                        },
                    );
                }
                (TypedCallee::Function(instance), parameters, None)
            }
            CheckedCall::BoundMethod {
                instance,
                adjustment,
            } => {
                let instance = self.concrete_instance(&instance);
                let signature = self.typed.instantiate_signature(self.resolved, &instance)?;
                self.enqueue_reachable(instance.clone(), node.span);
                let base = child_nodes(callee_node).into_iter().next()?;
                let receiver = self.lower_bound_receiver(base, signature.receiver?, adjustment)?;
                (
                    TypedCallee::Function(instance),
                    signature.parameters,
                    Some(receiver),
                )
            }
            CheckedCall::Indirect => {
                let callee = self.lower_expression(callee_node)?;
                let parameters = self.function_parameters(callee.ty)?;
                (TypedCallee::Indirect(Box::new(callee)), parameters, None)
            }
            CheckedCall::Print { newline } => (TypedCallee::Print { newline }, Vec::new(), None),
        };
        let is_print = matches!(callee, TypedCallee::Print { .. });
        let mut arguments = if is_print {
            source_arguments
                .drain(..)
                .map(|argument| {
                    if is_print && argument.kind == SyntaxKind::FormattedStringExpression {
                        self.lower_print_formatted(argument)
                    } else {
                        self.lower_expression(argument)
                    }
                })
                .collect::<Option<Vec<_>>>()?
        } else {
            self.lower_call_arguments(node.span, &source_arguments, &parameters)?
        };
        if let Some(receiver) = receiver {
            arguments.insert(0, receiver);
        }
        Some(TypedExpressionKind::Call { callee, arguments })
    }

    fn function_parameters(&self, ty: TypeId) -> Option<Vec<crate::types::FunctionParameter>> {
        let ty = self.typed.types.resolve_inference(ty);
        let target = match self.typed.types.kind(ty) {
            TypeKind::Reference { target, .. } => self.typed.types.resolve_inference(*target),
            TypeKind::Alias { target, .. } => return self.function_parameters(*target),
            _ => return None,
        };
        match self.typed.types.kind(target) {
            TypeKind::Function { parameters, .. } => Some(parameters.clone()),
            _ => None,
        }
    }

    fn lower_call_arguments(
        &mut self,
        call_span: Span,
        arguments: &[&SyntaxNode],
        parameters: &[crate::types::FunctionParameter],
    ) -> Option<Vec<TypedExpression>> {
        let variadic = parameters
            .last()
            .is_some_and(|parameter| parameter.variadic);
        if !variadic {
            return arguments
                .iter()
                .map(|argument| self.lower_expression(argument))
                .collect();
        }
        let fixed = parameters.len().saturating_sub(1);
        let mut lowered = arguments[..arguments.len().min(fixed)]
            .iter()
            .map(|argument| self.lower_expression(argument))
            .collect::<Option<Vec<_>>>()?;
        let element = parameters.last()?.ty;
        let ty = self.typed.types.id_for_kind(&TypeKind::Slice(element))?;
        let values = arguments[arguments.len().min(fixed)..]
            .iter()
            .map(|argument| self.lower_expression(argument))
            .collect::<Option<Vec<_>>>()?;
        lowered.push(TypedExpression {
            ty,
            place: PlaceKind::Value,
            copy: false,
            span: arguments
                .get(fixed)
                .map_or(call_span, |argument| argument.span),
            kind: TypedExpressionKind::VariadicSlice(values),
        });
        Some(lowered)
    }

    fn lower_bound_receiver(
        &mut self,
        base: &SyntaxNode,
        receiver_type: TypeId,
        adjustment: ReceiverAdjustment,
    ) -> Option<TypedExpression> {
        match adjustment {
            ReceiverAdjustment::Pass => self.lower_expression(base),
            ReceiverAdjustment::CopyValue => {
                let mut receiver = self.lower_expression(base)?;
                receiver.copy = true;
                Some(receiver)
            }
            ReceiverAdjustment::DereferenceAndCopy => {
                let operand = self.lower_expression(base)?;
                Some(TypedExpression {
                    ty: receiver_type,
                    place: PlaceKind::Value,
                    copy: true,
                    span: base.span,
                    kind: TypedExpressionKind::Dereference(Box::new(operand)),
                })
            }
            ReceiverAdjustment::BorrowShared | ReceiverAdjustment::BorrowMutable => {
                Some(TypedExpression {
                    ty: receiver_type,
                    place: PlaceKind::Value,
                    copy: false,
                    span: base.span,
                    kind: TypedExpressionKind::AddressOf(Box::new(self.lower_place(base)?)),
                })
            }
        }
    }

    fn lower_print_formatted(&mut self, node: &SyntaxNode) -> Option<TypedExpression> {
        let template = self.checked.expression_types.get(&node.span).copied()?;
        let ty = self.concrete_type(template);
        if ty == self.typed.types.error() {
            return None;
        }
        Some(TypedExpression {
            ty,
            place: self
                .checked
                .expression_places
                .get(&node.span)
                .copied()
                .unwrap_or(PlaceKind::Value),
            copy: self.checked.copies.contains(&node.span),
            span: node.span,
            kind: self.lower_formatted(node)?,
        })
    }

    fn lower_record(&mut self, node: &SyntaxNode) -> Option<TypedExpressionKind> {
        let callee = child_nodes(node).into_iter().next()?;
        let enum_variant = self.resolve_enum_variant_expression(callee);
        let declaration = if let Some((declaration, _)) = enum_variant {
            declaration
        } else {
            let template = self.checked.expression_types.get(&node.span).copied()?;
            let ty = self.concrete_type(template);
            match self.expanded_kind(ty) {
                TypeKind::Nominal { identity, .. } => identity.declaration,
                _ => {
                    self.unsupported(node.span, "a non-struct record construction");
                    return None;
                }
            }
        };
        if self.resolved.declarations[declaration.index()].kind != DeclarationKind::Struct
            && enum_variant.is_none()
        {
            self.unsupported(node.span, "a non-record construction");
            return None;
        }
        let required = enum_variant.map_or_else(
            || {
                self.resolved
                    .fields
                    .iter()
                    .filter(|field| {
                        field.parent_declaration == declaration && field.parent_variant.is_none()
                    })
                    .map(|field| field.id)
                    .collect::<Vec<_>>()
            },
            |(_, variant)| self.resolved.variants[variant.index()].fields.clone(),
        );
        let mut fields = Vec::new();
        for field_node in child_nodes(node)
            .into_iter()
            .filter(|child| child.kind == SyntaxKind::RecordField)
        {
            let name_token = first_identifier(field_node)?;
            let field = required.iter().copied().find(|field| {
                self.resolved
                    .symbol_text(self.resolved.fields[field.index()].name)
                    == identifier_text(name_token)
            })?;
            let value = if let Some(value) = child_nodes(field_node).into_iter().next() {
                self.lower_expression(value)?
            } else {
                let target = self.resolved.reference_at(name_token.span)?.target;
                let NameTarget::Local(binding) = target else {
                    return None;
                };
                let template = self.typed.field_types.get(&field).copied()?;
                let ty = self.concrete_type(template);
                TypedExpression {
                    ty,
                    place: PlaceKind::Addressable,
                    copy: true,
                    span: name_token.span,
                    kind: TypedExpressionKind::Local(binding),
                }
            };
            fields.push((field, value));
        }
        if let Some((enum_declaration, variant)) = enum_variant {
            Some(TypedExpressionKind::Enum {
                declaration: enum_declaration,
                variant,
                fields,
            })
        } else {
            Some(TypedExpressionKind::Struct {
                declaration,
                fields,
            })
        }
    }

    fn lower_formatted(&mut self, node: &SyntaxNode) -> Option<TypedExpressionKind> {
        let token = first_token(node)?;
        let TokenKind::FormattedString(segments) = &token.kind else {
            return None;
        };
        let mut expressions = child_nodes(node).into_iter();
        let mut parts = Vec::new();
        for segment in segments {
            match &segment.kind {
                FormattedSegmentKind::Text(text) => {
                    if !text.is_empty() {
                        parts.push(FormattedPart::Text(text.clone()));
                    }
                }
                FormattedSegmentKind::Expression { .. } => {
                    parts.push(FormattedPart::Expression(
                        self.lower_expression(expressions.next()?)?,
                    ));
                }
            }
        }
        Some(TypedExpressionKind::FormattedString(parts))
    }

    fn lower_place(&mut self, node: &SyntaxNode) -> Option<TypedPlace> {
        let template = self.checked.expression_types.get(&node.span).copied()?;
        let ty = self.concrete_type(template);
        match node.kind {
            SyntaxKind::NameExpression => {
                let token = first_token(node)?;
                let NameTarget::Local(binding) = self.resolved.reference_at(token.span)?.target
                else {
                    return None;
                };
                Some(TypedPlace::Local {
                    binding,
                    ty,
                    span: node.span,
                })
            }
            SyntaxKind::MemberExpression => {
                let base_node = child_nodes(node).into_iter().next()?;
                let field = self.resolve_field(node, base_node)?;
                Some(TypedPlace::Field {
                    base: Box::new(self.lower_place_receiver(base_node)?),
                    field,
                    ty,
                    span: node.span,
                })
            }
            SyntaxKind::BracketExpression => {
                let nodes = child_nodes(node);
                let base_node = *nodes.first()?;
                let index = self.lower_expression(*nodes.get(1)?)?;
                let base_type = self
                    .checked
                    .expression_types
                    .get(&base_node.span)
                    .copied()?;
                let length = match self.expanded_kind(base_type) {
                    TypeKind::Array { length, .. } => *length,
                    _ => {
                        self.unsupported(node.span, "non-array indexed assignment");
                        return None;
                    }
                };
                Some(TypedPlace::Index {
                    base: Box::new(self.lower_place(base_node)?),
                    index,
                    ty,
                    length,
                    span: node.span,
                })
            }
            SyntaxKind::UnaryExpression => {
                let token = first_token(node)?;
                if !matches!(token.kind, TokenKind::Star) {
                    return None;
                }
                let operand = child_nodes(node).into_iter().next_back()?;
                Some(TypedPlace::Dereference {
                    base: Box::new(self.lower_expression(operand)?),
                    ty,
                    span: node.span,
                })
            }
            _ => None,
        }
    }

    fn resolve_field(&self, node: &SyntaxNode, base: &SyntaxNode) -> Option<FieldId> {
        let base_type = self.checked.expression_types.get(&base.span).copied()?;
        let declaration = self.field_owner(base_type)?;
        let name = identifier_text(first_identifier(node)?);
        self.find_field(declaration, name)
    }

    /// Lowers a field-access base, inserting the automatic dereference when it
    /// is a safe reference (SPEC 3.2).
    fn lower_receiver(&mut self, base: &SyntaxNode) -> Option<TypedExpression> {
        let value = self.lower_expression(base)?;
        if !self.is_reference_type(value.ty) {
            return Some(value);
        }
        let target = self.reference_target(value.ty)?;
        Some(TypedExpression {
            ty: target,
            place: PlaceKind::Mutable,
            copy: false,
            span: base.span,
            kind: TypedExpressionKind::Dereference(Box::new(value)),
        })
    }

    /// The place form of [`Self::lower_receiver`].
    fn lower_place_receiver(&mut self, base: &SyntaxNode) -> Option<TypedPlace> {
        let template = self.checked.expression_types.get(&base.span).copied()?;
        let base_type = self.concrete_type(template);
        if !self.is_reference_type(base_type) {
            return self.lower_place(base);
        }
        let target = self.reference_target(base_type)?;
        Some(TypedPlace::Dereference {
            base: Box::new(self.lower_expression(base)?),
            ty: target,
            span: base.span,
        })
    }

    fn is_reference_type(&self, ty: TypeId) -> bool {
        matches!(self.expanded_kind(ty), TypeKind::Reference { .. })
    }

    fn reference_target(&self, ty: TypeId) -> Option<TypeId> {
        match self.expanded_kind(ty) {
            TypeKind::Reference { target, .. } => Some(*target),
            _ => None,
        }
    }

    /// The struct a field selection resolves against, seeing through a safe
    /// reference: reference field access automatically dereferences (SPEC 3.2).
    fn field_owner(&self, base_type: TypeId) -> Option<DeclarationId> {
        match self.expanded_kind(base_type) {
            TypeKind::Nominal { identity, .. } => Some(identity.declaration),
            TypeKind::Reference { target, .. } => match self.expanded_kind(*target) {
                TypeKind::Nominal { identity, .. } => Some(identity.declaration),
                _ => None,
            },
            _ => None,
        }
    }

    fn find_field(&self, declaration: DeclarationId, name: &str) -> Option<FieldId> {
        self.find_member(declaration, name)
            .and_then(|member| match member {
                MemberId::Field(field) => Some(field),
                _ => None,
            })
    }

    fn find_member(&self, declaration: DeclarationId, name: &str) -> Option<MemberId> {
        self.resolved
            .declaration_members
            .get(&declaration)?
            .iter()
            .find_map(|(symbol, member)| {
                (self.resolved.symbol_text(*symbol) == name).then_some(*member)
            })
    }

    fn resolve_enum_variant_expression(
        &self,
        node: &SyntaxNode,
    ) -> Option<(DeclarationId, VariantId)> {
        if node.kind != SyntaxKind::MemberExpression {
            return None;
        }
        let base = child_nodes(node).into_iter().next()?;
        let base_span = callee_target_span(base)?;
        let NameTarget::Item(ItemId::Declaration(declaration)) =
            self.resolved.reference_at(base_span)?.target
        else {
            return None;
        };
        if self.resolved.declarations[declaration.index()].kind != DeclarationKind::Enum {
            return None;
        }
        let member = node.children.iter().rev().find_map(|child| match child {
            SyntaxElement::Token(token) if matches!(token.kind, TokenKind::Identifier(_)) => {
                Some(token)
            }
            _ => None,
        })?;
        match self.find_member(declaration, identifier_text(member)) {
            Some(MemberId::Variant(variant)) => Some((declaration, variant)),
            _ => None,
        }
    }

    fn expanded_kind(&self, mut ty: TypeId) -> &TypeKind {
        loop {
            match self.typed.types.kind(ty) {
                TypeKind::Alias { target, .. } => ty = *target,
                kind => return kind,
            }
        }
    }

    fn unsupported(&mut self, span: Span, feature: &str) {
        self.diagnostics.push(
            Diagnostic::new(
                Category::Lowering,
                format!("{feature} is not supported by the Milestone 8 executable subset"),
            )
            .with_primary(span),
        );
    }
}

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
        /// Fixed arrays carry a constant bound; slices read `.length` from
        /// their evaluated base.
        length: Option<u128>,
        trap: TrapKind,
    },
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
    Store {
        place: ControlFlowPlace,
        value: TemporaryId,
        span: Span,
    },
    PrintText {
        text: String,
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
    /// Whether lowering produced any managed allocation, promoted storage, or
    /// managed root. The backend engages `ManagedMemoryStrategy` — and the
    /// driver links its native libraries — only when this is set, so programs
    /// that never need the collector keep a dependency-free translation unit.
    /// Milestone 10 promotion analysis is what turns this on.
    pub requires_managed_memory: bool,
}

#[must_use]
pub fn lower_control_flow(program: &TypedIrProgram, types: &TypedProgram) -> ControlFlowProgram {
    ControlFlowProgram {
        functions: program
            .functions
            .iter()
            .map(|function| FunctionLowerer::new(types, function).run())
            .collect(),
        structs: program.structs.clone(),
        enums: program.enums.clone(),
        // Managed storage is needed exactly when some local was promoted.
        requires_managed_memory: program
            .functions
            .iter()
            .any(|function| !function.promoted_locals.is_empty() || function.allocates_managed),
    }
}

struct OpenBlock {
    instructions: Vec<Instruction>,
    terminator: Option<Terminator>,
}

struct FunctionLowerer<'a> {
    types: &'a TypedProgram,
    function: &'a TypedFunction,
    blocks: Vec<OpenBlock>,
    current: BlockId,
    temporary_types: Vec<TypeId>,
    loops: Vec<(BlockId, BlockId)>,
}

impl<'a> FunctionLowerer<'a> {
    fn new(types: &'a TypedProgram, function: &'a TypedFunction) -> Self {
        Self {
            types,
            function,
            blocks: vec![OpenBlock {
                instructions: Vec::new(),
                terminator: None,
            }],
            current: BlockId(0),
            temporary_types: Vec::new(),
            loops: Vec::new(),
        }
    }

    fn run(mut self) -> ControlFlowFunction {
        self.lower_statements(&self.function.body);
        if self.is_open(self.current) {
            self.terminate(Terminator::Return(None));
        }
        let blocks = self
            .blocks
            .into_iter()
            .enumerate()
            .map(|(index, block)| BasicBlock {
                id: BlockId(u32::try_from(index).expect("too many basic blocks")),
                instructions: block.instructions,
                terminator: block.terminator.unwrap_or(Terminator::Unreachable),
            })
            .collect();
        ControlFlowFunction {
            declaration: self.function.declaration,
            instance: self.function.instance.clone(),
            name: self.function.name.clone(),
            span: self.function.span,
            parameters: self.function.parameters.clone(),
            return_type: self.function.return_type,
            local_types: self.function.local_types.clone(),
            promoted_locals: self.function.promoted_locals.clone(),
            allocates_managed: self.function.allocates_managed,
            temporary_types: self.temporary_types,
            entry: BlockId(0),
            blocks,
        }
    }

    fn lower_statements(&mut self, statements: &[TypedStatement]) {
        for statement in statements {
            if !self.is_open(self.current) {
                self.current = self.new_block();
            }
            self.lower_statement(statement);
        }
    }

    fn lower_statement(&mut self, statement: &TypedStatement) {
        match &statement.kind {
            TypedStatementKind::Let { binding, value, .. } => {
                let value = self.lower_expression(value);
                self.emit(Instruction::Store {
                    place: ControlFlowPlace::Local(*binding),
                    value,
                    span: statement.span,
                });
            }
            TypedStatementKind::Assign {
                place,
                operator,
                value,
            } => {
                let place_type = place.ty();
                let source_span = place.span();
                let place = self.lower_place(place);
                if *operator == AssignmentOperator::Assign {
                    let value = self.lower_expression(value);
                    self.emit(Instruction::Store {
                        place,
                        value,
                        span: statement.span,
                    });
                } else {
                    let old = self.temp(place_type);
                    self.emit(Instruction::Assign {
                        destination: old,
                        value: Rvalue::Load(place.clone()),
                        span: source_span,
                    });
                    let right = self.lower_expression(value);
                    let operator = assignment_binary(*operator);
                    let result = self.temp(value.ty);
                    self.emit(Instruction::Assign {
                        destination: result,
                        value: Rvalue::Binary {
                            operator,
                            left: old,
                            right,
                            trap: binary_trap(operator, value.ty, self.types),
                        },
                        span: statement.span,
                    });
                    self.emit(Instruction::Store {
                        place,
                        value: result,
                        span: statement.span,
                    });
                }
            }
            TypedStatementKind::Expression(expression) => {
                self.lower_expression(expression);
            }
            TypedStatementKind::Return(value) => {
                let value = value
                    .as_ref()
                    .map(|expression| self.lower_expression(expression));
                self.terminate(Terminator::Return(value));
            }
            TypedStatementKind::If {
                condition,
                then_body,
                else_body,
            } => self.lower_if(condition, then_body, else_body),
            TypedStatementKind::While { condition, body } => self.lower_while(condition, body),
            TypedStatementKind::Match { scrutinee, arms } => self.lower_match(scrutinee, arms),
            TypedStatementKind::Block(body) => self.lower_statements(body),
            TypedStatementKind::Break => {
                if let Some((break_block, _)) = self.loops.last().copied() {
                    self.terminate(Terminator::Goto(break_block));
                }
            }
            TypedStatementKind::Continue => {
                if let Some((_, continue_block)) = self.loops.last().copied() {
                    self.terminate(Terminator::Goto(continue_block));
                }
            }
            TypedStatementKind::Pass => {}
        }
    }

    fn lower_if(
        &mut self,
        condition: &TypedExpression,
        then_body: &[TypedStatement],
        else_body: &[TypedStatement],
    ) {
        let condition = self.lower_expression(condition);
        let then_block = self.new_block();
        let else_block = self.new_block();
        let join_block = self.new_block();
        self.terminate(Terminator::Branch {
            condition,
            then_block,
            else_block,
        });
        self.current = then_block;
        self.lower_statements(then_body);
        if self.is_open(self.current) {
            self.terminate(Terminator::Goto(join_block));
        }
        self.current = else_block;
        self.lower_statements(else_body);
        if self.is_open(self.current) {
            self.terminate(Terminator::Goto(join_block));
        }
        self.current = join_block;
    }

    fn lower_while(&mut self, condition: &TypedExpression, body: &[TypedStatement]) {
        let condition_block = self.new_block();
        let body_block = self.new_block();
        let exit_block = self.new_block();
        self.terminate(Terminator::Goto(condition_block));
        self.current = condition_block;
        let condition = self.lower_expression(condition);
        self.terminate(Terminator::Branch {
            condition,
            then_block: body_block,
            else_block: exit_block,
        });
        self.current = body_block;
        self.loops.push((exit_block, condition_block));
        self.lower_statements(body);
        self.loops.pop();
        if self.is_open(self.current) {
            self.terminate(Terminator::Goto(condition_block));
        }
        self.current = exit_block;
    }

    fn lower_match(&mut self, scrutinee: &TypedExpression, arms: &[TypedMatchArm]) {
        let value = self.lower_expression(scrutinee);
        let exit_block = self.new_block();
        for arm in arms {
            let matched_block = self.new_block();
            let next_arm = self.new_block();
            self.lower_pattern(&arm.pattern, value, matched_block, next_arm);
            self.current = matched_block;
            let body_block = if let Some(guard) = &arm.guard {
                let guard_value = self.lower_expression(guard);
                let body_block = self.new_block();
                self.terminate(Terminator::Branch {
                    condition: guard_value,
                    then_block: body_block,
                    else_block: next_arm,
                });
                body_block
            } else {
                matched_block
            };
            self.current = body_block;
            self.lower_statements(&arm.body);
            if self.is_open(self.current) {
                self.terminate(Terminator::Goto(exit_block));
            }
            self.current = next_arm;
        }
        if self.is_open(self.current) {
            self.terminate(Terminator::Unreachable);
        }
        self.current = exit_block;
    }

    fn lower_pattern(
        &mut self,
        pattern: &TypedPattern,
        value: TemporaryId,
        success: BlockId,
        failure: BlockId,
    ) {
        match &pattern.kind {
            TypedPatternKind::Wildcard => self.terminate(Terminator::Goto(success)),
            TypedPatternKind::Binding(binding) => {
                let copied = self.temp(pattern.ty);
                self.emit(Instruction::Assign {
                    destination: copied,
                    value: Rvalue::Copy(value),
                    span: pattern.span,
                });
                self.emit(Instruction::Store {
                    place: ControlFlowPlace::Local(*binding),
                    value: copied,
                    span: pattern.span,
                });
                self.terminate(Terminator::Goto(success));
            }
            TypedPatternKind::Literal(constant) => {
                let literal = self.temp(pattern.ty);
                self.emit(Instruction::Assign {
                    destination: literal,
                    value: Rvalue::Constant(constant.clone()),
                    span: pattern.span,
                });
                let condition =
                    self.emit_pattern_equality(value, literal, pattern.ty, pattern.span);
                self.terminate(Terminator::Branch {
                    condition,
                    then_block: success,
                    else_block: failure,
                });
            }
            TypedPatternKind::Alternative(alternatives) => {
                if alternatives.is_empty() {
                    self.terminate(Terminator::Goto(failure));
                    return;
                }
                for (index, alternative) in alternatives.iter().enumerate() {
                    let alternative_failure = if index + 1 == alternatives.len() {
                        failure
                    } else {
                        self.new_block()
                    };
                    self.lower_pattern(alternative, value, success, alternative_failure);
                    if alternative_failure != failure {
                        self.current = alternative_failure;
                    }
                }
            }
            TypedPatternKind::Dereference(inner) => {
                let loaded = self.temp(inner.ty);
                self.emit(Instruction::Assign {
                    destination: loaded,
                    value: Rvalue::Load(ControlFlowPlace::Dereference {
                        base: Box::new(ControlFlowPlace::Temporary(value)),
                    }),
                    span: pattern.span,
                });
                self.lower_pattern(inner, loaded, success, failure);
            }
            TypedPatternKind::Tuple(elements) => {
                let values = elements
                    .iter()
                    .enumerate()
                    .map(|(index, element)| {
                        let loaded = self.temp(element.ty);
                        self.emit(Instruction::Assign {
                            destination: loaded,
                            value: Rvalue::Load(ControlFlowPlace::TupleField {
                                base: Box::new(ControlFlowPlace::Temporary(value)),
                                index,
                            }),
                            span: element.span,
                        });
                        (element, loaded)
                    })
                    .collect::<Vec<_>>();
                self.lower_pattern_sequence(&values, success, failure);
            }
            TypedPatternKind::Struct { fields, .. } => {
                let values = fields
                    .iter()
                    .map(|(field, pattern)| {
                        let loaded = self.temp(pattern.ty);
                        self.emit(Instruction::Assign {
                            destination: loaded,
                            value: Rvalue::Load(ControlFlowPlace::Field {
                                base: Box::new(ControlFlowPlace::Temporary(value)),
                                field: *field,
                            }),
                            span: pattern.span,
                        });
                        (pattern, loaded)
                    })
                    .collect::<Vec<_>>();
                self.lower_pattern_sequence(&values, success, failure);
            }
            TypedPatternKind::Variant { variant, fields } => {
                let discriminant = self.temp(self.types.types.primitive_id(PrimitiveType::U32));
                self.emit(Instruction::Assign {
                    destination: discriminant,
                    value: Rvalue::Discriminant(value),
                    span: pattern.span,
                });
                let expected = self.temp(self.types.types.primitive_id(PrimitiveType::U32));
                self.emit(Instruction::Assign {
                    destination: expected,
                    value: Rvalue::Constant(Constant::Integer {
                        magnitude: variant.index() as u128,
                        negative: false,
                    }),
                    span: pattern.span,
                });
                let payload_block = self.new_block();
                let condition = self.emit_pattern_equality(
                    discriminant,
                    expected,
                    self.types.types.primitive_id(PrimitiveType::U32),
                    pattern.span,
                );
                self.terminate(Terminator::Branch {
                    condition,
                    then_block: payload_block,
                    else_block: failure,
                });
                self.current = payload_block;
                let values = fields
                    .iter()
                    .map(|(field, pattern)| {
                        let loaded = self.temp(pattern.ty);
                        self.emit(Instruction::Assign {
                            destination: loaded,
                            value: Rvalue::Load(ControlFlowPlace::VariantField {
                                base: Box::new(ControlFlowPlace::Temporary(value)),
                                variant: *variant,
                                field: *field,
                            }),
                            span: pattern.span,
                        });
                        (pattern, loaded)
                    })
                    .collect::<Vec<_>>();
                self.lower_pattern_sequence(&values, success, failure);
            }
        }
    }

    fn lower_pattern_sequence(
        &mut self,
        values: &[(&TypedPattern, TemporaryId)],
        success: BlockId,
        failure: BlockId,
    ) {
        if values.is_empty() {
            self.terminate(Terminator::Goto(success));
            return;
        }
        for (index, (pattern, value)) in values.iter().enumerate() {
            let next = if index + 1 == values.len() {
                success
            } else {
                self.new_block()
            };
            self.lower_pattern(pattern, *value, next, failure);
            if next != success {
                self.current = next;
            }
        }
    }

    fn emit_pattern_equality(
        &mut self,
        left: TemporaryId,
        right: TemporaryId,
        operand_type: TypeId,
        span: Span,
    ) -> TemporaryId {
        let condition = self.temp(self.types.types.primitive_id(PrimitiveType::Bool));
        self.emit(Instruction::Assign {
            destination: condition,
            value: Rvalue::CompareEqual {
                left,
                right,
                operand_type,
            },
            span,
        });
        condition
    }

    fn lower_expression(&mut self, expression: &TypedExpression) -> TemporaryId {
        let value = match &expression.kind {
            TypedExpressionKind::Constant(constant) => Rvalue::Constant(constant.clone()),
            TypedExpressionKind::FunctionReference(instance) => {
                Rvalue::FunctionReference(instance.clone())
            }
            TypedExpressionKind::Local(binding) => Rvalue::Load(ControlFlowPlace::Local(*binding)),
            TypedExpressionKind::AddressOf(place) => {
                let place = self.lower_place(place);
                Rvalue::AddressOf(place)
            }
            TypedExpressionKind::AddressOfTemporary(value) => {
                let value_type = value.ty;
                let value = self.lower_expression(value);
                Rvalue::AllocateManaged { value, value_type }
            }
            TypedExpressionKind::Dereference(operand) => {
                let operand = self.lower_expression(operand);
                Rvalue::Load(ControlFlowPlace::Dereference {
                    base: Box::new(ControlFlowPlace::Temporary(operand)),
                })
            }
            TypedExpressionKind::Unary { operator, operand } => {
                let operand = self.lower_expression(operand);
                Rvalue::Unary {
                    operator: *operator,
                    operand,
                    trap: unary_trap(*operator, expression.ty, self.types),
                }
            }
            TypedExpressionKind::Binary {
                operator,
                left,
                right,
            } if matches!(
                operator,
                BinaryOperator::LogicalAnd | BinaryOperator::LogicalOr
            ) =>
            {
                return self.lower_short_circuit(*operator, left, right, expression);
            }
            TypedExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                let left = self.lower_expression(left);
                let right = self.lower_expression(right);
                Rvalue::Binary {
                    operator: *operator,
                    left,
                    right,
                    trap: binary_trap(*operator, expression.ty, self.types),
                }
            }
            TypedExpressionKind::Cast { value } => {
                let source_type = value.ty;
                let value = self.lower_expression(value);
                Rvalue::Cast {
                    value,
                    source_type,
                    trap: cast_trap(source_type, expression.ty, self.types),
                }
            }
            TypedExpressionKind::Call {
                callee: TypedCallee::Function(instance),
                arguments,
            } => {
                let arguments = arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect();
                Rvalue::Call {
                    instance: instance.clone(),
                    arguments,
                }
            }
            TypedExpressionKind::Call {
                callee: TypedCallee::Indirect(callee),
                arguments,
            } => {
                let callee = self.lower_expression(callee);
                let arguments = arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect();
                Rvalue::IndirectCall { callee, arguments }
            }
            TypedExpressionKind::Call {
                callee: TypedCallee::Print { newline },
                arguments,
            } => {
                for argument in arguments {
                    self.lower_print(argument);
                }
                if *newline {
                    self.emit(Instruction::PrintNewline {
                        span: expression.span,
                    });
                }
                Rvalue::Constant(Constant::Unit)
            }
            TypedExpressionKind::Field { base, field } => {
                let place = self.expression_place(base);
                Rvalue::Load(ControlFlowPlace::Field {
                    base: Box::new(place),
                    field: *field,
                })
            }
            TypedExpressionKind::Index { base, index } => {
                let base_place = self.expression_place(base);
                let index = self.lower_expression(index);
                let length = match expanded_kind(&self.types.types, base.ty) {
                    TypeKind::Array { length, .. } => Some(*length),
                    TypeKind::Slice(_) => None,
                    _ => Some(0),
                };
                Rvalue::Load(ControlFlowPlace::Index {
                    base: Box::new(base_place),
                    index,
                    length,
                    trap: TrapKind::IndexOutOfBounds,
                })
            }
            TypedExpressionKind::Tuple(elements) => Rvalue::Aggregate(AggregateValue::Tuple(
                elements
                    .iter()
                    .map(|element| self.lower_expression(element))
                    .collect(),
            )),
            TypedExpressionKind::Array(elements) => Rvalue::Aggregate(AggregateValue::Array(
                elements
                    .iter()
                    .map(|element| self.lower_expression(element))
                    .collect(),
            )),
            TypedExpressionKind::Struct {
                declaration,
                fields,
            } => Rvalue::Aggregate(AggregateValue::Struct {
                declaration: *declaration,
                fields: fields
                    .iter()
                    .map(|(field, value)| (*field, self.lower_expression(value)))
                    .collect(),
            }),
            TypedExpressionKind::Enum {
                declaration,
                variant,
                fields,
            } => Rvalue::Aggregate(AggregateValue::Enum {
                declaration: *declaration,
                variant: *variant,
                fields: fields
                    .iter()
                    .map(|(field, value)| (*field, self.lower_expression(value)))
                    .collect(),
            }),
            TypedExpressionKind::VariadicSlice(elements) => {
                let element_type = match expanded_kind(&self.types.types, expression.ty) {
                    TypeKind::Slice(element) => *element,
                    _ => self.types.types.error(),
                };
                Rvalue::VariadicSlice {
                    elements: elements
                        .iter()
                        .map(|element| self.lower_expression(element))
                        .collect(),
                    element_type,
                }
            }
            TypedExpressionKind::FormattedString(_) => {
                self.lower_print(expression);
                Rvalue::Constant(Constant::String(String::new()))
            }
        };
        let raw = self.temp(expression.ty);
        self.emit(Instruction::Assign {
            destination: raw,
            value,
            span: expression.span,
        });
        if expression.copy {
            let copied = self.temp(expression.ty);
            self.emit(Instruction::Assign {
                destination: copied,
                value: Rvalue::Copy(raw),
                span: expression.span,
            });
            copied
        } else {
            raw
        }
    }

    fn lower_short_circuit(
        &mut self,
        operator: BinaryOperator,
        left: &TypedExpression,
        right: &TypedExpression,
        expression: &TypedExpression,
    ) -> TemporaryId {
        let result = self.temp(expression.ty);
        let left = self.lower_expression(left);
        let rhs_block = self.new_block();
        let short_block = self.new_block();
        let join_block = self.new_block();
        let (then_block, else_block, short_value) = if operator == BinaryOperator::LogicalAnd {
            (rhs_block, short_block, false)
        } else {
            (short_block, rhs_block, true)
        };
        self.terminate(Terminator::Branch {
            condition: left,
            then_block,
            else_block,
        });
        self.current = short_block;
        self.emit(Instruction::Assign {
            destination: result,
            value: Rvalue::Constant(Constant::Bool(short_value)),
            span: expression.span,
        });
        self.terminate(Terminator::Goto(join_block));
        self.current = rhs_block;
        let right = self.lower_expression(right);
        self.emit(Instruction::Assign {
            destination: result,
            value: Rvalue::Copy(right),
            span: expression.span,
        });
        self.terminate(Terminator::Goto(join_block));
        self.current = join_block;
        result
    }

    fn lower_print(&mut self, expression: &TypedExpression) {
        if let TypedExpressionKind::FormattedString(parts) = &expression.kind {
            for part in parts {
                match part {
                    FormattedPart::Text(text) => self.emit(Instruction::PrintText {
                        text: text.clone(),
                        span: expression.span,
                    }),
                    FormattedPart::Expression(value) => {
                        let temporary = self.lower_expression(value);
                        self.emit(Instruction::PrintValue {
                            value: temporary,
                            ty: value.ty,
                            span: value.span,
                        });
                    }
                }
            }
        } else {
            let temporary = self.lower_expression(expression);
            self.emit(Instruction::PrintValue {
                value: temporary,
                ty: expression.ty,
                span: expression.span,
            });
        }
    }

    fn lower_place(&mut self, place: &TypedPlace) -> ControlFlowPlace {
        match place {
            TypedPlace::Local { binding, .. } => ControlFlowPlace::Local(*binding),
            TypedPlace::Field { base, field, .. } => ControlFlowPlace::Field {
                base: Box::new(self.lower_place(base)),
                field: *field,
            },
            TypedPlace::Index {
                base,
                index,
                length,
                ..
            } => {
                let base = self.lower_place(base);
                let index = self.lower_expression(index);
                ControlFlowPlace::Index {
                    base: Box::new(base),
                    index,
                    length: Some(*length),
                    trap: TrapKind::IndexOutOfBounds,
                }
            }
            TypedPlace::Dereference { base, .. } => {
                let base = self.lower_expression(base);
                ControlFlowPlace::Dereference {
                    base: Box::new(ControlFlowPlace::Temporary(base)),
                }
            }
        }
    }

    fn expression_place(&mut self, expression: &TypedExpression) -> ControlFlowPlace {
        match &expression.kind {
            TypedExpressionKind::Local(binding) => ControlFlowPlace::Local(*binding),
            TypedExpressionKind::Field { base, field } => ControlFlowPlace::Field {
                base: Box::new(self.expression_place(base)),
                field: *field,
            },
            TypedExpressionKind::Index { base, index } => {
                let base = self.expression_place(base);
                let index = self.lower_expression(index);
                let length = match expanded_kind(
                    &self.types.types,
                    base_expression_type(&expression.kind),
                ) {
                    TypeKind::Array { length, .. } => Some(*length),
                    TypeKind::Slice(_) => None,
                    _ => Some(0),
                };
                ControlFlowPlace::Index {
                    base: Box::new(base),
                    index,
                    length,
                    trap: TrapKind::IndexOutOfBounds,
                }
            }
            TypedExpressionKind::Dereference(operand) => {
                let operand = self.lower_expression(operand);
                ControlFlowPlace::Dereference {
                    base: Box::new(ControlFlowPlace::Temporary(operand)),
                }
            }
            _ => ControlFlowPlace::Temporary(self.lower_expression(expression)),
        }
    }

    fn temp(&mut self, ty: TypeId) -> TemporaryId {
        let id = TemporaryId(
            u32::try_from(self.temporary_types.len()).expect("too many control-flow temporaries"),
        );
        self.temporary_types.push(ty);
        id
    }

    fn new_block(&mut self) -> BlockId {
        let id = BlockId(u32::try_from(self.blocks.len()).expect("too many basic blocks"));
        self.blocks.push(OpenBlock {
            instructions: Vec::new(),
            terminator: None,
        });
        id
    }

    fn emit(&mut self, instruction: Instruction) {
        self.blocks[self.current.index()]
            .instructions
            .push(instruction);
    }

    fn terminate(&mut self, terminator: Terminator) {
        self.blocks[self.current.index()].terminator = Some(terminator);
    }

    fn is_open(&self, block: BlockId) -> bool {
        self.blocks[block.index()].terminator.is_none()
    }
}

fn unary_trap(operator: UnaryOperator, ty: TypeId, types: &TypedProgram) -> Option<TrapKind> {
    (operator == UnaryOperator::Negative
        && types
            .types
            .expanded_primitive(ty)
            .is_some_and(PrimitiveType::is_integer))
    .then_some(TrapKind::IntegerOverflow)
}

fn binary_trap(operator: BinaryOperator, ty: TypeId, types: &TypedProgram) -> Option<TrapKind> {
    let integer = types
        .types
        .expanded_primitive(ty)
        .is_some_and(PrimitiveType::is_integer);
    if !integer {
        return None;
    }
    match operator {
        BinaryOperator::Add | BinaryOperator::Subtract | BinaryOperator::Multiply => {
            Some(TrapKind::IntegerOverflow)
        }
        BinaryOperator::Divide | BinaryOperator::Remainder => Some(TrapKind::DivisionByZero),
        BinaryOperator::ShiftLeft | BinaryOperator::ShiftRight => Some(TrapKind::InvalidShift),
        _ => None,
    }
}

fn cast_trap(source: TypeId, target: TypeId, types: &TypedProgram) -> Option<TrapKind> {
    let source = types.types.expanded_primitive(source)?;
    let target = types.types.expanded_primitive(target)?;
    (source != target).then_some(TrapKind::InvalidNumericConversion)
}

fn expanded_kind(types: &crate::types::TypeContext, mut ty: TypeId) -> &TypeKind {
    loop {
        match types.kind(ty) {
            TypeKind::Alias { target, .. } => ty = *target,
            kind => return kind,
        }
    }
}

fn base_expression_type(kind: &TypedExpressionKind) -> TypeId {
    match kind {
        TypedExpressionKind::Index { base, .. } => base.ty,
        _ => unreachable!("caller selected an index expression"),
    }
}

fn assignment_binary(operator: AssignmentOperator) -> BinaryOperator {
    match operator {
        AssignmentOperator::Add => BinaryOperator::Add,
        AssignmentOperator::Subtract => BinaryOperator::Subtract,
        AssignmentOperator::Multiply => BinaryOperator::Multiply,
        AssignmentOperator::Divide => BinaryOperator::Divide,
        AssignmentOperator::Remainder => BinaryOperator::Remainder,
        AssignmentOperator::BitAnd => BinaryOperator::BitAnd,
        AssignmentOperator::BitOr => BinaryOperator::BitOr,
        AssignmentOperator::BitXor => BinaryOperator::BitXor,
        AssignmentOperator::ShiftLeft => BinaryOperator::ShiftLeft,
        AssignmentOperator::ShiftRight => BinaryOperator::ShiftRight,
        AssignmentOperator::Assign => unreachable!("plain assignment has no binary operator"),
    }
}

fn lower_constant(node: &SyntaxNode, negative: bool) -> Option<Constant> {
    let token = first_token(node)?;
    Some(match &token.kind {
        TokenKind::IntegerLiteral { raw, radix, suffix } => Constant::Integer {
            magnitude: parse_integer_magnitude(raw, *radix, *suffix).ok()?,
            negative,
        },
        TokenKind::FloatLiteral { raw, .. } => Constant::Float(if negative {
            format!("-{raw}")
        } else {
            raw.clone()
        }),
        TokenKind::StringLiteral(value) => Constant::String(value.clone()),
        TokenKind::CharacterLiteral(value) => Constant::Character(value.chars().next()?),
        TokenKind::Keyword(Keyword::True) => Constant::Bool(true),
        TokenKind::Keyword(Keyword::False) => Constant::Bool(false),
        TokenKind::Keyword(Keyword::Null) => Constant::Null,
        _ => return None,
    })
}

fn unary_operator(kind: &TokenKind) -> Option<UnaryOperator> {
    Some(match kind {
        TokenKind::Plus => UnaryOperator::Positive,
        TokenKind::Minus => UnaryOperator::Negative,
        TokenKind::Bang => UnaryOperator::LogicalNot,
        TokenKind::Tilde => UnaryOperator::BitwiseNot,
        _ => return None,
    })
}

fn binary_operator(kind: &TokenKind) -> Option<BinaryOperator> {
    Some(match kind {
        TokenKind::Plus => BinaryOperator::Add,
        TokenKind::Minus => BinaryOperator::Subtract,
        TokenKind::Star => BinaryOperator::Multiply,
        TokenKind::Slash => BinaryOperator::Divide,
        TokenKind::Percent => BinaryOperator::Remainder,
        TokenKind::Amp => BinaryOperator::BitAnd,
        TokenKind::Pipe => BinaryOperator::BitOr,
        TokenKind::Caret => BinaryOperator::BitXor,
        TokenKind::Shl => BinaryOperator::ShiftLeft,
        TokenKind::Shr => BinaryOperator::ShiftRight,
        TokenKind::EqEq => BinaryOperator::Equal,
        TokenKind::NotEq => BinaryOperator::NotEqual,
        TokenKind::Less => BinaryOperator::Less,
        TokenKind::LessEq => BinaryOperator::LessEqual,
        TokenKind::Greater => BinaryOperator::Greater,
        TokenKind::GreaterEq => BinaryOperator::GreaterEqual,
        TokenKind::AndAnd => BinaryOperator::LogicalAnd,
        TokenKind::OrOr => BinaryOperator::LogicalOr,
        _ => return None,
    })
}

fn assignment_operator(kind: &TokenKind) -> Option<AssignmentOperator> {
    Some(match kind {
        TokenKind::Assign => AssignmentOperator::Assign,
        TokenKind::PlusAssign => AssignmentOperator::Add,
        TokenKind::MinusAssign => AssignmentOperator::Subtract,
        TokenKind::StarAssign => AssignmentOperator::Multiply,
        TokenKind::SlashAssign => AssignmentOperator::Divide,
        TokenKind::PercentAssign => AssignmentOperator::Remainder,
        TokenKind::AmpAssign => AssignmentOperator::BitAnd,
        TokenKind::PipeAssign => AssignmentOperator::BitOr,
        TokenKind::CaretAssign => AssignmentOperator::BitXor,
        TokenKind::ShlAssign => AssignmentOperator::ShiftLeft,
        TokenKind::ShrAssign => AssignmentOperator::ShiftRight,
        _ => return None,
    })
}

fn child_nodes(node: &SyntaxNode) -> Vec<&SyntaxNode> {
    node.children
        .iter()
        .filter_map(|child| match child {
            SyntaxElement::Node(node) => Some(node.as_ref()),
            SyntaxElement::Token(_) => None,
        })
        .collect()
}

fn first_token(node: &SyntaxNode) -> Option<&Token> {
    node.children.iter().find_map(|child| match child {
        SyntaxElement::Token(token) => Some(token),
        SyntaxElement::Node(_) => None,
    })
}

fn first_identifier(node: &SyntaxNode) -> Option<&Token> {
    node.children.iter().find_map(|child| match child {
        SyntaxElement::Token(token) if matches!(token.kind, TokenKind::Identifier(_)) => {
            Some(token)
        }
        _ => None,
    })
}

fn pattern_path_tokens(node: &SyntaxNode) -> Vec<&Token> {
    node.children
        .iter()
        .take_while(|child| {
            !matches!(
                child,
                SyntaxElement::Token(Token {
                    kind: TokenKind::LParen | TokenKind::LBrace,
                    ..
                })
            )
        })
        .filter_map(|child| match child {
            SyntaxElement::Token(token) => Some(token),
            SyntaxElement::Node(_) => None,
        })
        .collect()
}

fn parameter_name_token(node: &SyntaxNode) -> Option<&Token> {
    node.children.iter().find_map(|child| match child {
        SyntaxElement::Token(token)
            if matches!(
                token.kind,
                TokenKind::Identifier(_) | TokenKind::Keyword(Keyword::SelfValue)
            ) =>
        {
            Some(token)
        }
        _ => None,
    })
}

fn let_name_token(node: &SyntaxNode) -> Option<&Token> {
    let mut saw_binding = false;
    node.children.iter().find_map(|child| {
        let SyntaxElement::Token(token) = child else {
            return None;
        };
        match token.kind {
            TokenKind::Keyword(Keyword::Let | Keyword::Var) => {
                saw_binding = true;
                None
            }
            TokenKind::Identifier(_) if saw_binding => Some(token),
            _ => None,
        }
    })
}

fn callee_target_span(node: &SyntaxNode) -> Option<Span> {
    match node.kind {
        SyntaxKind::NameExpression => first_token(node).map(|token| token.span),
        SyntaxKind::MemberExpression => node.children.iter().rev().find_map(|child| match child {
            SyntaxElement::Token(token) if matches!(token.kind, TokenKind::Identifier(_)) => {
                Some(token.span)
            }
            _ => None,
        }),
        SyntaxKind::BracketExpression => child_nodes(node)
            .into_iter()
            .next()
            .and_then(callee_target_span),
        _ => None,
    }
}

fn identifier_text(token: &Token) -> &str {
    match &token.kind {
        TokenKind::Identifier(text) => text,
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::{LogicalCopyStrategy, logical_copy_strategy};
    use crate::types::{Mutability, PrimitiveType, TypeContext, TypeKind};

    #[test]
    fn logical_copy_contract_distinguishes_values_aliases_and_owned_buffers() {
        let mut types = TypeContext::new();
        let integer = types.primitive(PrimitiveType::I32);
        let string = types.primitive(PrimitiveType::String);
        let tuple = types.intern(TypeKind::Tuple(vec![integer, string]));
        let reference = types.intern(TypeKind::Reference {
            mutability: Mutability::Shared,
            target: tuple,
        });
        let pointer = types.intern(TypeKind::RawPointer {
            mutability: Mutability::Mutable,
            target: tuple,
        });

        assert_eq!(
            logical_copy_strategy(&types, integer),
            LogicalCopyStrategy::Trivial
        );
        assert_eq!(
            logical_copy_strategy(&types, string),
            LogicalCopyStrategy::OwnedString
        );
        assert_eq!(
            logical_copy_strategy(&types, tuple),
            LogicalCopyStrategy::Recursive
        );
        assert_eq!(
            logical_copy_strategy(&types, reference),
            LogicalCopyStrategy::PreserveIdentity
        );
        assert_eq!(
            logical_copy_strategy(&types, pointer),
            LogicalCopyStrategy::PreserveIdentity
        );
    }
}
