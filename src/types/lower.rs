//! Source-type annotation and declaration lowering.

use super::*;

/// Creates canonical types for declarations, fields, bounds, implementation
/// heads, and function signatures.
#[must_use]
pub fn resolve_types(resolved: &ResolvedProgram) -> TypeOutput {
    TypeBuilder::new(resolved).run()
}

/// Lowers one body annotation through the same implementation used for
/// declaration signatures. Calling this lazily preserves expression-checking
/// type-allocation order while keeping one source-type grammar.
pub fn lower_annotation(
    resolved: &ResolvedProgram,
    typed: &mut TypedProgram,
    node: &SyntaxNode,
    self_type: Option<TypeId>,
) -> (TypeId, Vec<Diagnostic>) {
    if let Some(ty) = typed.annotation_types.get(&node.span).copied() {
        return (
            ty,
            typed
                .annotation_diagnostics
                .get(&node.span)
                .cloned()
                .unwrap_or_default(),
        );
    }

    let program = std::mem::take(typed);
    let mut builder = TypeBuilder::from_program(resolved, program);
    let ty = builder.lower_type(node, self_type);
    let diagnostics = std::mem::take(&mut builder.diagnostics);
    if !diagnostics.is_empty() {
        builder
            .annotation_diagnostics
            .insert(node.span, diagnostics.clone());
    }
    *typed = builder.into_program();
    (ty, diagnostics)
}

struct TypeBuilder<'a> {
    resolved: &'a ResolvedProgram,
    types: TypeContext,
    annotation_types: BTreeMap<Span, TypeId>,
    annotation_diagnostics: BTreeMap<Span, Vec<Diagnostic>>,
    declaration_types: BTreeMap<DeclarationId, TypeId>,
    field_types: BTreeMap<FieldId, TypeId>,
    function_signatures: BTreeMap<DeclarationId, FunctionSignature>,
    function_instance_signatures: BTreeMap<FunctionInstance, FunctionSignature>,
    impl_trait_types: BTreeMap<ImplId, TypeId>,
    impl_target_types: BTreeMap<ImplId, TypeId>,
    obligations: Vec<TraitObligation>,
    ownership_cache: BTreeMap<TypeId, crate::operations::OwnershipFacts>,
    foreign_fields: BTreeMap<DeclarationId, Vec<TypeId>>,
    nominal_fields: BTreeMap<DeclarationId, Vec<TypeId>>,
    nominal_parameters: BTreeMap<DeclarationId, Vec<GenericParameterId>>,
    layout_nominals: BTreeSet<DeclarationId>,
    builtin_layout: BTreeMap<BuiltinId, bool>,
    diagnostics: Vec<Diagnostic>,
    alias_stack: Vec<DeclarationId>,
    reported_alias_cycles: BTreeSet<DeclarationId>,
}

impl<'a> TypeBuilder<'a> {
    fn new(resolved: &'a ResolvedProgram) -> Self {
        Self {
            resolved,
            types: TypeContext::new(),
            annotation_types: BTreeMap::new(),
            annotation_diagnostics: BTreeMap::new(),
            declaration_types: BTreeMap::new(),
            field_types: BTreeMap::new(),
            function_signatures: BTreeMap::new(),
            function_instance_signatures: BTreeMap::new(),
            impl_trait_types: BTreeMap::new(),
            impl_target_types: BTreeMap::new(),
            obligations: Vec::new(),
            ownership_cache: BTreeMap::new(),
            foreign_fields: BTreeMap::new(),
            nominal_fields: BTreeMap::new(),
            nominal_parameters: BTreeMap::new(),
            layout_nominals: BTreeSet::new(),
            builtin_layout: BTreeMap::new(),
            diagnostics: Vec::new(),
            alias_stack: Vec::new(),
            reported_alias_cycles: BTreeSet::new(),
        }
    }

    fn run(mut self) -> TypeOutput {
        for declaration in &self.resolved.declarations {
            let arguments = declaration
                .generic_parameters
                .iter()
                .map(|parameter| self.types.intern(TypeKind::GenericParameter(*parameter)))
                .collect::<Vec<_>>();
            if is_type_declaration(declaration.kind) {
                let ty = self.declaration_application(declaration.id, arguments, declaration.span);
                self.declaration_types.insert(declaration.id, ty);
            }
            self.lower_generic_bounds(declaration.id);
        }

        // Implementation targets must be canonical before method signatures:
        // `Self` in either implementation form denotes the complete target.
        for implementation in &self.resolved.impls {
            self.lower_impl_bounds(implementation.id);
            self.lower_impl(implementation.id);
        }

        for field in &self.resolved.fields {
            let self_type = self.self_type_for_declaration(field.parent_declaration);
            let type_node = if field.syntax.kind == SyntaxKind::Type {
                Some(&field.syntax)
            } else {
                direct_child(&field.syntax, SyntaxKind::Type)
            };
            if let Some(node) = type_node {
                let ty = self.lower_type(node, self_type);
                self.field_types.insert(field.id, ty);
                self.nominal_fields
                    .entry(field.parent_declaration)
                    .or_default()
                    .push(ty);
                if self.resolved.declarations[field.parent_declaration.index()].kind
                    == DeclarationKind::ForeignStruct
                {
                    self.foreign_fields
                        .entry(field.parent_declaration)
                        .or_default()
                        .push(ty);
                }
            }
        }

        for declaration in &self.resolved.declarations {
            if matches!(
                declaration.kind,
                DeclarationKind::Function
                    | DeclarationKind::Closure
                    | DeclarationKind::ForeignFunction
            ) || (declaration.kind == DeclarationKind::Test && declaration.test_selected)
            {
                self.lower_function_signature(declaration.id);
            }
        }

        self.nominal_parameters = self
            .resolved
            .declarations
            .iter()
            .map(|declaration| (declaration.id, declaration.generic_parameters.clone()))
            .collect();
        self.layout_nominals = self
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
        let resolved = self.resolved;
        let mut diagnostics = std::mem::take(&mut self.diagnostics);
        let program = self.into_program();
        diagnostics.extend(validate_foreign_declarations(resolved, &program));
        TypeOutput {
            program,
            diagnostics,
        }
    }

    fn from_program(resolved: &'a ResolvedProgram, program: TypedProgram) -> Self {
        let TypedProgram {
            semantic_revision,
            types,
            annotation_types,
            annotation_diagnostics,
            declaration_types,
            field_types,
            function_signatures,
            function_instance_signatures,
            impl_trait_types,
            impl_target_types,
            obligations,
            function_provenance: _,
            ownership_cache,
            foreign_fields,
            nominal_fields,
            nominal_parameters,
            layout_nominals,
            builtin_layout,
        } = program;
        debug_assert_eq!(semantic_revision, resolved.semantic_revision);
        Self {
            resolved,
            types,
            annotation_types,
            annotation_diagnostics,
            declaration_types,
            field_types,
            function_signatures,
            function_instance_signatures,
            impl_trait_types,
            impl_target_types,
            obligations,
            ownership_cache,
            foreign_fields,
            nominal_fields,
            nominal_parameters,
            layout_nominals,
            builtin_layout,
            diagnostics: Vec::new(),
            alias_stack: Vec::new(),
            reported_alias_cycles: BTreeSet::new(),
        }
    }

    fn into_program(self) -> TypedProgram {
        TypedProgram {
            semantic_revision: self.resolved.semantic_revision,
            types: self.types,
            annotation_types: self.annotation_types,
            annotation_diagnostics: self.annotation_diagnostics,
            declaration_types: self.declaration_types,
            field_types: self.field_types,
            function_signatures: self.function_signatures,
            function_instance_signatures: self.function_instance_signatures,
            impl_trait_types: self.impl_trait_types,
            impl_target_types: self.impl_target_types,
            obligations: self.obligations,
            function_provenance: BTreeMap::new(),
            ownership_cache: self.ownership_cache,
            foreign_fields: self.foreign_fields,
            nominal_fields: self.nominal_fields,
            nominal_parameters: self.nominal_parameters,
            layout_nominals: self.layout_nominals,
            builtin_layout: self.builtin_layout,
        }
    }

    fn declaration_application(
        &mut self,
        declaration: DeclarationId,
        mut arguments: Vec<TypeId>,
        span: Span,
    ) -> TypeId {
        let declared = &self.resolved.declarations[declaration.index()];
        self.check_arity(
            &mut arguments,
            declared.generic_parameters.len(),
            span,
            "type",
        );
        match declared.kind {
            DeclarationKind::TypeAlias => self.lower_alias(declaration, arguments, span),
            DeclarationKind::Struct | DeclarationKind::Enum | DeclarationKind::Trait => {
                let identity = self.nominal_identity(declaration);
                self.types.intern(TypeKind::Nominal {
                    identity,
                    arguments,
                })
            }
            DeclarationKind::ForeignType => {
                let identity = self.nominal_identity(declaration);
                self.types.intern(TypeKind::Foreign {
                    identity,
                    complete: false,
                })
            }
            DeclarationKind::ForeignStruct => {
                let identity = self.nominal_identity(declaration);
                self.types.intern(TypeKind::Foreign {
                    identity,
                    complete: true,
                })
            }
            DeclarationKind::Function
            | DeclarationKind::Closure
            | DeclarationKind::Test
            | DeclarationKind::ForeignFunction => {
                self.diagnostics.push(
                    Diagnostic::new(Category::TypeSystem, "a function name is not a type")
                        .with_primary(span),
                );
                self.types.error()
            }
        }
    }

    fn nominal_identity(&self, declaration: DeclarationId) -> NominalIdentity {
        let declaration_data = &self.resolved.declarations[declaration.index()];
        NominalIdentity {
            package: self.resolved.modules[declaration_data.module.index()]
                .package
                .clone(),
            declaration,
        }
    }

    fn lower_alias(
        &mut self,
        declaration: DeclarationId,
        arguments: Vec<TypeId>,
        use_span: Span,
    ) -> TypeId {
        if let Some(cycle_start) = self
            .alias_stack
            .iter()
            .position(|active| *active == declaration)
        {
            let cycle = &self.alias_stack[cycle_start..];
            let should_report = cycle
                .iter()
                .all(|active| !self.reported_alias_cycles.contains(active));
            self.reported_alias_cycles.extend(cycle.iter().copied());
            if should_report {
                let declared = &self.resolved.declarations[declaration.index()];
                let mut diagnostic = Diagnostic::new(
                    Category::TypeSystem,
                    format!(
                        "transparent type alias `{}` is recursive",
                        self.resolved.symbol_text(declared.name)
                    ),
                )
                .with_primary(use_span);
                for active in cycle {
                    let active = &self.resolved.declarations[active.index()];
                    diagnostic = diagnostic.with_related(active.span, "alias in this cycle");
                }
                self.diagnostics.push(diagnostic);
            }
            return self.types.error();
        }

        let declaration_data = &self.resolved.declarations[declaration.index()];
        let Some(target_node) = alias_target_node(&declaration_data.syntax) else {
            return self.types.error();
        };
        self.alias_stack.push(declaration);
        let template = self.lower_type(target_node, None);
        self.alias_stack.pop();
        let mut substitution = Substitution::new();
        for (parameter, argument) in declaration_data
            .generic_parameters
            .iter()
            .zip(arguments.iter())
        {
            substitution.insert(*parameter, *argument);
        }
        let target = self.types.substitute(template, &substitution);
        self.types.intern(TypeKind::Alias {
            declaration,
            arguments,
            target,
        })
    }

    /// Lowers a type in ordinary value position, where a trait name is invalid.
    fn lower_type(&mut self, node: &SyntaxNode, self_type: Option<TypeId>) -> TypeId {
        let ty = self.lower_type_in(node, self_type, TypePosition::Value);
        self.annotation_types.insert(node.span, ty);
        ty
    }

    /// Lowers a type in a position that names a trait: a generic or impl
    /// bound, or the trait of an `impl Trait for Type`.
    fn lower_trait_type(&mut self, node: &SyntaxNode, self_type: Option<TypeId>) -> TypeId {
        let ty = self.lower_type_in(node, self_type, TypePosition::TraitAllowed);
        self.annotation_types.insert(node.span, ty);
        ty
    }

    fn lower_return_type(&mut self, node: &SyntaxNode, self_type: Option<TypeId>) -> TypeId {
        let ty = self.lower_type_in(node, self_type, TypePosition::Return);
        self.annotation_types.insert(node.span, ty);
        ty
    }

    fn lower_type_in(
        &mut self,
        node: &SyntaxNode,
        self_type: Option<TypeId>,
        position: TypePosition,
    ) -> TypeId {
        let tokens = direct_tokens(node);
        let first = tokens.first().map(|token| &token.kind);
        if matches!(first, Some(TokenKind::Bang)) {
            if position == TypePosition::Return
                && tokens.len() == 1
                && direct_children(node, SyntaxKind::Type).is_empty()
            {
                return self.types.never();
            }
            self.diagnostics.push(
                Diagnostic::new(
                    Category::TypeSystem,
                    "`!` is valid only by itself as a function return type",
                )
                .with_primary(node.span),
            );
            return self.types.error();
        }
        if matches!(first, Some(TokenKind::Amp | TokenKind::Star)) {
            let raw = matches!(first, Some(TokenKind::Star));
            let mutable = tokens
                .iter()
                .any(|token| matches!(token.kind, TokenKind::Keyword(Keyword::Var)));
            let mut target = if let Some(child) = direct_child(node, SyntaxKind::Type) {
                self.lower_type_in(child, self_type, TypePosition::TraitAllowed)
            } else {
                self.types.error()
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
                } = self.types.kind(target).clone()
            {
                target = self.types.intern(TypeKind::Function {
                    safety: Safety::Unsafe,
                    abi,
                    receiver,
                    parameters,
                    return_type,
                });
            }
            // A trait names a type only behind a safe reference, where `&Trait`
            // and `&var Trait` denote a trait object (SPEC 6). Trait names in
            // bound position keep their nominal trait type because bounds do
            // not pass through this form.
            if !raw && self.is_trait_type(target) {
                target = self
                    .types
                    .intern(TypeKind::TraitObject { trait_type: target });
            } else if raw && self.is_trait_type(target) {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::TypeSystem,
                        "a raw pointer cannot address a trait object",
                    )
                    .with_primary(node.span),
                );
                target = self.types.error();
            }
            if mutable && matches!(self.types.kind(target), TypeKind::Function { .. }) {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::TypeSystem,
                        "function pointers and references cannot be mutable",
                    )
                    .with_primary(node.span),
                );
            }
            let mutability = if mutable {
                Mutability::Mutable
            } else {
                Mutability::Shared
            };
            return if raw {
                self.types
                    .intern(TypeKind::RawPointer { mutability, target })
            } else {
                self.types
                    .intern(TypeKind::Reference { mutability, target })
            };
        }
        if matches!(
            first,
            Some(TokenKind::Keyword(Keyword::Fn | Keyword::Unsafe))
        ) {
            if position == TypePosition::Value {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::TypeSystem,
                        "a bare function type has no value representation; use `&fn` or \
                         `&unsafe fn`",
                    )
                    .with_primary(node.span),
                );
                return self.types.error();
            }
            return self.lower_function_type(node);
        }
        if matches!(first, Some(TokenKind::LBracket)) {
            let element = if let Some(child) = direct_child(node, SyntaxKind::Type) {
                self.lower_type(child, self_type)
            } else {
                self.types.error()
            };
            if tokens
                .iter()
                .any(|token| matches!(token.kind, TokenKind::Semicolon))
            {
                let length = direct_child_not(node, SyntaxKind::Type)
                    .and_then(array_length_literal)
                    .unwrap_or_else(|| {
                        self.diagnostics.push(
                            Diagnostic::new(
                                Category::TypeSystem,
                                "array length must be a nonnegative integer constant",
                            )
                            .with_primary(node.span),
                        );
                        0
                    });
                return self.types.intern(TypeKind::Array { element, length });
            }
            let mutable = tokens
                .iter()
                .any(|token| matches!(token.kind, TokenKind::Keyword(Keyword::Var)));
            let mutability = if mutable {
                Mutability::Mutable
            } else {
                Mutability::Shared
            };
            return self.types.intern(TypeKind::Slice {
                mutability,
                element,
            });
        }
        if matches!(first, Some(TokenKind::LParen)) {
            let elements = direct_children(node, SyntaxKind::Type)
                .into_iter()
                .map(|child| self.lower_type(child, self_type))
                .collect::<Vec<_>>();
            if elements.is_empty() {
                return self.types.primitive(PrimitiveType::Unit);
            }
            let has_comma = direct_tokens(node)
                .iter()
                .any(|token| matches!(token.kind, TokenKind::Comma));
            if elements.len() == 1 && !has_comma {
                return elements[0];
            }
            return self.types.intern(TypeKind::Tuple(elements));
        }
        self.lower_path_type(node, self_type, position)
    }

    fn lower_path_type(
        &mut self,
        node: &SyntaxNode,
        self_type: Option<TypeId>,
        position: TypePosition,
    ) -> TypeId {
        let Some(span) = direct_path_span(node) else {
            return self.types.error();
        };
        let Some(reference) = self.resolved.reference_at(span) else {
            return self.types.error();
        };
        let arguments = direct_child(node, SyntaxKind::TypeArguments)
            .map(|arguments| {
                direct_children(arguments, SyntaxKind::Type)
                    .into_iter()
                    .map(|argument| self.lower_type(argument, self_type))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let lowered = match reference.target {
            NameTarget::GenericParameter(parameter) => {
                if !arguments.is_empty() {
                    self.diagnostics.push(
                        Diagnostic::new(
                            Category::TypeSystem,
                            "a generic parameter cannot take type arguments",
                        )
                        .with_primary(node.span),
                    );
                }
                self.types.intern(TypeKind::GenericParameter(parameter))
            }
            NameTarget::SelfType => self_type.unwrap_or_else(|| {
                // Resolution has already diagnosed an out-of-scope `Self`.
                self.types.error()
            }),
            NameTarget::Item(ItemId::Declaration(declaration)) => {
                self.declaration_application(declaration, arguments, node.span)
            }
            NameTarget::Item(ItemId::Builtin(builtin)) => {
                self.builtin_application(builtin, arguments, node.span)
            }
            NameTarget::Item(ItemId::Module(_)) | NameTarget::Local(_) => {
                self.diagnostics.push(
                    Diagnostic::new(Category::TypeSystem, "this name does not denote a type")
                        .with_primary(node.span),
                );
                self.types.error()
            }
        };
        // A trait has no value representation, so a bare trait name is a type
        // only where a trait is expected (SPEC 6). Rejecting it here keeps an
        // unsized type out of fields, parameters, returns, locals, aliases,
        // and generic arguments rather than deferring the failure to lowering.
        if position != TypePosition::TraitAllowed && self.is_trait_type(lowered) {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::TypeSystem,
                    "a trait names a type only behind a safe reference; \
                     write `&Trait` or `&var Trait`",
                )
                .with_primary(node.span),
            );
            return self.types.error();
        }
        lowered
    }

    fn builtin_application(
        &mut self,
        builtin: BuiltinId,
        mut arguments: Vec<TypeId>,
        span: Span,
    ) -> TypeId {
        let name = self.resolved.builtin_name(builtin);
        if let Some(primitive) = primitive_from_name(name) {
            self.check_arity(&mut arguments, 0, span, "primitive type");
            return self.types.primitive(primitive);
        }
        let arity = match name {
            "Vec" | "Set" | "Box" | "Shared" | "Weak" | "Store" | "Handle" | "Identity"
            | "ForeignRoot" | "ForeignRootMut" | "Thread" | "Sender" | "Receiver" | "Mutex" => 1,
            "Map" => 2,
            "print" | "println" | "spawn" | "channel" | "unbounded_channel" => {
                self.diagnostics.push(
                    Diagnostic::new(Category::TypeSystem, "this builtin name is not a type")
                        .with_primary(span),
                );
                return self.types.error();
            }
            _ => 0,
        };
        self.check_arity(&mut arguments, arity, span, "builtin type");
        self.builtin_layout
            .entry(builtin)
            .or_insert_with(|| builtin_has_layout(name));
        self.types.intern(TypeKind::Builtin { builtin, arguments })
    }

    fn lower_function_type(&mut self, node: &SyntaxNode) -> TypeId {
        let direct = direct_tokens(node);
        let safety = if direct
            .iter()
            .any(|token| matches!(token.kind, TokenKind::Keyword(Keyword::Unsafe)))
        {
            Safety::Unsafe
        } else {
            Safety::Safe
        };
        let abi = abi_from_tokens(&direct);
        if let Abi::Unsupported(name) = &abi {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::TypeSystem,
                    format!("unsupported foreign ABI `{name}`; only `C` is supported"),
                )
                .with_primary(node.span),
            );
        }
        let children = direct_children(node, SyntaxKind::Type);
        let mut parameters = Vec::new();
        let return_type = if let Some(result) = children.last() {
            self.lower_return_type(result, None)
        } else {
            self.types.error()
        };
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
                    if type_position < children.len() {
                        parameters.push(FunctionParameter {
                            ty: self.lower_type(child, None),
                            variadic: variadic_next,
                        });
                    }
                    variadic_next = false;
                }
                _ => {}
            }
        }
        self.types.intern(TypeKind::Function {
            safety,
            abi,
            receiver: None,
            parameters,
            return_type,
        })
    }

    fn lower_function_signature(&mut self, declaration: DeclarationId) {
        let declaration_data = &self.resolved.declarations[declaration.index()];
        let self_type = self.self_type_for_declaration(declaration);
        let mut receiver = None;
        let mut parameters = Vec::new();
        if let Some(parameter_list) = direct_child(&declaration_data.syntax, SyntaxKind::Parameters)
        {
            for parameter in direct_children(parameter_list, SyntaxKind::Parameter) {
                let Some(type_node) = direct_child(parameter, SyntaxKind::Type) else {
                    continue;
                };
                let ty = self.lower_type(type_node, self_type);
                let variadic = direct_tokens(parameter)
                    .iter()
                    .any(|token| matches!(token.kind, TokenKind::Ellipsis));
                let is_receiver = direct_tokens(parameter)
                    .iter()
                    .any(|token| matches!(token.kind, TokenKind::Keyword(Keyword::SelfValue)));
                if is_receiver && receiver.is_none() {
                    receiver = Some(ty);
                } else {
                    parameters.push(FunctionParameter { ty, variadic });
                }
            }
        }
        let return_type = if let Some(node) =
            direct_children(&declaration_data.syntax, SyntaxKind::Type).last()
        {
            self.lower_return_type(node, self_type)
        } else if declaration_data.kind == DeclarationKind::Closure {
            self.types.fresh_inference_variable()
        } else {
            self.types.primitive(PrimitiveType::Unit)
        };
        let foreign = declaration_data.kind == DeclarationKind::ForeignFunction;
        let direct = direct_tokens(&declaration_data.syntax);
        let safety = if foreign
            || direct
                .iter()
                .any(|token| matches!(token.kind, TokenKind::Keyword(Keyword::Unsafe)))
        {
            Safety::Unsafe
        } else {
            Safety::Safe
        };
        let abi = if foreign || declaration_data.foreign_binding.is_some() {
            Abi::C
        } else {
            abi_from_tokens(&direct)
        };
        let ty = self.types.intern(TypeKind::Function {
            safety,
            abi,
            receiver,
            parameters: parameters.clone(),
            return_type,
        });
        if let (Some(receiver), Some(self_type)) = (receiver, self_type)
            && !self.valid_receiver_type(receiver, self_type)
        {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::TypeSystem,
                    "a `self` receiver must have type `Self`, `&Self`, `&var Self`, `*Self`, or \
                     `*var Self`",
                )
                .with_primary(declaration_data.span),
            );
        }
        self.function_signatures.insert(
            declaration,
            FunctionSignature {
                ty,
                receiver,
                parameters,
                return_type,
            },
        );
    }

    fn valid_receiver_type(&self, receiver: TypeId, self_type: TypeId) -> bool {
        let receiver = self.types.resolve_inference(receiver);
        if self.types.exactly_equal(receiver, self_type) {
            return true;
        }
        match self.types.kind(receiver) {
            TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => {
                self.types.exactly_equal(*target, self_type)
            }
            _ => false,
        }
    }

    fn lower_generic_bounds(&mut self, declaration: DeclarationId) {
        let declaration_data = &self.resolved.declarations[declaration.index()];
        let Some(parameters) =
            direct_child(&declaration_data.syntax, SyntaxKind::GenericParameters)
        else {
            return;
        };
        for (node, parameter) in direct_children(parameters, SyntaxKind::GenericParameter)
            .into_iter()
            .zip(&declaration_data.generic_parameters)
        {
            for bound in direct_children(node, SyntaxKind::Type) {
                let self_type = self.self_type_for_declaration(declaration);
                let trait_type = self.lower_trait_type(bound, self_type);
                if !self.is_trait_type(trait_type) && trait_type != self.types.error() {
                    self.diagnostics.push(
                        Diagnostic::new(Category::TypeSystem, "generic bounds must name traits")
                            .with_primary(bound.span),
                    );
                }
                self.obligations.push(TraitObligation {
                    parameter: *parameter,
                    trait_type,
                });
            }
        }
    }

    fn lower_impl(&mut self, implementation: ImplId) {
        let data = &self.resolved.impls[implementation.index()];
        let types = direct_children(&data.syntax, SyntaxKind::Type);
        let (trait_node, target_node) = match data.kind {
            crate::resolution::ImplKind::Trait => (types.first().copied(), types.get(1).copied()),
            crate::resolution::ImplKind::Inherent => (None, types.first().copied()),
        };
        if let Some(trait_node) = trait_node {
            let trait_type = self.lower_trait_type(trait_node, None);
            self.impl_trait_types.insert(implementation, trait_type);
        }
        if let Some(target_node) = target_node {
            let target_type = self.lower_type(target_node, None);
            self.impl_target_types.insert(implementation, target_type);
        }
    }

    fn lower_impl_bounds(&mut self, implementation: ImplId) {
        let data = &self.resolved.impls[implementation.index()];
        let Some(parameters) = direct_child(&data.syntax, SyntaxKind::GenericParameters) else {
            return;
        };
        for (node, parameter) in direct_children(parameters, SyntaxKind::GenericParameter)
            .into_iter()
            .zip(&data.generic_parameters)
        {
            for bound in direct_children(node, SyntaxKind::Type) {
                let trait_type = self.lower_trait_type(bound, None);
                if !self.is_trait_type(trait_type) && trait_type != self.types.error() {
                    self.diagnostics.push(
                        Diagnostic::new(Category::TypeSystem, "generic bounds must name traits")
                            .with_primary(bound.span),
                    );
                }
                self.obligations.push(TraitObligation {
                    parameter: *parameter,
                    trait_type,
                });
            }
        }
    }

    fn self_type_for_declaration(&mut self, declaration: DeclarationId) -> Option<TypeId> {
        let data = &self.resolved.declarations[declaration.index()];
        if let Some(parent) = data.parent_declaration {
            let parent_data = &self.resolved.declarations[parent.index()];
            if parent_data.kind == DeclarationKind::Trait {
                return Some(self.types.intern(TypeKind::SelfType(parent)));
            }
            let arguments = parent_data
                .generic_parameters
                .iter()
                .map(|parameter| self.types.intern(TypeKind::GenericParameter(*parameter)))
                .collect();
            return Some(self.declaration_application(parent, arguments, data.span));
        }
        if let Some(implementation) = data.parent_impl {
            if let Some(target) = self.impl_target_types.get(&implementation) {
                return Some(*target);
            }
            let implementation_data = &self.resolved.impls[implementation.index()];
            let nodes = direct_children(&implementation_data.syntax, SyntaxKind::Type);
            let target = match implementation_data.kind {
                crate::resolution::ImplKind::Trait => nodes.get(1),
                crate::resolution::ImplKind::Inherent => nodes.first(),
            };
            if let Some(target) = target {
                return Some(self.lower_type(target, None));
            }
        }
        if matches!(
            data.kind,
            DeclarationKind::Struct | DeclarationKind::Enum | DeclarationKind::ForeignStruct
        ) {
            let arguments = data
                .generic_parameters
                .iter()
                .map(|parameter| self.types.intern(TypeKind::GenericParameter(*parameter)))
                .collect();
            return Some(self.declaration_application(declaration, arguments, data.span));
        }
        None
    }

    fn is_trait_type(&self, mut ty: TypeId) -> bool {
        while let TypeKind::Alias { target, .. } = self.types.kind(ty) {
            ty = *target;
        }
        match self.types.kind(ty) {
            TypeKind::Nominal { identity, .. } => {
                self.resolved.declarations[identity.declaration.index()].kind
                    == DeclarationKind::Trait
            }
            TypeKind::Builtin { builtin, .. } => matches!(
                self.resolved.builtin_name(*builtin),
                "Default"
                    | "PartialEq"
                    | "Eq"
                    | "PartialOrd"
                    | "Ord"
                    | "Hash"
                    | "StableHash"
                    | "Copy"
                    | "Send"
                    | "Sync"
                    | "Display"
            ),
            _ => false,
        }
    }

    fn check_arity(
        &mut self,
        arguments: &mut Vec<TypeId>,
        expected: usize,
        span: Span,
        what: &str,
    ) {
        if arguments.len() == expected {
            return;
        }
        self.diagnostics.push(
            Diagnostic::new(
                Category::TypeSystem,
                format!(
                    "{what} expects {expected} type argument{}, but {} supplied",
                    if expected == 1 { "" } else { "s" },
                    arguments.len()
                ),
            )
            .with_primary(span),
        );
        arguments.truncate(expected);
        while arguments.len() < expected {
            arguments.push(self.types.error());
        }
    }
}
