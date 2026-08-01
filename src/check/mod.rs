//! Core expression, function, method, and control-flow checker façade (`ROADMAP.md`
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
//! separate modules: `ROADMAP.md` assigns pattern typing to Milestone 7, but a
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
//! `for`-binding element typing (Milestone 14), and `?`/`defer`
//! propagation semantics (Milestone 15). Expressions that fall into those
//! areas are still walked (so nested diagnostics and copies are not lost)
//! but resolve to the type-system error type without an additional
//! diagnostic of their own. Left-to-right evaluation order and temporary
//! lifetimes (also Milestone 7) are satisfied by this module's single-pass,
//! source-order recursive-descent structure; materializing them as explicit
//! IR metadata is Milestone 8's job, since no IR exists yet.

mod calls;
mod containment;
mod coverage;
mod model;
mod patterns;

pub use model::*;

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostics::{Category, Diagnostic};
pub use crate::operations::{
    NumericAlternative, NumericOperator, NumericOutcome, ReceiverAdjustment, StandardCall,
};
use crate::resolution::{
    ClosureCaptureKind, DeclarationId, DeclarationKind, FieldId, GenericParameterId,
    LocalBindingId, MemberId, ModuleId, NameTarget, ResolvedProgram, VariantId, Visibility,
};
use crate::source::Span;
use crate::syntax::{
    Keyword, SyntaxElement, SyntaxKind, SyntaxNode, Token, TokenKind, child_nodes,
};
use crate::types::{
    self, ExpectedType, FunctionInstance, FunctionParameter, FunctionSignature, Mutability,
    PlaceKind, PrimitiveType, Safety, Substitution, TypeId, TypeKind, TypedProgram,
};
use containment::detect_cycle;
use coverage::Coverage;

/// Whether a checked local binding's root storage is a rebindable, mutable
/// place (`var`) or a non-rebindable, non-mutable place (`let`, and function
/// parameters, which the grammar never allows `var` on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rebindable {
    Let,
    Var,
}

/// Checks every module-level function and inherent method body once,
/// and rejects struct/enum containment cycles across the whole program.
/// `typed` is extended in place with any composite types that first appear
/// inside an expression rather than a declared signature.
/// Whether this expression is the `null` literal, looked at strictly within
/// the expression itself: grouping changes nothing, and a pointee-changing
/// cast preserves the address (`SPEC.md` 3.3), so a cast null is still null.
fn expression_locally_null(node: &SyntaxNode) -> bool {
    match node.kind {
        SyntaxKind::LiteralExpression => node.children.iter().any(|child| {
            matches!(
                child,
                SyntaxElement::Token(Token {
                    kind: TokenKind::Keyword(Keyword::Null),
                    ..
                })
            )
        }),
        SyntaxKind::ParenthesizedExpression | SyntaxKind::CastExpression => child_nodes(node)
            .first()
            .is_some_and(|inner| expression_locally_null(inner)),
        _ => false,
    }
}

/// The `(ok, err)` payload types of `ty` when it is the *standard*
/// `Result[T, E]`, looking through aliases and inference.
///
/// This is the Milestone 15.1 identity query for `?` propagation: only the
/// standard declaration qualifies, so a user enum that merely shares the
/// spelling `Result` receives no propagation behavior.
#[must_use]
pub fn standard_result_payloads(
    resolved: &ResolvedProgram,
    types: &crate::types::TypeContext,
    ty: TypeId,
) -> Option<(TypeId, TypeId)> {
    let mut ty = types.resolve_inference(ty);
    loop {
        match types.kind(ty) {
            TypeKind::Alias { target, .. } => ty = types.resolve_inference(*target),
            TypeKind::Nominal {
                identity,
                arguments,
            } if resolved.is_standard_declaration(identity.declaration, "Result")
                && arguments.len() == 2 =>
            {
                return Some((arguments[0], arguments[1]));
            }
            _ => return None,
        }
    }
}

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
    /// Set while checking a call that selected a trait's default body, so the
    /// resulting instance specializes `Self` to the receiver's type.
    pending_self_type: Option<TypeId>,
    loop_depth: u32,
    /// Nesting depth of `defer` statements being checked. A deferred body runs
    /// while its scope is already exiting, so it may not redirect control.
    defer_depth: u32,
    expect_depth: u32,
    current_is_test: bool,
    /// The enclosing function's declared return type, for postfix `?`
    /// propagation-target checking (`SPEC.md` 8).
    current_return_type: Option<TypeId>,
    /// Nesting depth of `unsafe` blocks being checked.
    unsafe_depth: u32,
    pointer_bits: u8,
    current_self_declaration: Option<DeclarationId>,
    current_module: Option<ModuleId>,
    program: CheckedProgram,
    diagnostics: Vec<Diagnostic>,
}

/// Outcome of selecting a trait method for a receiver.
enum TraitSelection {
    Found(crate::traits::SelectedTraitMethod),
    Ambiguous,
    None,
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
            pending_self_type: None,
            loop_depth: 0,
            defer_depth: 0,
            expect_depth: 0,
            current_is_test: false,
            current_return_type: None,
            unsafe_depth: 0,
            pointer_bits,
            current_self_declaration: None,
            current_module: None,
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
                // Trait *declaration* methods without bodies have nothing to
                // check; default bodies and implementation methods do.
                (declaration.kind == DeclarationKind::Function
                    || (declaration.kind == DeclarationKind::Test && declaration.test_selected))
                    && (declaration.parent_impl.is_some()
                        || declaration.parent_declaration.is_none_or(|parent| {
                            self.resolved.declarations[parent.index()].kind
                                != DeclarationKind::Trait
                        })
                        || crate::syntax::direct_child(&declaration.syntax, SyntaxKind::Block)
                            .is_some())
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
        if ["panic", "trap", "assert", "fail"]
            .into_iter()
            .any(|name| self.resolved.is_standard_declaration(declaration_id, name))
        {
            return;
        }
        let syntax = self.resolved.declarations[declaration_id.index()]
            .syntax
            .clone();
        self.current_is_test =
            self.resolved.declarations[declaration_id.index()].kind == DeclarationKind::Test;
        self.current_self_declaration = self.resolved.declarations[declaration_id.index()]
            .parent_declaration
            .filter(|parent| {
                self.resolved.declarations[parent.index()].kind != DeclarationKind::Trait
            });
        self.current_module = Some(self.resolved.declarations[declaration_id.index()].module);
        self.local_types.clear();
        self.local_rebindable.clear();
        if let Some(parameters_node) = crate::syntax::direct_child(&syntax, SyntaxKind::Parameters)
        {
            let parameter_nodes =
                crate::syntax::direct_children(parameters_node, SyntaxKind::Parameter);
            let mut ordinary_parameters = signature.parameters.iter();
            for parameter_node in parameter_nodes {
                let is_receiver = crate::syntax::direct_tokens(parameter_node)
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
        if let Some(block) = crate::syntax::direct_child(&syntax, SyntaxKind::Block) {
            // Postfix `?` needs the enclosing function's return type to
            // validate its propagation target (`SPEC.md` 8), and expressions
            // are checked without threading `return_type` through every call.
            self.current_return_type = Some(signature.return_type);
            let definitely_returns = self.check_block(block, signature.return_type);
            self.current_return_type = None;
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
        self.current_module = None;
        self.current_is_test = false;
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

    fn is_never_type(&self, ty: TypeId) -> bool {
        matches!(
            self.typed
                .types
                .kind(self.typed.types.resolve_inference(ty)),
            TypeKind::Never
        )
    }

    /// Whether a field can be named from the function currently being
    /// checked. Unmarked fields are package-private; standard-library modules
    /// share the `None` package identity and therefore form one package for
    /// this purpose.
    fn field_is_accessible(&self, field: FieldId) -> bool {
        let field = &self.resolved.fields[field.index()];
        if field.visibility == Visibility::Public {
            return true;
        }
        let Some(current_module) = self.current_module else {
            return false;
        };
        let owner_module = self.resolved.declarations[field.parent_declaration.index()].module;
        self.resolved.modules[current_module.index()]
            .package
            .as_ref()
            == self.resolved.modules[owner_module.index()].package.as_ref()
    }

    /// Reports one external use of a package-private field. Callers continue
    /// checking the field's type after this diagnostic so one visibility
    /// mistake does not cause unrelated type or control-flow cascades.
    fn require_field_access(&mut self, field: FieldId, use_span: Span) -> bool {
        if self.field_is_accessible(field) {
            return true;
        }
        let field = &self.resolved.fields[field.index()];
        let name = self.resolved.symbol_text(field.name);
        self.diagnostics.push(
            Diagnostic::new(
                Category::Visibility,
                format!("field `{name}` is package-private"),
            )
            .with_primary(use_span)
            .with_related(field.span, "package-private field declared here"),
        );
        false
    }

    fn resolve_aliases(&self, mut ty: TypeId) -> TypeId {
        loop {
            ty = self.typed.types.resolve_inference(ty);
            match self.typed.types.kind(ty) {
                TypeKind::Alias { target, .. } => ty = *target,
                _ => return ty,
            }
        }
    }

    fn node_contains_never_expression(&self, node: &SyntaxNode) -> bool {
        node.direct_nodes().into_iter().any(|child| {
            self.program
                .expression_types
                .get(&child.span)
                .is_some_and(|ty| self.is_never_type(*ty))
                || self.node_contains_never_expression(child)
        })
    }

    fn check_statement(&mut self, node: &SyntaxNode, return_type: TypeId) -> bool {
        match node.kind {
            SyntaxKind::LetStatement => {
                self.check_let_statement(node);
                self.node_contains_never_expression(node)
            }
            SyntaxKind::AssignmentStatement => {
                self.check_assignment_statement(node);
                self.node_contains_never_expression(node)
            }
            SyntaxKind::ExpressionStatement => {
                if let Some(expression) = child_nodes(node).into_iter().next() {
                    let (ty, _) = self.check_expr(expression, ExpectedType::None);
                    return self.is_never_type(ty);
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
                    // `defer:` block form: an ordinary lexical scope; the M7
                    // control-redirection bans apply through `defer_depth`.
                    Some(block) => {
                        self.check_block(block, return_type);
                    }
                    // `defer call` single-call form: exactly one safe
                    // unit-returning call (`SPEC.md` 8). The parser already
                    // rejects a non-call expression.
                    None => {
                        if let Some(call) = child_nodes(node).into_iter().next() {
                            let (ty, _) = self.check_expr(call, ExpectedType::None);
                            let unit = self.typed.types.primitive(PrimitiveType::Unit);
                            if ty != self.typed.types.error()
                                && !self.typed.types.exactly_equal(ty, unit)
                            {
                                self.diagnostics.push(
                                    Diagnostic::new(
                                        Category::ControlFlow,
                                        "a deferred call must return `()`; handle a fallible \
                                         operation before scope exit instead",
                                    )
                                    .with_primary(call.span),
                                );
                            }
                            if call.kind == SyntaxKind::CallExpression && self.call_is_unsafe(call)
                            {
                                self.diagnostics.push(
                                    Diagnostic::new(
                                        Category::ControlFlow,
                                        "an unsafe or foreign call cannot be deferred; wrap it \
                                         in a safe unit-returning function",
                                    )
                                    .with_primary(call.span),
                                );
                            }
                        }
                    }
                }
                self.defer_depth -= 1;
                false
            }
            SyntaxKind::ExpectStatement => {
                if !self.current_is_test {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ControlFlow,
                            "`expect` is valid only inside a test declaration",
                        )
                        .with_primary(node.span),
                    );
                }
                if self.expect_depth > 0 {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ControlFlow,
                            "an `expect` block cannot contain another `expect` block",
                        )
                        .with_primary(node.span),
                    );
                }
                let children = child_nodes(node);
                if let Some(selector) = children
                    .iter()
                    .copied()
                    .find(|child| child.kind != SyntaxKind::Block)
                {
                    let (selector_type, _) = self.check_expr(selector, ExpectedType::None);
                    if selector_type != self.typed.types.error()
                        && let Some(runtime_trap) =
                            self.resolved.standard_declaration("RuntimeTrap")
                        && !crate::traits::implements_trait(
                            self.resolved,
                            self.typed,
                            selector_type,
                            runtime_trap,
                        )
                    {
                        self.diagnostics.push(
                            Diagnostic::new(
                                Category::ExpressionType,
                                "an `expect` selector must implement `std.testing.RuntimeTrap`",
                            )
                            .with_primary(selector.span),
                        );
                    }
                }
                self.expect_depth += 1;
                if let Some(block) = children
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)
                {
                    self.check_block(block, return_type);
                }
                self.expect_depth -= 1;
                false
            }
            SyntaxKind::IfStatement => self.check_if_statement(node, return_type),
            SyntaxKind::WhileStatement => {
                let children = child_nodes(node);
                let mut condition_terminates = false;
                if let Some(condition) = children.first() {
                    condition_terminates = self.check_condition(condition);
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
                condition_terminates
            }
            SyntaxKind::ForStatement => {
                let children = child_nodes(node);
                let iterable = children
                    .iter()
                    .copied()
                    .find(|child| child.kind != SyntaxKind::Block);
                let iterable_type = iterable
                    .map(|iterable| {
                        let (ty, _) = self.check_expr(iterable, ExpectedType::None);
                        self.program.copies.insert(iterable.span);
                        ty
                    })
                    .unwrap_or_else(|| self.typed.types.error());
                let element_type = match self
                    .typed
                    .types
                    .kind(self.typed.types.resolve_inference(iterable_type))
                    .clone()
                {
                    TypeKind::Slice(element) => Some(element),
                    TypeKind::Array { element, .. } => Some(element),
                    TypeKind::Builtin { builtin, arguments } => {
                        match (self.resolved.builtin_name(builtin), arguments.as_slice()) {
                            ("Vec" | "Set", [element]) => Some(*element),
                            ("Map", [key, value]) => {
                                Some(self.typed.types.intern(TypeKind::Tuple(vec![*key, *value])))
                            }
                            _ => None,
                        }
                    }
                    TypeKind::Error => Some(self.typed.types.error()),
                    _ => None,
                };
                let binding = node.children.iter().find_map(|child| match child {
                    SyntaxElement::Token(token)
                        if matches!(token.kind, TokenKind::Identifier(_)) =>
                    {
                        self.span_to_local.get(&token.span).copied()
                    }
                    _ => None,
                });
                if let (Some(binding), Some(element_type)) = (binding, element_type) {
                    self.local_types.insert(binding, element_type);
                    self.local_rebindable.insert(binding, Rebindable::Let);
                } else if iterable_type != self.typed.types.error() {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "`for` supports only slices, arrays, `Vec`, `Map`, and `Set`",
                        )
                        .with_primary(node.span),
                    );
                }
                if let Some(block) = children
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)
                {
                    self.loop_depth += 1;
                    self.check_block(block, return_type);
                    self.loop_depth -= 1;
                }
                self.is_never_type(iterable_type)
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
                if self.defer_depth > 0 || self.expect_depth > 0 {
                    let what = if node.kind == SyntaxKind::BreakStatement {
                        "break"
                    } else {
                        "continue"
                    };
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ControlFlow,
                            if self.expect_depth > 0 {
                                format!("`{what}` cannot escape an `expect` block")
                            } else {
                                format!("`{what}` cannot appear inside a deferred block")
                            },
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
        let pattern = child_nodes(node)
            .into_iter()
            .find(|child| child.kind == SyntaxKind::TuplePattern);
        let nodes = child_nodes(node)
            .into_iter()
            .filter(|child| child.kind != SyntaxKind::TuplePattern)
            .collect::<Vec<_>>();
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
        if let Some(pattern) = pattern {
            self.check_local_binding_pattern(pattern, final_type, is_var);
        } else if let Some(token) = let_name_token(node)
            && token_text(token) != "_"
            && let Some(&id) = self.span_to_local.get(&token.span)
        {
            self.record_local_binding(id, final_type, is_var);
        }
    }

    fn check_local_binding_pattern(&mut self, pattern: &SyntaxNode, ty: TypeId, is_var: bool) {
        match pattern.kind {
            SyntaxKind::TuplePattern => {
                let elements = child_nodes(pattern);
                let resolved = self.resolve_aliases(ty);
                let members = match self.typed.types.kind(resolved) {
                    TypeKind::Tuple(members) if members.len() == elements.len() => {
                        Some(members.clone())
                    }
                    TypeKind::Primitive(PrimitiveType::Unit) if elements.is_empty() => {
                        Some(Vec::new())
                    }
                    TypeKind::Error => Some(vec![self.typed.types.error(); elements.len()]),
                    _ => None,
                };
                if members.is_none() && ty != self.typed.types.error() {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::Pattern,
                            "this tuple binding pattern does not match the initializer's type",
                        )
                        .with_primary(pattern.span),
                    );
                }
                for (index, element) in elements.into_iter().enumerate() {
                    let element_type = members
                        .as_ref()
                        .and_then(|members| members.get(index).copied())
                        .unwrap_or_else(|| self.typed.types.error());
                    self.check_local_binding_pattern(element, element_type, is_var);
                }
            }
            SyntaxKind::Pattern => {
                let Some(token) = first_identifier_token(pattern) else {
                    return;
                };
                if token_text(token) == "_" {
                    return;
                }
                if let Some(&id) = self.span_to_local.get(&token.span) {
                    self.record_local_binding(id, ty, is_var);
                    self.program.copied_pattern_bindings.insert(id);
                    self.program.pattern_binding_types.insert(id, ty);
                }
            }
            _ => {}
        }
    }

    fn record_local_binding(&mut self, id: LocalBindingId, ty: TypeId, is_var: bool) {
        self.local_types.insert(id, ty);
        self.local_rebindable.insert(
            id,
            if is_var {
                Rebindable::Var
            } else {
                Rebindable::Let
            },
        );
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
        let shift_assignment = operator.is_some_and(|operator| {
            matches!(operator.kind, TokenKind::ShlAssign | TokenKind::ShrAssign)
        });
        let (value_type, _) = self.check_expr(
            value_node,
            if shift_assignment {
                ExpectedType::None
            } else {
                ExpectedType::Exact(place_type)
            },
        );
        self.program.copies.insert(value_node.span);
        if shift_assignment {
            if value_type != self.typed.types.error()
                && !self
                    .typed
                    .types
                    .expanded_primitive(value_type)
                    .is_some_and(PrimitiveType::is_unsigned_integer)
            {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        "a shift count must have an unsigned integer type",
                    )
                    .with_primary(value_node.span),
                );
            }
        } else if !self.types_compatible(value_type, place_type) {
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
        if self.expect_depth > 0 {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ControlFlow,
                    "`return` cannot escape an `expect` block",
                )
                .with_primary(node.span),
            );
        }
        let expression = child_nodes(node).into_iter().next();
        let unit_type = self.typed.types.primitive(PrimitiveType::Unit);
        let is_unit = self.typed.types.exactly_equal(return_type, unit_type);
        if self.is_never_type(return_type) {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ControlFlow,
                    "a never-returning function cannot contain an ordinary `return`",
                )
                .with_primary(node.span),
            );
        }
        match expression {
            Some(expression) => {
                let return_is_inferred = matches!(
                    self.typed
                        .types
                        .kind(self.typed.types.resolve_inference(return_type)),
                    TypeKind::InferenceVariable(_)
                );
                let expected = if return_is_inferred {
                    ExpectedType::None
                } else {
                    ExpectedType::Exact(return_type)
                };
                let (expression_type, _) = self.check_expr(expression, expected);
                self.program.copies.insert(expression.span);
                if is_unit {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "a unit-returning function must use `return` without a value",
                        )
                        .with_primary(expression.span),
                    );
                } else if return_is_inferred {
                    if self
                        .typed
                        .types
                        .unify(return_type, expression_type)
                        .is_err()
                    {
                        self.diagnostics.push(
                            Diagnostic::new(
                                Category::ExpressionType,
                                "closure return values must have one exact type",
                            )
                            .with_primary(expression.span),
                        );
                    }
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
                let return_is_inferred = matches!(
                    self.typed
                        .types
                        .kind(self.typed.types.resolve_inference(return_type)),
                    TypeKind::InferenceVariable(_)
                );
                if return_is_inferred {
                    let _ = self.typed.types.unify(return_type, unit_type);
                } else if !is_unit {
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

    fn check_condition(&mut self, node: &SyntaxNode) -> bool {
        let bool_type = self.typed.types.primitive(PrimitiveType::Bool);
        let (condition_type, _) = self.check_expr(node, ExpectedType::Exact(bool_type));
        if self.is_never_type(condition_type) {
            return true;
        }
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
        false
    }

    fn check_if_statement(&mut self, node: &SyntaxNode, return_type: TypeId) -> bool {
        let children = child_nodes(node);
        let mut then_returns = false;
        let mut else_returns = None;
        let mut condition_terminates = false;
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
                _ => condition_terminates = self.check_condition(child),
            }
        }
        condition_terminates || (then_returns && else_returns.unwrap_or(false))
    }

    fn check_expr(&mut self, node: &SyntaxNode, expected: ExpectedType) -> (TypeId, PlaceKind) {
        let (mut ty, mut place) = match node.kind {
            SyntaxKind::LiteralExpression => self.check_literal(node, expected),
            SyntaxKind::FormattedStringExpression => self.check_formatted_string(node),
            SyntaxKind::NameExpression => self.check_name_expression(node, expected),
            SyntaxKind::MemberExpression => self.check_member_expression(node, expected),
            SyntaxKind::TupleFieldExpression => self.check_tuple_field_expression(node),
            SyntaxKind::ClosureExpression => self.check_closure_expression(node),
            SyntaxKind::UnaryExpression => self.check_unary(node, expected),
            SyntaxKind::BinaryExpression => self.check_binary(node),
            SyntaxKind::CastExpression => self.check_cast(node),
            SyntaxKind::CallExpression => self.check_call(node, expected),
            SyntaxKind::BracketExpression => self.check_bracket_expression(node, expected),
            SyntaxKind::TryExpression => self.check_try(node),
            SyntaxKind::ParenthesizedExpression => self.check_parenthesized(node, expected),
            SyntaxKind::TupleExpression => self.check_tuple(node, expected),
            SyntaxKind::ArrayExpression => self.check_array(node, expected),
            SyntaxKind::MacroExpression => self.check_macro(node, expected),
            SyntaxKind::RecordExpression => self.check_record_expression(node, expected),
            _ => (self.typed.types.error(), PlaceKind::Value),
        };
        if let ExpectedType::Exact(target) = expected
            && ty != self.typed.types.error()
            && target != self.typed.types.error()
            && !self.typed.types.exactly_equal(ty, target)
            && !self.is_never_type(ty)
            && self.is_trait_object_reference(target)
        {
            match self.record_trait_object_coercion(node.span, ty, target) {
                Some(coercion) => {
                    self.program
                        .trait_object_coercions
                        .insert(node.span, coercion);
                    ty = target;
                    place = PlaceKind::Value;
                }
                None => {
                    ty = self.typed.types.error();
                    place = PlaceKind::Value;
                }
            }
        }
        // Calling an unsafe or foreign function requires an `unsafe:` block
        // at the call site (`SPEC.md` 10, Milestones 16.3 and 16.4);
        // referencing one without calling it stays safe. This single gate
        // covers direct calls, bound and unbound methods, trait dispatch, and
        // indirect calls through `&unsafe fn` values.
        if node.kind == SyntaxKind::CallExpression && self.call_is_unsafe(node) {
            self.require_unsafe_context(node.span, "calling an unsafe or foreign function");
        }
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
            let (ty, _) = self.check_expr(expression, ExpectedType::None);
            self.program.copies.insert(expression.span);
            if ty != self.typed.types.error()
                && !crate::traits::provides(self.resolved, self.typed, ty, "Display")
            {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        "formatted-string interpolation requires `Display`",
                    )
                    .with_primary(expression.span),
                );
            }
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
                            } else if operand_place == PlaceKind::RawPointerTarget {
                                "cannot form a safe reference to a raw pointer's target; \
                                 use the explicit `as` conversion"
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
                    TypeKind::Reference { mutability, target } => {
                        let place = if mutability == Mutability::Mutable {
                            PlaceKind::Mutable
                        } else {
                            PlaceKind::Addressable
                        };
                        (target, place)
                    }
                    TypeKind::RawPointer { mutability, target } => {
                        // Raw dereference is unsafe-only (`SPEC.md` 3.3 and
                        // 10). A `*var T` target is an assignable place; a
                        // `*T` target is read-only even in unsafe code, and
                        // neither can form a safe reference — that path is
                        // the explicit `as` conversion.
                        self.require_unsafe_context(node.span, "dereferencing a raw pointer");
                        self.reject_locally_invalid_pointer(operand);
                        let place = if mutability == Mutability::Mutable {
                            PlaceKind::RawPointerTarget
                        } else {
                            PlaceKind::Value
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
        if matches!(operator, TokenKind::Shl | TokenKind::Shr) {
            let (right_type, _) = self.check_expr(right, ExpectedType::None);
            let left_valid =
                left_type == self.typed.types.error() || self.is_integer_type(left_type);
            let right_valid = right_type == self.typed.types.error()
                || self
                    .typed
                    .types
                    .expanded_primitive(right_type)
                    .is_some_and(PrimitiveType::is_unsigned_integer);
            if !left_valid {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        "a shift value must have an integer type",
                    )
                    .with_primary(left.span),
                );
            }
            if !right_valid {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        "a shift count must have an unsigned integer type",
                    )
                    .with_primary(right.span),
                );
            }
            if left_valid && right_valid {
                self.reject_statically_invalid_arithmetic(node, operator, left_type, left, right);
            }
            return (
                if left_valid && right_valid {
                    left_type
                } else {
                    self.typed.types.error()
                },
                PlaceKind::Value,
            );
        }
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
            TokenKind::PlusPlus => {
                if left_type != self.typed.types.error() && !self.is_concatenable_type(left_type) {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "`++` requires `str`, `String`, or `Vec[T]` operands",
                        )
                        .with_primary(node.span),
                    );
                    return (self.typed.types.error(), PlaceKind::Value);
                }
                (left_type, PlaceKind::Value)
            }
            TokenKind::EqEq | TokenKind::NotEq => {
                if left_type != self.typed.types.error()
                    && !crate::traits::provides(self.resolved, self.typed, left_type, "PartialEq")
                {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::TypeSystem,
                            "this type does not implement `PartialEq`; derive or implement it \
                             to compare values",
                        )
                        .with_primary(node.span),
                    );
                    return (self.typed.types.error(), PlaceKind::Value);
                }
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
                if left_type != self.typed.types.error()
                    && !crate::traits::provides(self.resolved, self.typed, left_type, "PartialOrd")
                {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::TypeSystem,
                            "this type does not implement `PartialOrd`; derive or implement it \
                             to order values",
                        )
                        .with_primary(node.span),
                    );
                    return (self.typed.types.error(), PlaceKind::Value);
                }
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
                self.reject_statically_invalid_arithmetic(node, operator, left_type, left, right);
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
                self.reject_statically_invalid_arithmetic(node, operator, left_type, left, right);
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

    fn reject_statically_invalid_arithmetic(
        &mut self,
        node: &SyntaxNode,
        operator: &TokenKind,
        result_type: TypeId,
        left: &SyntaxNode,
        right: &SyntaxNode,
    ) {
        let Some(primitive) = self.typed.types.expanded_primitive(result_type) else {
            return;
        };
        if !primitive.is_integer() {
            return;
        }
        let (Some(left), Some(right)) = (
            static_integer_expression(left),
            static_integer_expression(right),
        ) else {
            return;
        };
        let result = match operator {
            TokenKind::Plus => static_integer_add(left, right),
            TokenKind::Minus => static_integer_subtract(left, right),
            TokenKind::Star => static_integer_multiply(left, right),
            TokenKind::Slash if right.magnitude == 0 => {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        "statically evident division by zero is invalid",
                    )
                    .with_primary(node.span),
                );
                return;
            }
            TokenKind::Slash => Some(StaticInteger {
                negative: left.negative != right.negative,
                magnitude: left.magnitude / right.magnitude,
            }),
            TokenKind::Percent if right.magnitude == 0 => {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        "statically evident remainder by zero is invalid",
                    )
                    .with_primary(node.span),
                );
                return;
            }
            TokenKind::Percent => Some(StaticInteger {
                negative: left.negative,
                magnitude: left.magnitude % right.magnitude,
            }),
            TokenKind::Shl | TokenKind::Shr => {
                let bits = integer_primitive_bits(primitive, self.pointer_bits);
                if right.negative || right.magnitude >= u128::from(bits) {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "a statically evident shift count is outside the value type's width",
                        )
                        .with_primary(node.span),
                    );
                    return;
                }
                let count = right.magnitude as u32;
                if matches!(operator, TokenKind::Shl) {
                    left.magnitude
                        .checked_shl(count)
                        .map(|magnitude| StaticInteger {
                            negative: left.negative,
                            magnitude,
                        })
                } else {
                    let magnitude = if left.negative && count != 0 {
                        let rounding = (1u128 << count) - 1;
                        left.magnitude
                            .checked_add(rounding)
                            .map(|magnitude| magnitude >> count)
                    } else {
                        Some(left.magnitude >> count)
                    };
                    magnitude.map(|magnitude| StaticInteger {
                        negative: left.negative,
                        magnitude,
                    })
                }
            }
            _ => return,
        };
        let Some(result) = result else {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ExpressionType,
                    "this statically evident integer operation overflows",
                )
                .with_primary(node.span),
            );
            return;
        };
        if !crate::types::integer_fits(
            primitive,
            result.magnitude,
            result.negative,
            self.pointer_bits,
        ) {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ExpressionType,
                    format!(
                        "this statically evident integer operation overflows `{}`",
                        crate::types::primitive_name(primitive)
                    ),
                )
                .with_primary(node.span),
            );
        }
    }

    fn is_concatenable_type(&self, mut ty: TypeId) -> bool {
        loop {
            ty = self.typed.types.resolve_inference(ty);
            match self.typed.types.kind(ty) {
                TypeKind::Alias { target, .. } => ty = *target,
                TypeKind::Primitive(PrimitiveType::Str | PrimitiveType::String) => return true,
                TypeKind::Builtin { builtin, arguments } => {
                    return self.resolved.builtin_name(*builtin) == "Vec" && arguments.len() == 1;
                }
                _ => return false,
            }
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
        // Explicit `reference as &Trait` / `as &var Trait` remains available
        // alongside contextual coercion. The source must be a safe reference
        // of matching mutability whose target implements an object-safe trait.
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
                    let TypeKind::Reference {
                        target: source_target,
                        ..
                    } = self.typed.types.kind(source).clone()
                    else {
                        return (target_type, PlaceKind::Value);
                    };
                    if !self.check_trait_object_formation(target_type, source_target, node.span) {
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
        // The pointer conversion matrix (`SPEC.md` 3.3, Milestones 16.2 and
        // 16.5). Every permitted conversion preserves the address and
        // provenance; none upgrades mutability.
        match (
            self.typed.types.kind(source_resolved).clone(),
            self.typed.types.kind(target_resolved).clone(),
        ) {
            // `&T as *T`, `&var T as *var T`, and the `&var T as *T`
            // downgrade are all safe.
            (
                TypeKind::Reference {
                    mutability: source_mutability,
                    target: source_target,
                },
                TypeKind::RawPointer {
                    mutability: target_mutability,
                    target: target_target,
                },
            ) => {
                if !self.typed.types.exactly_equal(source_target, target_target) {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "a safe reference converts only to a raw pointer with exactly \
                             its pointee type",
                        )
                        .with_primary(node.span),
                    );
                    return (self.typed.types.error(), PlaceKind::Value);
                }
                if target_mutability == Mutability::Mutable
                    && source_mutability == Mutability::Shared
                {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "a shared reference cannot convert to a mutable raw pointer",
                        )
                        .with_primary(node.span),
                    );
                    return (self.typed.types.error(), PlaceKind::Value);
                }
                return (target_type, PlaceKind::Value);
            }
            (TypeKind::RawPointer { .. }, TypeKind::RawPointer { .. })
                if self
                    .typed
                    .types
                    .exactly_equal(source_resolved, target_resolved) =>
            {
                return (target_type, PlaceKind::Value);
            }
            (
                TypeKind::RawPointer {
                    mutability: source_mutability,
                    target: source_target,
                },
                TypeKind::RawPointer {
                    mutability: target_mutability,
                    target: target_target,
                },
            ) => {
                let source_function = matches!(
                    self.typed
                        .types
                        .kind(self.typed.types.resolve_inference(source_target)),
                    TypeKind::Function { .. }
                );
                let target_function = matches!(
                    self.typed
                        .types
                        .kind(self.typed.types.resolve_inference(target_target)),
                    TypeKind::Function { .. }
                );
                if source_function != target_function {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "raw function pointers and raw data pointers cannot be converted \
                             between one another",
                        )
                        .with_primary(node.span),
                    );
                    return (self.typed.types.error(), PlaceKind::Value);
                }
                // No cast may upgrade `*T` to any `*var U`.
                if target_mutability == Mutability::Mutable
                    && source_mutability == Mutability::Shared
                {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "a `*T` pointer cannot be cast to any `*var U`; the cast \
                             preserves mutability permission",
                        )
                        .with_primary(node.span),
                    );
                    return (self.typed.types.error(), PlaceKind::Value);
                }
                // The `*var T as *T` downgrade with the same pointee is safe;
                // changing the pointee type requires an `unsafe:` block.
                if !self.typed.types.exactly_equal(source_target, target_target) {
                    self.require_unsafe_context(
                        node.span,
                        "a cast that changes a raw pointer's pointee type",
                    );
                }
                return (target_type, PlaceKind::Value);
            }
            // `pointer as &T` / `as &var T`: unsafe-only, exact mutability
            // and pointee, checked for null and alignment at runtime.
            (
                TypeKind::RawPointer {
                    mutability: source_mutability,
                    target: source_target,
                },
                TypeKind::Reference {
                    mutability: target_mutability,
                    target: target_target,
                },
            ) => {
                // A locally known-null operand is invalid regardless of the
                // pointee question, so it is diagnosed first — `null` has an
                // unconstrained pointee that would otherwise misreport as a
                // type mismatch.
                if self.expression_locally_null(source) {
                    self.reject_locally_invalid_pointer(source);
                    return (self.typed.types.error(), PlaceKind::Value);
                }
                if source_mutability != target_mutability
                    || !self.typed.types.exactly_equal(source_target, target_target)
                {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "a raw pointer converts only to a reference with exactly its \
                             pointee type and mutability",
                        )
                        .with_primary(node.span),
                    );
                    return (self.typed.types.error(), PlaceKind::Value);
                }
                self.require_unsafe_context(node.span, "converting a raw pointer to a reference");
                return (target_type, PlaceKind::Value);
            }
            // The initial language has no pointer/integer conversion in
            // either direction (`SPEC.md` 3.3).
            (TypeKind::RawPointer { .. }, _) | (_, TypeKind::RawPointer { .. }) => {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        "raw pointers do not convert to or from any non-pointer type; \
                         there is no pointer arithmetic or pointer/integer conversion",
                    )
                    .with_primary(node.span),
                );
                return (self.typed.types.error(), PlaceKind::Value);
            }
            _ => {}
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
        // Postfix `?` propagation (`SPEC.md` 8): the operand must be the
        // *standard* `Result[T, E]` — a user type that merely shares the
        // spelling is an ordinary value — and the enclosing function must
        // return `Result[U, E]` with exactly the same `E`. There is no
        // implicit error conversion.
        if self.defer_depth > 0 {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ControlFlow,
                    "postfix `?` cannot appear inside a deferred statement",
                )
                .with_primary(node.span),
            );
        }
        if self.expect_depth > 0 {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ControlFlow,
                    "postfix `?` cannot escape an `expect` block",
                )
                .with_primary(node.span),
            );
        }
        let Some(operand) = child_nodes(node).into_iter().next() else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let (operand_type, _) = self.check_expr(operand, ExpectedType::None);
        if operand_type == self.typed.types.error() {
            return (self.typed.types.error(), PlaceKind::Value);
        }
        let Some((ok_type, err_type)) =
            standard_result_payloads(self.resolved, &self.typed.types, operand_type)
        else {
            let message = if self
                .standard_nominal_arguments(operand_type, "Option")
                .is_some()
            {
                "postfix `?` does not apply to `Option`; handle it with `match`"
            } else {
                "postfix `?` requires an operand of the standard `Result[T, E]` type"
            };
            self.diagnostics
                .push(Diagnostic::new(Category::ExpressionType, message).with_primary(node.span));
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let return_error = self.current_return_type.and_then(|return_type| {
            standard_result_payloads(self.resolved, &self.typed.types, return_type)
                .map(|(_, err)| err)
        });
        let Some(return_error) = return_error else {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ExpressionType,
                    "postfix `?` is valid only inside a function returning the standard \
                     `Result[U, E]` type",
                )
                .with_primary(node.span),
            );
            return (self.typed.types.error(), PlaceKind::Value);
        };
        if !self.typed.types.exactly_equal(err_type, return_error) {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ExpressionType,
                    "the operand's error type must exactly match the enclosing function's \
                     error type; convert a different error explicitly, such as with `match`",
                )
                .with_primary(node.span),
            );
            return (self.typed.types.error(), PlaceKind::Value);
        }
        (ok_type, PlaceKind::Value)
    }

    /// Reports an unsafe-only operation used outside an `unsafe:` block
    /// (`SPEC.md` 10). The lexical block is the only unsafe context: an
    /// `unsafe` function's body is deliberately *not* one, so each unsafe
    /// assumption stays locally visible.
    fn require_unsafe_context(&mut self, span: Span, what: &str) {
        if self.unsafe_depth == 0 {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::UnsafeContext,
                    format!("{what} requires an `unsafe:` block"),
                )
                .with_primary(span),
            );
        }
    }

    /// The mandatory expression-local validity determination (`SPEC.md` 3.3):
    /// a raw dereference or raw-to-reference conversion is a compile-time
    /// error only when its pointer operand is an expression-local constant
    /// known to be null or misaligned.
    ///
    /// The determination may evaluate literals, casts, and operators within
    /// the operand expression, and nothing else: facts never propagate
    /// through bindings, assignments, branch conditions, reachability, or
    /// calls. The initial language has no integer-to-pointer conversion or
    /// pointer arithmetic, so no expression can construct a non-null constant
    /// address; `null` is therefore the only constant this evaluator can
    /// prove invalid, and the misalignment half of the rule is vacuously
    /// satisfied until such an expression exists.
    fn reject_locally_invalid_pointer(&mut self, operand: &SyntaxNode) {
        if self.expression_locally_null(operand) {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::PointerValidity,
                    "this pointer operand is known to be null",
                )
                .with_primary(operand.span),
            );
        }
    }

    fn expression_locally_null(&self, node: &SyntaxNode) -> bool {
        expression_locally_null(node)
    }

    /// Whether a checked call expression invokes an unsafe or foreign target.
    ///
    /// `SPEC.md` 8: a direct unsafe or foreign call cannot be deferred;
    /// native cleanup is wrapped in a safe unit-returning method. Milestone
    /// 17 applies this same recorded rule when foreign calls become
    /// executable.
    fn call_is_unsafe(&self, call: &SyntaxNode) -> bool {
        match self.program.calls.get(&call.span) {
            Some(CheckedCall::Direct(instance) | CheckedCall::BoundMethod { instance, .. }) => {
                self.declaration_signature_is_unsafe(instance.declaration)
            }
            Some(
                CheckedCall::TraitSelfMethod { method, .. }
                | CheckedCall::DynamicMethod { method, .. },
            ) => self.declaration_signature_is_unsafe(*method),
            Some(CheckedCall::Indirect) => child_nodes(call)
                .first()
                .and_then(|callee| self.program.expression_types.get(&callee.span))
                .is_some_and(|&ty| self.function_value_is_unsafe(ty)),
            _ => false,
        }
    }

    fn declaration_signature_is_unsafe(&self, declaration: DeclarationId) -> bool {
        if self.resolved.declarations[declaration.index()].kind == DeclarationKind::ForeignFunction
        {
            return true;
        }
        self.typed
            .function_signatures
            .get(&declaration)
            .is_some_and(|signature| self.function_value_is_unsafe(signature.ty))
    }

    /// Whether `ty` is (or refers to) an unsafe function type.
    fn function_value_is_unsafe(&self, ty: TypeId) -> bool {
        let mut ty = self.typed.types.resolve_inference(ty);
        loop {
            match self.typed.types.kind(ty) {
                TypeKind::RawPointer { target, .. }
                    if matches!(
                        self.typed
                            .types
                            .kind(self.typed.types.resolve_inference(*target)),
                        TypeKind::Function { .. }
                    ) =>
                {
                    return true;
                }
                TypeKind::Alias { target, .. } | TypeKind::Reference { target, .. } => {
                    ty = self.typed.types.resolve_inference(*target);
                }
                TypeKind::Function { safety, .. } => return *safety == Safety::Unsafe,
                _ => return false,
            }
        }
    }

    /// The canonical type arguments of `ty` when it is the standard
    /// declaration named `name`, looking through aliases.
    fn standard_nominal_arguments(&self, ty: TypeId, name: &str) -> Option<Vec<TypeId>> {
        let mut ty = self.typed.types.resolve_inference(ty);
        loop {
            match self.typed.types.kind(ty) {
                TypeKind::Alias { target, .. } => {
                    ty = self.typed.types.resolve_inference(*target);
                }
                TypeKind::Nominal {
                    identity,
                    arguments,
                } if self
                    .resolved
                    .is_standard_declaration(identity.declaration, name) =>
                {
                    return Some(arguments.clone());
                }
                _ => return None,
            }
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

    fn check_tuple_field_expression(&mut self, node: &SyntaxNode) -> (TypeId, PlaceKind) {
        let Some(base) = child_nodes(node).into_iter().next() else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let (base_type, base_place) = self.check_expr(base, ExpectedType::None);
        let Some(index_token) = node.children.iter().rev().find_map(|child| match child {
            SyntaxElement::Token(
                token @ Token {
                    kind: TokenKind::IntegerLiteral { .. },
                    ..
                },
            ) => Some(token),
            _ => None,
        }) else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let Some(index) = tuple_field_index(index_token) else {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Syntax,
                    "a tuple selector must be a canonical unsuffixed decimal index",
                )
                .with_primary(index_token.span),
            );
            return (self.typed.types.error(), PlaceKind::Value);
        };

        let resolved = self.resolve_aliases(base_type);
        let (tuple_type, place) = match self.typed.types.kind(resolved).clone() {
            TypeKind::Tuple(_) | TypeKind::Primitive(PrimitiveType::Unit) => (resolved, base_place),
            TypeKind::Reference { mutability, target } => {
                let place = if mutability == Mutability::Mutable {
                    PlaceKind::Mutable
                } else {
                    PlaceKind::Addressable
                };
                (self.resolve_aliases(target), place)
            }
            TypeKind::RawPointer { mutability, target } => {
                self.require_unsafe_context(node.span, "accessing a tuple through a raw pointer");
                self.reject_locally_invalid_pointer(base);
                let place = if mutability == Mutability::Mutable {
                    PlaceKind::RawPointerTarget
                } else {
                    PlaceKind::Value
                };
                (self.resolve_aliases(target), place)
            }
            TypeKind::Error => return (self.typed.types.error(), PlaceKind::Value),
            _ => {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        "positional field access requires a tuple receiver",
                    )
                    .with_primary(base.span),
                );
                return (self.typed.types.error(), PlaceKind::Value);
            }
        };
        let member = match self.typed.types.kind(tuple_type) {
            TypeKind::Tuple(members) => members.get(index).copied(),
            TypeKind::Primitive(PrimitiveType::Unit) => None,
            _ => {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        "positional field access requires a tuple receiver",
                    )
                    .with_primary(base.span),
                );
                return (self.typed.types.error(), PlaceKind::Value);
            }
        };
        match member {
            Some(member) => (member, place),
            None => {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::ExpressionType,
                        format!("tuple index {index} is out of range"),
                    )
                    .with_primary(index_token.span),
                );
                (self.typed.types.error(), PlaceKind::Value)
            }
        }
    }

    fn check_closure_expression(&mut self, node: &SyntaxNode) -> (TypeId, PlaceKind) {
        let Some(resolved_closure) = self
            .resolved
            .closures
            .iter()
            .find(|closure| closure.span == node.span)
            .cloned()
        else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let declaration = resolved_closure.declaration;
        let Some(mut signature) = self.typed.function_signatures.get(&declaration).cloned() else {
            return (self.typed.types.error(), PlaceKind::Value);
        };

        let mut checked_captures = Vec::new();
        for capture in resolved_closure.captures {
            let source_type = self
                .local_types
                .get(&capture.source)
                .copied()
                .unwrap_or_else(|| self.typed.types.error());
            let source_place = match self.local_rebindable.get(&capture.source) {
                Some(Rebindable::Var) => PlaceKind::Mutable,
                Some(Rebindable::Let) => PlaceKind::Addressable,
                None => PlaceKind::Value,
            };
            let ty = match capture.kind {
                ClosureCaptureKind::Value => {
                    if matches!(
                        self.typed.types.kind(self.resolve_aliases(source_type)),
                        TypeKind::RawPointer { .. }
                    ) {
                        self.diagnostics.push(
                            Diagnostic::new(
                                Category::ExpressionType,
                                "a raw-pointer local must be captured with `*name` or `*var name`",
                            )
                            .with_primary(capture.span),
                        );
                    }
                    source_type
                }
                ClosureCaptureKind::SharedReference => {
                    if !source_place.permits_safe_reference() {
                        self.diagnostics.push(
                            Diagnostic::new(
                                Category::Place,
                                "this capture source is not addressable",
                            )
                            .with_primary(capture.span),
                        );
                    }
                    self.typed.types.intern(TypeKind::Reference {
                        mutability: Mutability::Shared,
                        target: source_type,
                    })
                }
                ClosureCaptureKind::MutableReference => {
                    if !source_place.is_mutable() {
                        self.diagnostics.push(
                            Diagnostic::new(
                                Category::Place,
                                "a mutable-reference capture requires mutable binding storage",
                            )
                            .with_primary(capture.span),
                        );
                    }
                    self.typed.types.intern(TypeKind::Reference {
                        mutability: Mutability::Mutable,
                        target: source_type,
                    })
                }
                ClosureCaptureKind::SharedRawPointer => {
                    match self
                        .typed
                        .types
                        .kind(self.resolve_aliases(source_type))
                        .clone()
                    {
                        TypeKind::RawPointer { target, .. } => {
                            self.typed.types.intern(TypeKind::RawPointer {
                                mutability: Mutability::Shared,
                                target,
                            })
                        }
                        _ => {
                            self.diagnostics.push(
                                Diagnostic::new(
                                    Category::ExpressionType,
                                    "`*name` capture requires a raw-pointer local",
                                )
                                .with_primary(capture.span),
                            );
                            self.typed.types.error()
                        }
                    }
                }
                ClosureCaptureKind::MutableRawPointer => {
                    match self
                        .typed
                        .types
                        .kind(self.resolve_aliases(source_type))
                        .clone()
                    {
                        TypeKind::RawPointer {
                            mutability: Mutability::Mutable,
                            target,
                        } => self.typed.types.intern(TypeKind::RawPointer {
                            mutability: Mutability::Mutable,
                            target,
                        }),
                        _ => {
                            self.diagnostics.push(
                                Diagnostic::new(
                                    Category::ExpressionType,
                                    "`*var name` capture requires a mutable raw-pointer local",
                                )
                                .with_primary(capture.span),
                            );
                            self.typed.types.error()
                        }
                    }
                }
            };
            checked_captures.push(CheckedClosureCapture {
                source: capture.source,
                binding: capture.binding,
                kind: capture.kind,
                source_ty: source_type,
                ty,
                span: capture.span,
            });
        }

        let saved_local_types = std::mem::take(&mut self.local_types);
        let saved_rebindable = std::mem::take(&mut self.local_rebindable);
        let saved_loop = self.loop_depth;
        let saved_defer = self.defer_depth;
        let saved_expect = self.expect_depth;
        let saved_unsafe = self.unsafe_depth;
        let saved_return = self.current_return_type;
        let saved_self = self.current_self_declaration;
        let saved_test = self.current_is_test;

        for capture in &checked_captures {
            self.local_types.insert(capture.binding, capture.ty);
            self.local_rebindable
                .insert(capture.binding, Rebindable::Let);
        }
        if let Some(parameters) = node.direct_child(SyntaxKind::Parameters) {
            for (parameter, parameter_type) in parameters
                .direct_children(SyntaxKind::Parameter)
                .into_iter()
                .zip(&signature.parameters)
            {
                if let Some(token) = parameter_name_token(parameter)
                    && let Some(binding) = self.span_to_local.get(&token.span).copied()
                {
                    self.local_types.insert(binding, parameter_type.ty);
                    self.local_rebindable.insert(binding, Rebindable::Let);
                }
            }
        }
        self.loop_depth = 0;
        self.defer_depth = 0;
        self.expect_depth = 0;
        self.unsafe_depth = 0;
        self.current_return_type = Some(signature.return_type);
        self.current_self_declaration = None;
        self.current_is_test = false;
        let definitely_returns = node
            .direct_child(SyntaxKind::Block)
            .is_some_and(|block| self.check_block(block, signature.return_type));
        let unresolved_return = self.typed.types.resolve_inference(signature.return_type);
        if matches!(
            self.typed.types.kind(unresolved_return),
            TypeKind::InferenceVariable(_)
        ) {
            let inferred = if definitely_returns {
                self.typed.types.never()
            } else {
                self.typed.types.primitive(PrimitiveType::Unit)
            };
            let _ = self.typed.types.unify(signature.return_type, inferred);
        } else if !definitely_returns
            && !self.typed.types.exactly_equal(
                unresolved_return,
                self.typed.types.primitive_id(PrimitiveType::Unit),
            )
        {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ControlFlow,
                    "this closure has a reachable path that does not return its result type",
                )
                .with_primary(node.span),
            );
        }
        signature.return_type = self.typed.types.resolve_inference(signature.return_type);
        signature.ty = self.typed.types.intern(TypeKind::Function {
            safety: Safety::Safe,
            abi: crate::types::Abi::Elamite,
            receiver: None,
            parameters: signature.parameters.clone(),
            return_type: signature.return_type,
        });
        self.typed
            .function_signatures
            .insert(declaration, signature.clone());

        self.local_types = saved_local_types;
        self.local_rebindable = saved_rebindable;
        self.loop_depth = saved_loop;
        self.defer_depth = saved_defer;
        self.expect_depth = saved_expect;
        self.unsafe_depth = saved_unsafe;
        self.current_return_type = saved_return;
        self.current_self_declaration = saved_self;
        self.current_is_test = saved_test;

        let closure_type = self.typed.types.intern(TypeKind::Closure {
            declaration,
            captures: checked_captures.iter().map(|capture| capture.ty).collect(),
            parameters: signature
                .parameters
                .iter()
                .map(|parameter| parameter.ty)
                .collect(),
            return_type: signature.return_type,
        });
        self.program.closures.insert(
            declaration,
            CheckedClosure {
                declaration,
                ty: closure_type,
                captures: checked_captures,
            },
        );
        (closure_type, PlaceKind::Value)
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

    fn check_macro(&mut self, node: &SyntaxNode, expected: ExpectedType) -> (TypeId, PlaceKind) {
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
        let expected_arguments = match expected {
            ExpectedType::Exact(expected) => {
                let expected = self.typed.types.resolve_inference(expected);
                match self.typed.types.kind(expected) {
                    TypeKind::Builtin { builtin, arguments }
                        if self.resolved.builtin_name(*builtin) == name.to_uppercase_first() =>
                    {
                        Some(arguments.clone())
                    }
                    _ => None,
                }
            }
            ExpectedType::None => None,
        };
        match name.as_str() {
            "vec" | "set" => {
                let Some(builtin) = self.resolved.builtin_named(&name.to_uppercase_first()) else {
                    for element in &elements {
                        self.check_expr(element, ExpectedType::None);
                    }
                    return (self.typed.types.error(), PlaceKind::Value);
                };
                let mut element_type = expected_arguments
                    .as_ref()
                    .and_then(|arguments| arguments.first())
                    .copied();
                for element in &elements {
                    let element_expected =
                        element_type.map_or(ExpectedType::None, ExpectedType::Exact);
                    let (ty, _) = self.check_expr(element, element_expected);
                    self.program.copies.insert(element.span);
                    match element_type {
                        None if ty != self.typed.types.error() => element_type = Some(ty),
                        Some(previous)
                            if ty != self.typed.types.error()
                                && !self.typed.types.exactly_equal(previous, ty) =>
                        {
                            self.diagnostics.push(
                                Diagnostic::new(
                                    Category::ExpressionType,
                                    format!("every `{name}` element must have the same exact type"),
                                )
                                .with_primary(element.span),
                            );
                        }
                        _ => {}
                    }
                }
                let argument = element_type.unwrap_or_else(|| {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            format!("an empty `@{name}` literal requires an expected type"),
                        )
                        .with_primary(node.span),
                    );
                    self.typed.types.error()
                });
                if name == "set"
                    && argument != self.typed.types.error()
                    && !crate::traits::provides(self.resolved, self.typed, argument, "StableHash")
                {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::TypeSystem,
                            "`Set` elements must provide `StableHash`",
                        )
                        .with_primary(node.span),
                    );
                }
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
                let mut key_type = expected_arguments
                    .as_ref()
                    .and_then(|arguments| arguments.first())
                    .copied();
                let mut value_type = expected_arguments
                    .as_ref()
                    .and_then(|arguments| arguments.get(1))
                    .copied();
                for pair in elements.chunks(2) {
                    if let [key, value] = pair {
                        let key_expected = key_type.map_or(ExpectedType::None, ExpectedType::Exact);
                        let (kt, _) = self.check_expr(key, key_expected);
                        self.program.copies.insert(key.span);
                        let value_expected =
                            value_type.map_or(ExpectedType::None, ExpectedType::Exact);
                        let (vt, _) = self.check_expr(value, value_expected);
                        self.program.copies.insert(value.span);
                        if let Some(previous) = key_type {
                            if kt != self.typed.types.error()
                                && !self.typed.types.exactly_equal(previous, kt)
                            {
                                self.diagnostics.push(
                                    Diagnostic::new(
                                        Category::ExpressionType,
                                        "every `@map` key must have the same exact type",
                                    )
                                    .with_primary(key.span),
                                );
                            }
                        } else if kt != self.typed.types.error() {
                            key_type = Some(kt);
                        }
                        if let Some(previous) = value_type {
                            if vt != self.typed.types.error()
                                && !self.typed.types.exactly_equal(previous, vt)
                            {
                                self.diagnostics.push(
                                    Diagnostic::new(
                                        Category::ExpressionType,
                                        "every `@map` value must have the same exact type",
                                    )
                                    .with_primary(value.span),
                                );
                            }
                        } else if vt != self.typed.types.error() {
                            value_type = Some(vt);
                        }
                    }
                }
                if elements.is_empty() && (key_type.is_none() || value_type.is_none()) {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "an empty `@map` literal requires an expected type",
                        )
                        .with_primary(node.span),
                    );
                }
                let key = key_type.unwrap_or_else(|| self.typed.types.error());
                let value = value_type.unwrap_or_else(|| self.typed.types.error());
                if key != self.typed.types.error()
                    && !crate::traits::provides(self.resolved, self.typed, key, "StableHash")
                {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::TypeSystem,
                            "`Map` keys must provide `StableHash`",
                        )
                        .with_primary(node.span),
                    );
                }
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
            TypeKind::Array { element, length } => {
                self.check_expr(index, ExpectedType::Exact(usize_type));
                if let Some(token) = index.children.iter().find_map(|child| match child {
                    SyntaxElement::Token(token) => Some(token),
                    SyntaxElement::Node(_) => None,
                }) && let TokenKind::IntegerLiteral { raw, radix, suffix } = &token.kind
                    && crate::types::parse_integer_magnitude(raw, *radix, *suffix)
                        .is_ok_and(|magnitude| magnitude >= length)
                {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::ExpressionType,
                            "this constant array index is out of bounds",
                        )
                        .with_primary(index.span),
                    );
                }
                Some(element)
            }
            TypeKind::Slice(element) => {
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
                    self.program.copies.insert(index.span);
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
                let place = if matches!(self.typed.types.kind(resolved_base), TypeKind::Slice(_)) {
                    PlaceKind::Value
                } else if base_place.is_mutable() {
                    PlaceKind::CollectionInterior
                } else {
                    PlaceKind::Value
                };
                (element_type, place)
            }
            None => (self.typed.types.error(), PlaceKind::Value),
        }
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
            self.require_field_access(field_id, name_token.span);
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
                if !self.require_field_access(*field_id, whole_span) {
                    continue;
                }
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

    /// Lazily requests the canonical result from the type subsystem so body
    /// annotations and declaration signatures share one lowering grammar.
    fn lower_type(&mut self, node: &SyntaxNode) -> TypeId {
        let self_type = self
            .current_self_declaration
            .and_then(|declaration| self.typed.declaration_types.get(&declaration).copied());
        let (ty, diagnostics) = types::lower_annotation(self.resolved, self.typed, node, self_type);
        self.diagnostics.extend(diagnostics);
        ty
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

fn tuple_field_index(token: &Token) -> Option<usize> {
    let TokenKind::IntegerLiteral { raw, radix, suffix } = &token.kind else {
        return None;
    };
    if *radix != 10
        || suffix.is_some()
        || raw.is_empty()
        || !raw.bytes().all(|byte| byte.is_ascii_digit())
        || (raw.len() > 1 && raw.starts_with('0'))
    {
        return None;
    }
    raw.parse().ok()
}

#[derive(Clone, Copy)]
struct StaticInteger {
    negative: bool,
    magnitude: u128,
}

fn static_integer_expression(node: &SyntaxNode) -> Option<StaticInteger> {
    match node.kind {
        SyntaxKind::LiteralExpression => node.children.iter().find_map(|child| {
            let SyntaxElement::Token(Token {
                kind: TokenKind::IntegerLiteral { raw, radix, suffix },
                ..
            }) = child
            else {
                return None;
            };
            Some(StaticInteger {
                negative: false,
                magnitude: crate::types::parse_integer_magnitude(raw, *radix, *suffix).ok()?,
            })
        }),
        SyntaxKind::ParenthesizedExpression => child_nodes(node)
            .into_iter()
            .next()
            .and_then(static_integer_expression),
        SyntaxKind::UnaryExpression => {
            let value = child_nodes(node)
                .into_iter()
                .next()
                .and_then(static_integer_expression)?;
            let operator = node.children.iter().find_map(|child| match child {
                SyntaxElement::Token(token) => Some(&token.kind),
                SyntaxElement::Node(_) => None,
            })?;
            match operator {
                TokenKind::Plus => Some(value),
                TokenKind::Minus => Some(static_integer_negate(value)),
                _ => None,
            }
        }
        SyntaxKind::BinaryExpression => {
            let nodes = child_nodes(node);
            let left = static_integer_expression(*nodes.first()?)?;
            let right = static_integer_expression(*nodes.get(1)?)?;
            let operator = node.children.iter().find_map(|child| match child {
                SyntaxElement::Token(token) => Some(&token.kind),
                SyntaxElement::Node(_) => None,
            })?;
            match operator {
                TokenKind::Plus => static_integer_add(left, right),
                TokenKind::Minus => static_integer_subtract(left, right),
                TokenKind::Star => static_integer_multiply(left, right),
                TokenKind::Slash if right.magnitude != 0 => Some(StaticInteger {
                    negative: left.negative != right.negative,
                    magnitude: left.magnitude / right.magnitude,
                }),
                TokenKind::Percent if right.magnitude != 0 => Some(StaticInteger {
                    negative: left.negative,
                    magnitude: left.magnitude % right.magnitude,
                }),
                _ => None,
            }
        }
        _ => None,
    }
    .map(static_integer_normalize)
}

fn static_integer_normalize(mut value: StaticInteger) -> StaticInteger {
    if value.magnitude == 0 {
        value.negative = false;
    }
    value
}

fn static_integer_negate(mut value: StaticInteger) -> StaticInteger {
    if value.magnitude != 0 {
        value.negative = !value.negative;
    }
    value
}

fn static_integer_add(left: StaticInteger, right: StaticInteger) -> Option<StaticInteger> {
    if left.negative == right.negative {
        Some(StaticInteger {
            negative: left.negative,
            magnitude: left.magnitude.checked_add(right.magnitude)?,
        })
    } else if left.magnitude >= right.magnitude {
        Some(static_integer_normalize(StaticInteger {
            negative: left.negative,
            magnitude: left.magnitude - right.magnitude,
        }))
    } else {
        Some(static_integer_normalize(StaticInteger {
            negative: right.negative,
            magnitude: right.magnitude - left.magnitude,
        }))
    }
}

fn static_integer_subtract(left: StaticInteger, right: StaticInteger) -> Option<StaticInteger> {
    static_integer_add(left, static_integer_negate(right))
}

fn static_integer_multiply(left: StaticInteger, right: StaticInteger) -> Option<StaticInteger> {
    Some(static_integer_normalize(StaticInteger {
        negative: left.negative != right.negative,
        magnitude: left.magnitude.checked_mul(right.magnitude)?,
    }))
}

fn integer_primitive_bits(primitive: PrimitiveType, pointer_bits: u8) -> u8 {
    match primitive {
        PrimitiveType::I8 | PrimitiveType::U8 => 8,
        PrimitiveType::I16 | PrimitiveType::U16 => 16,
        PrimitiveType::I32 | PrimitiveType::U32 => 32,
        PrimitiveType::I64 | PrimitiveType::U64 => 64,
        PrimitiveType::I128 | PrimitiveType::U128 => 128,
        PrimitiveType::Isize | PrimitiveType::Usize => pointer_bits,
        _ => 0,
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
