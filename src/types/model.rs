//! Typed-program facts built on canonical type identities.

use super::*;

/// A declaration's canonical function signature.
#[derive(Debug, Clone)]
pub struct FunctionSignature {
    pub ty: TypeId,
    pub receiver: Option<TypeId>,
    pub parameters: Vec<FunctionParameter>,
    pub return_type: TypeId,
}

/// One concrete (or still-template-relative) named function instantiation.
/// Arguments are ordered as enclosing nominal parameters followed by the
/// function's own generic parameters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FunctionInstance {
    pub declaration: DeclarationId,
    pub arguments: Vec<TypeId>,
    /// The concrete type `Self` denotes, when this instance is a trait's
    /// *default* method body specialized for one implementing type. A default
    /// body is declared once against `Self`, so it monomorphizes per
    /// implementing type exactly as a generic function does over its
    /// parameters. `None` for every other callable.
    pub self_type: Option<TypeId>,
}

/// Whether a syntactic type position may name a trait.
///
/// A trait has no value representation, so it names a type only as the target
/// of a safe reference (`&Trait`), as a generic or impl bound, or as the trait
/// of an `impl Trait for Type` (`SPEC.md` 6). Every other position is a value
/// position where a bare trait name is an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TypePosition {
    Value,
    TraitAllowed,
}

/// One declared generic capability requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraitObligation {
    pub parameter: GenericParameterId,
    pub trait_type: TypeId,
}

/// Canonical type data derived from a resolved program.
pub struct TypedProgram {
    pub types: TypeContext,
    /// Canonical type selected for every accepted source `Type` node,
    /// including annotations inside function bodies.
    pub annotation_types: BTreeMap<Span, TypeId>,
    /// Type diagnostics for body annotations remain emitted when body
    /// checking reaches that annotation, preserving public diagnostic order
    /// while keeping source-type lowering centralized here.
    pub annotation_diagnostics: BTreeMap<Span, Vec<Diagnostic>>,
    pub declaration_types: BTreeMap<DeclarationId, TypeId>,
    pub field_types: BTreeMap<FieldId, TypeId>,
    pub function_signatures: BTreeMap<DeclarationId, FunctionSignature>,
    pub function_instance_signatures: BTreeMap<FunctionInstance, FunctionSignature>,
    pub impl_trait_types: BTreeMap<ImplId, TypeId>,
    pub impl_target_types: BTreeMap<ImplId, TypeId>,
    pub obligations: Vec<TraitObligation>,
    pub(super) foreign_fields: BTreeMap<DeclarationId, Vec<TypeId>>,
    pub(super) nominal_fields: BTreeMap<DeclarationId, Vec<TypeId>>,
    pub(super) nominal_parameters: BTreeMap<DeclarationId, Vec<GenericParameterId>>,
    pub(super) layout_nominals: BTreeSet<DeclarationId>,
    pub(super) builtin_layout: BTreeMap<BuiltinId, bool>,
}

impl Default for TypedProgram {
    fn default() -> Self {
        Self {
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
            foreign_fields: BTreeMap::new(),
            nominal_fields: BTreeMap::new(),
            nominal_parameters: BTreeMap::new(),
            layout_nominals: BTreeSet::new(),
            builtin_layout: BTreeMap::new(),
        }
    }
}

/// Result of canonicalizing every signature in a resolved program.
pub struct TypeOutput {
    pub program: TypedProgram,
    pub diagnostics: Vec<Diagnostic>,
}
impl TypedProgram {
    /// Deterministic canonical-type and signature dump for developer tooling.
    #[must_use]
    pub fn dump(&self) -> String {
        let mut output = String::new();
        for index in 0..self.types.len() {
            let ty = TypeId(index as u32);
            output.push_str(&format!("type {index} {:?}\n", self.types.kind(ty)));
        }
        for (declaration, ty) in &self.declaration_types {
            output.push_str(&format!(
                "declaration {} type {}\n",
                declaration.index(),
                ty.index()
            ));
        }
        for (declaration, signature) in &self.function_signatures {
            output.push_str(&format!(
                "signature {} {:?}\n",
                declaration.index(),
                signature
            ));
        }
        for obligation in &self.obligations {
            output.push_str(&format!("obligation {obligation:?}\n"));
        }
        output
    }

    #[must_use]
    pub fn callable_generic_parameters(
        &self,
        resolved: &ResolvedProgram,
        declaration: DeclarationId,
    ) -> Vec<GenericParameterId> {
        let data = &resolved.declarations[declaration.index()];
        let mut parameters = if let Some(parent) = data.parent_declaration {
            resolved.declarations[parent.index()]
                .generic_parameters
                .clone()
        } else if let Some(implementation) = data.parent_impl {
            resolved.impls[implementation.index()]
                .generic_parameters
                .clone()
        } else {
            Vec::new()
        };
        parameters.extend(data.generic_parameters.iter().copied());
        parameters
    }

    #[must_use]
    pub fn instance_substitution(
        &self,
        resolved: &ResolvedProgram,
        instance: &FunctionInstance,
    ) -> Substitution {
        let mut substitution = Substitution::new();
        for (parameter, argument) in self
            .callable_generic_parameters(resolved, instance.declaration)
            .into_iter()
            .zip(instance.arguments.iter().copied())
        {
            substitution.insert(parameter, argument);
        }
        substitution
    }

    pub fn instantiate_signature(
        &mut self,
        resolved: &ResolvedProgram,
        instance: &FunctionInstance,
    ) -> Option<FunctionSignature> {
        if let Some(signature) = self.function_instance_signatures.get(instance) {
            return Some(signature.clone());
        }
        let mut template = self.function_signatures.get(&instance.declaration)?.clone();
        if let Some(self_type) = instance.self_type {
            template = FunctionSignature {
                ty: template.ty,
                receiver: template
                    .receiver
                    .map(|receiver| self.types.substitute_self(receiver, self_type)),
                parameters: template
                    .parameters
                    .iter()
                    .map(|parameter| FunctionParameter {
                        ty: self.types.substitute_self(parameter.ty, self_type),
                        variadic: parameter.variadic,
                    })
                    .collect(),
                return_type: self.types.substitute_self(template.return_type, self_type),
            };
        }
        let substitution = self.instance_substitution(resolved, instance);
        let receiver = template
            .receiver
            .map(|receiver| self.types.substitute(receiver, &substitution));
        let parameters = template
            .parameters
            .iter()
            .map(|parameter| FunctionParameter {
                ty: self.types.substitute(parameter.ty, &substitution),
                variadic: parameter.variadic,
            })
            .collect::<Vec<_>>();
        let return_type = self.types.substitute(template.return_type, &substitution);
        let TypeKind::Function { safety, abi, .. } = self.types.kind(template.ty).clone() else {
            return None;
        };
        let ty = self.types.intern(TypeKind::Function {
            safety,
            abi,
            receiver,
            parameters: parameters.clone(),
            return_type,
        });
        let signature = FunctionSignature {
            ty,
            receiver,
            parameters,
            return_type,
        };
        self.function_instance_signatures
            .insert(instance.clone(), signature.clone());
        Some(signature)
    }

    pub fn instantiate_declaration_type(
        &mut self,
        resolved: &ResolvedProgram,
        declaration: DeclarationId,
        arguments: &[TypeId],
    ) -> Option<TypeId> {
        let parameters = &resolved.declarations[declaration.index()].generic_parameters;
        if parameters.len() != arguments.len() {
            return None;
        }
        let template = *self.declaration_types.get(&declaration)?;
        let mut substitution = Substitution::new();
        for (parameter, argument) in parameters.iter().zip(arguments.iter()) {
            substitution.insert(*parameter, *argument);
        }
        Some(self.types.substitute(template, &substitution))
    }

    pub fn instantiate_field_type(
        &mut self,
        resolved: &ResolvedProgram,
        field: FieldId,
        owner_arguments: &[TypeId],
    ) -> Option<TypeId> {
        let template = *self.field_types.get(&field)?;
        let owner = resolved.fields[field.index()].parent_declaration;
        let parameters = &resolved.declarations[owner.index()].generic_parameters;
        if parameters.len() != owner_arguments.len() {
            return None;
        }
        let mut substitution = Substitution::new();
        for (parameter, argument) in parameters.iter().zip(owner_arguments.iter()) {
            substitution.insert(*parameter, *argument);
        }
        Some(self.types.substitute(template, &substitution))
    }

    pub fn obligations_for(
        &self,
        parameter: GenericParameterId,
    ) -> impl Iterator<Item = &TraitObligation> {
        self.obligations
            .iter()
            .filter(move |obligation| obligation.parameter == parameter)
    }

    /// Returns whether layout is computable once a concrete target pointer
    /// width is selected. Opaque foreign and still-parametric types are false.
    #[must_use]
    pub fn layout_available(&self, ty: TypeId, pointer_bits: u8) -> bool {
        matches!(pointer_bits, 32 | 64)
            && self.layout_inner(ty, pointer_bits, &BTreeMap::new(), &mut BTreeSet::new())
    }

    fn layout_inner(
        &self,
        ty: TypeId,
        pointer_bits: u8,
        substitution: &BTreeMap<GenericParameterId, TypeId>,
        visiting: &mut BTreeSet<TypeId>,
    ) -> bool {
        let ty = self.types.resolve_inference(ty);
        if let TypeKind::GenericParameter(parameter) = self.types.kind(ty)
            && let Some(replacement) = substitution.get(parameter)
        {
            return self.layout_inner(*replacement, pointer_bits, substitution, visiting);
        }
        if !visiting.insert(ty) {
            return false;
        }
        let available = match self.types.kind(ty) {
            TypeKind::Error
            | TypeKind::GenericParameter(_)
            | TypeKind::SelfType(_)
            | TypeKind::InferenceVariable(_)
            | TypeKind::TraitObject { .. }
            | TypeKind::Function { .. } => false,
            TypeKind::Foreign {
                complete: false, ..
            } => false,
            TypeKind::Primitive(_) | TypeKind::RawPointer { .. } => true,
            TypeKind::Reference { .. } => pointer_bits == 32 || pointer_bits == 64,
            TypeKind::Tuple(elements) => elements
                .iter()
                .all(|element| self.layout_inner(*element, pointer_bits, substitution, visiting)),
            TypeKind::Array { element, length } => {
                let max = if pointer_bits == 32 {
                    u32::MAX as u128
                } else {
                    u64::MAX as u128
                };
                *length <= max && self.layout_inner(*element, pointer_bits, substitution, visiting)
            }
            TypeKind::Slice(element) => {
                self.layout_inner(*element, pointer_bits, substitution, visiting)
            }
            TypeKind::Nominal {
                identity,
                arguments,
            } => {
                if !self.layout_nominals.contains(&identity.declaration) {
                    visiting.remove(&ty);
                    return false;
                }
                let mut nested = substitution.clone();
                if let Some(parameters) = self.nominal_parameters.get(&identity.declaration) {
                    for (parameter, argument) in parameters.iter().zip(arguments) {
                        nested.insert(*parameter, *argument);
                    }
                }
                self.nominal_fields
                    .get(&identity.declaration)
                    .into_iter()
                    .flatten()
                    .all(|field| self.layout_inner(*field, pointer_bits, &nested, visiting))
            }
            TypeKind::Builtin { builtin, arguments } => {
                self.builtin_layout.get(builtin).copied().unwrap_or(false)
                    && arguments.iter().all(|argument| {
                        self.layout_inner(*argument, pointer_bits, substitution, visiting)
                    })
            }
            TypeKind::Foreign {
                identity,
                complete: true,
            } => self
                .foreign_fields
                .get(&identity.declaration)
                .is_some_and(|fields| {
                    fields.iter().all(|field| {
                        self.layout_inner(*field, pointer_bits, substitution, visiting)
                    })
                }),
            TypeKind::Alias { target, .. } => {
                self.layout_inner(*target, pointer_bits, substitution, visiting)
            }
        };
        visiting.remove(&ty);
        available
    }

    /// C ABI safety for a parameter or field position. Unit is intentionally
    /// excluded; callers may permit it specially as a return type.
    #[must_use]
    pub fn is_abi_safe(&self, ty: TypeId) -> bool {
        self.abi_safe_inner(ty, &mut BTreeSet::new())
    }

    fn abi_safe_inner(&self, ty: TypeId, visiting: &mut BTreeSet<TypeId>) -> bool {
        let ty = self.types.resolve_inference(ty);
        if !visiting.insert(ty) {
            return false;
        }
        let safe = match self.types.kind(ty) {
            TypeKind::Primitive(
                PrimitiveType::I8
                | PrimitiveType::I16
                | PrimitiveType::I32
                | PrimitiveType::I64
                | PrimitiveType::Isize
                | PrimitiveType::U8
                | PrimitiveType::U16
                | PrimitiveType::U32
                | PrimitiveType::U64
                | PrimitiveType::Usize
                | PrimitiveType::F32
                | PrimitiveType::F64,
            ) => true,
            TypeKind::RawPointer { target, .. } => {
                let target = self.types.resolve_inference(*target);
                match self.types.kind(target) {
                    TypeKind::Function {
                        parameters,
                        return_type,
                        ..
                    } => {
                        parameters.iter().all(|parameter| {
                            !parameter.variadic && self.abi_safe_inner(parameter.ty, visiting)
                        }) && (matches!(
                            self.types.kind(*return_type),
                            TypeKind::Primitive(PrimitiveType::Unit)
                        ) || self.abi_safe_inner(*return_type, visiting))
                    }
                    _ => true,
                }
            }
            TypeKind::Foreign {
                identity,
                complete: true,
            } => self
                .foreign_fields
                .get(&identity.declaration)
                .is_some_and(|fields| {
                    fields
                        .iter()
                        .all(|field| self.abi_safe_inner(*field, visiting))
                }),
            TypeKind::Alias { target, .. } => self.abi_safe_inner(*target, visiting),
            _ => false,
        };
        visiting.remove(&ty);
        safe
    }
}

pub(super) fn validate_foreign_declarations(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for declaration in &resolved.declarations {
        let Some(binding) = &declaration.foreign_binding else {
            continue;
        };
        if !declaration.generic_parameters.is_empty() {
            diagnostics.push(
                Diagnostic::new(
                    Category::TypeSystem,
                    "an imported or exported C declaration cannot be generic",
                )
                .with_primary(declaration.span),
            );
        }
        match declaration.kind {
            DeclarationKind::ForeignType => {}
            DeclarationKind::ForeignStruct => {
                if direct_child(&declaration.syntax, SyntaxKind::DeriveList).is_some() {
                    diagnostics.push(
                        Diagnostic::new(
                            Category::TypeSystem,
                            "an imported C struct cannot derive traits",
                        )
                        .with_primary(declaration.span),
                    );
                }
                for field in resolved
                    .fields
                    .iter()
                    .filter(|field| field.parent_declaration == declaration.id)
                {
                    if typed
                        .field_types
                        .get(&field.id)
                        .is_some_and(|ty| !typed.is_abi_safe(*ty))
                    {
                        diagnostics.push(
                            Diagnostic::new(
                                Category::TypeSystem,
                                "an imported C struct field must have an ABI-safe type",
                            )
                            .with_primary(field.span),
                        );
                    }
                }
            }
            DeclarationKind::ForeignFunction | DeclarationKind::Function => {
                let Some(signature) = typed.function_signatures.get(&declaration.id) else {
                    continue;
                };
                if signature.receiver.is_some() {
                    diagnostics.push(
                        Diagnostic::new(
                            Category::TypeSystem,
                            "an imported or exported C function cannot have a receiver",
                        )
                        .with_primary(declaration.span),
                    );
                }
                for parameter in &signature.parameters {
                    if parameter.variadic {
                        diagnostics.push(
                            Diagnostic::new(
                                Category::TypeSystem,
                                "C variadic functions are not supported",
                            )
                            .with_primary(declaration.span),
                        );
                    }
                    if !typed.is_abi_safe(parameter.ty) {
                        diagnostics.push(
                            Diagnostic::new(
                                Category::TypeSystem,
                                "a C function parameter must have an ABI-safe type",
                            )
                            .with_primary(declaration.span),
                        );
                    }
                }
                let unit = matches!(
                    typed.types.kind(signature.return_type),
                    TypeKind::Primitive(PrimitiveType::Unit)
                );
                if !unit && !typed.is_abi_safe(signature.return_type) {
                    diagnostics.push(
                        Diagnostic::new(
                            Category::TypeSystem,
                            "a C function result must be unit or an ABI-safe type",
                        )
                        .with_primary(declaration.span),
                    );
                }
                if binding.direction == ForeignDirection::Export
                    && declaration.kind != DeclarationKind::Function
                {
                    diagnostics.push(
                        Diagnostic::new(
                            Category::TypeSystem,
                            "`@exportc` requires an Elamite function definition",
                        )
                        .with_primary(declaration.span),
                    );
                }
            }
            _ => diagnostics.push(
                Diagnostic::new(
                    Category::TypeSystem,
                    "this declaration kind cannot carry an FFI attribute",
                )
                .with_primary(declaration.span),
            ),
        }
    }
    diagnostics
}

/// Expression storage classification consumed by Milestone 6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceKind {
    Value,
    Addressable,
    Mutable,
    CollectionInterior,
    /// The target of a `*var T` dereference: assignable in an `unsafe`
    /// context, but never the source of a safe reference — the sanctioned
    /// path from a raw pointer to a reference is the explicit, asserted
    /// `as &var T` conversion (`SPEC.md` 3.3), not `&var *pointer`.
    RawPointerTarget,
}

impl PlaceKind {
    #[must_use]
    pub fn is_addressable(self) -> bool {
        matches!(self, Self::Addressable | Self::Mutable)
    }

    #[must_use]
    pub fn is_mutable(self) -> bool {
        matches!(
            self,
            Self::Mutable | Self::CollectionInterior | Self::RawPointerTarget
        )
    }

    #[must_use]
    pub fn permits_safe_reference(self) -> bool {
        matches!(self, Self::Addressable | Self::Mutable)
    }
}
