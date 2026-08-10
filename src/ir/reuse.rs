//! Conservative temporary and return storage reuse.
//!
//! Semantic copy records remain in IR, but a proven-dead source can supply the
//! destination storage directly. Uncertain cases retain materialized copying.

use super::*;

pub(super) fn apply_temporary_and_return_reuse(program: &mut TypedIrProgram) {
    for function in &mut program.functions {
        let has_defer = statements_contain_defer(&function.body);
        let parameters = function
            .parameters
            .iter()
            .map(|parameter| parameter.binding)
            .collect::<BTreeSet<_>>();
        optimize_statements(
            &mut function.body,
            &function.promoted_locals,
            &parameters,
            !has_defer,
        );
    }
}

fn statements_contain_defer(statements: &[TypedStatement]) -> bool {
    statements.iter().any(|statement| match &statement.kind {
        TypedStatementKind::Defer(_) => true,
        TypedStatementKind::If {
            then_body,
            else_body,
            ..
        } => statements_contain_defer(then_body) || statements_contain_defer(else_body),
        TypedStatementKind::While { body, .. }
        | TypedStatementKind::For { body, .. }
        | TypedStatementKind::Block(body)
        | TypedStatementKind::Expect { body, .. } => statements_contain_defer(body),
        TypedStatementKind::Match { arms, .. } => {
            arms.iter().any(|arm| statements_contain_defer(&arm.body))
        }
        TypedStatementKind::Let { .. }
        | TypedStatementKind::Destructure { .. }
        | TypedStatementKind::Assign { .. }
        | TypedStatementKind::Expression(_)
        | TypedStatementKind::Return(_)
        | TypedStatementKind::Break
        | TypedStatementKind::Continue
        | TypedStatementKind::Pass => false,
    })
}

fn optimize_statements(
    statements: &mut [TypedStatement],
    promoted: &BTreeSet<LocalBindingId>,
    parameters: &BTreeSet<LocalBindingId>,
    allow_local_return: bool,
) {
    for statement in statements {
        match &mut statement.kind {
            TypedStatementKind::Let { value, .. }
            | TypedStatementKind::Destructure { value, .. }
            | TypedStatementKind::Expression(value) => {
                optimize_expression(value, promoted, parameters, false);
            }
            TypedStatementKind::Assign { place, value, .. } => {
                optimize_place(place, promoted, parameters);
                optimize_expression(value, promoted, parameters, false);
            }
            TypedStatementKind::Return(Some(value)) => {
                optimize_expression(value, promoted, parameters, allow_local_return);
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
                optimize_expression(condition, promoted, parameters, false);
                optimize_statements(then_body, promoted, parameters, allow_local_return);
                optimize_statements(else_body, promoted, parameters, allow_local_return);
            }
            TypedStatementKind::While { condition, body } => {
                optimize_expression(condition, promoted, parameters, false);
                optimize_statements(body, promoted, parameters, allow_local_return);
            }
            TypedStatementKind::For { iterable, body, .. } => {
                optimize_expression(iterable, promoted, parameters, false);
                optimize_statements(body, promoted, parameters, allow_local_return);
            }
            TypedStatementKind::Match { scrutinee, arms } => {
                optimize_expression(scrutinee, promoted, parameters, false);
                for arm in arms {
                    if let Some(guard) = &mut arm.guard {
                        optimize_expression(guard, promoted, parameters, false);
                    }
                    optimize_statements(&mut arm.body, promoted, parameters, allow_local_return);
                }
            }
            TypedStatementKind::Block(body) | TypedStatementKind::Defer(body) => {
                optimize_statements(body, promoted, parameters, allow_local_return);
            }
            TypedStatementKind::Expect { selector, body, .. } => {
                optimize_expression(selector, promoted, parameters, false);
                optimize_statements(body, promoted, parameters, allow_local_return);
            }
        }
    }
}

fn optimize_expression(
    expression: &mut TypedExpression,
    promoted: &BTreeSet<LocalBindingId>,
    parameters: &BTreeSet<LocalBindingId>,
    allow_local_return: bool,
) {
    match &mut expression.kind {
        TypedExpressionKind::Constant(_)
        | TypedExpressionKind::FunctionReference(_)
        | TypedExpressionKind::Local(_)
        | TypedExpressionKind::ClosureCapture(_)
        | TypedExpressionKind::DefaultValue(_) => {}
        TypedExpressionKind::Closure { captures, .. }
        | TypedExpressionKind::CollectionLiteral {
            elements: captures, ..
        }
        | TypedExpressionKind::Tuple(captures)
        | TypedExpressionKind::Array(captures)
        | TypedExpressionKind::VariadicSlice(captures) => {
            for value in captures {
                optimize_expression(value, promoted, parameters, false);
            }
        }
        TypedExpressionKind::Unary { operand, .. }
        | TypedExpressionKind::Cast { value: operand }
        | TypedExpressionKind::Dereference(operand)
        | TypedExpressionKind::AddressOfTemporary(operand)
        | TypedExpressionKind::Propagate {
            operand,
            result_declaration: _,
            ok_variant: _,
            ok_field: _,
            err_variant: _,
            err_field: _,
            error_type: _,
        } => optimize_expression(operand, promoted, parameters, false),
        TypedExpressionKind::Binary { left, right, .. } => {
            optimize_expression(left, promoted, parameters, false);
            optimize_expression(right, promoted, parameters, false);
        }
        TypedExpressionKind::Call {
            callee, arguments, ..
        } => {
            match callee {
                TypedCallee::Indirect(value) | TypedCallee::Closure { value, .. } => {
                    optimize_expression(value, promoted, parameters, false);
                }
                TypedCallee::Function(_)
                | TypedCallee::Dynamic { .. }
                | TypedCallee::Print { .. } => {}
            }
            for argument in arguments {
                optimize_expression(argument, promoted, parameters, false);
            }
        }
        TypedExpressionKind::Field { base, .. } | TypedExpressionKind::TupleField { base, .. } => {
            optimize_expression(base, promoted, parameters, false);
        }
        TypedExpressionKind::Index { base, index } => {
            optimize_expression(base, promoted, parameters, false);
            optimize_expression(index, promoted, parameters, false);
        }
        TypedExpressionKind::AddressOf(place) => optimize_place(place, promoted, parameters),
        TypedExpressionKind::NumericConversion { value, .. } => {
            optimize_expression(value, promoted, parameters, false);
        }
        TypedExpressionKind::NumericAlternative {
            receiver, operand, ..
        } => {
            optimize_expression(receiver, promoted, parameters, false);
            if let Some(operand) = operand {
                optimize_expression(operand, promoted, parameters, false);
            }
        }
        TypedExpressionKind::StandardCall { arguments, .. } => {
            for argument in arguments {
                optimize_expression(argument, promoted, parameters, false);
            }
        }
        TypedExpressionKind::MakeTraitObject { value, .. } => {
            optimize_expression(value, promoted, parameters, false);
        }
        TypedExpressionKind::Struct { fields, .. } | TypedExpressionKind::Enum { fields, .. } => {
            for (_, value) in fields {
                optimize_expression(value, promoted, parameters, false);
            }
        }
        TypedExpressionKind::FormattedString(parts) => {
            for part in parts {
                if let FormattedPart::Expression(value) = part {
                    optimize_expression(value, promoted, parameters, false);
                }
            }
        }
    }

    let Some(facts) = &mut expression.copy else {
        return;
    };
    let fresh_temporary = facts.context.source_lifetime == LogicalCopyLifetime::Temporary;
    let dead_return_local = allow_local_return
        && facts.context.kind == LogicalCopyKind::Return
        && facts.context.source_lifetime == LogicalCopyLifetime::LexicalScope
        && matches!(expression.kind, TypedExpressionKind::Local(binding)
            if !promoted.contains(&binding) && !parameters.contains(&binding));
    if fresh_temporary || dead_return_local {
        facts.mode = LogicalCopyMode::ReuseSource;
    }
    let updated = *facts;
    if let Some(operation) = expression
        .ownership
        .iter_mut()
        .find(|operation| operation.kind == OwnershipUseKind::LegacyCopy)
    {
        operation.legacy_copy = Some(updated);
    }
}

fn optimize_place(
    place: &mut TypedPlace,
    promoted: &BTreeSet<LocalBindingId>,
    parameters: &BTreeSet<LocalBindingId>,
) {
    match place {
        TypedPlace::Local { .. } | TypedPlace::ClosureCapture { .. } => {}
        TypedPlace::Field { base, .. } | TypedPlace::TupleField { base, .. } => {
            optimize_place(base, promoted, parameters);
        }
        TypedPlace::Index { base, index, .. } => {
            optimize_place(base, promoted, parameters);
            optimize_expression(index, promoted, parameters, false);
        }
        TypedPlace::Dereference { base, .. } => {
            optimize_expression(base, promoted, parameters, false);
        }
    }
}
