//! Typed high-level IR and explicit control-flow IR (`IMPL.md` Milestone 8).
//!
//! The high-level form owns selected declaration and type identities while
//! retaining source spans, place classifications, and explicit logical-copy
//! markers from [`crate::check`]. The control-flow form then makes evaluation
//! order, temporaries, branches, loops, calls, returns, and potentially
//! trapping operations explicit before any C is emitted.

use std::collections::BTreeMap;

use crate::check::CheckedProgram;
use crate::diagnostics::{Category, Diagnostic};
use crate::lexer::{FormattedSegmentKind, Keyword, Token, TokenKind};
use crate::parser::{SyntaxElement, SyntaxKind, SyntaxNode};
use crate::resolution::{
    DeclarationId, DeclarationKind, FieldId, ItemId, LocalBindingId, MemberId, NameTarget,
    ResolvedProgram,
};
use crate::source::Span;
use crate::types::{
    PlaceKind, PrimitiveType, TypeId, TypeKind, TypedProgram, parse_integer_magnitude,
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
    Function(DeclarationId),
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
    Tuple(Vec<TypedExpression>),
    Array(Vec<TypedExpression>),
    Struct {
        declaration: DeclarationId,
        fields: Vec<(FieldId, TypedExpression)>,
    },
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
}

impl TypedPlace {
    #[must_use]
    pub fn ty(&self) -> TypeId {
        match self {
            Self::Local { ty, .. } | Self::Field { ty, .. } | Self::Index { ty, .. } => *ty,
        }
    }

    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Self::Local { span, .. } | Self::Field { span, .. } | Self::Index { span, .. } => *span,
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
    pub name: String,
    pub span: Span,
    pub parameters: Vec<TypedParameter>,
    pub return_type: TypeId,
    pub body: Vec<TypedStatement>,
    pub local_types: BTreeMap<LocalBindingId, TypeId>,
}

#[derive(Debug, Clone)]
pub struct TypedStruct {
    pub declaration: DeclarationId,
    pub name: String,
    pub fields: Vec<(FieldId, String, TypeId)>,
}

#[derive(Debug, Default)]
pub struct TypedIrProgram {
    pub functions: Vec<TypedFunction>,
    pub structs: Vec<TypedStruct>,
}

pub struct TypedIrOutput {
    pub program: TypedIrProgram,
    pub diagnostics: Vec<Diagnostic>,
}

/// Lowers the plain, non-generic Milestone 6/7 subset into typed high-level
/// IR. Features assigned to later milestones are not selected.
#[must_use]
pub fn lower_typed_ir(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    checked: &CheckedProgram,
) -> TypedIrOutput {
    TypedLowerer::new(resolved, typed, checked).run()
}

struct TypedLowerer<'a> {
    resolved: &'a ResolvedProgram,
    typed: &'a TypedProgram,
    checked: &'a CheckedProgram,
    binding_by_span: BTreeMap<Span, LocalBindingId>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> TypedLowerer<'a> {
    fn new(
        resolved: &'a ResolvedProgram,
        typed: &'a TypedProgram,
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
        }
    }

    fn run(mut self) -> TypedIrOutput {
        let mut program = TypedIrProgram::default();
        for declaration in &self.resolved.declarations {
            if declaration.kind == DeclarationKind::Struct
                && declaration.generic_parameters.is_empty()
            {
                let fields = self
                    .resolved
                    .fields
                    .iter()
                    .filter(|field| {
                        field.parent_declaration == declaration.id && field.parent_variant.is_none()
                    })
                    .filter_map(|field| {
                        self.typed.field_types.get(&field.id).copied().map(|ty| {
                            (
                                field.id,
                                self.resolved.symbol_text(field.name).to_string(),
                                ty,
                            )
                        })
                    })
                    .collect();
                program.structs.push(TypedStruct {
                    declaration: declaration.id,
                    name: self.resolved.symbol_text(declaration.name).to_string(),
                    fields,
                });
            }
        }
        for declaration in &self.resolved.declarations {
            if declaration.kind != DeclarationKind::Function
                || declaration.parent_declaration.is_some()
                || declaration.parent_impl.is_some()
                || !declaration.generic_parameters.is_empty()
            {
                continue;
            }
            if let Some(function) = self.lower_function(declaration.id) {
                program.functions.push(function);
            }
        }
        TypedIrOutput {
            program,
            diagnostics: self.diagnostics,
        }
    }

    fn lower_function(&mut self, declaration: DeclarationId) -> Option<TypedFunction> {
        let data = &self.resolved.declarations[declaration.index()];
        let signature = self.typed.function_signatures.get(&declaration)?;
        let mut parameters = Vec::new();
        let mut local_types = BTreeMap::new();
        if let Some(parameter_list) =
            crate::types::direct_child(&data.syntax, SyntaxKind::Parameters)
        {
            let nodes = crate::types::direct_children(parameter_list, SyntaxKind::Parameter);
            for (node, parameter) in nodes.into_iter().zip(&signature.parameters) {
                let Some(token) = parameter_name_token(node) else {
                    continue;
                };
                let Some(binding) = self.binding_by_span.get(&token.span).copied() else {
                    continue;
                };
                parameters.push(TypedParameter {
                    binding,
                    ty: parameter.ty,
                    span: token.span,
                });
                local_types.insert(binding, parameter.ty);
            }
        }
        let body = crate::types::direct_child(&data.syntax, SyntaxKind::Block)
            .map(|block| self.lower_block(block, &mut local_types))
            .unwrap_or_default();
        Some(TypedFunction {
            declaration,
            name: self.resolved.symbol_text(data.name).to_string(),
            span: data.span,
            parameters,
            return_type: signature.return_type,
            body,
            local_types,
        })
    }

    fn lower_block(
        &mut self,
        block: &SyntaxNode,
        local_types: &mut BTreeMap<LocalBindingId, TypeId>,
    ) -> Vec<TypedStatement> {
        child_nodes(block)
            .into_iter()
            .filter_map(|statement| self.lower_statement(statement, local_types))
            .collect()
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
                let place = self.lower_place(*nodes.first()?)?;
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
            SyntaxKind::UnsafeBlock => {
                let block = child_nodes(node)
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)?;
                TypedStatementKind::Block(self.lower_block(block, local_types))
            }
            SyntaxKind::BreakStatement => TypedStatementKind::Break,
            SyntaxKind::ContinueStatement => TypedStatementKind::Continue,
            SyntaxKind::PassStatement => TypedStatementKind::Pass,
            SyntaxKind::MatchStatement | SyntaxKind::ForStatement | SyntaxKind::DeferStatement => {
                self.unsupported(
                    node.span,
                    match node.kind {
                        SyntaxKind::MatchStatement => "`match` lowering",
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

    fn lower_expression(&mut self, node: &SyntaxNode) -> Option<TypedExpression> {
        let ty = self
            .checked
            .expression_types
            .get(&node.span)
            .copied()
            .unwrap_or_else(|| self.typed.types.error());
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
            SyntaxKind::UnaryExpression => {
                let operand_node = child_nodes(node).into_iter().next_back()?;
                let token = first_token(node)?;
                if matches!(token.kind, TokenKind::Amp | TokenKind::Star) {
                    self.unsupported(node.span, "managed or raw reference lowering");
                    return None;
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
                let base_node = child_nodes(node).into_iter().next()?;
                let field = self.resolve_field(node, base_node)?;
                TypedExpressionKind::Field {
                    base: Box::new(self.lower_expression(base_node)?),
                    field,
                }
            }
            SyntaxKind::BracketExpression => {
                let nodes = child_nodes(node);
                TypedExpressionKind::Index {
                    base: Box::new(self.lower_expression(*nodes.first()?)?),
                    index: Box::new(self.lower_expression(*nodes.get(1)?)?),
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
        let span = callee_target_span(callee_node)?;
        let reference = self.resolved.reference_at(span)?;
        let callee = match reference.target {
            NameTarget::Item(ItemId::Declaration(declaration))
                if self.resolved.declarations[declaration.index()].kind
                    == DeclarationKind::Function =>
            {
                TypedCallee::Function(declaration)
            }
            NameTarget::Item(ItemId::Builtin(builtin))
                if matches!(self.resolved.builtin_name(builtin), "print" | "println") =>
            {
                TypedCallee::Print {
                    newline: self.resolved.builtin_name(builtin) == "println",
                }
            }
            _ => {
                self.unsupported(node.span, "an indirect, method, generic, or foreign call");
                return None;
            }
        };
        let is_print = matches!(callee, TypedCallee::Print { .. });
        let arguments = nodes[1..]
            .iter()
            .map(|argument| {
                if is_print && argument.kind == SyntaxKind::FormattedStringExpression {
                    self.lower_print_formatted(argument)
                } else {
                    self.lower_expression(argument)
                }
            })
            .collect::<Option<Vec<_>>>()?;
        Some(TypedExpressionKind::Call { callee, arguments })
    }

    fn lower_print_formatted(&mut self, node: &SyntaxNode) -> Option<TypedExpression> {
        let ty = self.checked.expression_types.get(&node.span).copied()?;
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
        let span = callee_target_span(callee)?;
        let NameTarget::Item(ItemId::Declaration(declaration)) =
            self.resolved.reference_at(span)?.target
        else {
            self.unsupported(node.span, "a non-struct record construction");
            return None;
        };
        if self.resolved.declarations[declaration.index()].kind != DeclarationKind::Struct {
            self.unsupported(node.span, "enum record construction");
            return None;
        }
        let mut fields = Vec::new();
        for field_node in child_nodes(node)
            .into_iter()
            .filter(|child| child.kind == SyntaxKind::RecordField)
        {
            let name_token = first_identifier(field_node)?;
            let field = self.find_field(declaration, identifier_text(name_token))?;
            let value = if let Some(value) = child_nodes(field_node).into_iter().next() {
                self.lower_expression(value)?
            } else {
                let target = self.resolved.reference_at(name_token.span)?.target;
                let NameTarget::Local(binding) = target else {
                    return None;
                };
                let ty = self.typed.field_types.get(&field).copied()?;
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
        Some(TypedExpressionKind::Struct {
            declaration,
            fields,
        })
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
        let ty = self.checked.expression_types.get(&node.span).copied()?;
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
                    base: Box::new(self.lower_place(base_node)?),
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
            _ => None,
        }
    }

    fn resolve_field(&self, node: &SyntaxNode, base: &SyntaxNode) -> Option<FieldId> {
        let base_type = self.checked.expression_types.get(&base.span).copied()?;
        let declaration = match self.expanded_kind(base_type) {
            TypeKind::Nominal { identity, .. } => identity.declaration,
            _ => return None,
        };
        let name = identifier_text(first_identifier(node)?);
        self.find_field(declaration, name)
    }

    fn find_field(&self, declaration: DeclarationId, name: &str) -> Option<FieldId> {
        self.resolved
            .declaration_members
            .get(&declaration)?
            .iter()
            .find_map(|(symbol, member)| {
                (self.resolved.symbol_text(*symbol) == name).then_some(*member)
            })
            .and_then(|member| match member {
                MemberId::Field(field) => Some(field),
                _ => None,
            })
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
    Index {
        base: Box<ControlFlowPlace>,
        index: TemporaryId,
        length: u128,
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
}

#[derive(Debug, Clone)]
pub enum Rvalue {
    Constant(Constant),
    Load(ControlFlowPlace),
    Copy(TemporaryId),
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
        declaration: DeclarationId,
        arguments: Vec<TemporaryId>,
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
    pub name: String,
    pub span: Span,
    pub parameters: Vec<TypedParameter>,
    pub return_type: TypeId,
    pub local_types: BTreeMap<LocalBindingId, TypeId>,
    pub temporary_types: Vec<TypeId>,
    pub entry: BlockId,
    pub blocks: Vec<BasicBlock>,
}

#[derive(Debug, Default)]
pub struct ControlFlowProgram {
    pub functions: Vec<ControlFlowFunction>,
    pub structs: Vec<TypedStruct>,
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
            name: self.function.name.clone(),
            span: self.function.span,
            parameters: self.function.parameters.clone(),
            return_type: self.function.return_type,
            local_types: self.function.local_types.clone(),
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

    fn lower_expression(&mut self, expression: &TypedExpression) -> TemporaryId {
        let value = match &expression.kind {
            TypedExpressionKind::Constant(constant) => Rvalue::Constant(constant.clone()),
            TypedExpressionKind::Local(binding) => Rvalue::Load(ControlFlowPlace::Local(*binding)),
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
                callee: TypedCallee::Function(declaration),
                arguments,
            } => {
                let arguments = arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect();
                Rvalue::Call {
                    declaration: *declaration,
                    arguments,
                }
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
                    TypeKind::Array { length, .. } => *length,
                    _ => 0,
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
                    length: *length,
                    trap: TrapKind::IndexOutOfBounds,
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
                    TypeKind::Array { length, .. } => *length,
                    _ => 0,
                };
                ControlFlowPlace::Index {
                    base: Box::new(base),
                    index,
                    length,
                    trap: TrapKind::IndexOutOfBounds,
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
        _ => None,
    }
}

fn identifier_text(token: &Token) -> &str {
    match &token.kind {
        TokenKind::Identifier(text) => text,
        _ => "",
    }
}
