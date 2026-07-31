//! Function-value, call, member-selection, and receiver-selection checking.

use super::*;

impl<'a> Checker<'a> {
    pub(super) fn function_reference_type(&mut self, instance: &FunctionInstance) -> TypeId {
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
        let kind = if self.resolved.declarations[instance.declaration.index()].kind
            == DeclarationKind::ForeignFunction
        {
            TypeKind::RawPointer {
                mutability: Mutability::Shared,
                target: function,
            }
        } else {
            TypeKind::Reference {
                mutability: Mutability::Shared,
                target: function,
            }
        };
        self.typed.types.intern(kind)
    }

    pub(super) fn callable_parameters(
        &self,
        declaration: DeclarationId,
    ) -> Vec<GenericParameterId> {
        self.typed
            .callable_generic_parameters(self.resolved, declaration)
    }

    pub(super) fn validate_instance_obligations(
        &mut self,
        instance: &FunctionInstance,
        span: Span,
    ) -> bool {
        let obligations = self
            .callable_parameters(instance.declaration)
            .into_iter()
            .zip(instance.arguments.iter().copied())
            .flat_map(|(parameter, argument)| {
                self.typed
                    .obligations_for(parameter)
                    .map(move |obligation| (argument, obligation.trait_type))
            })
            .collect::<Vec<_>>();
        let mut valid = true;
        for (argument, trait_type) in obligations {
            let trait_name = crate::traits::trait_name(self.resolved, self.typed, trait_type);
            let substitution = self.typed.instance_substitution(self.resolved, instance);
            let required = self.typed.types.substitute(trait_type, &substitution);
            let satisfies = if trait_name == "Callable" {
                self.satisfies_callable_signature(argument, required)
            } else {
                crate::traits::provides(self.resolved, self.typed, argument, &trait_name)
            };
            if !satisfies {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::TypeSystem,
                        format!(
                            "generic argument does not satisfy required `{trait_name}` capability"
                        ),
                    )
                    .with_primary(span),
                );
                valid = false;
            }
        }
        valid
    }

    fn satisfies_callable_signature(&mut self, actual: TypeId, required: TypeId) -> bool {
        let TypeKind::Nominal {
            identity,
            arguments,
        } = self
            .typed
            .types
            .kind(self.typed.types.resolve_inference(required))
        else {
            return false;
        };
        if !self
            .resolved
            .is_standard_declaration(identity.declaration, "Callable")
        {
            return false;
        }
        let [argument_tuple, required_return] = arguments.as_slice() else {
            return false;
        };
        let required_kind = self
            .typed
            .types
            .kind(self.typed.types.resolve_inference(*argument_tuple));
        let required_parameters = match required_kind {
            TypeKind::Tuple(parameters) => parameters.clone(),
            TypeKind::Primitive(PrimitiveType::Unit) => Vec::new(),
            _ => return false,
        };
        let required_return = *required_return;
        let actual_kind = self
            .typed
            .types
            .kind(self.typed.types.resolve_inference(actual))
            .clone();
        if matches!(actual_kind, TypeKind::Nominal { .. }) {
            let Some((_, parameters, return_type)) = self.callable_impl_signature(actual) else {
                return false;
            };
            return parameters.len() == required_parameters.len()
                && parameters
                    .iter()
                    .zip(&required_parameters)
                    .all(|(actual, required)| self.typed.types.exactly_equal(*actual, *required))
                && self.typed.types.exactly_equal(return_type, required_return);
        }
        let exact_signature = |parameters: &[TypeId], return_type: TypeId| {
            parameters.len() == required_parameters.len()
                && parameters
                    .iter()
                    .zip(&required_parameters)
                    .all(|(actual, required)| self.typed.types.exactly_equal(*actual, *required))
                && self.typed.types.exactly_equal(return_type, required_return)
        };
        match actual_kind {
            TypeKind::Closure {
                parameters,
                return_type,
                ..
            } => exact_signature(&parameters, return_type),
            TypeKind::Reference { target, .. } => match self
                .typed
                .types
                .kind(self.typed.types.resolve_inference(target))
            {
                TypeKind::Function {
                    safety: Safety::Safe,
                    receiver: None,
                    parameters,
                    return_type,
                    ..
                } if parameters.iter().all(|parameter| !parameter.variadic) => {
                    let parameters = parameters
                        .iter()
                        .map(|parameter| parameter.ty)
                        .collect::<Vec<_>>();
                    exact_signature(&parameters, *return_type)
                }
                _ => false,
            },
            TypeKind::Nominal { .. } => unreachable!("handled before immutable comparison"),
            _ => false,
        }
    }

    pub(super) fn generic_inference_variables(
        &mut self,
        parameters: &[GenericParameterId],
    ) -> BTreeMap<GenericParameterId, TypeId> {
        parameters
            .iter()
            .map(|parameter| (*parameter, self.typed.types.fresh_inference_variable()))
            .collect()
    }

    pub(super) fn inference_substitution(
        variables: &BTreeMap<GenericParameterId, TypeId>,
    ) -> Substitution {
        let mut substitution = Substitution::new();
        for (parameter, variable) in variables {
            substitution.insert(*parameter, *variable);
        }
        substitution
    }

    pub(super) fn infer_against(
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

    pub(super) fn finish_inference(
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

    pub(super) fn type_from_expression(&mut self, node: &SyntaxNode) -> Option<TypeId> {
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

    pub(super) fn explicit_function_selection(
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

    pub(super) fn function_instance_from_expected(
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
            let instance = FunctionInstance {
                declaration,
                arguments,
                self_type: None,
            };
            return self
                .validate_instance_obligations(&instance, span)
                .then_some(instance);
        }
        if parameters.is_empty() {
            return Some(FunctionInstance {
                declaration,
                arguments: Vec::new(),
                self_type: None,
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
            self_type: None,
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
        let instance = FunctionInstance {
            declaration,
            arguments,
            self_type: None,
        };
        self.validate_instance_obligations(&instance, span)
            .then_some(instance)
    }

    pub(super) fn check_bracket_expression(
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

    pub(super) fn check_function_call(
        &mut self,
        call_span: Span,
        declaration: DeclarationId,
        explicit: Option<Vec<TypeId>>,
        arguments: &[&SyntaxNode],
        expected: ExpectedType,
        include_receiver: bool,
    ) -> (TypeId, PlaceKind) {
        let selected_self_type = self.pending_self_type.take();
        if self.resolved.is_standard_declaration(declaration, "panic")
            || self.resolved.is_standard_declaration(declaration, "assert")
        {
            if let Some(explicit) = explicit {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Call,
                        format!(
                            "this function requires 0 generic arguments, but {} were supplied",
                            explicit.len()
                        ),
                    )
                    .with_primary(call_span),
                );
            }
            let Some(signature) = self.typed.function_signatures.get(&declaration).cloned() else {
                return (self.typed.types.error(), PlaceKind::Value);
            };
            self.check_call_arguments(call_span, &signature.parameters, arguments);
            let operation = if self.resolved.is_standard_declaration(declaration, "panic") {
                StandardCall::Panic
            } else {
                StandardCall::Assert
            };
            self.program
                .calls
                .insert(call_span, CheckedCall::Standard(operation));
            return (signature.return_type, PlaceKind::Value);
        }
        let generic_parameters = self.callable_parameters(declaration);
        if let Some(explicit) = explicit {
            let Some(mut instance) = self.function_instance_from_expected(
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
            instance.self_type = selected_self_type;
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
                && !self.is_trait_object_reference(expected)
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
            let call = self
                .testing_intrinsic_call(declaration, &instance)
                .map_or_else(|| CheckedCall::Direct(instance), CheckedCall::Standard);
            self.program.calls.insert(call_span, call);
            return (signature.return_type, PlaceKind::Value);
        }

        if generic_parameters.is_empty() {
            let instance = FunctionInstance {
                declaration,
                arguments: Vec::new(),
                self_type: selected_self_type,
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

        let Some(mut template) = self.typed.function_signatures.get(&declaration).cloned() else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        if let Some(self_type) = selected_self_type {
            template = FunctionSignature {
                ty: template.ty,
                receiver: template
                    .receiver
                    .map(|receiver| self.typed.types.substitute_self(receiver, self_type)),
                parameters: template
                    .parameters
                    .into_iter()
                    .map(|parameter| FunctionParameter {
                        ty: self.typed.types.substitute_self(parameter.ty, self_type),
                        variadic: parameter.variadic,
                    })
                    .collect(),
                return_type: self
                    .typed
                    .types
                    .substitute_self(template.return_type, self_type),
            };
        }
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
            self_type: selected_self_type,
        };
        if !self.validate_instance_obligations(&instance, call_span) {
            return (self.typed.types.error(), PlaceKind::Value);
        }
        let Some(signature) = self.typed.instantiate_signature(self.resolved, &instance) else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        if let ExpectedType::Exact(expected) = expected
            && !self.is_trait_object_reference(expected)
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
        let call = self
            .testing_intrinsic_call(declaration, &instance)
            .map_or_else(|| CheckedCall::Direct(instance), CheckedCall::Standard);
        self.program.calls.insert(call_span, call);
        (signature.return_type, PlaceKind::Value)
    }

    fn testing_intrinsic_call(
        &self,
        declaration: DeclarationId,
        instance: &FunctionInstance,
    ) -> Option<StandardCall> {
        let value_type = instance.arguments.first().copied()?;
        if self.resolved.is_standard_declaration(declaration, "fail") {
            return Some(StandardCall::Fail { value_type });
        }
        if self.resolved.is_standard_declaration(declaration, "trap") {
            return Some(StandardCall::Trap {
                reason_type: value_type,
                trait_declaration: self.resolved.standard_declaration("RuntimeTrap")?,
            });
        }
        None
    }

    pub(super) fn report_call_arity(
        &mut self,
        span: Span,
        supplied: usize,
        fixed: usize,
        variadic: bool,
    ) {
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
    pub(super) fn callee_target_span(node: &SyntaxNode) -> Option<Span> {
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

    pub(super) fn find_member(&self, declaration: DeclarationId, name: &str) -> Option<MemberId> {
        self.resolved
            .declaration_members
            .get(&declaration)?
            .iter()
            .find(|(symbol, _)| self.resolved.symbol_text(**symbol) == name)
            .map(|(_, member)| *member)
    }

    pub(super) fn type_declaration_expression(&self, node: &SyntaxNode) -> Option<DeclarationId> {
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

    pub(super) fn resolve_enum_variant_callee(
        &self,
        node: &SyntaxNode,
    ) -> Option<(DeclarationId, VariantId)> {
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

    pub(super) fn check_member_expression(
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
                self_type: None,
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
                        self_type: None,
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
                TypeKind::Foreign {
                    identity,
                    complete: true,
                } => (identity.declaration, Vec::new(), base_place),
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
                        TypeKind::Foreign {
                            identity,
                            complete: true,
                        } => {
                            let place = if mutability == Mutability::Mutable {
                                PlaceKind::Mutable
                            } else {
                                PlaceKind::Addressable
                            };
                            (identity.declaration, Vec::new(), place)
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
                    // A field of a raw dereference stays inside the raw
                    // target: assignable, never safe-reference-formable.
                    PlaceKind::RawPointerTarget => PlaceKind::RawPointerTarget,
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

    pub(super) fn is_integer_type(&self, ty: TypeId) -> bool {
        self.typed
            .types
            .expanded_primitive(ty)
            .is_some_and(PrimitiveType::is_integer)
    }

    pub(super) fn is_float_type(&self, ty: TypeId) -> bool {
        self.typed
            .types
            .expanded_primitive(ty)
            .is_some_and(PrimitiveType::is_float)
    }

    pub(super) fn is_numeric_type(&self, ty: TypeId) -> bool {
        self.is_integer_type(ty) || self.is_float_type(ty)
    }

    /// Verifies that `source_target` implements the trait named by a
    /// trait-object type, and that the trait is object-safe.
    pub(super) fn check_trait_object_formation(
        &mut self,
        object_type: TypeId,
        source_target: TypeId,
        span: Span,
    ) -> bool {
        let Some(trait_declaration) =
            crate::traits::object_trait(self.resolved, self.typed, object_type)
        else {
            return true;
        };
        let trait_name = self
            .resolved
            .symbol_text(self.resolved.declarations[trait_declaration.index()].name)
            .to_string();
        if let Some(violation) =
            crate::traits::object_safety_violation(self.resolved, self.typed, trait_declaration)
        {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::TypeSystem,
                    format!("trait `{trait_name}` cannot form a trait object because {violation}"),
                )
                .with_primary(span),
            );
            return false;
        }
        let implemented = if self
            .resolved
            .is_standard_declaration(trait_declaration, "Callable")
        {
            let target = self.typed.types.resolve_inference(object_type);
            let trait_type = match self.typed.types.kind(target) {
                TypeKind::Reference { target, .. } => {
                    match self
                        .typed
                        .types
                        .kind(self.typed.types.resolve_inference(*target))
                    {
                        TypeKind::TraitObject { trait_type } => Some(*trait_type),
                        _ => None,
                    }
                }
                _ => None,
            };
            trait_type
                .is_some_and(|required| self.satisfies_callable_signature(source_target, required))
        } else {
            crate::traits::implements_trait(
                self.resolved,
                self.typed,
                source_target,
                trait_declaration,
            )
        };
        if !implemented {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::TypeSystem,
                    format!("this type does not implement trait `{trait_name}`"),
                )
                .with_primary(span),
            );
            return false;
        }
        true
    }

    pub(super) fn is_trait_object_reference(&self, ty: TypeId) -> bool {
        let resolved = self.typed.types.resolve_inference(ty);
        let target = match self.typed.types.kind(resolved) {
            TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => *target,
            _ => return false,
        };
        let target = self.typed.types.resolve_inference(target);
        matches!(self.typed.types.kind(target), TypeKind::TraitObject { .. })
    }

    /// Validates and records the contextual, mutability-preserving conversion
    /// from a concrete safe reference to a trait-object reference.
    pub(super) fn record_trait_object_coercion(
        &mut self,
        span: Span,
        source_type: TypeId,
        target_type: TypeId,
    ) -> Option<TraitObjectCoercion> {
        let source = self.typed.types.resolve_inference(source_type);
        let TypeKind::Reference {
            mutability: source_mutability,
            target: concrete,
        } = self.typed.types.kind(source).clone()
        else {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ExpressionType,
                    "an implicit trait-object conversion requires a safe reference",
                )
                .with_primary(span),
            );
            return None;
        };
        let target = self.typed.types.resolve_inference(target_type);
        let TypeKind::Reference {
            mutability: target_mutability,
            ..
        } = self.typed.types.kind(target)
        else {
            return None;
        };
        if source_mutability != *target_mutability {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::ExpressionType,
                    "a trait-object conversion must preserve reference mutability",
                )
                .with_primary(span),
            );
            return None;
        }
        if !self.check_trait_object_formation(target_type, concrete, span) {
            return None;
        }
        let trait_declaration =
            crate::traits::object_trait(self.resolved, self.typed, target_type)?;
        Some(TraitObjectCoercion {
            source: source_type,
            target: target_type,
            trait_declaration,
            concrete,
        })
    }

    /// Whether an expression of type `actual` may be used where `expected`
    /// is required. Contextual trait-object conversion is performed and
    /// recorded by `check_expr`, so compatible checked expressions already
    /// carry the expected trait-object type here.
    pub(super) fn types_compatible(&self, actual: TypeId, expected: TypeId) -> bool {
        self.is_never_type(actual)
            || actual == self.typed.types.error()
            || expected == self.typed.types.error()
            || self.typed.types.exactly_equal(actual, expected)
    }

    pub(super) fn check_call(
        &mut self,
        node: &SyntaxNode,
        expected: ExpectedType,
    ) -> (TypeId, PlaceKind) {
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
                    if declaration.kind == DeclarationKind::Test {
                        self.diagnostics.push(
                            Diagnostic::new(Category::Call, "a test declaration is not callable")
                                .with_primary(node.span)
                                .with_related(declaration.span, "test declared here"),
                        );
                        for argument in arguments {
                            self.check_expr(argument, ExpectedType::None);
                        }
                        return (self.typed.types.error(), PlaceKind::Value);
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
                        let (ty, _) = self.check_expr(argument, ExpectedType::None);
                        self.program.copies.insert(argument.span);
                        if ty != self.typed.types.error()
                            && !crate::traits::provides(self.resolved, self.typed, ty, "Display")
                        {
                            self.diagnostics.push(
                                Diagnostic::new(
                                    Category::Call,
                                    "`print` and `println` require a `Display` value",
                                )
                                .with_primary(argument.span),
                            );
                        }
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
        if let TypeKind::Closure {
            declaration,
            parameters,
            return_type,
            ..
        } = self
            .typed
            .types
            .kind(self.typed.types.resolve_inference(callee_type))
            .clone()
        {
            self.program
                .calls
                .insert(node.span, CheckedCall::Closure { declaration });
            let parameters = parameters
                .into_iter()
                .map(|ty| FunctionParameter {
                    ty,
                    variadic: false,
                })
                .collect::<Vec<_>>();
            self.check_call_arguments(node.span, &parameters, arguments);
            return (return_type, PlaceKind::Value);
        }
        if let TypeKind::GenericParameter(parameter) = self
            .typed
            .types
            .kind(self.typed.types.resolve_inference(callee_type))
            .clone()
            && let Some((trait_declaration, parameters, return_type)) =
                self.callable_bound_signature(parameter)
        {
            self.program.calls.insert(
                node.span,
                CheckedCall::CallableBound {
                    trait_declaration,
                    receiver_type: callee_type,
                    parameters: parameters.clone(),
                },
            );
            let parameters = parameters
                .into_iter()
                .map(|ty| FunctionParameter {
                    ty,
                    variadic: false,
                })
                .collect::<Vec<_>>();
            self.check_call_arguments(node.span, &parameters, arguments);
            return (return_type, PlaceKind::Value);
        }
        if let Some((trait_declaration, parameters, return_type)) =
            self.callable_object_signature(callee_type)
        {
            let slot = crate::traits::vtable_slots(self.resolved, trait_declaration)
                .iter()
                .position(|(name, _)| name == "call")
                .unwrap_or(0);
            self.program.calls.insert(
                node.span,
                CheckedCall::CallableDynamic {
                    trait_declaration,
                    slot,
                    parameters: parameters.clone(),
                },
            );
            let parameters = parameters
                .into_iter()
                .map(|ty| FunctionParameter {
                    ty,
                    variadic: false,
                })
                .collect::<Vec<_>>();
            self.check_call_arguments(node.span, &parameters, arguments);
            return (return_type, PlaceKind::Value);
        }
        if let Some((trait_declaration, parameters, return_type)) =
            self.callable_impl_signature(callee_type)
        {
            self.program.calls.insert(
                node.span,
                CheckedCall::CallableBound {
                    trait_declaration,
                    receiver_type: callee_type,
                    parameters: parameters.clone(),
                },
            );
            let parameters = parameters
                .into_iter()
                .map(|ty| FunctionParameter {
                    ty,
                    variadic: false,
                })
                .collect::<Vec<_>>();
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

    fn callable_bound_signature(
        &self,
        parameter: GenericParameterId,
    ) -> Option<(DeclarationId, Vec<TypeId>, TypeId)> {
        self.typed
            .obligations_for(parameter)
            .find_map(|obligation| self.callable_trait_signature(obligation.trait_type))
    }

    fn callable_trait_signature(
        &self,
        trait_type: TypeId,
    ) -> Option<(DeclarationId, Vec<TypeId>, TypeId)> {
        let TypeKind::Nominal {
            identity,
            arguments,
        } = self
            .typed
            .types
            .kind(self.typed.types.resolve_inference(trait_type))
        else {
            return None;
        };
        if !self
            .resolved
            .is_standard_declaration(identity.declaration, "Callable")
        {
            return None;
        }
        let [argument_tuple, return_type] = arguments.as_slice() else {
            return None;
        };
        let argument_kind = self
            .typed
            .types
            .kind(self.typed.types.resolve_inference(*argument_tuple));
        let parameters = match argument_kind {
            TypeKind::Tuple(parameters) => parameters.clone(),
            TypeKind::Primitive(PrimitiveType::Unit) => Vec::new(),
            _ => return None,
        };
        Some((identity.declaration, parameters, *return_type))
    }

    fn callable_object_signature(
        &self,
        ty: TypeId,
    ) -> Option<(DeclarationId, Vec<TypeId>, TypeId)> {
        let TypeKind::Reference { target, .. } = self
            .typed
            .types
            .kind(self.typed.types.resolve_inference(ty))
        else {
            return None;
        };
        let TypeKind::TraitObject { trait_type } = self
            .typed
            .types
            .kind(self.typed.types.resolve_inference(*target))
        else {
            return None;
        };
        self.callable_trait_signature(*trait_type)
    }

    fn callable_impl_signature(
        &mut self,
        ty: TypeId,
    ) -> Option<(DeclarationId, Vec<TypeId>, TypeId)> {
        let trait_declaration = self.resolved.standard_declaration("Callable")?;
        let selected =
            crate::traits::vtable_entry(self.resolved, self.typed, trait_declaration, ty, "call")?;
        let instance = FunctionInstance {
            declaration: selected.declaration,
            arguments: selected.arguments,
            self_type: selected.self_type,
        };
        let signature = self.typed.instantiate_signature(self.resolved, &instance)?;
        let [argument] = signature.parameters.as_slice() else {
            return None;
        };
        let argument_kind = self
            .typed
            .types
            .kind(self.typed.types.resolve_inference(argument.ty));
        let parameters = match argument_kind {
            TypeKind::Tuple(parameters) => parameters.clone(),
            TypeKind::Primitive(PrimitiveType::Unit) => Vec::new(),
            _ => return None,
        };
        Some((trait_declaration, parameters, signature.return_type))
    }

    pub(super) fn check_member_call(
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

        // `Type.Trait.method(...)` selects that implementation's member
        // unconditionally, bypassing fields, inherent methods, and bound trait
        // lookup (`SPEC.md` 6). Like any unbound selection, a receiver-bearing
        // method takes its receiver as the first explicit argument.
        if let Some(method) = self.qualified_trait_method(base, &member_name) {
            return Some(
                self.check_function_call(call_span, method, None, arguments, expected, true),
            );
        }

        // Primitive defaults are compiler-supplied `Default` implementations,
        // just like structural defaults for derived nominal types. Recognize
        // them before the closed numeric and text associated-function
        // surfaces below.
        if member_name == "default"
            && let Some(primitive) = self.primitive_selection(base)
        {
            let ty = self.typed.types.primitive(primitive);
            self.check_standard_arguments(call_span, "default", arguments, &[]);
            self.program
                .calls
                .insert(call_span, CheckedCall::DerivedDefault { ty });
            return Some((ty, PlaceKind::Value));
        }

        // `String.from(text)` is the sole text conversion. A string literal
        // can materialize contextually as either text type, but an existing
        // `str` value never converts implicitly.
        if self.primitive_selection(base) == Some(PrimitiveType::String) {
            if member_name == "from" {
                let str_type = self.typed.types.primitive(PrimitiveType::Str);
                let string_type = self.typed.types.primitive(PrimitiveType::String);
                self.check_standard_arguments(call_span, "String.from", arguments, &[str_type]);
                self.program
                    .calls
                    .insert(call_span, CheckedCall::Standard(StandardCall::StringFrom));
                return Some((string_type, PlaceKind::Value));
            }
            for argument in arguments {
                self.check_expr(argument, ExpectedType::None);
            }
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Call,
                    format!("`String` has no associated function named `{member_name}`"),
                )
                .with_primary(member_token.span),
            );
            return Some((self.typed.types.error(), PlaceKind::Value));
        }

        if member_name == "from"
            && let Some(wrapper) = self.type_from_expression(base)
            && let TypeKind::Builtin {
                builtin,
                arguments: type_arguments,
            } = self.typed.types.kind(wrapper).clone()
            && self.resolved.builtin_name(builtin) == "Identity"
            && let [reference] = type_arguments.as_slice()
        {
            if !matches!(
                self.typed
                    .types
                    .kind(self.typed.types.resolve_inference(*reference)),
                TypeKind::Reference { .. }
            ) {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::TypeSystem,
                        "`Identity` wraps only a safe reference type",
                    )
                    .with_primary(call_span),
                );
            }
            self.check_standard_arguments(call_span, "Identity.from", arguments, &[*reference]);
            self.program.calls.insert(
                call_span,
                CheckedCall::Standard(StandardCall::IdentityFrom { wrapper }),
            );
            return Some((wrapper, PlaceKind::Value));
        }

        if member_name == "retain"
            && let Some(handle) = self.type_from_expression(base)
            && let TypeKind::Builtin {
                builtin,
                arguments: type_arguments,
            } = self.typed.types.kind(handle).clone()
            && matches!(
                self.resolved.builtin_name(builtin),
                "ForeignRoot" | "ForeignRootMut"
            )
            && let [target] = type_arguments.as_slice()
        {
            let mutable = self.resolved.builtin_name(builtin) == "ForeignRootMut";
            let reference = self.typed.types.intern(TypeKind::Reference {
                mutability: if mutable {
                    Mutability::Mutable
                } else {
                    Mutability::Shared
                },
                target: *target,
            });
            self.check_standard_arguments(
                call_span,
                &format!("{}.retain", self.resolved.builtin_name(builtin)),
                arguments,
                &[reference],
            );
            self.program.calls.insert(
                call_span,
                CheckedCall::Standard(StandardCall::ForeignRootRetain { handle, mutable }),
            );
            return Some((handle, PlaceKind::Value));
        }

        // Empty collection constructors are associated functions. Their
        // element types come from explicit generic arguments or from the
        // expected result type.
        if matches!(member_name.as_str(), "new" | "default")
            && let Some(collection) = self.standard_collection_selection(base, expected)
        {
            let TypeKind::Builtin {
                builtin,
                arguments: type_arguments,
            } = self.typed.types.kind(collection).clone()
            else {
                unreachable!("collection selection must produce a builtin type");
            };
            let operation = match self.resolved.builtin_name(builtin) {
                "Vec" if type_arguments.len() == 1 => StandardCall::VecNew { collection },
                "Map" if type_arguments.len() == 2 => {
                    if !crate::traits::provides(
                        self.resolved,
                        self.typed,
                        type_arguments[0],
                        "StableHash",
                    ) {
                        self.diagnostics.push(
                            Diagnostic::new(
                                Category::TypeSystem,
                                "`Map` keys must provide `StableHash`",
                            )
                            .with_primary(call_span),
                        );
                    }
                    StandardCall::MapNew { collection }
                }
                "Set" if type_arguments.len() == 1 => {
                    if !crate::traits::provides(
                        self.resolved,
                        self.typed,
                        type_arguments[0],
                        "StableHash",
                    ) {
                        self.diagnostics.push(
                            Diagnostic::new(
                                Category::TypeSystem,
                                "`Set` elements must provide `StableHash`",
                            )
                            .with_primary(call_span),
                        );
                    }
                    StandardCall::SetNew { collection }
                }
                _ => {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::Call,
                            "collection `new` requires complete element type arguments",
                        )
                        .with_primary(call_span),
                    );
                    return Some((self.typed.types.error(), PlaceKind::Value));
                }
            };
            self.check_standard_arguments(call_span, &member_name, arguments, &[]);
            self.program
                .calls
                .insert(call_span, CheckedCall::Standard(operation));
            return Some((collection, PlaceKind::Value));
        }

        // `Type.try_from(value)` is a standard associated function on a
        // concrete numeric type (`SPEC.md` 4.1). A primitive has no
        // declaration to select a member from, so it is recognized here, and
        // an unrecognized member on a primitive is reported here too rather
        // than falling through to nominal selection, which cannot see it.
        // Only the numeric associated-function surface is complete, so only a
        // numeric primitive rejects an unrecognized member here. `str` and
        // `String` gain theirs in Milestones 14.5 and 14.6.
        if let Some(primitive) = self
            .primitive_selection(base)
            .filter(|primitive| primitive.is_integer() || primitive.is_float())
        {
            if let Some(outcome) = match member_name.as_str() {
                "try_from" => Some(NumericOutcome::Checked),
                "wrapping_from" => Some(NumericOutcome::Wrapping),
                "saturating_from" => Some(NumericOutcome::Saturating),
                _ => None,
            } {
                return Some(self.check_numeric_conversion(
                    call_span,
                    primitive,
                    outcome,
                    &member_name,
                    arguments,
                ));
            }
            for argument in arguments {
                self.check_expr(argument, ExpectedType::None);
            }
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Call,
                    format!(
                        "`{}` has no associated function named `{member_name}`",
                        types::primitive_name(primitive)
                    ),
                )
                .with_primary(member_token.span),
            );
            return Some((self.typed.types.error(), PlaceKind::Value));
        }

        // `Type.member(...)` is an ordinary call of the selected associated
        // function or unbound method. A receiver-bearing method therefore
        // expects its receiver as the first explicit argument.
        if let Some((owner, owner_arguments)) = self.nominal_selection(base) {
            // `Type.default()` comes from a derivation, so there is no member
            // declaration to select (`SPEC.md` 4.3).
            if member_name == "default"
                && self.find_member(owner, &member_name).is_none()
                && crate::traits::derives_or_intrinsic(self.resolved, owner, "Default")
            {
                for argument in arguments {
                    self.check_expr(argument, ExpectedType::None);
                }
                if !arguments.is_empty() {
                    self.diagnostics.push(
                        Diagnostic::new(Category::Call, "`default` takes no arguments")
                            .with_primary(call_span),
                    );
                    return Some((self.typed.types.error(), PlaceKind::Value));
                }
                let Some((parameters, variables)) = self.initial_nominal_inference(
                    call_span,
                    owner,
                    owner_arguments.clone(),
                    expected,
                ) else {
                    return Some((self.typed.types.error(), PlaceKind::Value));
                };
                let Some(inferred) =
                    self.finish_nominal_inference(call_span, &parameters, &variables)
                else {
                    return Some((self.typed.types.error(), PlaceKind::Value));
                };
                let ty = self
                    .typed
                    .instantiate_declaration_type(self.resolved, owner, &inferred)
                    .unwrap_or_else(|| self.typed.types.error());
                if !crate::traits::provides(self.resolved, self.typed, ty, "Default") {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::TypeSystem,
                            "this instantiation does not implement `Default` because a field \
                             type does not provide it",
                        )
                        .with_primary(call_span),
                    );
                    return Some((self.typed.types.error(), PlaceKind::Value));
                }
                self.program
                    .calls
                    .insert(call_span, CheckedCall::DerivedDefault { ty });
                return Some((ty, PlaceKind::Value));
            }
            let method = match self.find_member(owner, &member_name) {
                Some(MemberId::Method(method)) => method,
                Some(_) => {
                    for argument in arguments {
                        self.check_expr(argument, ExpectedType::None);
                    }
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::Call,
                            format!("this type has no associated function named `{member_name}`"),
                        )
                        .with_primary(member_token.span),
                    );
                    return Some((self.typed.types.error(), PlaceKind::Value));
                }
                None => {
                    let target = owner_arguments
                        .as_ref()
                        .and_then(|arguments| {
                            self.typed
                                .instantiate_declaration_type(self.resolved, owner, arguments)
                        })
                        .or_else(|| self.typed.declaration_types.get(&owner).copied())
                        .unwrap_or_else(|| self.typed.types.error());
                    match self.select_trait_method(target, &member_name, member_token.span) {
                        TraitSelection::Found(selected) => {
                            self.pending_self_type = selected.self_type;
                            selected.declaration
                        }
                        TraitSelection::Ambiguous => {
                            for argument in arguments {
                                self.check_expr(argument, ExpectedType::None);
                            }
                            return Some((self.typed.types.error(), PlaceKind::Value));
                        }
                        TraitSelection::None => {
                            for argument in arguments {
                                self.check_expr(argument, ExpectedType::None);
                            }
                            self.diagnostics.push(
                                Diagnostic::new(
                                    Category::Call,
                                    format!(
                                        "this type has no associated function named \
                                         `{member_name}`"
                                    ),
                                )
                                .with_primary(member_token.span),
                            );
                            return Some((self.typed.types.error(), PlaceKind::Value));
                        }
                    }
                }
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
        if let Some((operation, parameters, result, mutable)) =
            self.standard_receiver_call(base_type, &member_name, call_span)
        {
            if mutable && !base_place.is_mutable() {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Place,
                        format!("`{member_name}` requires a mutable collection receiver"),
                    )
                    .with_primary(base.span),
                );
            }
            self.check_standard_arguments(call_span, &member_name, arguments, &parameters);
            self.program
                .calls
                .insert(call_span, CheckedCall::Standard(operation));
            return Some((result, PlaceKind::Value));
        }
        // A call on a trait object dispatches through its vtable rather than
        // selecting a concrete implementation (`SPEC.md` 6).
        if let Some(trait_declaration) =
            crate::traits::object_trait(self.resolved, self.typed, base_type)
        {
            return Some(self.check_dynamic_call(
                call_span,
                callee,
                trait_declaration,
                &member_name,
                member_token.span,
                arguments,
            ));
        }
        // Inside a trait's default body the receiver is `&Self`, whose methods
        // are the trait's own.
        if let Some(trait_declaration) = self.self_receiver_trait(base_type) {
            return Some(self.check_trait_self_call(
                call_span,
                callee,
                trait_declaration,
                &member_name,
                member_token.span,
                arguments,
            ));
        }
        // `value.checked_add(other)` and friends are standard methods on the
        // integer types (`SPEC.md` 4.1). An integer has no declaration, so
        // they are recognized here rather than through member lookup.
        if let Some(operation) = self
            .integer_receiver(base_type)
            .and_then(|_| NumericAlternative::from_name(&member_name))
        {
            return Some(self.check_numeric_alternative(
                call_span,
                base_type,
                operation,
                arguments,
                member_token.span,
            ));
        }
        if let Some(generic_target) = self.generic_receiver_target(base_type) {
            let candidates = self
                .typed
                .obligations_for(generic_target.0)
                .filter_map(|obligation| {
                    let TypeKind::Nominal {
                        identity,
                        arguments,
                    } = self
                        .typed
                        .types
                        .kind(self.typed.types.resolve_inference(obligation.trait_type))
                    else {
                        return None;
                    };
                    let trait_declaration = identity.declaration;
                    if self.resolved.declarations[trait_declaration.index()].kind
                        != DeclarationKind::Trait
                        || self.current_module.is_some_and(|module| {
                            !self
                                .resolved
                                .declaration_in_scope(module, trait_declaration)
                        })
                    {
                        return None;
                    }
                    let method =
                        self.find_member(trait_declaration, &member_name)
                            .and_then(|member| match member {
                                MemberId::Method(method) => Some(method),
                                MemberId::Field(_) | MemberId::Variant(_) => None,
                            })?;
                    Some((
                        trait_declaration,
                        method,
                        arguments.clone(),
                        crate::traits::name_of(self.resolved, trait_declaration),
                    ))
                })
                .collect::<Vec<_>>();
            if candidates.len() > 1 {
                let names = candidates
                    .iter()
                    .map(|(_, _, _, name)| format!("`{name}`"))
                    .collect::<Vec<_>>()
                    .join(" and ");
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Call,
                        format!(
                            "method `{member_name}` is provided by more than one bound \
                             ({names}); select the intended trait explicitly"
                        ),
                    )
                    .with_primary(member_token.span),
                );
                for argument in arguments {
                    self.check_expr(argument, ExpectedType::None);
                }
                return Some((self.typed.types.error(), PlaceKind::Value));
            }
            if let Some((trait_declaration, method, trait_arguments, _)) =
                candidates.into_iter().next()
            {
                let instance = FunctionInstance {
                    declaration: method,
                    arguments: trait_arguments,
                    self_type: Some(generic_target.1),
                };
                let Some(signature) = self.typed.instantiate_signature(self.resolved, &instance)
                else {
                    return Some((self.typed.types.error(), PlaceKind::Value));
                };
                let Some(receiver_type) = signature.receiver else {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::Call,
                            format!(
                                "associated function `{member_name}` must be selected from its \
                                 trait"
                            ),
                        )
                        .with_primary(member_token.span),
                    );
                    for argument in arguments {
                        self.check_expr(argument, ExpectedType::None);
                    }
                    return Some((self.typed.types.error(), PlaceKind::Value));
                };
                let Some(adjustment) =
                    self.check_receiver_adjustment(base, base_type, base_place, receiver_type)
                else {
                    for argument in arguments {
                        self.check_expr(argument, ExpectedType::None);
                    }
                    return Some((self.typed.types.error(), PlaceKind::Value));
                };
                self.check_call_arguments(call_span, &signature.parameters, arguments);
                self.program.calls.insert(
                    call_span,
                    CheckedCall::GenericBoundMethod {
                        trait_declaration,
                        method,
                        receiver_type: generic_target.1,
                        adjustment,
                    },
                );
                return Some((signature.return_type, PlaceKind::Value));
            }
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Call,
                    format!(
                        "no method named `{member_name}` is provided by this generic parameter's \
                         bounds"
                    ),
                )
                .with_primary(member_token.span),
            );
            for argument in arguments {
                self.check_expr(argument, ExpectedType::None);
            }
            return Some((self.typed.types.error(), PlaceKind::Value));
        }
        let (owner, self_type) = self.receiver_owner(base_type)?;
        // Fields and inherent methods are found first; a trait method is
        // selected only when the type itself provides no member of that name
        // (`SPEC.md` 6).
        let member = match self.find_member(owner, &member_name) {
            Some(member) => Some(member),
            None => match self.select_trait_method(self_type, &member_name, member_token.span) {
                TraitSelection::Found(selected) => {
                    self.pending_self_type = selected.self_type;
                    Some(MemberId::Method(selected.declaration))
                }
                TraitSelection::Ambiguous => {
                    for argument in arguments {
                        self.check_expr(argument, ExpectedType::None);
                    }
                    return Some((self.typed.types.error(), PlaceKind::Value));
                }
                TraitSelection::None => None,
            },
        };
        let result = match member {
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
                    // A trait default body is specialized to the receiver.
                    self_type: self.pending_self_type,
                };
                if !self.validate_instance_obligations(&instance, call_span) {
                    return Some((self.typed.types.error(), PlaceKind::Value));
                }
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
        };
        self.pending_self_type = None;
        if result.is_none() {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Call,
                    format!("no method named `{member_name}` is available for this type"),
                )
                .with_primary(member_token.span),
            );
            for argument in arguments {
                self.check_expr(argument, ExpectedType::None);
            }
            return Some((self.typed.types.error(), PlaceKind::Value));
        }
        result
    }

    pub(super) fn check_standard_arguments(
        &mut self,
        call_span: Span,
        name: &str,
        arguments: &[&SyntaxNode],
        parameters: &[TypeId],
    ) {
        for (index, argument) in arguments.iter().enumerate() {
            let expected = parameters
                .get(index)
                .copied()
                .map_or(ExpectedType::None, ExpectedType::Exact);
            self.check_expr(argument, expected);
            self.program.copies.insert(argument.span);
        }
        if arguments.len() != parameters.len() {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Call,
                    format!(
                        "`{name}` expects exactly {} argument{}, but {} were supplied",
                        parameters.len(),
                        if parameters.len() == 1 { "" } else { "s" },
                        arguments.len()
                    ),
                )
                .with_primary(call_span),
            );
        }
    }

    pub(super) fn standard_collection_selection(
        &mut self,
        base: &SyntaxNode,
        expected: ExpectedType,
    ) -> Option<TypeId> {
        let selected = self.type_from_expression(base)?;
        let TypeKind::Builtin { builtin, arguments } = self.typed.types.kind(selected).clone()
        else {
            return None;
        };
        if !matches!(self.resolved.builtin_name(builtin), "Vec" | "Map" | "Set") {
            return None;
        }
        let arity = if self.resolved.builtin_name(builtin) == "Map" {
            2
        } else {
            1
        };
        if arguments.len() == arity
            && arguments
                .iter()
                .all(|argument| *argument != self.typed.types.error())
        {
            return Some(selected);
        }
        let ExpectedType::Exact(expected) = expected else {
            return Some(selected);
        };
        let expected = self.typed.types.resolve_inference(expected);
        match self.typed.types.kind(expected) {
            TypeKind::Builtin {
                builtin: expected_builtin,
                arguments: expected_arguments,
            } if *expected_builtin == builtin && expected_arguments.len() == arity => {
                Some(expected)
            }
            _ => Some(selected),
        }
    }

    pub(super) fn standard_receiver_call(
        &mut self,
        receiver: TypeId,
        name: &str,
        span: Span,
    ) -> Option<(StandardCall, Vec<TypeId>, TypeId, bool)> {
        let receiver = self.typed.types.resolve_inference(receiver);
        let usize_type = self.typed.types.primitive(PrimitiveType::Usize);
        let bool_type = self.typed.types.primitive(PrimitiveType::Bool);
        let unit_type = self.typed.types.primitive(PrimitiveType::Unit);
        if let TypeKind::Reference {
            mutability: Mutability::Mutable,
            target,
        } = self.typed.types.kind(receiver).clone()
            && let TypeKind::Builtin { builtin, arguments } = self
                .typed
                .types
                .kind(self.typed.types.resolve_inference(target))
            && self.resolved.builtin_name(*builtin) == "Formatter"
            && arguments.is_empty()
            && name == "write"
        {
            return Some((
                StandardCall::FormatterWrite { formatter: target },
                vec![self.typed.types.primitive(PrimitiveType::Str)],
                unit_type,
                false,
            ));
        }
        match self.typed.types.kind(receiver).clone() {
            TypeKind::Array { element, .. } => match name {
                "len" => Some((
                    StandardCall::ArrayLen {
                        collection: receiver,
                    },
                    Vec::new(),
                    usize_type,
                    false,
                )),
                "get" => Some((
                    StandardCall::ArrayGet {
                        collection: receiver,
                    },
                    vec![usize_type],
                    self.option_type(span, element),
                    false,
                )),
                _ => None,
            },
            TypeKind::Builtin { builtin, arguments } => {
                match (
                    self.resolved.builtin_name(builtin),
                    name,
                    arguments.as_slice(),
                ) {
                    ("ForeignRoot", "pointer", [target]) => {
                        let pointer = self.typed.types.intern(TypeKind::RawPointer {
                            mutability: Mutability::Shared,
                            target: *target,
                        });
                        Some((
                            StandardCall::ForeignRootPointer {
                                handle: receiver,
                                mutable: false,
                            },
                            Vec::new(),
                            pointer,
                            false,
                        ))
                    }
                    ("ForeignRootMut", "pointer", [target]) => {
                        let pointer = self.typed.types.intern(TypeKind::RawPointer {
                            mutability: Mutability::Mutable,
                            target: *target,
                        });
                        Some((
                            StandardCall::ForeignRootPointer {
                                handle: receiver,
                                mutable: true,
                            },
                            Vec::new(),
                            pointer,
                            false,
                        ))
                    }
                    ("ForeignRoot" | "ForeignRootMut", "close", [_]) => Some((
                        StandardCall::ForeignRootClose { handle: receiver },
                        Vec::new(),
                        unit_type,
                        false,
                    )),
                    ("Vec", "len", [_]) => Some((
                        StandardCall::VecLen {
                            collection: receiver,
                        },
                        Vec::new(),
                        usize_type,
                        false,
                    )),
                    ("Vec", "is_empty", [_]) => Some((
                        StandardCall::VecIsEmpty {
                            collection: receiver,
                        },
                        Vec::new(),
                        bool_type,
                        false,
                    )),
                    ("Vec", "get", [element]) => Some((
                        StandardCall::VecGet {
                            collection: receiver,
                        },
                        vec![usize_type],
                        self.option_type(span, *element),
                        false,
                    )),
                    ("Vec", "append", [element]) => Some((
                        StandardCall::VecAppend {
                            collection: receiver,
                        },
                        vec![*element],
                        unit_type,
                        true,
                    )),
                    ("Vec", "insert", [element]) => Some((
                        StandardCall::VecInsert {
                            collection: receiver,
                        },
                        vec![usize_type, *element],
                        unit_type,
                        true,
                    )),
                    ("Vec", "remove", [element]) => Some((
                        StandardCall::VecRemove {
                            collection: receiver,
                        },
                        vec![usize_type],
                        *element,
                        true,
                    )),
                    ("Vec", "clear", [_]) => Some((
                        StandardCall::VecClear {
                            collection: receiver,
                        },
                        Vec::new(),
                        unit_type,
                        true,
                    )),
                    ("Map", "len", [_, _]) => Some((
                        StandardCall::MapLen {
                            collection: receiver,
                        },
                        Vec::new(),
                        usize_type,
                        false,
                    )),
                    ("Map", "is_empty", [_, _]) => Some((
                        StandardCall::MapIsEmpty {
                            collection: receiver,
                        },
                        Vec::new(),
                        bool_type,
                        false,
                    )),
                    ("Map", "contains_key", [key, _]) => Some((
                        StandardCall::MapContainsKey {
                            collection: receiver,
                        },
                        vec![*key],
                        bool_type,
                        false,
                    )),
                    ("Map", "get", [key, value]) => Some((
                        StandardCall::MapGet {
                            collection: receiver,
                        },
                        vec![*key],
                        self.option_type(span, *value),
                        false,
                    )),
                    ("Map", "insert", [key, value]) => Some((
                        StandardCall::MapInsert {
                            collection: receiver,
                        },
                        vec![*key, *value],
                        self.option_type(span, *value),
                        true,
                    )),
                    ("Map", "remove", [key, value]) => Some((
                        StandardCall::MapRemove {
                            collection: receiver,
                        },
                        vec![*key],
                        self.option_type(span, *value),
                        true,
                    )),
                    ("Map", "clear", [_, _]) => Some((
                        StandardCall::MapClear {
                            collection: receiver,
                        },
                        Vec::new(),
                        unit_type,
                        true,
                    )),
                    ("Set", "len", [_]) => Some((
                        StandardCall::SetLen {
                            collection: receiver,
                        },
                        Vec::new(),
                        usize_type,
                        false,
                    )),
                    ("Set", "is_empty", [_]) => Some((
                        StandardCall::SetIsEmpty {
                            collection: receiver,
                        },
                        Vec::new(),
                        bool_type,
                        false,
                    )),
                    ("Set", "contains", [element]) => Some((
                        StandardCall::SetContains {
                            collection: receiver,
                        },
                        vec![*element],
                        bool_type,
                        false,
                    )),
                    ("Set", "insert", [element]) => Some((
                        StandardCall::SetInsert {
                            collection: receiver,
                        },
                        vec![*element],
                        bool_type,
                        true,
                    )),
                    ("Set", "remove", [element]) => Some((
                        StandardCall::SetRemove {
                            collection: receiver,
                        },
                        vec![*element],
                        bool_type,
                        true,
                    )),
                    ("Set", "clear", [_]) => Some((
                        StandardCall::SetClear {
                            collection: receiver,
                        },
                        Vec::new(),
                        unit_type,
                        true,
                    )),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Selects a trait method for a receiver, reporting ambiguity when more
    /// than one in-scope trait supplies the name.
    pub(super) fn select_trait_method(
        &mut self,
        self_type: TypeId,
        name: &str,
        span: Span,
    ) -> TraitSelection {
        match crate::traits::select_trait_method(
            self.resolved,
            self.typed,
            self_type,
            name,
            self.current_module,
        ) {
            Ok(Some(selected)) => TraitSelection::Found(selected),
            Ok(None) => TraitSelection::None,
            Err(traits) => {
                let names = traits
                    .iter()
                    .map(|trait_name| format!("`{trait_name}`"))
                    .collect::<Vec<_>>()
                    .join(" and ");
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Call,
                        format!(
                            "method `{name}` is provided by more than one trait ({names});                              select it explicitly with `Type.Trait.{name}`"
                        ),
                    )
                    .with_primary(span),
                );
                TraitSelection::Ambiguous
            }
        }
    }

    pub(super) fn receiver_owner(&self, ty: TypeId) -> Option<(DeclarationId, TypeId)> {
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

    pub(super) fn generic_receiver_target(
        &self,
        ty: TypeId,
    ) -> Option<(GenericParameterId, TypeId)> {
        let ty = self.typed.types.resolve_inference(ty);
        match self.typed.types.kind(ty) {
            TypeKind::GenericParameter(parameter) => Some((*parameter, ty)),
            TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => {
                self.generic_receiver_target(*target)
            }
            TypeKind::Alias { target, .. } => self.generic_receiver_target(*target),
            _ => None,
        }
    }

    pub(super) fn check_receiver_adjustment(
        &mut self,
        base: &SyntaxNode,
        actual: TypeId,
        place: PlaceKind,
        expected: TypeId,
    ) -> Option<ReceiverAdjustment> {
        let actual = self.typed.types.resolve_inference(actual);
        let expected = self.typed.types.resolve_inference(expected);
        let self_type = match self.typed.types.kind(expected).clone() {
            TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => target,
            TypeKind::Alias { target, .. } => {
                return self.check_receiver_adjustment(base, actual, place, target);
            }
            _ => expected,
        };
        let adjustment = match self.typed.types.kind(expected).clone() {
            TypeKind::Nominal { .. } | TypeKind::GenericParameter(_) => {
                match self.typed.types.kind(actual).clone() {
                    _ if self.typed.types.exactly_equal(actual, self_type) => {
                        Some(ReceiverAdjustment::CopyValue)
                    }
                    TypeKind::Reference { target, .. }
                        if self.typed.types.exactly_equal(target, self_type) =>
                    {
                        Some(ReceiverAdjustment::DereferenceAndCopy)
                    }
                    _ => None,
                }
            }
            TypeKind::Reference { mutability, target } => {
                match self.typed.types.kind(actual).clone() {
                    _ if self.typed.types.exactly_equal(actual, target) => {
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

    pub(super) fn function_value_signature(
        &self,
        ty: TypeId,
    ) -> Option<(Vec<FunctionParameter>, TypeId)> {
        let ty = self.typed.types.resolve_inference(ty);
        let target = match self.typed.types.kind(ty) {
            TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => {
                self.typed.types.resolve_inference(*target)
            }
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

    pub(super) fn check_call_arguments(
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

    pub(super) fn check_variant_tuple_construction(
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

    /// The trait whose `Self` a receiver names, if any.
    pub(super) fn self_receiver_trait(&self, ty: TypeId) -> Option<DeclarationId> {
        let ty = self.typed.types.resolve_inference(ty);
        let target = match self.typed.types.kind(ty) {
            TypeKind::Reference { target, .. } => self.typed.types.resolve_inference(*target),
            _ => ty,
        };
        match self.typed.types.kind(target) {
            TypeKind::SelfType(declaration)
                if self.resolved.declarations[declaration.index()].kind
                    == DeclarationKind::Trait =>
            {
                Some(*declaration)
            }
            _ => None,
        }
    }

    /// Checks a `self.method(...)` call inside a trait's default body.
    pub(super) fn check_trait_self_call(
        &mut self,
        call_span: Span,
        callee: &SyntaxNode,
        trait_declaration: DeclarationId,
        member_name: &str,
        member_span: Span,
        arguments: &[&SyntaxNode],
    ) -> (TypeId, PlaceKind) {
        let slots = crate::traits::vtable_slots(self.resolved, trait_declaration);
        let Some(method) = slots
            .iter()
            .find(|(name, _)| name == member_name)
            .map(|(_, method)| *method)
        else {
            for argument in arguments {
                self.check_expr(argument, ExpectedType::None);
            }
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Call,
                    format!(
                        "trait `{}` has no method named `{member_name}`",
                        crate::traits::name_of(self.resolved, trait_declaration)
                    ),
                )
                .with_primary(member_span),
            );
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let Some(signature) = self.typed.function_signatures.get(&method).cloned() else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        self.program
            .expression_types
            .insert(callee.span, signature.return_type);
        self.check_call_arguments(call_span, &signature.parameters, arguments);
        self.program.calls.insert(
            call_span,
            CheckedCall::TraitSelfMethod {
                trait_declaration,
                method,
            },
        );
        (signature.return_type, PlaceKind::Value)
    }

    /// Checks a call through a trait object, recording its vtable slot.
    pub(super) fn check_dynamic_call(
        &mut self,
        call_span: Span,
        callee: &SyntaxNode,
        trait_declaration: DeclarationId,
        member_name: &str,
        member_span: Span,
        arguments: &[&SyntaxNode],
    ) -> (TypeId, PlaceKind) {
        let slots = crate::traits::vtable_slots(self.resolved, trait_declaration);
        let Some(slot) = slots.iter().position(|(name, _)| name == member_name) else {
            for argument in arguments {
                self.check_expr(argument, ExpectedType::None);
            }
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Call,
                    format!(
                        "trait `{}` has no method named `{member_name}`",
                        crate::traits::name_of(self.resolved, trait_declaration)
                    ),
                )
                .with_primary(member_span),
            );
            return (self.typed.types.error(), PlaceKind::Value);
        };
        let method = slots[slot].1;
        let Some(signature) = self.typed.function_signatures.get(&method).cloned() else {
            return (self.typed.types.error(), PlaceKind::Value);
        };
        self.program
            .expression_types
            .insert(callee.span, signature.return_type);
        self.check_call_arguments(call_span, &signature.parameters, arguments);
        self.program.calls.insert(
            call_span,
            CheckedCall::DynamicMethod {
                trait_declaration,
                method,
                slot,
            },
        );
        (signature.return_type, PlaceKind::Value)
    }

    /// Resolves `Type.Trait` to the implementation's method named `name`.
    ///
    /// Returns `None` unless `base` is exactly a `Type.Trait` path with an
    /// implementation of that trait for that type.
    pub(super) fn qualified_trait_method(
        &mut self,
        base: &SyntaxNode,
        name: &str,
    ) -> Option<DeclarationId> {
        if base.kind != SyntaxKind::MemberExpression {
            return None;
        }
        let type_node = child_nodes(base).into_iter().next()?;
        let (owner, arguments) = self.nominal_selection(type_node)?;
        let target = match arguments {
            Some(arguments) => {
                self.typed
                    .instantiate_declaration_type(self.resolved, owner, &arguments)?
            }
            None => *self.typed.declaration_types.get(&owner)?,
        };
        let trait_token = base.children.iter().rev().find_map(|child| match child {
            SyntaxElement::Token(token) if matches!(token.kind, TokenKind::Identifier(_)) => {
                Some(token)
            }
            _ => None,
        })?;
        let trait_name = token_text(trait_token);
        crate::traits::qualified_trait_method(self.resolved, self.typed, target, &trait_name, name)
    }

    /// The primitive `node` names as a type, if any.
    ///
    /// `SPEC.md` 4.1 puts `try_from` (and, from Milestone 14.4, the wrapping
    /// and saturating conversions) on the numeric types themselves. Those have
    /// no declaration, so they are not reachable through
    /// [`Self::nominal_selection`].
    pub(super) fn primitive_selection(&mut self, node: &SyntaxNode) -> Option<PrimitiveType> {
        let span = Self::callee_target_span(node)?;
        let NameTarget::Item(crate::resolution::ItemId::Builtin(builtin)) =
            self.resolved.reference_at(span)?.target
        else {
            return None;
        };
        types::primitive_from_name(self.resolved.builtin_name(builtin))
    }

    /// The integer primitive `ty` is, after alias and inference resolution.
    pub(super) fn integer_receiver(&mut self, ty: TypeId) -> Option<PrimitiveType> {
        self.typed
            .types
            .expanded_primitive(ty)
            .filter(|primitive| primitive.is_integer())
    }

    /// Checks one standard arithmetic alternative (`SPEC.md` 4.1).
    ///
    /// These share the trapping operators' operand rules — the same operand
    /// type for a binary operation, an integer shift amount for a shift — so
    /// only the *result* differs: a checked operation reports failure as
    /// `Option[T]`, while a wrapping or saturating operation always produces a
    /// `T`.
    pub(super) fn check_numeric_alternative(
        &mut self,
        call_span: Span,
        receiver: TypeId,
        operation: NumericAlternative,
        arguments: &[&SyntaxNode],
        member_span: Span,
    ) -> (TypeId, PlaceKind) {
        let result = match operation.outcome {
            NumericOutcome::Checked => self.option_type(call_span, receiver),
            NumericOutcome::Wrapping | NumericOutcome::Saturating => receiver,
        };
        let expected_arity = usize::from(operation.operator != NumericOperator::Negate);
        // Every operand, including a shift amount, has the receiver's exact
        // type — the same rule the trapping operator applies.
        for argument in arguments {
            let (ty, _) = self.check_expr(argument, ExpectedType::Exact(receiver));
            self.program.copies.insert(argument.span);
            if ty != self.typed.types.error() && !self.typed.types.exactly_equal(ty, receiver) {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::Call,
                        "this operand must have the receiver's exact type",
                    )
                    .with_primary(argument.span),
                );
            }
        }
        if arguments.len() != expected_arity {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Call,
                    format!(
                        "`{}` expects exactly {expected_arity} argument{}, but {} were supplied",
                        operation.name,
                        if expected_arity == 1 { "" } else { "s" },
                        arguments.len()
                    ),
                )
                .with_primary(member_span),
            );
            return (result, PlaceKind::Value);
        }
        self.program.calls.insert(
            call_span,
            CheckedCall::NumericAlternative {
                operation,
                operand_type: self.typed.types.resolve_inference(receiver),
                result,
            },
        );
        (result, PlaceKind::Value)
    }

    /// `Option[payload]`, or the error type when the standard declaration is
    /// unavailable.
    pub(super) fn option_type(&mut self, span: Span, payload: TypeId) -> TypeId {
        let Some(option) = self.resolved.standard_declaration("Option") else {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::TypeSystem,
                    "the standard `Option` declaration is unavailable",
                )
                .with_primary(span),
            );
            return self.typed.types.error();
        };
        self.typed
            .instantiate_declaration_type(self.resolved, option, &[payload])
            .unwrap_or_else(|| self.typed.types.error())
    }

    /// Checks `Target.try_from(value)`: a nontrapping numeric conversion whose
    /// result is `Result[Target, NumericError]` (`SPEC.md` 4.1). The source
    /// must already be a concrete numeric type — `try_from` is a conversion,
    /// not a way to avoid literal materialization — so the argument is checked
    /// against no expected type and then required to be numeric.
    pub(super) fn check_numeric_conversion(
        &mut self,
        call_span: Span,
        target: PrimitiveType,
        outcome: NumericOutcome,
        member_name: &str,
        arguments: &[&SyntaxNode],
    ) -> (TypeId, PlaceKind) {
        let target_type = self.typed.types.primitive(target);
        let result_type = match outcome {
            NumericOutcome::Checked => self.numeric_conversion_result_type(call_span, target_type),
            NumericOutcome::Wrapping | NumericOutcome::Saturating => target_type,
        };
        let mut source_type = self.typed.types.error();
        for (index, argument) in arguments.iter().enumerate() {
            let (ty, _) = self.check_expr(argument, ExpectedType::None);
            self.program.copies.insert(argument.span);
            if index == 0 {
                source_type = ty;
            }
        }
        if arguments.len() != 1 {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Call,
                    format!(
                        "`{member_name}` expects exactly 1 argument, but {} were supplied",
                        arguments.len()
                    ),
                )
                .with_primary(call_span),
            );
            return (result_type, PlaceKind::Value);
        }
        if source_type == self.typed.types.error() {
            return (result_type, PlaceKind::Value);
        }
        if !self.is_numeric_type(source_type) {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Call,
                    format!("`{member_name}` converts only between numeric types"),
                )
                .with_primary(arguments[0].span),
            );
            return (result_type, PlaceKind::Value);
        }
        // Wrapping and saturating conversions are defined by the target's
        // integer range, so both ends must be integers (`SPEC.md` 4.1 offers
        // them only "where those behaviors are meaningful"). A checked
        // conversion has an answer for every numeric pair.
        if outcome != NumericOutcome::Checked
            && !(target.is_integer() && self.integer_receiver(source_type).is_some())
        {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::Call,
                    format!("`{member_name}` converts only between integer types"),
                )
                .with_primary(call_span),
            );
            return (result_type, PlaceKind::Value);
        }
        self.program.calls.insert(
            call_span,
            CheckedCall::NumericConversion {
                outcome,
                source: self.typed.types.resolve_inference(source_type),
                target: target_type,
                result: result_type,
            },
        );
        (result_type, PlaceKind::Value)
    }

    /// `Result[target, NumericError]`, or the error type when the standard
    /// declarations that name it are unavailable.
    pub(super) fn numeric_conversion_result_type(&mut self, span: Span, target: TypeId) -> TypeId {
        let (Some(result), Some(error)) = (
            self.resolved.standard_declaration("Result"),
            self.resolved.standard_declaration("NumericError"),
        ) else {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::TypeSystem,
                    "the standard `Result` and `NumericError` declarations are unavailable",
                )
                .with_primary(span),
            );
            return self.typed.types.error();
        };
        let Some(error_type) = self
            .typed
            .instantiate_declaration_type(self.resolved, error, &[])
        else {
            return self.typed.types.error();
        };
        self.typed
            .instantiate_declaration_type(self.resolved, result, &[target, error_type])
            .unwrap_or_else(|| self.typed.types.error())
    }
}
