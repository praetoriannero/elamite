//! Function and explicit-control-flow emission.

use super::*;

impl<'a> CEmitter<'a> {
    pub(super) fn emit_prototypes(&mut self) {
        for function in &self.program.functions {
            let Some(return_type) =
                self.c_function_return_type(function.return_type, Some(function.span))
            else {
                continue;
            };
            let symbol = self.function_symbol(&function.instance);
            let parameters = self.parameter_list(function);
            let _ = writeln!(self.output, "{return_type} {symbol}({parameters});");
        }
        self.output.push('\n');
    }

    pub(super) fn emit_function(&mut self, function: &ControlFlowFunction) {
        self.promoted = function.promoted_locals.clone();
        self.borrowed_parameters = function
            .parameters
            .iter()
            .filter(|parameter| parameter.passing == ValuePassingMode::ReadOnlyBorrowed)
            .map(|parameter| parameter.binding)
            .collect();
        let capture_bindings = function
            .closure
            .as_ref()
            .map(|closure| closure.captures.keys().copied().collect::<BTreeSet<_>>())
            .unwrap_or_default();
        self.promoted
            .retain(|binding| !capture_bindings.contains(binding));
        let Some(return_type) =
            self.c_function_return_type(function.return_type, Some(function.span))
        else {
            self.promoted.clear();
            return;
        };
        let symbol = self.function_symbol(&function.instance);
        let parameters = self.parameter_list(function);
        let location = self.location(function.span);
        let _ = writeln!(
            self.output,
            "/* {}:{}:{} — {} */",
            c_comment(&location.path),
            location.line,
            location.column,
            c_comment(&function.name)
        );
        let _ = writeln!(self.output, "{return_type} {symbol}({parameters}) {{");
        if function.closure.is_some() {
            self.output.push_str("    (void)el_env;\n");
        }
        let parameter_bindings = function
            .parameters
            .iter()
            .map(|parameter| parameter.binding)
            .collect::<BTreeSet<_>>();
        for (binding, ty) in &function.local_types {
            if parameter_bindings.contains(binding)
                || capture_bindings.contains(binding)
                || self.promoted.contains(binding)
            {
                continue;
            }
            if let Some(c_type) = self.c_type(*ty, Some(function.span)) {
                let _ = writeln!(
                    self.output,
                    "    {c_type} {} = {};",
                    local_name(*binding),
                    zero_value(*ty, &self.typed.types)
                );
            }
        }
        for parameter in &function.parameters {
            let _ = writeln!(self.output, "    (void){};", local_name(parameter.binding));
        }
        // A promoted local lives in a managed cell so a reference to it stays
        // valid after the frame ends. The cell pointer itself is an ordinary
        // stack variable, which Boehm's conservative stack scan treats as a
        // root for as long as the frame is live.
        let promoted = self.promoted.clone();
        for binding in &promoted {
            let Some(ty) = function.local_types.get(binding).copied() else {
                continue;
            };
            let Some(c_type) = self.c_type(ty, Some(function.span)) else {
                continue;
            };
            let cell = cell_name(*binding);
            let class = if self.scanned_allocation(ty) {
                AllocationClass::Scanned
            } else {
                AllocationClass::PointerFree
            };
            let _ = writeln!(self.output, "    {c_type} *{cell} = NULL;");
            let byte_count = format!("sizeof({c_type})");
            self.emit_managed_operation(ManagedMemoryOperation::Allocate {
                destination: &cell,
                byte_count: &byte_count,
                class,
            });
            let _ = writeln!(self.output, "    if ({cell} == NULL) el_out_of_memory();");
            let initial = if parameter_bindings.contains(binding) {
                local_name(*binding)
            } else {
                // A braced zero initializer is only valid in a declaration, so
                // an assignment into the cell needs a C99 compound literal.
                let zero = zero_value(ty, &self.typed.types);
                if zero.starts_with('{') {
                    format!("({c_type}){zero}")
                } else {
                    zero
                }
            };
            let _ = writeln!(self.output, "    *{cell} = {initial};");
        }
        for (index, ty) in function.temporary_types.iter().enumerate() {
            if matches!(
                self.typed
                    .types
                    .kind(self.typed.types.resolve_inference(*ty)),
                TypeKind::Never
            ) {
                continue;
            }
            if let Some(c_type) = self.c_type(*ty, Some(function.span)) {
                let _ = writeln!(
                    self.output,
                    "    {c_type} t{index} = {};\n    (void)t{index};",
                    zero_value(*ty, &self.typed.types)
                );
            }
        }
        let _ = writeln!(self.output, "    goto b{};", function.entry.index());
        // Only blocks reachable from the entry are emitted. Reachability must
        // be transitive: a block reached solely from an unreachable block
        // would otherwise emit a label with no `goto` naming it, which C
        // compilers reject under `-Werror=unused-label`. Lowering produces
        // unreachable blocks routinely — a `match` whose every arm returns
        // leaves its join block with no live predecessor.
        let reachable_blocks = reachable_blocks(function);
        for block in &function.blocks {
            if !reachable_blocks.contains(&block.id) {
                continue;
            }
            let _ = writeln!(self.output, "b{}:", block.id.index());
            for instruction in &block.instructions {
                self.emit_instruction(function, instruction);
            }
            self.emit_terminator(function, &block.terminator);
        }
        self.output.push_str("}\n\n");
        self.promoted.clear();
        self.borrowed_parameters.clear();
    }

    pub(super) fn parameter_list(&mut self, function: &ControlFlowFunction) -> String {
        let mut parameters = Vec::new();
        if let Some(closure) = &function.closure
            && let Some(ty) = self.c_type(closure.ty, Some(function.span))
        {
            parameters.push(format!("{ty} el_env"));
        }
        parameters.extend(function.parameters.iter().filter_map(|parameter| {
            self.c_type(parameter.ty, Some(parameter.span)).map(|ty| {
                if parameter.passing == ValuePassingMode::ReadOnlyBorrowed {
                    format!("const {ty} *{}", local_name(parameter.binding))
                } else {
                    format!("{ty} {}", local_name(parameter.binding))
                }
            })
        }));
        if parameters.is_empty() {
            "void".to_string()
        } else {
            parameters.join(", ")
        }
    }

    pub(super) fn emit_instruction(
        &mut self,
        function: &ControlFlowFunction,
        instruction: &Instruction,
    ) {
        match instruction {
            Instruction::CheckPointer {
                pointer,
                pointee,
                span,
            } => {
                let arguments = self.trap_arguments(*span);
                let _ = writeln!(
                    self.output,
                    "    {}((const void *){}, {arguments});",
                    pointer_check_name(self.resolve_alias(*pointee)),
                    temporary_name(*pointer)
                );
            }
            Instruction::CheckFunctionPointer { pointer, span } => {
                let arguments = self.trap_arguments(*span);
                let _ = writeln!(
                    self.output,
                    "    if ({} == NULL) el_trap(\"E-RUN-NULL\", {arguments});",
                    temporary_name(*pointer)
                );
            }
            Instruction::Assign {
                destination,
                value,
                span,
            } => {
                self.emit_place_checks(value_place(value), *span);
                if let Some(expression) = self.rvalue(function, *destination, value, *span) {
                    let destination_name = temporary_name(*destination);
                    let _ = writeln!(
                        self.output,
                        "    {destination_name} = {expression};\n    (void){destination_name};"
                    );
                    if self.program.requires_managed_memory
                        && let Rvalue::Call {
                            instance,
                            arguments,
                            ..
                        } = value
                        && self.resolved.declarations[instance.declaration.index()].kind
                            == crate::resolution::DeclarationKind::ForeignFunction
                    {
                        for argument in arguments {
                            if matches!(
                                self.typed.types.kind(
                                    self.typed.types.resolve_inference(
                                        function.temporary_types[argument.index()]
                                    )
                                ),
                                TypeKind::RawPointer { .. }
                            ) {
                                self.emit_managed_operation(ManagedMemoryOperation::KeepAlive {
                                    expression: &temporary_name(*argument),
                                });
                            }
                        }
                    }
                }
            }
            Instruction::Store { place, value, span } => {
                self.emit_place_checks(Some(place), *span);
                let expression = self.place_expression(place);
                let _ = writeln!(
                    self.output,
                    "    {expression} = {};\n    (void){expression};",
                    temporary_name(*value),
                );
            }
            Instruction::PrintValue {
                value,
                ty,
                span,
                newline,
            } => {
                self.emit_print(*value, *ty, *span, *newline);
            }
            Instruction::CompleteExpectation { .. } => {
                self.output.push_str("    el_expect_complete();\n");
            }
            Instruction::WaitExpectation { span } => {
                let _ = writeln!(
                    self.output,
                    "    el_expect_wait({});",
                    self.trap_arguments(*span)
                );
            }
        }
    }

    pub(super) fn rvalue(
        &mut self,
        function: &ControlFlowFunction,
        destination: TemporaryId,
        value: &Rvalue,
        span: Span,
    ) -> Option<String> {
        let destination_type = function.temporary_types[destination.index()];
        Some(match value {
            Rvalue::Constant(constant) => {
                self.constant_expression(constant, destination_type, span)?
            }
            Rvalue::FunctionReference(instance) => self.function_symbol(instance),
            Rvalue::Closure { ty, captures } => {
                let destination_name = temporary_name(destination);
                let c_type = self.c_type(*ty, Some(span))?;
                let class = if captures.iter().any(|capture| {
                    let capture_type = function.temporary_types[capture.index()];
                    self.scanned_allocation(capture_type)
                }) {
                    AllocationClass::Scanned
                } else {
                    AllocationClass::PointerFree
                };
                self.emit_managed_operation(ManagedMemoryOperation::Allocate {
                    destination: &destination_name,
                    byte_count: &format!("sizeof(*{destination_name})"),
                    class,
                });
                let _ = writeln!(
                    self.output,
                    "    if ({destination_name} == NULL) el_out_of_memory();"
                );
                for (index, capture) in captures.iter().enumerate() {
                    let _ = writeln!(
                        self.output,
                        "    {destination_name}->v{index} = {};",
                        temporary_name(*capture)
                    );
                }
                let _ = c_type;
                destination_name
            }
            Rvalue::Load(place) => self.place_expression(place),
            Rvalue::AddressOf(place) => format!("&{}", self.place_expression(place)),
            Rvalue::DefaultValue(ty) => self.default_expression(*ty, span)?,
            Rvalue::NumericConversion {
                outcome,
                value,
                source_type,
                ..
            } => format!(
                "{}({})",
                numeric_conversion_name(*outcome, *source_type, destination_type),
                temporary_name(*value)
            ),
            Rvalue::NumericAlternative {
                operation,
                receiver,
                operand,
                operand_type,
            } => {
                let mut call = format!(
                    "{}({}",
                    numeric_alternative_name(*operation, *operand_type, destination_type),
                    temporary_name(*receiver)
                );
                if let Some(operand) = operand {
                    let _ = write!(call, ", {}", temporary_name(*operand));
                }
                // Only the wrapping division and remainder helpers can trap,
                // and only on a zero divisor, so only they need a location.
                if numeric_alternative_traps(*operation) {
                    let _ = write!(call, ", {}", self.trap_arguments(span));
                }
                call.push(')');
                call
            }
            Rvalue::StandardCall {
                operation,
                arguments,
            } => {
                if matches!(operation, StandardCall::SliceLen { .. }) {
                    return Some(format!("{}.length", temporary_name(*arguments.first()?)));
                }
                if matches!(operation, StandardCall::Assert) {
                    let condition = arguments
                        .first()
                        .map_or_else(|| "false".to_string(), |value| temporary_name(*value));
                    return Some(format!(
                        "el_assert({condition}, {})",
                        self.trap_arguments(span)
                    ));
                }
                if matches!(operation, StandardCall::ForeignRootRetain { .. }) {
                    let value = arguments
                        .first()
                        .map_or_else(|| "NULL".to_string(), |value| temporary_name(*value));
                    return Some(format!("el_foreign_root_retain((void *){value})"));
                }
                if matches!(operation, StandardCall::ForeignRootPointer { .. }) {
                    let receiver = arguments
                        .first()
                        .map_or_else(|| "NULL".to_string(), |value| temporary_name(*value));
                    return Some(format!(
                        "({})el_foreign_root_pointer({receiver}, {})",
                        self.c_type(destination_type, Some(span))?,
                        self.trap_arguments(span)
                    ));
                }
                if matches!(operation, StandardCall::ForeignRootClose { .. }) {
                    let receiver = arguments
                        .first()
                        .map_or_else(|| "NULL".to_string(), |value| temporary_name(*value));
                    return Some(format!("el_foreign_root_close({receiver})"));
                }
                if matches!(operation, StandardCall::FormatterWrite { .. }) {
                    let receiver = arguments
                        .first()
                        .map_or_else(|| "NULL".to_string(), |value| temporary_name(*value));
                    let text = arguments
                        .get(1)
                        .map_or_else(|| "\"\"".to_string(), |value| temporary_name(*value));
                    return Some(format!(
                        "(el_fmt_append_n(*{receiver}, ({text}).bytes, ({text}).length), \
                         (el_unit){{0}})"
                    ));
                }
                if matches!(operation, StandardCall::IdentityFrom { .. }) {
                    let c_type = self.c_type(destination_type, Some(span))?;
                    let value = arguments
                        .first()
                        .map_or_else(|| "NULL".to_string(), |value| temporary_name(*value));
                    return Some(format!("({c_type}){{ .target = (void *){value} }}"));
                }
                if matches!(operation, StandardCall::ThreadJoin { .. }) {
                    let receiver = arguments
                        .first()
                        .map_or_else(|| "NULL".to_string(), |value| temporary_name(*value));
                    return Some(format!(
                        "{}({receiver}, {})",
                        standard_call_name(*operation),
                        self.trap_arguments(span)
                    ));
                }
                let mut arguments = arguments
                    .iter()
                    .map(|argument| temporary_name(*argument))
                    .collect::<Vec<_>>();
                if matches!(
                    operation,
                    StandardCall::VecInsert { .. } | StandardCall::VecRemove { .. }
                ) {
                    arguments.push(self.trap_arguments(span));
                }
                format!(
                    "{}({})",
                    standard_call_name(*operation),
                    arguments.join(", ")
                )
            }
            Rvalue::CollectionLiteral { kind, elements } => {
                let name = match kind {
                    CollectionLiteralKind::Vec => "vec_literal",
                    CollectionLiteralKind::Map => "map_literal",
                    CollectionLiteralKind::Set => "set_literal",
                };
                format!(
                    "el_{name}_t{}_n{}({})",
                    destination_type.index(),
                    elements.len(),
                    elements
                        .iter()
                        .map(|element| temporary_name(*element))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            Rvalue::CollectionLength { collection, kind } => match kind {
                IterationKind::Slice { .. } => {
                    format!("{}.length", temporary_name(*collection))
                }
                IterationKind::Array { length, .. } => format!("(uintptr_t){length}U"),
                IterationKind::Vec { .. }
                | IterationKind::Map { .. }
                | IterationKind::Set { .. } => {
                    format!("{}->length", temporary_name(*collection))
                }
            },
            Rvalue::IterationElement {
                collection,
                index,
                kind,
            } => {
                let collection_name = temporary_name(*collection);
                let index_name = temporary_name(*index);
                match kind {
                    IterationKind::Slice { .. } | IterationKind::Array { .. } => {
                        format!("{collection_name}.values[{index_name}]")
                    }
                    IterationKind::Vec { .. } | IterationKind::Set { .. } => {
                        format!("{collection_name}->values[{index_name}]")
                    }
                    IterationKind::Map { pair, .. } => {
                        let pair_type = self.c_type(*pair, Some(span))?;
                        format!(
                            "({pair_type}){{ .v0 = {collection_name}->keys[{index_name}], \
                             .v1 = {collection_name}->values[{index_name}] }}"
                        )
                    }
                }
            }
            Rvalue::FormattedString(parts) => {
                let formatter = format!("el_fmt_{}", destination.index());
                let _ = writeln!(
                    self.output,
                    "    el_formatter {formatter} = {{NULL, 0U, 0U}};"
                );
                for part in parts {
                    match part {
                        RuntimeFormattedPart::Text(text) => {
                            let _ = writeln!(
                                self.output,
                                "    el_fmt_append_n(&{formatter}, {}, (size_t){}U);",
                                c_string(text),
                                text.len()
                            );
                        }
                        RuntimeFormattedPart::Value { value, ty } => {
                            self.emit_formatter_value(
                                &formatter,
                                &temporary_name(*value),
                                *ty,
                                span,
                            );
                        }
                    }
                }
                format!("el_fmt_finish(&{formatter})")
            }
            Rvalue::MakeTraitObject {
                trait_declaration,
                trait_type,
                concrete,
                value,
            } => {
                let object = object_name(*trait_declaration, *trait_type);
                let table =
                    vtable_instance_name(self.typed, *trait_declaration, *trait_type, *concrete);
                format!(
                    "({object}){{ (void *){}, &{table} }}",
                    temporary_name(*value)
                )
            }
            Rvalue::BeginExpectation {
                selector,
                selector_type,
                trait_declaration,
            } => {
                let Some(code) = crate::traits::vtable_entry(
                    self.resolved,
                    self.typed,
                    *trait_declaration,
                    *selector_type,
                    "code",
                ) else {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::CodeGeneration,
                            "an expected trap has no `RuntimeTrap.code` implementation",
                        )
                        .with_primary(span),
                    );
                    return None;
                };
                let code = FunctionInstance {
                    declaration: code.declaration,
                    arguments: code.arguments,
                    self_type: code.self_type,
                };
                format!(
                    "el_expect_begin(UINT32_C({}), {}(&{}), {})",
                    selector_type.index(),
                    self.function_symbol(&code),
                    temporary_name(*selector),
                    self.trap_arguments(span)
                )
            }
            Rvalue::DynamicCall {
                receiver,
                trait_declaration,
                slot,
                arguments,
            } => {
                let receiver_name = temporary_name(*receiver);
                let mut call = format!(
                    "{receiver_name}.vtable->{}({receiver_name}.data",
                    vtable_slot_name(*slot)
                );
                for argument in arguments {
                    call.push_str(&format!(", {}", temporary_name(*argument)));
                }
                let _ = trait_declaration;
                call.push(')');
                self.call_rvalue(call, destination_type)
            }
            Rvalue::AllocateManaged { value, value_type } => {
                // A referenced composite literal needs its own managed cell.
                // Allocation is a statement, so it is emitted before the
                // assignment that consumes the resulting address.
                let cell_type = self.c_type(*value_type, Some(span))?;
                let destination_name = temporary_name(destination);
                let class = if self.scanned_allocation(*value_type) {
                    AllocationClass::Scanned
                } else {
                    AllocationClass::PointerFree
                };
                let byte_count = format!("sizeof({cell_type})");
                self.emit_managed_operation(ManagedMemoryOperation::Allocate {
                    destination: &destination_name,
                    byte_count: &byte_count,
                    class,
                });
                let _ = writeln!(
                    self.output,
                    "    if ({destination_name} == NULL) el_out_of_memory();"
                );
                let _ = writeln!(
                    self.output,
                    "    *{destination_name} = {};",
                    temporary_name(*value)
                );
                destination_name
            }
            Rvalue::Copy { source, .. } => format!(
                "{}({})",
                copy_helper_name(self.resolve_alias(destination_type)),
                temporary_name(*source)
            ),
            Rvalue::Discriminant(source) => format!("{}.tag", temporary_name(*source)),
            Rvalue::CompareEqual {
                left,
                right,
                operand_type,
            } => {
                let left = temporary_name(*left);
                let right = temporary_name(*right);
                self.component_equality(*operand_type, &left, &right)
            }
            Rvalue::CompareOrder {
                operator,
                left,
                right,
                operand_type,
            } => {
                let left = temporary_name(*left);
                let right = temporary_name(*right);
                let comparison = self.component_ordering(*operand_type, &left, &right);
                match operator {
                    BinaryOperator::Less => format!("(({comparison}) == -1)"),
                    BinaryOperator::LessEqual => {
                        format!("((({comparison}) == -1) || (({comparison}) == 0))")
                    }
                    BinaryOperator::Greater => format!("(({comparison}) == 1)"),
                    BinaryOperator::GreaterEqual => {
                        format!("((({comparison}) == 0) || (({comparison}) == 1))")
                    }
                    _ => "false".to_string(),
                }
            }
            Rvalue::Unary {
                operator, operand, ..
            } => self.unary_expression(*operator, *operand, destination_type, span)?,
            Rvalue::Binary {
                operator,
                left,
                right,
                ..
            } => self.binary_expression(*operator, *left, *right, destination_type, span)?,
            Rvalue::Cast {
                value, source_type, ..
            } => self.cast_expression(*value, *source_type, destination_type, span)?,
            Rvalue::Call {
                instance,
                arguments,
                argument_modes,
            } => {
                let call = format!(
                    "{}({})",
                    self.function_symbol(instance),
                    arguments
                        .iter()
                        .zip(argument_modes)
                        .map(|(argument, mode)| {
                            let argument = temporary_name(*argument);
                            if *mode == ValuePassingMode::ReadOnlyBorrowed {
                                format!("&{argument}")
                            } else {
                                argument
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                self.call_rvalue(call, destination_type)
            }
            Rvalue::IndirectCall { callee, arguments } => {
                let call = format!(
                    "{}({})",
                    temporary_name(*callee),
                    arguments
                        .iter()
                        .map(|argument| temporary_name(*argument))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                self.call_rvalue(call, destination_type)
            }
            Rvalue::ClosureCall {
                instance,
                closure,
                arguments,
            } => {
                let mut values = vec![temporary_name(*closure)];
                values.extend(arguments.iter().map(|argument| temporary_name(*argument)));
                let call = format!("{}({})", self.function_symbol(instance), values.join(", "));
                self.call_rvalue(call, destination_type)
            }
            Rvalue::VariadicSlice {
                elements,
                element_type: _,
            } => {
                let values = elements
                    .iter()
                    .map(|element| temporary_name(*element))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "{}({values})",
                    variadic_slice_name(destination_type, elements.len())
                )
            }
            Rvalue::Aggregate(aggregate) => {
                self.aggregate_expression(aggregate, destination_type, span)?
            }
        })
    }

    pub(super) fn constant_expression(
        &mut self,
        constant: &crate::ir::Constant,
        ty: TypeId,
        span: Span,
    ) -> Option<String> {
        let c_type = self.c_type(ty, Some(span))?;
        Some(match constant {
            crate::ir::Constant::Unit => "(el_unit){0}".to_string(),
            crate::ir::Constant::Bool(value) => if *value { "true" } else { "false" }.to_string(),
            crate::ir::Constant::Integer {
                magnitude,
                negative,
            } => {
                let primitive = self.typed.types.expanded_primitive(ty)?;
                if matches!(primitive, PrimitiveType::I128 | PrimitiveType::U128) {
                    self.type_error(
                        ty,
                        Some(span),
                        "128-bit integer constants are deferred past the Milestone 8 backend skeleton",
                    );
                    return None;
                }
                let magnitude = *magnitude;
                let sign = if *negative { "-" } else { "" };
                match primitive {
                    PrimitiveType::I64 if *negative && magnitude == (1_u128 << 63) => {
                        "INT64_MIN".to_string()
                    }
                    PrimitiveType::I64 => format!("({c_type})INT64_C({sign}{magnitude})"),
                    PrimitiveType::U64 => format!("({c_type})UINT64_C({magnitude})"),
                    PrimitiveType::Isize if self.options.target == Target::X86_64 => {
                        if *negative && magnitude == (1_u128 << 63) {
                            "INT64_MIN".to_string()
                        } else {
                            format!("({c_type})INT64_C({sign}{magnitude})")
                        }
                    }
                    PrimitiveType::Usize if self.options.target == Target::X86_64 => {
                        format!("({c_type})UINT64_C({magnitude})")
                    }
                    _ => format!("({c_type})({sign}{magnitude})"),
                }
            }
            crate::ir::Constant::Float(source) => {
                let number = strip_numeric_suffix(source).replace('_', "");
                format!("({c_type})({number})")
            }
            crate::ir::Constant::Character(value) => {
                format!("({c_type})UINT32_C({})", u32::from(*value))
            }
            crate::ir::Constant::String(value) => {
                let literal = format!(
                    "(el_str){{ {}, (size_t){}U }}",
                    c_string(value),
                    value.len()
                );
                if self.typed.types.expanded_primitive(ty) == Some(PrimitiveType::String) {
                    format!("el_string_from({literal})")
                } else {
                    literal
                }
            }
            crate::ir::Constant::Null => "NULL".to_string(),
        })
    }

    pub(super) fn unary_expression(
        &mut self,
        operator: UnaryOperator,
        operand: TemporaryId,
        ty: TypeId,
        span: Span,
    ) -> Option<String> {
        let operand = temporary_name(operand);
        match operator {
            UnaryOperator::Positive => Some(operand),
            UnaryOperator::LogicalNot => Some(format!("(!{operand})")),
            UnaryOperator::BitwiseNot => Some(format!("(~{operand})")),
            UnaryOperator::Negative => {
                if let Some(primitive) = self.typed.types.expanded_primitive(ty)
                    && primitive.is_integer()
                {
                    let helper = integer_helper_name(primitive)?;
                    let arguments = self.trap_arguments(span);
                    return Some(format!("el_neg_{helper}({operand}, {arguments})"));
                }
                Some(format!("(-{operand})"))
            }
        }
    }

    pub(super) fn binary_expression(
        &mut self,
        operator: BinaryOperator,
        left: TemporaryId,
        right: TemporaryId,
        ty: TypeId,
        span: Span,
    ) -> Option<String> {
        let left = temporary_name(left);
        let right = temporary_name(right);
        if operator == BinaryOperator::Concatenate {
            if let Some(primitive) = self.typed.types.expanded_primitive(ty) {
                return match primitive {
                    PrimitiveType::Str => Some(format!("el_concat_str({left}, {right})")),
                    PrimitiveType::String => Some(format!("el_concat_string({left}, {right})")),
                    _ => {
                        self.type_error(ty, Some(span), "unsupported concatenation operand type");
                        None
                    }
                };
            }
            let resolved = self.resolve_alias(ty);
            if matches!(
                self.typed.types.kind(resolved),
                TypeKind::Builtin { builtin, arguments }
                    if self.resolved.builtin_name(*builtin) == "Vec" && arguments.len() == 1
            ) {
                return Some(format!("{}({left}, {right})", concatenate_name(resolved)));
            }
            self.type_error(ty, Some(span), "unsupported concatenation operand type");
            return None;
        }
        if let Some(primitive) = self.typed.types.expanded_primitive(ty)
            && primitive.is_integer()
            && let Some(operation) = checked_binary_name(operator)
        {
            let helper = integer_helper_name(primitive).or_else(|| {
                self.type_error(
                    ty,
                    Some(span),
                    "128-bit arithmetic is deferred past the Milestone 8 backend skeleton",
                );
                None
            })?;
            let arguments = self.trap_arguments(span);
            return Some(format!(
                "el_{operation}_{helper}({left}, {right}, {arguments})"
            ));
        }
        let operator = c_binary_operator(operator)?;
        Some(format!("({left} {operator} {right})"))
    }

    pub(super) fn cast_expression(
        &mut self,
        value: TemporaryId,
        source: TypeId,
        target: TypeId,
        span: Span,
    ) -> Option<String> {
        let source_kind = self
            .typed
            .types
            .kind(self.typed.types.resolve_inference(source));
        let target_kind = self
            .typed
            .types
            .kind(self.typed.types.resolve_inference(target));
        if matches!(
            (source_kind, target_kind),
            (TypeKind::Reference { .. }, TypeKind::RawPointer { .. })
                | (TypeKind::RawPointer { .. }, TypeKind::Reference { .. })
                | (TypeKind::RawPointer { .. }, TypeKind::RawPointer { .. })
        ) {
            let target_type = self.c_type(target, Some(span))?;
            return Some(format!("({target_type}){}", temporary_name(value)));
        }
        let source_primitive = self.typed.types.expanded_primitive(source)?;
        let target_primitive = self.typed.types.expanded_primitive(target)?;
        let target_type = self.c_type(target, Some(span))?;
        let value = temporary_name(value);
        if target_primitive.is_integer() {
            let (minimum, maximum) = primitive_bounds(target_primitive, self.options.target)?;
            let location = self.trap_arguments(span);
            return Some(format!(
                "el_cast_integer((long double){value}, (long double)({minimum}), \
                 (long double)({maximum}), {location}, ({target_type}){value})"
            ));
        }
        if source_primitive.is_integer() || source_primitive.is_float() {
            return Some(format!("({target_type})({value})"));
        }
        None
    }

    pub(super) fn aggregate_expression(
        &mut self,
        aggregate: &AggregateValue,
        ty: TypeId,
        span: Span,
    ) -> Option<String> {
        let c_type = self.c_type(ty, Some(span))?;
        Some(match aggregate {
            AggregateValue::Tuple(values) => format!(
                "({c_type}){{ {} }}",
                values
                    .iter()
                    .enumerate()
                    .map(|(index, value)| format!(".v{index} = {}", temporary_name(*value)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            AggregateValue::Array(values) => format!(
                "({c_type}){{ .values = {{ {} }} }}",
                values
                    .iter()
                    .map(|value| temporary_name(*value))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            AggregateValue::Struct { fields, .. } => format!(
                "({c_type}){{ {} }}",
                fields
                    .iter()
                    .map(|(field, value)| {
                        format!(
                            ".{} = {}",
                            self.c_field_name(*field),
                            temporary_name(*value)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            AggregateValue::Enum {
                declaration: _,
                variant,
                fields,
            } => {
                let member = variant_member_name(*variant);
                if fields.is_empty() {
                    let payload = self
                        .enums
                        .get(&ty)
                        .and_then(|enumeration| {
                            enumeration
                                .variants
                                .iter()
                                .find(|variant| !variant.fields.is_empty())
                        })
                        .map_or_else(
                            || ".payload._empty = 0".to_string(),
                            |variant| {
                                format!(".payload.{} = {{0}}", variant_member_name(variant.id))
                            },
                        );
                    format!(
                        "({c_type}){{ .tag = UINT32_C({}), {payload} }}",
                        variant.index(),
                    )
                } else {
                    format!(
                        "({c_type}){{ .tag = UINT32_C({}), .payload.{member} = {{ {} }} }}",
                        variant.index(),
                        fields
                            .iter()
                            .map(|(field, value)| {
                                format!(".{} = {}", field_name(*field), temporary_name(*value))
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
        })
    }

    pub(super) fn emit_formatter_text(&mut self, formatter: &str, text: &str) {
        let _ = writeln!(
            self.output,
            "    el_fmt_append_n(&{formatter}, {}, (size_t){}U);",
            c_string(text),
            text.len()
        );
    }

    pub(super) fn emit_formatter_value(
        &mut self,
        formatter: &str,
        value: &str,
        ty: TypeId,
        span: Span,
    ) {
        let ty = self.resolve_alias(ty);
        if let Some(trait_declaration) = crate::traits::object_trait(self.resolved, self.typed, ty)
            && self
                .resolved
                .is_standard_declaration(trait_declaration, "Display")
        {
            let slot = crate::traits::vtable_slots(self.resolved, trait_declaration)
                .iter()
                .position(|(name, _)| name == "fmt")
                .unwrap_or(0);
            let _ = writeln!(
                self.output,
                "    {{ el_formatter *el_user_formatter = &{formatter}; \
                 (void){value}.vtable->{}({value}.data, &el_user_formatter); }}",
                vtable_slot_name(slot)
            );
            return;
        }
        match self.typed.types.kind(ty).clone() {
            TypeKind::Primitive(primitive) => match primitive {
                PrimitiveType::Unit => self.emit_formatter_text(formatter, "()"),
                PrimitiveType::Bool => {
                    let _ = writeln!(
                        self.output,
                        "    el_fmt_append(&{formatter}, {value} ? \"true\" : \"false\");"
                    );
                }
                PrimitiveType::Char => {
                    let _ = writeln!(self.output, "    el_fmt_char(&{formatter}, {value});");
                }
                PrimitiveType::I8
                | PrimitiveType::I16
                | PrimitiveType::I32
                | PrimitiveType::I64
                | PrimitiveType::Isize => {
                    let _ = writeln!(
                        self.output,
                        "    el_fmt_signed(&{formatter}, (intmax_t){value});"
                    );
                }
                PrimitiveType::U8
                | PrimitiveType::U16
                | PrimitiveType::U32
                | PrimitiveType::U64
                | PrimitiveType::Usize => {
                    let _ = writeln!(
                        self.output,
                        "    el_fmt_unsigned(&{formatter}, (uintmax_t){value});"
                    );
                }
                PrimitiveType::F32 | PrimitiveType::F64 => {
                    let _ = writeln!(
                        self.output,
                        "    el_fmt_float(&{formatter}, (double){value});"
                    );
                }
                PrimitiveType::Str | PrimitiveType::String => {
                    let _ = writeln!(
                        self.output,
                        "    el_fmt_append_n(&{formatter}, ({value}).bytes, ({value}).length);"
                    );
                }
                PrimitiveType::I128 | PrimitiveType::U128 => {
                    self.type_error(ty, Some(span), "128-bit values cannot yet be displayed");
                }
            },
            TypeKind::Reference { target, .. } => {
                self.emit_formatter_value(formatter, &format!("(*{value})"), target, span);
            }
            TypeKind::Tuple(elements) => {
                self.emit_formatter_text(formatter, "(");
                for (index, element) in elements.iter().enumerate() {
                    if index != 0 {
                        self.emit_formatter_text(formatter, ", ");
                    }
                    self.emit_formatter_value(
                        formatter,
                        &format!("{value}.v{index}"),
                        *element,
                        span,
                    );
                }
                if elements.len() == 1 {
                    self.emit_formatter_text(formatter, ",");
                }
                self.emit_formatter_text(formatter, ")");
            }
            TypeKind::Array { element, length } => {
                self.emit_formatter_text(formatter, "[");
                for index in 0..length {
                    if index != 0 {
                        self.emit_formatter_text(formatter, ", ");
                    }
                    self.emit_formatter_value(
                        formatter,
                        &format!("{value}.values[{index}U]"),
                        element,
                        span,
                    );
                }
                self.emit_formatter_text(formatter, "]");
            }
            TypeKind::Builtin { builtin, arguments } => {
                match (self.resolved.builtin_name(builtin), arguments.as_slice()) {
                    ("Vec", [element]) => {
                        let index = format!("el_i_t{}", ty.index());
                        self.emit_formatter_text(formatter, "[");
                        let _ = writeln!(
                            self.output,
                            "    for (uintptr_t {index} = 0U; {index} < {value}->length; ++{index}) {{"
                        );
                        let _ = writeln!(
                            self.output,
                            "        if ({index} != 0U) el_fmt_append(&{formatter}, \", \");"
                        );
                        self.emit_formatter_value(
                            formatter,
                            &format!("{value}->values[{index}]"),
                            *element,
                            span,
                        );
                        self.output.push_str("    }\n");
                        self.emit_formatter_text(formatter, "]");
                    }
                    ("Set", [element]) => {
                        let index = format!("el_i_t{}", ty.index());
                        self.emit_formatter_text(formatter, "{");
                        let _ = writeln!(
                            self.output,
                            "    for (uintptr_t {index} = 0U; {index} < {value}->length; ++{index}) {{"
                        );
                        let _ = writeln!(
                            self.output,
                            "        if ({index} != 0U) el_fmt_append(&{formatter}, \", \");"
                        );
                        self.emit_formatter_value(
                            formatter,
                            &format!("{value}->values[{index}]"),
                            *element,
                            span,
                        );
                        self.output.push_str("    }\n");
                        self.emit_formatter_text(formatter, "}");
                    }
                    ("Map", [key, item]) => {
                        let index = format!("el_i_t{}", ty.index());
                        self.emit_formatter_text(formatter, "{");
                        let _ = writeln!(
                            self.output,
                            "    for (uintptr_t {index} = 0U; {index} < {value}->length; ++{index}) {{"
                        );
                        let _ = writeln!(
                            self.output,
                            "        if ({index} != 0U) el_fmt_append(&{formatter}, \", \");"
                        );
                        self.emit_formatter_value(
                            formatter,
                            &format!("{value}->keys[{index}]"),
                            *key,
                            span,
                        );
                        self.emit_formatter_text(formatter, ": ");
                        self.emit_formatter_value(
                            formatter,
                            &format!("{value}->values[{index}]"),
                            *item,
                            span,
                        );
                        self.output.push_str("    }\n");
                        self.emit_formatter_text(formatter, "}");
                    }
                    _ => self.type_error(
                        ty,
                        Some(span),
                        "this builtin type has no `Display` implementation",
                    ),
                }
            }
            TypeKind::Nominal { .. } => {
                match crate::traits::select_trait_method(self.resolved, self.typed, ty, "fmt", None)
                    .ok()
                    .flatten()
                {
                    Some(selected) => {
                        let instance = FunctionInstance {
                            declaration: selected.declaration,
                            arguments: selected.arguments,
                            self_type: selected.self_type,
                        };
                        let symbol = self.function_symbol(&instance);
                        let _ = writeln!(
                            self.output,
                            "    {{ el_formatter *el_user_formatter = &{formatter}; \
                             (void){symbol}(&({value}), &el_user_formatter); }}"
                        );
                    }
                    None => self.type_error(
                        ty,
                        Some(span),
                        "this nominal type has no `Display.fmt` implementation",
                    ),
                }
            }
            _ => self.type_error(ty, Some(span), "this type cannot be displayed"),
        }
    }

    pub(super) fn emit_place_checks(&mut self, place: Option<&ControlFlowPlace>, span: Span) {
        let Some(place) = place else { return };
        match place {
            ControlFlowPlace::Local(_) | ControlFlowPlace::ClosureCapture(_) => {}
            ControlFlowPlace::Temporary(_) => {}
            ControlFlowPlace::Field { base, .. }
            | ControlFlowPlace::TupleField { base, .. }
            | ControlFlowPlace::VariantField { base, .. }
            | ControlFlowPlace::Dereference { base } => {
                self.emit_place_checks(Some(base), span);
            }
            ControlFlowPlace::Index {
                base,
                index,
                kind,
                trap,
            } => {
                self.emit_place_checks(Some(base), span);
                let arguments = self.trap_arguments(span);
                match kind {
                    IndexKind::Array { length } => {
                        let _ = writeln!(
                            self.output,
                            "    if ((uintmax_t){} >= UINTMAX_C({length})) \
                             el_trap(\"{}\", {arguments});",
                            temporary_name(*index),
                            trap.code()
                        );
                    }
                    IndexKind::Slice => {
                        let bound = format!("(uintmax_t){}.length", self.place_expression(base));
                        let _ = writeln!(
                            self.output,
                            "    if ((uintmax_t){} >= {bound}) el_trap(\"{}\", {arguments});",
                            temporary_name(*index),
                            trap.code()
                        );
                    }
                    IndexKind::Vec { .. } => {
                        let bound = format!("(uintmax_t){}->length", self.place_expression(base));
                        let _ = writeln!(
                            self.output,
                            "    if ((uintmax_t){} >= {bound}) el_trap(\"{}\", {arguments});",
                            temporary_name(*index),
                            trap.code()
                        );
                    }
                    IndexKind::Map { collection } => {
                        let find = format!("el_map_find_t{}", collection.index());
                        let _ = writeln!(
                            self.output,
                            "    if ({find}({}, {}) < 0) el_trap(\"{}\", {arguments});",
                            self.place_expression(base),
                            temporary_name(*index),
                            trap.code()
                        );
                    }
                }
            }
        }
    }

    pub(super) fn place_expression(&self, place: &ControlFlowPlace) -> String {
        match place {
            ControlFlowPlace::Local(binding) => {
                if self.promoted.contains(binding) {
                    format!("(*{})", cell_name(*binding))
                } else if self.borrowed_parameters.contains(binding) {
                    format!("(*{})", local_name(*binding))
                } else {
                    local_name(*binding)
                }
            }
            ControlFlowPlace::ClosureCapture(index) => format!("el_env->v{index}"),
            ControlFlowPlace::Temporary(temporary) => temporary_name(*temporary),
            ControlFlowPlace::Field { base, field } => {
                format!(
                    "{}.{}",
                    self.place_expression(base),
                    self.c_field_name(*field)
                )
            }
            ControlFlowPlace::TupleField { base, index } => {
                format!("{}.v{index}", self.place_expression(base))
            }
            ControlFlowPlace::VariantField {
                base,
                variant,
                field,
            } => format!(
                "{}.payload.{}.{}",
                self.place_expression(base),
                variant_member_name(*variant),
                field_name(*field)
            ),
            ControlFlowPlace::Dereference { base } => {
                format!("(*{})", self.place_expression(base))
            }
            ControlFlowPlace::Index {
                base, index, kind, ..
            } => match kind {
                IndexKind::Array { .. } | IndexKind::Slice => format!(
                    "{}.values[{}]",
                    self.place_expression(base),
                    temporary_name(*index)
                ),
                IndexKind::Vec { .. } => format!(
                    "{}->values[{}]",
                    self.place_expression(base),
                    temporary_name(*index)
                ),
                IndexKind::Map { collection } => format!(
                    "{}->values[(uintptr_t)el_map_find_t{}({}, {})]",
                    self.place_expression(base),
                    collection.index(),
                    self.place_expression(base),
                    temporary_name(*index)
                ),
            },
        }
    }

    pub(super) fn emit_print(&mut self, value: TemporaryId, ty: TypeId, span: Span, newline: bool) {
        let value_id = value;
        let value = temporary_name(value);
        let primitive = self.typed.types.expanded_primitive(ty);
        let rendered = if primitive.is_none() {
            let formatter = format!("el_print_fmt_{}", value_id.index());
            let _ = writeln!(
                self.output,
                "    el_formatter {formatter} = {{NULL, 0U, 0U}};"
            );
            self.emit_formatter_value(&formatter, &value, ty, span);
            let rendered = format!("el_print_text_{}", value_id.index());
            let _ = writeln!(
                self.output,
                "    el_str {rendered} = el_fmt_finish(&{formatter});"
            );
            Some(rendered)
        } else {
            None
        };
        if self.uses_synchronized_output() {
            self.output
                .push_str("    (void)pthread_mutex_lock(&el_stdout_lock);\n");
        }
        match primitive {
            Some(PrimitiveType::Unit) => {
                self.output.push_str("    fputs(\"()\", stdout);\n");
            }
            Some(PrimitiveType::Bool) => {
                let _ = writeln!(
                    self.output,
                    "    fputs({value} ? \"true\" : \"false\", stdout);"
                );
            }
            Some(PrimitiveType::Char) => {
                let _ = writeln!(self.output, "    el_print_char({value});");
            }
            Some(
                PrimitiveType::I8
                | PrimitiveType::I16
                | PrimitiveType::I32
                | PrimitiveType::I64
                | PrimitiveType::Isize,
            ) => {
                let _ = writeln!(
                    self.output,
                    "    fprintf(stdout, \"%\" PRIdMAX, (intmax_t){value});"
                );
            }
            Some(
                PrimitiveType::U8
                | PrimitiveType::U16
                | PrimitiveType::U32
                | PrimitiveType::U64
                | PrimitiveType::Usize,
            ) => {
                let _ = writeln!(
                    self.output,
                    "    fprintf(stdout, \"%\" PRIuMAX, (uintmax_t){value});"
                );
            }
            Some(PrimitiveType::F32) => {
                let _ = writeln!(
                    self.output,
                    "    fprintf(stdout, \"%.9g\", (double){value});"
                );
            }
            Some(PrimitiveType::F64) => {
                let _ = writeln!(self.output, "    fprintf(stdout, \"%.17g\", {value});");
            }
            Some(PrimitiveType::Str | PrimitiveType::String) => {
                let _ = writeln!(
                    self.output,
                    "    fwrite(({value}).bytes, 1U, ({value}).length, stdout);"
                );
            }
            _ => {
                let rendered = rendered.expect("nonprimitive print value was formatted");
                let _ = writeln!(
                    self.output,
                    "    fwrite({rendered}.bytes, 1U, {rendered}.length, stdout);"
                );
            }
        }
        if newline {
            self.output.push_str("    fputc('\\n', stdout);\n");
        }
        if self.uses_synchronized_output() {
            self.output
                .push_str("    (void)pthread_mutex_unlock(&el_stdout_lock);\n");
        }
    }

    pub(super) fn emit_terminator(
        &mut self,
        function: &ControlFlowFunction,
        terminator: &Terminator,
    ) {
        match terminator {
            Terminator::Goto(block) => {
                let _ = writeln!(self.output, "    goto b{};", block.index());
            }
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            } => {
                let _ = writeln!(
                    self.output,
                    "    if ({}) goto b{}; else goto b{};",
                    temporary_name(*condition),
                    then_block.index(),
                    else_block.index()
                );
            }
            Terminator::Return(Some(value)) => {
                if matches!(
                    self.typed.types.expanded_primitive(function.return_type),
                    Some(PrimitiveType::Unit)
                ) {
                    let _ = writeln!(
                        self.output,
                        "    (void){};\n    return;",
                        temporary_name(*value)
                    );
                } else {
                    let _ = writeln!(self.output, "    return {};", temporary_name(*value));
                }
            }
            Terminator::Return(None) => {
                if matches!(
                    self.typed.types.expanded_primitive(function.return_type),
                    Some(PrimitiveType::Unit)
                ) {
                    self.output.push_str("    return;\n");
                } else {
                    self.output.push_str("    abort();\n");
                }
            }
            Terminator::Trap { kind, span } => {
                let arguments = self.trap_arguments(*span);
                let _ = writeln!(
                    self.output,
                    "    el_trap(\"{}\", {arguments});",
                    kind.code()
                );
            }
            Terminator::NeverCall { call, span } => {
                let expression = match call {
                    NeverCall::Panic { message } => format!(
                        "el_panic({}, {})",
                        temporary_name(*message),
                        self.trap_arguments(*span)
                    ),
                    NeverCall::AssertionFail { value, value_type } => {
                        let formatter = format!("el_failure_formatter_{}", value.index());
                        let _ = writeln!(
                            self.output,
                            "    {{ el_formatter {formatter} = {{NULL, 0U, 0U}};"
                        );
                        self.emit_formatter_value(
                            &formatter,
                            &temporary_name(*value),
                            *value_type,
                            *span,
                        );
                        let _ = writeln!(
                            self.output,
                            "    el_assert_fail(el_fmt_finish(&{formatter}), {}); }}",
                            self.trap_arguments(*span)
                        );
                        "abort()".to_string()
                    }
                    NeverCall::TypedTrap {
                        reason,
                        reason_type,
                        trait_declaration,
                    } => {
                        let code = crate::traits::vtable_entry(
                            self.resolved,
                            self.typed,
                            *trait_declaration,
                            *reason_type,
                            "code",
                        );
                        let message = crate::traits::vtable_entry(
                            self.resolved,
                            self.typed,
                            *trait_declaration,
                            *reason_type,
                            "message",
                        );
                        let (Some(code), Some(message)) = (code, message) else {
                            self.diagnostics.push(
                                Diagnostic::new(
                                    Category::CodeGeneration,
                                    "a typed trap has no complete `RuntimeTrap` implementation",
                                )
                                .with_primary(*span),
                            );
                            return;
                        };
                        let suffix = reason.index();
                        let reason = temporary_name(*reason);
                        let code = FunctionInstance {
                            declaration: code.declaration,
                            arguments: code.arguments,
                            self_type: code.self_type,
                        };
                        let message = FunctionInstance {
                            declaration: message.declaration,
                            arguments: message.arguments,
                            self_type: message.self_type,
                        };
                        let _ = writeln!(
                            self.output,
                            "    {{ el_str el_trap_code_{suffix} = {}(&{reason});\n\
                             \x20\x20\x20\x20el_string el_trap_message_{suffix} = {}(&{reason});\n\
                             \x20\x20\x20\x20el_typed_trap(UINT32_C({}), el_trap_code_{suffix}, \
                             (el_str){{el_trap_message_{suffix}.bytes, \
                             el_trap_message_{suffix}.length}}, {}); }}",
                            self.function_symbol(&code),
                            self.function_symbol(&message),
                            reason_type.index(),
                            self.trap_arguments(*span),
                        );
                        "abort()".to_string()
                    }
                    NeverCall::Standard {
                        operation,
                        arguments,
                    } => {
                        let mut values = arguments
                            .iter()
                            .map(|argument| temporary_name(*argument))
                            .collect::<Vec<_>>();
                        if matches!(operation, StandardCall::ThreadJoin { .. }) {
                            values.push(self.trap_arguments(*span));
                        }
                        format!("{}({})", standard_call_name(*operation), values.join(", "))
                    }
                    NeverCall::Direct {
                        instance,
                        arguments,
                        argument_modes,
                    } => format!(
                        "{}({})",
                        self.function_symbol(instance),
                        arguments
                            .iter()
                            .zip(argument_modes)
                            .map(|(argument, mode)| {
                                let argument = temporary_name(*argument);
                                if *mode == ValuePassingMode::ReadOnlyBorrowed {
                                    format!("&{argument}")
                                } else {
                                    argument
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    NeverCall::Dynamic {
                        receiver,
                        trait_declaration,
                        slot,
                        arguments,
                    } => {
                        let receiver = temporary_name(*receiver);
                        let mut call = format!(
                            "{receiver}.vtable->{}({receiver}.data",
                            vtable_slot_name(*slot)
                        );
                        for argument in arguments {
                            let _ = write!(call, ", {}", temporary_name(*argument));
                        }
                        let _ = trait_declaration;
                        call.push(')');
                        call
                    }
                    NeverCall::Indirect { callee, arguments } => format!(
                        "{}({})",
                        temporary_name(*callee),
                        arguments
                            .iter()
                            .map(|argument| temporary_name(*argument))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    NeverCall::Closure {
                        instance,
                        closure,
                        arguments,
                    } => {
                        let mut values = vec![temporary_name(*closure)];
                        values.extend(arguments.iter().map(|argument| temporary_name(*argument)));
                        format!("{}({})", self.function_symbol(instance), values.join(", "))
                    }
                };
                let _ = writeln!(self.output, "    {expression};\n    abort();");
            }
            Terminator::Unreachable => self.output.push_str("    abort();\n"),
        }
    }
}
