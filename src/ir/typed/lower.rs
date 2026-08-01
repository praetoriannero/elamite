//! Lowering from checked syntax into typed high-level IR.

use super::super::*;
use crate::check::CheckedClosureCapture;
use crate::resolution::ClosureCaptureKind;

/// Monomorphizes and lowers checked syntax into typed high-level IR.
#[must_use]
pub fn lower_typed_ir(
    resolved: &ResolvedProgram,
    typed: &mut TypedProgram,
    checked: &CheckedProgram,
) -> TypedIrOutput {
    TypedLowerer::new(resolved, typed, checked).run()
}

#[derive(Clone)]
struct PendingInstance {
    instance: FunctionInstance,
    ancestry: Vec<FunctionInstance>,
}

struct TypedLowerer<'a> {
    vtables: Vec<Vtable>,
    resolved: &'a ResolvedProgram,
    typed: &'a mut TypedProgram,
    checked: &'a CheckedProgram,
    binding_by_span: BTreeMap<Span, LocalBindingId>,
    diagnostics: Vec<Diagnostic>,
    substitution: Substitution,
    current_instance: Option<FunctionInstance>,
    ancestry: Vec<FunctionInstance>,
    pending: VecDeque<PendingInstance>,
    queued: BTreeSet<FunctionInstance>,
}

impl<'a> TypedLowerer<'a> {
    fn new(
        resolved: &'a ResolvedProgram,
        typed: &'a mut TypedProgram,
        checked: &'a CheckedProgram,
    ) -> Self {
        let binding_by_span = resolved
            .local_bindings
            .iter()
            .map(|binding| (binding.span, binding.id))
            .collect();
        Self {
            vtables: Vec::new(),
            resolved,
            typed,
            checked,
            binding_by_span,
            diagnostics: Vec::new(),
            substitution: Substitution::new(),
            current_instance: None,
            ancestry: Vec::new(),
            pending: VecDeque::new(),
            queued: BTreeSet::new(),
        }
    }

    fn run(mut self) -> TypedIrOutput {
        let mut program = TypedIrProgram::default();
        let roots = self
            .resolved
            .declarations
            .iter()
            .filter(|declaration| {
                (declaration.kind == DeclarationKind::Function
                    || (declaration.kind == DeclarationKind::Test && declaration.test_selected))
                    && declaration.parent_impl.is_none()
                    && !["panic", "trap", "assert", "fail"]
                        .into_iter()
                        .any(|name| self.resolved.is_standard_declaration(declaration.id, name))
                    && self
                        .typed
                        .callable_generic_parameters(self.resolved, declaration.id)
                        .is_empty()
                    && !declaration.parent_declaration.is_some_and(|parent| {
                        self.resolved.declarations[parent.index()].kind == DeclarationKind::Trait
                    })
            })
            .map(|declaration| FunctionInstance {
                declaration: declaration.id,
                arguments: Vec::new(),
                self_type: None,
            })
            .collect::<Vec<_>>();
        for instance in roots {
            let span = self.resolved.declarations[instance.declaration.index()].span;
            self.enqueue(instance, Vec::new(), span);
        }
        while let Some(pending) = self.pending.pop_front() {
            self.current_instance = Some(pending.instance.clone());
            self.ancestry = pending.ancestry;
            self.substitution = self
                .typed
                .instance_substitution(self.resolved, &pending.instance);
            if let Some(function) = self.lower_function(&pending.instance) {
                program.functions.push(function);
            }
        }
        self.collect_concrete_nominals(&mut program);
        program.vtables = std::mem::take(&mut self.vtables);
        TypedIrOutput {
            program,
            diagnostics: self.diagnostics,
        }
    }

    fn concrete_type(&mut self, ty: TypeId) -> TypeId {
        let ty = self.typed.types.substitute(ty, &self.substitution);
        // A trait default body is written against `Self`; specializing it for
        // an implementing type rewrites `Self` the same way generic arguments
        // rewrite type parameters.
        match self.current_instance.as_ref().and_then(|i| i.self_type) {
            Some(self_type) => self.typed.types.substitute_self(ty, self_type),
            None => ty,
        }
    }

    fn concrete_instance(&mut self, instance: &FunctionInstance) -> FunctionInstance {
        FunctionInstance {
            declaration: instance.declaration,
            arguments: instance
                .arguments
                .iter()
                .map(|argument| self.concrete_type(*argument))
                .collect(),
            self_type: instance
                .self_type
                .map(|self_type| self.concrete_type(self_type)),
        }
    }

    fn enqueue(&mut self, instance: FunctionInstance, ancestry: Vec<FunctionInstance>, span: Span) {
        if ancestry.iter().any(|ancestor| {
            ancestor.declaration == instance.declaration
                && ancestor.arguments != instance.arguments
                && ancestor
                    .arguments
                    .iter()
                    .zip(&instance.arguments)
                    .any(|(old, new)| old != new && self.typed.types.contains_type(*new, *old))
        }) {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::TypeSystem,
                    "generic instantiation expands without bound",
                )
                .with_primary(span),
            );
            return;
        }
        if self.queued.insert(instance.clone()) {
            self.pending
                .push_back(PendingInstance { instance, ancestry });
        }
    }

    /// Records the vtable for one (trait, implementing type) pair and enqueues
    /// every method it names, since a dynamic call can select any of them.
    fn register_vtable(
        &mut self,
        trait_declaration: DeclarationId,
        trait_type: TypeId,
        concrete: TypeId,
        span: Span,
    ) {
        if self.vtables.iter().any(|vtable| {
            self.typed
                .types
                .exactly_equal(vtable.trait_type, trait_type)
                && self.typed.types.exactly_equal(vtable.concrete, concrete)
        }) {
            return;
        }
        let mut methods = Vec::new();
        let mut signatures = Vec::new();
        for (name, method) in crate::traits::vtable_slots(self.resolved, trait_declaration) {
            let Some(signature) = self.instantiate_trait_slot_signature(trait_type, method) else {
                return;
            };
            signatures.push(signature);
            if self
                .resolved
                .is_standard_declaration(trait_declaration, "Callable")
            {
                match self.expanded_kind(concrete).clone() {
                    TypeKind::Closure { declaration, .. } if name == "call" => {
                        let instance = self.closure_instance(declaration);
                        self.enqueue_reachable(instance.clone(), span);
                        methods.push(VtableMethod::Closure(instance));
                        continue;
                    }
                    TypeKind::Reference { target, .. }
                        if name == "call"
                            && matches!(
                                self.expanded_kind(target),
                                TypeKind::Function {
                                    safety: crate::types::Safety::Safe,
                                    ..
                                }
                            ) =>
                    {
                        methods.push(VtableMethod::FunctionReference);
                        continue;
                    }
                    _ => {}
                }
            }
            let Some(entry) = crate::traits::vtable_entry(
                self.resolved,
                self.typed,
                trait_declaration,
                concrete,
                &name,
            ) else {
                return;
            };
            let instance = FunctionInstance {
                declaration: entry.declaration,
                arguments: entry.arguments,
                self_type: entry.self_type,
            };
            self.enqueue_reachable(instance.clone(), span);
            methods.push(VtableMethod::Function(instance));
        }
        self.vtables.push(Vtable {
            trait_declaration,
            trait_type,
            concrete,
            methods,
            signatures,
        });
    }

    fn instantiate_trait_slot_signature(
        &mut self,
        trait_type: TypeId,
        method: DeclarationId,
    ) -> Option<crate::types::FunctionSignature> {
        let TypeKind::Nominal {
            identity,
            arguments,
        } = self.expanded_kind(trait_type).clone()
        else {
            return None;
        };
        let mut substitution = Substitution::new();
        for (parameter, argument) in self.resolved.declarations[identity.declaration.index()]
            .generic_parameters
            .iter()
            .copied()
            .zip(arguments)
        {
            substitution.insert(parameter, argument);
        }
        let template = self.typed.function_signatures.get(&method)?.clone();
        let receiver = template
            .receiver
            .map(|ty| self.typed.types.substitute(ty, &substitution));
        let parameters = template
            .parameters
            .iter()
            .map(|parameter| crate::types::FunctionParameter {
                ty: self.typed.types.substitute(parameter.ty, &substitution),
                variadic: parameter.variadic,
            })
            .collect::<Vec<_>>();
        let return_type = self
            .typed
            .types
            .substitute(template.return_type, &substitution);
        let ty = self.typed.types.intern(TypeKind::Function {
            safety: crate::types::Safety::Safe,
            abi: crate::types::Abi::Elamite,
            receiver,
            parameters: parameters.clone(),
            return_type,
        });
        Some(crate::types::FunctionSignature {
            ty,
            receiver,
            parameters,
            return_type,
        })
    }

    fn enqueue_trait_methods(
        &mut self,
        trait_declaration: DeclarationId,
        concrete: TypeId,
        names: &[&str],
        span: Span,
    ) {
        for name in names {
            let Some(entry) = crate::traits::vtable_entry(
                self.resolved,
                self.typed,
                trait_declaration,
                concrete,
                name,
            ) else {
                continue;
            };
            self.enqueue_reachable(
                FunctionInstance {
                    declaration: entry.declaration,
                    arguments: entry.arguments,
                    self_type: entry.self_type,
                },
                span,
            );
        }
    }

    /// Registers vtables for every implementation of a trait, which a dynamic
    /// call may reach.
    fn enqueue_trait_implementations(&mut self, trait_declaration: DeclarationId, span: Span) {
        let targets = self
            .resolved
            .impls
            .iter()
            .filter_map(|block| {
                let trait_type = self.typed.impl_trait_types.get(&block.id).copied()?;
                let target = self.typed.impl_target_types.get(&block.id).copied()?;
                (crate::traits::object_trait_of_nominal(self.resolved, self.typed, trait_type)
                    == Some(trait_declaration)
                    && !self.typed.types.contains_generic_parameter(target))
                .then_some((trait_type, target))
            })
            .collect::<Vec<_>>();
        for (trait_type, target) in targets {
            self.register_vtable(trait_declaration, trait_type, target, span);
        }
    }

    fn enqueue_reachable(&mut self, instance: FunctionInstance, span: Span) {
        if !matches!(
            self.resolved.declarations[instance.declaration.index()].kind,
            DeclarationKind::Function | DeclarationKind::Closure | DeclarationKind::Test
        ) {
            return;
        }
        let mut ancestry = self.ancestry.clone();
        if let Some(current) = &self.current_instance {
            ancestry.push(current.clone());
        }
        self.enqueue(instance, ancestry, span);
    }

    fn enqueue_default_dependencies(
        &mut self,
        ty: TypeId,
        span: Span,
        visiting: &mut BTreeSet<TypeId>,
    ) {
        let ty = self.concrete_type(ty);
        if !visiting.insert(ty) {
            return;
        }
        match self.typed.types.kind(ty).clone() {
            TypeKind::Alias { target, .. } => {
                self.enqueue_default_dependencies(target, span, visiting);
            }
            TypeKind::Tuple(elements) => {
                for element in elements {
                    self.enqueue_default_dependencies(element, span, visiting);
                }
            }
            TypeKind::Array { element, .. } => {
                self.enqueue_default_dependencies(element, span, visiting);
            }
            // An intrinsic default (`Option.None`) selects a variant with no
            // payload, so it depends on no other type's default.
            TypeKind::Nominal { identity, .. }
                if crate::traits::intrinsic_derivation(
                    self.resolved,
                    identity.declaration,
                    "Default",
                ) => {}
            TypeKind::Nominal {
                identity,
                arguments,
            } if crate::traits::derives(self.resolved, identity.declaration, "Default") => {
                let fields = self
                    .resolved
                    .fields
                    .iter()
                    .filter(|field| {
                        field.parent_declaration == identity.declaration
                            && field.parent_variant.is_none()
                    })
                    .map(|field| field.id)
                    .collect::<Vec<_>>();
                for field in fields {
                    if let Some(field_type) =
                        self.typed
                            .instantiate_field_type(self.resolved, field, &arguments)
                    {
                        self.enqueue_default_dependencies(field_type, span, visiting);
                    }
                }
            }
            TypeKind::Nominal { .. } => {
                if let Ok(Some(selected)) = crate::traits::select_trait_method(
                    self.resolved,
                    self.typed,
                    ty,
                    "default",
                    None,
                ) {
                    self.enqueue_reachable(
                        FunctionInstance {
                            declaration: selected.declaration,
                            arguments: selected.arguments,
                            self_type: selected.self_type,
                        },
                        span,
                    );
                }
            }
            _ => {}
        }
        visiting.remove(&ty);
    }

    fn collect_concrete_nominals(&mut self, program: &mut TypedIrProgram) {
        // Instantiating one nominal's fields can intern further concrete
        // nominals (for example `Chain[i32]` creates
        // `Option[&Chain[i32]]`). Walk the arena to a fixed point instead of
        // snapshotting its initial length, otherwise those nested instances
        // reach the backend without a representation.
        let mut index = 0;
        while index < self.typed.types.len() {
            let Some(ty) = self.typed.types.id_at(index) else {
                break;
            };
            index += 1;
            if self.typed.types.contains_generic_parameter(ty) {
                continue;
            }
            let TypeKind::Nominal {
                identity,
                arguments,
            } = self.typed.types.kind(ty).clone()
            else {
                continue;
            };
            let declaration = &self.resolved.declarations[identity.declaration.index()];
            if declaration.kind == DeclarationKind::Struct {
                let fields = self
                    .resolved
                    .fields
                    .iter()
                    .filter(|field| {
                        field.parent_declaration == declaration.id && field.parent_variant.is_none()
                    })
                    .filter_map(|field| {
                        self.typed
                            .instantiate_field_type(self.resolved, field.id, &arguments)
                            .map(|field_type| {
                                (
                                    field.id,
                                    self.resolved.symbol_text(field.name).to_string(),
                                    field_type,
                                )
                            })
                    })
                    .collect();
                program.structs.push(TypedStruct {
                    ty,
                    declaration: declaration.id,
                    name: self.resolved.symbol_text(declaration.name).to_string(),
                    fields,
                });
            } else if declaration.kind == DeclarationKind::Enum {
                let variants = self
                    .resolved
                    .variants
                    .iter()
                    .filter(|variant| variant.parent == declaration.id)
                    .map(|variant| TypedVariant {
                        id: variant.id,
                        name: self.resolved.symbol_text(variant.name).to_string(),
                        fields: variant
                            .fields
                            .iter()
                            .filter_map(|field_id| {
                                let field = &self.resolved.fields[field_id.index()];
                                self.typed
                                    .instantiate_field_type(self.resolved, *field_id, &arguments)
                                    .map(|field_type| {
                                        (
                                            *field_id,
                                            self.resolved.symbol_text(field.name).to_string(),
                                            field_type,
                                        )
                                    })
                            })
                            .collect(),
                    })
                    .collect();
                program.enums.push(TypedEnum {
                    ty,
                    declaration: declaration.id,
                    name: self.resolved.symbol_text(declaration.name).to_string(),
                    variants,
                });
            }
        }
    }

    fn lower_function(&mut self, instance: &FunctionInstance) -> Option<TypedFunction> {
        let declaration = instance.declaration;
        let data = &self.resolved.declarations[declaration.index()];
        let signature = self.typed.instantiate_signature(self.resolved, instance)?;
        let mut parameters = Vec::new();
        let mut local_types = BTreeMap::new();
        let closure = self.checked.closures.get(&declaration).map(|closure| {
            let captures = closure
                .captures
                .iter()
                .enumerate()
                .map(|(index, capture)| {
                    local_types.insert(capture.binding, self.concrete_type(capture.ty));
                    (capture.binding, index)
                })
                .collect();
            TypedClosureBody {
                ty: self.concrete_type(closure.ty),
                captures,
            }
        });
        if let Some(parameter_list) =
            crate::syntax::direct_child(&data.syntax, SyntaxKind::Parameters)
        {
            let nodes = crate::syntax::direct_children(parameter_list, SyntaxKind::Parameter);
            let mut ordinary_parameters = signature.parameters.iter();
            for node in nodes {
                let is_receiver = crate::syntax::direct_tokens(node)
                    .iter()
                    .any(|token| matches!(token.kind, TokenKind::Keyword(Keyword::SelfValue)));
                let parameter_type = if is_receiver {
                    signature.receiver
                } else {
                    ordinary_parameters.next().map(|parameter| {
                        if parameter.variadic {
                            self.typed
                                .types
                                .id_for_kind(&TypeKind::Slice(parameter.ty))
                                .expect("checking interns every variadic binding's slice type")
                        } else {
                            parameter.ty
                        }
                    })
                };
                let Some(parameter_type) = parameter_type else {
                    continue;
                };
                let Some(token) = parameter_name_token(node) else {
                    continue;
                };
                let Some(binding) = self.binding_by_span.get(&token.span).copied() else {
                    continue;
                };
                parameters.push(TypedParameter {
                    binding,
                    ty: parameter_type,
                    span: token.span,
                });
                local_types.insert(binding, parameter_type);
            }
        }
        // Promotion is computed from the function's syntax rather than from
        // lowered statements, so it is available even for bodies whose
        // reference lowering is not yet represented.
        let body_block = crate::syntax::direct_child(&data.syntax, SyntaxKind::Block);
        let promoted_locals = body_block
            .map(|block| crate::promotion::address_taken_locals(self.resolved, block))
            .unwrap_or_default();
        let allocates_managed =
            body_block.is_some_and(crate::promotion::allocates_managed_temporary);
        let body = crate::syntax::direct_child(&data.syntax, SyntaxKind::Block)
            .map(|block| self.lower_block(block, &mut local_types))
            .unwrap_or_default();
        Some(TypedFunction {
            declaration,
            instance: instance.clone(),
            name: self.resolved.symbol_text(data.name).to_string(),
            span: data.span,
            parameters,
            return_type: signature.return_type,
            body,
            local_types,
            promoted_locals,
            allocates_managed,
            closure,
        })
    }

    fn lower_block(
        &mut self,
        block: &SyntaxNode,
        local_types: &mut BTreeMap<LocalBindingId, TypeId>,
    ) -> Vec<TypedStatement> {
        let mut statements = Vec::new();
        for node in child_nodes(block) {
            let before = self.diagnostics.len();
            match self.lower_statement(node, local_types) {
                Some(statement) => statements.push(statement),
                // A statement that cannot be lowered must be reported. Dropping
                // it silently would generate a program missing an effect the
                // source asked for.
                None if self.diagnostics.len() == before => {
                    self.unsupported(node.span, "this statement");
                }
                None => {}
            }
        }
        statements
    }

    fn lower_statement(
        &mut self,
        node: &SyntaxNode,
        local_types: &mut BTreeMap<LocalBindingId, TypeId>,
    ) -> Option<TypedStatement> {
        let kind = match node.kind {
            SyntaxKind::LetStatement => {
                let value_node = child_nodes(node).into_iter().next_back()?;
                let value = self.lower_expression(value_node)?;
                let ty = self
                    .checked
                    .expression_types
                    .get(&value_node.span)
                    .copied()
                    .unwrap_or(value.ty);
                let ty = self.concrete_type(ty);
                let mutable = node.children.iter().any(|child| {
                    matches!(
                        child,
                        SyntaxElement::Token(Token {
                            kind: TokenKind::Keyword(Keyword::Var),
                            ..
                        })
                    )
                });
                if let Some(pattern) = child_nodes(node)
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::TuplePattern)
                {
                    let mut bindings = Vec::new();
                    self.collect_tuple_bindings(
                        pattern,
                        &mut Vec::new(),
                        &mut bindings,
                        local_types,
                    );
                    TypedStatementKind::Destructure {
                        bindings,
                        mutable,
                        value,
                    }
                } else {
                    let token = let_name_token(node)?;
                    if identifier_text(token) == "_" {
                        return Some(TypedStatement {
                            span: node.span,
                            kind: TypedStatementKind::Destructure {
                                bindings: Vec::new(),
                                mutable,
                                value,
                            },
                        });
                    }
                    let binding = self.binding_by_span.get(&token.span).copied()?;
                    local_types.insert(binding, ty);
                    TypedStatementKind::Let {
                        binding,
                        mutable,
                        ty,
                        value,
                    }
                }
            }
            SyntaxKind::AssignmentStatement => {
                let nodes = child_nodes(node);
                let target = *nodes.first()?;
                let Some(place) = self.lower_place(target) else {
                    self.unsupported(target.span, "an assignment to this place");
                    return None;
                };
                let value = self.lower_expression(*nodes.get(1)?)?;
                let operator = node.children.iter().find_map(|child| match child {
                    SyntaxElement::Token(token) => assignment_operator(&token.kind),
                    SyntaxElement::Node(_) => None,
                })?;
                TypedStatementKind::Assign {
                    place,
                    operator,
                    value,
                }
            }
            SyntaxKind::ExpressionStatement => TypedStatementKind::Expression(
                self.lower_expression(child_nodes(node).into_iter().next()?)?,
            ),
            SyntaxKind::ReturnStatement => TypedStatementKind::Return(
                child_nodes(node)
                    .into_iter()
                    .next()
                    .and_then(|expression| self.lower_expression(expression)),
            ),
            SyntaxKind::IfStatement => {
                let condition_node = child_nodes(node).into_iter().find(|child| {
                    child.kind != SyntaxKind::Block && child.kind != SyntaxKind::ElseClause
                })?;
                let condition = self.lower_expression(condition_node)?;
                let then_body = child_nodes(node)
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)
                    .map(|block| self.lower_block(block, local_types))
                    .unwrap_or_default();
                let else_body = child_nodes(node)
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::ElseClause)
                    .and_then(|clause| {
                        child_nodes(clause)
                            .into_iter()
                            .find(|child| child.kind == SyntaxKind::Block)
                    })
                    .map(|block| self.lower_block(block, local_types))
                    .unwrap_or_default();
                TypedStatementKind::If {
                    condition,
                    then_body,
                    else_body,
                }
            }
            SyntaxKind::WhileStatement => {
                let condition_node = child_nodes(node)
                    .into_iter()
                    .find(|child| child.kind != SyntaxKind::Block)?;
                let condition = self.lower_expression(condition_node)?;
                let body = child_nodes(node)
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)
                    .map(|block| self.lower_block(block, local_types))
                    .unwrap_or_default();
                TypedStatementKind::While { condition, body }
            }
            SyntaxKind::ForStatement => {
                let children = child_nodes(node);
                let iterable_node = children
                    .iter()
                    .copied()
                    .find(|child| child.kind != SyntaxKind::Block)?;
                let iterable = self.lower_expression(iterable_node)?;
                let binding_span = node.children.iter().find_map(|child| match child {
                    SyntaxElement::Token(token)
                        if matches!(token.kind, TokenKind::Identifier(_)) =>
                    {
                        Some(token.span)
                    }
                    _ => None,
                })?;
                let binding = self
                    .resolved
                    .local_bindings
                    .iter()
                    .find(|binding| {
                        binding.kind == LocalBindingKind::Loop && binding.span == binding_span
                    })?
                    .id;
                let (kind, binding_type) = match self.expanded_kind(iterable.ty) {
                    TypeKind::Slice(element) => (
                        IterationKind::Slice {
                            collection: iterable.ty,
                            element: *element,
                        },
                        *element,
                    ),
                    TypeKind::Array { element, length } => (
                        IterationKind::Array {
                            length: *length,
                            element: *element,
                        },
                        *element,
                    ),
                    TypeKind::Builtin { builtin, arguments } => {
                        match (self.resolved.builtin_name(*builtin), arguments.as_slice()) {
                            ("Vec", [element]) => (
                                IterationKind::Vec {
                                    collection: iterable.ty,
                                    element: *element,
                                },
                                *element,
                            ),
                            ("Set", [element]) => (
                                IterationKind::Set {
                                    collection: iterable.ty,
                                    element: *element,
                                },
                                *element,
                            ),
                            ("Map", [key, value]) => {
                                let pair = self
                                    .typed
                                    .types
                                    .id_for_kind(&TypeKind::Tuple(vec![*key, *value]))?;
                                (
                                    IterationKind::Map {
                                        collection: iterable.ty,
                                        key: *key,
                                        value: *value,
                                        pair,
                                    },
                                    pair,
                                )
                            }
                            _ => return None,
                        }
                    }
                    _ => return None,
                };
                local_types.insert(binding, binding_type);
                let body = children
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)
                    .map(|block| self.lower_block(block, local_types))
                    .unwrap_or_default();
                TypedStatementKind::For {
                    binding,
                    iterable,
                    kind,
                    body,
                }
            }
            SyntaxKind::MatchStatement => {
                let nodes = child_nodes(node);
                let scrutinee_node = nodes
                    .iter()
                    .copied()
                    .find(|child| child.kind != SyntaxKind::Block)?;
                let scrutinee = self.lower_expression(scrutinee_node)?;
                let arm_block = nodes
                    .iter()
                    .copied()
                    .find(|child| child.kind == SyntaxKind::Block)?;
                let mut arms = Vec::new();
                for arm in child_nodes(arm_block)
                    .into_iter()
                    .filter(|child| child.kind == SyntaxKind::MatchArm)
                {
                    let pattern_node = child_nodes(arm).into_iter().find(|child| {
                        matches!(
                            child.kind,
                            SyntaxKind::Pattern | SyntaxKind::AlternativePattern
                        )
                    })?;
                    let bindings = self.pattern_bindings(arm);
                    let pattern =
                        self.lower_pattern(pattern_node, scrutinee.ty, &bindings, local_types)?;
                    let guard = child_nodes(arm)
                        .into_iter()
                        .find(|child| child.kind == SyntaxKind::Guard)
                        .and_then(|guard| child_nodes(guard).into_iter().next())
                        .and_then(|expression| self.lower_expression(expression));
                    let body = child_nodes(arm)
                        .into_iter()
                        .find(|child| child.kind == SyntaxKind::Block)
                        .map(|block| self.lower_block(block, local_types))
                        .unwrap_or_default();
                    arms.push(TypedMatchArm {
                        pattern,
                        guard,
                        body,
                        span: arm.span,
                    });
                }
                TypedStatementKind::Match { scrutinee, arms }
            }
            SyntaxKind::UnsafeBlock => {
                let block = child_nodes(node)
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)?;
                TypedStatementKind::Block(self.lower_block(block, local_types))
            }
            SyntaxKind::BreakStatement => TypedStatementKind::Break,
            SyntaxKind::ContinueStatement => TypedStatementKind::Continue,
            SyntaxKind::PassStatement => TypedStatementKind::Pass,
            SyntaxKind::DeferStatement => {
                let body = match child_nodes(node)
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)
                {
                    // `defer:` block form: one registration holding the
                    // block's statements in source order.
                    Some(block) => self.lower_block(block, local_types),
                    // `defer call` single-call form: a body holding exactly
                    // that call statement.
                    None => {
                        let call = child_nodes(node).into_iter().next()?;
                        vec![TypedStatement {
                            span: call.span,
                            kind: TypedStatementKind::Expression(self.lower_expression(call)?),
                        }]
                    }
                };
                TypedStatementKind::Defer(body)
            }
            SyntaxKind::ExpectStatement => {
                let children = child_nodes(node);
                let selector_node = children
                    .iter()
                    .copied()
                    .find(|child| child.kind != SyntaxKind::Block)?;
                let selector = self.lower_expression(selector_node)?;
                let trait_declaration = self.resolved.standard_declaration("RuntimeTrap")?;
                self.enqueue_trait_methods(
                    trait_declaration,
                    selector.ty,
                    &["code", "message"],
                    selector.span,
                );
                let body = children
                    .into_iter()
                    .find(|child| child.kind == SyntaxKind::Block)
                    .map(|block| self.lower_block(block, local_types))
                    .unwrap_or_default();
                TypedStatementKind::Expect {
                    selector,
                    trait_declaration,
                    body,
                }
            }
            _ => return None,
        };
        Some(TypedStatement {
            span: node.span,
            kind,
        })
    }

    fn pattern_bindings(&self, arm: &SyntaxNode) -> BTreeMap<String, LocalBindingId> {
        self.resolved
            .local_bindings
            .iter()
            .filter(|binding| {
                binding.kind == LocalBindingKind::PatternCandidate
                    && binding.span.file == arm.span.file
                    && binding.span.start >= arm.span.start
                    && binding.span.end <= arm.span.end
            })
            .map(|binding| {
                (
                    self.resolved.symbol_text(binding.name).to_string(),
                    binding.id,
                )
            })
            .collect()
    }

    fn collect_tuple_bindings(
        &mut self,
        pattern: &SyntaxNode,
        indices: &mut Vec<usize>,
        bindings: &mut Vec<TypedTupleBinding>,
        local_types: &mut BTreeMap<LocalBindingId, TypeId>,
    ) {
        match pattern.kind {
            SyntaxKind::TuplePattern => {
                for (index, element) in child_nodes(pattern).into_iter().enumerate() {
                    indices.push(index);
                    self.collect_tuple_bindings(element, indices, bindings, local_types);
                    indices.pop();
                }
            }
            SyntaxKind::Pattern => {
                let Some(token) = first_identifier(pattern) else {
                    return;
                };
                if identifier_text(token) == "_" {
                    return;
                }
                let Some(binding) = self.binding_by_span.get(&token.span).copied() else {
                    return;
                };
                let ty = self
                    .checked
                    .pattern_binding_types
                    .get(&binding)
                    .copied()
                    .unwrap_or_else(|| self.typed.types.error());
                let ty = self.concrete_type(ty);
                local_types.insert(binding, ty);
                bindings.push(TypedTupleBinding {
                    binding,
                    ty,
                    indices: indices.clone(),
                });
            }
            _ => {}
        }
    }

    fn lower_pattern(
        &mut self,
        node: &SyntaxNode,
        expected: TypeId,
        bindings: &BTreeMap<String, LocalBindingId>,
        local_types: &mut BTreeMap<LocalBindingId, TypeId>,
    ) -> Option<TypedPattern> {
        let kind = match node.kind {
            SyntaxKind::AlternativePattern => TypedPatternKind::Alternative(
                child_nodes(node)
                    .into_iter()
                    .map(|child| self.lower_pattern(child, expected, bindings, local_types))
                    .collect::<Option<Vec<_>>>()?,
            ),
            SyntaxKind::DereferencePattern => {
                let target = match self.expanded_kind(expected) {
                    TypeKind::Reference { target, .. } => *target,
                    TypeKind::Error => self.typed.types.error(),
                    _ => {
                        self.unsupported(node.span, "an invalid dereference pattern");
                        return None;
                    }
                };
                let inner = child_nodes(node).into_iter().next()?;
                TypedPatternKind::Dereference(Box::new(self.lower_pattern(
                    inner,
                    target,
                    bindings,
                    local_types,
                )?))
            }
            SyntaxKind::TuplePattern => {
                let elements = child_nodes(node);
                let member_types = match self.expanded_kind(expected) {
                    TypeKind::Tuple(members) => members.clone(),
                    TypeKind::Error => vec![self.typed.types.error(); elements.len()],
                    _ => return None,
                };
                TypedPatternKind::Tuple(
                    elements
                        .into_iter()
                        .zip(member_types)
                        .map(|(element, ty)| self.lower_pattern(element, ty, bindings, local_types))
                        .collect::<Option<Vec<_>>>()?,
                )
            }
            SyntaxKind::RecordPattern | SyntaxKind::VariantPattern => {
                return self.lower_aggregate_pattern(node, expected, bindings, local_types);
            }
            SyntaxKind::Pattern => {
                if let Some(inner) = child_nodes(node).into_iter().next() {
                    return self.lower_pattern(inner, expected, bindings, local_types);
                }
                if let Some(constant) = lower_constant(node, false) {
                    TypedPatternKind::Literal(constant)
                } else if let Some((_, variant)) = self.resolve_pattern_variant(node) {
                    TypedPatternKind::Variant {
                        variant,
                        fields: Vec::new(),
                    }
                } else {
                    let token = first_identifier(node)?;
                    let name = identifier_text(token);
                    if name == "_" {
                        TypedPatternKind::Wildcard
                    } else {
                        let binding = *bindings.get(name)?;
                        let ty = self
                            .checked
                            .pattern_binding_types
                            .get(&binding)
                            .copied()
                            .unwrap_or(expected);
                        let ty = self.concrete_type(ty);
                        local_types.insert(binding, ty);
                        TypedPatternKind::Binding(binding)
                    }
                }
            }
            _ => return None,
        };
        Some(TypedPattern {
            ty: expected,
            span: node.span,
            kind,
        })
    }

    fn lower_aggregate_pattern(
        &mut self,
        node: &SyntaxNode,
        expected: TypeId,
        bindings: &BTreeMap<String, LocalBindingId>,
        local_types: &mut BTreeMap<LocalBindingId, TypeId>,
    ) -> Option<TypedPattern> {
        if let Some((_, variant)) = self.resolve_pattern_variant(node) {
            let required = self.resolved.variants[variant.index()].fields.clone();
            let fields =
                self.lower_pattern_fields(node, &required, bindings, local_types, expected)?;
            return Some(TypedPattern {
                ty: expected,
                span: node.span,
                kind: TypedPatternKind::Variant { variant, fields },
            });
        }
        let declaration = match self.expanded_kind(expected) {
            TypeKind::Nominal { identity, .. }
            | TypeKind::Foreign {
                identity,
                complete: true,
            } => identity.declaration,
            _ => return None,
        };
        let required = self
            .resolved
            .fields
            .iter()
            .filter(|field| {
                field.parent_declaration == declaration && field.parent_variant.is_none()
            })
            .map(|field| field.id)
            .collect::<Vec<_>>();
        let fields = self.lower_pattern_fields(node, &required, bindings, local_types, expected)?;
        Some(TypedPattern {
            ty: expected,
            span: node.span,
            kind: TypedPatternKind::Struct {
                declaration,
                fields,
            },
        })
    }

    fn lower_pattern_fields(
        &mut self,
        node: &SyntaxNode,
        required: &[FieldId],
        bindings: &BTreeMap<String, LocalBindingId>,
        local_types: &mut BTreeMap<LocalBindingId, TypeId>,
        expected: TypeId,
    ) -> Option<Vec<(FieldId, TypedPattern)>> {
        if node.kind == SyntaxKind::VariantPattern {
            return child_nodes(node)
                .into_iter()
                .zip(required.iter().copied())
                .map(|(pattern, field)| {
                    let ty = self.instantiated_pattern_field_type(field, expected)?;
                    Some((
                        field,
                        self.lower_pattern(pattern, ty, bindings, local_types)?,
                    ))
                })
                .collect();
        }
        let mut fields = Vec::new();
        for field_node in child_nodes(node)
            .into_iter()
            .filter(|child| child.kind == SyntaxKind::PatternField)
        {
            if matches!(
                field_node.children.first(),
                Some(SyntaxElement::Token(Token {
                    kind: TokenKind::DotDot,
                    ..
                }))
            ) {
                continue;
            }
            let token = first_identifier(field_node)?;
            let name = identifier_text(token);
            let field = required.iter().copied().find(|field| {
                self.resolved
                    .symbol_text(self.resolved.fields[field.index()].name)
                    == name
            })?;
            let ty = self.instantiated_pattern_field_type(field, expected)?;
            let pattern = if let Some(nested) = child_nodes(field_node).into_iter().next() {
                self.lower_pattern(nested, ty, bindings, local_types)?
            } else {
                let binding = *bindings.get(name)?;
                local_types.insert(binding, ty);
                TypedPattern {
                    ty,
                    span: token.span,
                    kind: TypedPatternKind::Binding(binding),
                }
            };
            fields.push((field, pattern));
        }
        Some(fields)
    }

    fn instantiated_pattern_field_type(
        &mut self,
        field: FieldId,
        expected: TypeId,
    ) -> Option<TypeId> {
        let expected = self.concrete_type(expected);
        let arguments = match self.expanded_kind(expected) {
            TypeKind::Nominal { arguments, .. } => arguments.clone(),
            _ => Vec::new(),
        };
        self.typed
            .instantiate_field_type(self.resolved, field, &arguments)
            .map(|ty| self.concrete_type(ty))
    }

    fn resolve_pattern_variant(&self, node: &SyntaxNode) -> Option<(DeclarationId, VariantId)> {
        let tokens = pattern_path_tokens(node);
        let first = tokens.first()?;
        let last = tokens
            .iter()
            .rev()
            .find(|token| matches!(token.kind, TokenKind::Identifier(_)))?;
        if first.span == last.span {
            return None;
        }
        let NameTarget::Item(ItemId::Declaration(declaration)) =
            self.resolved.reference_at(first.span)?.target
        else {
            return None;
        };
        if self.resolved.declarations[declaration.index()].kind != DeclarationKind::Enum {
            return None;
        }
        match self.find_member(declaration, identifier_text(last)) {
            Some(MemberId::Variant(variant)) => Some((declaration, variant)),
            _ => None,
        }
    }

    fn lower_expression(&mut self, node: &SyntaxNode) -> Option<TypedExpression> {
        let result_template = self
            .checked
            .expression_types
            .get(&node.span)
            .copied()
            .unwrap_or_else(|| self.typed.types.error());
        let coercion = self.checked.trait_object_coercions.get(&node.span).copied();
        let template = coercion.map_or(result_template, |coercion| coercion.source);
        let ty = self.concrete_type(template);
        if ty == self.typed.types.error() {
            self.unsupported(
                node.span,
                "an expression whose type belongs to a later milestone",
            );
            return None;
        }
        let place = self
            .checked
            .expression_places
            .get(&node.span)
            .copied()
            .unwrap_or(PlaceKind::Value);
        let kind = match node.kind {
            SyntaxKind::LiteralExpression => {
                TypedExpressionKind::Constant(lower_constant(node, false)?)
            }
            SyntaxKind::NameExpression => {
                if let Some(instance) = self.checked.function_references.get(&node.span).cloned() {
                    let instance = self.concrete_instance(&instance);
                    self.enqueue_reachable(instance.clone(), node.span);
                    TypedExpressionKind::FunctionReference(instance)
                } else {
                    let token = first_token(node)?;
                    let target = self.resolved.reference_at(token.span)?.target;
                    match target {
                        NameTarget::Local(binding) => {
                            if let Some(index) = self.closure_capture_index(binding) {
                                TypedExpressionKind::ClosureCapture(index)
                            } else {
                                TypedExpressionKind::Local(binding)
                            }
                        }
                        _ => {
                            self.unsupported(node.span, "a non-local value reference");
                            return None;
                        }
                    }
                }
            }
            SyntaxKind::ClosureExpression => {
                let closure = self
                    .checked
                    .closures
                    .values()
                    .find(|closure| {
                        self.resolved.declarations[closure.declaration.index()].span == node.span
                    })?
                    .clone();
                let instance = self.closure_instance(closure.declaration);
                self.enqueue_reachable(instance.clone(), node.span);
                let captures = closure
                    .captures
                    .iter()
                    .map(|capture| self.lower_closure_capture(capture))
                    .collect::<Option<Vec<_>>>()?;
                TypedExpressionKind::Closure { instance, captures }
            }
            SyntaxKind::UnaryExpression => {
                let operand_node = child_nodes(node).into_iter().next_back()?;
                let token = first_token(node)?;
                if matches!(token.kind, TokenKind::Amp) {
                    // A referenced composite literal has no place to address;
                    // it allocates a managed cell of its own (SPEC 3.2).
                    if operand_node.kind == SyntaxKind::RecordExpression {
                        let value = self.lower_expression(operand_node)?;
                        return self.finish_expression(
                            node,
                            TypedExpression {
                                kind: TypedExpressionKind::AddressOfTemporary(Box::new(value)),
                                ty,
                                place,
                                copy: false,
                                span: node.span,
                            },
                        );
                    }
                    let Some(target) = self.lower_place(operand_node) else {
                        self.unsupported(node.span, "a reference to this place");
                        return None;
                    };
                    return self.finish_expression(
                        node,
                        TypedExpression {
                            kind: TypedExpressionKind::AddressOf(Box::new(target)),
                            ty,
                            place,
                            copy: false,
                            span: node.span,
                        },
                    );
                }
                if matches!(token.kind, TokenKind::Star) {
                    let operand = self.lower_expression(operand_node)?;
                    return self.finish_expression(
                        node,
                        TypedExpression {
                            kind: TypedExpressionKind::Dereference(Box::new(operand)),
                            ty,
                            place,
                            copy: self.checked.copies.contains(&node.span),
                            span: node.span,
                        },
                    );
                }
                let operator = unary_operator(&token.kind)?;
                let negative_literal = operator == UnaryOperator::Negative
                    && operand_node.kind == SyntaxKind::LiteralExpression;
                if negative_literal {
                    if let Some(constant) = lower_constant(operand_node, true) {
                        TypedExpressionKind::Constant(constant)
                    } else {
                        TypedExpressionKind::Unary {
                            operator,
                            operand: Box::new(self.lower_expression(operand_node)?),
                        }
                    }
                } else {
                    TypedExpressionKind::Unary {
                        operator,
                        operand: Box::new(self.lower_expression(operand_node)?),
                    }
                }
            }
            SyntaxKind::BinaryExpression => {
                let nodes = child_nodes(node);
                let operator = node.children.iter().find_map(|child| match child {
                    SyntaxElement::Token(token) => binary_operator(&token.kind),
                    SyntaxElement::Node(_) => None,
                })?;
                TypedExpressionKind::Binary {
                    operator,
                    left: Box::new(self.lower_expression(*nodes.first()?)?),
                    right: Box::new(self.lower_expression(*nodes.get(1)?)?),
                }
            }
            SyntaxKind::CastExpression => {
                let value = self.lower_expression(child_nodes(node).into_iter().next()?)?;
                // A conversion to a trait object pairs the concrete reference
                // with its implementing type's vtable rather than reinterpreting
                // a scalar.
                if let Some(trait_declaration) =
                    crate::traits::object_trait(self.resolved, self.typed, ty)
                {
                    let concrete = match self.expanded_kind(value.ty) {
                        TypeKind::Reference { target, .. } => *target,
                        _ => value.ty,
                    };
                    let TypeKind::Reference {
                        target: object_target,
                        ..
                    } = self.expanded_kind(ty)
                    else {
                        return None;
                    };
                    let TypeKind::TraitObject { trait_type } = self.expanded_kind(*object_target)
                    else {
                        return None;
                    };
                    let trait_type = *trait_type;
                    self.register_vtable(trait_declaration, trait_type, concrete, node.span);
                    TypedExpressionKind::MakeTraitObject {
                        value: Box::new(value),
                        trait_declaration,
                        trait_type,
                        concrete,
                    }
                } else {
                    TypedExpressionKind::Cast {
                        value: Box::new(value),
                    }
                }
            }
            SyntaxKind::CallExpression => self.lower_call(node)?,
            SyntaxKind::MemberExpression => {
                if let Some(instance) = self.checked.function_references.get(&node.span).cloned() {
                    let instance = self.concrete_instance(&instance);
                    self.enqueue_reachable(instance.clone(), node.span);
                    TypedExpressionKind::FunctionReference(instance)
                } else if let Some((declaration, variant)) =
                    self.resolve_enum_variant_expression(node)
                {
                    TypedExpressionKind::Enum {
                        declaration,
                        variant,
                        fields: Vec::new(),
                    }
                } else {
                    let base_node = child_nodes(node).into_iter().next()?;
                    let field = self.resolve_field(node, base_node)?;
                    TypedExpressionKind::Field {
                        base: Box::new(self.lower_receiver(base_node)?),
                        field,
                    }
                }
            }
            SyntaxKind::TupleFieldExpression => {
                let base_node = child_nodes(node).into_iter().next()?;
                TypedExpressionKind::TupleField {
                    base: Box::new(self.lower_tuple_receiver(base_node)?),
                    index: tuple_field_index(node)?,
                }
            }
            SyntaxKind::BracketExpression => {
                if let Some(instance) = self.checked.function_references.get(&node.span).cloned() {
                    let instance = self.concrete_instance(&instance);
                    self.enqueue_reachable(instance.clone(), node.span);
                    TypedExpressionKind::FunctionReference(instance)
                } else {
                    let nodes = child_nodes(node);
                    TypedExpressionKind::Index {
                        base: Box::new(self.lower_expression(*nodes.first()?)?),
                        index: Box::new(self.lower_expression(*nodes.get(1)?)?),
                    }
                }
            }
            SyntaxKind::ParenthesizedExpression => {
                return self.lower_expression(child_nodes(node).into_iter().next()?);
            }
            SyntaxKind::TupleExpression => TypedExpressionKind::Tuple(
                child_nodes(node)
                    .into_iter()
                    .map(|child| self.lower_expression(child))
                    .collect::<Option<Vec<_>>>()?,
            ),
            SyntaxKind::ArrayExpression => TypedExpressionKind::Array(
                child_nodes(node)
                    .into_iter()
                    .map(|child| self.lower_expression(child))
                    .collect::<Option<Vec<_>>>()?,
            ),
            SyntaxKind::RecordExpression => self.lower_record(node)?,
            SyntaxKind::FormattedStringExpression => self.lower_formatted(node)?,
            SyntaxKind::MacroExpression => {
                let TypeKind::Builtin { builtin, .. } = self.expanded_kind(ty) else {
                    return None;
                };
                let kind = match self.resolved.builtin_name(*builtin) {
                    "Vec" => CollectionLiteralKind::Vec,
                    "Map" => CollectionLiteralKind::Map,
                    "Set" => CollectionLiteralKind::Set,
                    _ => return None,
                };
                TypedExpressionKind::CollectionLiteral {
                    kind,
                    elements: child_nodes(node)
                        .into_iter()
                        .map(|element| self.lower_expression(element))
                        .collect::<Option<Vec<_>>>()?,
                }
            }
            SyntaxKind::TryExpression => {
                let operand_node = child_nodes(node).into_iter().next()?;
                let operand = self.lower_expression(operand_node)?;
                // The checker validated the operand as the standard
                // `Result[T, E]`; re-derive `E` from the *concrete* operand
                // type so generic instances carry their substituted error.
                let (_, error_type) = crate::check::standard_result_payloads(
                    self.resolved,
                    &self.typed.types,
                    operand.ty,
                )?;
                let result_declaration = self.resolved.standard_declaration("Result")?;
                let ok_variant = self.resolved.standard_variant("Result", "Ok")?;
                let err_variant = self.resolved.standard_variant("Result", "Err")?;
                let ok_field = *self.resolved.variants[ok_variant.index()].fields.first()?;
                let err_field = *self.resolved.variants[err_variant.index()].fields.first()?;
                TypedExpressionKind::Propagate {
                    operand: Box::new(operand),
                    result_declaration,
                    ok_variant,
                    ok_field,
                    err_variant,
                    err_field,
                    error_type,
                }
            }
            _ => return None,
        };
        self.finish_expression(
            node,
            TypedExpression {
                ty,
                place,
                copy: self.checked.copies.contains(&node.span),
                span: node.span,
                kind,
            },
        )
    }

    fn finish_expression(
        &mut self,
        node: &SyntaxNode,
        mut expression: TypedExpression,
    ) -> Option<TypedExpression> {
        let Some(coercion) = self.checked.trait_object_coercions.get(&node.span).copied() else {
            return Some(expression);
        };
        let TraitObjectCoercion {
            target,
            trait_declaration,
            concrete,
            ..
        } = coercion;
        let target = self.concrete_type(target);
        let concrete = self.concrete_type(concrete);
        let TypeKind::Reference {
            target: object_target,
            ..
        } = self.expanded_kind(target)
        else {
            return None;
        };
        let TypeKind::TraitObject { trait_type } = self.expanded_kind(*object_target) else {
            return None;
        };
        let trait_type = *trait_type;
        self.register_vtable(trait_declaration, trait_type, concrete, node.span);
        let copy = self.checked.copies.contains(&node.span);
        expression.copy = false;
        Some(TypedExpression {
            ty: target,
            place: PlaceKind::Value,
            copy,
            span: node.span,
            kind: TypedExpressionKind::MakeTraitObject {
                value: Box::new(expression),
                trait_declaration,
                trait_type,
                concrete,
            },
        })
    }

    fn lower_call(&mut self, node: &SyntaxNode) -> Option<TypedExpressionKind> {
        let nodes = child_nodes(node);
        let callee_node = *nodes.first()?;
        if let Some((declaration, variant)) = self.resolve_enum_variant_expression(callee_node) {
            let fields = self.resolved.variants[variant.index()]
                .fields
                .iter()
                .copied()
                .zip(nodes[1..].iter().copied())
                .map(|(field, argument)| Some((field, self.lower_expression(argument)?)))
                .collect::<Option<Vec<_>>>()?;
            return Some(TypedExpressionKind::Enum {
                declaration,
                variant,
                fields,
            });
        }
        let checked_call = self.checked.calls.get(&node.span).cloned()?;
        let mut source_arguments = nodes[1..].to_vec();
        let (callee, parameters, receiver) = match checked_call {
            CheckedCall::Direct(instance) => {
                let instance = self.concrete_instance(&instance);
                let signature = self.typed.instantiate_signature(self.resolved, &instance)?;
                self.enqueue_reachable(instance.clone(), node.span);
                let mut parameters = signature.parameters.clone();
                if let Some(receiver) = signature.receiver {
                    parameters.insert(
                        0,
                        crate::types::FunctionParameter {
                            ty: receiver,
                            variadic: false,
                        },
                    );
                }
                (TypedCallee::Function(instance), parameters, None)
            }
            CheckedCall::BoundMethod {
                instance,
                adjustment,
            } => {
                let instance = self.concrete_instance(&instance);
                let signature = self.typed.instantiate_signature(self.resolved, &instance)?;
                self.enqueue_reachable(instance.clone(), node.span);
                let base = child_nodes(callee_node).into_iter().next()?;
                let receiver = self.lower_bound_receiver(base, signature.receiver?, adjustment)?;
                (
                    TypedCallee::Function(instance),
                    signature.parameters,
                    Some(receiver),
                )
            }
            CheckedCall::Indirect => {
                let callee = self.lower_expression(callee_node)?;
                let parameters = self.function_parameters(callee.ty)?;
                (TypedCallee::Indirect(Box::new(callee)), parameters, None)
            }
            CheckedCall::Closure { declaration } => {
                let value = self.lower_expression(callee_node)?;
                let TypeKind::Closure {
                    parameters,
                    declaration: concrete_declaration,
                    ..
                } = self.expanded_kind(value.ty).clone()
                else {
                    return None;
                };
                debug_assert_eq!(declaration, concrete_declaration);
                let instance = self.closure_instance(declaration);
                self.enqueue_reachable(instance.clone(), node.span);
                let parameters = parameters
                    .into_iter()
                    .map(|ty| crate::types::FunctionParameter {
                        ty,
                        variadic: false,
                    })
                    .collect();
                (
                    TypedCallee::Closure {
                        instance,
                        value: Box::new(value),
                    },
                    parameters,
                    None,
                )
            }
            CheckedCall::CallableBound {
                trait_declaration,
                receiver_type,
                parameters,
            } => {
                let value = self.lower_expression(callee_node)?;
                let parameters = parameters
                    .into_iter()
                    .map(|ty| crate::types::FunctionParameter {
                        ty: self.concrete_type(ty),
                        variadic: false,
                    })
                    .collect::<Vec<_>>();
                match self.expanded_kind(value.ty).clone() {
                    TypeKind::Closure { declaration, .. } => {
                        let instance = self.closure_instance(declaration);
                        self.enqueue_reachable(instance.clone(), node.span);
                        (
                            TypedCallee::Closure {
                                instance,
                                value: Box::new(value),
                            },
                            parameters,
                            None,
                        )
                    }
                    TypeKind::Reference { target, .. }
                        if matches!(
                            self.expanded_kind(target),
                            TypeKind::Function {
                                safety: crate::types::Safety::Safe,
                                ..
                            }
                        ) =>
                    {
                        (TypedCallee::Indirect(Box::new(value)), parameters, None)
                    }
                    TypeKind::Nominal { .. } => {
                        let selected = crate::traits::vtable_entry(
                            self.resolved,
                            self.typed,
                            trait_declaration,
                            value.ty,
                            "call",
                        )?;
                        let instance = FunctionInstance {
                            declaration: selected.declaration,
                            arguments: selected.arguments,
                            self_type: selected.self_type,
                        };
                        self.enqueue_reachable(instance.clone(), node.span);
                        let lowered =
                            self.lower_call_arguments(node.span, &source_arguments, &parameters)?;
                        let tuple = if lowered.is_empty() {
                            TypedExpression {
                                ty: self.typed.types.primitive(PrimitiveType::Unit),
                                place: PlaceKind::Value,
                                copy: false,
                                span: node.span,
                                kind: TypedExpressionKind::Constant(crate::ir::Constant::Unit),
                            }
                        } else {
                            TypedExpression {
                                ty: self.typed.types.intern(TypeKind::Tuple(
                                    parameters.iter().map(|parameter| parameter.ty).collect(),
                                )),
                                place: PlaceKind::Value,
                                copy: false,
                                span: node.span,
                                kind: TypedExpressionKind::Tuple(lowered),
                            }
                        };
                        let receiver_target = self.concrete_type(receiver_type);
                        let receiver_type = self.typed.types.intern(TypeKind::Reference {
                            mutability: crate::types::Mutability::Shared,
                            target: receiver_target,
                        });
                        let receiver = TypedExpression {
                            ty: receiver_type,
                            place: PlaceKind::Value,
                            copy: false,
                            span: callee_node.span,
                            kind: TypedExpressionKind::AddressOf(Box::new(
                                self.lower_place(callee_node)?,
                            )),
                        };
                        return Some(TypedExpressionKind::Call {
                            callee: TypedCallee::Function(instance),
                            arguments: vec![receiver, tuple],
                        });
                    }
                    _ => return None,
                }
            }
            CheckedCall::CallableDynamic {
                trait_declaration,
                slot,
                parameters,
            } => {
                let parameters = parameters
                    .into_iter()
                    .map(|ty| crate::types::FunctionParameter {
                        ty: self.concrete_type(ty),
                        variadic: false,
                    })
                    .collect::<Vec<_>>();
                let lowered =
                    self.lower_call_arguments(node.span, &source_arguments, &parameters)?;
                let tuple = if lowered.is_empty() {
                    TypedExpression {
                        ty: self.typed.types.primitive(PrimitiveType::Unit),
                        place: PlaceKind::Value,
                        copy: false,
                        span: node.span,
                        kind: TypedExpressionKind::Constant(crate::ir::Constant::Unit),
                    }
                } else {
                    TypedExpression {
                        ty: self.typed.types.intern(TypeKind::Tuple(
                            parameters.iter().map(|parameter| parameter.ty).collect(),
                        )),
                        place: PlaceKind::Value,
                        copy: false,
                        span: node.span,
                        kind: TypedExpressionKind::Tuple(lowered),
                    }
                };
                let receiver = self.lower_expression(callee_node)?;
                return Some(TypedExpressionKind::Call {
                    callee: TypedCallee::Dynamic {
                        trait_declaration,
                        slot,
                    },
                    arguments: vec![receiver, tuple],
                });
            }
            CheckedCall::DerivedDefault { ty } => {
                let ty = self.concrete_type(ty);
                self.enqueue_default_dependencies(ty, node.span, &mut BTreeSet::new());
                return Some(TypedExpressionKind::DefaultValue(ty));
            }
            CheckedCall::NumericConversion {
                outcome, target, ..
            } => {
                let value = self.lower_expression(*nodes.get(1)?)?;
                return Some(TypedExpressionKind::NumericConversion {
                    outcome,
                    value: Box::new(value),
                    target: self.concrete_type(target),
                });
            }
            CheckedCall::NumericAlternative { operation, .. } => {
                let base = child_nodes(callee_node).into_iter().next()?;
                let receiver = self.lower_expression(base)?;
                let operand = match nodes.get(1) {
                    Some(argument) => Some(Box::new(self.lower_expression(argument)?)),
                    None => None,
                };
                return Some(TypedExpressionKind::NumericAlternative {
                    operation,
                    receiver: Box::new(receiver),
                    operand,
                });
            }
            CheckedCall::Standard(operation) => {
                match operation {
                    StandardCall::Trap {
                        reason_type,
                        trait_declaration,
                    } => self.enqueue_trait_methods(
                        trait_declaration,
                        reason_type,
                        &["code", "message"],
                        node.span,
                    ),
                    StandardCall::Fail { value_type } => {
                        if let Some(display) = self.resolved.standard_declaration("Display") {
                            self.enqueue_trait_methods(display, value_type, &["fmt"], node.span);
                        }
                    }
                    _ => {}
                }
                let mut arguments = Vec::new();
                let has_receiver = !matches!(
                    operation,
                    StandardCall::Panic
                        | StandardCall::Assert
                        | StandardCall::Fail { .. }
                        | StandardCall::Trap { .. }
                        | StandardCall::StringFrom
                        | StandardCall::IdentityFrom { .. }
                        | StandardCall::ForeignRootRetain { .. }
                        | StandardCall::VecNew { .. }
                        | StandardCall::MapNew { .. }
                        | StandardCall::SetNew { .. }
                );
                if has_receiver {
                    let base = child_nodes(callee_node).into_iter().next()?;
                    arguments.push(self.lower_expression(base)?);
                }
                arguments.extend(
                    source_arguments
                        .drain(..)
                        .map(|argument| self.lower_expression(argument))
                        .collect::<Option<Vec<_>>>()?,
                );
                return Some(TypedExpressionKind::StandardCall {
                    operation,
                    arguments,
                });
            }
            CheckedCall::TraitSelfMethod {
                trait_declaration,
                method,
            } => {
                // The enclosing default body is specialized for one
                // implementing type; resolve `self.method(...)` against it.
                let concrete = self
                    .current_instance
                    .as_ref()
                    .and_then(|instance| instance.self_type)?;
                let name = self
                    .resolved
                    .symbol_text(self.resolved.declarations[method.index()].name);
                let entry = crate::traits::vtable_entry(
                    self.resolved,
                    self.typed,
                    trait_declaration,
                    concrete,
                    name,
                )?;
                let instance = FunctionInstance {
                    declaration: entry.declaration,
                    arguments: entry.arguments,
                    self_type: entry.self_type,
                };
                let signature = self.typed.instantiate_signature(self.resolved, &instance)?;
                self.enqueue_reachable(instance.clone(), node.span);
                let base = child_nodes(callee_node).into_iter().next()?;
                let receiver = self.lower_expression(base)?;
                (
                    TypedCallee::Function(instance),
                    signature.parameters,
                    Some(receiver),
                )
            }
            CheckedCall::GenericBoundMethod {
                trait_declaration,
                method,
                receiver_type,
                adjustment,
            } => {
                let concrete = self.concrete_type(receiver_type);
                let name = self
                    .resolved
                    .symbol_text(self.resolved.declarations[method.index()].name);
                let selected = crate::traits::vtable_entry(
                    self.resolved,
                    self.typed,
                    trait_declaration,
                    concrete,
                    name,
                )?;
                let instance = FunctionInstance {
                    declaration: selected.declaration,
                    arguments: selected.arguments,
                    self_type: selected.self_type,
                };
                let signature = self.typed.instantiate_signature(self.resolved, &instance)?;
                self.enqueue_reachable(instance.clone(), node.span);
                let base = child_nodes(callee_node).into_iter().next()?;
                let receiver = self.lower_bound_receiver(base, signature.receiver?, adjustment)?;
                (
                    TypedCallee::Function(instance),
                    signature.parameters,
                    Some(receiver),
                )
            }
            CheckedCall::DynamicMethod {
                trait_declaration,
                method,
                slot,
            } => {
                let signature = self.typed.function_signatures.get(&method)?.clone();
                // Every concrete implementation reachable through this trait
                // must be lowered, since the vtable can select any of them.
                self.enqueue_trait_implementations(trait_declaration, node.span);
                let base = child_nodes(callee_node).into_iter().next()?;
                let receiver = self.lower_expression(base)?;
                (
                    TypedCallee::Dynamic {
                        trait_declaration,
                        slot,
                    },
                    signature.parameters,
                    Some(receiver),
                )
            }
            CheckedCall::Print { newline } => (TypedCallee::Print { newline }, Vec::new(), None),
        };
        let is_print = matches!(callee, TypedCallee::Print { .. });
        let mut arguments = if is_print {
            source_arguments
                .drain(..)
                .map(|argument| self.lower_expression(argument))
                .collect::<Option<Vec<_>>>()?
        } else {
            self.lower_call_arguments(node.span, &source_arguments, &parameters)?
        };
        if is_print {
            for argument in &arguments {
                self.enqueue_display_dependencies(argument.ty, argument.span);
            }
        }
        if let Some(receiver) = receiver {
            arguments.insert(0, receiver);
        }
        Some(TypedExpressionKind::Call { callee, arguments })
    }

    fn function_parameters(&self, ty: TypeId) -> Option<Vec<crate::types::FunctionParameter>> {
        let ty = self.typed.types.resolve_inference(ty);
        let target = match self.typed.types.kind(ty) {
            TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => {
                self.typed.types.resolve_inference(*target)
            }
            TypeKind::Alias { target, .. } => return self.function_parameters(*target),
            _ => return None,
        };
        match self.typed.types.kind(target) {
            TypeKind::Function { parameters, .. } => Some(parameters.clone()),
            _ => None,
        }
    }

    fn lower_call_arguments(
        &mut self,
        call_span: Span,
        arguments: &[&SyntaxNode],
        parameters: &[crate::types::FunctionParameter],
    ) -> Option<Vec<TypedExpression>> {
        let variadic = parameters
            .last()
            .is_some_and(|parameter| parameter.variadic);
        if !variadic {
            return arguments
                .iter()
                .map(|argument| self.lower_expression(argument))
                .collect();
        }
        let fixed = parameters.len().saturating_sub(1);
        let mut lowered = arguments[..arguments.len().min(fixed)]
            .iter()
            .map(|argument| self.lower_expression(argument))
            .collect::<Option<Vec<_>>>()?;
        let element = parameters.last()?.ty;
        let ty = self.typed.types.id_for_kind(&TypeKind::Slice(element))?;
        let values = arguments[arguments.len().min(fixed)..]
            .iter()
            .map(|argument| self.lower_expression(argument))
            .collect::<Option<Vec<_>>>()?;
        lowered.push(TypedExpression {
            ty,
            place: PlaceKind::Value,
            copy: false,
            span: arguments
                .get(fixed)
                .map_or(call_span, |argument| argument.span),
            kind: TypedExpressionKind::VariadicSlice(values),
        });
        Some(lowered)
    }

    fn lower_bound_receiver(
        &mut self,
        base: &SyntaxNode,
        receiver_type: TypeId,
        adjustment: ReceiverAdjustment,
    ) -> Option<TypedExpression> {
        match adjustment {
            ReceiverAdjustment::Pass => self.lower_expression(base),
            ReceiverAdjustment::CopyValue => {
                let mut receiver = self.lower_expression(base)?;
                receiver.copy = true;
                Some(receiver)
            }
            ReceiverAdjustment::DereferenceAndCopy => {
                let operand = self.lower_expression(base)?;
                Some(TypedExpression {
                    ty: receiver_type,
                    place: PlaceKind::Value,
                    copy: true,
                    span: base.span,
                    kind: TypedExpressionKind::Dereference(Box::new(operand)),
                })
            }
            ReceiverAdjustment::BorrowShared | ReceiverAdjustment::BorrowMutable => {
                Some(TypedExpression {
                    ty: receiver_type,
                    place: PlaceKind::Value,
                    copy: false,
                    span: base.span,
                    kind: TypedExpressionKind::AddressOf(Box::new(self.lower_place(base)?)),
                })
            }
        }
    }

    fn lower_record(&mut self, node: &SyntaxNode) -> Option<TypedExpressionKind> {
        let callee = child_nodes(node).into_iter().next()?;
        let enum_variant = self.resolve_enum_variant_expression(callee);
        let declaration = if let Some((declaration, _)) = enum_variant {
            declaration
        } else {
            let template = self.checked.expression_types.get(&node.span).copied()?;
            let ty = self.concrete_type(template);
            match self.expanded_kind(ty) {
                TypeKind::Nominal { identity, .. }
                | TypeKind::Foreign {
                    identity,
                    complete: true,
                } => identity.declaration,
                _ => {
                    self.unsupported(node.span, "a non-struct record construction");
                    return None;
                }
            }
        };
        if !matches!(
            self.resolved.declarations[declaration.index()].kind,
            DeclarationKind::Struct | DeclarationKind::ForeignStruct
        ) && enum_variant.is_none()
        {
            self.unsupported(node.span, "a non-record construction");
            return None;
        }
        let required = enum_variant.map_or_else(
            || {
                self.resolved
                    .fields
                    .iter()
                    .filter(|field| {
                        field.parent_declaration == declaration && field.parent_variant.is_none()
                    })
                    .map(|field| field.id)
                    .collect::<Vec<_>>()
            },
            |(_, variant)| self.resolved.variants[variant.index()].fields.clone(),
        );
        let mut fields = Vec::new();
        for field_node in child_nodes(node)
            .into_iter()
            .filter(|child| child.kind == SyntaxKind::RecordField)
        {
            let name_token = first_identifier(field_node)?;
            let field = required.iter().copied().find(|field| {
                self.resolved
                    .symbol_text(self.resolved.fields[field.index()].name)
                    == identifier_text(name_token)
            })?;
            let value = if let Some(value) = child_nodes(field_node).into_iter().next() {
                self.lower_expression(value)?
            } else {
                let target = self.resolved.reference_at(name_token.span)?.target;
                let NameTarget::Local(binding) = target else {
                    return None;
                };
                let template = self.typed.field_types.get(&field).copied()?;
                let ty = self.concrete_type(template);
                TypedExpression {
                    ty,
                    place: PlaceKind::Addressable,
                    copy: true,
                    span: name_token.span,
                    kind: TypedExpressionKind::Local(binding),
                }
            };
            fields.push((field, value));
        }
        if let Some((enum_declaration, variant)) = enum_variant {
            Some(TypedExpressionKind::Enum {
                declaration: enum_declaration,
                variant,
                fields,
            })
        } else {
            Some(TypedExpressionKind::Struct {
                declaration,
                fields,
            })
        }
    }

    fn lower_formatted(&mut self, node: &SyntaxNode) -> Option<TypedExpressionKind> {
        let token = first_token(node)?;
        let TokenKind::FormattedString(segments) = &token.kind else {
            return None;
        };
        let mut expressions = child_nodes(node).into_iter();
        let mut parts = Vec::new();
        for segment in segments {
            match &segment.kind {
                FormattedSegmentKind::Text(text) => {
                    if !text.is_empty() {
                        parts.push(FormattedPart::Text(text.clone()));
                    }
                }
                FormattedSegmentKind::Expression { .. } => {
                    let expression = self.lower_expression(expressions.next()?)?;
                    self.enqueue_display_dependencies(expression.ty, expression.span);
                    parts.push(FormattedPart::Expression(expression));
                }
            }
        }
        Some(TypedExpressionKind::FormattedString(parts))
    }

    fn enqueue_display_dependencies(&mut self, ty: TypeId, span: Span) {
        let ty = self.concrete_type(ty);
        match self.expanded_kind(ty).clone() {
            TypeKind::Reference { target, .. } => {
                self.enqueue_display_dependencies(target, span);
            }
            TypeKind::Tuple(elements) => {
                for element in elements {
                    self.enqueue_display_dependencies(element, span);
                }
            }
            TypeKind::Array { element, .. } => {
                self.enqueue_display_dependencies(element, span);
            }
            TypeKind::Builtin { arguments, .. } => {
                for argument in arguments {
                    self.enqueue_display_dependencies(argument, span);
                }
            }
            TypeKind::Nominal { .. } => {
                if let Ok(Some(selected)) =
                    crate::traits::select_trait_method(self.resolved, self.typed, ty, "fmt", None)
                {
                    self.enqueue_reachable(
                        FunctionInstance {
                            declaration: selected.declaration,
                            arguments: selected.arguments,
                            self_type: selected.self_type,
                        },
                        span,
                    );
                }
            }
            _ => {}
        }
    }

    fn lower_place(&mut self, node: &SyntaxNode) -> Option<TypedPlace> {
        let template = self.checked.expression_types.get(&node.span).copied()?;
        let ty = self.concrete_type(template);
        match node.kind {
            SyntaxKind::NameExpression => {
                let token = first_token(node)?;
                let NameTarget::Local(binding) = self.resolved.reference_at(token.span)?.target
                else {
                    return None;
                };
                if let Some(index) = self.closure_capture_index(binding) {
                    Some(TypedPlace::ClosureCapture {
                        index,
                        ty,
                        span: node.span,
                    })
                } else {
                    Some(TypedPlace::Local {
                        binding,
                        ty,
                        span: node.span,
                    })
                }
            }
            SyntaxKind::MemberExpression => {
                let base_node = child_nodes(node).into_iter().next()?;
                let field = self.resolve_field(node, base_node)?;
                Some(TypedPlace::Field {
                    base: Box::new(self.lower_place_receiver(base_node)?),
                    field,
                    ty,
                    span: node.span,
                })
            }
            SyntaxKind::TupleFieldExpression => {
                let base_node = child_nodes(node).into_iter().next()?;
                Some(TypedPlace::TupleField {
                    base: Box::new(self.lower_tuple_place_receiver(base_node)?),
                    index: tuple_field_index(node)?,
                    ty,
                    span: node.span,
                })
            }
            SyntaxKind::BracketExpression => {
                let nodes = child_nodes(node);
                let base_node = *nodes.first()?;
                let index = self.lower_expression(*nodes.get(1)?)?;
                let base_type = self
                    .checked
                    .expression_types
                    .get(&base_node.span)
                    .copied()?;
                let kind = match self.expanded_kind(base_type) {
                    TypeKind::Array { length, .. } => IndexKind::Array { length: *length },
                    TypeKind::Slice(_) => IndexKind::Slice,
                    TypeKind::Builtin { builtin, .. }
                        if self.resolved.builtin_name(*builtin) == "Vec" =>
                    {
                        IndexKind::Vec {
                            collection: base_type,
                        }
                    }
                    TypeKind::Builtin { builtin, .. }
                        if self.resolved.builtin_name(*builtin) == "Map" =>
                    {
                        IndexKind::Map {
                            collection: base_type,
                        }
                    }
                    _ => return None,
                };
                Some(TypedPlace::Index {
                    base: Box::new(self.lower_place(base_node)?),
                    index,
                    ty,
                    kind,
                    span: node.span,
                })
            }
            SyntaxKind::UnaryExpression => {
                let token = first_token(node)?;
                if !matches!(token.kind, TokenKind::Star) {
                    return None;
                }
                let operand = child_nodes(node).into_iter().next_back()?;
                Some(TypedPlace::Dereference {
                    base: Box::new(self.lower_expression(operand)?),
                    ty,
                    span: node.span,
                })
            }
            _ => None,
        }
    }

    fn resolve_field(&self, node: &SyntaxNode, base: &SyntaxNode) -> Option<FieldId> {
        let base_type = self.checked.expression_types.get(&base.span).copied()?;
        let declaration = self.field_owner(base_type)?;
        let name = identifier_text(first_identifier(node)?);
        self.find_field(declaration, name)
    }

    /// Lowers a field-access base, inserting the automatic dereference for a
    /// safe reference or raw pointer. Raw access has already been restricted
    /// to `unsafe:` by checking and traps during CFG lowering.
    fn lower_receiver(&mut self, base: &SyntaxNode) -> Option<TypedExpression> {
        let value = self.lower_expression(base)?;
        let Some(target) = self.field_pointer_target(value.ty) else {
            return Some(value);
        };
        Some(TypedExpression {
            ty: target,
            place: PlaceKind::Mutable,
            copy: false,
            span: base.span,
            kind: TypedExpressionKind::Dereference(Box::new(value)),
        })
    }

    /// The place form of [`Self::lower_receiver`].
    fn lower_place_receiver(&mut self, base: &SyntaxNode) -> Option<TypedPlace> {
        let template = self.checked.expression_types.get(&base.span).copied()?;
        let base_type = self.concrete_type(template);
        let Some(target) = self.field_pointer_target(base_type) else {
            return self.lower_place(base);
        };
        Some(TypedPlace::Dereference {
            base: Box::new(self.lower_expression(base)?),
            ty: target,
            span: base.span,
        })
    }

    /// Lowers a positional tuple selector's receiver. Both safe references and
    /// raw pointers select through their pointee; raw access is already
    /// restricted to `unsafe:` by checking and is trapped during CFG lowering.
    fn lower_tuple_receiver(&mut self, base: &SyntaxNode) -> Option<TypedExpression> {
        let value = self.lower_expression(base)?;
        let Some(target) = self.tuple_pointer_target(value.ty) else {
            return Some(value);
        };
        Some(TypedExpression {
            ty: target,
            place: PlaceKind::Mutable,
            copy: false,
            span: base.span,
            kind: TypedExpressionKind::Dereference(Box::new(value)),
        })
    }

    /// The place form of [`Self::lower_tuple_receiver`].
    fn lower_tuple_place_receiver(&mut self, base: &SyntaxNode) -> Option<TypedPlace> {
        let template = self.checked.expression_types.get(&base.span).copied()?;
        let base_type = self.concrete_type(template);
        let Some(target) = self.tuple_pointer_target(base_type) else {
            return self.lower_place(base);
        };
        Some(TypedPlace::Dereference {
            base: Box::new(self.lower_expression(base)?),
            ty: target,
            span: base.span,
        })
    }

    fn tuple_pointer_target(&self, ty: TypeId) -> Option<TypeId> {
        match self.expanded_kind(ty) {
            TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => {
                Some(*target)
            }
            _ => None,
        }
    }

    fn field_pointer_target(&self, ty: TypeId) -> Option<TypeId> {
        match self.expanded_kind(ty) {
            TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => {
                Some(*target)
            }
            _ => None,
        }
    }

    /// The struct a field selection resolves against, seeing through a safe
    /// reference or raw pointer.
    fn field_owner(&self, base_type: TypeId) -> Option<DeclarationId> {
        match self.expanded_kind(base_type) {
            TypeKind::Nominal { identity, .. }
            | TypeKind::Foreign {
                identity,
                complete: true,
            } => Some(identity.declaration),
            TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => {
                match self.expanded_kind(*target) {
                    TypeKind::Nominal { identity, .. }
                    | TypeKind::Foreign {
                        identity,
                        complete: true,
                    } => Some(identity.declaration),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn find_field(&self, declaration: DeclarationId, name: &str) -> Option<FieldId> {
        self.find_member(declaration, name)
            .and_then(|member| match member {
                MemberId::Field(field) => Some(field),
                _ => None,
            })
    }

    fn find_member(&self, declaration: DeclarationId, name: &str) -> Option<MemberId> {
        self.resolved
            .declaration_members
            .get(&declaration)?
            .iter()
            .find_map(|(symbol, member)| {
                (self.resolved.symbol_text(*symbol) == name).then_some(*member)
            })
    }

    fn resolve_enum_variant_expression(
        &self,
        node: &SyntaxNode,
    ) -> Option<(DeclarationId, VariantId)> {
        if node.kind != SyntaxKind::MemberExpression {
            return None;
        }
        let base = child_nodes(node).into_iter().next()?;
        let base_span = callee_target_span(base)?;
        let NameTarget::Item(ItemId::Declaration(declaration)) =
            self.resolved.reference_at(base_span)?.target
        else {
            return None;
        };
        if self.resolved.declarations[declaration.index()].kind != DeclarationKind::Enum {
            return None;
        }
        let member = node.children.iter().rev().find_map(|child| match child {
            SyntaxElement::Token(token) if matches!(token.kind, TokenKind::Identifier(_)) => {
                Some(token)
            }
            _ => None,
        })?;
        match self.find_member(declaration, identifier_text(member)) {
            Some(MemberId::Variant(variant)) => Some((declaration, variant)),
            _ => None,
        }
    }

    fn expanded_kind(&self, mut ty: TypeId) -> &TypeKind {
        loop {
            match self.typed.types.kind(ty) {
                TypeKind::Alias { target, .. } => ty = *target,
                kind => return kind,
            }
        }
    }

    fn closure_capture_index(&self, binding: LocalBindingId) -> Option<usize> {
        let declaration = self.current_instance.as_ref()?.declaration;
        self.checked
            .closures
            .get(&declaration)?
            .captures
            .iter()
            .position(|capture| capture.binding == binding)
    }

    fn closure_instance(&mut self, declaration: DeclarationId) -> FunctionInstance {
        let parameters = self.resolved.declarations[declaration.index()]
            .generic_parameters
            .clone();
        let arguments = parameters
            .into_iter()
            .map(|parameter| {
                let ty = self
                    .typed
                    .types
                    .intern(TypeKind::GenericParameter(parameter));
                self.concrete_type(ty)
            })
            .collect();
        FunctionInstance {
            declaration,
            arguments,
            self_type: None,
        }
    }

    fn lower_closure_capture(
        &mut self,
        capture: &CheckedClosureCapture,
    ) -> Option<TypedExpression> {
        let source_ty = self.concrete_type(capture.source_ty);
        let target_ty = self.concrete_type(capture.ty);
        let source = TypedExpression {
            kind: TypedExpressionKind::Local(capture.source),
            ty: source_ty,
            place: PlaceKind::Addressable,
            copy: matches!(
                capture.kind,
                ClosureCaptureKind::Value
                    | ClosureCaptureKind::SharedRawPointer
                    | ClosureCaptureKind::MutableRawPointer
            ),
            span: capture.span,
        };
        match capture.kind {
            ClosureCaptureKind::Value | ClosureCaptureKind::MutableRawPointer => Some(source),
            ClosureCaptureKind::SharedRawPointer => {
                if source_ty == target_ty {
                    Some(source)
                } else {
                    Some(TypedExpression {
                        kind: TypedExpressionKind::Cast {
                            value: Box::new(source),
                        },
                        ty: target_ty,
                        place: PlaceKind::Value,
                        copy: false,
                        span: capture.span,
                    })
                }
            }
            ClosureCaptureKind::SharedReference | ClosureCaptureKind::MutableReference => {
                Some(TypedExpression {
                    kind: TypedExpressionKind::AddressOf(Box::new(TypedPlace::Local {
                        binding: capture.source,
                        ty: source_ty,
                        span: capture.span,
                    })),
                    ty: target_ty,
                    place: PlaceKind::Value,
                    copy: false,
                    span: capture.span,
                })
            }
        }
    }

    fn unsupported(&mut self, span: Span, feature: &str) {
        self.diagnostics.push(
            Diagnostic::new(
                Category::Lowering,
                format!("{feature} is not supported by the Milestone 8 executable subset"),
            )
            .with_primary(span),
        );
    }
}

fn tuple_field_index(node: &SyntaxNode) -> Option<usize> {
    node.children.iter().find_map(|child| {
        let SyntaxElement::Token(Token {
            kind:
                TokenKind::IntegerLiteral {
                    raw,
                    radix: 10,
                    suffix: None,
                },
            ..
        }) = child
        else {
            return None;
        };
        (raw.len() == 1 || !raw.starts_with('0'))
            .then(|| raw.parse().ok())
            .flatten()
    })
}
