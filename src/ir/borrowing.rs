//! Conservative compiler-only read-only parameter borrowing.
//!
//! The pass specializes only concrete direct-call ABIs. Source-level value
//! types and copies remain unchanged everywhere the proof does not apply.

use super::*;

pub(super) fn apply_read_only_call_borrowing(
    program: &mut TypedIrProgram,
    resolved: &ResolvedProgram,
    types: &TypeContext,
) {
    let mut abi_exposed = BTreeSet::new();
    for vtable in &program.vtables {
        for method in &vtable.methods {
            match method {
                VtableMethod::Function(instance) | VtableMethod::Closure(instance) => {
                    abi_exposed.insert(instance.clone());
                }
                VtableMethod::FunctionReference => {}
            }
        }
    }
    for function in &program.functions {
        collect_function_references(&function.body, &mut abi_exposed);
        if function.closure.is_some()
            || resolved.declarations[function.declaration.index()]
                .foreign_binding
                .is_some()
        {
            abi_exposed.insert(function.instance.clone());
        }
    }

    let plans = program
        .functions
        .iter()
        .map(|function| {
            let borrowed = if abi_exposed.contains(&function.instance) {
                BTreeSet::new()
            } else {
                function
                    .parameters
                    .iter()
                    .enumerate()
                    .filter_map(|(index, parameter)| {
                        (copy_is_costly(types, parameter.ty)
                            && !function.promoted_locals.contains(&parameter.binding)
                            && statements_are_read_only(&function.body, parameter.binding))
                        .then_some(index)
                    })
                    .collect()
            };
            (function.instance.clone(), borrowed)
        })
        .collect::<BTreeMap<_, _>>();

    for function in &mut program.functions {
        let borrowed = &plans[&function.instance];
        for (index, parameter) in function.parameters.iter_mut().enumerate() {
            parameter.passing = if borrowed.contains(&index) {
                ValuePassingMode::ReadOnlyBorrowed
            } else {
                ValuePassingMode::Owned
            };
        }
        optimize_statements(&mut function.body, &plans);
    }
}

fn copy_is_costly(types: &TypeContext, ty: TypeId) -> bool {
    matches!(
        logical_copy_strategy(types, ty),
        LogicalCopyStrategy::Recursive | LogicalCopyStrategy::RuntimeManaged
    )
}

fn statements_are_read_only(statements: &[TypedStatement], binding: LocalBindingId) -> bool {
    statements
        .iter()
        .all(|statement| statement_is_read_only(statement, binding))
}

fn statement_is_read_only(statement: &TypedStatement, binding: LocalBindingId) -> bool {
    match &statement.kind {
        TypedStatementKind::Let { value, .. }
        | TypedStatementKind::Expression(value)
        | TypedStatementKind::Return(Some(value)) => expression_is_read_only(value, binding),
        TypedStatementKind::Destructure { value, .. } => expression_is_read_only(value, binding),
        TypedStatementKind::Assign { place, value, .. } => {
            !place_roots_at(place, binding)
                && place_is_read_only(place, binding)
                && expression_is_read_only(value, binding)
        }
        TypedStatementKind::Return(None)
        | TypedStatementKind::Break
        | TypedStatementKind::Continue
        | TypedStatementKind::Pass => true,
        TypedStatementKind::If {
            condition,
            then_body,
            else_body,
        } => {
            expression_is_read_only(condition, binding)
                && statements_are_read_only(then_body, binding)
                && statements_are_read_only(else_body, binding)
        }
        TypedStatementKind::While { condition, body } => {
            expression_is_read_only(condition, binding) && statements_are_read_only(body, binding)
        }
        TypedStatementKind::For { iterable, body, .. } => {
            expression_is_read_only(iterable, binding) && statements_are_read_only(body, binding)
        }
        TypedStatementKind::Match { scrutinee, arms } => {
            expression_is_read_only(scrutinee, binding)
                && arms.iter().all(|arm| {
                    arm.guard
                        .as_ref()
                        .is_none_or(|guard| expression_is_read_only(guard, binding))
                        && statements_are_read_only(&arm.body, binding)
                })
        }
        TypedStatementKind::Block(body) | TypedStatementKind::Defer(body) => {
            statements_are_read_only(body, binding)
        }
        TypedStatementKind::Expect { selector, body, .. } => {
            expression_is_read_only(selector, binding) && statements_are_read_only(body, binding)
        }
    }
}

fn expression_is_read_only(expression: &TypedExpression, binding: LocalBindingId) -> bool {
    match &expression.kind {
        TypedExpressionKind::Constant(_)
        | TypedExpressionKind::FunctionReference(_)
        | TypedExpressionKind::Local(_)
        | TypedExpressionKind::ClosureCapture(_)
        | TypedExpressionKind::DefaultValue(_) => true,
        TypedExpressionKind::Closure { captures, .. }
        | TypedExpressionKind::CollectionLiteral {
            elements: captures, ..
        }
        | TypedExpressionKind::Tuple(captures)
        | TypedExpressionKind::Array(captures)
        | TypedExpressionKind::VariadicSlice(captures) => captures
            .iter()
            .all(|capture| expression_is_read_only(capture, binding)),
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
        } => expression_is_read_only(operand, binding),
        TypedExpressionKind::Binary { left, right, .. } => {
            expression_is_read_only(left, binding) && expression_is_read_only(right, binding)
        }
        TypedExpressionKind::Call {
            callee, arguments, ..
        } => {
            let callee_is_read_only = match callee {
                TypedCallee::Indirect(value) => expression_is_read_only(value, binding),
                TypedCallee::Closure { value, .. } => expression_is_read_only(value, binding),
                TypedCallee::Function(_)
                | TypedCallee::Dynamic { .. }
                | TypedCallee::Print { .. } => true,
            };
            callee_is_read_only
                && arguments
                    .iter()
                    .all(|argument| expression_is_read_only(argument, binding))
        }
        TypedExpressionKind::Field { base, .. } | TypedExpressionKind::TupleField { base, .. } => {
            expression_is_read_only(base, binding)
        }
        TypedExpressionKind::Index { base, index } => {
            expression_is_read_only(base, binding) && expression_is_read_only(index, binding)
        }
        TypedExpressionKind::AddressOf(place) => {
            !place_roots_at(place, binding) && place_is_read_only(place, binding)
        }
        TypedExpressionKind::NumericConversion { value, .. } => {
            expression_is_read_only(value, binding)
        }
        TypedExpressionKind::NumericAlternative {
            receiver, operand, ..
        } => {
            expression_is_read_only(receiver, binding)
                && operand
                    .as_ref()
                    .is_none_or(|operand| expression_is_read_only(operand, binding))
        }
        TypedExpressionKind::StandardCall {
            operation,
            arguments,
        } => {
            let mutates_parameter = standard_call_mutates_receiver(*operation)
                && arguments
                    .first()
                    .is_some_and(|receiver| expression_aliases(receiver, binding));
            !mutates_parameter
                && arguments
                    .iter()
                    .all(|argument| expression_is_read_only(argument, binding))
        }
        TypedExpressionKind::MakeTraitObject { value, .. } => {
            expression_is_read_only(value, binding)
        }
        TypedExpressionKind::Struct { fields, .. } | TypedExpressionKind::Enum { fields, .. } => {
            fields
                .iter()
                .all(|(_, value)| expression_is_read_only(value, binding))
        }
        TypedExpressionKind::FormattedString(parts) => parts.iter().all(|part| match part {
            FormattedPart::Text(_) => true,
            FormattedPart::Expression(value) => expression_is_read_only(value, binding),
        }),
    }
}

fn standard_call_mutates_receiver(operation: StandardCall) -> bool {
    matches!(
        operation,
        StandardCall::VecAppend { .. }
            | StandardCall::VecInsert { .. }
            | StandardCall::VecRemove { .. }
            | StandardCall::VecClear { .. }
            | StandardCall::MapInsert { .. }
            | StandardCall::MapRemove { .. }
            | StandardCall::MapClear { .. }
            | StandardCall::SetInsert { .. }
            | StandardCall::SetRemove { .. }
            | StandardCall::SetClear { .. }
    )
}

fn expression_aliases(expression: &TypedExpression, binding: LocalBindingId) -> bool {
    if expression.copy.is_some() {
        return false;
    }
    match &expression.kind {
        TypedExpressionKind::Local(candidate) => *candidate == binding,
        TypedExpressionKind::Field { base, .. }
        | TypedExpressionKind::TupleField { base, .. }
        | TypedExpressionKind::Cast { value: base } => expression_aliases(base, binding),
        TypedExpressionKind::Index { base, .. } => expression_aliases(base, binding),
        _ => false,
    }
}

fn place_roots_at(place: &TypedPlace, binding: LocalBindingId) -> bool {
    match place {
        TypedPlace::Local {
            binding: candidate, ..
        } => *candidate == binding,
        TypedPlace::Field { base, .. }
        | TypedPlace::TupleField { base, .. }
        | TypedPlace::Index { base, .. } => place_roots_at(base, binding),
        TypedPlace::ClosureCapture { .. } | TypedPlace::Dereference { .. } => false,
    }
}

fn place_is_read_only(place: &TypedPlace, binding: LocalBindingId) -> bool {
    match place {
        TypedPlace::Local { .. } | TypedPlace::ClosureCapture { .. } => true,
        TypedPlace::Field { base, .. } | TypedPlace::TupleField { base, .. } => {
            place_is_read_only(base, binding)
        }
        TypedPlace::Index { base, index, .. } => {
            place_is_read_only(base, binding) && expression_is_read_only(index, binding)
        }
        TypedPlace::Dereference { base, .. } => expression_is_read_only(base, binding),
    }
}

fn optimize_statements(
    statements: &mut [TypedStatement],
    plans: &BTreeMap<FunctionInstance, BTreeSet<usize>>,
) {
    for statement in statements {
        match &mut statement.kind {
            TypedStatementKind::Let { value, .. }
            | TypedStatementKind::Destructure { value, .. }
            | TypedStatementKind::Expression(value)
            | TypedStatementKind::Return(Some(value)) => optimize_expression(value, plans),
            TypedStatementKind::Assign { place, value, .. } => {
                optimize_place(place, plans);
                optimize_expression(value, plans);
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
                optimize_expression(condition, plans);
                optimize_statements(then_body, plans);
                optimize_statements(else_body, plans);
            }
            TypedStatementKind::While { condition, body } => {
                optimize_expression(condition, plans);
                optimize_statements(body, plans);
            }
            TypedStatementKind::For { iterable, body, .. } => {
                optimize_expression(iterable, plans);
                optimize_statements(body, plans);
            }
            TypedStatementKind::Match { scrutinee, arms } => {
                optimize_expression(scrutinee, plans);
                for arm in arms {
                    if let Some(guard) = &mut arm.guard {
                        optimize_expression(guard, plans);
                    }
                    optimize_statements(&mut arm.body, plans);
                }
            }
            TypedStatementKind::Block(body) | TypedStatementKind::Defer(body) => {
                optimize_statements(body, plans);
            }
            TypedStatementKind::Expect { selector, body, .. } => {
                optimize_expression(selector, plans);
                optimize_statements(body, plans);
            }
        }
    }
}

fn optimize_expression(
    expression: &mut TypedExpression,
    plans: &BTreeMap<FunctionInstance, BTreeSet<usize>>,
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
            for capture in captures {
                optimize_expression(capture, plans);
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
        } => optimize_expression(operand, plans),
        TypedExpressionKind::Binary { left, right, .. } => {
            optimize_expression(left, plans);
            optimize_expression(right, plans);
        }
        TypedExpressionKind::Call {
            callee,
            arguments,
            argument_modes,
        } => {
            match callee {
                TypedCallee::Indirect(value) => optimize_expression(value, plans),
                TypedCallee::Closure { value, .. } => optimize_expression(value, plans),
                TypedCallee::Function(_)
                | TypedCallee::Dynamic { .. }
                | TypedCallee::Print { .. } => {}
            }
            for argument in arguments.iter_mut() {
                optimize_expression(argument, plans);
            }
            argument_modes.clear();
            argument_modes.resize(arguments.len(), ValuePassingMode::Owned);
            let TypedCallee::Function(instance) = callee else {
                return;
            };
            let Some(borrowed) = plans.get(instance) else {
                return;
            };
            for index in borrowed {
                let Some(argument) = arguments.get_mut(*index) else {
                    continue;
                };
                let eligible_copy = argument.copy.is_some_and(|copy| {
                    copy.context.purpose == LogicalCopyPurpose::Ordinary
                        && matches!(
                            copy.context.kind,
                            LogicalCopyKind::Argument | LogicalCopyKind::Receiver
                        )
                });
                if eligible_copy {
                    argument.copy = None;
                    argument_modes[*index] = ValuePassingMode::ReadOnlyBorrowed;
                }
            }
        }
        TypedExpressionKind::Field { base, .. } | TypedExpressionKind::TupleField { base, .. } => {
            optimize_expression(base, plans)
        }
        TypedExpressionKind::Index { base, index } => {
            optimize_expression(base, plans);
            optimize_expression(index, plans);
        }
        TypedExpressionKind::AddressOf(place) => optimize_place(place, plans),
        TypedExpressionKind::NumericConversion { value, .. } => {
            optimize_expression(value, plans);
        }
        TypedExpressionKind::NumericAlternative {
            receiver, operand, ..
        } => {
            optimize_expression(receiver, plans);
            if let Some(operand) = operand {
                optimize_expression(operand, plans);
            }
        }
        TypedExpressionKind::StandardCall { arguments, .. } => {
            for argument in arguments {
                optimize_expression(argument, plans);
            }
        }
        TypedExpressionKind::MakeTraitObject { value, .. } => optimize_expression(value, plans),
        TypedExpressionKind::Struct { fields, .. } | TypedExpressionKind::Enum { fields, .. } => {
            for (_, value) in fields {
                optimize_expression(value, plans);
            }
        }
        TypedExpressionKind::FormattedString(parts) => {
            for part in parts {
                if let FormattedPart::Expression(value) = part {
                    optimize_expression(value, plans);
                }
            }
        }
    }
}

fn optimize_place(place: &mut TypedPlace, plans: &BTreeMap<FunctionInstance, BTreeSet<usize>>) {
    match place {
        TypedPlace::Local { .. } | TypedPlace::ClosureCapture { .. } => {}
        TypedPlace::Field { base, .. } | TypedPlace::TupleField { base, .. } => {
            optimize_place(base, plans);
        }
        TypedPlace::Index { base, index, .. } => {
            optimize_place(base, plans);
            optimize_expression(index, plans);
        }
        TypedPlace::Dereference { base, .. } => optimize_expression(base, plans),
    }
}

fn collect_function_references(
    statements: &[TypedStatement],
    references: &mut BTreeSet<FunctionInstance>,
) {
    for statement in statements {
        match &statement.kind {
            TypedStatementKind::Let { value, .. }
            | TypedStatementKind::Destructure { value, .. }
            | TypedStatementKind::Expression(value)
            | TypedStatementKind::Return(Some(value)) => {
                collect_expression_function_references(value, references);
            }
            TypedStatementKind::Assign { place, value, .. } => {
                collect_place_function_references(place, references);
                collect_expression_function_references(value, references);
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
                collect_expression_function_references(condition, references);
                collect_function_references(then_body, references);
                collect_function_references(else_body, references);
            }
            TypedStatementKind::While { condition, body } => {
                collect_expression_function_references(condition, references);
                collect_function_references(body, references);
            }
            TypedStatementKind::For { iterable, body, .. } => {
                collect_expression_function_references(iterable, references);
                collect_function_references(body, references);
            }
            TypedStatementKind::Match { scrutinee, arms } => {
                collect_expression_function_references(scrutinee, references);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        collect_expression_function_references(guard, references);
                    }
                    collect_function_references(&arm.body, references);
                }
            }
            TypedStatementKind::Block(body) | TypedStatementKind::Defer(body) => {
                collect_function_references(body, references);
            }
            TypedStatementKind::Expect { selector, body, .. } => {
                collect_expression_function_references(selector, references);
                collect_function_references(body, references);
            }
        }
    }
}

fn collect_expression_function_references(
    expression: &TypedExpression,
    references: &mut BTreeSet<FunctionInstance>,
) {
    if let TypedExpressionKind::FunctionReference(instance) = &expression.kind {
        references.insert(instance.clone());
    }
    match &expression.kind {
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
            for capture in captures {
                collect_expression_function_references(capture, references);
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
        } => collect_expression_function_references(operand, references),
        TypedExpressionKind::Binary { left, right, .. } => {
            collect_expression_function_references(left, references);
            collect_expression_function_references(right, references);
        }
        TypedExpressionKind::Call {
            callee, arguments, ..
        } => {
            match callee {
                TypedCallee::Indirect(value) => {
                    collect_expression_function_references(value, references);
                }
                TypedCallee::Closure { value, .. } => {
                    collect_expression_function_references(value, references);
                }
                TypedCallee::Function(_)
                | TypedCallee::Dynamic { .. }
                | TypedCallee::Print { .. } => {}
            }
            for argument in arguments {
                collect_expression_function_references(argument, references);
            }
        }
        TypedExpressionKind::Field { base, .. } | TypedExpressionKind::TupleField { base, .. } => {
            collect_expression_function_references(base, references);
        }
        TypedExpressionKind::Index { base, index } => {
            collect_expression_function_references(base, references);
            collect_expression_function_references(index, references);
        }
        TypedExpressionKind::AddressOf(place) => {
            collect_place_function_references(place, references);
        }
        TypedExpressionKind::NumericConversion { value, .. } => {
            collect_expression_function_references(value, references);
        }
        TypedExpressionKind::NumericAlternative {
            receiver, operand, ..
        } => {
            collect_expression_function_references(receiver, references);
            if let Some(operand) = operand {
                collect_expression_function_references(operand, references);
            }
        }
        TypedExpressionKind::StandardCall { arguments, .. } => {
            for argument in arguments {
                collect_expression_function_references(argument, references);
            }
        }
        TypedExpressionKind::MakeTraitObject { value, .. } => {
            collect_expression_function_references(value, references);
        }
        TypedExpressionKind::Struct { fields, .. } | TypedExpressionKind::Enum { fields, .. } => {
            for (_, value) in fields {
                collect_expression_function_references(value, references);
            }
        }
        TypedExpressionKind::FormattedString(parts) => {
            for part in parts {
                if let FormattedPart::Expression(value) = part {
                    collect_expression_function_references(value, references);
                }
            }
        }
    }
}

fn collect_place_function_references(
    place: &TypedPlace,
    references: &mut BTreeSet<FunctionInstance>,
) {
    match place {
        TypedPlace::Local { .. } | TypedPlace::ClosureCapture { .. } => {}
        TypedPlace::Field { base, .. } | TypedPlace::TupleField { base, .. } => {
            collect_place_function_references(base, references);
        }
        TypedPlace::Index { base, index, .. } => {
            collect_place_function_references(base, references);
            collect_expression_function_references(index, references);
        }
        TypedPlace::Dereference { base, .. } => {
            collect_expression_function_references(base, references);
        }
    }
}
