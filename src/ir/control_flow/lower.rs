//! Lowering from typed high-level IR into explicit control flow.

use super::super::*;

#[must_use]
pub fn lower_control_flow(program: &TypedIrProgram, types: &TypedProgram) -> ControlFlowProgram {
    let functions = program
        .functions
        .iter()
        .map(|function| FunctionLowerer::new(types, function).run())
        .collect::<Vec<_>>();
    let runtime_expression_allocates = functions.iter().any(|function| {
        function.blocks.iter().any(|block| {
            block.instructions.iter().any(|instruction| {
                matches!(
                    instruction,
                    Instruction::Assign {
                        value: Rvalue::FormattedString(_),
                        ..
                    } | Instruction::Assign {
                        value: Rvalue::Binary {
                            operator: BinaryOperator::Concatenate,
                            ..
                        },
                        ..
                    }
                )
            })
        })
    });
    let materializes_managed_values = functions.iter().any(|function| {
        !function.promoted_locals.is_empty()
            || function.allocates_managed
            || std::iter::once(&function.return_type)
                .chain(function.local_types.values())
                .chain(function.parameters.iter().map(|parameter| &parameter.ty))
                .chain(function.temporary_types.iter())
                .any(|ty| type_contains_runtime_managed(&types.types, *ty, 32))
    });
    ControlFlowProgram {
        functions,
        structs: program.structs.clone(),
        enums: program.enums.clone(),
        vtables: program.vtables.clone(),
        // Managed storage is needed when promotion requests it or any
        // materialized value can allocate managed backing storage. Include
        // temporaries and return values: `println(String.from(text))`, for
        // example, need not introduce a source local.
        requires_managed_memory: runtime_expression_allocates || materializes_managed_values,
    }
}

fn type_contains_runtime_managed(types: &TypeContext, ty: TypeId, depth: u32) -> bool {
    if depth == 0 {
        return true;
    }
    match types.kind(types.resolve_inference(ty)) {
        TypeKind::Primitive(PrimitiveType::String)
        | TypeKind::Builtin { .. }
        | TypeKind::Closure { .. } => true,
        TypeKind::Tuple(elements) => elements
            .iter()
            .any(|element| type_contains_runtime_managed(types, *element, depth - 1)),
        TypeKind::Array { element, .. } | TypeKind::Slice(element) => {
            type_contains_runtime_managed(types, *element, depth - 1)
        }
        TypeKind::Nominal { arguments, .. } | TypeKind::Alias { arguments, .. } => arguments
            .iter()
            .any(|argument| type_contains_runtime_managed(types, *argument, depth - 1)),
        _ => false,
    }
}

struct OpenBlock {
    instructions: Vec<Instruction>,
    terminator: Option<Terminator>,
}

/// One lexical scope's cleanup plan: the deferred registrations control has
/// reached, in registration order (`SPEC.md` 8, Milestone 15.6).
///
/// Registration is purely static. A block's statement list is straight-line
/// at the statement level, so at any exit edge the reached registrations are
/// exactly the `defer` statements lexically preceding that edge in each
/// exited scope — no runtime registration list, callable, or environment
/// value exists. Each exit edge re-lowers the registered bodies in place,
/// which also gives deferred expressions their execution-time values
/// (Milestone 15.10): the bodies read their bindings when the edge runs, not
/// when the registration was reached.
type CleanupScope = Vec<Vec<TypedStatement>>;

struct FunctionLowerer<'a> {
    types: &'a TypedProgram,
    function: &'a TypedFunction,
    blocks: Vec<OpenBlock>,
    current: BlockId,
    temporary_types: Vec<TypeId>,
    /// Break target, continue target, and the cleanup-scope depth of the
    /// loop's body: `break`/`continue` run cleanup for every scope deeper
    /// than that base and no other (Milestone 15.8).
    loops: Vec<(BlockId, BlockId, usize)>,
    /// Open lexical scopes, outermost first; the function body is index 0.
    scopes: Vec<CleanupScope>,
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
            scopes: Vec::new(),
        }
    }

    fn run(mut self) -> ControlFlowFunction {
        self.lower_scope(&self.function.body);
        if self.is_open(self.current) {
            if matches!(
                self.types.types.kind(
                    self.types
                        .types
                        .resolve_inference(self.function.return_type)
                ),
                TypeKind::Never
            ) {
                self.terminate(Terminator::Unreachable);
            } else {
                self.terminate(Terminator::Return(None));
            }
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
            closure: self.function.closure.clone(),
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

    /// Lowers one lexical scope's statements: pushes its cleanup plan, and on
    /// normal fallthrough runs that scope's deferred registrations before
    /// control continues past the block (Milestone 15.7).
    fn lower_scope(&mut self, statements: &[TypedStatement]) {
        self.scopes.push(CleanupScope::new());
        self.lower_statements(statements);
        if self.is_open(self.current) {
            self.emit_cleanup(self.scopes.len() - 1);
        }
        self.scopes.pop();
    }

    /// Emits the deferred registrations of every scope at depth `from` or
    /// deeper, innermost scope first and within one scope in reverse
    /// registration order; a `defer:` body executes forward as one unit
    /// (`SPEC.md` 8).
    ///
    /// The registration lists are snapshotted before lowering because a
    /// deferred body may itself open nested (necessarily registration-free)
    /// scopes while it is lowered.
    fn emit_cleanup(&mut self, from: usize) {
        let pending: Vec<Vec<TypedStatement>> = self.scopes[from..]
            .iter()
            .rev()
            .flat_map(|scope| scope.iter().rev().cloned())
            .collect();
        for registration in &pending {
            self.lower_statements(registration);
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
            TypedStatementKind::Destructure {
                bindings, value, ..
            } => {
                // Evaluate the initializer exactly once, then copy each bound
                // component in source order into independent local storage.
                let root = self.lower_expression(value);
                for binding in bindings {
                    let mut place = ControlFlowPlace::Temporary(root);
                    for index in &binding.indices {
                        place = ControlFlowPlace::TupleField {
                            base: Box::new(place),
                            index: *index,
                        };
                    }
                    let loaded = self.temp(binding.ty);
                    self.emit(Instruction::Assign {
                        destination: loaded,
                        value: Rvalue::Load(place),
                        span: statement.span,
                    });
                    let copied = self.temp(binding.ty);
                    self.emit(Instruction::Assign {
                        destination: copied,
                        value: Rvalue::Copy(loaded),
                        span: statement.span,
                    });
                    self.emit(Instruction::Store {
                        place: ControlFlowPlace::Local(binding.binding),
                        value: copied,
                        span: statement.span,
                    });
                }
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
                // The return value is evaluated and independently copied into
                // its temporary *before* any deferred registration runs
                // (`SPEC.md` 8, Milestone 15.7); cleanup then cannot change
                // the returned value, though a shared handle inside it may
                // still observe deferred mutation through its alias.
                let value = value
                    .as_ref()
                    .map(|expression| self.lower_expression(expression));
                self.emit_cleanup(0);
                self.terminate(Terminator::Return(value));
            }
            TypedStatementKind::If {
                condition,
                then_body,
                else_body,
            } => self.lower_if(condition, then_body, else_body),
            TypedStatementKind::While { condition, body } => self.lower_while(condition, body),
            TypedStatementKind::For {
                binding,
                iterable,
                kind,
                body,
            } => self.lower_for(*binding, iterable, *kind, body, statement.span),
            TypedStatementKind::Match { scrutinee, arms } => self.lower_match(scrutinee, arms),
            TypedStatementKind::Block(body) => self.lower_scope(body),
            TypedStatementKind::Defer(body) => {
                // Registration only: control reaching this statement adds the
                // body to the innermost scope's cleanup plan and evaluates
                // nothing (Milestone 15.4).
                if let Some(scope) = self.scopes.last_mut() {
                    scope.push(body.clone());
                }
            }
            TypedStatementKind::Expect {
                selector,
                trait_declaration,
                body,
            } => {
                let selector_value = self.lower_expression(selector);
                let bool_type = self
                    .types
                    .types
                    .id_for_kind(&TypeKind::Primitive(PrimitiveType::Bool))
                    .expect("bool is always canonical");
                let in_child = self.temp(bool_type);
                self.emit(Instruction::Assign {
                    destination: in_child,
                    value: Rvalue::BeginExpectation {
                        selector: selector_value,
                        selector_type: selector.ty,
                        trait_declaration: *trait_declaration,
                    },
                    span: statement.span,
                });
                let child = self.new_block();
                let parent = self.new_block();
                let join = self.new_block();
                self.terminate(Terminator::Branch {
                    condition: in_child,
                    then_block: child,
                    else_block: parent,
                });

                self.current = child;
                self.lower_scope(body);
                if self.is_open(self.current) {
                    self.emit(Instruction::CompleteExpectation {
                        span: statement.span,
                    });
                    self.terminate(Terminator::Unreachable);
                }

                self.current = parent;
                self.emit(Instruction::WaitExpectation {
                    span: statement.span,
                });
                self.terminate(Terminator::Goto(join));
                self.current = join;
            }
            TypedStatementKind::Break => {
                if let Some((break_block, _, base)) = self.loops.last().copied() {
                    // `break` exits every scope down to the loop body and no
                    // enclosing one (Milestone 15.8).
                    self.emit_cleanup(base);
                    self.terminate(Terminator::Goto(break_block));
                }
            }
            TypedStatementKind::Continue => {
                if let Some((_, continue_block, base)) = self.loops.last().copied() {
                    self.emit_cleanup(base);
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
        self.lower_scope(then_body);
        if self.is_open(self.current) {
            self.terminate(Terminator::Goto(join_block));
        }
        self.current = else_block;
        self.lower_scope(else_body);
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
        self.loops
            .push((exit_block, condition_block, self.scopes.len()));
        self.lower_scope(body);
        self.loops.pop();
        if self.is_open(self.current) {
            self.terminate(Terminator::Goto(condition_block));
        }
        self.current = exit_block;
    }

    fn lower_for(
        &mut self,
        binding: LocalBindingId,
        iterable: &TypedExpression,
        kind: IterationKind,
        body: &[TypedStatement],
        span: Span,
    ) {
        let collection = self.lower_expression(iterable);
        let usize_type = self.types.types.primitive_id(PrimitiveType::Usize);
        let bool_type = self.types.types.primitive_id(PrimitiveType::Bool);
        let index = self.temp(usize_type);
        self.emit(Instruction::Assign {
            destination: index,
            value: Rvalue::Constant(Constant::Integer {
                magnitude: 0,
                negative: false,
            }),
            span,
        });
        let condition_block = self.new_block();
        let body_block = self.new_block();
        let increment_block = self.new_block();
        let exit_block = self.new_block();
        self.terminate(Terminator::Goto(condition_block));

        self.current = condition_block;
        let length = self.temp(usize_type);
        self.emit(Instruction::Assign {
            destination: length,
            value: Rvalue::CollectionLength { collection, kind },
            span,
        });
        let condition = self.temp(bool_type);
        self.emit(Instruction::Assign {
            destination: condition,
            value: Rvalue::Binary {
                operator: BinaryOperator::Less,
                left: index,
                right: length,
                trap: None,
            },
            span,
        });
        self.terminate(Terminator::Branch {
            condition,
            then_block: body_block,
            else_block: exit_block,
        });

        self.current = body_block;
        let element_type = self
            .function
            .local_types
            .get(&binding)
            .copied()
            .unwrap_or_else(|| self.types.types.error());
        let element = self.temp(element_type);
        self.emit(Instruction::Assign {
            destination: element,
            value: Rvalue::IterationElement {
                collection,
                index,
                kind,
            },
            span,
        });
        self.emit(Instruction::Store {
            place: ControlFlowPlace::Local(binding),
            value: element,
            span,
        });
        self.loops
            .push((exit_block, increment_block, self.scopes.len()));
        self.lower_scope(body);
        self.loops.pop();
        if self.is_open(self.current) {
            self.terminate(Terminator::Goto(increment_block));
        }

        self.current = increment_block;
        let one = self.temp(usize_type);
        self.emit(Instruction::Assign {
            destination: one,
            value: Rvalue::Constant(Constant::Integer {
                magnitude: 1,
                negative: false,
            }),
            span,
        });
        self.emit(Instruction::Assign {
            destination: index,
            value: Rvalue::Binary {
                operator: BinaryOperator::Add,
                left: index,
                right: one,
                trap: None,
            },
            span,
        });
        self.terminate(Terminator::Goto(condition_block));
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
            self.lower_scope(&arm.body);
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
        if matches!(
            self.types
                .types
                .kind(self.types.types.resolve_inference(expression.ty)),
            TypeKind::Never
        ) && let Some(call) = self.lower_never_call(expression)
        {
            self.terminate(Terminator::NeverCall {
                call,
                span: expression.span,
            });
            // Continue lowering structurally unreachable syntax in a detached
            // block. Reachability pruning removes it before C emission.
            self.current = self.new_block();
            return self.temp(expression.ty);
        }
        let value = match &expression.kind {
            TypedExpressionKind::Constant(constant) => Rvalue::Constant(constant.clone()),
            TypedExpressionKind::FunctionReference(instance) => {
                Rvalue::FunctionReference(instance.clone())
            }
            TypedExpressionKind::Local(binding) => Rvalue::Load(ControlFlowPlace::Local(*binding)),
            TypedExpressionKind::ClosureCapture(index) => {
                Rvalue::Load(ControlFlowPlace::ClosureCapture(*index))
            }
            TypedExpressionKind::Closure { captures, .. } => Rvalue::Closure {
                ty: expression.ty,
                captures: captures
                    .iter()
                    .map(|capture| self.lower_expression(capture))
                    .collect(),
            },
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
                let base = self.lower_expression(operand);
                self.check_raw_pointer_access(operand, base, expression.span);
                Rvalue::Load(ControlFlowPlace::Dereference {
                    base: Box::new(ControlFlowPlace::Temporary(base)),
                })
            }
            TypedExpressionKind::DefaultValue(ty) => Rvalue::DefaultValue(*ty),
            TypedExpressionKind::NumericConversion {
                outcome,
                value,
                target,
            } => {
                let source_type = value.ty;
                let value = self.lower_expression(value);
                Rvalue::NumericConversion {
                    outcome: *outcome,
                    value,
                    source_type,
                    target_type: *target,
                }
            }
            TypedExpressionKind::NumericAlternative {
                operation,
                receiver,
                operand,
            } => {
                let operand_type = receiver.ty;
                let receiver = self.lower_expression(receiver);
                let operand = operand
                    .as_ref()
                    .map(|operand| self.lower_expression(operand));
                Rvalue::NumericAlternative {
                    operation: *operation,
                    receiver,
                    operand,
                    operand_type,
                }
            }
            TypedExpressionKind::StandardCall {
                operation,
                arguments,
            } => Rvalue::StandardCall {
                operation: *operation,
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
            },
            TypedExpressionKind::CollectionLiteral { kind, elements } => {
                Rvalue::CollectionLiteral {
                    kind: *kind,
                    elements: elements
                        .iter()
                        .map(|element| self.lower_expression(element))
                        .collect(),
                }
            }
            TypedExpressionKind::MakeTraitObject {
                value,
                trait_declaration,
                trait_type,
                concrete,
            } => {
                let value = self.lower_expression(value);
                Rvalue::MakeTraitObject {
                    value,
                    trait_declaration: *trait_declaration,
                    trait_type: *trait_type,
                    concrete: *concrete,
                }
            }
            TypedExpressionKind::Unary { operator, operand } => {
                let operand = self.lower_expression(operand);
                Rvalue::Unary {
                    operator: *operator,
                    operand,
                    trap: unary_trap(*operator, expression.ty, self.types),
                }
            }
            TypedExpressionKind::Propagate {
                operand,
                result_declaration,
                ok_variant,
                ok_field,
                err_variant,
                err_field,
                error_type,
            } => {
                return self.lower_propagate(
                    expression,
                    operand,
                    *result_declaration,
                    *ok_variant,
                    *ok_field,
                    *err_variant,
                    *err_field,
                    *error_type,
                );
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
                let operand_type = left.ty;
                let left_value = self.lower_expression(left);
                let right_value = self.lower_expression(right);
                // Equality on an aggregate compares components (`SPEC.md`
                // 4.3), so it carries its operand type to the backend rather
                // than becoming a C `==` on a struct.
                if matches!(operator, BinaryOperator::Equal | BinaryOperator::NotEqual)
                    && structural_equality(&self.types.types, operand_type)
                {
                    let equal = self.temp(self.types.types.primitive_id(PrimitiveType::Bool));
                    self.emit(Instruction::Assign {
                        destination: equal,
                        value: Rvalue::CompareEqual {
                            left: left_value,
                            right: right_value,
                            operand_type,
                        },
                        span: expression.span,
                    });
                    if *operator == BinaryOperator::Equal {
                        return equal;
                    }
                    Rvalue::Unary {
                        operator: UnaryOperator::LogicalNot,
                        operand: equal,
                        trap: None,
                    }
                } else if matches!(
                    operator,
                    BinaryOperator::Less
                        | BinaryOperator::LessEqual
                        | BinaryOperator::Greater
                        | BinaryOperator::GreaterEqual
                ) && structural_ordering(&self.types.types, operand_type)
                {
                    Rvalue::CompareOrder {
                        operator: *operator,
                        left: left_value,
                        right: right_value,
                        operand_type,
                    }
                } else {
                    Rvalue::Binary {
                        operator: *operator,
                        left: left_value,
                        right: right_value,
                        trap: binary_trap(*operator, expression.ty, self.types),
                    }
                }
            }
            TypedExpressionKind::Cast { value } => {
                let source_type = value.ty;
                let operand = value;
                let value = self.lower_expression(operand);
                // A raw-to-reference conversion asserts validity, so the
                // null/alignment check precedes it (Milestones 16.8/16.9).
                if matches!(
                    expanded_kind(&self.types.types, source_type),
                    TypeKind::RawPointer { .. }
                ) && matches!(
                    expanded_kind(&self.types.types, expression.ty),
                    TypeKind::Reference { .. }
                ) {
                    self.check_raw_pointer_access(operand, value, expression.span);
                }
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
                callee:
                    TypedCallee::Dynamic {
                        trait_declaration,
                        slot,
                    },
                arguments,
            } => {
                // The receiver was lowered as the first argument by typed-IR
                // call lowering; the vtable supplies the function pointer.
                let mut lowered = arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect::<Vec<_>>();
                let receiver = lowered.remove(0);
                Rvalue::DynamicCall {
                    receiver,
                    trait_declaration: *trait_declaration,
                    slot: *slot,
                    arguments: lowered,
                }
            }
            TypedExpressionKind::Call {
                callee: TypedCallee::Indirect(callee),
                arguments,
            } => {
                let raw = matches!(
                    expanded_kind(&self.types.types, callee.ty),
                    TypeKind::RawPointer { target, .. }
                        if matches!(
                            expanded_kind(&self.types.types, *target),
                            TypeKind::Function { .. }
                        )
                );
                let callee = self.lower_expression(callee);
                if raw {
                    self.emit(Instruction::CheckFunctionPointer {
                        pointer: callee,
                        span: expression.span,
                    });
                }
                let arguments = arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect();
                Rvalue::IndirectCall { callee, arguments }
            }
            TypedExpressionKind::Call {
                callee: TypedCallee::Closure { instance, value },
                arguments,
            } => {
                let closure = self.lower_expression(value);
                let arguments = arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect();
                Rvalue::ClosureCall {
                    instance: instance.clone(),
                    closure,
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
            TypedExpressionKind::TupleField { base, index } => {
                let place = self.expression_place(base);
                Rvalue::Load(ControlFlowPlace::TupleField {
                    base: Box::new(place),
                    index: *index,
                })
            }
            TypedExpressionKind::Index { base, index } => {
                let base_place = self.expression_place(base);
                let index = self.lower_expression(index);
                let (kind, trap) = match expanded_kind(&self.types.types, base.ty) {
                    TypeKind::Array { length, .. } => (
                        IndexKind::Array { length: *length },
                        TrapKind::IndexOutOfBounds,
                    ),
                    TypeKind::Slice(_) => (IndexKind::Slice, TrapKind::IndexOutOfBounds),
                    TypeKind::Builtin { arguments, .. } => {
                        // Of the initial builtin collections only `Vec[T]`
                        // (one argument) and `Map[K, V]` (two arguments) are
                        // indexable; `Set[T]` was rejected by checking.
                        if arguments.len() == 1 {
                            (
                                IndexKind::Vec {
                                    collection: base.ty,
                                },
                                TrapKind::IndexOutOfBounds,
                            )
                        } else {
                            (
                                IndexKind::Map {
                                    collection: base.ty,
                                },
                                TrapKind::MissingMapKey,
                            )
                        }
                    }
                    _ => (IndexKind::Array { length: 0 }, TrapKind::IndexOutOfBounds),
                };
                Rvalue::Load(ControlFlowPlace::Index {
                    base: Box::new(base_place),
                    index,
                    kind,
                    trap,
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
            TypedExpressionKind::FormattedString(parts) => Rvalue::FormattedString(
                parts
                    .iter()
                    .map(|part| match part {
                        FormattedPart::Text(text) => RuntimeFormattedPart::Text(text.clone()),
                        FormattedPart::Expression(value) => RuntimeFormattedPart::Value {
                            value: self.lower_expression(value),
                            ty: value.ty,
                        },
                    })
                    .collect(),
            ),
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

    fn lower_never_call(&mut self, expression: &TypedExpression) -> Option<NeverCall> {
        match &expression.kind {
            TypedExpressionKind::StandardCall {
                operation: StandardCall::Panic,
                arguments,
            } => Some(NeverCall::Panic {
                message: self.lower_expression(arguments.first()?),
            }),
            TypedExpressionKind::StandardCall {
                operation: StandardCall::Fail { value_type },
                arguments,
            } => Some(NeverCall::AssertionFail {
                value: self.lower_expression(arguments.first()?),
                value_type: *value_type,
            }),
            TypedExpressionKind::StandardCall {
                operation:
                    StandardCall::Trap {
                        reason_type,
                        trait_declaration,
                    },
                arguments,
            } => Some(NeverCall::TypedTrap {
                reason: self.lower_expression(arguments.first()?),
                reason_type: *reason_type,
                trait_declaration: *trait_declaration,
            }),
            TypedExpressionKind::Call {
                callee: TypedCallee::Function(instance),
                arguments,
            } => Some(NeverCall::Direct {
                instance: instance.clone(),
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
            }),
            TypedExpressionKind::Call {
                callee:
                    TypedCallee::Dynamic {
                        trait_declaration,
                        slot,
                    },
                arguments,
            } => {
                let mut arguments = arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect::<Vec<_>>();
                let receiver = arguments.first().copied()?;
                arguments.remove(0);
                Some(NeverCall::Dynamic {
                    receiver,
                    trait_declaration: *trait_declaration,
                    slot: *slot,
                    arguments,
                })
            }
            TypedExpressionKind::Call {
                callee: TypedCallee::Indirect(callee),
                arguments,
            } => {
                let raw = matches!(
                    expanded_kind(&self.types.types, callee.ty),
                    TypeKind::RawPointer { target, .. }
                        if matches!(
                            expanded_kind(&self.types.types, *target),
                            TypeKind::Function { .. }
                        )
                );
                let callee = self.lower_expression(callee);
                if raw {
                    self.emit(Instruction::CheckFunctionPointer {
                        pointer: callee,
                        span: expression.span,
                    });
                }
                Some(NeverCall::Indirect {
                    callee,
                    arguments: arguments
                        .iter()
                        .map(|argument| self.lower_expression(argument))
                        .collect(),
                })
            }
            TypedExpressionKind::Call {
                callee: TypedCallee::Closure { instance, value },
                arguments,
            } => Some(NeverCall::Closure {
                instance: instance.clone(),
                closure: self.lower_expression(value),
                arguments: arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect(),
            }),
            _ => None,
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

    /// Lowers postfix `?` (`SPEC.md` 8, Milestones 15.3 and 15.9).
    ///
    /// The operand is evaluated exactly once into a temporary. Its tag is
    /// branched on explicitly: the `Ok` payload is copied into this
    /// expression's value, while the `Err` path copies the error payload,
    /// builds the enclosing function's `Result.Err` return value from that
    /// copy, *then* runs every open scope's deferred registrations, and
    /// returns. The error is copied before cleanup, so deferred mutation
    /// cannot change the propagated error.
    #[allow(clippy::too_many_arguments)]
    fn lower_propagate(
        &mut self,
        expression: &TypedExpression,
        operand: &TypedExpression,
        result_declaration: DeclarationId,
        ok_variant: VariantId,
        ok_field: FieldId,
        err_variant: VariantId,
        err_field: FieldId,
        error_type: TypeId,
    ) -> TemporaryId {
        let span = expression.span;
        let value = self.lower_expression(operand);
        let u32_type = self.types.types.primitive_id(PrimitiveType::U32);
        let bool_type = self.types.types.primitive_id(PrimitiveType::Bool);
        let discriminant = self.temp(u32_type);
        self.emit(Instruction::Assign {
            destination: discriminant,
            value: Rvalue::Discriminant(value),
            span,
        });
        let expected = self.temp(u32_type);
        self.emit(Instruction::Assign {
            destination: expected,
            value: Rvalue::Constant(Constant::Integer {
                magnitude: ok_variant.index() as u128,
                negative: false,
            }),
            span,
        });
        let condition = self.temp(bool_type);
        self.emit(Instruction::Assign {
            destination: condition,
            value: Rvalue::CompareEqual {
                left: discriminant,
                right: expected,
                operand_type: u32_type,
            },
            span,
        });
        let ok_block = self.new_block();
        let err_block = self.new_block();
        self.terminate(Terminator::Branch {
            condition,
            then_block: ok_block,
            else_block: err_block,
        });

        self.current = err_block;
        let error_raw = self.temp(error_type);
        self.emit(Instruction::Assign {
            destination: error_raw,
            value: Rvalue::Load(ControlFlowPlace::VariantField {
                base: Box::new(ControlFlowPlace::Temporary(value)),
                variant: err_variant,
                field: err_field,
            }),
            span,
        });
        let error = self.temp(error_type);
        self.emit(Instruction::Assign {
            destination: error,
            value: Rvalue::Copy(error_raw),
            span,
        });
        let propagated = self.temp(self.function.return_type);
        self.emit(Instruction::Assign {
            destination: propagated,
            value: Rvalue::Aggregate(AggregateValue::Enum {
                declaration: result_declaration,
                variant: err_variant,
                fields: vec![(err_field, error)],
            }),
            span,
        });
        self.emit_cleanup(0);
        self.terminate(Terminator::Return(Some(propagated)));

        self.current = ok_block;
        let payload_raw = self.temp(expression.ty);
        self.emit(Instruction::Assign {
            destination: payload_raw,
            value: Rvalue::Load(ControlFlowPlace::VariantField {
                base: Box::new(ControlFlowPlace::Temporary(value)),
                variant: ok_variant,
                field: ok_field,
            }),
            span,
        });
        let payload = self.temp(expression.ty);
        self.emit(Instruction::Assign {
            destination: payload,
            value: Rvalue::Copy(payload_raw),
            span,
        });
        payload
    }

    fn lower_print(&mut self, expression: &TypedExpression) {
        let temporary = self.lower_expression(expression);
        self.emit(Instruction::PrintValue {
            value: temporary,
            ty: expression.ty,
            span: expression.span,
        });
    }

    fn lower_place(&mut self, place: &TypedPlace) -> ControlFlowPlace {
        match place {
            TypedPlace::Local { binding, .. } => ControlFlowPlace::Local(*binding),
            TypedPlace::ClosureCapture { index, .. } => ControlFlowPlace::ClosureCapture(*index),
            TypedPlace::Field { base, field, .. } => ControlFlowPlace::Field {
                base: Box::new(self.lower_place(base)),
                field: *field,
            },
            TypedPlace::TupleField { base, index, .. } => ControlFlowPlace::TupleField {
                base: Box::new(self.lower_place(base)),
                index: *index,
            },
            TypedPlace::Index {
                base, index, kind, ..
            } => {
                let base = self.lower_place(base);
                let index = self.lower_expression(index);
                ControlFlowPlace::Index {
                    base: Box::new(base),
                    index,
                    kind: *kind,
                    trap: if matches!(kind, IndexKind::Map { .. }) {
                        TrapKind::MissingMapKey
                    } else {
                        TrapKind::IndexOutOfBounds
                    },
                }
            }
            TypedPlace::Dereference { base, span, .. } => {
                let value = self.lower_expression(base);
                self.check_raw_pointer_access(base, value, *span);
                ControlFlowPlace::Dereference {
                    base: Box::new(ControlFlowPlace::Temporary(value)),
                }
            }
        }
    }

    /// Emits the mandatory null/alignment check when `base` is a raw pointer
    /// about to be dereferenced or converted to a reference. Safe references
    /// are non-null by construction and need no check.
    fn check_raw_pointer_access(
        &mut self,
        base: &TypedExpression,
        pointer: TemporaryId,
        span: Span,
    ) {
        if let TypeKind::RawPointer { target, .. } = expanded_kind(&self.types.types, base.ty) {
            self.emit(Instruction::CheckPointer {
                pointer,
                pointee: *target,
                span,
            });
        }
    }

    fn expression_place(&mut self, expression: &TypedExpression) -> ControlFlowPlace {
        match &expression.kind {
            TypedExpressionKind::Local(binding) => ControlFlowPlace::Local(*binding),
            TypedExpressionKind::ClosureCapture(index) => ControlFlowPlace::ClosureCapture(*index),
            TypedExpressionKind::Field { base, field } => ControlFlowPlace::Field {
                base: Box::new(self.expression_place(base)),
                field: *field,
            },
            TypedExpressionKind::TupleField { base, index } => ControlFlowPlace::TupleField {
                base: Box::new(self.expression_place(base)),
                index: *index,
            },
            TypedExpressionKind::Index { base, index } => {
                let base = self.expression_place(base);
                let index = self.lower_expression(index);
                let (kind, trap) = match expanded_kind(
                    &self.types.types,
                    base_expression_type(&expression.kind),
                ) {
                    TypeKind::Array { length, .. } => (
                        IndexKind::Array { length: *length },
                        TrapKind::IndexOutOfBounds,
                    ),
                    TypeKind::Slice(_) => (IndexKind::Slice, TrapKind::IndexOutOfBounds),
                    TypeKind::Builtin { arguments, .. } if arguments.len() == 1 => (
                        IndexKind::Vec {
                            collection: base_expression_type(&expression.kind),
                        },
                        TrapKind::IndexOutOfBounds,
                    ),
                    TypeKind::Builtin { .. } => (
                        IndexKind::Map {
                            collection: base_expression_type(&expression.kind),
                        },
                        TrapKind::MissingMapKey,
                    ),
                    _ => (IndexKind::Array { length: 0 }, TrapKind::IndexOutOfBounds),
                };
                ControlFlowPlace::Index {
                    base: Box::new(base),
                    index,
                    kind,
                    trap,
                }
            }
            TypedExpressionKind::Dereference(operand) => {
                let value = self.lower_expression(operand);
                self.check_raw_pointer_access(operand, value, expression.span);
                ControlFlowPlace::Dereference {
                    base: Box::new(ControlFlowPlace::Temporary(value)),
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

/// Whether equality on `ty` compares components rather than machine values.
fn structural_equality(types: &TypeContext, ty: TypeId) -> bool {
    let mut ty = ty;
    while let TypeKind::Alias { target, .. } = types.kind(ty) {
        ty = *target;
    }
    matches!(
        types.kind(ty),
        TypeKind::Tuple(_)
            | TypeKind::Array { .. }
            | TypeKind::Nominal { .. }
            | TypeKind::Builtin { .. }
    ) || matches!(
        types.expanded_primitive(ty),
        Some(PrimitiveType::Str | PrimitiveType::String)
    )
}

/// Whether ordering on `ty` must compare components rather than machine
/// values.
fn structural_ordering(types: &TypeContext, ty: TypeId) -> bool {
    structural_equality(types, ty)
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
