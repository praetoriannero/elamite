//! Core expression, function, method, and control-flow checker (`IMPL.md`
//! Milestones 6, 7, 11, and 12).
//!
//! This module type-checks the pre-trait subset of the language: module-level
//! functions, generic and non-generic inherent methods, function references,
//! and ordinary value operations, control flow, and patterns in their bodies. It consumes the
//! stable identities from [`crate::resolution`] and the canonical types from
//! [`crate::types`] and adds: function parameter/return/arity checking for
//! direct named calls, place classification (value / addressable / mutable /
//! collection interior) for every checked expression, `let`/`var`/assignment
//! checking, struct/tuple/array/enum construction, primitive operator and
//! cast checking, rejection of struct/enum containment cycles that do not
//! cross an explicit indirection (Milestone 6); and `break`/`continue`
//! placement, reachable-path return analysis, pattern type-checking and
//! binding, and match exhaustiveness/reachability (Milestone 7).
//!
//! Milestones 6 and 7 are implemented together in one pass rather than as
//! separate modules: `IMPL.md` assigns pattern typing to Milestone 7, but a
//! match arm's body (Milestone 6 territory) needs its pattern bindings typed
//! at the exact point Milestone 6 already visits it, so splitting the two
//! into a second full tree-walk would either duplicate this module's
//! expression/statement checking or leave pattern-bound names permanently
//! untyped in arm bodies. This is a documented merge of two tightly-coupled
//! milestones' implementation, not a scope reduction of either.
//!
//! Several rules that the ledger assigns to later milestones are deliberately
//! left unchecked here rather than approximated unsoundly: trait-bound method
//! dispatch and full concrete `PartialEq`/`PartialOrd`/`Display` selection
//! (Milestone 13),
//! `for`-binding element typing (Milestone 14), and `?`/`Close`/`defer`
//! propagation semantics (Milestone 15). Expressions that fall into those
//! areas are still walked (so nested diagnostics and copies are not lost)
//! but resolve to the type-system error type without an additional
//! diagnostic of their own. Left-to-right evaluation order and temporary
//! lifetimes (also Milestone 7) are satisfied by this module's single-pass,
//! source-order recursive-descent structure; materializing them as explicit
//! IR metadata is Milestone 8's job, since no IR exists yet.

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostics::{Category, Diagnostic};
use crate::lexer::{Keyword, Token, TokenKind};
use crate::parser::{SyntaxElement, SyntaxKind, SyntaxNode};
use crate::resolution::{
    DeclarationId, DeclarationKind, FieldId, GenericParameterId, LocalBindingId, MemberId,
    NameTarget, ResolvedProgram, VariantId,
};
use crate::source::Span;
use crate::types::{
    self, Abi, ExpectedType, FunctionInstance, FunctionParameter, Mutability, PlaceKind,
    PrimitiveType, Safety, Substitution, TypeId, TypeKind, TypedProgram,
};

/// Whether a checked local binding's root storage is a rebindable, mutable
/// place (`var`) or a non-rebindable, non-mutable place (`let`, and function
/// parameters, which the grammar never allows `var` on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rebindable {
    Let,
    Var,
}

/// Milestones 6-7 output: per-expression classification and copied pattern
/// bindings consumed by later lowering passes. Expression facts are keyed by
/// source span since the syntax tree has no per-node stable identity.
#[derive(Debug, Default)]
pub struct CheckedProgram {
    pub expression_types: BTreeMap<Span, TypeId>,
    pub expression_places: BTreeMap<Span, PlaceKind>,
    /// Spans of expressions that produce an explicit logical-value copy:
    /// binding initializers, assignment sources, return values, call
    /// arguments, and aggregate-literal field/element values.
    pub copies: BTreeSet<Span>,
    /// Pattern bindings receive independent logical-value copies and behave
    /// as immutable `let` bindings. Match lowering consumes these stable
    /// binding identities when it materializes payload extraction.
    pub copied_pattern_bindings: BTreeSet<LocalBindingId>,
    /// Canonical payload type selected for each copied pattern binding.
    pub pattern_binding_types: BTreeMap<LocalBindingId, TypeId>,
    /// Named functions and type-selected methods used as first-class function
    /// references. The key is the complete expression span, not merely its
    /// final identifier, so lowering can distinguish a type selection from a
    /// bound member expression.
    pub function_references: BTreeMap<Span, FunctionInstance>,
    /// Callable resolution selected for each call expression.
    pub calls: BTreeMap<Span, CheckedCall>,
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
    Print {
        newline: bool,
    },
}

/// The only implicit adaptation permitted by Elamite: adaptation of a bound
/// method receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiverAdjustment {
    /// Pass the receiver expression unchanged.
    Pass,
    /// Copy an ordinary value receiver.
    CopyValue,
    /// Dereference a safe reference and copy its target for `self: Self`.
    DereferenceAndCopy,
    /// Form `&receiver` for `self: &Self`.
    BorrowShared,
    /// Form `&var receiver` for `self: &var Self`.
    BorrowMutable,
}

pub struct CheckOutput {
    pub program: CheckedProgram,
    pub diagnostics: Vec<Diagnostic>,
}

/// Checks every module-level function and inherent method body once,
/// and rejects struct/enum containment cycles across the whole program.
/// `typed` is extended in place with any composite types that first appear
/// inside an expression rather than a declared signature.
#[must_use]
pub fn check(resolved: &ResolvedProgram, typed: &mut TypedProgram) -> CheckOutput {
    check_for_target(resolved, typed, 64)
}

/// Checks with the selected target's pointer width for contextual
/// `isize`/`usize` literal materialization.
#[must_use]
pub fn check_for_target(
    resolved: &ResolvedProgram,
    typed: &mut TypedProgram,
    pointer_bits: u8,
) -> CheckOutput {
    Checker::new(resolved, typed, pointer_bits).run()
}

struct Checker<'a> {
    resolved: &'a ResolvedProgram,
    typed: &'a mut TypedProgram,
    span_to_local: BTreeMap<Span, LocalBindingId>,
    local_types: BTreeMap<LocalBindingId, TypeId>,
    local_rebindable: BTreeMap<LocalBindingId, Rebindable>,
    loop_depth: u32,
    /// Nesting depth of `defer` statements being checked. A deferred body runs
    /// while its scope is already exiting, so it may not redirect control.
    defer_depth: u32,
    /// Nesting depth of `unsafe` blocks being checked.
    unsafe_depth: u32,
    pointer_bits: u8,
    current_self_declaration: Option<DeclarationId>,
    program: CheckedProgram,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Checker<'a> {
    fn new(resolved: &'a ResolvedProgram, typed: &'a mut TypedProgram, pointer_bits: u8) -> Self {
        let mut span_to_local = BTreeMap::new();
        for local in &resolved.local_bindings {
            span_to_local.insert(local.span, local.id);
        }
        Self {
            resolved,
            typed,
            span_to_local,
            local_types: BTreeMap::new(),
            local_rebindable: BTreeMap::new(),
            loop_depth: 0,
            defer_depth: 0,
            unsafe_depth: 0,
            pointer_bits,
            current_self_declaration: None,
            program: CheckedProgram::default(),
            diagnostics: Vec::new(),
        }
    }

    fn run(mut self) -> CheckOutput {
        let declarations: Vec<DeclarationId> = self
            .resolved
            .declarations
            .iter()
            .filter(|declaration| {
                declaration.kind == DeclarationKind::Function
                    && declaration.parent_impl.is_none()
                    && declaration.parent_declaration.is_none_or(|parent| {
                        self.resolved.declarations[parent.index()].kind != DeclarationKind::Trait
                    })
            })
            .map(|declaration| declaration.id)
            .collect();
        for declaration_id in declarations {
            self.check_function(declaration_id);
        }
        self.check_containment_cycles();
        CheckOutput {
            program: self.program,
            diagnostics: self.diagnostics,
        }
    }

    // ---------------------------------------------------------------
    // Functions and statements
    // ---------------------------------------------------------------

    fn check_function(&mut self, declaration_id: DeclarationId) {
        let Some(signature) = self.typed.function_signatures.get(&declaration_id).cloned() else {
            return;
        };
        let syntax = self.resolved.declarations[declaration_id.index()]
            .syntax
            .clone();
        self.current_self_declaration = self.resolved.declarations[declaration_id.index()]
            .parent_declaration
            .filter(|parent| {
                self.resolved.declarations[parent.index()].kind != DeclarationKind::Trait
            });
        self.local_types.clear();
        self.local_rebindable.clear();
        if let Some(parameters_node) = types::direct_child(&syntax, SyntaxKind::Parameters) {
            let parameter_nodes = types::direct_children(parameters_node, SyntaxKind::Parameter);
            let mut ordinary_parameters = signature.parameters.iter();
            for parameter_node in parameter_nodes {
                let is_receiver = types::direct_tokens(parameter_node)
                    .iter()
                    .any(|token| matches!(token.kind, TokenKind::Keyword(Keyword::SelfValue)));
                let parameter_type = if is_receiver {
                    signature.receiver
                } else {
                    ordinary_parameters.next().map(|parameter| {
                        if parameter.variadic {
                            self.typed.types.intern(TypeKind::Slice(parameter.ty))
                        } else {
                            parameter.ty
                        }
                    })
                };
                let Some(parameter_type) = parameter_type else {
                    continue;
                };
                if let Some(token) = parameter_name_token(parameter_node)
                    && let Some(&id) = self.span_to_local.get(&token.span)
                {
                    self.local_types.insert(id, parameter_type);
                    self.local_rebindable.insert(id, Rebindable::Let);
                }
            }
        }
        if let Some(block) = types::direct_child(&syntax, SyntaxKind::Block) {
            let definitely_returns = self.check_block(block, signature.return_type);
            let unit_type = self.typed.types.primitive(PrimitiveType::Unit);
            if !definitely_returns
                && !self
                    .typed
                    .types
                    .exactly_equal(signature.return_type, unit_type)
            {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ControlFlow,
                        "this non-unit function has a reachable path that does not return a value",
                    )
                    .with_primary(syntax.span),
                );
            }
        }
        self.current_self_declaration = None;
    }

    /// Checks every statement in `block` and returns whether the block
    /// definitely returns on every path reaching its end (Milestone 7
    /// reachable-path return analysis). Every statement is still checked
    /// regardless of an earlier statement already guaranteeing a return, so
    /// diagnostics are never skipped.
    fn check_block(&mut self, block: &SyntaxNode, return_type: TypeId) -> bool {
        let mut definitely_returns = false;
        for statement in child_nodes(block) {
            if self.check_statement(statement, return_type) {
                definitely_returns = true;
            }
        }
        definitely_returns
    }

    fn check_statement(&mut self, node: &SyntaxNode, return_type: TypeId) -> bool {
        match node.kind {
            SyntaxKind::LetStatement => {
                self.check_let_statement(node);
                false
            }
            SyntaxKind::AssignmentStatement => {
                self.check_assignment_statement(node);
                false
            }
            SyntaxKind::ExpressionStatement => {
                if let Some(expression) = child_nodes(node).into_iter().next() {
                    self.check_expr(expression, ExpectedType::None);
                }
                false
            }
            SyntaxKind::ReturnStatement => {
                self.check_return_statement(node, return_type);
                true
            }
            SyntaxKind::DeferStatement => {
                if self.defer_depth > 0 {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ControlFlow,
                            "a `defer` statement cannot appear inside a deferred block",
                        )
                        .with_primary(node.span),
                    );
                }
                if self.unsafe_depth > 0 {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ControlFlow,
                            "a `defer` statement cannot appear inside an `unsafe` block",
                        )
                        .with_primary(node.span),
                    );
                }
                self.defer_depth += 1;
                match child_nodes(node)
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)
                {
                    // `defer:` block form.
                    Some(block) => {
                        self.check_block(block, return_type);
                    }
                    // `defer call` single-call form.
                    None => {
                        if let Some(call) = child_nodes(node).into_iter().next() {
                            self.check_expr(call, ExpectedType::None);
                        }
                    }
                }
                self.defer_depth -= 1;
                false
            }
            SyntaxKind::IfStatement => self.check_if_statement(node, return_type),
            SyntaxKind::WhileStatement => {
                let children = child_nodes(node);
                if let Some(condition) = children.first() {
                    self.check_condition(condition);
                }
                if let Some(block) = children
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)
                {
                    self.loop_depth += 1;
                    self.check_block(block, return_type);
                    self.loop_depth -= 1;
                }
                // A `while` loop may execute zero times (even `while true`
                // is not specially recognized here), so it never by itself
                // guarantees a return.
                false
            }
            SyntaxKind::ForStatement => {
                let children = child_nodes(node);
                for child in &children {
                    if child.kind != SyntaxKind::Block {
                        self.check_expr(child, ExpectedType::None);
                    }
                }
                // The loop binding's element type depends on the iterable's
                // collection kind, which Milestone 14 lowers; its uses are
                // left untyped here rather than approximated.
                if let Some(block) = children
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)
                {
                    self.loop_depth += 1;
                    self.check_block(block, return_type);
                    self.loop_depth -= 1;
                }
                false
            }
            SyntaxKind::MatchStatement => self.check_match_statement(node, return_type),
            SyntaxKind::UnsafeBlock => {
                if self.defer_depth > 0 {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ControlFlow,
                            "an `unsafe` block cannot appear inside a deferred block",
                        )
                        .with_primary(node.span),
                    );
                }
                self.unsafe_depth += 1;
                let terminates = match child_nodes(node)
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)
                {
                    Some(block) => self.check_block(block, return_type),
                    None => false,
                };
                self.unsafe_depth -= 1;
                terminates
            }
            SyntaxKind::BreakStatement | SyntaxKind::ContinueStatement => {
                if self.defer_depth > 0 {
                    let what = if node.kind == SyntaxKind::BreakStatement {
                        "break"
                    } else {
                        "continue"
                    };
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ControlFlow,
                            format!("`{what}` cannot appear inside a deferred block"),
                        )
                        .with_primary(node.span),
                    );
                } else if self.loop_depth == 0 {
                    let what = if node.kind == SyntaxKind::BreakStatement {
                        "break"
                    } else {
                        "continue"
                    };
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ControlFlow,
                            format!("`{what}` is valid only inside a loop"),
                        )
                        .with_primary(node.span),
                    );
                }
                false
            }
            SyntaxKind::PassStatement => false,
            _ => {
                for child in child_nodes(node) {
                    self.check_expr(child, ExpectedType::None);
                }
                false
            }
        }
    }

    fn check_let_statement(&mut self, node: &SyntaxNode) {
        let is_var = matches!(
            node.children.first(),
            Some(SyntaxElement::Token(Token {
                kind: TokenKind::Keyword(Keyword::Var),
                ..
            }))
        );
        let nodes = child_nodes(node);
        let (annotation, initializer) = match nodes.as_slice() {
            [annotation, initializer] => (Some(*annotation), *initializer),
            [initializer] => (None, *initializer),
            _ => return,
        };
        let annotated_type = annotation.map(|node| self.lower_type(node));
        let expected = annotated_type.map_or(ExpectedType::None, ExpectedType::Exact);
        let (initializer_type, _) = self.check_expr(initializer, expected);
        self.program.copies.insert(initializer.span);
        let final_type = if let Some(annotated_type) = annotated_type {
            if !self.types_compatible(initializer_type, annotated_type) {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        "the initializer's type does not match the binding's declared type",
                    )
                    .with_primary(initializer.span),
                );
            }
            annotated_type
        } else {
            initializer_type
        };
        if let Some(token) = let_name_token(node)
            && let Some(&id) = self.span_to_local.get(&token.span)
        {
            self.local_types.insert(id, final_type);
            self.local_rebindable.insert(
                id,
                if is_var {
                    Rebindable::Var
                } else {
                    Rebindable::Let
                },
            );
        }
    }

    fn check_assignment_statement(&mut self, node: &SyntaxNode) {
        let nodes = child_nodes(node);
        let Some(&place_node) = nodes.first() else {
            return;
        };
        let Some(&value_node) = nodes.get(1) else {
            return;
        };
        let operator = node.children.iter().find_map(|child| match child {
            SyntaxElement::Token(token) if is_assignment_operator(&token.kind) => Some(token),
            _ => None,
        });
        let (place_type, place_kind) = self.check_expr(place_node, ExpectedType::None);
        if !place_kind.is_mutable() && place_type != self.typed.types.error() {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Place,
                    "cannot assign through a non-mutable place; only a `var` binding, a \
                     mutable reference target, or a mutable collection element is assignable",
                )
                .with_primary(place_node.span),
            );
        }
        let (value_type, _) = self.check_expr(value_node, ExpectedType::Exact(place_type));
        self.program.copies.insert(value_node.span);
        if !self.types_compatible(value_type, place_type) {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ExpressionType,
                    "the assigned value's type does not match the target place",
                )
                .with_primary(value_node.span),
            );
        }
        if let Some(operator) = operator
            && !matches!(operator.kind, TokenKind::Assign)
            && place_type != self.typed.types.error()
            && !self.compound_assignment_operand_ok(&operator.kind, place_type)
        {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ExpressionType,
                    "this compound-assignment operator does not accept the target's type",
                )
                .with_primary(node.span),
            );
        }
    }

    fn compound_assignment_operand_ok(&self, operator: &TokenKind, ty: TypeId) -> bool {
        match operator {
            TokenKind::PlusAssign
            | TokenKind::MinusAssign
            | TokenKind::StarAssign
            | TokenKind::SlashAssign => self.is_numeric_type(ty),
            TokenKind::PercentAssign => self.is_integer_type(ty),
            TokenKind::AmpAssign
            | TokenKind::PipeAssign
            | TokenKind::CaretAssign
            | TokenKind::ShlAssign
            | TokenKind::ShrAssign => self.is_integer_type(ty),
            _ => true,
        }
    }

    fn check_return_statement(&mut self, node: &SyntaxNode, return_type: TypeId) {
        if self.defer_depth > 0 {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ControlFlow,
                    "`return` cannot appear inside a deferred block",
                )
                .with_primary(node.span),
            );
        }
        let expression = child_nodes(node).into_iter().next();
        let unit_type = self.typed.types.primitive(PrimitiveType::Unit);
        let is_unit = self.typed.types.exactly_equal(return_type, unit_type);
        match expression {
            Some(expression) => {
                let (expression_type, _) =
                    self.check_expr(expression, ExpectedType::Exact(return_type));
                self.program.copies.insert(expression.span);
                if is_unit {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "a unit-returning function must use `return` without a value",
                        )
                        .with_primary(expression.span),
                    );
                } else if !self.types_compatible(expression_type, return_type) {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "the returned value's type does not match the declared return type",
                        )
                        .with_primary(expression.span),
                    );
                }
            }
            None => {
                if !is_unit {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "a non-unit function must return a value",
                        )
                        .with_primary(node.span),
                    );
                }
            }
        }
    }

    fn check_condition(&mut self, node: &SyntaxNode) {
        let bool_type = self.typed.types.primitive(PrimitiveType::Bool);
        let (condition_type, _) = self.check_expr(node, ExpectedType::Exact(bool_type));
        if condition_type != self.typed.types.error()
            && !self.typed.types.exactly_equal(condition_type, bool_type)
        {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ExpressionType,
                    "a condition must have type `bool`",
                )
                .with_primary(node.span),
            );
        }
    }

    fn check_if_statement(&mut self, node: &SyntaxNode, return_type: TypeId) -> bool {
        let children = child_nodes(node);
        let mut then_returns = false;
        let mut else_returns = None;
        for child in &children {
            match child.kind {
                SyntaxKind::Block => then_returns = self.check_block(child, return_type),
                SyntaxKind::ElseClause => {
                    else_returns = Some(
                        match child_nodes(child)
                            .into_iter()
                            .find(|node| node.kind == SyntaxKind::Block)
                        {
                            Some(block) => self.check_block(block, return_type),
                            None => false,
                        },
                    );
                }
                _ => self.check_condition(child),
            }
        }
        then_returns && else_returns.unwrap_or(false)
    }

    fn check_match_statement(&mut self, node: &SyntaxNode, return_type: TypeId) -> bool {
        let children = child_nodes(node);
        let scrutinee_type = children
            .first()
            .map(|scrutinee| self.check_expr(scrutinee, ExpectedType::None).0)
            .unwrap_or_else(|| self.typed.types.error());
        let Some(block) = children
            .into_iter()
            .find(|child| child.kind == SyntaxKind::Block)
        else {
            return false;
        };
        let mut coverage = Coverage::None;
        let mut all_arms_return = true;
        let mut any_arm = false;
        for arm in child_nodes(block) {
            if arm.kind != SyntaxKind::MatchArm {
                continue;
            }
            any_arm = true;
            let unreachable = coverage.is_catchall();
            if unreachable {
                let span = child_nodes(arm).first().map_or(arm.span, |p| p.span);
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Pattern,
                        "this match arm is unreachable; an earlier arm already matches every value",
                    )
                    .with_primary(span),
                );
            }
            let mut guarded = false;
            let mut arm_coverage = Coverage::None;
            let mut arm_returns = false;
            for child in child_nodes(arm) {
                match child.kind {
                    SyntaxKind::Pattern | SyntaxKind::AlternativePattern => {
                        arm_coverage = self.check_pattern(child, scrutinee_type);
                        if !unreachable && !guarded {
                            let redundant = arm_coverage.is_covered_by(&coverage);
                            if redundant {
                                self.diagnostics.push(
                                    Diagnostic::new(
                                        Category::Pattern,
                                        "this match arm is unreachable; its pattern is already \
                                         fully covered by an earlier arm",
                                    )
                                    .with_primary(child.span),
                                );
                            }
                        }
                    }
                    SyntaxKind::Guard => {
                        guarded = true;
                        if let Some(condition) = child_nodes(child).into_iter().next() {
                            self.check_condition(condition);
                        }
                    }
                    SyntaxKind::Block => arm_returns = self.check_block(child, return_type),
                    _ => {}
                }
            }
            // Guarded arms do not contribute to exhaustiveness or
            // redundancy tracking (`SPEC.md` section 7): the guard may fail
            // at runtime, so this arm cannot be assumed to consume coverage.
            if !guarded {
                coverage = coverage.union(arm_coverage);
            }
            if !arm_returns {
                all_arms_return = false;
            }
        }
        let exhaustive = self.coverage_is_exhaustive(scrutinee_type, &coverage);
        if any_arm && !exhaustive {
            let missing = self.describe_missing_coverage(scrutinee_type, &coverage);
            if let Some(missing) = missing {
                self.diagnostics
                    .push(Diagnostic::new(Category::Pattern, missing).with_primary(node.span));
            }
        }
        any_arm && exhaustive && all_arms_return
    }

    /// Whether `coverage` matches every value of `scrutinee_type`: literally
    /// `Coverage::Catchall`, or `Coverage::Bools`/`Coverage::Variants`
    /// reaching the scrutinee's full closed domain.
    fn coverage_is_exhaustive(&self, scrutinee_type: TypeId, coverage: &Coverage) -> bool {
        match coverage {
            Coverage::Catchall => true,
            Coverage::Dereferenced(inner) => {
                let mut resolved = self.typed.types.resolve_inference(scrutinee_type);
                loop {
                    match self.typed.types.kind(resolved) {
                        TypeKind::Alias { target, .. } => resolved = *target,
                        TypeKind::Reference { target, .. } => {
                            return self.coverage_is_exhaustive(*target, inner);
                        }
                        _ => return false,
                    }
                }
            }
            Coverage::Bools(values) => values.len() >= 2,
            Coverage::BuiltinVariants(variants) => {
                let resolved = self.typed.types.resolve_inference(scrutinee_type);
                matches!(
                    self.typed.types.kind(resolved),
                    TypeKind::Builtin { builtin, .. }
                        if self.resolved.builtin_name(*builtin) == "Option"
                            && variants.contains("Some")
                            && variants.contains("None")
                )
            }
            Coverage::Variants(variants) => {
                let resolved = self.typed.types.resolve_inference(scrutinee_type);
                match self.typed.types.kind(resolved) {
                    TypeKind::Nominal { identity, .. } => self
                        .resolved
                        .variants
                        .iter()
                        .filter(|variant| variant.parent == identity.declaration)
                        .all(|variant| variants.contains(&variant.id)),
                    _ => false,
                }
            }
            Coverage::None | Coverage::Other => false,
        }
    }

    /// Produces a diagnostic message for a non-exhaustive match, or `None`
    /// when the scrutinee's shape is not one Milestone 7 reasons about
    /// precisely (tuples/structs with a refutable field, and any other
    /// non-`bool`/non-`enum` type): those conservatively require an
    /// explicit catch-all arm, matching `SPEC.md`'s "infinite domain" rule,
    /// without this module claiming to enumerate their exact cases.
    fn describe_missing_coverage(
        &self,
        scrutinee_type: TypeId,
        coverage: &Coverage,
    ) -> Option<String> {
        let resolved = self.typed.types.resolve_inference(scrutinee_type);
        if let Coverage::Dereferenced(inner) = coverage {
            return match self.typed.types.kind(resolved) {
                TypeKind::Reference { target, .. } => {
                    self.describe_missing_coverage(*target, inner)
                }
                _ => None,
            };
        }
        match self.typed.types.kind(resolved) {
            TypeKind::Primitive(PrimitiveType::Bool) => Some(
                "this match is not exhaustive; cover both `true` and `false`, or add a \
                 catch-all `_` arm"
                    .to_string(),
            ),
            TypeKind::Nominal { identity, .. }
                if self.resolved.declarations[identity.declaration.index()].kind
                    == DeclarationKind::Enum =>
            {
                let covered = match coverage {
                    Coverage::Variants(variants) => variants.clone(),
                    _ => BTreeSet::new(),
                };
                let missing: Vec<&str> = self
                    .resolved
                    .variants
                    .iter()
                    .filter(|variant| variant.parent == identity.declaration)
                    .filter(|variant| !covered.contains(&variant.id))
                    .map(|variant| self.resolved.symbol_text(variant.name))
                    .collect();
                if missing.is_empty() {
                    None
                } else {
                    Some(format!(
                        "this match is not exhaustive; unmatched variant(s): {}",
                        missing.join(", ")
                    ))
                }
            }
            TypeKind::Builtin { builtin, .. }
                if self.resolved.builtin_name(*builtin) == "Option" =>
            {
                let covered = match coverage {
                    Coverage::BuiltinVariants(variants) => variants,
                    _ => {
                        return Some(
                            "this match is not exhaustive; cover `Option.Some` and \
                             `Option.None`, or add a catch-all `_` arm"
                                .to_string(),
                        );
                    }
                };
                let missing = ["Some", "None"]
                    .into_iter()
                    .filter(|variant| !covered.contains(*variant))
                    .collect::<Vec<_>>();
                (!missing.is_empty()).then(|| {
                    format!(
                        "this match is not exhaustive; unmatched variant(s): {}",
                        missing.join(", ")
                    )
                })
            }
            TypeKind::Error => None,
            _ => Some("this match is not exhaustive; add a catch-all `_` arm".to_string()),
        }
    }

    // ---------------------------------------------------------------
    // Patterns
    // ---------------------------------------------------------------

    /// Type-checks `pattern` against `scrutinee_type`, registers its
    /// bindings (as ordinary immutable, independently copied `let`-like
    /// bindings per `SPEC.md` section 7) into the same local-type tables
    /// `check_expr` reads, and returns what the pattern covers for
    /// exhaustiveness/reachability purposes.
    ///
    /// This module reasons precisely about coverage for `bool` and `enum`
    /// scrutinees and for any pattern that is unconditionally irrefutable
    /// (`_`, a plain binding, or a tuple/struct pattern built entirely from
    /// those). Nested field-value refutability inside a matched enum
    /// variant is not tracked (an arm's outer constructor is treated as
    /// fully covering that variant once matched); everything else
    /// (tuples/structs with a refutable field, literals of an unbounded
    /// domain) conservatively requires an explicit catch-all, matching
    /// `SPEC.md`'s "infinite domain" rule rather than risking a false
    /// exhaustiveness claim.
    fn check_pattern(&mut self, pattern: &SyntaxNode, scrutinee_type: TypeId) -> Coverage {
        match pattern.kind {
            SyntaxKind::AlternativePattern => {
                let atoms = child_nodes(pattern);
                let mut binding_sets = Vec::with_capacity(atoms.len());
                let mut coverage = Coverage::None;
                for atom in &atoms {
                    let mut names = BTreeSet::new();
                    collect_pattern_binding_names(atom, &mut names);
                    binding_sets.push(names);
                    coverage = coverage.union(self.check_pattern(atom, scrutinee_type));
                }
                if let Some(first_names) = binding_sets.first().cloned() {
                    for (atom, names) in atoms.iter().zip(binding_sets.iter()).skip(1) {
                        if *names != first_names {
                            self.diagnostics.push(
                                Diagnostic::new(
                                    Category::Pattern,
                                    "every alternative in a `|` pattern must bind the same names",
                                )
                                .with_primary(atom.span),
                            );
                        }
                    }
                }
                coverage
            }
            SyntaxKind::TuplePattern => {
                let elements = child_nodes(pattern);
                let resolved = self.typed.types.resolve_inference(scrutinee_type);
                let members = match self.typed.types.kind(resolved) {
                    TypeKind::Tuple(members) if members.len() == elements.len() => {
                        Some(members.clone())
                    }
                    _ => None,
                };
                if members.is_none() && scrutinee_type != self.typed.types.error() {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::Pattern,
                            "this tuple pattern does not match the scrutinee's type",
                        )
                        .with_primary(pattern.span),
                    );
                }
                let mut all_irrefutable = true;
                for (index, element) in elements.iter().enumerate() {
                    let element_type = members
                        .as_ref()
                        .map_or_else(|| self.typed.types.error(), |members| members[index]);
                    if !self.check_pattern(element, element_type).is_catchall() {
                        all_irrefutable = false;
                    }
                }
                if all_irrefutable {
                    Coverage::Catchall
                } else {
                    Coverage::Other
                }
            }
            SyntaxKind::DereferencePattern => {
                let Some(inner) = child_nodes(pattern).into_iter().next() else {
                    return Coverage::Other;
                };
                let mut resolved = self.typed.types.resolve_inference(scrutinee_type);
                loop {
                    match self.typed.types.kind(resolved) {
                        TypeKind::Alias { target, .. } => resolved = *target,
                        TypeKind::Reference { target, .. } => {
                            return Coverage::Dereferenced(Box::new(
                                self.check_pattern(inner, *target),
                            ));
                        }
                        TypeKind::Error => return self.check_pattern(inner, resolved),
                        _ => {
                            self.diagnostics.push(
                                Diagnostic::new(
                                    Category::Pattern,
                                    "a dereference pattern requires a safe-reference scrutinee",
                                )
                                .with_primary(pattern.span),
                            );
                            return self.check_pattern(inner, self.typed.types.error());
                        }
                    }
                }
            }
            SyntaxKind::RecordPattern | SyntaxKind::VariantPattern => {
                self.check_aggregate_pattern(pattern, scrutinee_type)
            }
            SyntaxKind::Pattern => {
                let nested = child_nodes(pattern);
                if let Some(&atom) = nested.first() {
                    return self.check_pattern(atom, scrutinee_type);
                }
                self.check_pattern_leaf(pattern, scrutinee_type)
            }
            _ => Coverage::Other,
        }
    }

    fn check_pattern_leaf(&mut self, pattern: &SyntaxNode, scrutinee_type: TypeId) -> Coverage {
        if let Some(token) = pattern.children.iter().find_map(|child| match child {
            SyntaxElement::Token(token) if is_pattern_literal_token(&token.kind) => Some(token),
            _ => None,
        }) {
            let ty = self.literal_token_type(token, ExpectedType::Exact(scrutinee_type), false);
            if !self.types_compatible(ty, scrutinee_type) {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Pattern,
                        "this pattern's type does not match the scrutinee's type",
                    )
                    .with_primary(pattern.span),
                );
            }
            return match token.kind {
                TokenKind::Keyword(Keyword::True) => Coverage::Bools(BTreeSet::from([true])),
                TokenKind::Keyword(Keyword::False) => Coverage::Bools(BTreeSet::from([false])),
                _ => Coverage::Other,
            };
        }
        let identifiers: Vec<&Token> = pattern
            .children
            .iter()
            .filter_map(|child| match child {
                SyntaxElement::Token(token) if matches!(token.kind, TokenKind::Identifier(_)) => {
                    Some(token)
                }
                _ => None,
            })
            .collect();
        let has_dot = pattern.children.iter().any(|child| {
            matches!(
                child,
                SyntaxElement::Token(Token {
                    kind: TokenKind::Dot,
                    ..
                })
            )
        });
        if has_dot {
            let (Some(&first), Some(&last)) = (identifiers.first(), identifiers.last()) else {
                return Coverage::Other;
            };
            if let Some(coverage) =
                self.check_builtin_variant_pattern(first, last, pattern, scrutinee_type, false)
            {
                return coverage;
            }
            let Some((enum_declaration, variant)) = self.resolve_pattern_variant(first, last)
            else {
                return Coverage::Other;
            };
            if self
                .nominal_arguments_for_type(scrutinee_type, enum_declaration)
                .is_none()
            {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Pattern,
                        "this pattern's type does not match the scrutinee's type",
                    )
                    .with_primary(pattern.span),
                );
            }
            if !self.resolved.variants[variant.index()].fields.is_empty() {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Pattern,
                        "this variant has fields and must be matched with `(...)` or `{...}`",
                    )
                    .with_primary(pattern.span),
                );
            }
            return Coverage::Variants(BTreeSet::from([variant]));
        }
        if let [token] = identifiers.as_slice() {
            let text = token_text(token);
            if text != "_" {
                self.bind_pattern(token, scrutinee_type);
            }
        }
        Coverage::Catchall
    }

    fn resolve_pattern_variant(
        &self,
        first: &Token,
        last: &Token,
    ) -> Option<(DeclarationId, VariantId)> {
        let target = self.resolved.reference_at(first.span)?.target;
        let NameTarget::Item(crate::resolution::ItemId::Declaration(enum_declaration)) = target
        else {
            return None;
        };
        if self.resolved.declarations[enum_declaration.index()].kind != DeclarationKind::Enum {
            return None;
        }
        match self.find_member(enum_declaration, &token_text(last)) {
            Some(MemberId::Variant(variant)) => Some((enum_declaration, variant)),
            _ => None,
        }
    }

    fn check_builtin_variant_pattern(
        &mut self,
        first: &Token,
        last: &Token,
        pattern: &SyntaxNode,
        scrutinee_type: TypeId,
        positional: bool,
    ) -> Option<Coverage> {
        let NameTarget::Item(crate::resolution::ItemId::Builtin(builtin)) =
            self.resolved.reference_at(first.span)?.target
        else {
            return None;
        };
        if self.resolved.builtin_name(builtin) != "Option" {
            return None;
        }
        let variant = token_text(last);
        if !matches!(variant.as_str(), "Some" | "None") {
            return None;
        }
        let resolved = self.typed.types.resolve_inference(scrutinee_type);
        let argument = match self.typed.types.kind(resolved).clone() {
            TypeKind::Builtin {
                builtin: actual,
                arguments,
            } if actual == builtin && arguments.len() == 1 => Some(arguments[0]),
            TypeKind::Error => Some(self.typed.types.error()),
            _ => None,
        };
        if argument.is_none() {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Pattern,
                    "this pattern's type does not match the scrutinee's type",
                )
                .with_primary(pattern.span),
            );
        }
        let children = child_nodes(pattern);
        let required = usize::from(variant == "Some");
        if positional {
            if children.len() != required {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Pattern,
                        format!(
                            "`Option.{variant}` has {required} field{}, but the pattern supplies {}",
                            if required == 1 { "" } else { "s" },
                            children.len()
                        ),
                    )
                    .with_primary(pattern.span),
                );
            }
            if variant == "Some"
                && let (Some(child), Some(argument)) = (children.first(), argument)
            {
                self.check_pattern(child, argument);
            }
        } else if required != 0 {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Pattern,
                    "`Option.Some` has a field and must be matched with `(...)`",
                )
                .with_primary(pattern.span),
            );
        }
        Some(Coverage::BuiltinVariants(BTreeSet::from([variant])))
    }

    fn nominal_arguments_for_type(
        &self,
        mut ty: TypeId,
        declaration: DeclarationId,
    ) -> Option<Vec<TypeId>> {
        loop {
            ty = self.typed.types.resolve_inference(ty);
            match self.typed.types.kind(ty) {
                TypeKind::Alias { target, .. } => ty = *target,
                TypeKind::Nominal {
                    identity,
                    arguments,
                } if identity.declaration == declaration => return Some(arguments.clone()),
                TypeKind::Error => return Some(Vec::new()),
                _ => return None,
            }
        }
    }

    fn check_aggregate_pattern(
        &mut self,
        pattern: &SyntaxNode,
        scrutinee_type: TypeId,
    ) -> Coverage {
        let path_tokens = leading_pattern_path(pattern);
        let Some(&first) = path_tokens.first() else {
            return Coverage::Other;
        };
        let has_dot = path_tokens
            .iter()
            .any(|token| matches!(token.kind, TokenKind::Dot));
        if has_dot {
            let Some(&last) = path_tokens
                .iter()
                .rev()
                .find(|token| matches!(token.kind, TokenKind::Identifier(_)))
            else {
                return Coverage::Other;
            };
            if let Some(coverage) =
                self.check_builtin_variant_pattern(first, last, pattern, scrutinee_type, true)
            {
                return coverage;
            }
            let Some((enum_declaration, variant)) = self.resolve_pattern_variant(first, last)
            else {
                return Coverage::Other;
            };
            let owner_arguments = self.nominal_arguments_for_type(scrutinee_type, enum_declaration);
            if owner_arguments.is_none() {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Pattern,
                        "this pattern's type does not match the scrutinee's type",
                    )
                    .with_primary(pattern.span),
                );
            }
            let fields = self.resolved.variants[variant.index()].fields.clone();
            let owner_arguments = owner_arguments.unwrap_or_default();
            match pattern.kind {
                SyntaxKind::VariantPattern => self.check_variant_positional_pattern(
                    pattern,
                    &fields,
                    enum_declaration,
                    &owner_arguments,
                ),
                SyntaxKind::RecordPattern => {
                    self.check_record_pattern_fields(
                        pattern,
                        &fields,
                        enum_declaration,
                        &owner_arguments,
                    );
                }
                _ => {}
            }
            return Coverage::Variants(BTreeSet::from([variant]));
        }
        let Some(reference) = self.resolved.reference_at(first.span) else {
            return Coverage::Other;
        };
        let NameTarget::Item(crate::resolution::ItemId::Declaration(declaration_id)) =
            reference.target
        else {
            return Coverage::Other;
        };
        let declaration = &self.resolved.declarations[declaration_id.index()];
        if declaration.kind != DeclarationKind::Struct {
            return Coverage::Other;
        }
        let owner_arguments = self.nominal_arguments_for_type(scrutinee_type, declaration_id);
        if owner_arguments.is_none() {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Pattern,
                    "this pattern's type does not match the scrutinee's type",
                )
                .with_primary(pattern.span),
            );
        }
        let fields: Vec<FieldId> = self
            .resolved
            .fields
            .iter()
            .filter(|field| {
                field.parent_declaration == declaration_id && field.parent_variant.is_none()
            })
            .map(|field| field.id)
            .collect();
        let all_irrefutable = match pattern.kind {
            SyntaxKind::RecordPattern => self.check_record_pattern_fields(
                pattern,
                &fields,
                declaration_id,
                &owner_arguments.unwrap_or_default(),
            ),
            _ => false,
        };
        if all_irrefutable {
            Coverage::Catchall
        } else {
            Coverage::Other
        }
    }

    /// Checks a `VariantPattern`'s parenthesized positional sub-patterns
    /// against a tuple-like variant's field types.
    fn check_variant_positional_pattern(
        &mut self,
        pattern: &SyntaxNode,
        fields: &[FieldId],
        _owner: DeclarationId,
        owner_arguments: &[TypeId],
    ) {
        let sub_patterns = child_nodes(pattern);
        if sub_patterns.len() != fields.len() {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Pattern,
                    format!(
                        "this variant has {} field{}, but the pattern supplies {}",
                        fields.len(),
                        if fields.len() == 1 { "" } else { "s" },
                        sub_patterns.len(),
                    ),
                )
                .with_primary(pattern.span),
            );
        }
        for (index, sub_pattern) in sub_patterns.iter().enumerate() {
            let field_type = fields
                .get(index)
                .and_then(|field| {
                    self.typed
                        .instantiate_field_type(self.resolved, *field, owner_arguments)
                })
                .unwrap_or_else(|| self.typed.types.error());
            self.check_pattern(sub_pattern, field_type);
        }
    }

    /// Checks a `RecordPattern`'s named fields (struct or record-variant)
    /// and returns whether the pattern is irrefutable: every explicit field
    /// sub-pattern is itself irrefutable, or `..` was used to ignore the
    /// rest. Enforces "every field must appear, or use `..`" (`SPEC.md`
    /// section 7).
    fn check_record_pattern_fields(
        &mut self,
        pattern: &SyntaxNode,
        required: &[FieldId],
        _owner: DeclarationId,
        owner_arguments: &[TypeId],
    ) -> bool {
        let mut seen: BTreeSet<FieldId> = BTreeSet::new();
        let mut has_rest = false;
        let mut all_irrefutable = true;
        for field_node in child_nodes(pattern) {
            if field_node.kind != SyntaxKind::PatternField {
                continue;
            }
            if matches!(
                field_node.children.first(),
                Some(SyntaxElement::Token(Token {
                    kind: TokenKind::DotDot,
                    ..
                }))
            ) {
                has_rest = true;
                continue;
            }
            let Some(name_token) = first_identifier_token(field_node) else {
                continue;
            };
            let name_text = token_text(name_token);
            let matched = required.iter().copied().find(|field_id| {
                self.resolved
                    .symbol_text(self.resolved.fields[field_id.index()].name)
                    == name_text
            });
            let Some(field_id) = matched else {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Pattern,
                        format!("no field named `{name_text}` on this pattern's type"),
                    )
                    .with_primary(name_token.span),
                );
                all_irrefutable = false;
                continue;
            };
            if !seen.insert(field_id) {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Pattern,
                        format!("field `{name_text}` is matched more than once"),
                    )
                    .with_primary(name_token.span),
                );
            }
            let field_type = self
                .typed
                .instantiate_field_type(self.resolved, field_id, owner_arguments)
                .unwrap_or_else(|| self.typed.types.error());
            match child_nodes(field_node).into_iter().next() {
                Some(sub_pattern) => {
                    if !self.check_pattern(sub_pattern, field_type).is_catchall() {
                        all_irrefutable = false;
                    }
                }
                None => {
                    if name_text != "_" {
                        self.bind_pattern(name_token, field_type);
                    }
                }
            }
        }
        if !has_rest {
            for field_id in required {
                if !seen.contains(field_id) {
                    let name = self
                        .resolved
                        .symbol_text(self.resolved.fields[field_id.index()].name);
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::Pattern,
                            format!("this pattern is missing field `{name}`; add it or use `..`"),
                        )
                        .with_primary(pattern.span),
                    );
                    all_irrefutable = false;
                }
            }
        } else {
            // Fields ignored by `..` are untested, so they cannot make the
            // pattern refutable.
        }
        has_rest || all_irrefutable
    }

    fn bind_pattern(&mut self, token: &Token, ty: TypeId) {
        if let Some(&id) = self.span_to_local.get(&token.span) {
            self.local_types.insert(id, ty);
            self.local_rebindable.insert(id, Rebindable::Let);
            self.program.copied_pattern_bindings.insert(id);
            self.program.pattern_binding_types.insert(id, ty);
        }
    }

    // ---------------------------------------------------------------
    // Expressions
    // ---------------------------------------------------------------

    fn check_expr(&mut self, node: &SyntaxNode, expected: ExpectedType) -> (TypeId, PlaceKind) {
        let (ty, place) = match node.kind {
            SyntaxKind::LiteralExpression => self.check_literal(node, expected),
            SyntaxKind::FormattedStringExpression => self.check_formatted_string(node),
            SyntaxKind::NameExpression => self.check_name_expression(node, expected),
            SyntaxKind::MemberExpression => self.check_member_expression(node, expected),
            SyntaxKind::UnaryExpression => self.check_unary(node, expected),
            SyntaxKind::BinaryExpression => self.check_binary(node),
            SyntaxKind::CastExpression => self.check_cast(node),
            SyntaxKind::CallExpression => self.check_call(node, expected),
            SyntaxKind::BracketExpression => self.check_bracket_expression(node, expected),
            SyntaxKind::TryExpression => self.check_try(node),
            SyntaxKind::ParenthesizedExpression => self.check_parenthesized(node, expected),
            SyntaxKind::TupleExpression => self.check_tuple(node, expected),
            SyntaxKind::ArrayExpression => self.check_array(node, expected),
            SyntaxKind::MacroExpression => self.check_macro(node),
            SyntaxKind::RecordExpression => self.check_record_expression(node, expected),
            _ => (self.typed.types.error(), PlaceKind::Value),
        };
        self.program.expression_types.insert(node.span, ty);
        self.program.expression_places.insert(node.span, place);
        (ty, place)
    }

    fn error_literal(&mut self, span: Span, message: String) {
        self.diagnostics
            .push(Diagnostic::new(Category::LiteralType, message).with_primary(span));
    }

    fn check_literal(&mut self, node: &SyntaxNode, expected: ExpectedType) -> (TypeId, PlaceKind) {
        let Some(token) = node.children.iter().find_map(|child| match child {
            SyntaxElement::Token(token) => Some(token),
            SyntaxElement::Node(_) => None,
        }) else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let ty = self.literal_token_type(token, expected, false);
        (ty, PlaceKind::Value)
    }

    fn literal_token_type(
        &mut self,
        token: &Token,
        expected: ExpectedType,
        negative: bool,
    ) -> TypeId {
        match &token.kind {
            TokenKind::IntegerLiteral { raw, radix, suffix } => {
                match self.typed.types.materialize_integer(
                    raw,
                    *radix,
                    *suffix,
                    negative,
                    expected,
                    self.pointer_bits,
                ) {
                    Ok(ty) => ty,
                    Err(error) => {
                        self.error_literal(token.span, error.message);
                        self.typed.types.error()
                    }
                }
            }
            TokenKind::FloatLiteral { raw, suffix } => {
                match self.typed.types.materialize_float(raw, *suffix, expected) {
                    Ok(ty) => ty,
                    Err(error) => {
                        self.error_literal(token.span, error.message);
                        self.typed.types.error()
                    }
                }
            }
            TokenKind::StringLiteral(_) => match self.typed.types.materialize_string(expected) {
                Ok(ty) => ty,
                Err(error) => {
                    self.error_literal(token.span, error.message);
                    self.typed.types.error()
                }
            },
            TokenKind::CharacterLiteral(_) => self.typed.types.primitive(PrimitiveType::Char),
            TokenKind::Keyword(Keyword::True | Keyword::False) => {
                self.typed.types.primitive(PrimitiveType::Bool)
            }
            TokenKind::Keyword(Keyword::Null) => {
                if let ExpectedType::Exact(candidate) = expected {
                    let resolved = self.typed.types.resolve_inference(candidate);
                    if matches!(self.typed.types.kind(resolved), TypeKind::RawPointer { .. }) {
                        return candidate;
                    }
                }
                let target = self.typed.types.fresh_inference_variable();
                self.typed.types.intern(TypeKind::RawPointer {
                    mutability: Mutability::Shared,
                    target,
                })
            }
            _ => self.typed.types.error(),
        }
    }

    fn check_formatted_string(&mut self, node: &SyntaxNode) -> (TypeId, PlaceKind) {
        for expression in child_nodes(node) {
            self.check_expr(expression, ExpectedType::None);
        }
        (
            self.typed.types.primitive(PrimitiveType::Str),
            PlaceKind::Value,
        )
    }

    fn check_name_expression(
        &mut self,
        node: &SyntaxNode,
        expected: ExpectedType,
    ) -> (TypeId, PlaceKind) {
        let Some(token) = node.children.iter().find_map(|child| match child {
            SyntaxElement::Token(token) => Some(token),
            SyntaxElement::Node(_) => None,
        }) else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let result = self.check_name_target(token.span, expected);
        if let Some(instance) = self.program.function_references.get(&token.span).cloned() {
            self.program.function_references.insert(node.span, instance);
        }
        result
    }

    fn check_name_target(&mut self, span: Span, expected: ExpectedType) -> (TypeId, PlaceKind) {
        match self
            .resolved
            .reference_at(span)
            .map(|reference| reference.target)
        {
            Some(NameTarget::Local(id)) => {
                let ty = self
                    .local_types
                    .get(&id)
                    .copied()
                    .unwrap_or_else(|| self.typed.types.error());
                let place = match self.local_rebindable.get(&id) {
                    Some(Rebindable::Var) => PlaceKind::Mutable,
                    Some(Rebindable::Let) => PlaceKind::Addressable,
                    None => PlaceKind::Value,
                };
                (ty, place)
            }
            Some(NameTarget::Item(crate::resolution::ItemId::Declaration(declaration)))
                if matches!(
                    self.resolved.declarations[declaration.index()].kind,
                    DeclarationKind::Function | DeclarationKind::ForeignFunction
                ) =>
            {
                let Some(instance) =
                    self.function_instance_from_expected(declaration, None, expected, span)
                else {
                    return (self.typed.types.error(), PlaceKind::Value);
                };
                let ty = self.function_reference_type(&instance);
                self.program.function_references.insert(span, instance);
                (ty, PlaceKind::Value)
            }
            // Other items, `Self`, and generic parameters used as bare values
            // have no runtime value in the Milestone 11 subset.
            _ => (self.typed.types.error(), PlaceKind::Value),
        }
    }

    fn function_reference_type(&mut self, instance: &FunctionInstance) -> TypeId {
        let Some(signature) = self.typed.instantiate_signature(self.resolved, instance) else {
            return self.typed.types.error();
        };
        let TypeKind::Function {
            safety,
            abi,
            receiver,
            parameters,
            return_type,
        } = self.typed.types.kind(signature.ty).clone()
        else {
            return self.typed.types.error();
        };
        let parameters = receiver
            .map(|ty| FunctionParameter {
                ty,
                variadic: false,
            })
            .into_iter()
            .chain(parameters)
            .collect();
        let function = self.typed.types.intern(TypeKind::Function {
            safety,
            abi,
            receiver: None,
            parameters,
            return_type,
        });
        self.typed.types.intern(TypeKind::Reference {
            mutability: Mutability::Shared,
            target: function,
        })
    }

    fn callable_parameters(&self, declaration: DeclarationId) -> Vec<GenericParameterId> {
        self.typed
            .callable_generic_parameters(self.resolved, declaration)
    }

    fn generic_inference_variables(
        &mut self,
        parameters: &[GenericParameterId],
    ) -> BTreeMap<GenericParameterId, TypeId> {
        parameters
            .iter()
            .map(|parameter| (*parameter, self.typed.types.fresh_inference_variable()))
            .collect()
    }

    fn inference_substitution(variables: &BTreeMap<GenericParameterId, TypeId>) -> Substitution {
        let mut substitution = Substitution::new();
        for (parameter, variable) in variables {
            substitution.insert(*parameter, *variable);
        }
        substitution
    }

    fn infer_against(
        &mut self,
        template: TypeId,
        actual: TypeId,
        variables: &BTreeMap<GenericParameterId, TypeId>,
    ) -> bool {
        let pattern = self
            .typed
            .types
            .substitute(template, &Self::inference_substitution(variables));
        self.typed.types.unify(pattern, actual).is_ok()
    }

    fn finish_inference(
        &self,
        parameters: &[GenericParameterId],
        variables: &BTreeMap<GenericParameterId, TypeId>,
    ) -> Option<Vec<TypeId>> {
        parameters
            .iter()
            .map(|parameter| {
                let variable = variables.get(parameter)?;
                let resolved = self.typed.types.resolve_inference(*variable);
                (!matches!(
                    self.typed.types.kind(resolved),
                    TypeKind::InferenceVariable(_)
                ))
                .then_some(resolved)
            })
            .collect()
    }

    fn type_from_expression(&mut self, node: &SyntaxNode) -> Option<TypeId> {
        match node.kind {
            SyntaxKind::NameExpression | SyntaxKind::MemberExpression => {
                let span = Self::callee_target_span(node)?;
                match self.resolved.reference_at(span)?.target {
                    NameTarget::GenericParameter(parameter) => Some(
                        self.typed
                            .types
                            .intern(TypeKind::GenericParameter(parameter)),
                    ),
                    NameTarget::SelfType => self.current_self_declaration.and_then(|declaration| {
                        self.typed.declaration_types.get(&declaration).copied()
                    }),
                    NameTarget::Item(crate::resolution::ItemId::Declaration(declaration)) => self
                        .typed
                        .instantiate_declaration_type(self.resolved, declaration, &[]),
                    NameTarget::Item(crate::resolution::ItemId::Builtin(builtin)) => {
                        let name = self.resolved.builtin_name(builtin);
                        types::primitive_from_name(name)
                            .map(|primitive| self.typed.types.primitive(primitive))
                            .or_else(|| {
                                Some(self.typed.types.intern(TypeKind::Builtin {
                                    builtin,
                                    arguments: Vec::new(),
                                }))
                            })
                    }
                    _ => None,
                }
            }
            SyntaxKind::BracketExpression => {
                let nodes = child_nodes(node);
                let base = *nodes.first()?;
                let arguments = nodes[1..]
                    .iter()
                    .map(|argument| self.type_from_expression(argument))
                    .collect::<Option<Vec<_>>>()?;
                let span = Self::callee_target_span(base)?;
                match self.resolved.reference_at(span)?.target {
                    NameTarget::Item(crate::resolution::ItemId::Declaration(declaration)) => self
                        .typed
                        .instantiate_declaration_type(self.resolved, declaration, &arguments),
                    NameTarget::Item(crate::resolution::ItemId::Builtin(builtin)) => Some(
                        self.typed
                            .types
                            .intern(TypeKind::Builtin { builtin, arguments }),
                    ),
                    _ => None,
                }
            }
            SyntaxKind::TupleExpression => {
                let elements = child_nodes(node)
                    .into_iter()
                    .map(|element| self.type_from_expression(element))
                    .collect::<Option<Vec<_>>>()?;
                Some(if elements.is_empty() {
                    self.typed.types.primitive(PrimitiveType::Unit)
                } else {
                    self.typed.types.intern(TypeKind::Tuple(elements))
                })
            }
            SyntaxKind::UnaryExpression => {
                let token = node.children.iter().find_map(|child| match child {
                    SyntaxElement::Token(token) => Some(token),
                    SyntaxElement::Node(_) => None,
                })?;
                let target =
                    self.type_from_expression(child_nodes(node).into_iter().next_back()?)?;
                let mutability = if node.children.iter().any(|child| {
                    matches!(
                        child,
                        SyntaxElement::Token(Token {
                            kind: TokenKind::Keyword(Keyword::Var),
                            ..
                        })
                    )
                }) {
                    Mutability::Mutable
                } else {
                    Mutability::Shared
                };
                match token.kind {
                    TokenKind::Amp => Some(
                        self.typed
                            .types
                            .intern(TypeKind::Reference { mutability, target }),
                    ),
                    TokenKind::Star => Some(
                        self.typed
                            .types
                            .intern(TypeKind::RawPointer { mutability, target }),
                    ),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn explicit_function_selection(
        &mut self,
        node: &SyntaxNode,
    ) -> Option<(DeclarationId, Vec<TypeId>)> {
        if node.kind != SyntaxKind::BracketExpression {
            return None;
        }
        let nodes = child_nodes(node);
        let base = *nodes.first()?;
        let span = Self::callee_target_span(base)?;
        let NameTarget::Item(crate::resolution::ItemId::Declaration(declaration)) =
            self.resolved.reference_at(span)?.target
        else {
            return None;
        };
        if !matches!(
            self.resolved.declarations[declaration.index()].kind,
            DeclarationKind::Function | DeclarationKind::ForeignFunction
        ) {
            return None;
        }
        let arguments = nodes[1..]
            .iter()
            .map(|argument| self.type_from_expression(argument))
            .collect::<Option<Vec<_>>>()?;
        Some((declaration, arguments))
    }

    fn function_instance_from_expected(
        &mut self,
        declaration: DeclarationId,
        explicit: Option<Vec<TypeId>>,
        expected: ExpectedType,
        span: Span,
    ) -> Option<FunctionInstance> {
        let parameters = self.callable_parameters(declaration);
        if let Some(arguments) = explicit {
            if arguments.len() != parameters.len() {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Call,
                        format!(
                            "this function requires {} generic argument{}, but {} were supplied",
                            parameters.len(),
                            if parameters.len() == 1 { "" } else { "s" },
                            arguments.len()
                        ),
                    )
                    .with_primary(span),
                );
                return None;
            }
            return Some(FunctionInstance {
                declaration,
                arguments,
            });
        }
        if parameters.is_empty() {
            return Some(FunctionInstance {
                declaration,
                arguments: Vec::new(),
            });
        }
        let ExpectedType::Exact(expected) = expected else {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Call,
                    "generic arguments cannot be inferred uniquely without arguments or an \
                     expected function-reference type",
                )
                .with_primary(span),
            );
            return None;
        };
        let variables = self.generic_inference_variables(&parameters);
        let template_arguments = parameters
            .iter()
            .map(|parameter| {
                self.typed
                    .types
                    .intern(TypeKind::GenericParameter(*parameter))
            })
            .collect();
        let template_instance = FunctionInstance {
            declaration,
            arguments: template_arguments,
        };
        let template = self.function_reference_type(&template_instance);
        if !self.infer_against(template, expected, &variables) {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ExpressionType,
                    "the expected function-reference type is incompatible with this generic \
                     function",
                )
                .with_primary(span),
            );
            return None;
        }
        let arguments = self.finish_inference(&parameters, &variables)?;
        Some(FunctionInstance {
            declaration,
            arguments,
        })
    }

    fn check_bracket_expression(
        &mut self,
        node: &SyntaxNode,
        expected: ExpectedType,
    ) -> (TypeId, PlaceKind) {
        if let Some((declaration, arguments)) = self.explicit_function_selection(node) {
            let Some(instance) = self.function_instance_from_expected(
                declaration,
                Some(arguments),
                expected,
                node.span,
            ) else {
                return (self.typed.types.error(), PlaceKind::Value);
            };
            let ty = self.function_reference_type(&instance);
            self.program.function_references.insert(node.span, instance);
            return (ty, PlaceKind::Value);
        }
        self.check_index(node)
    }

    fn check_function_call(
        &mut self,
        call_span: Span,
        declaration: DeclarationId,
        explicit: Option<Vec<TypeId>>,
        arguments: &[&SyntaxNode],
        expected: ExpectedType,
        include_receiver: bool,
    ) -> (TypeId, PlaceKind) {
        let generic_parameters = self.callable_parameters(declaration);
        if let Some(explicit) = explicit {
            let Some(instance) = self.function_instance_from_expected(
                declaration,
                Some(explicit),
                expected,
                call_span,
            ) else {
                for argument in arguments {
                    self.check_expr(argument, ExpectedType::None);
                }
                return (self.typed.types.error(), PlaceKind::Value);
            };
            let Some(signature) = self.typed.instantiate_signature(self.resolved, &instance) else {
                return (self.typed.types.error(), PlaceKind::Value);
            };
            let mut parameters = signature.parameters.clone();
            if include_receiver && let Some(receiver) = signature.receiver {
                parameters.insert(
                    0,
                    FunctionParameter {
                        ty: receiver,
                        variadic: false,
                    },
                );
            }
            self.check_call_arguments(call_span, &parameters, arguments);
            if let ExpectedType::Exact(expected) = expected
                && !self.types_compatible(signature.return_type, expected)
            {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        "this call's result does not match the expected type",
                    )
                    .with_primary(call_span),
                );
            }
            self.program
                .calls
                .insert(call_span, CheckedCall::Direct(instance));
            return (signature.return_type, PlaceKind::Value);
        }

        if generic_parameters.is_empty() {
            let instance = FunctionInstance {
                declaration,
                arguments: Vec::new(),
            };
            let Some(signature) = self.typed.instantiate_signature(self.resolved, &instance) else {
                return (self.typed.types.error(), PlaceKind::Value);
            };
            let mut parameters = signature.parameters.clone();
            if include_receiver && let Some(receiver) = signature.receiver {
                parameters.insert(
                    0,
                    FunctionParameter {
                        ty: receiver,
                        variadic: false,
                    },
                );
            }
            self.check_call_arguments(call_span, &parameters, arguments);
            self.program
                .calls
                .insert(call_span, CheckedCall::Direct(instance));
            return (signature.return_type, PlaceKind::Value);
        }

        let Some(template) = self.typed.function_signatures.get(&declaration).cloned() else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let variables = self.generic_inference_variables(&generic_parameters);
        if let ExpectedType::Exact(expected) = expected {
            self.infer_against(template.return_type, expected, &variables);
        }

        let mut call_parameters = template.parameters.clone();
        if include_receiver && let Some(receiver) = template.receiver {
            call_parameters.insert(
                0,
                FunctionParameter {
                    ty: receiver,
                    variadic: false,
                },
            );
        }
        let variadic = call_parameters
            .last()
            .is_some_and(|parameter| parameter.variadic);
        let fixed = call_parameters.len() - usize::from(variadic);
        let arity_ok = if variadic {
            arguments.len() >= fixed
        } else {
            arguments.len() == fixed
        };
        if !arity_ok {
            self.report_call_arity(call_span, arguments.len(), fixed, variadic);
        }

        for (index, argument) in arguments.iter().enumerate() {
            let parameter = if index < fixed {
                call_parameters.get(index)
            } else {
                call_parameters.last().filter(|_| variadic)
            };
            let expected_argument = parameter.map(|parameter| {
                self.typed
                    .types
                    .substitute(parameter.ty, &Self::inference_substitution(&variables))
            });
            let (actual, _) = self.check_expr(
                argument,
                expected_argument.map_or(ExpectedType::None, ExpectedType::Exact),
            );
            self.program.copies.insert(argument.span);
            if let Some(parameter) = parameter
                && !self.infer_against(parameter.ty, actual, &variables)
            {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        "this argument's type cannot satisfy the generic parameter type",
                    )
                    .with_primary(argument.span),
                );
            }
        }

        let Some(concrete_arguments) = self.finish_inference(&generic_parameters, &variables)
        else {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Call,
                    "generic arguments cannot be inferred uniquely; supply the complete argument list",
                )
                .with_primary(call_span),
            );
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let instance = FunctionInstance {
            declaration,
            arguments: concrete_arguments,
        };
        let Some(signature) = self.typed.instantiate_signature(self.resolved, &instance) else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        if let ExpectedType::Exact(expected) = expected
            && !self.types_compatible(signature.return_type, expected)
        {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ExpressionType,
                    "this call's result does not match the expected type",
                )
                .with_primary(call_span),
            );
        }
        self.program
            .calls
            .insert(call_span, CheckedCall::Direct(instance));
        (signature.return_type, PlaceKind::Value)
    }

    fn report_call_arity(&mut self, span: Span, supplied: usize, fixed: usize, variadic: bool) {
        self.diagnostics.push(
            Diagnostic::new(
                Category::Call,
                format!(
                    "this call supplies {} argument{}, but the function requires {}{}",
                    supplied,
                    if supplied == 1 { "" } else { "s" },
                    if variadic { "at least " } else { "" },
                    fixed,
                ),
            )
            .with_primary(span),
        );
    }

    /// The span of the identifier a call or construction callee ultimately
    /// names: the sole token of a `NameExpression`, or the trailing member
    /// token of a `MemberExpression` whose base already resolved to a module
    /// path in Milestone 4.
    fn callee_target_span(node: &SyntaxNode) -> Option<Span> {
        match node.kind {
            SyntaxKind::NameExpression => node.children.iter().find_map(|child| match child {
                SyntaxElement::Token(token) => Some(token.span),
                SyntaxElement::Node(_) => None,
            }),
            SyntaxKind::MemberExpression => {
                node.children.iter().rev().find_map(|child| match child {
                    SyntaxElement::Token(token)
                        if matches!(token.kind, TokenKind::Identifier(_)) =>
                    {
                        Some(token.span)
                    }
                    _ => None,
                })
            }
            SyntaxKind::BracketExpression => child_nodes(node)
                .into_iter()
                .next()
                .and_then(Self::callee_target_span),
            _ => None,
        }
    }

    fn find_member(&self, declaration: DeclarationId, name: &str) -> Option<MemberId> {
        self.resolved
            .declaration_members
            .get(&declaration)?
            .iter()
            .find(|(symbol, _)| self.resolved.symbol_text(**symbol) == name)
            .map(|(_, member)| *member)
    }

    fn type_declaration_expression(&self, node: &SyntaxNode) -> Option<DeclarationId> {
        let span = Self::callee_target_span(node)?;
        match self.resolved.reference_at(span)?.target {
            NameTarget::Item(crate::resolution::ItemId::Declaration(declaration))
                if matches!(
                    self.resolved.declarations[declaration.index()].kind,
                    DeclarationKind::Struct
                        | DeclarationKind::Enum
                        | DeclarationKind::ForeignStruct
                ) =>
            {
                Some(declaration)
            }
            NameTarget::SelfType => self.current_self_declaration,
            _ => None,
        }
    }

    fn resolve_enum_variant_callee(&self, node: &SyntaxNode) -> Option<(DeclarationId, VariantId)> {
        if node.kind != SyntaxKind::MemberExpression {
            return None;
        }
        let base = child_nodes(node).into_iter().next()?;
        let base_span = Self::callee_target_span(base)?;
        let base_target = self.resolved.reference_at(base_span)?.target;
        let NameTarget::Item(crate::resolution::ItemId::Declaration(enum_declaration)) =
            base_target
        else {
            return None;
        };
        if self.resolved.declarations[enum_declaration.index()].kind != DeclarationKind::Enum {
            return None;
        }
        let member_token = node.children.iter().rev().find_map(|child| match child {
            SyntaxElement::Token(token) if matches!(token.kind, TokenKind::Identifier(_)) => {
                Some(token)
            }
            _ => None,
        })?;
        match self.find_member(enum_declaration, &token_text(member_token)) {
            Some(MemberId::Variant(variant)) => Some((enum_declaration, variant)),
            _ => None,
        }
    }

    fn check_member_expression(
        &mut self,
        node: &SyntaxNode,
        expected: ExpectedType,
    ) -> (TypeId, PlaceKind) {
        if let Some((enum_declaration, _)) = self.resolve_enum_variant_callee(node) {
            let explicit = self.enum_variant_arguments(node);
            let (parameters, variables) = match self.initial_nominal_inference(
                node.span,
                enum_declaration,
                explicit,
                expected,
            ) {
                Some(inference) => inference,
                None => return (self.typed.types.error(), PlaceKind::Value),
            };
            let ty = self
                .finish_nominal_inference(node.span, &parameters, &variables)
                .and_then(|arguments| {
                    self.typed.instantiate_declaration_type(
                        self.resolved,
                        enum_declaration,
                        &arguments,
                    )
                })
                .unwrap_or_else(|| self.typed.types.error());
            return (ty, PlaceKind::Value);
        }
        let Some(base) = child_nodes(node).into_iter().next() else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let member_token = node.children.iter().rev().find_map(|child| match child {
            SyntaxElement::Token(token) if matches!(token.kind, TokenKind::Identifier(_)) => {
                Some(token)
            }
            _ => None,
        });
        let Some(member_token) = member_token else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        if let Some(reference) = self.resolved.reference_at(member_token.span)
            && let NameTarget::Item(crate::resolution::ItemId::Declaration(declaration)) =
                reference.target
            && matches!(
                self.resolved.declarations[declaration.index()].kind,
                DeclarationKind::Function | DeclarationKind::ForeignFunction
            )
            && self.resolved.declarations[declaration.index()]
                .generic_parameters
                .is_empty()
        {
            let instance = FunctionInstance {
                declaration,
                arguments: Vec::new(),
            };
            let ty = self.function_reference_type(&instance);
            self.program.function_references.insert(node.span, instance);
            return (ty, PlaceKind::Value);
        }
        if let Some(declaration) = self.type_declaration_expression(base) {
            return match self.find_member(declaration, &token_text(member_token)) {
                Some(MemberId::Method(method))
                    if self.resolved.declarations[method.index()]
                        .generic_parameters
                        .is_empty() =>
                {
                    let instance = FunctionInstance {
                        declaration: method,
                        arguments: Vec::new(),
                    };
                    let ty = self.function_reference_type(&instance);
                    self.program.function_references.insert(node.span, instance);
                    (ty, PlaceKind::Value)
                }
                _ => (self.typed.types.error(), PlaceKind::Value),
            };
        }
        let (base_type, base_place) = self.check_expr(base, ExpectedType::None);
        let resolved_base = self.typed.types.resolve_inference(base_type);
        let (declaration, owner_arguments, field_place) =
            match self.typed.types.kind(resolved_base).clone() {
                TypeKind::Nominal {
                    identity,
                    arguments,
                } => (identity.declaration, arguments, base_place),
                TypeKind::Reference { mutability, target } => {
                    let target = self.typed.types.resolve_inference(target);
                    match self.typed.types.kind(target).clone() {
                        TypeKind::Nominal {
                            identity,
                            arguments,
                        } => {
                            let place = if mutability == Mutability::Mutable {
                                PlaceKind::Mutable
                            } else {
                                PlaceKind::Addressable
                            };
                            (identity.declaration, arguments, place)
                        }
                        _ => return (self.typed.types.error(), PlaceKind::Value),
                    }
                }
                TypeKind::Error => return (self.typed.types.error(), PlaceKind::Value),
                _ => return (self.typed.types.error(), PlaceKind::Value),
            };
        match self.find_member(declaration, &token_text(member_token)) {
            Some(MemberId::Field(field_id)) => {
                let field_type = self
                    .typed
                    .instantiate_field_type(self.resolved, field_id, &owner_arguments)
                    .unwrap_or_else(|| self.typed.types.error());
                let place = match field_place {
                    PlaceKind::Mutable | PlaceKind::CollectionInterior => PlaceKind::Mutable,
                    PlaceKind::Addressable => PlaceKind::Addressable,
                    PlaceKind::Value => PlaceKind::Value,
                };
                (field_type, place)
            }
            Some(MemberId::Method(method))
                if self
                    .typed
                    .function_signatures
                    .get(&method)
                    .is_some_and(|signature| signature.receiver.is_some()) =>
            {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Call,
                        "an instance method cannot be used as a bound-method value; call it \
                         directly or select the unbound method from its type",
                    )
                    .with_primary(node.span),
                );
                (self.typed.types.error(), PlaceKind::Value)
            }
            _ => (self.typed.types.error(), PlaceKind::Value),
        }
    }

    fn is_integer_type(&self, ty: TypeId) -> bool {
        self.typed
            .types
            .expanded_primitive(ty)
            .is_some_and(PrimitiveType::is_integer)
    }

    fn is_float_type(&self, ty: TypeId) -> bool {
        self.typed
            .types
            .expanded_primitive(ty)
            .is_some_and(PrimitiveType::is_float)
    }

    fn is_numeric_type(&self, ty: TypeId) -> bool {
        self.is_integer_type(ty) || self.is_float_type(ty)
    }

    /// Whether a canonical type names a trait declaration, which may appear as
    /// a type only behind a safe reference (`SPEC.md` 6).
    fn is_trait_declaration_type(&self, ty: TypeId) -> bool {
        let resolved = self.typed.types.resolve_inference(ty);
        match self.typed.types.kind(resolved) {
            TypeKind::Nominal { identity, .. } => {
                self.resolved.declarations[identity.declaration.index()].kind
                    == DeclarationKind::Trait
            }
            _ => false,
        }
    }

    fn is_trait_object_reference(&self, ty: TypeId) -> bool {
        let resolved = self.typed.types.resolve_inference(ty);
        let target = match self.typed.types.kind(resolved) {
            TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => *target,
            _ => return false,
        };
        let target = self.typed.types.resolve_inference(target);
        matches!(self.typed.types.kind(target), TypeKind::TraitObject { .. })
    }

    /// Whether an expression of type `actual` may be used where `expected`
    /// is required. Milestone 6 requires exact type equality (`SPEC.md` has
    /// no implicit conversion), with one deliberate exception: coercing a
    /// concrete reference to a matching `&Trait`/`&var Trait` is Milestone 13
    /// territory, so it is accepted here rather than misdiagnosed as a type
    /// mismatch.
    fn types_compatible(&self, actual: TypeId, expected: TypeId) -> bool {
        actual == self.typed.types.error()
            || expected == self.typed.types.error()
            || self.typed.types.exactly_equal(actual, expected)
            || self.is_trait_object_reference(expected)
    }

    fn check_unary(&mut self, node: &SyntaxNode, expected: ExpectedType) -> (TypeId, PlaceKind) {
        let Some(operator) = node.children.first().and_then(|child| match child {
            SyntaxElement::Token(token) => Some(token),
            SyntaxElement::Node(_) => None,
        }) else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let operator_kind = operator.kind.clone();
        let Some(operand) = child_nodes(node).into_iter().next_back() else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        match operator_kind {
            TokenKind::Amp => {
                let mutable = node.children.iter().any(|child| {
                    matches!(
                        child,
                        SyntaxElement::Token(Token {
                            kind: TokenKind::Keyword(Keyword::Var),
                            ..
                        })
                    )
                });
                let (operand_type, operand_place) = self.check_expr(operand, ExpectedType::None);
                let composite_literal_exception = operand.kind == SyntaxKind::RecordExpression;
                // A collection interior is an assignable place but never a
                // safe-reference target (SPEC 3.2), so `permits_safe_reference`
                // gates both forms and `&var` additionally requires mutability.
                let allowed = composite_literal_exception
                    || (operand_place.permits_safe_reference()
                        && (!mutable || operand_place.is_mutable()));
                if !allowed && operand_type != self.typed.types.error() {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::Place,
                            if operand_place == PlaceKind::CollectionInterior {
                                "cannot form a safe reference to a collection interior"
                            } else if mutable {
                                "cannot form `&var` from a non-mutable place"
                            } else {
                                "cannot form a reference to a non-addressable expression"
                            },
                        )
                        .with_primary(node.span),
                    );
                }
                let mutability = if mutable {
                    Mutability::Mutable
                } else {
                    Mutability::Shared
                };
                let ty = self.typed.types.intern(TypeKind::Reference {
                    mutability,
                    target: operand_type,
                });
                (ty, PlaceKind::Value)
            }
            TokenKind::Star => {
                let (operand_type, _) = self.check_expr(operand, ExpectedType::None);
                let resolved = self.typed.types.resolve_inference(operand_type);
                match self.typed.types.kind(resolved).clone() {
                    TypeKind::Reference { mutability, target }
                    | TypeKind::RawPointer { mutability, target } => {
                        let place = if mutability == Mutability::Mutable {
                            PlaceKind::Mutable
                        } else {
                            PlaceKind::Addressable
                        };
                        (target, place)
                    }
                    TypeKind::Error => (self.typed.types.error(), PlaceKind::Value),
                    _ => {
                        self.diagnostics.push(
                            Diagnostic::new(
                                Category::ExpressionType,
                                "dereference requires a reference or raw-pointer type",
                            )
                            .with_primary(node.span),
                        );
                        (self.typed.types.error(), PlaceKind::Value)
                    }
                }
            }
            TokenKind::Bang => {
                let bool_type = self.typed.types.primitive(PrimitiveType::Bool);
                let (operand_type, _) = self.check_expr(operand, ExpectedType::Exact(bool_type));
                if operand_type != self.typed.types.error()
                    && !self.typed.types.exactly_equal(operand_type, bool_type)
                {
                    self.diagnostics.push(
                        Diagnostic::new(Category::ExpressionType, "`!` requires `bool`")
                            .with_primary(node.span),
                    );
                }
                (bool_type, PlaceKind::Value)
            }
            TokenKind::Tilde => {
                let (operand_type, _) = self.check_expr(operand, expected);
                if operand_type != self.typed.types.error() && !self.is_integer_type(operand_type) {
                    self.diagnostics.push(
                        Diagnostic::new(Category::ExpressionType, "`~` requires an integer type")
                            .with_primary(node.span),
                    );
                    return (self.typed.types.error(), PlaceKind::Value);
                }
                (operand_type, PlaceKind::Value)
            }
            TokenKind::Plus => {
                let (operand_type, _) = self.check_expr(operand, expected);
                if operand_type != self.typed.types.error() && !self.is_numeric_type(operand_type) {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "unary `+` requires a numeric type",
                        )
                        .with_primary(node.span),
                    );
                    return (self.typed.types.error(), PlaceKind::Value);
                }
                (operand_type, PlaceKind::Value)
            }
            TokenKind::Minus => {
                if operand.kind == SyntaxKind::LiteralExpression
                    && let Some(SyntaxElement::Token(token)) = operand.children.first()
                    && matches!(token.kind, TokenKind::IntegerLiteral { .. })
                {
                    let ty = self.literal_token_type(token, expected, true);
                    self.program.expression_types.insert(operand.span, ty);
                    self.program
                        .expression_places
                        .insert(operand.span, PlaceKind::Value);
                    return (ty, PlaceKind::Value);
                }
                let (operand_type, _) = self.check_expr(operand, expected);
                if operand_type != self.typed.types.error() && !self.is_numeric_type(operand_type) {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "unary `-` requires a numeric type",
                        )
                        .with_primary(node.span),
                    );
                    return (self.typed.types.error(), PlaceKind::Value);
                }
                (operand_type, PlaceKind::Value)
            }
            _ => (self.typed.types.error(), PlaceKind::Value),
        }
    }

    fn check_binary(&mut self, node: &SyntaxNode) -> (TypeId, PlaceKind) {
        let nodes = child_nodes(node);
        let Some(&left) = nodes.first() else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let Some(&right) = nodes.get(1) else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let Some(operator) = node.children.iter().find_map(|child| match child {
            SyntaxElement::Token(token) => Some(&token.kind),
            SyntaxElement::Node(_) => None,
        }) else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let bool_type = self.typed.types.primitive(PrimitiveType::Bool);
        if matches!(operator, TokenKind::AndAnd | TokenKind::OrOr) {
            self.check_expr(left, ExpectedType::Exact(bool_type));
            self.check_expr(right, ExpectedType::Exact(bool_type));
            return (bool_type, PlaceKind::Value);
        }
        let (left_type, _) = self.check_expr(left, ExpectedType::None);
        let right_expected = if left_type == self.typed.types.error() {
            ExpectedType::None
        } else {
            ExpectedType::Exact(left_type)
        };
        let (right_type, _) = self.check_expr(right, right_expected);
        let operands_match = left_type == self.typed.types.error()
            || right_type == self.typed.types.error()
            || self.typed.types.exactly_equal(left_type, right_type);
        if !operands_match {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ExpressionType,
                    "operands of this operator must have the same concrete type",
                )
                .with_primary(node.span),
            );
            return (self.typed.types.error(), PlaceKind::Value);
        }
        match operator {
            TokenKind::EqEq | TokenKind::NotEq => {
                if !self.generic_operation_is_bound(left_type, "PartialEq") {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::TypeSystem,
                            "comparison on a generic parameter requires a `PartialEq` bound",
                        )
                        .with_primary(node.span),
                    );
                    return (self.typed.types.error(), PlaceKind::Value);
                }
                (bool_type, PlaceKind::Value)
            }
            TokenKind::Less | TokenKind::LessEq | TokenKind::Greater | TokenKind::GreaterEq
                if self.function_value_signature(left_type).is_some() =>
            {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        "function references support only `==` and `!=` identity comparison",
                    )
                    .with_primary(node.span),
                );
                (self.typed.types.error(), PlaceKind::Value)
            }
            TokenKind::Less | TokenKind::LessEq | TokenKind::Greater | TokenKind::GreaterEq => {
                if !self.generic_operation_is_bound(left_type, "PartialOrd") {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::TypeSystem,
                            "ordering a generic parameter requires a `PartialOrd` bound",
                        )
                        .with_primary(node.span),
                    );
                    return (self.typed.types.error(), PlaceKind::Value);
                }
                (bool_type, PlaceKind::Value)
            }
            TokenKind::Plus | TokenKind::Minus | TokenKind::Star | TokenKind::Slash => {
                if left_type != self.typed.types.error() && !self.is_numeric_type(left_type) {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "arithmetic operators require a numeric type",
                        )
                        .with_primary(node.span),
                    );
                    return (self.typed.types.error(), PlaceKind::Value);
                }
                (left_type, PlaceKind::Value)
            }
            TokenKind::Percent => {
                if left_type != self.typed.types.error() && !self.is_integer_type(left_type) {
                    self.diagnostics.push(
                        Diagnostic::new(Category::ExpressionType, "`%` requires an integer type")
                            .with_primary(node.span),
                    );
                    return (self.typed.types.error(), PlaceKind::Value);
                }
                (left_type, PlaceKind::Value)
            }
            TokenKind::Amp
            | TokenKind::Pipe
            | TokenKind::Caret
            | TokenKind::Shl
            | TokenKind::Shr => {
                if left_type != self.typed.types.error() && !self.is_integer_type(left_type) {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "bitwise and shift operators require an integer type",
                        )
                        .with_primary(node.span),
                    );
                    return (self.typed.types.error(), PlaceKind::Value);
                }
                (left_type, PlaceKind::Value)
            }
            _ => (self.typed.types.error(), PlaceKind::Value),
        }
    }

    fn generic_operation_is_bound(&self, mut ty: TypeId, required: &str) -> bool {
        loop {
            ty = self.typed.types.resolve_inference(ty);
            match self.typed.types.kind(ty) {
                TypeKind::Alias { target, .. } => ty = *target,
                TypeKind::GenericParameter(parameter) => {
                    return self.typed.obligations_for(*parameter).any(|obligation| {
                        matches!(
                            self.typed.types.kind(obligation.trait_type),
                            TypeKind::Builtin { builtin, .. }
                                if {
                                    let bound = self.resolved.builtin_name(*builtin);
                                    bound == required
                                        || (required == "PartialEq"
                                            && matches!(bound, "Eq" | "Ord"))
                                        || (required == "PartialOrd" && bound == "Ord")
                                }
                        )
                    });
                }
                _ => return true,
            }
        }
    }

    fn check_cast(&mut self, node: &SyntaxNode) -> (TypeId, PlaceKind) {
        let nodes = child_nodes(node);
        let Some(&source) = nodes.first() else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let Some(&type_node) = nodes.get(1) else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let (source_type, _) = self.check_expr(source, ExpectedType::None);
        let target_type = self.lower_type(type_node);
        if source_type == self.typed.types.error() || target_type == self.typed.types.error() {
            return (target_type, PlaceKind::Value);
        }
        // Forming a trait object is an explicit conversion (`SPEC.md` 6):
        // `reference as &Trait` / `as &var Trait`. Verifying that the source
        // target type actually implements the trait, and that the trait is
        // object-safe, is Milestone 13 territory; the mutability and
        // reference-shape requirements are checked here.
        if self.is_trait_object_reference(target_type) {
            let source = self.typed.types.resolve_inference(source_type);
            match self.typed.types.kind(source) {
                TypeKind::Reference { mutability, .. } => {
                    let source_mutability = *mutability;
                    let target = self.typed.types.resolve_inference(target_type);
                    let target_mutability = match self.typed.types.kind(target) {
                        TypeKind::Reference { mutability, .. } => *mutability,
                        _ => source_mutability,
                    };
                    if source_mutability != target_mutability {
                        self.diagnostics.push(
                            Diagnostic::new(
                                Category::ExpressionType,
                                "a trait-object conversion must preserve reference mutability",
                            )
                            .with_primary(node.span),
                        );
                        return (self.typed.types.error(), PlaceKind::Value);
                    }
                    return (target_type, PlaceKind::Value);
                }
                _ => {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "a trait object is formed only from a safe reference",
                        )
                        .with_primary(node.span),
                    );
                    return (self.typed.types.error(), PlaceKind::Value);
                }
            }
        }
        let source_resolved = self.typed.types.resolve_inference(source_type);
        let target_resolved = self.typed.types.resolve_inference(target_type);
        if let (
            TypeKind::Reference {
                mutability: source_mutability,
                target: source_target,
            },
            TypeKind::RawPointer {
                mutability: target_mutability,
                target: target_target,
            },
        ) = (
            self.typed.types.kind(source_resolved),
            self.typed.types.kind(target_resolved),
        ) && source_mutability == target_mutability
            && self
                .typed
                .types
                .exactly_equal(*source_target, *target_target)
        {
            return (target_type, PlaceKind::Value);
        }
        if let (
            TypeKind::RawPointer {
                mutability: source_mutability,
                target: source_target,
            },
            TypeKind::Reference {
                mutability: target_mutability,
                target: target_target,
            },
        ) = (
            self.typed.types.kind(source_resolved),
            self.typed.types.kind(target_resolved),
        ) && self.unsafe_depth > 0
            && source_mutability == target_mutability
            && self
                .typed
                .types
                .exactly_equal(*source_target, *target_target)
        {
            return (target_type, PlaceKind::Value);
        }
        if !self.is_numeric_type(source_type) || !self.is_numeric_type(target_type) {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ExpressionType,
                    "`as` performs only explicit numeric conversion between numeric types",
                )
                .with_primary(node.span),
            );
            return (self.typed.types.error(), PlaceKind::Value);
        }
        (target_type, PlaceKind::Value)
    }

    fn check_try(&mut self, node: &SyntaxNode) -> (TypeId, PlaceKind) {
        // Full `?` propagation-target checking is Milestone 15; here the
        // operand is still checked and, when it is visibly `Result[T, E]`,
        // its success payload type is threaded through.
        if self.defer_depth > 0 {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ControlFlow,
                    "postfix `?` cannot appear inside a deferred statement",
                )
                .with_primary(node.span),
            );
        }
        let Some(operand) = child_nodes(node).into_iter().next() else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let (operand_type, _) = self.check_expr(operand, ExpectedType::None);
        let resolved = self.typed.types.resolve_inference(operand_type);
        match self.typed.types.kind(resolved).clone() {
            TypeKind::Builtin { builtin, arguments }
                if self.resolved.builtin_name(builtin) == "Result" && arguments.len() == 2 =>
            {
                (arguments[0], PlaceKind::Value)
            }
            _ => (self.typed.types.error(), PlaceKind::Value),
        }
    }

    fn check_parenthesized(
        &mut self,
        node: &SyntaxNode,
        expected: ExpectedType,
    ) -> (TypeId, PlaceKind) {
        match child_nodes(node).into_iter().next() {
            Some(inner) => self.check_expr(inner, expected),
            None => (
                self.typed.types.primitive(PrimitiveType::Unit),
                PlaceKind::Value,
            ),
        }
    }

    fn check_tuple(&mut self, node: &SyntaxNode, expected: ExpectedType) -> (TypeId, PlaceKind) {
        let elements = child_nodes(node);
        if elements.is_empty() {
            return (
                self.typed.types.primitive(PrimitiveType::Unit),
                PlaceKind::Value,
            );
        }
        let expected_elements = match expected {
            ExpectedType::Exact(ty) => match self
                .typed
                .types
                .kind(self.typed.types.resolve_inference(ty))
            {
                TypeKind::Tuple(members) if members.len() == elements.len() => {
                    Some(members.clone())
                }
                _ => None,
            },
            ExpectedType::None => None,
        };
        let mut element_types = Vec::with_capacity(elements.len());
        for (index, element) in elements.into_iter().enumerate() {
            let element_expected = expected_elements
                .as_ref()
                .map_or(ExpectedType::None, |members| {
                    ExpectedType::Exact(members[index])
                });
            let (ty, _) = self.check_expr(element, element_expected);
            self.program.copies.insert(element.span);
            element_types.push(ty);
        }
        (
            self.typed.types.intern(TypeKind::Tuple(element_types)),
            PlaceKind::Value,
        )
    }

    fn check_array(&mut self, node: &SyntaxNode, expected: ExpectedType) -> (TypeId, PlaceKind) {
        let elements = child_nodes(node);
        let expected_element = match expected {
            ExpectedType::Exact(ty) => match self
                .typed
                .types
                .kind(self.typed.types.resolve_inference(ty))
            {
                TypeKind::Array { element, .. } | TypeKind::Slice(element) => Some(*element),
                _ => None,
            },
            ExpectedType::None => None,
        };
        if elements.is_empty() {
            let Some(element_type) = expected_element else {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        "an empty array literal requires an expected array type",
                    )
                    .with_primary(node.span),
                );
                return (self.typed.types.error(), PlaceKind::Value);
            };
            return (
                self.typed.types.intern(TypeKind::Array {
                    element: element_type,
                    length: 0,
                }),
                PlaceKind::Value,
            );
        }
        let mut element_type = None;
        for element in &elements {
            let per_element_expected =
                expected_element.map_or(ExpectedType::None, ExpectedType::Exact);
            let (ty, _) = self.check_expr(element, per_element_expected);
            self.program.copies.insert(element.span);
            match element_type {
                None if ty != self.typed.types.error() => element_type = Some(ty),
                Some(previous)
                    if ty != self.typed.types.error() && previous != self.typed.types.error() =>
                {
                    if !self.typed.types.exactly_equal(previous, ty) {
                        self.diagnostics.push(
                            Diagnostic::new(
                                Category::ExpressionType,
                                "every array element must have the same exact type",
                            )
                            .with_primary(element.span),
                        );
                    }
                }
                _ => {}
            }
        }
        let element_type = element_type
            .or(expected_element)
            .unwrap_or_else(|| self.typed.types.error());
        (
            self.typed.types.intern(TypeKind::Array {
                element: element_type,
                length: elements.len() as u128,
            }),
            PlaceKind::Value,
        )
    }

    fn check_macro(&mut self, node: &SyntaxNode) -> (TypeId, PlaceKind) {
        let Some(name_token) = node.children.iter().find_map(|child| match child {
            SyntaxElement::Token(token) if matches!(token.kind, TokenKind::Identifier(_)) => {
                Some(token)
            }
            _ => None,
        }) else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let name = token_text(name_token);
        let elements = child_nodes(node);
        match name.as_str() {
            "vec" | "set" => {
                let Some(builtin) = self.resolved.builtin_named(&name.to_uppercase_first()) else {
                    for element in &elements {
                        self.check_expr(element, ExpectedType::None);
                    }
                    return (self.typed.types.error(), PlaceKind::Value);
                };
                let mut element_type = None;
                for element in &elements {
                    let (ty, _) = self.check_expr(element, ExpectedType::None);
                    self.program.copies.insert(element.span);
                    if element_type.is_none() && ty != self.typed.types.error() {
                        element_type = Some(ty);
                    }
                }
                let argument = element_type.unwrap_or_else(|| self.typed.types.error());
                (
                    self.typed.types.intern(TypeKind::Builtin {
                        builtin,
                        arguments: vec![argument],
                    }),
                    PlaceKind::Value,
                )
            }
            "map" => {
                let Some(builtin) = self.resolved.builtin_named("Map") else {
                    for element in &elements {
                        self.check_expr(element, ExpectedType::None);
                    }
                    return (self.typed.types.error(), PlaceKind::Value);
                };
                let mut key_type = None;
                let mut value_type = None;
                for pair in elements.chunks(2) {
                    if let [key, value] = pair {
                        let (kt, _) = self.check_expr(key, ExpectedType::None);
                        self.program.copies.insert(key.span);
                        let (vt, _) = self.check_expr(value, ExpectedType::None);
                        self.program.copies.insert(value.span);
                        if key_type.is_none() && kt != self.typed.types.error() {
                            key_type = Some(kt);
                        }
                        if value_type.is_none() && vt != self.typed.types.error() {
                            value_type = Some(vt);
                        }
                    }
                }
                let key = key_type.unwrap_or_else(|| self.typed.types.error());
                let value = value_type.unwrap_or_else(|| self.typed.types.error());
                (
                    self.typed.types.intern(TypeKind::Builtin {
                        builtin,
                        arguments: vec![key, value],
                    }),
                    PlaceKind::Value,
                )
            }
            _ => (self.typed.types.error(), PlaceKind::Value),
        }
    }

    fn check_index(&mut self, node: &SyntaxNode) -> (TypeId, PlaceKind) {
        let nodes = child_nodes(node);
        let Some(&base) = nodes.first() else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        // More than one bracketed expression is an explicit generic
        // type-argument list (Milestone 12), not indexing.
        if nodes.len() != 2 {
            self.check_expr(base, ExpectedType::None);
            for extra in &nodes[1..] {
                self.check_expr(extra, ExpectedType::None);
            }
            return (self.typed.types.error(), PlaceKind::Value);
        }
        let index = nodes[1];
        let (base_type, base_place) = self.check_expr(base, ExpectedType::None);
        let resolved_base = self.typed.types.resolve_inference(base_type);
        let usize_type = self.typed.types.primitive(PrimitiveType::Usize);
        let result = match self.typed.types.kind(resolved_base).clone() {
            TypeKind::Array { element, .. } | TypeKind::Slice(element) => {
                self.check_expr(index, ExpectedType::Exact(usize_type));
                Some(element)
            }
            TypeKind::Builtin { builtin, arguments } => match self.resolved.builtin_name(builtin) {
                "Vec" if arguments.len() == 1 => {
                    self.check_expr(index, ExpectedType::Exact(usize_type));
                    Some(arguments[0])
                }
                "Map" if arguments.len() == 2 => {
                    self.check_expr(index, ExpectedType::Exact(arguments[0]));
                    Some(arguments[1])
                }
                "Set" => {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "`Set` has no indexing operation",
                        )
                        .with_primary(node.span),
                    );
                    self.check_expr(index, ExpectedType::None);
                    None
                }
                _ => {
                    self.check_expr(index, ExpectedType::None);
                    None
                }
            },
            TypeKind::Error => {
                self.check_expr(index, ExpectedType::None);
                None
            }
            _ => {
                self.check_expr(index, ExpectedType::None);
                self.diagnostics.push(
                    Diagnostic::new(Category::ExpressionType, "this type cannot be indexed")
                        .with_primary(node.span),
                );
                None
            }
        };
        match result {
            Some(element_type) => {
                let place = if base_place.is_mutable() {
                    PlaceKind::CollectionInterior
                } else {
                    PlaceKind::Value
                };
                (element_type, place)
            }
            None => (self.typed.types.error(), PlaceKind::Value),
        }
    }

    fn check_call(&mut self, node: &SyntaxNode, expected: ExpectedType) -> (TypeId, PlaceKind) {
        let nodes = child_nodes(node);
        let Some(&callee) = nodes.first() else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let arguments = &nodes[1..];

        if let Some((declaration, generic_arguments)) = self.explicit_function_selection(callee) {
            return self.check_function_call(
                node.span,
                declaration,
                Some(generic_arguments),
                arguments,
                expected,
                false,
            );
        }

        if let Some((enum_declaration, variant)) = self.resolve_enum_variant_callee(callee) {
            let explicit = self.enum_variant_arguments(callee);
            return self.check_variant_tuple_construction(
                node.span,
                enum_declaration,
                variant,
                arguments,
                explicit,
                expected,
            );
        }

        if callee.kind == SyntaxKind::MemberExpression
            && let Some(result) = self.check_member_call(node.span, callee, arguments, expected)
        {
            return result;
        }

        if let Some(span) = Self::callee_target_span(callee)
            && let Some(reference) = self.resolved.reference_at(span)
        {
            match reference.target {
                NameTarget::Item(crate::resolution::ItemId::Declaration(declaration_id)) => {
                    let declaration = &self.resolved.declarations[declaration_id.index()];
                    if matches!(
                        declaration.kind,
                        DeclarationKind::Function | DeclarationKind::ForeignFunction
                    ) {
                        return self.check_function_call(
                            node.span,
                            declaration_id,
                            None,
                            arguments,
                            expected,
                            false,
                        );
                    }
                }
                NameTarget::Item(crate::resolution::ItemId::Builtin(builtin_id))
                    if matches!(self.resolved.builtin_name(builtin_id), "print" | "println") =>
                {
                    self.program.calls.insert(
                        node.span,
                        CheckedCall::Print {
                            newline: self.resolved.builtin_name(builtin_id) == "println",
                        },
                    );
                    if arguments.len() != 1 {
                        self.diagnostics.push(
                            Diagnostic::new(
                                Category::Call,
                                format!(
                                    "`{}` expects exactly 1 argument, but {} were supplied",
                                    self.resolved.builtin_name(builtin_id),
                                    arguments.len()
                                ),
                            )
                            .with_primary(node.span),
                        );
                    }
                    for argument in arguments {
                        self.check_expr(argument, ExpectedType::None);
                        self.program.copies.insert(argument.span);
                    }
                    return (
                        self.typed.types.primitive(PrimitiveType::Unit),
                        PlaceKind::Value,
                    );
                }
                _ => {}
            }
        }

        let (callee_type, _) = self.check_expr(callee, ExpectedType::None);
        if let Some((parameters, return_type)) = self.function_value_signature(callee_type) {
            self.program.calls.insert(node.span, CheckedCall::Indirect);
            self.check_call_arguments(node.span, &parameters, arguments);
            return (return_type, PlaceKind::Value);
        }
        for argument in arguments {
            self.check_expr(argument, ExpectedType::None);
        }
        if callee_type != self.typed.types.error() {
            self.diagnostics.push(
                Diagnostic::new(Category::Call, "this expression is not callable")
                    .with_primary(callee.span),
            );
        }
        (self.typed.types.error(), PlaceKind::Value)
    }

    fn check_member_call(
        &mut self,
        call_span: Span,
        callee: &SyntaxNode,
        arguments: &[&SyntaxNode],
        expected: ExpectedType,
    ) -> Option<(TypeId, PlaceKind)> {
        let base = child_nodes(callee).into_iter().next()?;
        let member_token = callee.children.iter().rev().find_map(|child| match child {
            SyntaxElement::Token(token) if matches!(token.kind, TokenKind::Identifier(_)) => {
                Some(token)
            }
            _ => None,
        })?;
        let member_name = token_text(member_token);

        // `Type.member(...)` is an ordinary call of the selected associated
        // function or unbound method. A receiver-bearing method therefore
        // expects its receiver as the first explicit argument.
        if let Some((owner, owner_arguments)) = self.nominal_selection(base) {
            let Some(MemberId::Method(method)) = self.find_member(owner, &member_name) else {
                return None;
            };
            return Some(self.check_function_call(
                call_span,
                method,
                owner_arguments,
                arguments,
                expected,
                true,
            ));
        }

        let (base_type, base_place) = self.check_expr(base, ExpectedType::None);
        let (owner, _) = self.receiver_owner(base_type)?;
        match self.find_member(owner, &member_name) {
            // Field lookup has precedence over method lookup. Calling it is an
            // ordinary indirect call and performs no receiver adaptation.
            Some(MemberId::Field(field)) => {
                let field_type = self
                    .typed
                    .field_types
                    .get(&field)
                    .copied()
                    .unwrap_or_else(|| self.typed.types.error());
                self.program
                    .expression_types
                    .insert(callee.span, field_type);
                self.program
                    .expression_places
                    .insert(callee.span, base_place);
                if let Some((parameters, return_type)) = self.function_value_signature(field_type) {
                    self.program.calls.insert(call_span, CheckedCall::Indirect);
                    self.check_call_arguments(call_span, &parameters, arguments);
                    Some((return_type, PlaceKind::Value))
                } else {
                    for argument in arguments {
                        self.check_expr(argument, ExpectedType::None);
                    }
                    if field_type != self.typed.types.error() {
                        self.diagnostics.push(
                            Diagnostic::new(
                                Category::Call,
                                format!("field `{member_name}` is not callable"),
                            )
                            .with_primary(member_token.span),
                        );
                    }
                    Some((self.typed.types.error(), PlaceKind::Value))
                }
            }
            Some(MemberId::Method(method)) => {
                let template = self.typed.function_signatures.get(&method)?.clone();
                let Some(template_receiver) = template.receiver else {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::Call,
                            format!(
                                "associated function `{member_name}` must be selected from its type"
                            ),
                        )
                        .with_primary(member_token.span),
                    );
                    for argument in arguments {
                        self.check_expr(argument, ExpectedType::None);
                    }
                    return Some((self.typed.types.error(), PlaceKind::Value));
                };
                let generic_parameters = self.callable_parameters(method);
                let variables = self.generic_inference_variables(&generic_parameters);
                let template_self = match self.typed.types.kind(template_receiver).clone() {
                    TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => {
                        target
                    }
                    _ => template_receiver,
                };
                let actual_self = self
                    .receiver_owner(base_type)
                    .map_or(base_type, |(_, owner)| owner);
                self.infer_against(template_self, actual_self, &variables);
                if let ExpectedType::Exact(expected) = expected {
                    self.infer_against(template.return_type, expected, &variables);
                }
                let variadic = template
                    .parameters
                    .last()
                    .is_some_and(|parameter| parameter.variadic);
                let fixed = template.parameters.len() - usize::from(variadic);
                if (!variadic && arguments.len() != fixed) || (variadic && arguments.len() < fixed)
                {
                    self.report_call_arity(call_span, arguments.len(), fixed, variadic);
                }
                for (index, argument) in arguments.iter().enumerate() {
                    let parameter = if index < fixed {
                        template.parameters.get(index)
                    } else {
                        template.parameters.last().filter(|_| variadic)
                    };
                    let expected_argument = parameter.map(|parameter| {
                        self.typed
                            .types
                            .substitute(parameter.ty, &Self::inference_substitution(&variables))
                    });
                    let (actual, _) = self.check_expr(
                        argument,
                        expected_argument.map_or(ExpectedType::None, ExpectedType::Exact),
                    );
                    self.program.copies.insert(argument.span);
                    if let Some(parameter) = parameter
                        && !self.infer_against(parameter.ty, actual, &variables)
                    {
                        self.diagnostics.push(
                            Diagnostic::new(
                                Category::ExpressionType,
                                "this argument's type cannot satisfy the generic method parameter",
                            )
                            .with_primary(argument.span),
                        );
                    }
                }
                let Some(concrete_arguments) =
                    self.finish_inference(&generic_parameters, &variables)
                else {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::Call,
                            "generic method arguments cannot be inferred uniquely",
                        )
                        .with_primary(call_span),
                    );
                    return Some((self.typed.types.error(), PlaceKind::Value));
                };
                let instance = FunctionInstance {
                    declaration: method,
                    arguments: concrete_arguments,
                };
                let signature = self.typed.instantiate_signature(self.resolved, &instance)?;
                let receiver_type = signature.receiver?;
                let Some(adjustment) =
                    self.check_receiver_adjustment(base, base_type, base_place, receiver_type)
                else {
                    for argument in arguments {
                        self.check_expr(argument, ExpectedType::None);
                    }
                    return Some((self.typed.types.error(), PlaceKind::Value));
                };
                if matches!(
                    adjustment,
                    ReceiverAdjustment::CopyValue | ReceiverAdjustment::DereferenceAndCopy
                ) {
                    self.program.copies.insert(base.span);
                }
                self.program.calls.insert(
                    call_span,
                    CheckedCall::BoundMethod {
                        instance,
                        adjustment,
                    },
                );
                Some((signature.return_type, PlaceKind::Value))
            }
            _ => None,
        }
    }

    fn receiver_owner(&self, ty: TypeId) -> Option<(DeclarationId, TypeId)> {
        let ty = self.typed.types.resolve_inference(ty);
        match self.typed.types.kind(ty) {
            TypeKind::Nominal { identity, .. } => Some((identity.declaration, ty)),
            TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => {
                let target = self.typed.types.resolve_inference(*target);
                match self.typed.types.kind(target) {
                    TypeKind::Nominal { identity, .. } => Some((identity.declaration, target)),
                    _ => None,
                }
            }
            TypeKind::Alias { target, .. } => self.receiver_owner(*target),
            _ => None,
        }
    }

    fn check_receiver_adjustment(
        &mut self,
        base: &SyntaxNode,
        actual: TypeId,
        place: PlaceKind,
        expected: TypeId,
    ) -> Option<ReceiverAdjustment> {
        let actual = self.typed.types.resolve_inference(actual);
        let expected = self.typed.types.resolve_inference(expected);
        let (_, self_type) = self.receiver_owner(expected)?;
        let adjustment = match self.typed.types.kind(expected).clone() {
            TypeKind::Nominal { .. } => match self.typed.types.kind(actual).clone() {
                TypeKind::Nominal { .. } if self.typed.types.exactly_equal(actual, self_type) => {
                    Some(ReceiverAdjustment::CopyValue)
                }
                TypeKind::Reference { target, .. }
                    if self.typed.types.exactly_equal(target, self_type) =>
                {
                    Some(ReceiverAdjustment::DereferenceAndCopy)
                }
                _ => None,
            },
            TypeKind::Reference { mutability, target } => {
                match self.typed.types.kind(actual).clone() {
                    TypeKind::Nominal { .. } if self.typed.types.exactly_equal(actual, target) => {
                        if mutability == Mutability::Mutable {
                            place
                                .is_mutable()
                                .then_some(ReceiverAdjustment::BorrowMutable)
                        } else {
                            place
                                .permits_safe_reference()
                                .then_some(ReceiverAdjustment::BorrowShared)
                        }
                    }
                    TypeKind::Reference {
                        mutability: actual_mutability,
                        target: actual_target,
                    } if self.typed.types.exactly_equal(actual_target, target)
                        && (actual_mutability == mutability
                            || (actual_mutability == Mutability::Mutable
                                && mutability == Mutability::Shared)) =>
                    {
                        Some(ReceiverAdjustment::Pass)
                    }
                    _ => None,
                }
            }
            TypeKind::RawPointer { .. } if self.typed.types.exactly_equal(actual, expected) => {
                Some(ReceiverAdjustment::Pass)
            }
            _ => None,
        };
        if adjustment.is_none() {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Call,
                    "this value cannot satisfy the method's declared receiver type",
                )
                .with_primary(base.span),
            );
        }
        adjustment
    }

    fn function_value_signature(&self, ty: TypeId) -> Option<(Vec<FunctionParameter>, TypeId)> {
        let ty = self.typed.types.resolve_inference(ty);
        let target = match self.typed.types.kind(ty) {
            TypeKind::Reference { target, .. } => self.typed.types.resolve_inference(*target),
            TypeKind::Alias { target, .. } => {
                return self.function_value_signature(*target);
            }
            _ => return None,
        };
        match self.typed.types.kind(target) {
            TypeKind::Function {
                parameters,
                return_type,
                ..
            } => Some((parameters.clone(), *return_type)),
            _ => None,
        }
    }

    fn check_call_arguments(
        &mut self,
        call_span: Span,
        parameters: &[FunctionParameter],
        arguments: &[&SyntaxNode],
    ) {
        let variadic = parameters
            .last()
            .is_some_and(|parameter| parameter.variadic);
        let fixed = if variadic {
            parameters.len() - 1
        } else {
            parameters.len()
        };
        let arity_ok = if variadic {
            arguments.len() >= fixed
        } else {
            arguments.len() == fixed
        };
        if !arity_ok {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Call,
                    format!(
                        "this call supplies {} argument{}, but the function requires {}{}",
                        arguments.len(),
                        if arguments.len() == 1 { "" } else { "s" },
                        if variadic { "at least " } else { "" },
                        fixed,
                    ),
                )
                .with_primary(call_span),
            );
        }
        for (index, argument) in arguments.iter().enumerate() {
            let parameter_type = if index < fixed {
                Some(parameters[index].ty)
            } else {
                parameters.last().filter(|_| variadic).map(|p| p.ty)
            };
            let expected = parameter_type.map_or(ExpectedType::None, ExpectedType::Exact);
            let (argument_type, _) = self.check_expr(argument, expected);
            self.program.copies.insert(argument.span);
            if let Some(parameter_type) = parameter_type
                && !self.types_compatible(argument_type, parameter_type)
            {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        "this argument's type does not match the declared parameter type",
                    )
                    .with_primary(argument.span),
                );
            }
        }
    }

    fn check_variant_tuple_construction(
        &mut self,
        span: Span,
        enum_declaration: DeclarationId,
        variant: VariantId,
        arguments: &[&SyntaxNode],
        explicit: Option<Vec<TypeId>>,
        expected: ExpectedType,
    ) -> (TypeId, PlaceKind) {
        let field_ids = self.resolved.variants[variant.index()].fields.clone();
        if field_ids.len() != arguments.len() {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Construction,
                    format!(
                        "this variant has {} field{}, but {} value{} were supplied",
                        field_ids.len(),
                        if field_ids.len() == 1 { "" } else { "s" },
                        arguments.len(),
                        if arguments.len() == 1 { "" } else { "s" },
                    ),
                )
                .with_primary(span),
            );
        }
        let owner_arguments = self.infer_nominal_from_positional_fields(
            span,
            enum_declaration,
            explicit,
            expected,
            &field_ids,
            arguments,
        );
        let ty = owner_arguments
            .and_then(|arguments| {
                self.typed
                    .instantiate_declaration_type(self.resolved, enum_declaration, &arguments)
            })
            .unwrap_or_else(|| self.typed.types.error());
        (ty, PlaceKind::Value)
    }

    fn nominal_selection(
        &mut self,
        node: &SyntaxNode,
    ) -> Option<(DeclarationId, Option<Vec<TypeId>>)> {
        if node.kind == SyntaxKind::BracketExpression {
            let nodes = child_nodes(node);
            let base = *nodes.first()?;
            let span = Self::callee_target_span(base)?;
            let NameTarget::Item(crate::resolution::ItemId::Declaration(declaration)) =
                self.resolved.reference_at(span)?.target
            else {
                return None;
            };
            if !matches!(
                self.resolved.declarations[declaration.index()].kind,
                DeclarationKind::Struct | DeclarationKind::Enum | DeclarationKind::ForeignStruct
            ) {
                return None;
            }
            let arguments = nodes[1..]
                .iter()
                .map(|argument| self.type_from_expression(argument))
                .collect::<Option<Vec<_>>>()?;
            return Some((declaration, Some(arguments)));
        }
        self.type_declaration_expression(node)
            .map(|declaration| (declaration, None))
    }

    fn enum_variant_arguments(&mut self, node: &SyntaxNode) -> Option<Vec<TypeId>> {
        let base = child_nodes(node).into_iter().next()?;
        (base.kind == SyntaxKind::BracketExpression)
            .then(|| {
                self.nominal_selection(base)
                    .and_then(|(_, arguments)| arguments)
            })
            .flatten()
    }

    fn initial_nominal_inference(
        &mut self,
        span: Span,
        declaration: DeclarationId,
        explicit: Option<Vec<TypeId>>,
        expected: ExpectedType,
    ) -> Option<(
        Vec<GenericParameterId>,
        BTreeMap<GenericParameterId, TypeId>,
    )> {
        let parameters = self.resolved.declarations[declaration.index()]
            .generic_parameters
            .clone();
        if let Some(arguments) = explicit {
            if arguments.len() != parameters.len() {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Construction,
                        format!(
                            "this type requires {} generic argument{}, but {} were supplied",
                            parameters.len(),
                            if parameters.len() == 1 { "" } else { "s" },
                            arguments.len()
                        ),
                    )
                    .with_primary(span),
                );
                return None;
            }
            return Some((
                parameters.clone(),
                parameters.into_iter().zip(arguments).collect(),
            ));
        }
        let variables = self.generic_inference_variables(&parameters);
        if let ExpectedType::Exact(expected) = expected
            && let Some(template) = self.typed.declaration_types.get(&declaration).copied()
        {
            self.infer_against(template, expected, &variables);
        }
        Some((parameters, variables))
    }

    fn finish_nominal_inference(
        &mut self,
        span: Span,
        parameters: &[GenericParameterId],
        variables: &BTreeMap<GenericParameterId, TypeId>,
    ) -> Option<Vec<TypeId>> {
        let result = self.finish_inference(parameters, variables);
        if result.is_none() {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Construction,
                    "generic arguments cannot be inferred uniquely; supply the complete argument list",
                )
                .with_primary(span),
            );
        }
        result
    }

    fn infer_nominal_from_positional_fields(
        &mut self,
        span: Span,
        declaration: DeclarationId,
        explicit: Option<Vec<TypeId>>,
        expected: ExpectedType,
        fields: &[FieldId],
        values: &[&SyntaxNode],
    ) -> Option<Vec<TypeId>> {
        let (parameters, variables) =
            self.initial_nominal_inference(span, declaration, explicit, expected)?;
        for (index, value) in values.iter().enumerate() {
            let field_template = fields
                .get(index)
                .and_then(|field| self.typed.field_types.get(field))
                .copied();
            let expected_field = field_template.map(|template| {
                self.typed
                    .types
                    .substitute(template, &Self::inference_substitution(&variables))
            });
            let (actual, _) = self.check_expr(
                value,
                expected_field.map_or(ExpectedType::None, ExpectedType::Exact),
            );
            self.program.copies.insert(value.span);
            if let Some(template) = field_template
                && !self.infer_against(template, actual, &variables)
            {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        "this value's type does not match the variant's field type",
                    )
                    .with_primary(value.span),
                );
            }
        }
        self.finish_nominal_inference(span, &parameters, &variables)
    }

    fn check_record_expression(
        &mut self,
        node: &SyntaxNode,
        expected: ExpectedType,
    ) -> (TypeId, PlaceKind) {
        let nodes = child_nodes(node);
        let Some(&callee) = nodes.first() else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let fields = &nodes[1..];

        if let Some((declaration_id, explicit)) = self.nominal_selection(callee) {
            let declaration = &self.resolved.declarations[declaration_id.index()];
            if declaration.kind == DeclarationKind::Struct {
                let required: Vec<FieldId> = self
                    .resolved
                    .fields
                    .iter()
                    .filter(|field| {
                        field.parent_declaration == declaration_id && field.parent_variant.is_none()
                    })
                    .map(|field| field.id)
                    .collect();
                let ty = self
                    .infer_nominal_from_record_fields(
                        node.span,
                        declaration_id,
                        explicit,
                        expected,
                        fields,
                        &required,
                    )
                    .and_then(|arguments| {
                        self.typed.instantiate_declaration_type(
                            self.resolved,
                            declaration_id,
                            &arguments,
                        )
                    })
                    .unwrap_or_else(|| self.typed.types.error());
                return (ty, PlaceKind::Value);
            }
        }

        if let Some((enum_declaration, variant)) = self.resolve_enum_variant_callee(callee) {
            let explicit = self.enum_variant_arguments(callee);
            let required = self.resolved.variants[variant.index()].fields.clone();
            let ty = self
                .infer_nominal_from_record_fields(
                    node.span,
                    enum_declaration,
                    explicit,
                    expected,
                    fields,
                    &required,
                )
                .and_then(|arguments| {
                    self.typed.instantiate_declaration_type(
                        self.resolved,
                        enum_declaration,
                        &arguments,
                    )
                })
                .unwrap_or_else(|| self.typed.types.error());
            return (ty, PlaceKind::Value);
        }

        for field in fields {
            self.check_expr_lenient_record_field(field);
        }
        (self.typed.types.error(), PlaceKind::Value)
    }

    fn check_expr_lenient_record_field(&mut self, field_node: &SyntaxNode) {
        if let Some(expression) = child_nodes(field_node).into_iter().next() {
            self.check_expr(expression, ExpectedType::None);
        }
    }

    fn infer_nominal_from_record_fields(
        &mut self,
        whole_span: Span,
        declaration: DeclarationId,
        explicit: Option<Vec<TypeId>>,
        expected: ExpectedType,
        field_nodes: &[&SyntaxNode],
        required: &[FieldId],
    ) -> Option<Vec<TypeId>> {
        let (parameters, variables) =
            self.initial_nominal_inference(whole_span, declaration, explicit, expected)?;
        let mut seen: BTreeSet<FieldId> = BTreeSet::new();
        for field_node in field_nodes {
            let Some(name_token) = field_node.children.iter().find_map(|child| match child {
                SyntaxElement::Token(token) if matches!(token.kind, TokenKind::Identifier(_)) => {
                    Some(token)
                }
                _ => None,
            }) else {
                continue;
            };
            let name_text = token_text(name_token);
            let matched = required.iter().copied().find(|field_id| {
                self.resolved
                    .symbol_text(self.resolved.fields[field_id.index()].name)
                    == name_text
            });
            let Some(field_id) = matched else {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Construction,
                        format!("no field named `{name_text}` in this construction"),
                    )
                    .with_primary(name_token.span),
                );
                self.check_expr_lenient_record_field(field_node);
                continue;
            };
            if !seen.insert(field_id) {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Construction,
                        format!("field `{name_text}` is initialized more than once"),
                    )
                    .with_primary(name_token.span),
                );
            }
            let field_template = self
                .typed
                .field_types
                .get(&field_id)
                .copied()
                .unwrap_or_else(|| self.typed.types.error());
            let field_type = self
                .typed
                .types
                .substitute(field_template, &Self::inference_substitution(&variables));
            let (value_type, value_span) = match child_nodes(field_node).into_iter().next() {
                Some(expression) => (
                    self.check_expr(expression, ExpectedType::Exact(field_type))
                        .0,
                    expression.span,
                ),
                None => (
                    self.check_name_target(name_token.span, ExpectedType::None)
                        .0,
                    name_token.span,
                ),
            };
            self.program.copies.insert(value_span);
            if !self.infer_against(field_template, value_type, &variables) {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        format!("field `{name_text}` expects a different type"),
                    )
                    .with_primary(value_span),
                );
            }
        }
        for field_id in required {
            if !seen.contains(field_id) {
                let name = self
                    .resolved
                    .symbol_text(self.resolved.fields[field_id.index()].name);
                self.diagnostics.push(
                    Diagnostic::new(Category::Construction, format!("missing field `{name}`"))
                        .with_primary(whole_span),
                );
            }
        }
        self.finish_nominal_inference(whole_span, &parameters, &variables)
    }

    // ---------------------------------------------------------------
    // Reduced type-annotation lowering
    // ---------------------------------------------------------------

    /// Lowers a `Type` syntax node using names Milestone 4 already resolved.
    /// This intentionally supports the pre-trait subset of the full
    /// Milestone 5 lowering used for declared signatures.
    fn lower_type(&mut self, node: &SyntaxNode) -> TypeId {
        let tokens = types::direct_tokens(node);
        let first = tokens.first().map(|token| &token.kind);
        let error = self.typed.types.error();
        if matches!(first, Some(TokenKind::Amp | TokenKind::Star)) {
            let raw = matches!(first, Some(TokenKind::Star));
            let mutable = tokens
                .iter()
                .any(|token| matches!(token.kind, TokenKind::Keyword(Keyword::Var)));
            let mut target = match types::direct_child(node, SyntaxKind::Type) {
                Some(child)
                    if matches!(
                        types::direct_tokens(child).first().map(|token| &token.kind),
                        Some(TokenKind::Keyword(
                            Keyword::Fn | Keyword::Unsafe | Keyword::Extern
                        ))
                    ) =>
                {
                    self.lower_function_type_annotation(child)
                }
                Some(child) => self.lower_type(child),
                None => error,
            };
            if !raw
                && tokens
                    .iter()
                    .any(|token| matches!(token.kind, TokenKind::Keyword(Keyword::Unsafe)))
                && let TypeKind::Function {
                    abi,
                    receiver,
                    parameters,
                    return_type,
                    ..
                } = self.typed.types.kind(target).clone()
            {
                target = self.typed.types.intern(TypeKind::Function {
                    safety: Safety::Unsafe,
                    abi,
                    receiver,
                    parameters,
                    return_type,
                });
            }
            if target == error {
                return error;
            }
            // A trait names a type only behind a safe reference, where
            // `&Trait`/`&var Trait` denote a trait object (`SPEC.md` 6).
            if !raw && self.is_trait_declaration_type(target) {
                target = self
                    .typed
                    .types
                    .intern(TypeKind::TraitObject { trait_type: target });
            }
            let mutability = if mutable {
                Mutability::Mutable
            } else {
                Mutability::Shared
            };
            if !raw && mutable && matches!(self.typed.types.kind(target), TypeKind::Function { .. })
            {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::TypeSystem,
                        "function references cannot be mutable",
                    )
                    .with_primary(node.span),
                );
                return error;
            }
            return self.typed.types.intern(if raw {
                TypeKind::RawPointer { mutability, target }
            } else {
                TypeKind::Reference { mutability, target }
            });
        }
        if matches!(
            first,
            Some(TokenKind::Keyword(
                Keyword::Fn | Keyword::Unsafe | Keyword::Extern
            ))
        ) {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::TypeSystem,
                    "a bare function type has no value representation; use `&fn` or `&unsafe fn`",
                )
                .with_primary(node.span),
            );
            return error;
        }
        if matches!(first, Some(TokenKind::LBracket)) {
            let element = match types::direct_child(node, SyntaxKind::Type) {
                Some(child) => self.lower_type(child),
                None => error,
            };
            if element == error {
                return error;
            }
            if tokens
                .iter()
                .any(|token| matches!(token.kind, TokenKind::Semicolon))
            {
                let length = types::direct_child_not_type(node)
                    .and_then(types::array_length_literal)
                    .unwrap_or(0);
                return self.typed.types.intern(TypeKind::Array { element, length });
            }
            return self.typed.types.intern(TypeKind::Slice(element));
        }
        if matches!(first, Some(TokenKind::LParen)) {
            let elements: Vec<TypeId> = types::direct_children(node, SyntaxKind::Type)
                .into_iter()
                .map(|child| self.lower_type(child))
                .collect();
            if elements.is_empty() {
                return self.typed.types.primitive(PrimitiveType::Unit);
            }
            if elements.contains(&error) {
                return error;
            }
            let has_comma = tokens
                .iter()
                .any(|token| matches!(token.kind, TokenKind::Comma));
            if elements.len() == 1 && !has_comma {
                return elements[0];
            }
            return self.typed.types.intern(TypeKind::Tuple(elements));
        }
        let Some(span) = types::direct_path_span(node) else {
            return error;
        };
        let Some(reference) = self.resolved.reference_at(span) else {
            return self.typed.types.error();
        };
        match reference.target {
            NameTarget::GenericParameter(parameter) => self
                .typed
                .types
                .intern(TypeKind::GenericParameter(parameter)),
            NameTarget::Item(crate::resolution::ItemId::Declaration(declaration_id)) => {
                let arguments: Vec<TypeId> = types::direct_child(node, SyntaxKind::TypeArguments)
                    .map(|arguments| {
                        types::direct_children(arguments, SyntaxKind::Type)
                            .into_iter()
                            .map(|argument| self.lower_type(argument))
                            .collect()
                    })
                    .unwrap_or_default();
                if arguments.contains(&error) {
                    return error;
                }
                self.typed
                    .instantiate_declaration_type(self.resolved, declaration_id, &arguments)
                    .unwrap_or_else(|| self.typed.types.error())
            }
            NameTarget::Item(crate::resolution::ItemId::Builtin(builtin_id)) => {
                let name = self.resolved.builtin_name(builtin_id).to_string();
                if let Some(primitive) = types::primitive_from_name(&name) {
                    return self.typed.types.primitive(primitive);
                }
                let arguments: Vec<TypeId> = types::direct_child(node, SyntaxKind::TypeArguments)
                    .map(|arguments| {
                        types::direct_children(arguments, SyntaxKind::Type)
                            .into_iter()
                            .map(|argument| self.lower_type(argument))
                            .collect()
                    })
                    .unwrap_or_default();
                if arguments.contains(&error) {
                    return error;
                }
                self.typed.types.intern(TypeKind::Builtin {
                    builtin: builtin_id,
                    arguments,
                })
            }
            NameTarget::SelfType => self
                .current_self_declaration
                .and_then(|declaration| self.typed.declaration_types.get(&declaration).copied())
                .unwrap_or_else(|| self.typed.types.error()),
            _ => self.typed.types.error(),
        }
    }

    fn lower_function_type_annotation(&mut self, node: &SyntaxNode) -> TypeId {
        let tokens = types::direct_tokens(node);
        let safety = if tokens
            .iter()
            .any(|token| matches!(token.kind, TokenKind::Keyword(Keyword::Unsafe)))
        {
            Safety::Unsafe
        } else {
            Safety::Safe
        };
        let abi = if tokens
            .iter()
            .any(|token| matches!(token.kind, TokenKind::Keyword(Keyword::Extern)))
        {
            Abi::C
        } else {
            Abi::Elamite
        };
        let type_nodes = types::direct_children(node, SyntaxKind::Type);
        let return_type = match type_nodes.last() {
            Some(result) => self.lower_type(result),
            None => self.typed.types.error(),
        };
        let mut parameters = Vec::new();
        let mut type_position = 0usize;
        let mut variadic_next = false;
        for child in &node.children {
            match child {
                SyntaxElement::Token(Token {
                    kind: TokenKind::Ellipsis,
                    ..
                }) => variadic_next = true,
                SyntaxElement::Node(child) if child.kind == SyntaxKind::Type => {
                    type_position += 1;
                    if type_position < type_nodes.len() {
                        parameters.push(FunctionParameter {
                            ty: self.lower_type(child),
                            variadic: variadic_next,
                        });
                    }
                    variadic_next = false;
                }
                _ => {}
            }
        }
        self.typed.types.intern(TypeKind::Function {
            safety,
            abi,
            receiver: None,
            parameters,
            return_type,
        })
    }

    // ---------------------------------------------------------------
    // Containment cycles
    // ---------------------------------------------------------------

    fn check_containment_cycles(&mut self) {
        let nominal_declarations: Vec<DeclarationId> = self
            .resolved
            .declarations
            .iter()
            .filter(|declaration| {
                matches!(
                    declaration.kind,
                    DeclarationKind::Struct | DeclarationKind::Enum
                )
            })
            .map(|declaration| declaration.id)
            .collect();
        let mut edges: BTreeMap<DeclarationId, BTreeSet<DeclarationId>> = BTreeMap::new();
        for &declaration_id in &nominal_declarations {
            let mut targets = BTreeSet::new();
            for field in self
                .resolved
                .fields
                .iter()
                .filter(|field| field.parent_declaration == declaration_id)
            {
                if let Some(&ty) = self.typed.field_types.get(&field.id) {
                    self.collect_transparent_nominals(ty, &mut targets, &mut BTreeSet::new());
                }
            }
            edges.insert(declaration_id, targets);
        }
        let mut state: BTreeMap<DeclarationId, u8> = BTreeMap::new();
        let mut flagged: BTreeSet<DeclarationId> = BTreeSet::new();
        for &declaration_id in &nominal_declarations {
            if state.get(&declaration_id).copied().unwrap_or(0) == 0 {
                detect_cycle(declaration_id, &edges, &mut state, &mut flagged);
            }
        }
        for declaration_id in flagged {
            let span = self.resolved.declarations[declaration_id.index()].span;
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Containment,
                    "recursive struct/enum containment must cross an explicit safe reference or \
                     raw-pointer indirection",
                )
                .with_primary(span),
            );
        }
    }

    fn collect_transparent_nominals(
        &self,
        ty: TypeId,
        out: &mut BTreeSet<DeclarationId>,
        visiting: &mut BTreeSet<TypeId>,
    ) {
        let ty = self.typed.types.resolve_inference(ty);
        if !visiting.insert(ty) {
            return;
        }
        match self.typed.types.kind(ty).clone() {
            TypeKind::Nominal {
                identity,
                arguments,
            } => {
                out.insert(identity.declaration);
                for argument in arguments {
                    self.collect_transparent_nominals(argument, out, visiting);
                }
            }
            TypeKind::Builtin { arguments, .. } => {
                for argument in arguments {
                    self.collect_transparent_nominals(argument, out, visiting);
                }
            }
            TypeKind::Tuple(elements) => {
                for element in elements {
                    self.collect_transparent_nominals(element, out, visiting);
                }
            }
            TypeKind::Array { element, .. } | TypeKind::Slice(element) => {
                self.collect_transparent_nominals(element, out, visiting);
            }
            TypeKind::Alias { target, .. } => {
                self.collect_transparent_nominals(target, out, visiting);
            }
            _ => {}
        }
        visiting.remove(&ty);
    }
}

fn detect_cycle(
    node: DeclarationId,
    edges: &BTreeMap<DeclarationId, BTreeSet<DeclarationId>>,
    state: &mut BTreeMap<DeclarationId, u8>,
    flagged: &mut BTreeSet<DeclarationId>,
) {
    state.insert(node, 1);
    if let Some(targets) = edges.get(&node) {
        for &target in targets {
            match state.get(&target).copied().unwrap_or(0) {
                0 => detect_cycle(target, edges, state, flagged),
                1 => {
                    flagged.insert(node);
                    flagged.insert(target);
                }
                _ => {}
            }
        }
    }
    state.insert(node, 2);
}

fn is_assignment_operator(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Assign
            | TokenKind::PlusAssign
            | TokenKind::MinusAssign
            | TokenKind::StarAssign
            | TokenKind::SlashAssign
            | TokenKind::PercentAssign
            | TokenKind::AmpAssign
            | TokenKind::PipeAssign
            | TokenKind::CaretAssign
            | TokenKind::ShlAssign
            | TokenKind::ShrAssign
    )
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

fn parameter_name_token(node: &SyntaxNode) -> Option<&Token> {
    node.children.iter().find_map(|child| match child {
        SyntaxElement::Token(
            token @ Token {
                kind: TokenKind::Identifier(_) | TokenKind::Keyword(Keyword::SelfValue),
                ..
            },
        ) => Some(token),
        _ => None,
    })
}

fn let_name_token(node: &SyntaxNode) -> Option<&Token> {
    node.children
        .iter()
        .filter_map(|child| match child {
            SyntaxElement::Token(token) => Some(token),
            SyntaxElement::Node(_) => None,
        })
        .nth(1)
}

fn token_text(token: &Token) -> String {
    match &token.kind {
        TokenKind::Identifier(name) => name.clone(),
        TokenKind::Keyword(Keyword::SelfValue) => "self".to_string(),
        _ => String::new(),
    }
}

fn first_identifier_token(node: &SyntaxNode) -> Option<&Token> {
    node.children.iter().find_map(|child| match child {
        SyntaxElement::Token(token) if matches!(token.kind, TokenKind::Identifier(_)) => {
            Some(token)
        }
        _ => None,
    })
}

fn is_pattern_literal_token(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::IntegerLiteral { .. }
            | TokenKind::FloatLiteral { .. }
            | TokenKind::StringLiteral(_)
            | TokenKind::CharacterLiteral(_)
            | TokenKind::Keyword(Keyword::True | Keyword::False | Keyword::Null)
    )
}

/// The path tokens leading a `RecordPattern`/`VariantPattern`: everything
/// before the opening `(`/`{`. Mirrors `crate::resolution`'s private
/// `leading_pattern_path`, which Milestone 4 uses to resolve the same span.
fn leading_pattern_path(node: &SyntaxNode) -> Vec<&Token> {
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

/// Collects the names a pattern binds, approximating
/// `crate::resolution`'s private `collect_pattern_bindings` well enough to
/// compare binding sets across `|` alternatives (Milestone 7's "identical
/// binding names" rule).
fn collect_pattern_binding_names(pattern: &SyntaxNode, names: &mut BTreeSet<String>) {
    match pattern.kind {
        SyntaxKind::TuplePattern
        | SyntaxKind::AlternativePattern
        | SyntaxKind::DereferencePattern => {
            for child in child_nodes(pattern) {
                collect_pattern_binding_names(child, names);
            }
        }
        SyntaxKind::RecordPattern | SyntaxKind::VariantPattern => {
            for child in child_nodes(pattern) {
                if child.kind == SyntaxKind::PatternField {
                    let nested = child_nodes(child);
                    if nested.is_empty() {
                        if let Some(token) = first_identifier_token(child) {
                            let text = token_text(token);
                            if text != "_" {
                                names.insert(text);
                            }
                        }
                    } else {
                        for nested in nested {
                            collect_pattern_binding_names(nested, names);
                        }
                    }
                } else {
                    collect_pattern_binding_names(child, names);
                }
            }
        }
        SyntaxKind::Pattern => {
            let nested = child_nodes(pattern);
            if !nested.is_empty() {
                for child in nested {
                    collect_pattern_binding_names(child, names);
                }
                return;
            }
            let has_dot = pattern.children.iter().any(|child| {
                matches!(
                    child,
                    SyntaxElement::Token(Token {
                        kind: TokenKind::Dot,
                        ..
                    })
                )
            });
            if has_dot {
                return;
            }
            let identifiers: Vec<&Token> = pattern
                .children
                .iter()
                .filter_map(|child| match child {
                    SyntaxElement::Token(token)
                        if matches!(token.kind, TokenKind::Identifier(_)) =>
                    {
                        Some(token)
                    }
                    _ => None,
                })
                .collect();
            if let [token] = identifiers.as_slice() {
                let text = token_text(token);
                if text != "_" {
                    names.insert(text);
                }
            }
        }
        _ => {}
    }
}

/// What a pattern (or, for `|`, the union of its alternatives) covers, for
/// exhaustiveness and redundancy analysis.
#[derive(Debug, Clone)]
enum Coverage {
    /// No pattern has contributed yet (an empty `AlternativePattern`, which
    /// the parser never actually produces).
    None,
    /// Matches every value of the scrutinee's type: `_`, a plain binding,
    /// or a tuple/struct pattern built entirely from those.
    Catchall,
    /// Matches exactly these enum variants, unconditionally on their field
    /// values.
    Variants(BTreeSet<VariantId>),
    /// Matches named constructors of a compiler builtin ADT.
    BuiltinVariants(BTreeSet<String>),
    /// Matches exactly these `bool` literals.
    Bools(BTreeSet<bool>),
    /// Coverage after explicitly dereferencing one non-null safe reference.
    Dereferenced(Box<Coverage>),
    /// A pattern this module does not reason about precisely for coverage
    /// (a literal of an unbounded domain, or a tuple/struct with a
    /// refutable field); never counted as covering anything.
    Other,
}

impl Coverage {
    fn union(self, other: Coverage) -> Coverage {
        match (self, other) {
            (Coverage::None, other) => other,
            (this, Coverage::None) => this,
            (Coverage::Catchall, _) | (_, Coverage::Catchall) => Coverage::Catchall,
            (Coverage::Variants(mut a), Coverage::Variants(b)) => {
                a.extend(b);
                Coverage::Variants(a)
            }
            (Coverage::BuiltinVariants(mut a), Coverage::BuiltinVariants(b)) => {
                a.extend(b);
                Coverage::BuiltinVariants(a)
            }
            (Coverage::Bools(mut a), Coverage::Bools(b)) => {
                a.extend(b);
                Coverage::Bools(a)
            }
            (Coverage::Dereferenced(a), Coverage::Dereferenced(b)) => {
                Coverage::Dereferenced(Box::new(a.union(*b)))
            }
            _ => Coverage::Other,
        }
    }

    fn is_catchall(&self) -> bool {
        match self {
            Coverage::Catchall => true,
            Coverage::Dereferenced(inner) => inner.is_catchall(),
            _ => false,
        }
    }

    fn covers_variants(&self, variants: &BTreeSet<VariantId>) -> bool {
        match self {
            Coverage::Variants(covered) => variants.is_subset(covered),
            Coverage::Dereferenced(inner) => inner.covers_variants(variants),
            _ => false,
        }
    }

    fn covers_bools(&self, values: &BTreeSet<bool>) -> bool {
        match self {
            Coverage::Bools(covered) => values.is_subset(covered),
            Coverage::Dereferenced(inner) => inner.covers_bools(values),
            _ => false,
        }
    }

    fn covers_builtin_variants(&self, variants: &BTreeSet<String>) -> bool {
        match self {
            Coverage::BuiltinVariants(covered) => variants.is_subset(covered),
            Coverage::Dereferenced(inner) => inner.covers_builtin_variants(variants),
            _ => false,
        }
    }

    fn is_covered_by(&self, previous: &Coverage) -> bool {
        match self {
            Coverage::Variants(variants) => {
                !variants.is_empty() && previous.covers_variants(variants)
            }
            Coverage::Bools(values) => !values.is_empty() && previous.covers_bools(values),
            Coverage::BuiltinVariants(variants) => {
                !variants.is_empty() && previous.covers_builtin_variants(variants)
            }
            Coverage::Dereferenced(inner) => match previous {
                Coverage::Dereferenced(previous_inner) => inner.is_covered_by(previous_inner),
                _ => false,
            },
            Coverage::Catchall => previous.is_catchall(),
            Coverage::None | Coverage::Other => false,
        }
    }
}

trait UppercaseFirst {
    fn to_uppercase_first(&self) -> String;
}

impl UppercaseFirst for str {
    fn to_uppercase_first(&self) -> String {
        let mut chars = self.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        }
    }
}
