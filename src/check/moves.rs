//! Move and definite-initialization analysis over typed high-level IR.
//!
//! The typed IR is the first representation that combines concrete generic
//! substitutions, ordered ownership operations, and stable projected places.
//! Keeping this analysis here lets it diagnose source spans without rebuilding
//! places from syntax or leaking ownership policy into lowering.

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostics::{Category, Diagnostic};
use crate::ir::{
    AssignmentOperator, BinaryOperator, FormattedPart, TypedCallee, TypedExpression,
    TypedExpressionKind, TypedFunction, TypedIrProgram, TypedMatchArm, TypedPattern,
    TypedPatternKind, TypedPlace, TypedStatement, TypedStatementKind,
};
use crate::operations::{
    OwnershipPlace, OwnershipPlaceRoot, OwnershipProjection, OwnershipUseKind,
};
use crate::resolution::{DeclarationKind, LocalBindingId, ResolvedProgram};
use crate::source::Span;
use crate::types::{TypeId, TypeKind, TypedProgram};

/// Enforces the owned revision's move-by-default and definite-initialization
/// rules. Legacy programs deliberately bypass this pass.
#[must_use]
pub fn check_moves(
    resolved: &ResolvedProgram,
    typed: &mut TypedProgram,
    program: &TypedIrProgram,
) -> Vec<Diagnostic> {
    if !program.semantic_revision.supports_owned_surface() {
        return Vec::new();
    }
    let mut checker = MoveChecker {
        resolved,
        typed,
        diagnostics: Vec::new(),
        reported: BTreeSet::new(),
        root_types: BTreeMap::new(),
    };
    for function in &program.functions {
        checker.check_function(function);
    }
    checker.diagnostics
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum UnavailableKind {
    Uninitialized,
    Moved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Transition {
    available: bool,
    span: Span,
    unavailable_kind: UnavailableKind,
}

impl Transition {
    fn initialized(span: Span) -> Self {
        Self {
            available: true,
            span,
            unavailable_kind: UnavailableKind::Uninitialized,
        }
    }

    fn uninitialized(span: Span) -> Self {
        Self {
            available: false,
            span,
            unavailable_kind: UnavailableKind::Uninitialized,
        }
    }

    fn moved(span: Span) -> Self {
        Self {
            available: false,
            span,
            unavailable_kind: UnavailableKind::Moved,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StateNode {
    transition: Transition,
    children: BTreeMap<OwnershipProjection, StateNode>,
}

impl StateNode {
    fn new(transition: Transition) -> Self {
        Self {
            transition,
            children: BTreeMap::new(),
        }
    }

    fn set(&mut self, projections: &[OwnershipProjection], transition: Transition) {
        if let Some((projection, rest)) = projections.split_first() {
            let inherited = self.transition;
            self.children
                .entry(*projection)
                .or_insert_with(|| Self::new(inherited))
                .set(rest, transition);
        } else {
            self.transition = transition;
            self.children.clear();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct MoveState {
    roots: BTreeMap<OwnershipPlaceRoot, StateNode>,
}

impl MoveState {
    fn set(&mut self, place: &OwnershipPlace, transition: Transition) {
        if let Some(root) = self.roots.get_mut(&place.root) {
            root.set(&place.projections, transition);
        }
    }

    fn join(states: impl IntoIterator<Item = Self>) -> Option<Self> {
        let mut states = states.into_iter();
        let mut joined = states.next()?;
        for state in states {
            let roots = joined
                .roots
                .keys()
                .chain(state.roots.keys())
                .copied()
                .collect::<BTreeSet<_>>();
            let mut next = BTreeMap::new();
            for root in roots {
                match (joined.roots.get(&root), state.roots.get(&root)) {
                    (Some(left), Some(right)) => {
                        next.insert(root, join_nodes(left, right));
                    }
                    (Some(node), None) | (None, Some(node)) => {
                        next.insert(root, node.clone());
                    }
                    (None, None) => unreachable!(),
                }
            }
            joined.roots = next;
        }
        Some(joined)
    }
}

fn join_transition(left: Transition, right: Transition) -> Transition {
    match (left.available, right.available) {
        (true, true) => {
            if left.span <= right.span {
                left
            } else {
                right
            }
        }
        (false, true) => left,
        (true, false) => right,
        (false, false) => {
            let left_key = (left.span, left.unavailable_kind);
            let right_key = (right.span, right.unavailable_kind);
            if left_key <= right_key { left } else { right }
        }
    }
}

fn join_nodes(left: &StateNode, right: &StateNode) -> StateNode {
    let transition = join_transition(left.transition, right.transition);
    let projections = left
        .children
        .keys()
        .chain(right.children.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    let mut children = BTreeMap::new();
    for projection in projections {
        let left_child = left
            .children
            .get(&projection)
            .cloned()
            .unwrap_or_else(|| StateNode::new(left.transition));
        let right_child = right
            .children
            .get(&projection)
            .cloned()
            .unwrap_or_else(|| StateNode::new(right.transition));
        let child = join_nodes(&left_child, &right_child);
        if child.transition != transition || !child.children.is_empty() {
            children.insert(projection, child);
        }
    }
    StateNode {
        transition,
        children,
    }
}

#[derive(Default)]
struct Flow {
    normal: Option<MoveState>,
    /// State after a path that cannot reach the following statement. It is
    /// retained only so unreachable source is still checked for local move
    /// errors; it never rejoins reachable flow.
    dead: Option<MoveState>,
    breaks: Vec<MoveState>,
    continues: Vec<MoveState>,
}

struct MoveChecker<'a> {
    resolved: &'a ResolvedProgram,
    typed: &'a mut TypedProgram,
    diagnostics: Vec<Diagnostic>,
    /// Fixed-point loop visits intentionally revisit expressions. Diagnostics
    /// are keyed by their invalid use and kind so convergence never duplicates
    /// user-facing errors.
    reported: BTreeSet<(Span, &'static str)>,
    root_types: BTreeMap<OwnershipPlaceRoot, TypeId>,
}

impl MoveChecker<'_> {
    fn check_function(&mut self, function: &TypedFunction) {
        self.root_types.clear();
        let parameter_ids = function
            .parameters
            .iter()
            .map(|parameter| parameter.binding)
            .collect::<BTreeSet<_>>();
        let mut state = MoveState::default();
        for (binding, ty) in &function.local_types {
            let span = self.resolved.local_bindings[binding.index()].span;
            let transition = if parameter_ids.contains(binding) {
                Transition::initialized(span)
            } else {
                Transition::uninitialized(span)
            };
            let root = OwnershipPlaceRoot::Local(*binding);
            self.root_types.insert(root, *ty);
            state.roots.insert(root, StateNode::new(transition));
        }
        if let Some(closure) = &function.closure {
            for (binding, index) in &closure.captures {
                if let Some(ty) = function.local_types.get(binding) {
                    let span = self.resolved.local_bindings[binding.index()].span;
                    let root = OwnershipPlaceRoot::ClosureCapture(*index);
                    self.root_types.insert(root, *ty);
                    state
                        .roots
                        .insert(root, StateNode::new(Transition::initialized(span)));
                }
            }
        }
        let _ = self.check_block(&function.body, state);
    }

    fn check_block(&mut self, statements: &[TypedStatement], state: MoveState) -> Flow {
        let mut flow = Flow {
            normal: Some(state),
            ..Flow::default()
        };
        let mut unreachable_state = None;
        for statement in statements {
            if let Some(current) = flow.normal.take() {
                let fallback = current.clone();
                let next = self.check_statement(statement, current);
                flow.normal = next.normal;
                flow.breaks.extend(next.breaks);
                flow.continues.extend(next.continues);
                if flow.normal.is_none() {
                    unreachable_state = next.dead.or(Some(fallback));
                }
            } else if let Some(current) = unreachable_state.clone() {
                // Unreachable source is still checked, matching the ordinary
                // body checker's bounded diagnostic behavior, but its state
                // never rejoins reachable control flow.
                let next = self.check_statement(statement, current.clone());
                unreachable_state = next.normal.or(Some(current));
            }
        }
        flow
    }

    fn check_statement(&mut self, statement: &TypedStatement, mut state: MoveState) -> Flow {
        match &statement.kind {
            TypedStatementKind::Let { binding, value, .. } => {
                self.visit_expression(value, &mut state, true);
                self.initialize_local(*binding, statement.span, &mut state);
                normal(state)
            }
            TypedStatementKind::Destructure {
                bindings, value, ..
            } => {
                self.visit_expression(value, &mut state, true);
                for binding in bindings {
                    self.initialize_local(binding.binding, statement.span, &mut state);
                }
                normal(state)
            }
            TypedStatementKind::Assign {
                place,
                operator,
                value,
            } => {
                self.visit_place_inputs(place, &mut state);
                self.visit_expression(value, &mut state, true);
                let ownership_place = normalized_place(place.ownership_place());
                if *operator != AssignmentOperator::Assign {
                    self.require_available(&ownership_place, place.ty(), place.span(), &state);
                }
                state.set(&ownership_place, Transition::initialized(place.span()));
                normal(state)
            }
            TypedStatementKind::Expression(expression) => {
                self.visit_expression(expression, &mut state, true);
                normal(state)
            }
            TypedStatementKind::Return(value) => {
                if let Some(value) = value {
                    self.visit_expression(value, &mut state, true);
                }
                Flow {
                    dead: Some(state),
                    ..Flow::default()
                }
            }
            TypedStatementKind::If {
                condition,
                then_body,
                else_body,
            } => {
                self.visit_expression(condition, &mut state, true);
                let then_flow = self.check_block(then_body, state.clone());
                let else_flow = self.check_block(else_body, state);
                Flow {
                    normal: MoveState::join(then_flow.normal.into_iter().chain(else_flow.normal)),
                    dead: MoveState::join(then_flow.dead.into_iter().chain(else_flow.dead)),
                    breaks: then_flow
                        .breaks
                        .into_iter()
                        .chain(else_flow.breaks)
                        .collect(),
                    continues: then_flow
                        .continues
                        .into_iter()
                        .chain(else_flow.continues)
                        .collect(),
                }
            }
            TypedStatementKind::While { condition, body } => {
                self.check_while(condition, body, state)
            }
            TypedStatementKind::For {
                binding,
                iterable,
                body,
                ..
            } => self.check_for(*binding, iterable, body, state, statement.span),
            TypedStatementKind::Match { scrutinee, arms } => {
                self.check_match(scrutinee, arms, state)
            }
            TypedStatementKind::Block(body) => self.check_block(body, state),
            TypedStatementKind::Defer(body) => {
                // Registration evaluates nothing. Validate the deferred source
                // against the registration state but leave its effects for the
                // deterministic-destruction milestone, which owns exit edges.
                let _ = self.check_block(body, state.clone());
                normal(state)
            }
            TypedStatementKind::Expect { selector, body, .. } => {
                self.visit_expression(selector, &mut state, true);
                let handler = self.check_block(body, state.clone());
                Flow {
                    normal: MoveState::join(state.into_iter().chain(handler.normal)),
                    dead: handler.dead,
                    breaks: handler.breaks,
                    continues: handler.continues,
                }
            }
            TypedStatementKind::Break => Flow {
                dead: Some(state.clone()),
                breaks: vec![state],
                ..Flow::default()
            },
            TypedStatementKind::Continue => Flow {
                dead: Some(state.clone()),
                continues: vec![state],
                ..Flow::default()
            },
            TypedStatementKind::Pass => normal(state),
        }
    }

    fn check_while(
        &mut self,
        condition: &TypedExpression,
        body: &[TypedStatement],
        entry: MoveState,
    ) -> Flow {
        let mut head = entry.clone();
        let mut breaks = Vec::new();
        for _ in 0..=self.transition_bound() {
            let mut condition_state = head.clone();
            self.visit_expression(condition, &mut condition_state, true);
            let body_flow = self.check_block(body, condition_state.clone());
            breaks.extend(body_flow.breaks.clone());
            let back = MoveState::join(
                body_flow
                    .normal
                    .into_iter()
                    .chain(body_flow.continues.into_iter()),
            );
            let next = MoveState::join(entry.clone().into_iter().chain(back));
            let Some(next) = next else { break };
            if next == head {
                return Flow {
                    normal: MoveState::join(condition_state.into_iter().chain(breaks)),
                    ..Flow::default()
                };
            }
            head = next;
        }
        let mut exit = head;
        self.visit_expression(condition, &mut exit, true);
        Flow {
            normal: MoveState::join(exit.into_iter().chain(breaks)),
            ..Flow::default()
        }
    }

    fn check_for(
        &mut self,
        binding: LocalBindingId,
        iterable: &TypedExpression,
        body: &[TypedStatement],
        mut state: MoveState,
        span: Span,
    ) -> Flow {
        self.visit_expression(iterable, &mut state, true);
        let entry = state;
        let mut head = entry.clone();
        let mut breaks = Vec::new();
        for _ in 0..=self.transition_bound() {
            let mut iteration = head.clone();
            self.initialize_local(binding, span, &mut iteration);
            let body_flow = self.check_block(body, iteration);
            breaks.extend(body_flow.breaks.clone());
            let back = MoveState::join(
                body_flow
                    .normal
                    .into_iter()
                    .chain(body_flow.continues.into_iter()),
            );
            let Some(next) = MoveState::join(entry.clone().into_iter().chain(back)) else {
                break;
            };
            if next == head {
                return Flow {
                    normal: MoveState::join(head.into_iter().chain(breaks)),
                    ..Flow::default()
                };
            }
            head = next;
        }
        Flow {
            normal: MoveState::join(head.into_iter().chain(breaks)),
            ..Flow::default()
        }
    }

    fn check_match(
        &mut self,
        scrutinee: &TypedExpression,
        arms: &[TypedMatchArm],
        mut state: MoveState,
    ) -> Flow {
        self.visit_expression(scrutinee, &mut state, true);
        let source = scrutinee.ownership_place().map(normalized_place);
        let mut normal_states = Vec::new();
        let mut dead_states = Vec::new();
        let mut breaks = Vec::new();
        let mut continues = Vec::new();
        for arm in arms {
            let mut arm_state = state.clone();
            self.apply_pattern(&arm.pattern, source.as_ref(), &mut arm_state);
            if let Some(guard) = &arm.guard {
                self.visit_expression(guard, &mut arm_state, true);
            }
            let flow = self.check_block(&arm.body, arm_state);
            normal_states.extend(flow.normal);
            dead_states.extend(flow.dead);
            breaks.extend(flow.breaks);
            continues.extend(flow.continues);
        }
        Flow {
            normal: MoveState::join(normal_states),
            dead: MoveState::join(dead_states),
            breaks,
            continues,
        }
    }

    fn apply_pattern(
        &mut self,
        pattern: &TypedPattern,
        source: Option<&OwnershipPlace>,
        state: &mut MoveState,
    ) {
        match &pattern.kind {
            TypedPatternKind::Wildcard => {
                self.consume_pattern_source(source, pattern.ty, pattern.span, state);
            }
            TypedPatternKind::Binding(binding) => {
                self.consume_pattern_source(source, pattern.ty, pattern.span, state);
                self.initialize_local(*binding, pattern.span, state);
            }
            TypedPatternKind::Literal(_) => {}
            TypedPatternKind::Alternative(alternatives) => {
                let states = alternatives
                    .iter()
                    .map(|alternative| {
                        let mut alternative_state = state.clone();
                        self.apply_pattern(alternative, source, &mut alternative_state);
                        alternative_state
                    })
                    .collect::<Vec<_>>();
                if let Some(joined) = MoveState::join(states) {
                    *state = joined;
                }
            }
            TypedPatternKind::Dereference(inner) => {
                let projected =
                    source.map(|source| projected(source, OwnershipProjection::Dereference));
                self.apply_pattern(inner, projected.as_ref(), state);
            }
            TypedPatternKind::Tuple(elements) => {
                for (index, element) in elements.iter().enumerate() {
                    let projected = source
                        .map(|source| projected(source, OwnershipProjection::TupleField(index)));
                    self.apply_pattern(element, projected.as_ref(), state);
                }
            }
            TypedPatternKind::Struct { fields, .. } => {
                for (field, child) in fields {
                    let projected =
                        source.map(|source| projected(source, OwnershipProjection::Field(*field)));
                    self.apply_pattern(child, projected.as_ref(), state);
                }
            }
            TypedPatternKind::Variant { fields, .. } => {
                // Variant payloads cannot leave the enclosing enum partially
                // initialized. A move from any payload consumes the enum.
                let consumes = fields
                    .iter()
                    .any(|(_, child)| self.pattern_consumes_non_copy(child));
                if consumes {
                    self.consume_pattern_source(source, pattern.ty, pattern.span, state);
                }
                for (_, child) in fields {
                    self.initialize_pattern_bindings(child, state);
                }
            }
        }
    }

    fn pattern_consumes_non_copy(&mut self, pattern: &TypedPattern) -> bool {
        match &pattern.kind {
            TypedPatternKind::Wildcard | TypedPatternKind::Binding(_) => {
                !crate::types::ownership_facts(self.resolved, self.typed, pattern.ty)
                    .copy
                    .is_present()
            }
            TypedPatternKind::Literal(_) => false,
            TypedPatternKind::Alternative(patterns) | TypedPatternKind::Tuple(patterns) => patterns
                .iter()
                .any(|pattern| self.pattern_consumes_non_copy(pattern)),
            TypedPatternKind::Dereference(pattern) => self.pattern_consumes_non_copy(pattern),
            TypedPatternKind::Struct { fields, .. } | TypedPatternKind::Variant { fields, .. } => {
                fields
                    .iter()
                    .any(|(_, pattern)| self.pattern_consumes_non_copy(pattern))
            }
        }
    }

    fn initialize_pattern_bindings(&mut self, pattern: &TypedPattern, state: &mut MoveState) {
        match &pattern.kind {
            TypedPatternKind::Binding(binding) => {
                self.initialize_local(*binding, pattern.span, state);
            }
            TypedPatternKind::Alternative(patterns) | TypedPatternKind::Tuple(patterns) => {
                for pattern in patterns {
                    self.initialize_pattern_bindings(pattern, state);
                }
            }
            TypedPatternKind::Dereference(pattern) => {
                self.initialize_pattern_bindings(pattern, state);
            }
            TypedPatternKind::Struct { fields, .. } | TypedPatternKind::Variant { fields, .. } => {
                for (_, pattern) in fields {
                    self.initialize_pattern_bindings(pattern, state);
                }
            }
            TypedPatternKind::Wildcard | TypedPatternKind::Literal(_) => {}
        }
    }

    fn consume_pattern_source(
        &mut self,
        source: Option<&OwnershipPlace>,
        ty: TypeId,
        span: Span,
        state: &mut MoveState,
    ) {
        let facts = crate::types::ownership_facts(self.resolved, self.typed, ty);
        if facts.copy.is_present() {
            if let Some(source) = source {
                self.require_available(source, ty, span, state);
            }
        } else if let Some(source) = source {
            self.move_place(source, ty, span, state);
        }
    }

    fn visit_expression(
        &mut self,
        expression: &TypedExpression,
        state: &mut MoveState,
        apply_self: bool,
    ) {
        match &expression.kind {
            TypedExpressionKind::Constant(_)
            | TypedExpressionKind::FunctionReference(_)
            | TypedExpressionKind::Local(_)
            | TypedExpressionKind::ClosureCapture(_)
            | TypedExpressionKind::DefaultValue(_) => {}
            TypedExpressionKind::Closure { captures, .. }
            | TypedExpressionKind::Tuple(captures)
            | TypedExpressionKind::Array(captures)
            | TypedExpressionKind::VariadicSlice(captures) => {
                for capture in captures {
                    self.visit_expression(capture, state, true);
                }
            }
            TypedExpressionKind::Unary { operand, .. }
            | TypedExpressionKind::Cast { value: operand }
            | TypedExpressionKind::Dereference(operand)
            | TypedExpressionKind::AddressOfTemporary(operand)
            | TypedExpressionKind::MakeTraitObject { value: operand, .. }
            | TypedExpressionKind::Propagate { operand, .. } => {
                self.visit_expression(operand, state, true);
            }
            TypedExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                self.visit_expression(left, state, true);
                if matches!(
                    operator,
                    BinaryOperator::LogicalAnd | BinaryOperator::LogicalOr
                ) {
                    let mut right_state = state.clone();
                    self.visit_expression(right, &mut right_state, true);
                    if let Some(joined) = MoveState::join([state.clone(), right_state]) {
                        *state = joined;
                    }
                } else {
                    self.visit_expression(right, state, true);
                }
            }
            TypedExpressionKind::Call {
                callee, arguments, ..
            } => {
                match callee {
                    TypedCallee::Indirect(value) => self.visit_expression(value, state, true),
                    TypedCallee::Closure { value, .. } => {
                        self.visit_expression(value, state, true);
                    }
                    TypedCallee::Function(_)
                    | TypedCallee::Dynamic { .. }
                    | TypedCallee::Print { .. } => {}
                }
                for argument in arguments {
                    self.visit_expression(argument, state, true);
                }
            }
            TypedExpressionKind::Field { base, .. }
            | TypedExpressionKind::TupleField { base, .. } => {
                self.visit_expression(base, state, false);
            }
            TypedExpressionKind::Index { base, index } => {
                self.visit_expression(base, state, false);
                self.visit_expression(index, state, true);
            }
            TypedExpressionKind::AddressOf(place) => self.visit_place_inputs(place, state),
            TypedExpressionKind::NumericConversion { value, .. } => {
                self.visit_expression(value, state, true);
            }
            TypedExpressionKind::NumericAlternative {
                receiver, operand, ..
            } => {
                self.visit_expression(receiver, state, true);
                if let Some(operand) = operand {
                    self.visit_expression(operand, state, true);
                }
            }
            TypedExpressionKind::StandardCall { arguments, .. }
            | TypedExpressionKind::CollectionLiteral {
                elements: arguments,
                ..
            } => {
                for argument in arguments {
                    self.visit_expression(argument, state, true);
                }
            }
            TypedExpressionKind::Struct { fields, .. }
            | TypedExpressionKind::Enum { fields, .. } => {
                for (_, value) in fields {
                    self.visit_expression(value, state, true);
                }
            }
            TypedExpressionKind::FormattedString(parts) => {
                for part in parts {
                    if let FormattedPart::Expression(value) = part {
                        self.visit_expression(value, state, true);
                    }
                }
            }
        }
        if apply_self {
            let borrows_place = matches!(expression.kind, TypedExpressionKind::AddressOf(_))
                && expression.ownership.iter().any(|operation| {
                    matches!(
                        operation.kind,
                        OwnershipUseKind::BorrowShared
                            | OwnershipUseKind::BorrowExclusive
                            | OwnershipUseKind::ReborrowShared
                            | OwnershipUseKind::ReborrowExclusive
                    )
                });
            for operation in &expression.ownership {
                let Some(place) = operation.place.clone().map(normalized_place) else {
                    continue;
                };
                if borrows_place
                    && matches!(
                        operation.kind,
                        OwnershipUseKind::Move | OwnershipUseKind::Copy
                    )
                {
                    // This operation transfers the newly produced reference,
                    // not the borrowed source place attached to the same
                    // expression for provenance analysis.
                    continue;
                }
                match operation.kind {
                    OwnershipUseKind::Move => {
                        self.move_place(&place, expression.ty, expression.span, state);
                    }
                    OwnershipUseKind::Produce
                    | OwnershipUseKind::Copy
                    | OwnershipUseKind::Clone
                    | OwnershipUseKind::BorrowShared
                    | OwnershipUseKind::BorrowExclusive
                    | OwnershipUseKind::ReborrowShared
                    | OwnershipUseKind::ReborrowExclusive
                    | OwnershipUseKind::Drop
                    | OwnershipUseKind::LegacyCopy => {
                        self.require_available(&place, expression.ty, expression.span, state);
                    }
                }
            }
        }
    }

    fn visit_place_inputs(&mut self, place: &TypedPlace, state: &mut MoveState) {
        match place {
            TypedPlace::Local { .. } | TypedPlace::ClosureCapture { .. } => {}
            TypedPlace::Field { base, .. } | TypedPlace::TupleField { base, .. } => {
                self.visit_place_inputs(base, state);
            }
            TypedPlace::Index { base, index, .. } => {
                self.visit_place_inputs(base, state);
                self.visit_expression(index, state, true);
            }
            TypedPlace::Dereference { base, .. } => {
                self.visit_expression(base, state, true);
            }
        }
    }

    fn move_place(
        &mut self,
        place: &OwnershipPlace,
        ty: TypeId,
        span: Span,
        state: &mut MoveState,
    ) {
        if self.invalid_move_projection(place, span) {
            return;
        }
        if !self.require_available(place, ty, span, state) {
            return;
        }
        if self.partially_moves_custom_drop(place) {
            self.report(
                span,
                "partial-drop",
                Diagnostic::new(
                    Category::Ownership,
                    "cannot partially move a value with a custom `Drop` implementation",
                )
                .with_primary(span),
            );
            return;
        }
        state.set(place, Transition::moved(span));
    }

    fn partially_moves_custom_drop(&mut self, place: &OwnershipPlace) -> bool {
        let mut current = self.root_types.get(&place.root).copied();
        for projection in &place.projections {
            if current.is_some_and(|ty| self.has_custom_drop(ty)) {
                return true;
            }
            current = current.and_then(|ty| self.project_type(ty, *projection));
        }
        false
    }

    fn invalid_move_projection(&mut self, place: &OwnershipPlace, span: Span) -> bool {
        let mut current = self.root_types.get(&place.root).copied();
        for projection in &place.projections {
            match projection {
                OwnershipProjection::Dereference => {
                    self.report(
                        span,
                        "move-dereference",
                        Diagnostic::new(
                            Category::Ownership,
                            "cannot move a non-`Copy` value through a borrowed or raw pointer place",
                        )
                        .with_primary(span),
                    );
                    return true;
                }
                OwnershipProjection::DynamicIndex => {
                    self.report_indexed_move(span);
                    return true;
                }
                OwnershipProjection::ConstantIndex(_) => {
                    let array = current
                        .is_some_and(|ty| matches!(self.expanded_kind(ty), TypeKind::Array { .. }));
                    if !array {
                        self.report_indexed_move(span);
                        return true;
                    }
                }
                OwnershipProjection::ReceiverAdaptation => continue,
                OwnershipProjection::Field(_) | OwnershipProjection::TupleField(_) => {}
            }
            current = current.and_then(|ty| self.project_type(ty, *projection));
        }
        false
    }

    fn report_indexed_move(&mut self, span: Span) {
        self.report(
            span,
            "move-index",
            Diagnostic::new(
                Category::Ownership,
                "cannot move a non-`Copy` value out of an indexed place; use an \
                 ownership-taking operation such as `remove`, `pop`, or `take`",
            )
            .with_primary(span),
        );
    }

    fn require_available(
        &mut self,
        place: &OwnershipPlace,
        ty: TypeId,
        span: Span,
        state: &MoveState,
    ) -> bool {
        let Some(cause) = self.unavailable_cause(place, ty, state) else {
            return true;
        };
        let (key, message, related) = match cause.unavailable_kind {
            UnavailableKind::Moved => (
                "use-after-move",
                "cannot use this place because it is moved or partially moved",
                "this consuming use moved the value",
            ),
            UnavailableKind::Uninitialized => (
                "use-before-init",
                "cannot use this place before it is definitely initialized",
                "the place is uninitialized from here",
            ),
        };
        self.report(
            span,
            key,
            Diagnostic::new(Category::Ownership, message)
                .with_primary(span)
                .with_related(cause.span, related),
        );
        false
    }

    fn unavailable_cause(
        &mut self,
        place: &OwnershipPlace,
        ty: TypeId,
        state: &MoveState,
    ) -> Option<Transition> {
        let mut node = state.roots.get(&place.root)?.clone();
        let mut inherited = node.transition;
        let mut current_ty = self.root_types.get(&place.root).copied();
        for projection in &place.projections {
            if *projection == OwnershipProjection::DynamicIndex {
                return self.first_unavailable(&node, inherited, current_ty.unwrap_or(ty));
            }
            if let Some(child) = node.children.get(projection).cloned() {
                inherited = node.transition;
                node = child;
            } else {
                inherited = node.transition;
                node = StateNode::new(inherited);
            }
            current_ty = current_ty.and_then(|current| self.project_type(current, *projection));
        }
        self.first_unavailable(&node, inherited, current_ty.unwrap_or(ty))
    }

    fn first_unavailable(
        &mut self,
        node: &StateNode,
        inherited: Transition,
        ty: TypeId,
    ) -> Option<Transition> {
        let effective = node.transition;
        if let TypeKind::Array { element, length } = self.expanded_kind(ty).clone() {
            if effective.available {
                for child in node.children.values() {
                    if let Some(cause) = self.first_unavailable(child, effective, element) {
                        return Some(cause);
                    }
                }
                return None;
            }
            let initialized_indices = node
                .children
                .keys()
                .filter_map(|projection| match projection {
                    OwnershipProjection::ConstantIndex(index) if *index < length => Some(*index),
                    _ => None,
                })
                .collect::<BTreeSet<_>>();
            if initialized_indices.len() as u128 != length
                || initialized_indices.len() != node.children.len()
            {
                return Some(effective);
            }
            for child in node.children.values() {
                if let Some(cause) = self.first_unavailable(child, effective, element) {
                    return Some(cause);
                }
            }
            return None;
        }
        let structural = self.structural_children(ty);
        if effective.available {
            for (projection, child) in &node.children {
                let child_ty = structural
                    .iter()
                    .find_map(|(candidate, ty)| (candidate == projection).then_some(*ty))
                    .unwrap_or(ty);
                if let Some(cause) = self.first_unavailable(child, effective, child_ty) {
                    return Some(cause);
                }
            }
            return None;
        }
        if structural.is_empty() {
            return Some(effective);
        }
        for (projection, child_ty) in structural {
            let child = node
                .children
                .get(&projection)
                .cloned()
                .unwrap_or_else(|| StateNode::new(effective));
            if let Some(cause) = self.first_unavailable(&child, effective, child_ty) {
                return Some(cause);
            }
        }
        let _ = inherited;
        None
    }

    fn structural_children(&mut self, ty: TypeId) -> Vec<(OwnershipProjection, TypeId)> {
        match self.expanded_kind(ty).clone() {
            TypeKind::Tuple(elements) => elements
                .into_iter()
                .enumerate()
                .map(|(index, ty)| (OwnershipProjection::TupleField(index), ty))
                .collect(),
            TypeKind::Array { .. } => Vec::new(),
            TypeKind::Nominal {
                identity,
                arguments,
            } if self.resolved.declarations[identity.declaration.index()].kind
                != DeclarationKind::Enum =>
            {
                self.nominal_children(identity.declaration, &arguments)
            }
            TypeKind::Foreign {
                identity,
                complete: true,
            } => self.nominal_children(identity.declaration, &[]),
            _ => Vec::new(),
        }
    }

    fn nominal_children(
        &mut self,
        declaration: crate::resolution::DeclarationId,
        arguments: &[TypeId],
    ) -> Vec<(OwnershipProjection, TypeId)> {
        self.resolved
            .fields
            .iter()
            .filter(|field| {
                field.parent_declaration == declaration && field.parent_variant.is_none()
            })
            .map(|field| field.id)
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|field| {
                self.typed
                    .instantiate_field_type(self.resolved, field, arguments)
                    .map(|ty| (OwnershipProjection::Field(field), ty))
            })
            .collect()
    }

    fn project_type(&mut self, ty: TypeId, projection: OwnershipProjection) -> Option<TypeId> {
        match projection {
            OwnershipProjection::ReceiverAdaptation => Some(ty),
            OwnershipProjection::Dereference => match self.expanded_kind(ty) {
                TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => {
                    Some(*target)
                }
                _ => None,
            },
            OwnershipProjection::TupleField(index) => match self.expanded_kind(ty) {
                TypeKind::Tuple(elements) => elements.get(index).copied(),
                _ => None,
            },
            OwnershipProjection::ConstantIndex(_) | OwnershipProjection::DynamicIndex => match self
                .expanded_kind(ty)
            {
                TypeKind::Array { element, .. } | TypeKind::Slice { element, .. } => Some(*element),
                TypeKind::Builtin { builtin, arguments } => {
                    match (self.resolved.builtin_name(*builtin), arguments.as_slice()) {
                        ("Vec" | "Set", [element]) => Some(*element),
                        ("Map", [_, value]) => Some(*value),
                        _ => None,
                    }
                }
                _ => None,
            },
            OwnershipProjection::Field(field) => match self.expanded_kind(ty).clone() {
                TypeKind::Nominal { arguments, .. } => {
                    self.typed
                        .instantiate_field_type(self.resolved, field, &arguments)
                }
                TypeKind::Foreign { complete: true, .. } => {
                    self.typed.instantiate_field_type(self.resolved, field, &[])
                }
                _ => None,
            },
        }
    }

    fn has_custom_drop(&mut self, ty: TypeId) -> bool {
        let Some(drop_trait) = self.resolved.standard_declaration("Drop") else {
            return false;
        };
        crate::traits::implements_trait(self.resolved, self.typed, ty, drop_trait)
    }

    fn expanded_kind(&self, mut ty: TypeId) -> &TypeKind {
        loop {
            ty = self.typed.types.resolve_inference(ty);
            match self.typed.types.kind(ty) {
                TypeKind::Alias { target, .. } => ty = *target,
                kind => return kind,
            }
        }
    }

    fn initialize_local(&self, binding: LocalBindingId, span: Span, state: &mut MoveState) {
        state.set(
            &OwnershipPlace {
                root: OwnershipPlaceRoot::Local(binding),
                projections: Vec::new(),
            },
            Transition::initialized(span),
        );
    }

    fn transition_bound(&self) -> usize {
        self.root_types.len().saturating_mul(4).max(8)
    }

    fn report(&mut self, span: Span, key: &'static str, diagnostic: Diagnostic) {
        if self.reported.insert((span, key)) {
            self.diagnostics.push(diagnostic);
        }
    }
}

fn normal(state: MoveState) -> Flow {
    Flow {
        normal: Some(state),
        ..Flow::default()
    }
}

fn normalized_place(mut place: OwnershipPlace) -> OwnershipPlace {
    place
        .projections
        .retain(|projection| *projection != OwnershipProjection::ReceiverAdaptation);
    place
}

fn projected(place: &OwnershipPlace, projection: OwnershipProjection) -> OwnershipPlace {
    let mut place = place.clone();
    place.projections.push(projection);
    place
}

trait IntoStateIterator {
    type Iterator: Iterator<Item = MoveState>;
    fn into_iter(self) -> Self::Iterator;
}

impl IntoStateIterator for MoveState {
    type Iterator = std::iter::Once<MoveState>;

    fn into_iter(self) -> Self::Iterator {
        std::iter::once(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn span(offset: u32) -> Span {
        let mut sources = crate::source::SourceManager::new();
        let file = sources.add_text("moves.elx".into(), " ".repeat(64));
        Span::new(file, offset, offset)
    }

    fn transition(available: bool, offset: u32) -> Transition {
        if available {
            Transition::initialized(span(offset))
        } else {
            Transition::moved(span(offset))
        }
    }

    fn node(root: bool, child: Option<bool>, offset: u32) -> StateNode {
        let mut node = StateNode::new(transition(root, offset));
        if let Some(child) = child {
            node.children.insert(
                OwnershipProjection::TupleField(0),
                StateNode::new(transition(child, offset + 1)),
            );
        }
        node
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn joins_are_commutative_associative_and_never_grant_availability(
            a_root in any::<bool>(),
            a_child in proptest::option::of(any::<bool>()),
            b_root in any::<bool>(),
            b_child in proptest::option::of(any::<bool>()),
            c_root in any::<bool>(),
            c_child in proptest::option::of(any::<bool>()),
        ) {
            let a = node(a_root, a_child, 1);
            let b = node(b_root, b_child, 3);
            let c = node(c_root, c_child, 5);

            prop_assert_eq!(join_nodes(&a, &b), join_nodes(&b, &a));
            prop_assert_eq!(
                join_nodes(&join_nodes(&a, &b), &c),
                join_nodes(&a, &join_nodes(&b, &c)),
            );
            prop_assert_eq!(join_nodes(&a, &b).transition.available, a_root && b_root);
        }
    }

    #[test]
    fn whole_place_reinitialization_clears_partial_move_markers() {
        let mut node = StateNode::new(Transition::initialized(span(0)));
        node.set(
            &[OwnershipProjection::TupleField(0)],
            Transition::moved(span(1)),
        );
        assert!(!node.children.is_empty());
        node.set(&[], Transition::initialized(span(2)));
        assert!(node.transition.available);
        assert!(node.children.is_empty());
    }
}
