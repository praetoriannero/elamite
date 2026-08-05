//! Checked, deterministic interpreter for compile-time declarations.
//!
//! The interpreter deliberately consumes the phase-neutral syntax tree and
//! produces detached structural syntax values. It has no host services and is
//! metered by the scheduler's per-execution resource account.

use std::collections::BTreeMap;

use crate::diagnostics::{Category, Diagnostic};
use crate::parser::{QuoteFragmentKind, parse_quote_fragment};
use crate::source::Span;
use crate::syntax::{
    FormattedSegmentKind, Keyword, SyntaxElement, SyntaxKind, SyntaxNode, Token, TokenKind,
};

use super::namespace::{CompileTimeDeclaration, CompileTimeNamespace};
use super::quote::{QuoteRole, role_from_type};
use super::scheduler::{ExecutionResources, ResourceLimitKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileTimeType {
    Unit,
    Bool,
    Integer,
    String,
    Ast(QuoteRole),
    Sequence(Box<CompileTimeType>),
}

impl CompileTimeType {
    fn description(&self) -> String {
        match self {
            Self::Unit => "()`".to_string(),
            Self::Bool => "bool".to_string(),
            Self::Integer => "an integer".to_string(),
            Self::String => "str".to_string(),
            Self::Ast(role) => format!("std.ast.{role:?}"),
            Self::Sequence(element) => format!("[{}]", element.description()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompileTimeParameter {
    pub name: String,
    pub ty: CompileTimeType,
    pub variadic: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct LoweredDeclaration {
    pub namespace: CompileTimeNamespace,
    pub name: String,
    pub parameters: Vec<CompileTimeParameter>,
    pub return_type: CompileTimeType,
    pub body: SyntaxNode,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Value {
    Unit,
    Bool(bool),
    Integer(i128),
    String(String),
    Identifier(String),
    Sequence(Vec<Value>),
    Syntax {
        role: QuoteRole,
        nodes: Vec<SyntaxNode>,
    },
    CallView(SyntaxNode),
    MemberView(SyntaxNode),
}

impl Value {
    #[must_use]
    pub fn syntax(role: QuoteRole, node: SyntaxNode) -> Self {
        Self::Syntax {
            role,
            nodes: vec![node],
        }
    }

    #[must_use]
    pub fn syntax_nodes(role: QuoteRole, nodes: Vec<SyntaxNode>) -> Self {
        Self::Syntax { role, nodes }
    }

    #[must_use]
    pub fn into_syntax(self) -> Option<(QuoteRole, Vec<SyntaxNode>)> {
        match self {
            Self::Syntax { role, nodes } => Some((role, nodes)),
            _ => None,
        }
    }

    fn live_bytes(&self) -> u64 {
        match self {
            Self::Unit | Self::Bool(_) | Self::Integer(_) => 16,
            Self::String(value) | Self::Identifier(value) => 24 + value.len() as u64,
            Self::Sequence(values) => 24 + values.iter().map(Self::live_bytes).sum::<u64>(),
            Self::Syntax { nodes, .. } => 32 + nodes.iter().map(syntax_bytes).sum::<u64>(),
            Self::CallView(node) => 24 + syntax_bytes(node),
            Self::MemberView(node) => 24 + syntax_bytes(node),
        }
    }
}

#[derive(Debug)]
pub struct ExecutionFailure {
    pub message: String,
    pub span: Span,
    pub limit: Option<ResourceLimitKind>,
}

impl ExecutionFailure {
    #[must_use]
    pub fn diagnostic(&self, declaration: &LoweredDeclaration) -> Diagnostic {
        Diagnostic::new(Category::CompileTime, &self.message)
            .with_primary(self.span)
            .with_related(declaration.span, "compile-time declaration is here")
    }
}

/// Validates and lowers one physical compile-time declaration. The admitted
/// body stays in a small, version-independent interpreter IR represented by
/// phase-neutral nodes; no runtime type or resolved compiler table is retained.
pub fn lower_declaration(
    declaration: &CompileTimeDeclaration,
) -> Result<LoweredDeclaration, Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let syntax = &declaration.syntax;
    let parameters = syntax
        .direct_child(SyntaxKind::Parameters)
        .into_iter()
        .flat_map(|parameters| parameters.direct_children(SyntaxKind::Parameter))
        .filter_map(|parameter| lower_parameter(parameter, &mut diagnostics))
        .collect::<Vec<_>>();
    let return_type = syntax
        .direct_children(SyntaxKind::Type)
        .into_iter()
        .next_back()
        .and_then(parse_compile_time_type);
    let Some(return_type) = return_type else {
        diagnostics.push(
            Diagnostic::new(
                Category::CompileTime,
                "compile-time return types must use deterministic values or `std.ast`",
            )
            .with_primary(syntax.span),
        );
        return Err(diagnostics);
    };
    validate_signature(
        declaration.namespace,
        &parameters,
        &return_type,
        syntax.span,
        &mut diagnostics,
    );
    let body = syntax
        .direct_child(SyntaxKind::Block)
        .cloned()
        .unwrap_or_else(|| SyntaxNode::new(SyntaxKind::Block, Vec::new(), syntax.span));
    validate_capabilities(&body, &mut diagnostics);
    if diagnostics.is_empty() {
        Ok(LoweredDeclaration {
            namespace: declaration.namespace,
            name: declaration.name.clone(),
            parameters,
            return_type,
            body,
            span: syntax.span,
        })
    } else {
        Err(diagnostics)
    }
}

fn lower_parameter(
    node: &SyntaxNode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<CompileTimeParameter> {
    let name = node
        .direct_tokens()
        .into_iter()
        .find_map(|token| match &token.kind {
            TokenKind::Identifier(name) => Some(name.clone()),
            _ => None,
        })?;
    let variadic = node
        .direct_tokens()
        .iter()
        .any(|token| token.kind == TokenKind::Ellipsis);
    let ty_node = node.direct_child(SyntaxKind::Type)?;
    let Some(mut ty) = parse_compile_time_type(ty_node) else {
        diagnostics.push(
            Diagnostic::new(
                Category::CompileTime,
                "compile-time parameters cannot contain references, pointers, functions, or runtime-only types",
            )
            .with_primary(ty_node.span),
        );
        return None;
    };
    if variadic {
        ty = CompileTimeType::Sequence(Box::new(ty));
    }
    Some(CompileTimeParameter {
        name,
        ty,
        variadic,
        span: node.span,
    })
}

fn parse_compile_time_type(node: &SyntaxNode) -> Option<CompileTimeType> {
    if let Some(role) = role_from_type(node) {
        return Some(CompileTimeType::Ast(role));
    }
    let text = type_text(node);
    Some(match text.as_str() {
        "()" => CompileTimeType::Unit,
        "bool" => CompileTimeType::Bool,
        "str" | "String" => CompileTimeType::String,
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" => CompileTimeType::Integer,
        _ if text.starts_with('[') && text.ends_with(']') => {
            let inner = &text[1..text.len() - 1];
            let role = role_from_text(inner)?;
            CompileTimeType::Sequence(Box::new(CompileTimeType::Ast(role)))
        }
        _ => CompileTimeType::Ast(role_from_text(&text)?),
    })
}

fn role_from_text(text: &str) -> Option<QuoteRole> {
    Some(match text {
        "std.ast.Expression" => QuoteRole::Expression,
        "std.ast.Pattern" => QuoteRole::Pattern,
        "std.ast.TypeSyntax" => QuoteRole::TypeSyntax,
        "std.ast.StatementList" => QuoteRole::StatementList,
        "std.ast.MemberList" => QuoteRole::MemberList,
        "std.ast.Item" => QuoteRole::Item,
        "std.ast.ItemList" => QuoteRole::ItemList,
        "std.ast.StructDefinition" => QuoteRole::StructDefinition,
        "std.ast.EnumDefinition" => QuoteRole::EnumDefinition,
        "std.ast.FunctionDefinition" => QuoteRole::FunctionDefinition,
        "std.ast.Implementation" => QuoteRole::Implementation,
        "std.ast.InherentImplementation" => QuoteRole::InherentImplementation,
        "std.ast.FieldDefinition" => QuoteRole::FieldDefinition,
        _ => return None,
    })
}

fn validate_signature(
    namespace: CompileTimeNamespace,
    parameters: &[CompileTimeParameter],
    return_type: &CompileTimeType,
    span: Span,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let variadics = parameters
        .iter()
        .enumerate()
        .filter(|(_, parameter)| parameter.variadic)
        .collect::<Vec<_>>();
    if variadics.len() > 1
        || variadics
            .first()
            .is_some_and(|(index, _)| *index + 1 != parameters.len())
    {
        diagnostics.push(
            Diagnostic::new(
                Category::CompileTime,
                "only the final compile-time parameter may be variadic",
            )
            .with_primary(span),
        );
    }
    match namespace {
        CompileTimeNamespace::Macro => {
            if !matches!(
                return_type,
                CompileTimeType::Ast(
                    QuoteRole::Expression
                        | QuoteRole::Pattern
                        | QuoteRole::TypeSyntax
                        | QuoteRole::StatementList
                        | QuoteRole::Item
                        | QuoteRole::ItemList
                )
            ) {
                diagnostics.push(
                    Diagnostic::new(
                        Category::CompileTime,
                        "a macro must return an expandable `std.ast` role",
                    )
                    .with_primary(span),
                );
            }
        }
        CompileTimeNamespace::Attribute => {
            if parameters.is_empty()
                || !matches!(
                    parameters[0].ty,
                    CompileTimeType::Ast(
                        QuoteRole::StructDefinition
                            | QuoteRole::EnumDefinition
                            | QuoteRole::FunctionDefinition
                    )
                )
            {
                diagnostics.push(
                    Diagnostic::new(
                        Category::CompileTime,
                        "an attribute's first parameter must be its definition target",
                    )
                    .with_primary(span),
                );
            }
            if let Some(CompileTimeParameter {
                ty: CompileTimeType::Ast(target),
                ..
            }) = parameters.first()
                && !matches!(
                    return_type,
                    CompileTimeType::Ast(returned)
                        if returned == target || *returned == QuoteRole::ItemList
                )
            {
                diagnostics.push(
                    Diagnostic::new(
                        Category::CompileTime,
                        "an attribute must return its target definition kind or `std.ast.ItemList`",
                    )
                    .with_primary(span),
                );
            }
        }
        CompileTimeNamespace::Derive => {
            if parameters.len() != 1 || parameters[0].variadic {
                diagnostics.push(
                    Diagnostic::new(
                        Category::CompileTime,
                        "a derive requires exactly one fixed target parameter",
                    )
                    .with_primary(span),
                );
            }
            if !matches!(return_type, CompileTimeType::Ast(QuoteRole::Implementation)) {
                diagnostics.push(
                    Diagnostic::new(
                        Category::CompileTime,
                        "a derive must return `std.ast.Implementation`",
                    )
                    .with_primary(span),
                );
            }
            if !matches!(
                parameters.first().map(|parameter| &parameter.ty),
                Some(CompileTimeType::Ast(
                    QuoteRole::StructDefinition | QuoteRole::EnumDefinition
                ))
            ) {
                diagnostics.push(
                    Diagnostic::new(
                        Category::CompileTime,
                        "a derive target must be `std.ast.StructDefinition` or `std.ast.EnumDefinition`",
                    )
                    .with_primary(span),
                );
            }
        }
    }
}

fn validate_capabilities(node: &SyntaxNode, diagnostics: &mut Vec<Diagnostic>) {
    if matches!(
        node.kind,
        SyntaxKind::UnsafeBlock | SyntaxKind::DeferStatement | SyntaxKind::ClosureExpression
    ) {
        diagnostics.push(
            Diagnostic::new(
                Category::CompileTime,
                "this runtime or unsafe capability is unavailable during compile-time execution",
            )
            .with_primary(node.span),
        );
    }
    if node.kind == SyntaxKind::CallExpression {
        let path = node
            .direct_nodes()
            .first()
            .and_then(|callee| expression_path(callee));
        if path.as_deref().is_some_and(|path| {
            [
                "std.fs",
                "std.env",
                "std.process",
                "std.net",
                "std.time",
                "std.random",
                "std.thread",
                "std.sync",
                "std.target",
                "std.ffi",
            ]
            .iter()
            .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}.")))
        }) {
            diagnostics.push(
                Diagnostic::new(
                    Category::CompileTime,
                    "compile-time code has no ambient host, target, FFI, or threading capabilities",
                )
                .with_primary(node.span),
            );
        }
    }
    for child in node.direct_nodes() {
        validate_capabilities(child, diagnostics);
    }
}

pub fn execute(
    declaration: &LoweredDeclaration,
    arguments: Vec<Value>,
    resources: &mut ExecutionResources,
) -> Result<Value, ExecutionFailure> {
    let mut evaluator = Evaluator {
        declaration,
        resources,
        scopes: vec![BTreeMap::new()],
    };
    evaluator.bind_arguments(arguments)?;
    let result = match evaluator.execute_block(&declaration.body)? {
        Flow::Return(value) => value,
        _ if declaration.return_type == CompileTimeType::Unit => Value::Unit,
        _ => {
            return evaluator.fail(
                declaration.span,
                "compile-time execution reached the end without returning a value",
            );
        }
    };
    if !value_matches_type(&result, &declaration.return_type) {
        return evaluator.fail(
            declaration.span,
            format!(
                "compile-time declaration returned the wrong value; expected {}",
                declaration.return_type.description()
            ),
        );
    }
    Ok(result)
}

struct Evaluator<'a> {
    declaration: &'a LoweredDeclaration,
    resources: &'a mut ExecutionResources,
    scopes: Vec<BTreeMap<String, Value>>,
}

#[derive(Debug)]
enum Flow {
    Continue,
    Return(Value),
    Break,
    LoopContinue,
}

impl Evaluator<'_> {
    fn bind_arguments(&mut self, arguments: Vec<Value>) -> Result<(), ExecutionFailure> {
        let fixed = self
            .declaration
            .parameters
            .iter()
            .take_while(|parameter| !parameter.variadic)
            .count();
        let variadic = self
            .declaration
            .parameters
            .last()
            .is_some_and(|parameter| parameter.variadic);
        if (!variadic && arguments.len() != fixed) || (variadic && arguments.len() < fixed) {
            return self.fail(
                self.declaration.span,
                format!(
                    "compile-time call has {} arguments but `{}` expects {}{}",
                    arguments.len(),
                    self.declaration.name,
                    fixed,
                    if variadic { " or more" } else { "" }
                ),
            );
        }
        let mut arguments = arguments.into_iter();
        for parameter in &self.declaration.parameters {
            let value = if parameter.variadic {
                Value::Sequence(arguments.by_ref().collect())
            } else {
                arguments.next().expect("arity checked above")
            };
            if !value_matches_type(&value, &parameter.ty) {
                return self.fail(
                    parameter.span,
                    format!(
                        "argument `{}` does not match {}",
                        parameter.name,
                        parameter.ty.description()
                    ),
                );
            }
            self.store(parameter.name.clone(), value)?;
        }
        Ok(())
    }

    fn execute_block(&mut self, block: &SyntaxNode) -> Result<Flow, ExecutionFailure> {
        self.step(block.span)?;
        self.scopes.push(BTreeMap::new());
        for statement in block.direct_nodes() {
            let flow = self.execute_statement(statement)?;
            if !matches!(flow, Flow::Continue) {
                self.scopes.pop();
                return Ok(flow);
            }
        }
        self.scopes.pop();
        Ok(Flow::Continue)
    }

    fn execute_statement(&mut self, node: &SyntaxNode) -> Result<Flow, ExecutionFailure> {
        self.step(node.span)?;
        match node.kind {
            SyntaxKind::PassStatement => Ok(Flow::Continue),
            SyntaxKind::LetStatement => {
                let name = first_identifier(node)
                    .ok_or_else(|| self.failure(node.span, "invalid compile-time binding"))?;
                let expression = expression_children(node)
                    .into_iter()
                    .next_back()
                    .ok_or_else(|| self.failure(node.span, "compile-time binding has no value"))?;
                let expected = node
                    .direct_child(SyntaxKind::Type)
                    .and_then(parse_compile_time_type);
                let value = self.evaluate(expression, expected.as_ref())?;
                if expected
                    .as_ref()
                    .is_some_and(|expected| !value_matches_type(&value, expected))
                {
                    return self.fail(
                        node.span,
                        "compile-time binding value does not match its annotation",
                    );
                }
                self.store(name, value)?;
                Ok(Flow::Continue)
            }
            SyntaxKind::AssignmentStatement => {
                let expressions = expression_children(node);
                let name = expressions
                    .first()
                    .and_then(|target| expression_path(target))
                    .ok_or_else(|| {
                        self.failure(node.span, "compile-time assignment requires a local name")
                    })?;
                let value = self.evaluate(
                    expressions.get(1).copied().ok_or_else(|| {
                        self.failure(node.span, "compile-time assignment has no value")
                    })?,
                    None,
                )?;
                self.assign(&name, value)?;
                Ok(Flow::Continue)
            }
            SyntaxKind::ExpressionStatement => {
                if let Some(expression) = expression_children(node).first() {
                    self.evaluate(expression, None)?;
                }
                Ok(Flow::Continue)
            }
            SyntaxKind::ReturnStatement => {
                let value = match expression_children(node).first() {
                    Some(expression) => {
                        self.evaluate(expression, Some(&self.declaration.return_type))?
                    }
                    None => Value::Unit,
                };
                Ok(Flow::Return(value))
            }
            SyntaxKind::BreakStatement => Ok(Flow::Break),
            SyntaxKind::ContinueStatement => Ok(Flow::LoopContinue),
            SyntaxKind::IfStatement => self.execute_if(node),
            SyntaxKind::WhileStatement => self.execute_while(node),
            SyntaxKind::ForStatement => self.execute_for(node),
            SyntaxKind::MatchStatement => self.execute_match(node),
            _ => self.fail(
                node.span,
                format!(
                    "{:?} is not supported by the compile-time interpreter",
                    node.kind
                ),
            ),
        }
    }

    fn execute_if(&mut self, node: &SyntaxNode) -> Result<Flow, ExecutionFailure> {
        let condition = expression_children(node)
            .first()
            .copied()
            .ok_or_else(|| self.failure(node.span, "if has no condition"))?;
        if self
            .evaluate(condition, Some(&CompileTimeType::Bool))?
            .into_bool(node.span)?
        {
            return self.execute_block(
                node.direct_child(SyntaxKind::Block)
                    .expect("parsed if block"),
            );
        }
        if let Some(otherwise) = node.direct_child(SyntaxKind::ElseClause)
            && let Some(block) = otherwise.direct_child(SyntaxKind::Block)
        {
            return self.execute_block(block);
        }
        Ok(Flow::Continue)
    }

    fn execute_while(&mut self, node: &SyntaxNode) -> Result<Flow, ExecutionFailure> {
        let condition = expression_children(node)
            .first()
            .copied()
            .ok_or_else(|| self.failure(node.span, "while has no condition"))?;
        let block = node
            .direct_child(SyntaxKind::Block)
            .expect("parsed while block");
        loop {
            self.step(node.span)?;
            if !self
                .evaluate(condition, Some(&CompileTimeType::Bool))?
                .into_bool(node.span)?
            {
                return Ok(Flow::Continue);
            }
            match self.execute_block(block)? {
                Flow::Continue | Flow::LoopContinue => {}
                Flow::Break => return Ok(Flow::Continue),
                flow @ Flow::Return(_) => return Ok(flow),
            }
        }
    }

    fn execute_for(&mut self, node: &SyntaxNode) -> Result<Flow, ExecutionFailure> {
        let name = first_identifier_after(node, Keyword::For)
            .ok_or_else(|| self.failure(node.span, "for has no binding"))?;
        let iterable = expression_children(node)
            .first()
            .copied()
            .ok_or_else(|| self.failure(node.span, "for has no iterable"))?;
        let values = match self.evaluate(iterable, None)? {
            Value::Sequence(values) => values,
            Value::Syntax { role, nodes } if role_is_list(role) => nodes
                .into_iter()
                .map(|node| Value::syntax(element_role(role, &node), node))
                .collect(),
            _ => {
                return self.fail(
                    node.span,
                    "compile-time for loops require a deterministic sequence",
                );
            }
        };
        let block = node
            .direct_child(SyntaxKind::Block)
            .expect("parsed for block");
        for value in values {
            self.step(node.span)?;
            self.scopes.push(BTreeMap::new());
            self.store(name.clone(), value)?;
            let flow = self.execute_block(block)?;
            self.scopes.pop();
            match flow {
                Flow::Continue | Flow::LoopContinue => {}
                Flow::Break => break,
                flow @ Flow::Return(_) => return Ok(flow),
            }
        }
        Ok(Flow::Continue)
    }

    fn execute_match(&mut self, node: &SyntaxNode) -> Result<Flow, ExecutionFailure> {
        let expression = expression_children(node)
            .first()
            .copied()
            .ok_or_else(|| self.failure(node.span, "match has no value"))?;
        let value = self.evaluate(expression, None)?;
        let block = node
            .direct_child(SyntaxKind::Block)
            .expect("parsed match block");
        for arm in block.direct_children(SyntaxKind::MatchArm) {
            let pattern = arm
                .direct_nodes()
                .first()
                .copied()
                .expect("parsed match pattern");
            let mut bindings = BTreeMap::new();
            if pattern_matches(pattern, &value, &mut bindings) {
                self.scopes.push(bindings);
                if let Some(guard) = arm.direct_child(SyntaxKind::Guard)
                    && let Some(condition) = expression_children(guard).first()
                    && !self
                        .evaluate(condition, Some(&CompileTimeType::Bool))?
                        .into_bool(guard.span)?
                {
                    self.scopes.pop();
                    continue;
                }
                let flow = self.execute_block(
                    arm.direct_child(SyntaxKind::Block)
                        .expect("parsed match arm block"),
                )?;
                self.scopes.pop();
                return Ok(flow);
            }
        }
        self.fail(
            node.span,
            "compile-time match is not exhaustive for this value",
        )
    }

    fn evaluate(
        &mut self,
        node: &SyntaxNode,
        expected: Option<&CompileTimeType>,
    ) -> Result<Value, ExecutionFailure> {
        self.step(node.span)?;
        let value = match node.kind {
            SyntaxKind::LiteralExpression => evaluate_literal(node)?,
            SyntaxKind::FormattedStringExpression => self.evaluate_formatted(node)?,
            SyntaxKind::NameExpression => {
                let name = expression_path(node)
                    .ok_or_else(|| self.failure(node.span, "invalid compile-time name"))?;
                self.lookup(&name).cloned().ok_or_else(|| {
                    self.failure(
                        node.span,
                        format!("compile-time name `{name}` is not defined"),
                    )
                })?
            }
            SyntaxKind::ParenthesizedExpression => {
                self.evaluate(expression_children(node)[0], expected)?
            }
            SyntaxKind::TupleExpression | SyntaxKind::ArrayExpression => {
                let mut values = Vec::new();
                for element in expression_children(node) {
                    values.push(self.evaluate(element, None)?);
                }
                Value::Sequence(values)
            }
            SyntaxKind::UnaryExpression => self.evaluate_unary(node)?,
            SyntaxKind::BinaryExpression => self.evaluate_binary(node)?,
            SyntaxKind::CallExpression => self.evaluate_call(node)?,
            SyntaxKind::MemberExpression => self.evaluate_member(node)?,
            SyntaxKind::QuoteExpression => {
                let Some(CompileTimeType::Ast(role)) = expected else {
                    return self.fail(
                        node.span,
                        "a compile-time quote needs an explicit `std.ast` role",
                    );
                };
                self.evaluate_quote(node, *role)?
            }
            _ => {
                return self.fail(
                    node.span,
                    format!(
                        "{:?} is not supported in a compile-time expression",
                        node.kind
                    ),
                );
            }
        };
        self.allocate(&value, node.span)?;
        Ok(value)
    }

    fn evaluate_unary(&mut self, node: &SyntaxNode) -> Result<Value, ExecutionFailure> {
        let value = self.evaluate(expression_children(node)[0], None)?;
        let operator = node.direct_tokens().first().map(|token| &token.kind);
        match (operator, value) {
            (Some(TokenKind::Bang), Value::Bool(value)) => Ok(Value::Bool(!value)),
            (Some(TokenKind::Minus), Value::Integer(value)) => Ok(Value::Integer(-value)),
            (Some(TokenKind::Plus), Value::Integer(value)) => Ok(Value::Integer(value)),
            _ => self.fail(
                node.span,
                "invalid unary operation during compile-time execution",
            ),
        }
    }

    fn evaluate_formatted(&mut self, node: &SyntaxNode) -> Result<Value, ExecutionFailure> {
        let token =
            node.direct_tokens().first().copied().ok_or_else(|| {
                self.failure(node.span, "formatted compile-time string has no token")
            })?;
        let TokenKind::FormattedString(segments) = &token.kind else {
            return self.fail(node.span, "invalid formatted compile-time string");
        };
        let mut expressions = expression_children(node).into_iter();
        let mut output = String::new();
        for segment in segments {
            match &segment.kind {
                FormattedSegmentKind::Text(text) => output.push_str(text),
                FormattedSegmentKind::Expression { .. } => {
                    let expression = expressions.next().ok_or_else(|| {
                        self.failure(segment.span, "formatted string interpolation is missing")
                    })?;
                    output.push_str(&display_value(&self.evaluate(expression, None)?));
                }
            }
        }
        Ok(Value::String(output))
    }

    fn evaluate_binary(&mut self, node: &SyntaxNode) -> Result<Value, ExecutionFailure> {
        let expressions = expression_children(node);
        let left = self.evaluate(expressions[0], None)?;
        let right = self.evaluate(expressions[1], None)?;
        let operator = node.direct_tokens().first().map(|token| &token.kind);
        match (operator, left, right) {
            (Some(TokenKind::PlusPlus), Value::String(mut left), Value::String(right)) => {
                left.push_str(&right);
                Ok(Value::String(left))
            }
            (Some(TokenKind::PlusPlus), Value::Sequence(mut left), Value::Sequence(right)) => {
                left.extend(right);
                Ok(Value::Sequence(left))
            }
            (
                Some(TokenKind::PlusPlus),
                Value::Syntax {
                    role: left_role,
                    mut nodes,
                },
                Value::Syntax {
                    role: right_role,
                    nodes: right,
                },
            ) if left_role == right_role && role_is_list(left_role) => {
                nodes.extend(right);
                Ok(Value::Syntax {
                    role: left_role,
                    nodes,
                })
            }
            (Some(TokenKind::Plus), Value::Integer(left), Value::Integer(right)) => {
                Ok(Value::Integer(left + right))
            }
            (Some(TokenKind::Minus), Value::Integer(left), Value::Integer(right)) => {
                Ok(Value::Integer(left - right))
            }
            (Some(TokenKind::Star), Value::Integer(left), Value::Integer(right)) => {
                Ok(Value::Integer(left * right))
            }
            (Some(TokenKind::Slash), Value::Integer(_), Value::Integer(0)) => {
                self.fail(node.span, "division by zero in compile-time code")
            }
            (Some(TokenKind::Slash), Value::Integer(left), Value::Integer(right)) => {
                Ok(Value::Integer(left / right))
            }
            (Some(TokenKind::Percent), Value::Integer(_), Value::Integer(0)) => {
                self.fail(node.span, "remainder by zero in compile-time code")
            }
            (Some(TokenKind::Percent), Value::Integer(left), Value::Integer(right)) => {
                Ok(Value::Integer(left % right))
            }
            (Some(TokenKind::EqEq), left, right) => Ok(Value::Bool(values_equal(&left, &right))),
            (Some(TokenKind::NotEq), left, right) => Ok(Value::Bool(!values_equal(&left, &right))),
            (Some(TokenKind::Less), Value::Integer(left), Value::Integer(right)) => {
                Ok(Value::Bool(left < right))
            }
            (Some(TokenKind::LessEq), Value::Integer(left), Value::Integer(right)) => {
                Ok(Value::Bool(left <= right))
            }
            (Some(TokenKind::Greater), Value::Integer(left), Value::Integer(right)) => {
                Ok(Value::Bool(left > right))
            }
            (Some(TokenKind::GreaterEq), Value::Integer(left), Value::Integer(right)) => {
                Ok(Value::Bool(left >= right))
            }
            (Some(TokenKind::AndAnd), Value::Bool(left), Value::Bool(right)) => {
                Ok(Value::Bool(left && right))
            }
            (Some(TokenKind::OrOr), Value::Bool(left), Value::Bool(right)) => {
                Ok(Value::Bool(left || right))
            }
            _ => self.fail(
                node.span,
                "invalid binary operation during compile-time execution",
            ),
        }
    }

    fn evaluate_call(&mut self, node: &SyntaxNode) -> Result<Value, ExecutionFailure> {
        let expressions = expression_children(node);
        let callee = expressions
            .first()
            .copied()
            .ok_or_else(|| self.failure(node.span, "call has no callee"))?;
        let mut arguments = Vec::new();
        for argument in expressions.iter().skip(1) {
            arguments.push(self.evaluate(argument, None)?);
        }
        if let Some(path) = expression_path(callee)
            && path.starts_with("std.ast.")
        {
            return self.call_intrinsic(&path, arguments, node.span);
        }
        if callee.kind == SyntaxKind::MemberExpression {
            let receiver_node = expression_children(callee)[0];
            let receiver = self.evaluate(receiver_node, None)?;
            let name = last_identifier(callee)
                .ok_or_else(|| self.failure(callee.span, "method has no name"))?;
            return self.call_method(receiver, &name, arguments, node.span);
        }
        self.fail(node.span, "compile-time calls are limited to deterministic `std.ast` intrinsics and value methods")
    }

    fn call_intrinsic(
        &mut self,
        path: &str,
        arguments: Vec<Value>,
        span: Span,
    ) -> Result<Value, ExecutionFailure> {
        match path {
            "std.ast.StatementList.empty" if arguments.is_empty() => {
                Ok(Value::syntax_nodes(QuoteRole::StatementList, Vec::new()))
            }
            "std.ast.MemberList.empty" if arguments.is_empty() => {
                Ok(Value::syntax_nodes(QuoteRole::MemberList, Vec::new()))
            }
            "std.ast.ItemList.empty" if arguments.is_empty() => {
                Ok(Value::syntax_nodes(QuoteRole::ItemList, Vec::new()))
            }
            "std.ast.literal" if arguments.len() == 1 => {
                literal_syntax(arguments.into_iter().next().unwrap(), span)
            }
            "std.ast.identifier" if arguments.len() == 1 => {
                match arguments.into_iter().next().unwrap() {
                    Value::String(text) if valid_identifier(&text) => Ok(Value::Identifier(text)),
                    Value::String(_) => {
                        self.fail(span, "`std.ast.identifier` received an invalid identifier")
                    }
                    _ => self.fail(span, "`std.ast.identifier` requires a string"),
                }
            }
            "std.ast.error" if arguments.len() == 2 => {
                let message = match arguments.get(1) {
                    Some(Value::String(message)) => message.clone(),
                    _ => "compile-time AST error".to_string(),
                };
                self.fail(span, message)
            }
            _ => self.fail(
                span,
                format!("compile-time call `{path}` is not an admitted intrinsic"),
            ),
        }
    }

    fn call_method(
        &mut self,
        receiver: Value,
        name: &str,
        arguments: Vec<Value>,
        span: Span,
    ) -> Result<Value, ExecutionFailure> {
        match (receiver, name, arguments.as_slice()) {
            (Value::String(value), "length", []) => {
                Ok(Value::Integer(value.chars().count() as i128))
            }
            (Value::Sequence(values), "length", []) => Ok(Value::Integer(values.len() as i128)),
            (Value::Syntax { nodes, .. }, "length", []) => Ok(Value::Integer(nodes.len() as i128)),
            (
                Value::Syntax { role, mut nodes },
                "push",
                [
                    Value::Syntax {
                        role: element_role,
                        nodes: element,
                    },
                ],
            ) if role_accepts_element(role, *element_role) && element.len() == 1 => {
                nodes.extend(element.clone());
                Ok(Value::Syntax { role, nodes })
            }
            (Value::Syntax { nodes, .. }, "display", []) => Ok(Value::String(
                nodes
                    .iter()
                    .map(display_syntax)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )),
            (Value::CallView(call), "callee", []) => {
                let callee = expression_children(&call)
                    .first()
                    .copied()
                    .ok_or_else(|| self.failure(span, "call AST has no callee"))?;
                Ok(Value::syntax(QuoteRole::Expression, callee.clone()))
            }
            (Value::CallView(call), "arguments", []) => Ok(Value::Sequence(
                expression_children(&call)
                    .into_iter()
                    .skip(1)
                    .cloned()
                    .map(|node| Value::syntax(QuoteRole::Expression, node))
                    .collect(),
            )),
            (Value::MemberView(member), "receiver", []) => {
                let receiver = expression_children(&member)
                    .first()
                    .copied()
                    .ok_or_else(|| self.failure(span, "member AST has no receiver"))?;
                Ok(Value::syntax(QuoteRole::Expression, receiver.clone()))
            }
            (Value::MemberView(member), "name", []) => last_identifier(&member)
                .map(Value::Identifier)
                .ok_or_else(|| self.failure(span, "member AST has no name")),
            (
                Value::Syntax {
                    role: QuoteRole::StructDefinition | QuoteRole::EnumDefinition,
                    nodes,
                },
                "members",
                [],
            ) => Ok(Value::syntax_nodes(
                QuoteRole::MemberList,
                definition_members(nodes.first().expect("scalar definition")),
            )),
            (
                Value::Syntax {
                    role: QuoteRole::StructDefinition,
                    nodes,
                },
                "fields",
                [],
            ) => Ok(Value::syntax_nodes(
                QuoteRole::MemberList,
                definition_members(nodes.first().expect("scalar definition"))
                    .into_iter()
                    .filter(|node| node.kind == SyntaxKind::Field)
                    .collect(),
            )),
            (
                Value::Syntax {
                    role: QuoteRole::StructDefinition | QuoteRole::EnumDefinition,
                    nodes,
                },
                "type_syntax",
                [],
            ) => definition_type_syntax(nodes.first().expect("scalar definition")),
            (Value::Syntax { role, nodes }, "with_name", [Value::Identifier(name)])
                if matches!(
                    role,
                    QuoteRole::StructDefinition
                        | QuoteRole::EnumDefinition
                        | QuoteRole::FunctionDefinition
                ) =>
            {
                Ok(Value::syntax(
                    role,
                    definition_with_name(nodes.first().expect("scalar definition"), name)?,
                ))
            }
            (
                Value::Syntax { role, nodes },
                "with_members",
                [
                    Value::Syntax {
                        role: QuoteRole::MemberList,
                        nodes: members,
                    },
                ],
            ) if role == QuoteRole::StructDefinition => {
                if members
                    .iter()
                    .any(|member| member.kind != SyntaxKind::Field)
                {
                    return self.fail(
                        span,
                        "`StructDefinition` accepts fields only; emit methods in a sibling \
                         `InherentImplementation` item",
                    );
                }
                Ok(Value::syntax(
                    role,
                    definition_with_members(
                        nodes.first().expect("scalar definition"),
                        members.clone(),
                    ),
                ))
            }
            (
                Value::Syntax { role, nodes },
                "with_members",
                [
                    Value::Syntax {
                        role: QuoteRole::MemberList,
                        nodes: members,
                    },
                ],
            ) if role == QuoteRole::EnumDefinition => Ok(Value::syntax(
                role,
                definition_with_members(nodes.first().expect("scalar definition"), members.clone()),
            )),
            _ => self.fail(
                span,
                format!("method `{name}` is not available for this compile-time value"),
            ),
        }
    }

    fn evaluate_member(&mut self, node: &SyntaxNode) -> Result<Value, ExecutionFailure> {
        let receiver = expression_children(node)
            .first()
            .copied()
            .ok_or_else(|| self.failure(node.span, "member has no receiver"))?;
        let receiver = self.evaluate(receiver, None)?;
        let name =
            last_identifier(node).ok_or_else(|| self.failure(node.span, "member has no name"))?;
        self.call_method(receiver, &name, Vec::new(), node.span)
    }

    fn evaluate_quote(
        &mut self,
        node: &SyntaxNode,
        role: QuoteRole,
    ) -> Result<Value, ExecutionFailure> {
        let body = node
            .direct_child(SyntaxKind::QuoteBody)
            .ok_or_else(|| self.failure(node.span, "quote has no body"))?;
        let mut tokens = Vec::new();
        let last = body.children.len().saturating_sub(1);
        for (index, child) in body.children.iter().enumerate() {
            match child {
                SyntaxElement::Token(token)
                    if (index == 0 && token.kind == TokenKind::Indent)
                        || (index == last && token.kind == TokenKind::Dedent) => {}
                SyntaxElement::Token(token) => tokens.push(token.clone()),
                SyntaxElement::Node(interpolation)
                    if interpolation.kind == SyntaxKind::QuoteInterpolation =>
                {
                    let value = if let Some(name) = first_identifier(interpolation) {
                        self.lookup(&name).cloned().ok_or_else(|| {
                            self.failure(
                                interpolation.span,
                                format!("interpolation `{name}` is not defined"),
                            )
                        })?
                    } else {
                        let expression = expression_children(interpolation)
                            .first()
                            .copied()
                            .ok_or_else(|| {
                                self.failure(
                                    interpolation.span,
                                    "computed interpolation has no expression",
                                )
                            })?;
                        self.evaluate(expression, None)?
                    };
                    append_interpolation_tokens(&value, &mut tokens, interpolation.span)?;
                }
                SyntaxElement::Node(_) => {
                    return self.fail(child.span(), "invalid node in quote template");
                }
            }
        }
        let boundary = tokens.last().map_or(body.span, |token| token.span);
        tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::new(boundary.file, boundary.end, boundary.end),
        });
        let output = parse_quote_fragment(&tokens, quote_fragment(role));
        if let Some(diagnostic) = output.diagnostics.first() {
            return self.fail(
                diagnostic.primary.unwrap_or(node.span),
                format!("generated quote is invalid: {}", diagnostic.message),
            );
        }
        Ok(Value::syntax_nodes(role, quote_nodes(output.tree, role)))
    }

    fn store(&mut self, name: String, value: Value) -> Result<(), ExecutionFailure> {
        self.allocate(&value, self.declaration.span)?;
        self.scopes
            .last_mut()
            .expect("scope exists")
            .insert(name, value);
        Ok(())
    }

    fn assign(&mut self, name: &str, value: Value) -> Result<(), ExecutionFailure> {
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_string(), value);
                return Ok(());
            }
        }
        self.fail(
            self.declaration.span,
            format!("cannot assign undefined compile-time local `{name}`"),
        )
    }

    fn lookup(&self, name: &str) -> Option<&Value> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    fn step(&mut self, span: Span) -> Result<(), ExecutionFailure> {
        self.resources
            .charge_steps(1)
            .map_err(|limit| self.limit_failure(span, limit))
    }

    fn allocate(&mut self, value: &Value, span: Span) -> Result<(), ExecutionFailure> {
        self.resources
            .allocate_live_value_bytes(value.live_bytes())
            .map_err(|limit| self.limit_failure(span, limit))
    }

    fn limit_failure(&self, span: Span, limit: ResourceLimitKind) -> ExecutionFailure {
        ExecutionFailure {
            message: format!("compile-time execution exceeded the deterministic {limit:?} limit"),
            span,
            limit: Some(limit),
        }
    }

    fn failure(&self, span: Span, message: impl Into<String>) -> ExecutionFailure {
        ExecutionFailure {
            message: message.into(),
            span,
            limit: None,
        }
    }

    fn fail<T>(&self, span: Span, message: impl Into<String>) -> Result<T, ExecutionFailure> {
        Err(self.failure(span, message))
    }
}

trait ValueBool {
    fn into_bool(self, span: Span) -> Result<bool, ExecutionFailure>;
}

impl ValueBool for Value {
    fn into_bool(self, span: Span) -> Result<bool, ExecutionFailure> {
        match self {
            Self::Bool(value) => Ok(value),
            _ => Err(ExecutionFailure {
                message: "compile-time condition must be bool".to_string(),
                span,
                limit: None,
            }),
        }
    }
}

fn evaluate_literal(node: &SyntaxNode) -> Result<Value, ExecutionFailure> {
    let token = node
        .direct_tokens()
        .first()
        .copied()
        .ok_or_else(|| ExecutionFailure {
            message: "literal has no token".to_string(),
            span: node.span,
            limit: None,
        })?;
    Ok(match &token.kind {
        TokenKind::StringLiteral(value) | TokenKind::CharacterLiteral(value) => {
            Value::String(value.clone())
        }
        TokenKind::Keyword(Keyword::True) => Value::Bool(true),
        TokenKind::Keyword(Keyword::False) => Value::Bool(false),
        TokenKind::IntegerLiteral { raw, radix, .. } => {
            let digits = raw
                .trim_start_matches(|character: char| {
                    character == '0' || matches!(character, 'x' | 'X' | 'o' | 'O' | 'b' | 'B')
                })
                .replace('_', "");
            Value::Integer(
                i128::from_str_radix(
                    if digits.is_empty() { "0" } else { &digits },
                    u32::from(*radix),
                )
                .map_err(|_| ExecutionFailure {
                    message: "integer is outside the compile-time range".to_string(),
                    span: token.span,
                    limit: None,
                })?,
            )
        }
        _ => {
            return Err(ExecutionFailure {
                message: "literal type is unavailable at compile time".to_string(),
                span: token.span,
                limit: None,
            });
        }
    })
}

fn literal_syntax(value: Value, span: Span) -> Result<Value, ExecutionFailure> {
    let kind = match value {
        Value::String(value) | Value::Identifier(value) => TokenKind::StringLiteral(value),
        Value::Bool(true) => TokenKind::Keyword(Keyword::True),
        Value::Bool(false) => TokenKind::Keyword(Keyword::False),
        Value::Integer(value) => TokenKind::IntegerLiteral {
            raw: value.to_string(),
            radix: 10,
            suffix: None,
        },
        _ => {
            return Err(ExecutionFailure {
                message: "`std.ast.literal` requires a string, bool, or integer".to_string(),
                span,
                limit: None,
            });
        }
    };
    Ok(Value::syntax(
        QuoteRole::Expression,
        SyntaxNode::new(
            SyntaxKind::LiteralExpression,
            vec![SyntaxElement::Token(Token { kind, span })],
            span,
        ),
    ))
}

fn type_text(node: &SyntaxNode) -> String {
    let mut output = String::new();
    for token in all_tokens(node) {
        output.push_str(&token_text(&token.kind));
    }
    output
}

fn expression_children(node: &SyntaxNode) -> Vec<&SyntaxNode> {
    node.direct_nodes()
        .into_iter()
        .filter(|child| {
            !matches!(
                child.kind,
                SyntaxKind::Type | SyntaxKind::Block | SyntaxKind::ElseClause | SyntaxKind::Guard
            )
        })
        .collect()
}

fn expression_path(node: &SyntaxNode) -> Option<String> {
    match node.kind {
        SyntaxKind::NameExpression => first_identifier(node),
        SyntaxKind::MemberExpression => {
            let receiver = expression_path(expression_children(node).first().copied()?)?;
            Some(format!("{receiver}.{}", last_identifier(node)?))
        }
        _ => None,
    }
}

fn first_identifier(node: &SyntaxNode) -> Option<String> {
    all_tokens(node)
        .into_iter()
        .find_map(|token| match &token.kind {
            TokenKind::Identifier(name) => Some(name.clone()),
            _ => None,
        })
}

fn last_identifier(node: &SyntaxNode) -> Option<String> {
    node.direct_tokens()
        .into_iter()
        .rev()
        .find_map(|token| match &token.kind {
            TokenKind::Identifier(name) => Some(name.clone()),
            _ => None,
        })
}

fn first_identifier_after(node: &SyntaxNode, keyword: Keyword) -> Option<String> {
    let mut seen = false;
    node.direct_tokens()
        .into_iter()
        .find_map(|token| match &token.kind {
            TokenKind::Keyword(found) if *found == keyword => {
                seen = true;
                None
            }
            TokenKind::Identifier(name) if seen => Some(name.clone()),
            _ => None,
        })
}

fn value_matches_type(value: &Value, ty: &CompileTimeType) -> bool {
    match (value, ty) {
        (Value::Unit, CompileTimeType::Unit)
        | (Value::Bool(_), CompileTimeType::Bool)
        | (Value::Integer(_), CompileTimeType::Integer)
        | (Value::String(_), CompileTimeType::String) => true,
        (Value::Syntax { role, .. }, CompileTimeType::Ast(expected)) => {
            role == expected
                || (*expected == QuoteRole::Item
                    && matches!(
                        role,
                        QuoteRole::StructDefinition
                            | QuoteRole::EnumDefinition
                            | QuoteRole::FunctionDefinition
                            | QuoteRole::Implementation
                            | QuoteRole::InherentImplementation
                    ))
        }
        (Value::Sequence(values), CompileTimeType::Sequence(element)) => values
            .iter()
            .all(|value| value_matches_type(value, element)),
        _ => false,
    }
}

fn values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Unit, Value::Unit) => true,
        (Value::Bool(left), Value::Bool(right)) => left == right,
        (Value::Integer(left), Value::Integer(right)) => left == right,
        (Value::String(left), Value::String(right))
        | (Value::Identifier(left), Value::Identifier(right)) => left == right,
        _ => false,
    }
}

fn role_is_list(role: QuoteRole) -> bool {
    matches!(
        role,
        QuoteRole::StatementList | QuoteRole::MemberList | QuoteRole::ItemList
    )
}

fn role_accepts_element(list: QuoteRole, element: QuoteRole) -> bool {
    matches!(
        (list, element),
        (QuoteRole::StatementList, _)
            | (
                QuoteRole::MemberList,
                QuoteRole::FieldDefinition | QuoteRole::FunctionDefinition
            )
            | (
                QuoteRole::ItemList,
                QuoteRole::Item
                    | QuoteRole::StructDefinition
                    | QuoteRole::EnumDefinition
                    | QuoteRole::FunctionDefinition
                    | QuoteRole::Implementation
                    | QuoteRole::InherentImplementation
            )
    )
}

fn element_role(list: QuoteRole, node: &SyntaxNode) -> QuoteRole {
    match list {
        QuoteRole::ItemList => match node.kind {
            SyntaxKind::Struct => QuoteRole::StructDefinition,
            SyntaxKind::Enum => QuoteRole::EnumDefinition,
            SyntaxKind::Function => QuoteRole::FunctionDefinition,
            SyntaxKind::Impl
                if node
                    .direct_tokens()
                    .into_iter()
                    .any(|token| matches!(token.kind, TokenKind::Keyword(Keyword::For))) =>
            {
                QuoteRole::Implementation
            }
            SyntaxKind::Impl => QuoteRole::InherentImplementation,
            _ => QuoteRole::Item,
        },
        QuoteRole::MemberList => match node.kind {
            SyntaxKind::Field => QuoteRole::FieldDefinition,
            SyntaxKind::Function => QuoteRole::FunctionDefinition,
            _ => QuoteRole::MemberList,
        },
        _ => QuoteRole::StatementList,
    }
}

fn display_value(value: &Value) -> String {
    match value {
        Value::Unit => "()".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Integer(value) => value.to_string(),
        Value::String(value) | Value::Identifier(value) => value.clone(),
        Value::Syntax { nodes, .. } => nodes
            .iter()
            .map(display_syntax)
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Sequence(values) => values
            .iter()
            .map(display_value)
            .collect::<Vec<_>>()
            .join(", "),
        Value::CallView(node) | Value::MemberView(node) => display_syntax(node),
    }
}

fn quote_fragment(role: QuoteRole) -> QuoteFragmentKind {
    match role {
        QuoteRole::Expression => QuoteFragmentKind::Expression,
        QuoteRole::Pattern => QuoteFragmentKind::Pattern,
        QuoteRole::TypeSyntax => QuoteFragmentKind::Type,
        QuoteRole::StatementList => QuoteFragmentKind::StatementList,
        QuoteRole::MemberList => QuoteFragmentKind::MemberList,
        QuoteRole::FieldDefinition => QuoteFragmentKind::Member,
        QuoteRole::Item
        | QuoteRole::StructDefinition
        | QuoteRole::EnumDefinition
        | QuoteRole::FunctionDefinition
        | QuoteRole::Implementation
        | QuoteRole::InherentImplementation => QuoteFragmentKind::Item,
        QuoteRole::ItemList => QuoteFragmentKind::ItemList,
    }
}

fn quote_nodes(tree: SyntaxNode, role: QuoteRole) -> Vec<SyntaxNode> {
    if role_is_list(role) {
        tree.direct_nodes().into_iter().cloned().collect()
    } else {
        vec![tree]
    }
}

fn append_interpolation_tokens(
    value: &Value,
    output: &mut Vec<Token>,
    span: Span,
) -> Result<(), ExecutionFailure> {
    match value {
        Value::Syntax { nodes, .. } => {
            for node in nodes {
                output.extend(
                    all_tokens(node)
                        .into_iter()
                        .filter(|token| token.kind != TokenKind::Eof)
                        .cloned(),
                );
            }
            Ok(())
        }
        Value::Sequence(values) => {
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(Token {
                        kind: TokenKind::Comma,
                        span,
                    });
                }
                append_interpolation_tokens(value, output, span)?;
            }
            Ok(())
        }
        Value::Identifier(name) => {
            output.push(Token {
                kind: TokenKind::Identifier(name.clone()),
                span,
            });
            Ok(())
        }
        _ => Err(ExecutionFailure {
            message: "only AST values and identifiers can be interpolated into a quote".to_string(),
            span,
            limit: None,
        }),
    }
}

fn pattern_matches(
    pattern: &SyntaxNode,
    value: &Value,
    bindings: &mut BTreeMap<String, Value>,
) -> bool {
    if pattern.kind == SyntaxKind::Pattern && pattern.direct_nodes().len() == 1 {
        return pattern_matches(pattern.direct_nodes()[0], value, bindings);
    }
    if pattern.kind == SyntaxKind::AlternativePattern {
        return pattern
            .direct_nodes()
            .into_iter()
            .any(|pattern| pattern_matches(pattern, value, bindings));
    }
    if pattern.kind == SyntaxKind::VariantPattern {
        let Some(variant) = last_identifier(pattern) else {
            return false;
        };
        let matches = match (variant.as_str(), value) {
            (
                "Call",
                Value::Syntax {
                    role: QuoteRole::Expression,
                    nodes,
                },
            ) if nodes
                .first()
                .is_some_and(|node| node.kind == SyntaxKind::CallExpression) =>
            {
                Some(Value::CallView(nodes[0].clone()))
            }
            (
                "Identifier",
                Value::Syntax {
                    role: QuoteRole::Expression,
                    nodes,
                },
            ) if nodes
                .first()
                .is_some_and(|node| node.kind == SyntaxKind::NameExpression) =>
            {
                Some(value.clone())
            }
            (
                "Literal",
                Value::Syntax {
                    role: QuoteRole::Expression,
                    nodes,
                },
            ) if nodes
                .first()
                .is_some_and(|node| node.kind == SyntaxKind::LiteralExpression) =>
            {
                Some(value.clone())
            }
            (
                "Member",
                Value::Syntax {
                    role: QuoteRole::Expression,
                    nodes,
                },
            ) if nodes
                .first()
                .is_some_and(|node| node.kind == SyntaxKind::MemberExpression) =>
            {
                Some(Value::MemberView(nodes[0].clone()))
            }
            (
                "Tuple",
                Value::Syntax {
                    role: QuoteRole::Expression,
                    nodes,
                },
            ) if nodes
                .first()
                .is_some_and(|node| node.kind == SyntaxKind::TupleExpression) =>
            {
                Some(Value::Sequence(
                    expression_children(&nodes[0])
                        .into_iter()
                        .cloned()
                        .map(|node| Value::syntax(QuoteRole::Expression, node))
                        .collect(),
                ))
            }
            _ => None,
        };
        let Some(payload) = matches else {
            return false;
        };
        return pattern
            .direct_nodes()
            .first()
            .is_none_or(|nested| pattern_matches(nested, &payload, bindings));
    }
    let Some(name) = first_identifier(pattern) else {
        return false;
    };
    if name == "_" {
        true
    } else {
        bindings.insert(name, value.clone());
        true
    }
}

fn definition_members(node: &SyntaxNode) -> Vec<SyntaxNode> {
    node.direct_child(SyntaxKind::Block)
        .into_iter()
        .flat_map(SyntaxNode::direct_nodes)
        .filter(|member| matches!(member.kind, SyntaxKind::Field | SyntaxKind::Function))
        .cloned()
        .collect()
}

fn definition_type_syntax(node: &SyntaxNode) -> Result<Value, ExecutionFailure> {
    let name = declaration_name_token(node)
        .ok_or_else(|| ExecutionFailure {
            message: "definition has no name".to_string(),
            span: node.span,
            limit: None,
        })?
        .clone();
    Ok(Value::syntax(
        QuoteRole::TypeSyntax,
        SyntaxNode::new(
            SyntaxKind::Type,
            vec![SyntaxElement::Token(name)],
            node.span,
        ),
    ))
}

fn definition_with_name(node: &SyntaxNode, name: &str) -> Result<SyntaxNode, ExecutionFailure> {
    if !valid_identifier(name) {
        return Err(ExecutionFailure {
            message: "definition name is not a valid identifier".to_string(),
            span: node.span,
            limit: None,
        });
    }
    let mut replacement = node.clone();
    let mut after_keyword = false;
    for child in &mut replacement.children {
        if let SyntaxElement::Token(token) = child {
            if matches!(
                token.kind,
                TokenKind::Keyword(Keyword::Struct | Keyword::Enum | Keyword::Fn)
            ) {
                after_keyword = true;
            } else if after_keyword && matches!(token.kind, TokenKind::Identifier(_)) {
                token.kind = TokenKind::Identifier(name.to_string());
                return Ok(replacement);
            }
        }
    }
    Err(ExecutionFailure {
        message: "definition has no replaceable name".to_string(),
        span: node.span,
        limit: None,
    })
}

fn definition_with_members(node: &SyntaxNode, members: Vec<SyntaxNode>) -> SyntaxNode {
    let mut replacement = node.clone();
    for child in &mut replacement.children {
        let SyntaxElement::Node(block) = child else {
            continue;
        };
        if block.kind != SyntaxKind::Block {
            continue;
        }
        let mut children = Vec::new();
        if let Some(indent) = block.children.iter().find(|child| {
            matches!(
                child,
                SyntaxElement::Token(Token {
                    kind: TokenKind::Indent,
                    ..
                })
            )
        }) {
            children.push(indent.clone());
        }
        children.extend(
            members
                .into_iter()
                .map(|member| SyntaxElement::Node(Box::new(member))),
        );
        if let Some(dedent) = block.children.iter().rev().find(|child| {
            matches!(
                child,
                SyntaxElement::Token(Token {
                    kind: TokenKind::Dedent,
                    ..
                })
            )
        }) {
            children.push(dedent.clone());
        }
        **block = SyntaxNode::new(SyntaxKind::Block, children, block.span);
        break;
    }
    replacement
}

fn declaration_name_token(node: &SyntaxNode) -> Option<&Token> {
    let mut after_keyword = false;
    node.direct_tokens().into_iter().find(|token| {
        if matches!(
            token.kind,
            TokenKind::Keyword(Keyword::Struct | Keyword::Enum | Keyword::Fn)
        ) {
            after_keyword = true;
            false
        } else {
            after_keyword && matches!(token.kind, TokenKind::Identifier(_))
        }
    })
}

fn display_syntax(node: &SyntaxNode) -> String {
    all_tokens(node)
        .into_iter()
        .map(|token| token_text(&token.kind))
        .collect::<Vec<_>>()
        .join(" ")
}

fn all_tokens(node: &SyntaxNode) -> Vec<&Token> {
    fn walk<'a>(node: &'a SyntaxNode, output: &mut Vec<&'a Token>) {
        for child in &node.children {
            match child {
                SyntaxElement::Token(token) => output.push(token),
                SyntaxElement::Node(node) => walk(node, output),
            }
        }
    }
    let mut output = Vec::new();
    walk(node, &mut output);
    output
}

fn syntax_bytes(node: &SyntaxNode) -> u64 {
    32 + node
        .children
        .iter()
        .map(|child| match child {
            SyntaxElement::Token(token) => 24 + token_text(&token.kind).len() as u64,
            SyntaxElement::Node(node) => syntax_bytes(node),
        })
        .sum::<u64>()
}

fn valid_identifier(text: &str) -> bool {
    let mut characters = text.chars();
    characters
        .next()
        .is_some_and(|first| first == '_' || first.is_alphabetic())
        && characters.all(|character| character == '_' || character.is_alphanumeric())
}

fn token_text(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Identifier(value) => value.clone(),
        TokenKind::Keyword(keyword) => format!("{keyword:?}").to_ascii_lowercase(),
        TokenKind::IntegerLiteral { raw, .. } | TokenKind::FloatLiteral { raw, .. } => raw.clone(),
        TokenKind::StringLiteral(value) => format!("\"{value}\""),
        TokenKind::CharacterLiteral(value) => format!("'{value}'"),
        TokenKind::Newline => "\n".to_string(),
        TokenKind::Indent => "<indent>".to_string(),
        TokenKind::Dedent => "<dedent>".to_string(),
        TokenKind::Eof => String::new(),
        TokenKind::LParen => "(".to_string(),
        TokenKind::RParen => ")".to_string(),
        TokenKind::LBracket => "[".to_string(),
        TokenKind::RBracket => "]".to_string(),
        TokenKind::LBrace => "{".to_string(),
        TokenKind::RBrace => "}".to_string(),
        TokenKind::Comma => ",".to_string(),
        TokenKind::Semicolon => ";".to_string(),
        TokenKind::Colon => ":".to_string(),
        TokenKind::Dot => ".".to_string(),
        TokenKind::DotDot => "..".to_string(),
        TokenKind::Ellipsis => "...".to_string(),
        TokenKind::At => "@".to_string(),
        TokenKind::Dollar => "$".to_string(),
        TokenKind::Question => "?".to_string(),
        TokenKind::Arrow => "->".to_string(),
        TokenKind::Assign => "=".to_string(),
        TokenKind::Plus => "+".to_string(),
        TokenKind::PlusPlus => "++".to_string(),
        TokenKind::Minus => "-".to_string(),
        TokenKind::Star => "*".to_string(),
        TokenKind::Slash => "/".to_string(),
        TokenKind::Percent => "%".to_string(),
        TokenKind::Amp => "&".to_string(),
        TokenKind::Pipe => "|".to_string(),
        TokenKind::Caret => "^".to_string(),
        TokenKind::Tilde => "~".to_string(),
        TokenKind::Bang => "!".to_string(),
        TokenKind::Shl => "<<".to_string(),
        TokenKind::Shr => ">>".to_string(),
        TokenKind::EqEq => "==".to_string(),
        TokenKind::NotEq => "!=".to_string(),
        TokenKind::Less => "<".to_string(),
        TokenKind::LessEq => "<=".to_string(),
        TokenKind::Greater => ">".to_string(),
        TokenKind::GreaterEq => ">=".to_string(),
        TokenKind::AndAnd => "&&".to_string(),
        TokenKind::OrOr => "||".to_string(),
        TokenKind::PlusAssign => "+=".to_string(),
        TokenKind::MinusAssign => "-=".to_string(),
        TokenKind::StarAssign => "*=".to_string(),
        TokenKind::SlashAssign => "/=".to_string(),
        TokenKind::PercentAssign => "%=".to_string(),
        TokenKind::AmpAssign => "&=".to_string(),
        TokenKind::PipeAssign => "|=".to_string(),
        TokenKind::CaretAssign => "^=".to_string(),
        TokenKind::ShlAssign => "<<=".to_string(),
        TokenKind::ShrAssign => ">>=".to_string(),
        TokenKind::FormattedString(_) | TokenKind::DocComment(_) => "<text>".to_string(),
    }
}
