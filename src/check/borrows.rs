//! Structural borrow provenance and non-lexical loan checking.
//!
//! Elamite deliberately has no written lifetime parameters. This pass infers
//! one public return-source relationship per callable and tracks concrete
//! loans through typed values, projections, calls, aggregates, and control
//! flow. Liveness is solved backwards; access checking then runs forwards.

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostics::{Category, Diagnostic};
use crate::ir::{
    AssignmentOperator, BinaryOperator, FormattedPart, TypedCallee, TypedExpression,
    TypedExpressionKind, TypedFunction, TypedIrProgram, TypedPlace, TypedStatement,
    TypedStatementKind,
};
use crate::operations::{
    CapabilityState, OwnershipPlace, OwnershipPlaceRoot, OwnershipProjection, OwnershipUseKind,
    PlaceOverlap, StandardCall,
};
use crate::resolution::{LocalBindingId, ResolvedProgram};
use crate::source::Span;
use crate::types::{FunctionProvenance, ProvenanceSource, TypeId, TypeKind, TypedProgram};

use super::LoanId;

/// Infers the sole input source for every result that may contain a borrow.
/// These facts are retained in [`TypedProgram`] as dependency-facing metadata.
pub fn infer_provenance_signatures(
    resolved: &ResolvedProgram,
    typed: &mut TypedProgram,
) -> Vec<Diagnostic> {
    if !resolved.semantic_revision.supports_owned_surface() {
        return Vec::new();
    }
    typed.function_provenance.clear();
    let declarations = typed
        .function_signatures
        .keys()
        .copied()
        .collect::<Vec<_>>();
    let mut diagnostics = Vec::new();
    for declaration in declarations {
        if resolved.declarations[declaration.index()].kind
            == crate::resolution::DeclarationKind::Closure
        {
            continue;
        }
        let Some(signature) = typed.function_signatures.get(&declaration).cloned() else {
            continue;
        };
        let result =
            crate::types::ownership_facts(resolved, typed, signature.return_type).contains_borrow;
        if matches!(result, CapabilityState::Absent | CapabilityState::Error) {
            continue;
        }
        let mut candidates = Vec::new();
        if signature
            .receiver
            .is_some_and(|receiver| borrow_capability(resolved, typed, receiver))
        {
            // A borrow-bearing receiver is the designated result source even
            // when ordinary parameters also contain borrows.
            candidates.push(ProvenanceSource::Receiver);
        } else {
            for (index, parameter) in signature.parameters.iter().enumerate() {
                if borrow_capability(resolved, typed, parameter.ty) {
                    candidates.push(ProvenanceSource::Parameter(index));
                }
            }
        }
        let declaration_data = &resolved.declarations[declaration.index()];
        let span = declaration_data.span;
        let name = resolved.symbol_text(declaration_data.name);
        let is_standard = resolved.modules[declaration_data.module.index()].origin
            == crate::resolution::ModuleOrigin::Standard;
        match candidates.as_slice() {
            [source] => {
                typed.function_provenance.insert(
                    declaration,
                    FunctionProvenance {
                        returned_from: *source,
                    },
                );
            }
            [] => {
                typed.function_provenance.insert(
                    declaration,
                    FunctionProvenance {
                        returned_from: ProvenanceSource::Static,
                    },
                );
            }
            _ if is_standard => {
                // The shipped source library is still the 0.10 baseline. Its
                // few body-selected view APIs remain safe during migration by
                // retaining every possible input until their owned signatures
                // are migrated and can expose one exact structural source.
                typed.function_provenance.insert(
                    declaration,
                    FunctionProvenance {
                        returned_from: ProvenanceSource::AllBorrowingInputs,
                    },
                );
            }
            _ => diagnostics.push(
                Diagnostic::new(
                    Category::Borrow,
                    format!(
                        "the borrow-bearing return of `{name}` is ambiguous because more than one \
                         input could supply its provenance"
                    ),
                )
                .with_primary(span),
            ),
        }
    }
    diagnostics
}

fn borrow_capability(resolved: &ResolvedProgram, typed: &mut TypedProgram, ty: TypeId) -> bool {
    matches!(
        crate::types::ownership_facts(resolved, typed, ty).contains_borrow,
        CapabilityState::Present | CapabilityState::Conditional
    )
}

/// Checks concrete loans and returned provenance after move analysis.
#[must_use]
pub fn check_borrows(
    resolved: &ResolvedProgram,
    typed: &mut TypedProgram,
    program: &TypedIrProgram,
) -> Vec<Diagnostic> {
    if !program.semantic_revision.supports_owned_surface() {
        return Vec::new();
    }
    let mut checker = BorrowChecker {
        resolved,
        typed,
        diagnostics: Vec::new(),
        reported: BTreeSet::new(),
        loan_ids: BTreeMap::new(),
        root_types: BTreeMap::new(),
        parameter_sources: BTreeMap::new(),
        liveness: BTreeMap::new(),
        current_return_source: None,
    };
    checker.assign_loan_ids(program);
    for function in &program.functions {
        checker.check_function(function);
    }
    checker.diagnostics
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoanKind {
    Shared,
    Exclusive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Loan {
    places: BTreeSet<OwnershipPlace>,
    kind: LoanKind,
    span: Span,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Origins {
    loans: BTreeSet<LoanId>,
    inputs: BTreeSet<ProvenanceSource>,
    locals: BTreeSet<OwnershipPlaceRoot>,
    temporary: bool,
}

impl Origins {
    fn union_with(&mut self, other: &Self) {
        self.loans.extend(other.loans.iter().copied());
        self.inputs.extend(other.inputs.iter().copied());
        self.locals.extend(other.locals.iter().copied());
        self.temporary |= other.temporary;
    }
}

/// Provenance is stored structurally by path. Projection selects and strips a
/// prefix; aggregate construction adds one. This prevents an unrelated field
/// from inheriting another field's loan merely because both share a wrapper.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ValueProvenance {
    paths: BTreeMap<Vec<OwnershipProjection>, Origins>,
}

impl ValueProvenance {
    fn scalar(origins: Origins) -> Self {
        let mut paths = BTreeMap::new();
        if origins != Origins::default() {
            paths.insert(Vec::new(), origins);
        }
        Self { paths }
    }

    fn union_with(&mut self, other: &Self) {
        for (path, origins) in &other.paths {
            self.paths
                .entry(path.clone())
                .or_default()
                .union_with(origins);
        }
    }

    fn prefixed(&self, projection: OwnershipProjection) -> Self {
        Self {
            paths: self
                .paths
                .iter()
                .map(|(path, origins)| {
                    let mut result = Vec::with_capacity(path.len() + 1);
                    result.push(projection);
                    result.extend(path.iter().copied());
                    (result, origins.clone())
                })
                .collect(),
        }
    }

    fn projected(&self, projection: OwnershipProjection) -> Self {
        let mut result = Self::default();
        for (path, origins) in &self.paths {
            if path.first() == Some(&projection) {
                result
                    .paths
                    .entry(path[1..].to_vec())
                    .or_default()
                    .union_with(origins);
            } else if path.is_empty() {
                // A scalar reference is automatically dereferenced by field,
                // tuple, method, and indexing adaptation.
                result
                    .paths
                    .entry(Vec::new())
                    .or_default()
                    .union_with(origins);
            }
        }
        result
    }

    fn all_origins(&self) -> Origins {
        let mut result = Origins::default();
        for origins in self.paths.values() {
            result.union_with(origins);
        }
        result
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BorrowState {
    loans: BTreeMap<LoanId, Loan>,
    values: BTreeMap<OwnershipPlaceRoot, ValueProvenance>,
}

impl BorrowState {
    fn join(states: impl IntoIterator<Item = Self>) -> Option<Self> {
        let mut states = states.into_iter();
        let mut result = states.next()?;
        for state in states {
            result.loans.extend(state.loans);
            for (root, provenance) in state.values {
                result
                    .values
                    .entry(root)
                    .or_default()
                    .union_with(&provenance);
            }
        }
        Some(result)
    }
}

#[derive(Default)]
struct Flow {
    normal: Option<BorrowState>,
    breaks: Vec<BorrowState>,
    continues: Vec<BorrowState>,
}

struct BorrowChecker<'a> {
    resolved: &'a ResolvedProgram,
    typed: &'a mut TypedProgram,
    diagnostics: Vec<Diagnostic>,
    reported: BTreeSet<(Span, &'static str)>,
    loan_ids: BTreeMap<Span, LoanId>,
    root_types: BTreeMap<OwnershipPlaceRoot, TypeId>,
    parameter_sources: BTreeMap<OwnershipPlaceRoot, ProvenanceSource>,
    liveness: BTreeMap<Span, BTreeSet<LocalBindingId>>,
    current_return_source: Option<ProvenanceSource>,
}

impl BorrowChecker<'_> {
    fn assign_loan_ids(&mut self, program: &TypedIrProgram) {
        let mut spans = BTreeSet::new();
        for function in &program.functions {
            collect_borrow_spans(&function.body, &mut spans);
        }
        self.loan_ids = spans
            .into_iter()
            .enumerate()
            .map(|(index, span)| (span, LoanId(index as u32)))
            .collect();
    }

    fn check_function(&mut self, function: &TypedFunction) {
        self.root_types.clear();
        self.parameter_sources.clear();
        self.liveness.clear();
        self.current_return_source = if function.closure.is_some() {
            None
        } else {
            self.typed
                .function_provenance
                .get(&function.declaration)
                .map(|facts| facts.returned_from)
        };
        let mut state = BorrowState::default();
        let signature = self
            .typed
            .function_instance_signatures
            .get(&function.instance)
            .cloned()
            .or_else(|| {
                self.typed
                    .function_signatures
                    .get(&function.declaration)
                    .cloned()
            });
        let has_receiver = signature
            .as_ref()
            .is_some_and(|value| value.receiver.is_some());
        for (index, parameter) in function.parameters.iter().enumerate() {
            let root = OwnershipPlaceRoot::Local(parameter.binding);
            self.root_types.insert(root, parameter.ty);
            let source = if has_receiver && index == 0 {
                ProvenanceSource::Receiver
            } else {
                ProvenanceSource::Parameter(index - usize::from(has_receiver))
            };
            self.parameter_sources.insert(root, source);
            if borrow_capability(self.resolved, self.typed, parameter.ty) {
                let mut origins = Origins::default();
                origins.inputs.insert(source);
                state.values.insert(root, ValueProvenance::scalar(origins));
            }
        }
        for (binding, ty) in &function.local_types {
            self.root_types
                .entry(OwnershipPlaceRoot::Local(*binding))
                .or_insert(*ty);
        }
        if let Some(closure) = &function.closure {
            for (binding, index) in &closure.captures {
                let Some(ty) = function.local_types.get(binding).copied() else {
                    continue;
                };
                let root = OwnershipPlaceRoot::ClosureCapture(*index);
                self.root_types.insert(root, ty);
                if borrow_capability(self.resolved, self.typed, ty) {
                    let mut origins = Origins::default();
                    origins
                        .inputs
                        .insert(ProvenanceSource::ClosureCapture(*index));
                    state.values.insert(root, ValueProvenance::scalar(origins));
                }
            }
        }
        let mut live = BTreeSet::new();
        liveness_block(&function.body, &mut live, &mut self.liveness);
        let _ = self.check_block(&function.body, state);
    }

    fn check_block(&mut self, statements: &[TypedStatement], state: BorrowState) -> Flow {
        let mut flow = Flow {
            normal: Some(state),
            ..Flow::default()
        };
        for statement in statements {
            let Some(current) = flow.normal.take() else {
                break;
            };
            let next = self.check_statement(statement, current);
            flow.normal = next.normal;
            flow.breaks.extend(next.breaks);
            flow.continues.extend(next.continues);
        }
        flow
    }

    fn check_statement(&mut self, statement: &TypedStatement, mut state: BorrowState) -> Flow {
        let live = self
            .liveness
            .get(&statement.span)
            .cloned()
            .unwrap_or_default();
        let mut ephemeral = BTreeSet::new();
        match &statement.kind {
            TypedStatementKind::Let { binding, value, .. } => {
                let provenance =
                    self.visit_expression(value, &mut state, &live, &mut ephemeral, true);
                state
                    .values
                    .insert(OwnershipPlaceRoot::Local(*binding), provenance);
                normal(state)
            }
            TypedStatementKind::Destructure {
                bindings, value, ..
            } => {
                let provenance =
                    self.visit_expression(value, &mut state, &live, &mut ephemeral, true);
                for binding in bindings {
                    let mut selected = provenance.clone();
                    for index in &binding.indices {
                        selected = selected.projected(OwnershipProjection::TupleField(*index));
                    }
                    state
                        .values
                        .insert(OwnershipPlaceRoot::Local(binding.binding), selected);
                }
                normal(state)
            }
            TypedStatementKind::Assign {
                place,
                operator,
                value,
            } => {
                self.visit_place_inputs(place, &mut state, &live, &mut ephemeral);
                let provenance =
                    self.visit_expression(value, &mut state, &live, &mut ephemeral, true);
                let target = normalized_place(place.ownership_place());
                let authorized = self.authorized_loans(&target, &state);
                self.check_access(
                    &target,
                    if *operator == AssignmentOperator::Assign {
                        AccessKind::Write
                    } else {
                        AccessKind::ReadWrite
                    },
                    place.span(),
                    &state,
                    &live,
                    &ephemeral,
                    &authorized,
                );
                self.assign_provenance(&target, provenance, &mut state);
                normal(state)
            }
            TypedStatementKind::Expression(value) => {
                self.visit_expression(value, &mut state, &live, &mut ephemeral, true);
                normal(state)
            }
            TypedStatementKind::Return(value) => {
                if let Some(value) = value {
                    let provenance =
                        self.visit_expression(value, &mut state, &live, &mut ephemeral, true);
                    self.check_return_provenance(value.span, &provenance);
                }
                Flow::default()
            }
            TypedStatementKind::If {
                condition,
                then_body,
                else_body,
            } => {
                self.visit_expression(condition, &mut state, &live, &mut ephemeral, true);
                let left = self.check_block(then_body, state.clone());
                let right = self.check_block(else_body, state);
                Flow {
                    normal: BorrowState::join(left.normal.into_iter().chain(right.normal)),
                    breaks: left.breaks.into_iter().chain(right.breaks).collect(),
                    continues: left.continues.into_iter().chain(right.continues).collect(),
                }
            }
            TypedStatementKind::While { condition, body } => {
                self.check_loop(Some(condition), None, body, state, statement.span)
            }
            TypedStatementKind::For {
                binding,
                iterable,
                body,
                ..
            } => self.check_loop(Some(iterable), Some(*binding), body, state, statement.span),
            TypedStatementKind::Match { scrutinee, arms } => {
                self.visit_expression(scrutinee, &mut state, &live, &mut ephemeral, true);
                let mut normals = Vec::new();
                let mut breaks = Vec::new();
                let mut continues = Vec::new();
                for arm in arms {
                    let mut arm_state = state.clone();
                    if let Some(guard) = &arm.guard {
                        let mut temporary = BTreeSet::new();
                        self.visit_expression(guard, &mut arm_state, &live, &mut temporary, true);
                    }
                    let flow = self.check_block(&arm.body, arm_state);
                    normals.extend(flow.normal);
                    breaks.extend(flow.breaks);
                    continues.extend(flow.continues);
                }
                Flow {
                    normal: BorrowState::join(normals),
                    breaks,
                    continues,
                }
            }
            TypedStatementKind::Block(body) => self.check_block(body, state),
            TypedStatementKind::Defer(body) => {
                let _ = self.check_block(body, state.clone());
                normal(state)
            }
            TypedStatementKind::Expect { selector, body, .. } => {
                self.visit_expression(selector, &mut state, &live, &mut ephemeral, true);
                let handler = self.check_block(body, state.clone());
                Flow {
                    normal: BorrowState::join(state.into_iter().chain(handler.normal)),
                    breaks: handler.breaks,
                    continues: handler.continues,
                }
            }
            TypedStatementKind::Break => Flow {
                breaks: vec![state],
                ..Flow::default()
            },
            TypedStatementKind::Continue => Flow {
                continues: vec![state],
                ..Flow::default()
            },
            TypedStatementKind::Pass => normal(state),
        }
    }

    fn check_loop(
        &mut self,
        condition: Option<&TypedExpression>,
        binding: Option<LocalBindingId>,
        body: &[TypedStatement],
        mut entry: BorrowState,
        span: Span,
    ) -> Flow {
        let live = self.liveness.get(&span).cloned().unwrap_or_default();
        if let Some(condition) = condition {
            let mut ephemeral = BTreeSet::new();
            self.visit_expression(condition, &mut entry, &live, &mut ephemeral, true);
        }
        let mut head = entry.clone();
        let mut breaks = Vec::new();
        for _ in 0..=self.loan_ids.len().saturating_add(1) {
            let mut iteration = head.clone();
            if let Some(binding) = binding {
                iteration.values.insert(
                    OwnershipPlaceRoot::Local(binding),
                    ValueProvenance::default(),
                );
            }
            let flow = self.check_block(body, iteration);
            breaks.extend(flow.breaks.clone());
            let back = BorrowState::join(flow.normal.into_iter().chain(flow.continues.into_iter()));
            let Some(next) = BorrowState::join(entry.clone().into_iter().chain(back)) else {
                break;
            };
            if next == head {
                return Flow {
                    normal: BorrowState::join(head.into_iter().chain(breaks)),
                    ..Flow::default()
                };
            }
            head = next;
        }
        Flow {
            normal: BorrowState::join(head.into_iter().chain(breaks)),
            ..Flow::default()
        }
    }

    fn visit_expression(
        &mut self,
        expression: &TypedExpression,
        state: &mut BorrowState,
        live: &BTreeSet<LocalBindingId>,
        ephemeral: &mut BTreeSet<LoanId>,
        apply_self: bool,
    ) -> ValueProvenance {
        let mut result = match &expression.kind {
            TypedExpressionKind::Constant(_) | TypedExpressionKind::FunctionReference(_) => {
                ValueProvenance::default()
            }
            TypedExpressionKind::Local(binding) => state
                .values
                .get(&OwnershipPlaceRoot::Local(*binding))
                .cloned()
                .unwrap_or_default(),
            TypedExpressionKind::ClosureCapture(index) => state
                .values
                .get(&OwnershipPlaceRoot::ClosureCapture(*index))
                .cloned()
                .unwrap_or_default(),
            TypedExpressionKind::Closure { captures, .. }
            | TypedExpressionKind::Tuple(captures)
            | TypedExpressionKind::Array(captures)
            | TypedExpressionKind::VariadicSlice(captures) => {
                let mut value = ValueProvenance::default();
                for (index, capture) in captures.iter().enumerate() {
                    let child = self.visit_expression(capture, state, live, ephemeral, true);
                    value.union_with(&child.prefixed(OwnershipProjection::TupleField(index)));
                }
                value
            }
            TypedExpressionKind::Unary { operand, .. }
            | TypedExpressionKind::Cast { value: operand }
            | TypedExpressionKind::AddressOfTemporary(operand)
            | TypedExpressionKind::MakeTraitObject { value: operand, .. }
            | TypedExpressionKind::Propagate { operand, .. }
            | TypedExpressionKind::NumericConversion { value: operand, .. } => {
                self.visit_expression(operand, state, live, ephemeral, true)
            }
            TypedExpressionKind::Dereference(operand) => self
                .visit_expression(operand, state, live, ephemeral, false)
                .projected(OwnershipProjection::Dereference),
            TypedExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                let mut value = self.visit_expression(left, state, live, ephemeral, true);
                if matches!(
                    operator,
                    BinaryOperator::LogicalAnd | BinaryOperator::LogicalOr
                ) {
                    let mut right_state = state.clone();
                    let mut right_ephemeral = ephemeral.clone();
                    let right_value = self.visit_expression(
                        right,
                        &mut right_state,
                        live,
                        &mut right_ephemeral,
                        true,
                    );
                    if let Some(joined) = BorrowState::join([state.clone(), right_state]) {
                        *state = joined;
                    }
                    ephemeral.extend(right_ephemeral);
                    value.union_with(&right_value);
                } else {
                    let right_value = self.visit_expression(right, state, live, ephemeral, true);
                    value.union_with(&right_value);
                }
                value
            }
            TypedExpressionKind::Call {
                callee, arguments, ..
            } => {
                let callee_value = match callee {
                    TypedCallee::Indirect(value) | TypedCallee::Closure { value, .. } => {
                        self.visit_expression(value, state, live, ephemeral, true)
                    }
                    _ => ValueProvenance::default(),
                };
                let mut values = Vec::new();
                for argument in arguments {
                    let value = self.visit_expression(argument, state, live, ephemeral, true);
                    ephemeral.extend(value.all_origins().loans);
                    values.push(value);
                }
                self.call_result_provenance(callee, &values, callee_value)
            }
            TypedExpressionKind::Field { base, field } => self
                .visit_expression(base, state, live, ephemeral, false)
                .projected(OwnershipProjection::Field(*field)),
            TypedExpressionKind::TupleField { base, index } => self
                .visit_expression(base, state, live, ephemeral, false)
                .projected(OwnershipProjection::TupleField(*index)),
            TypedExpressionKind::Index { base, index } => {
                let base_value = self.visit_expression(base, state, live, ephemeral, false);
                self.visit_expression(index, state, live, ephemeral, true);
                base_value.projected(index_projection(index))
            }
            TypedExpressionKind::AddressOf(place) => {
                self.visit_place_inputs(place, state, live, ephemeral);
                ValueProvenance::default()
            }
            TypedExpressionKind::DefaultValue(_) => ValueProvenance::default(),
            TypedExpressionKind::NumericAlternative {
                receiver, operand, ..
            } => {
                let mut value = self.visit_expression(receiver, state, live, ephemeral, true);
                if let Some(operand) = operand {
                    value.union_with(&self.visit_expression(operand, state, live, ephemeral, true));
                }
                value
            }
            TypedExpressionKind::StandardCall {
                operation,
                arguments,
            } => {
                let mut value = ValueProvenance::default();
                for (index, argument) in arguments.iter().enumerate() {
                    let child = self.visit_expression(argument, state, live, ephemeral, true);
                    value.union_with(&child.prefixed(OwnershipProjection::TupleField(index)));
                }
                if standard_call_exclusively_accesses_receiver(*operation)
                    && let Some(receiver) = arguments.first()
                    && let Some(place) = receiver.ownership_place().map(normalized_place)
                {
                    let authorized = self.authorized_loans(&place, state);
                    self.check_access(
                        &place,
                        AccessKind::ReadWrite,
                        expression.span,
                        state,
                        live,
                        ephemeral,
                        &authorized,
                    );
                }
                if matches!(
                    operation,
                    StandardCall::SharedGet { .. }
                        | StandardCall::MutexLock { .. }
                        | StandardCall::MutexGuardGet { .. }
                        | StandardCall::StoreGet { mutable: false, .. }
                        | StandardCall::StoreGet { mutable: true, .. }
                ) && let Some(receiver) = arguments.first()
                    && let Some(place) = receiver.ownership_place().map(normalized_place)
                {
                    value = self.create_loan(
                        expression.span,
                        place,
                        if matches!(
                            operation,
                            StandardCall::StoreGet { mutable: true, .. }
                                | StandardCall::MutexGuardGet { mutable: true, .. }
                        ) {
                            LoanKind::Exclusive
                        } else {
                            LoanKind::Shared
                        },
                        false,
                        &value,
                        state,
                        live,
                        ephemeral,
                    );
                }
                value
            }
            TypedExpressionKind::CollectionLiteral {
                elements: arguments,
                ..
            } => {
                let mut value = ValueProvenance::default();
                for (index, argument) in arguments.iter().enumerate() {
                    let child = self.visit_expression(argument, state, live, ephemeral, true);
                    value.union_with(&child.prefixed(OwnershipProjection::TupleField(index)));
                }
                value
            }
            TypedExpressionKind::Struct { fields, .. }
            | TypedExpressionKind::Enum { fields, .. } => {
                let mut value = ValueProvenance::default();
                for (field, expression) in fields {
                    let child = self.visit_expression(expression, state, live, ephemeral, true);
                    value.union_with(&child.prefixed(OwnershipProjection::Field(*field)));
                }
                value
            }
            TypedExpressionKind::FormattedString(parts) => {
                for part in parts {
                    if let FormattedPart::Expression(value) = part {
                        self.visit_expression(value, state, live, ephemeral, true);
                    }
                }
                ValueProvenance::default()
            }
        };

        if !borrow_capability(self.resolved, self.typed, expression.ty) {
            // Provenance constrains only values whose canonical type can
            // contain a borrow. Computing an owned scalar through a reference
            // must not make that scalar retain the source loan.
            result = ValueProvenance::default();
        }

        if apply_self {
            let authorized = expression
                .ownership_place()
                .map(normalized_place)
                .map(|place| self.authorized_loans(&place, state))
                .unwrap_or_default();
            for operation in &expression.ownership {
                let Some(place) = operation.place.clone().map(normalized_place) else {
                    continue;
                };
                match operation.kind {
                    OwnershipUseKind::BorrowShared | OwnershipUseKind::ReborrowShared => {
                        let borrowed = self.create_loan(
                            expression.span,
                            place,
                            LoanKind::Shared,
                            operation.kind == OwnershipUseKind::ReborrowShared,
                            &result,
                            state,
                            live,
                            ephemeral,
                        );
                        result = borrowed;
                    }
                    OwnershipUseKind::BorrowExclusive | OwnershipUseKind::ReborrowExclusive => {
                        let borrowed = self.create_loan(
                            expression.span,
                            place,
                            LoanKind::Exclusive,
                            operation.kind == OwnershipUseKind::ReborrowExclusive,
                            &result,
                            state,
                            live,
                            ephemeral,
                        );
                        result = borrowed;
                    }
                    OwnershipUseKind::Move | OwnershipUseKind::Drop => {
                        if !matches!(expression.kind, TypedExpressionKind::AddressOf(_)) {
                            self.check_access(
                                &place,
                                AccessKind::Write,
                                expression.span,
                                state,
                                live,
                                ephemeral,
                                &authorized,
                            );
                            self.clear_moved_root(&place, state);
                        }
                    }
                    OwnershipUseKind::Produce
                    | OwnershipUseKind::Copy
                    | OwnershipUseKind::Clone
                    | OwnershipUseKind::LegacyCopy => self.check_access(
                        &place,
                        AccessKind::Read,
                        expression.span,
                        state,
                        live,
                        ephemeral,
                        &authorized,
                    ),
                }
            }
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn create_loan(
        &mut self,
        span: Span,
        place: OwnershipPlace,
        kind: LoanKind,
        reborrow: bool,
        operand_provenance: &ValueProvenance,
        state: &mut BorrowState,
        live: &BTreeSet<LocalBindingId>,
        ephemeral: &mut BTreeSet<LoanId>,
    ) -> ValueProvenance {
        let authorized = self.authorized_loans(&place, state);
        self.check_access(
            &place,
            if kind == LoanKind::Shared {
                AccessKind::BorrowShared
            } else {
                AccessKind::BorrowExclusive
            },
            span,
            state,
            live,
            ephemeral,
            &authorized,
        );
        let mut origins = self.origins_for_borrow(&place, operand_provenance, state, reborrow);
        let referents = authorized
            .iter()
            .filter_map(|id| state.loans.get(id))
            .flat_map(|loan| loan.places.iter().cloned())
            .collect::<BTreeSet<_>>();
        let places = if reborrow && !referents.is_empty() {
            referents
        } else {
            BTreeSet::from([place])
        };
        let Some(id) = self.loan_ids.get(&span).copied() else {
            return ValueProvenance::scalar(origins);
        };
        state.loans.insert(id, Loan { places, kind, span });
        origins.loans.insert(id);
        ephemeral.insert(id);
        ValueProvenance::scalar(origins)
    }

    fn origins_for_borrow(
        &self,
        place: &OwnershipPlace,
        operand: &ValueProvenance,
        state: &BorrowState,
        reborrow: bool,
    ) -> Origins {
        if !operand.paths.is_empty() {
            return operand.all_origins();
        }
        let mut origins = self
            .stored_provenance_at(place, state)
            .map_or_else(Origins::default, |value| value.all_origins());
        let borrows_through_existing_storage = reborrow
            || place
                .projections
                .contains(&OwnershipProjection::Dereference)
            || self
                .root_types
                .get(&place.root)
                .is_some_and(|ty| matches!(self.expanded_kind(*ty), TypeKind::Slice { .. }));
        if borrows_through_existing_storage {
            return origins;
        }
        match place.root {
            OwnershipPlaceRoot::Local(_) | OwnershipPlaceRoot::ClosureCapture(_) => {
                origins.locals.insert(place.root);
            }
            OwnershipPlaceRoot::Expression(_) => origins.temporary = true,
        }
        origins
    }

    fn stored_provenance_at(
        &self,
        place: &OwnershipPlace,
        state: &BorrowState,
    ) -> Option<ValueProvenance> {
        let mut selected = state.values.get(&place.root)?.clone();
        for projection in place
            .projections
            .iter()
            .take_while(|projection| **projection != OwnershipProjection::Dereference)
        {
            selected = selected.projected(*projection);
        }
        Some(selected)
    }

    #[allow(clippy::too_many_arguments)]
    fn check_access(
        &mut self,
        place: &OwnershipPlace,
        access: AccessKind,
        span: Span,
        state: &BorrowState,
        live: &BTreeSet<LocalBindingId>,
        ephemeral: &BTreeSet<LoanId>,
        authorized: &BTreeSet<LoanId>,
    ) {
        // Access through a reference is an access to the referent. Comparing
        // the synthetic `reference-local.*` place against other loans would
        // conservatively alias every dereference, losing field disjointness.
        let effective_places = authorized
            .iter()
            .filter_map(|id| state.loans.get(id))
            .flat_map(|loan| loan.places.iter())
            .collect::<Vec<_>>();
        for id in self.active_loans(state, live, ephemeral) {
            if authorized.contains(&id) {
                continue;
            }
            let Some(loan) = state.loans.get(&id) else {
                continue;
            };
            let disjoint = if effective_places.is_empty() {
                loan.places
                    .iter()
                    .all(|loan_place| loan_place.overlap(place) == PlaceOverlap::Disjoint)
            } else {
                loan.places.iter().all(|loan_place| {
                    effective_places.iter().all(|effective_place| {
                        loan_place.overlap(effective_place) == PlaceOverlap::Disjoint
                    })
                })
            };
            if disjoint {
                continue;
            }
            let conflicts = match access {
                AccessKind::Read => loan.kind == LoanKind::Exclusive,
                AccessKind::Write | AccessKind::ReadWrite | AccessKind::BorrowExclusive => true,
                AccessKind::BorrowShared => loan.kind == LoanKind::Exclusive,
            };
            if conflicts {
                self.report(
                    span,
                    "borrow-conflict",
                    Diagnostic::new(
                        Category::Borrow,
                        "this access conflicts with an overlapping live borrow",
                    )
                    .with_primary(span)
                    .with_related(
                        loan.span,
                        match loan.kind {
                            LoanKind::Shared => "shared borrow created here",
                            LoanKind::Exclusive => "exclusive borrow created here",
                        },
                    ),
                );
                break;
            }
        }
    }

    fn active_loans(
        &self,
        state: &BorrowState,
        live: &BTreeSet<LocalBindingId>,
        ephemeral: &BTreeSet<LoanId>,
    ) -> BTreeSet<LoanId> {
        let mut result = ephemeral.clone();
        for (root, provenance) in &state.values {
            let active = match root {
                OwnershipPlaceRoot::Local(binding) => live.contains(binding),
                OwnershipPlaceRoot::ClosureCapture(_) => true,
                OwnershipPlaceRoot::Expression(_) => false,
            };
            if active {
                result.extend(provenance.all_origins().loans);
            }
        }
        result
    }

    fn authorized_loans(&self, place: &OwnershipPlace, state: &BorrowState) -> BTreeSet<LoanId> {
        if !place
            .projections
            .contains(&OwnershipProjection::Dereference)
        {
            return BTreeSet::new();
        }
        self.stored_provenance_at(place, state)
            .as_ref()
            .map(|value| value.all_origins().loans)
            .unwrap_or_default()
    }

    fn call_result_provenance(
        &self,
        callee: &TypedCallee,
        arguments: &[ValueProvenance],
        indirect: ValueProvenance,
    ) -> ValueProvenance {
        if matches!(callee, TypedCallee::Closure { .. }) {
            let mut result = indirect;
            for argument in arguments {
                result.union_with(argument);
            }
            return result;
        }
        let declaration = match callee {
            TypedCallee::Function(instance) => Some(instance.declaration),
            _ => None,
        };
        let Some(declaration) = declaration else {
            let mut result = indirect;
            for argument in arguments {
                result.union_with(argument);
            }
            return result;
        };
        let Some(provenance) = self.typed.function_provenance.get(&declaration) else {
            return ValueProvenance::default();
        };
        let has_receiver = self
            .typed
            .function_signatures
            .get(&declaration)
            .is_some_and(|signature| signature.receiver.is_some());
        let index = match provenance.returned_from {
            ProvenanceSource::Static => return ValueProvenance::default(),
            ProvenanceSource::Receiver => 0,
            ProvenanceSource::Parameter(index) => index + usize::from(has_receiver),
            ProvenanceSource::ClosureCapture(_) => return ValueProvenance::default(),
            ProvenanceSource::AllBorrowingInputs => {
                let mut result = ValueProvenance::default();
                for argument in arguments {
                    result.union_with(argument);
                }
                return result;
            }
        };
        arguments.get(index).cloned().unwrap_or_default()
    }

    fn check_return_provenance(&mut self, span: Span, value: &ValueProvenance) {
        let origins = value.all_origins();
        if !origins.locals.is_empty() || origins.temporary {
            self.report(
                span,
                "borrow-escape",
                Diagnostic::new(
                    Category::Borrow,
                    "cannot return a borrow of local or temporary storage",
                )
                .with_primary(span),
            );
            return;
        }
        if let Some(expected) = self.current_return_source
            && expected != ProvenanceSource::Static
            && expected != ProvenanceSource::AllBorrowingInputs
            && (!origins.inputs.is_empty() && origins.inputs != BTreeSet::from([expected]))
        {
            self.report(
                span,
                "borrow-source",
                Diagnostic::new(
                    Category::Borrow,
                    "the returned borrow does not come from the signature's inferred input",
                )
                .with_primary(span),
            );
        }
    }

    fn assign_provenance(
        &self,
        place: &OwnershipPlace,
        provenance: ValueProvenance,
        state: &mut BorrowState,
    ) {
        if place.projections.is_empty() {
            state.values.insert(place.root, provenance);
            return;
        }
        let value = state.values.entry(place.root).or_default();
        value.paths.retain(|path, _| {
            !path.starts_with(&place.projections) && !place.projections.starts_with(path)
        });
        for (path, origins) in provenance.paths {
            let mut full = place.projections.clone();
            full.extend(path);
            value.paths.insert(full, origins);
        }
    }

    fn clear_moved_root(&self, place: &OwnershipPlace, state: &mut BorrowState) {
        if place.projections.is_empty() {
            state.values.remove(&place.root);
        }
    }

    fn visit_place_inputs(
        &mut self,
        place: &TypedPlace,
        state: &mut BorrowState,
        live: &BTreeSet<LocalBindingId>,
        ephemeral: &mut BTreeSet<LoanId>,
    ) {
        match place {
            TypedPlace::Local { .. } | TypedPlace::ClosureCapture { .. } => {}
            TypedPlace::Field { base, .. } | TypedPlace::TupleField { base, .. } => {
                self.visit_place_inputs(base, state, live, ephemeral);
            }
            TypedPlace::Index { base, index, .. } => {
                self.visit_place_inputs(base, state, live, ephemeral);
                self.visit_expression(index, state, live, ephemeral, true);
            }
            TypedPlace::Dereference { base, .. } => {
                self.visit_expression(base, state, live, ephemeral, false);
            }
        }
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

    fn report(&mut self, span: Span, key: &'static str, diagnostic: Diagnostic) {
        if self.reported.insert((span, key)) {
            self.diagnostics.push(diagnostic);
        }
    }
}

fn standard_call_exclusively_accesses_receiver(operation: StandardCall) -> bool {
    matches!(
        operation,
        StandardCall::VecGetVar { .. }
            | StandardCall::MutexGuardGet { mutable: true, .. }
            | StandardCall::VecAppend { .. }
            | StandardCall::VecInsert { .. }
            | StandardCall::VecRemove { .. }
            | StandardCall::VecPop { .. }
            | StandardCall::VecClear { .. }
            | StandardCall::MapGetVar { .. }
            | StandardCall::MapInsert { .. }
            | StandardCall::MapRemove { .. }
            | StandardCall::MapClear { .. }
            | StandardCall::SetInsert { .. }
            | StandardCall::SetRemove { .. }
            | StandardCall::SetClear { .. }
            | StandardCall::StoreInsert { .. }
            | StandardCall::StoreGet { mutable: true, .. }
            | StandardCall::StoreRemove { .. }
            | StandardCall::StoreCompact { .. }
            | StandardCall::StoreClear { .. }
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessKind {
    Read,
    Write,
    ReadWrite,
    BorrowShared,
    BorrowExclusive,
}

fn normal(state: BorrowState) -> Flow {
    Flow {
        normal: Some(state),
        ..Flow::default()
    }
}

trait IntoStateIterator {
    type Iterator: Iterator<Item = BorrowState>;
    fn into_iter(self) -> Self::Iterator;
}

impl IntoStateIterator for BorrowState {
    type Iterator = std::iter::Once<BorrowState>;

    fn into_iter(self) -> Self::Iterator {
        std::iter::once(self)
    }
}

fn normalized_place(mut place: OwnershipPlace) -> OwnershipPlace {
    place
        .projections
        .retain(|projection| *projection != OwnershipProjection::ReceiverAdaptation);
    place
}

fn index_projection(index: &TypedExpression) -> OwnershipProjection {
    match &index.kind {
        TypedExpressionKind::Constant(crate::ir::Constant::Integer {
            magnitude,
            negative: false,
        }) => OwnershipProjection::ConstantIndex(*magnitude),
        _ => OwnershipProjection::DynamicIndex,
    }
}

fn collect_borrow_spans(statements: &[TypedStatement], spans: &mut BTreeSet<Span>) {
    walk_statements(statements, &mut |expression| {
        if expression.ownership.iter().any(|operation| {
            matches!(
                operation.kind,
                OwnershipUseKind::BorrowShared
                    | OwnershipUseKind::BorrowExclusive
                    | OwnershipUseKind::ReborrowShared
                    | OwnershipUseKind::ReborrowExclusive
            )
        }) || matches!(
            expression.kind,
            TypedExpressionKind::StandardCall {
                operation: StandardCall::SharedGet { .. } | StandardCall::StoreGet { .. },
                ..
            }
        ) {
            spans.insert(expression.span);
        }
    });
}

fn liveness_block(
    statements: &[TypedStatement],
    live: &mut BTreeSet<LocalBindingId>,
    facts: &mut BTreeMap<Span, BTreeSet<LocalBindingId>>,
) {
    // Deferred bodies execute at scope exit, so their captured locals remain
    // live across statements following each registration.
    for statement in statements {
        if let TypedStatementKind::Defer(body) = &statement.kind {
            walk_statements(body, &mut |expression| {
                if let TypedExpressionKind::Local(binding) = expression.kind {
                    live.insert(binding);
                }
            });
        }
    }
    for statement in statements.iter().rev() {
        liveness_statement(statement, live, facts);
        facts.insert(statement.span, live.clone());
    }
}

fn liveness_statement(
    statement: &TypedStatement,
    live: &mut BTreeSet<LocalBindingId>,
    facts: &mut BTreeMap<Span, BTreeSet<LocalBindingId>>,
) {
    match &statement.kind {
        TypedStatementKind::Let { binding, value, .. } => {
            live.remove(binding);
            expression_uses(value, live);
        }
        TypedStatementKind::Destructure {
            bindings, value, ..
        } => {
            for binding in bindings {
                live.remove(&binding.binding);
            }
            expression_uses(value, live);
        }
        TypedStatementKind::Assign {
            place,
            operator,
            value,
        } => {
            if *operator == AssignmentOperator::Assign
                && let TypedPlace::Local { binding, .. } = place
            {
                live.remove(binding);
            } else {
                place_uses(place, live);
            }
            expression_uses(value, live);
        }
        TypedStatementKind::Expression(value) | TypedStatementKind::Return(Some(value)) => {
            expression_uses(value, live);
        }
        TypedStatementKind::Return(None)
        | TypedStatementKind::Break
        | TypedStatementKind::Continue
        | TypedStatementKind::Pass => {}
        TypedStatementKind::If {
            condition,
            then_body,
            else_body,
        } => {
            let mut left = live.clone();
            let mut right = live.clone();
            liveness_block(then_body, &mut left, facts);
            liveness_block(else_body, &mut right, facts);
            live.extend(left);
            live.extend(right);
            expression_uses(condition, live);
        }
        TypedStatementKind::While { condition, body } => loop {
            let before = live.clone();
            let mut body_live = live.clone();
            liveness_block(body, &mut body_live, facts);
            live.extend(body_live);
            expression_uses(condition, live);
            if *live == before {
                break;
            }
        },
        TypedStatementKind::For {
            binding,
            iterable,
            body,
            ..
        } => loop {
            let before = live.clone();
            let mut body_live = live.clone();
            body_live.remove(binding);
            liveness_block(body, &mut body_live, facts);
            live.extend(body_live);
            expression_uses(iterable, live);
            if *live == before {
                break;
            }
        },
        TypedStatementKind::Match { scrutinee, arms } => {
            for arm in arms {
                let mut arm_live = live.clone();
                liveness_block(&arm.body, &mut arm_live, facts);
                if let Some(guard) = &arm.guard {
                    expression_uses(guard, &mut arm_live);
                }
                live.extend(arm_live);
            }
            expression_uses(scrutinee, live);
        }
        TypedStatementKind::Block(body) | TypedStatementKind::Defer(body) => {
            liveness_block(body, live, facts);
        }
        TypedStatementKind::Expect { selector, body, .. } => {
            let mut body_live = live.clone();
            liveness_block(body, &mut body_live, facts);
            live.extend(body_live);
            expression_uses(selector, live);
        }
    }
}

fn expression_uses(expression: &TypedExpression, uses: &mut BTreeSet<LocalBindingId>) {
    walk_expression(expression, &mut |expression| {
        if let TypedExpressionKind::Local(binding) = expression.kind {
            uses.insert(binding);
        }
    });
}

fn place_uses(place: &TypedPlace, uses: &mut BTreeSet<LocalBindingId>) {
    match place {
        TypedPlace::Local { binding, .. } => {
            uses.insert(*binding);
        }
        TypedPlace::ClosureCapture { .. } => {}
        TypedPlace::Field { base, .. } | TypedPlace::TupleField { base, .. } => {
            place_uses(base, uses);
        }
        TypedPlace::Index { base, index, .. } => {
            place_uses(base, uses);
            expression_uses(index, uses);
        }
        TypedPlace::Dereference { base, .. } => expression_uses(base, uses),
    }
}

fn walk_statements(statements: &[TypedStatement], visit: &mut impl FnMut(&TypedExpression)) {
    for statement in statements {
        match &statement.kind {
            TypedStatementKind::Let { value, .. }
            | TypedStatementKind::Destructure { value, .. }
            | TypedStatementKind::Expression(value)
            | TypedStatementKind::Return(Some(value)) => walk_expression(value, visit),
            TypedStatementKind::Assign { place, value, .. } => {
                walk_place(place, visit);
                walk_expression(value, visit);
            }
            TypedStatementKind::Return(None)
            | TypedStatementKind::Break
            | TypedStatementKind::Continue
            | TypedStatementKind::Pass => {}
            TypedStatementKind::If {
                condition,
                then_body,
                else_body,
            } => {
                walk_expression(condition, visit);
                walk_statements(then_body, visit);
                walk_statements(else_body, visit);
            }
            TypedStatementKind::While { condition, body } => {
                walk_expression(condition, visit);
                walk_statements(body, visit);
            }
            TypedStatementKind::For { iterable, body, .. } => {
                walk_expression(iterable, visit);
                walk_statements(body, visit);
            }
            TypedStatementKind::Match { scrutinee, arms } => {
                walk_expression(scrutinee, visit);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        walk_expression(guard, visit);
                    }
                    walk_statements(&arm.body, visit);
                }
            }
            TypedStatementKind::Block(body) | TypedStatementKind::Defer(body) => {
                walk_statements(body, visit);
            }
            TypedStatementKind::Expect { selector, body, .. } => {
                walk_expression(selector, visit);
                walk_statements(body, visit);
            }
        }
    }
}

fn walk_expression(expression: &TypedExpression, visit: &mut impl FnMut(&TypedExpression)) {
    visit(expression);
    match &expression.kind {
        TypedExpressionKind::Constant(_)
        | TypedExpressionKind::FunctionReference(_)
        | TypedExpressionKind::Local(_)
        | TypedExpressionKind::ClosureCapture(_)
        | TypedExpressionKind::DefaultValue(_) => {}
        TypedExpressionKind::Closure { captures, .. }
        | TypedExpressionKind::Tuple(captures)
        | TypedExpressionKind::Array(captures)
        | TypedExpressionKind::VariadicSlice(captures)
        | TypedExpressionKind::CollectionLiteral {
            elements: captures, ..
        } => {
            for capture in captures {
                walk_expression(capture, visit);
            }
        }
        TypedExpressionKind::Unary { operand, .. }
        | TypedExpressionKind::Cast { value: operand }
        | TypedExpressionKind::Dereference(operand)
        | TypedExpressionKind::AddressOfTemporary(operand)
        | TypedExpressionKind::MakeTraitObject { value: operand, .. }
        | TypedExpressionKind::Propagate { operand, .. }
        | TypedExpressionKind::NumericConversion { value: operand, .. } => {
            walk_expression(operand, visit);
        }
        TypedExpressionKind::Binary { left, right, .. } => {
            walk_expression(left, visit);
            walk_expression(right, visit);
        }
        TypedExpressionKind::Call {
            callee, arguments, ..
        } => {
            match callee {
                TypedCallee::Indirect(value) | TypedCallee::Closure { value, .. } => {
                    walk_expression(value, visit);
                }
                _ => {}
            }
            for argument in arguments {
                walk_expression(argument, visit);
            }
        }
        TypedExpressionKind::Field { base, .. } | TypedExpressionKind::TupleField { base, .. } => {
            walk_expression(base, visit)
        }
        TypedExpressionKind::Index { base, index } => {
            walk_expression(base, visit);
            walk_expression(index, visit);
        }
        TypedExpressionKind::AddressOf(place) => walk_place(place, visit),
        TypedExpressionKind::NumericAlternative {
            receiver, operand, ..
        } => {
            walk_expression(receiver, visit);
            if let Some(operand) = operand {
                walk_expression(operand, visit);
            }
        }
        TypedExpressionKind::StandardCall { arguments, .. } => {
            for argument in arguments {
                walk_expression(argument, visit);
            }
        }
        TypedExpressionKind::Struct { fields, .. } | TypedExpressionKind::Enum { fields, .. } => {
            for (_, value) in fields {
                walk_expression(value, visit);
            }
        }
        TypedExpressionKind::FormattedString(parts) => {
            for part in parts {
                if let FormattedPart::Expression(value) = part {
                    walk_expression(value, visit);
                }
            }
        }
    }
}

fn walk_place(place: &TypedPlace, visit: &mut impl FnMut(&TypedExpression)) {
    match place {
        TypedPlace::Local { .. } | TypedPlace::ClosureCapture { .. } => {}
        TypedPlace::Field { base, .. } | TypedPlace::TupleField { base, .. } => {
            walk_place(base, visit);
        }
        TypedPlace::Index { base, index, .. } => {
            walk_place(base, visit);
            walk_expression(index, visit);
        }
        TypedPlace::Dereference { base, .. } => walk_expression(base, visit),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn structural_paths_survive_wrapping_and_projection(
            path in proptest::collection::vec(0usize..8, 0..16),
            loan in any::<u16>(),
        ) {
            let mut origins = Origins::default();
            origins.loans.insert(LoanId(u32::from(loan)));
            let scalar = ValueProvenance::scalar(origins);
            let mut wrapped = scalar.clone();
            for index in path.iter().rev() {
                wrapped = wrapped.prefixed(OwnershipProjection::TupleField(*index));
            }
            for index in &path {
                wrapped = wrapped.projected(OwnershipProjection::TupleField(*index));
            }
            prop_assert_eq!(wrapped, scalar);
        }

        #[test]
        fn provenance_union_is_commutative(left in any::<u16>(), right in any::<u16>()) {
            let value = |loan| {
                let mut origins = Origins::default();
                origins.loans.insert(LoanId(u32::from(loan)));
                ValueProvenance::scalar(origins)
            };
            let mut left_then_right = value(left);
            left_then_right.union_with(&value(right));
            let mut right_then_left = value(right);
            right_then_left.union_with(&value(left));
            prop_assert_eq!(left_then_right, right_then_left);
        }
    }
}
