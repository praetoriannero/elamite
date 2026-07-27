//! Canonical types, transparent aliases, substitutions, and literal inference.
//!
//! This module is the Milestone 5 boundary between name resolution and
//! expression checking. Every constructed type is interned in an owned arena;
//! source aliases remain visible in that arena for diagnostics, while exact
//! equivalence recursively compares their expanded targets.

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostics::{Category, Diagnostic};
use crate::lexer::{Keyword, NumericSuffix, Token, TokenKind};
use crate::package::PackageId;
use crate::parser::{SyntaxElement, SyntaxKind, SyntaxNode};
use crate::resolution::{
    BuiltinId, DeclarationId, DeclarationKind, FieldId, GenericParameterId, ImplId, ItemId,
    NameTarget, ResolvedProgram,
};
use crate::source::Span;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u32);

        impl $name {
            #[must_use]
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

id_type!(TypeId);
id_type!(InferenceVariableId);

/// Concrete primitive and string types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PrimitiveType {
    Unit,
    Bool,
    Char,
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
    F32,
    F64,
    Str,
    String,
}

impl PrimitiveType {
    #[must_use]
    pub fn is_integer(self) -> bool {
        matches!(
            self,
            Self::I8
                | Self::I16
                | Self::I32
                | Self::I64
                | Self::I128
                | Self::Isize
                | Self::U8
                | Self::U16
                | Self::U32
                | Self::U64
                | Self::U128
                | Self::Usize
        )
    }

    #[must_use]
    pub fn is_float(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }
}

/// Whether an indirection permits writes through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mutability {
    Shared,
    Mutable,
}

/// Whether invoking a function requires an unsafe context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Safety {
    Safe,
    Unsafe,
}

/// Calling convention carried by a function type.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Abi {
    Elamite,
    C,
    Unsupported(String),
}

/// One parameter in a canonical function signature.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FunctionParameter {
    pub ty: TypeId,
    pub variadic: bool,
}

/// Stable nominal identity. Package identity is retained explicitly even
/// though declaration IDs are also stable within one resolved program.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NominalIdentity {
    pub package: Option<PackageId>,
    pub declaration: DeclarationId,
}

/// The complete set of canonical type forms.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TypeKind {
    Error,
    Primitive(PrimitiveType),
    Nominal {
        identity: NominalIdentity,
        arguments: Vec<TypeId>,
    },
    Builtin {
        builtin: BuiltinId,
        arguments: Vec<TypeId>,
    },
    Tuple(Vec<TypeId>),
    Array {
        element: TypeId,
        length: u128,
    },
    Slice(TypeId),
    Reference {
        mutability: Mutability,
        target: TypeId,
    },
    RawPointer {
        mutability: Mutability,
        target: TypeId,
    },
    Function {
        safety: Safety,
        abi: Abi,
        receiver: Option<TypeId>,
        parameters: Vec<FunctionParameter>,
        return_type: TypeId,
    },
    TraitObject {
        trait_type: TypeId,
    },
    Foreign {
        identity: NominalIdentity,
        complete: bool,
    },
    Alias {
        declaration: DeclarationId,
        arguments: Vec<TypeId>,
        target: TypeId,
    },
    GenericParameter(GenericParameterId),
    SelfType(DeclarationId),
    InferenceVariable(InferenceVariableId),
}

/// Exact generic-parameter replacement used by later instantiation passes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Substitution {
    entries: BTreeMap<GenericParameterId, TypeId>,
}

impl Substitution {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, parameter: GenericParameterId, ty: TypeId) -> Option<TypeId> {
        self.entries.insert(parameter, ty)
    }

    #[must_use]
    pub fn get(&self, parameter: GenericParameterId) -> Option<TypeId> {
        self.entries.get(&parameter).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (GenericParameterId, TypeId)> + '_ {
        self.entries.iter().map(|(parameter, ty)| (*parameter, *ty))
    }
}

/// Optional contextual type supplied to literal inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedType {
    None,
    Exact(TypeId),
}

/// A stable literal-materialization failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiteralTypeError {
    pub message: String,
}

/// A strict-unification failure. Elamite has no conversion or variance step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeMismatch {
    pub left: TypeId,
    pub right: TypeId,
}

/// Owned canonical type arena and inference-variable bindings.
pub struct TypeContext {
    kinds: Vec<TypeKind>,
    interned: BTreeMap<TypeKind, TypeId>,
    inference_bindings: Vec<Option<TypeId>>,
}

impl Default for TypeContext {
    fn default() -> Self {
        let mut context = Self {
            kinds: Vec::new(),
            interned: BTreeMap::new(),
            inference_bindings: Vec::new(),
        };
        context.intern(TypeKind::Error);
        for primitive in [
            PrimitiveType::Unit,
            PrimitiveType::Bool,
            PrimitiveType::Char,
            PrimitiveType::I8,
            PrimitiveType::I16,
            PrimitiveType::I32,
            PrimitiveType::I64,
            PrimitiveType::I128,
            PrimitiveType::Isize,
            PrimitiveType::U8,
            PrimitiveType::U16,
            PrimitiveType::U32,
            PrimitiveType::U64,
            PrimitiveType::U128,
            PrimitiveType::Usize,
            PrimitiveType::F32,
            PrimitiveType::F64,
            PrimitiveType::Str,
            PrimitiveType::String,
        ] {
            context.intern(TypeKind::Primitive(primitive));
        }
        context
    }
}

impl TypeContext {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    #[must_use]
    pub fn kind(&self, ty: TypeId) -> &TypeKind {
        &self.kinds[ty.index()]
    }

    #[must_use]
    pub fn id_at(&self, index: usize) -> Option<TypeId> {
        (index < self.kinds.len())
            .then(|| TypeId(u32::try_from(index).expect("canonical type index fits in u32")))
    }

    /// Returns an already-interned canonical type without mutating the arena.
    /// Lowering uses this for derived signature representations, such as the
    /// slice bound to a homogeneous variadic parameter.
    #[must_use]
    pub fn id_for_kind(&self, kind: &TypeKind) -> Option<TypeId> {
        self.interned.get(kind).copied()
    }

    pub fn intern(&mut self, kind: TypeKind) -> TypeId {
        if let Some(existing) = self.interned.get(&kind) {
            return *existing;
        }
        let id = TypeId(u32::try_from(self.kinds.len()).expect("too many canonical types"));
        self.kinds.push(kind.clone());
        self.interned.insert(kind, id);
        id
    }

    #[must_use]
    pub fn error(&self) -> TypeId {
        TypeId(0)
    }

    pub fn primitive(&mut self, primitive: PrimitiveType) -> TypeId {
        self.intern(TypeKind::Primitive(primitive))
    }

    /// Returns the canonical identity of a primitive installed when the type
    /// context was created, without requiring mutable access during lowering.
    #[must_use]
    pub fn primitive_id(&self, primitive: PrimitiveType) -> TypeId {
        *self
            .interned
            .get(&TypeKind::Primitive(primitive))
            .expect("every primitive type is pre-interned")
    }

    pub fn fresh_inference_variable(&mut self) -> TypeId {
        let variable = InferenceVariableId(
            u32::try_from(self.inference_bindings.len()).expect("too many inference variables"),
        );
        self.inference_bindings.push(None);
        self.intern(TypeKind::InferenceVariable(variable))
    }

    /// Resolves a chain of inference bindings, without erasing aliases.
    #[must_use]
    pub fn resolve_inference(&self, mut ty: TypeId) -> TypeId {
        let mut seen = BTreeSet::new();
        while let TypeKind::InferenceVariable(variable) = self.kind(ty) {
            if !seen.insert(*variable) {
                return self.error();
            }
            let Some(bound) = self.inference_bindings[variable.index()] else {
                break;
            };
            ty = bound;
        }
        ty
    }

    /// Strictly unifies two types. Only inference variables and transparent
    /// alias expansion can change the comparison; concrete types never coerce.
    pub fn unify(&mut self, left: TypeId, right: TypeId) -> Result<TypeId, TypeMismatch> {
        let left = self.resolve_inference(left);
        let right = self.resolve_inference(right);
        if left == right || left == self.error() || right == self.error() {
            return Ok(if left == self.error() { right } else { left });
        }
        if let TypeKind::Alias { target, .. } = self.kind(left) {
            return self.unify(*target, right);
        }
        if let TypeKind::Alias { target, .. } = self.kind(right) {
            return self.unify(left, *target);
        }
        if let TypeKind::InferenceVariable(variable) = self.kind(left) {
            let variable = *variable;
            if self.contains_inference(right, variable, &mut BTreeSet::new()) {
                return Err(TypeMismatch { left, right });
            }
            self.inference_bindings[variable.index()] = Some(right);
            return Ok(right);
        }
        if let TypeKind::InferenceVariable(variable) = self.kind(right) {
            let variable = *variable;
            if self.contains_inference(left, variable, &mut BTreeSet::new()) {
                return Err(TypeMismatch { left, right });
            }
            self.inference_bindings[variable.index()] = Some(left);
            return Ok(left);
        }

        let left_kind = self.kind(left).clone();
        let right_kind = self.kind(right).clone();
        if !same_outer_shape(&left_kind, &right_kind) {
            return Err(TypeMismatch { left, right });
        }
        for (left_child, right_child) in paired_children(&left_kind, &right_kind) {
            self.unify(left_child, right_child)?;
        }
        Ok(left)
    }

    #[must_use]
    pub fn exactly_equal(&self, left: TypeId, right: TypeId) -> bool {
        self.equal_inner(left, right, &mut BTreeSet::new())
    }

    fn equal_inner(
        &self,
        left: TypeId,
        right: TypeId,
        seen: &mut BTreeSet<(TypeId, TypeId)>,
    ) -> bool {
        let left = self.resolve_inference(left);
        let right = self.resolve_inference(right);
        if left == right {
            return true;
        }
        if !seen.insert((left, right)) {
            return true;
        }
        if let TypeKind::Alias { target, .. } = self.kind(left) {
            return self.equal_inner(*target, right, seen);
        }
        if let TypeKind::Alias { target, .. } = self.kind(right) {
            return self.equal_inner(left, *target, seen);
        }
        let left_kind = self.kind(left);
        let right_kind = self.kind(right);
        same_outer_shape(left_kind, right_kind)
            && paired_children(left_kind, right_kind)
                .into_iter()
                .all(|(left, right)| self.equal_inner(left, right, seen))
    }

    /// Replaces `Self` with a concrete implementation target.
    ///
    /// A trait method's declared signature mentions `Self`, so comparing it
    /// with an implementation's signature requires rewriting `Self` to the
    /// implemented type first.
    pub fn substitute_self(&mut self, ty: TypeId, target: TypeId) -> TypeId {
        let ty = self.resolve_inference(ty);
        if matches!(self.kind(ty), TypeKind::SelfType(_)) {
            return target;
        }
        let kind = self.kind(ty).clone();
        let replaced = map_type_children(kind, |child| self.substitute_self(child, target));
        self.intern(replaced)
    }

    pub fn substitute(&mut self, ty: TypeId, substitution: &Substitution) -> TypeId {
        let ty = self.resolve_inference(ty);
        if let TypeKind::GenericParameter(parameter) = self.kind(ty) {
            return substitution.get(*parameter).unwrap_or(ty);
        }
        let kind = self.kind(ty).clone();
        let replaced = map_type_children(kind, |child| self.substitute(child, substitution));
        self.intern(replaced)
    }

    /// Whether `ty` mentions `Self` anywhere, used by object-safety checking.
    #[must_use]
    pub fn mentions_self(&self, ty: TypeId) -> bool {
        self.any_type(ty, &mut BTreeSet::new(), &|kind| {
            matches!(kind, TypeKind::SelfType(_))
        })
    }

    pub fn contains_explicit_alias(&self, ty: TypeId) -> bool {
        self.any_type(ty, &mut BTreeSet::new(), &|kind| {
            matches!(kind, TypeKind::Alias { .. })
        })
    }

    #[must_use]
    pub fn contains_managed_reference(&self, ty: TypeId) -> bool {
        self.any_type(ty, &mut BTreeSet::new(), &|kind| {
            matches!(
                kind,
                TypeKind::Reference { target, .. }
                    if !matches!(self.kind(*target), TypeKind::Function { .. })
            )
        })
    }

    #[must_use]
    pub fn contains_generic_parameter(&self, ty: TypeId) -> bool {
        self.any_type(ty, &mut BTreeSet::new(), &|kind| {
            matches!(
                kind,
                TypeKind::GenericParameter(_)
                    | TypeKind::SelfType(_)
                    | TypeKind::InferenceVariable(_)
            )
        })
    }

    #[must_use]
    pub fn contains_type(&self, outer: TypeId, needle: TypeId) -> bool {
        fn visit(
            types: &TypeContext,
            outer: TypeId,
            needle: TypeId,
            seen: &mut BTreeSet<TypeId>,
        ) -> bool {
            let outer = types.resolve_inference(outer);
            if outer == needle {
                return true;
            }
            seen.insert(outer)
                && type_children(types.kind(outer))
                    .into_iter()
                    .any(|child| visit(types, child, needle, seen))
        }
        visit(
            self,
            outer,
            self.resolve_inference(needle),
            &mut BTreeSet::new(),
        )
    }

    #[must_use]
    pub fn contains_mutable_indirection(&self, ty: TypeId) -> bool {
        self.any_type(ty, &mut BTreeSet::new(), &|kind| {
            matches!(
                kind,
                TypeKind::Reference {
                    mutability: Mutability::Mutable,
                    ..
                } | TypeKind::RawPointer {
                    mutability: Mutability::Mutable,
                    ..
                }
            )
        })
    }

    fn any_type(
        &self,
        ty: TypeId,
        seen: &mut BTreeSet<TypeId>,
        predicate: &impl Fn(&TypeKind) -> bool,
    ) -> bool {
        let ty = self.resolve_inference(ty);
        if !seen.insert(ty) {
            return false;
        }
        let kind = self.kind(ty);
        predicate(kind)
            || type_children(kind)
                .into_iter()
                .any(|child| self.any_type(child, seen, predicate))
    }

    fn contains_inference(
        &self,
        ty: TypeId,
        needle: InferenceVariableId,
        seen: &mut BTreeSet<TypeId>,
    ) -> bool {
        let ty = self.resolve_inference(ty);
        if !seen.insert(ty) {
            return false;
        }
        match self.kind(ty) {
            TypeKind::InferenceVariable(variable) => *variable == needle,
            kind => type_children(kind)
                .into_iter()
                .any(|child| self.contains_inference(child, needle, seen)),
        }
    }

    /// Materializes an integer literal using its suffix, contextual type, or
    /// the required `i32` default, in that order.
    pub fn materialize_integer(
        &mut self,
        raw: &str,
        radix: u8,
        suffix: Option<NumericSuffix>,
        negative: bool,
        expected: ExpectedType,
        pointer_bits: u8,
    ) -> Result<TypeId, LiteralTypeError> {
        let primitive = suffix
            .map(primitive_for_suffix)
            .or_else(|| self.expected_numeric(expected, true))
            .unwrap_or(PrimitiveType::I32);
        if !(primitive.is_integer() || primitive.is_float()) {
            return Err(literal_error(
                "integer literal requires a numeric expected type",
            ));
        }
        let magnitude = parse_integer_magnitude(raw, radix, suffix)?;
        if primitive.is_integer() {
            if !integer_fits(primitive, magnitude, negative, pointer_bits) {
                return Err(literal_error(format!(
                    "integer literal is out of range for `{}`",
                    primitive_name(primitive)
                )));
            }
        } else {
            let signed = if negative {
                -(magnitude as f64)
            } else {
                magnitude as f64
            };
            let representable = match primitive {
                PrimitiveType::F32 => (signed as f32).is_finite(),
                PrimitiveType::F64 => signed.is_finite(),
                _ => unreachable!("primitive was checked as a float"),
            };
            if !representable {
                return Err(literal_error(format!(
                    "integer literal is out of range for `{}`",
                    primitive_name(primitive)
                )));
            }
        }
        Ok(self.primitive(primitive))
    }

    /// Materializes a floating literal using its suffix, contextual floating
    /// type, or the required `f64` default.
    pub fn materialize_float(
        &mut self,
        raw: &str,
        suffix: Option<NumericSuffix>,
        expected: ExpectedType,
    ) -> Result<TypeId, LiteralTypeError> {
        let primitive = suffix
            .map(primitive_for_suffix)
            .or_else(|| self.expected_numeric(expected, false))
            .unwrap_or(PrimitiveType::F64);
        if !primitive.is_float() {
            return Err(literal_error(
                "floating literal requires an `f32` or `f64` expected type",
            ));
        }
        let suffix_text = suffix.map(numeric_suffix_text).unwrap_or("");
        let number = raw
            .strip_suffix(suffix_text)
            .unwrap_or(raw)
            .replace('_', "");
        let value = number
            .parse::<f64>()
            .map_err(|_| literal_error("floating literal could not be represented"))?;
        if !value.is_finite() || (primitive == PrimitiveType::F32 && !(value as f32).is_finite()) {
            return Err(literal_error(format!(
                "floating literal is out of range for `{}`",
                primitive_name(primitive)
            )));
        }
        Ok(self.primitive(primitive))
    }

    /// Materializes an ordinary string literal as contextual `str`/`String`,
    /// defaulting to `str`.
    pub fn materialize_string(
        &mut self,
        expected: ExpectedType,
    ) -> Result<TypeId, LiteralTypeError> {
        let primitive = match expected {
            ExpectedType::None => PrimitiveType::Str,
            ExpectedType::Exact(ty) => match self.expanded_primitive(ty) {
                Some(primitive @ (PrimitiveType::Str | PrimitiveType::String)) => primitive,
                _ => {
                    return Err(literal_error(
                        "string literal requires a `str` or `String` expected type",
                    ));
                }
            },
        };
        Ok(self.primitive(primitive))
    }

    fn expected_numeric(
        &self,
        expected: ExpectedType,
        integer_literal: bool,
    ) -> Option<PrimitiveType> {
        let ExpectedType::Exact(expected) = expected else {
            return None;
        };
        self.expanded_primitive(expected)
            .filter(|primitive| primitive.is_float() || (integer_literal && primitive.is_integer()))
    }

    pub(crate) fn expanded_primitive(&self, mut ty: TypeId) -> Option<PrimitiveType> {
        loop {
            match self.kind(self.resolve_inference(ty)) {
                TypeKind::Alias { target, .. } => ty = *target,
                TypeKind::Primitive(primitive) => return Some(*primitive),
                _ => return None,
            }
        }
    }
}

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
enum TypePosition {
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
    pub declaration_types: BTreeMap<DeclarationId, TypeId>,
    pub field_types: BTreeMap<FieldId, TypeId>,
    pub function_signatures: BTreeMap<DeclarationId, FunctionSignature>,
    pub function_instance_signatures: BTreeMap<FunctionInstance, FunctionSignature>,
    pub impl_trait_types: BTreeMap<ImplId, TypeId>,
    pub impl_target_types: BTreeMap<ImplId, TypeId>,
    pub obligations: Vec<TraitObligation>,
    foreign_fields: BTreeMap<DeclarationId, Vec<TypeId>>,
    nominal_fields: BTreeMap<DeclarationId, Vec<TypeId>>,
    nominal_parameters: BTreeMap<DeclarationId, Vec<GenericParameterId>>,
    layout_nominals: BTreeSet<DeclarationId>,
    builtin_layout: BTreeMap<BuiltinId, bool>,
}

/// Result of canonicalizing every signature in a resolved program.
pub struct TypeOutput {
    pub program: TypedProgram,
    pub diagnostics: Vec<Diagnostic>,
}

/// Creates canonical types for declarations, fields, bounds, implementation
/// heads, and function signatures.
#[must_use]
pub fn resolve_types(resolved: &ResolvedProgram) -> TypeOutput {
    TypeBuilder::new(resolved).run()
}

impl TypedProgram {
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
            )
            | TypeKind::RawPointer { .. } => true,
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
            TypeKind::Function {
                abi: Abi::C,
                parameters,
                return_type,
                ..
            } => {
                parameters
                    .iter()
                    .all(|parameter| self.abi_safe_inner(parameter.ty, visiting))
                    && (matches!(
                        self.types.kind(*return_type),
                        TypeKind::Primitive(PrimitiveType::Unit)
                    ) || self.abi_safe_inner(*return_type, visiting))
            }
            TypeKind::Alias { target, .. } => self.abi_safe_inner(*target, visiting),
            _ => false,
        };
        visiting.remove(&ty);
        safe
    }
}

struct TypeBuilder<'a> {
    resolved: &'a ResolvedProgram,
    types: TypeContext,
    declaration_types: BTreeMap<DeclarationId, TypeId>,
    field_types: BTreeMap<FieldId, TypeId>,
    function_signatures: BTreeMap<DeclarationId, FunctionSignature>,
    impl_trait_types: BTreeMap<ImplId, TypeId>,
    impl_target_types: BTreeMap<ImplId, TypeId>,
    obligations: Vec<TraitObligation>,
    foreign_fields: BTreeMap<DeclarationId, Vec<TypeId>>,
    nominal_fields: BTreeMap<DeclarationId, Vec<TypeId>>,
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
            declaration_types: BTreeMap::new(),
            field_types: BTreeMap::new(),
            function_signatures: BTreeMap::new(),
            impl_trait_types: BTreeMap::new(),
            impl_target_types: BTreeMap::new(),
            obligations: Vec::new(),
            foreign_fields: BTreeMap::new(),
            nominal_fields: BTreeMap::new(),
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
                DeclarationKind::Function | DeclarationKind::ForeignFunction
            ) {
                self.lower_function_signature(declaration.id);
            }
        }

        for implementation in &self.resolved.impls {
            self.lower_impl_bounds(implementation.id);
            self.lower_impl(implementation.id);
        }

        TypeOutput {
            program: TypedProgram {
                types: self.types,
                declaration_types: self.declaration_types,
                field_types: self.field_types,
                function_signatures: self.function_signatures,
                function_instance_signatures: BTreeMap::new(),
                impl_trait_types: self.impl_trait_types,
                impl_target_types: self.impl_target_types,
                obligations: self.obligations,
                foreign_fields: self.foreign_fields,
                nominal_fields: self.nominal_fields,
                nominal_parameters: self
                    .resolved
                    .declarations
                    .iter()
                    .map(|declaration| (declaration.id, declaration.generic_parameters.clone()))
                    .collect(),
                layout_nominals: self
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
                    .collect(),
                builtin_layout: self.builtin_layout,
            },
            diagnostics: self.diagnostics,
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
            DeclarationKind::Function | DeclarationKind::ForeignFunction => {
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
        self.lower_type_in(node, self_type, TypePosition::Value)
    }

    /// Lowers a type in a position that names a trait: a generic or impl
    /// bound, or the trait of an `impl Trait for Type`.
    fn lower_trait_type(&mut self, node: &SyntaxNode, self_type: Option<TypeId>) -> TypeId {
        self.lower_type_in(node, self_type, TypePosition::TraitAllowed)
    }

    fn lower_type_in(
        &mut self,
        node: &SyntaxNode,
        self_type: Option<TypeId>,
        position: TypePosition,
    ) -> TypeId {
        let tokens = direct_tokens(node);
        let first = tokens.first().map(|token| &token.kind);
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
            if !raw && mutable && matches!(self.types.kind(target), TypeKind::Function { .. }) {
                self.diagnostics.push(
                    Diagnostic::new(
                        Category::TypeSystem,
                        "function references cannot be mutable",
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
            Some(TokenKind::Keyword(
                Keyword::Fn | Keyword::Unsafe | Keyword::Extern
            ))
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
                let length = direct_child_not_type(node)
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
            return self.types.intern(TypeKind::Slice(element));
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
        if position == TypePosition::Value && self.is_trait_type(lowered) {
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
            "Option" | "Vec" | "Set" | "Identity" | "ForeignRoot" | "ForeignRootMut" => 1,
            "Result" | "Map" => 2,
            "print" | "println" => {
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
            self.lower_type(result, None)
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
            self.lower_type(node, self_type)
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
        let abi = if foreign {
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
        if let Some(trait_node) = types.first() {
            let trait_type = self.lower_trait_type(trait_node, None);
            self.impl_trait_types.insert(implementation, trait_type);
        }
        if let Some(target_node) = types.get(1) {
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
            if let Some(target) = nodes.get(1) {
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
                    | "Display"
                    | "Close"
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

/// Expression storage classification consumed by Milestone 6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaceKind {
    Value,
    Addressable,
    Mutable,
    CollectionInterior,
}

impl PlaceKind {
    #[must_use]
    pub fn is_addressable(self) -> bool {
        matches!(self, Self::Addressable | Self::Mutable)
    }

    #[must_use]
    pub fn is_mutable(self) -> bool {
        matches!(self, Self::Mutable | Self::CollectionInterior)
    }

    #[must_use]
    pub fn permits_safe_reference(self) -> bool {
        matches!(self, Self::Addressable | Self::Mutable)
    }
}

fn same_outer_shape(left: &TypeKind, right: &TypeKind) -> bool {
    match (left, right) {
        (TypeKind::Error, TypeKind::Error)
        | (TypeKind::Primitive(_), TypeKind::Primitive(_))
        | (TypeKind::Nominal { .. }, TypeKind::Nominal { .. })
        | (TypeKind::Builtin { .. }, TypeKind::Builtin { .. })
        | (TypeKind::Tuple(_), TypeKind::Tuple(_))
        | (TypeKind::Array { .. }, TypeKind::Array { .. })
        | (TypeKind::Slice(_), TypeKind::Slice(_))
        | (TypeKind::Reference { .. }, TypeKind::Reference { .. })
        | (TypeKind::RawPointer { .. }, TypeKind::RawPointer { .. })
        | (TypeKind::Function { .. }, TypeKind::Function { .. })
        | (TypeKind::TraitObject { .. }, TypeKind::TraitObject { .. })
        | (TypeKind::Foreign { .. }, TypeKind::Foreign { .. })
        | (TypeKind::GenericParameter(_), TypeKind::GenericParameter(_))
        | (TypeKind::SelfType(_), TypeKind::SelfType(_))
        | (TypeKind::InferenceVariable(_), TypeKind::InferenceVariable(_)) => {
            type_discriminants_equal(left, right)
        }
        _ => false,
    }
}

fn type_discriminants_equal(left: &TypeKind, right: &TypeKind) -> bool {
    match (left, right) {
        (TypeKind::Primitive(left), TypeKind::Primitive(right)) => left == right,
        (
            TypeKind::Nominal { identity: left, .. },
            TypeKind::Nominal {
                identity: right, ..
            },
        ) => left == right,
        (TypeKind::Builtin { builtin: left, .. }, TypeKind::Builtin { builtin: right, .. }) => {
            left == right
        }
        (TypeKind::Tuple(left), TypeKind::Tuple(right)) => left.len() == right.len(),
        (TypeKind::Array { length: left, .. }, TypeKind::Array { length: right, .. }) => {
            left == right
        }
        (TypeKind::Slice(_), TypeKind::Slice(_))
        | (TypeKind::TraitObject { .. }, TypeKind::TraitObject { .. })
        | (TypeKind::Error, TypeKind::Error) => true,
        (
            TypeKind::Reference {
                mutability: left, ..
            },
            TypeKind::Reference {
                mutability: right, ..
            },
        )
        | (
            TypeKind::RawPointer {
                mutability: left, ..
            },
            TypeKind::RawPointer {
                mutability: right, ..
            },
        ) => left == right,
        (
            TypeKind::Function {
                safety: left_safety,
                abi: left_abi,
                receiver: left_receiver,
                parameters: left_parameters,
                ..
            },
            TypeKind::Function {
                safety: right_safety,
                abi: right_abi,
                receiver: right_receiver,
                parameters: right_parameters,
                ..
            },
        ) => {
            left_safety == right_safety
                && left_abi == right_abi
                && left_receiver.is_some() == right_receiver.is_some()
                && left_parameters.len() == right_parameters.len()
                && left_parameters
                    .iter()
                    .zip(right_parameters)
                    .all(|(left, right)| left.variadic == right.variadic)
        }
        (
            TypeKind::Foreign {
                identity: left_identity,
                complete: left_complete,
            },
            TypeKind::Foreign {
                identity: right_identity,
                complete: right_complete,
            },
        ) => left_identity == right_identity && left_complete == right_complete,
        (TypeKind::GenericParameter(left), TypeKind::GenericParameter(right)) => left == right,
        (TypeKind::SelfType(left), TypeKind::SelfType(right)) => left == right,
        (TypeKind::InferenceVariable(left), TypeKind::InferenceVariable(right)) => left == right,
        _ => false,
    }
}

fn paired_children(left: &TypeKind, right: &TypeKind) -> Vec<(TypeId, TypeId)> {
    let left = type_children(left);
    let right = type_children(right);
    left.into_iter().zip(right).collect()
}

fn type_children(kind: &TypeKind) -> Vec<TypeId> {
    match kind {
        TypeKind::Nominal { arguments, .. }
        | TypeKind::Builtin { arguments, .. }
        | TypeKind::Tuple(arguments)
        | TypeKind::Alias { arguments, .. } => {
            let mut children = arguments.clone();
            if let TypeKind::Alias { target, .. } = kind {
                children.push(*target);
            }
            children
        }
        TypeKind::Array { element, .. }
        | TypeKind::Slice(element)
        | TypeKind::TraitObject {
            trait_type: element,
        } => vec![*element],
        TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => vec![*target],
        TypeKind::Function {
            receiver,
            parameters,
            return_type,
            ..
        } => receiver
            .iter()
            .copied()
            .chain(parameters.iter().map(|parameter| parameter.ty))
            .chain(std::iter::once(*return_type))
            .collect(),
        TypeKind::Error
        | TypeKind::Primitive(_)
        | TypeKind::Foreign { .. }
        | TypeKind::GenericParameter(_)
        | TypeKind::SelfType(_)
        | TypeKind::InferenceVariable(_) => Vec::new(),
    }
}

fn map_type_children(mut kind: TypeKind, mut map: impl FnMut(TypeId) -> TypeId) -> TypeKind {
    match &mut kind {
        TypeKind::Nominal { arguments, .. }
        | TypeKind::Builtin { arguments, .. }
        | TypeKind::Tuple(arguments) => {
            for argument in arguments {
                *argument = map(*argument);
            }
        }
        TypeKind::Array { element, .. }
        | TypeKind::Slice(element)
        | TypeKind::TraitObject {
            trait_type: element,
        } => *element = map(*element),
        TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => {
            *target = map(*target);
        }
        TypeKind::Function {
            receiver,
            parameters,
            return_type,
            ..
        } => {
            if let Some(receiver) = receiver {
                *receiver = map(*receiver);
            }
            for parameter in parameters {
                parameter.ty = map(parameter.ty);
            }
            *return_type = map(*return_type);
        }
        TypeKind::Alias {
            arguments, target, ..
        } => {
            for argument in arguments {
                *argument = map(*argument);
            }
            *target = map(*target);
        }
        TypeKind::Error
        | TypeKind::Primitive(_)
        | TypeKind::Foreign { .. }
        | TypeKind::GenericParameter(_)
        | TypeKind::SelfType(_)
        | TypeKind::InferenceVariable(_) => {}
    }
    kind
}

fn is_type_declaration(kind: DeclarationKind) -> bool {
    matches!(
        kind,
        DeclarationKind::TypeAlias
            | DeclarationKind::Struct
            | DeclarationKind::Enum
            | DeclarationKind::Trait
            | DeclarationKind::ForeignType
            | DeclarationKind::ForeignStruct
    )
}

pub(crate) fn direct_tokens(node: &SyntaxNode) -> Vec<&Token> {
    node.children
        .iter()
        .filter_map(|child| match child {
            SyntaxElement::Token(token) => Some(token),
            SyntaxElement::Node(_) => None,
        })
        .collect()
}

pub(crate) fn direct_children(node: &SyntaxNode, kind: SyntaxKind) -> Vec<&SyntaxNode> {
    node.children
        .iter()
        .filter_map(|child| match child {
            SyntaxElement::Node(node) if node.kind == kind => Some(node.as_ref()),
            _ => None,
        })
        .collect()
}

pub(crate) fn direct_child(node: &SyntaxNode, kind: SyntaxKind) -> Option<&SyntaxNode> {
    direct_children(node, kind).into_iter().next()
}

pub(crate) fn direct_child_not_type(node: &SyntaxNode) -> Option<&SyntaxNode> {
    node.children.iter().find_map(|child| match child {
        SyntaxElement::Node(node) if node.kind != SyntaxKind::Type => Some(node.as_ref()),
        _ => None,
    })
}

fn alias_target_node(node: &SyntaxNode) -> Option<&SyntaxNode> {
    let mut saw_assign = false;
    for child in &node.children {
        match child {
            SyntaxElement::Token(Token {
                kind: TokenKind::Assign,
                ..
            }) => saw_assign = true,
            SyntaxElement::Node(node) if saw_assign && node.kind == SyntaxKind::Type => {
                return Some(node);
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn direct_path_span(node: &SyntaxNode) -> Option<Span> {
    let meaningful = direct_tokens(node)
        .into_iter()
        .filter(|token| {
            matches!(
                token.kind,
                TokenKind::Identifier(_)
                    | TokenKind::Keyword(
                        Keyword::Root | Keyword::SelfValue | Keyword::SelfType | Keyword::Super
                    )
            )
        })
        .collect::<Vec<_>>();
    let first = meaningful.first()?;
    let last = meaningful.last()?;
    Some(Span::new(first.span.file, first.span.start, last.span.end))
}

pub(crate) fn primitive_from_name(name: &str) -> Option<PrimitiveType> {
    Some(match name {
        "bool" => PrimitiveType::Bool,
        "char" => PrimitiveType::Char,
        "i8" => PrimitiveType::I8,
        "i16" => PrimitiveType::I16,
        "i32" => PrimitiveType::I32,
        "i64" => PrimitiveType::I64,
        "i128" => PrimitiveType::I128,
        "isize" => PrimitiveType::Isize,
        "u8" => PrimitiveType::U8,
        "u16" => PrimitiveType::U16,
        "u32" => PrimitiveType::U32,
        "u64" => PrimitiveType::U64,
        "u128" => PrimitiveType::U128,
        "usize" => PrimitiveType::Usize,
        "f32" => PrimitiveType::F32,
        "f64" => PrimitiveType::F64,
        "str" => PrimitiveType::Str,
        "String" => PrimitiveType::String,
        _ => return None,
    })
}

fn builtin_has_layout(name: &str) -> bool {
    matches!(
        name,
        "Option"
            | "Result"
            | "Vec"
            | "Map"
            | "Set"
            | "Formatter"
            | "Identity"
            | "NumericError"
            | "IoError"
            | "ForeignRoot"
            | "ForeignRootMut"
    )
}

fn primitive_for_suffix(suffix: NumericSuffix) -> PrimitiveType {
    match suffix {
        NumericSuffix::I8 => PrimitiveType::I8,
        NumericSuffix::I16 => PrimitiveType::I16,
        NumericSuffix::I32 => PrimitiveType::I32,
        NumericSuffix::I64 => PrimitiveType::I64,
        NumericSuffix::I128 => PrimitiveType::I128,
        NumericSuffix::Isize => PrimitiveType::Isize,
        NumericSuffix::U8 => PrimitiveType::U8,
        NumericSuffix::U16 => PrimitiveType::U16,
        NumericSuffix::U32 => PrimitiveType::U32,
        NumericSuffix::U64 => PrimitiveType::U64,
        NumericSuffix::U128 => PrimitiveType::U128,
        NumericSuffix::Usize => PrimitiveType::Usize,
        NumericSuffix::F32 => PrimitiveType::F32,
        NumericSuffix::F64 => PrimitiveType::F64,
    }
}

fn numeric_suffix_text(suffix: NumericSuffix) -> &'static str {
    match suffix {
        NumericSuffix::I8 => "i8",
        NumericSuffix::I16 => "i16",
        NumericSuffix::I32 => "i32",
        NumericSuffix::I64 => "i64",
        NumericSuffix::I128 => "i128",
        NumericSuffix::Isize => "isize",
        NumericSuffix::U8 => "u8",
        NumericSuffix::U16 => "u16",
        NumericSuffix::U32 => "u32",
        NumericSuffix::U64 => "u64",
        NumericSuffix::U128 => "u128",
        NumericSuffix::Usize => "usize",
        NumericSuffix::F32 => "f32",
        NumericSuffix::F64 => "f64",
    }
}

fn primitive_name(primitive: PrimitiveType) -> &'static str {
    match primitive {
        PrimitiveType::Unit => "()",
        PrimitiveType::Bool => "bool",
        PrimitiveType::Char => "char",
        PrimitiveType::I8 => "i8",
        PrimitiveType::I16 => "i16",
        PrimitiveType::I32 => "i32",
        PrimitiveType::I64 => "i64",
        PrimitiveType::I128 => "i128",
        PrimitiveType::Isize => "isize",
        PrimitiveType::U8 => "u8",
        PrimitiveType::U16 => "u16",
        PrimitiveType::U32 => "u32",
        PrimitiveType::U64 => "u64",
        PrimitiveType::U128 => "u128",
        PrimitiveType::Usize => "usize",
        PrimitiveType::F32 => "f32",
        PrimitiveType::F64 => "f64",
        PrimitiveType::Str => "str",
        PrimitiveType::String => "String",
    }
}

pub(crate) fn parse_integer_magnitude(
    raw: &str,
    radix: u8,
    suffix: Option<NumericSuffix>,
) -> Result<u128, LiteralTypeError> {
    let suffix = suffix.map(numeric_suffix_text).unwrap_or("");
    let without_suffix = raw.strip_suffix(suffix).unwrap_or(raw);
    let without_prefix = if radix == 10 {
        without_suffix
    } else {
        without_suffix.get(2..).unwrap_or("")
    };
    u128::from_str_radix(&without_prefix.replace('_', ""), u32::from(radix))
        .map_err(|_| literal_error("integer literal exceeds the supported 128-bit magnitude"))
}

fn integer_fits(
    primitive: PrimitiveType,
    magnitude: u128,
    negative: bool,
    pointer_bits: u8,
) -> bool {
    let (signed, bits) = match primitive {
        PrimitiveType::I8 => (true, 8),
        PrimitiveType::I16 => (true, 16),
        PrimitiveType::I32 => (true, 32),
        PrimitiveType::I64 => (true, 64),
        PrimitiveType::I128 => (true, 128),
        PrimitiveType::Isize => (true, pointer_bits),
        PrimitiveType::U8 => (false, 8),
        PrimitiveType::U16 => (false, 16),
        PrimitiveType::U32 => (false, 32),
        PrimitiveType::U64 => (false, 64),
        PrimitiveType::U128 => (false, 128),
        PrimitiveType::Usize => (false, pointer_bits),
        _ => return false,
    };
    if !matches!(bits, 8 | 16 | 32 | 64 | 128) || (!signed && negative) {
        return false;
    }
    if signed {
        let boundary = 1u128 << (bits - 1);
        if negative {
            magnitude <= boundary
        } else {
            magnitude < boundary
        }
    } else if bits == 128 {
        true
    } else {
        magnitude < (1u128 << bits)
    }
}

fn literal_error(message: impl Into<String>) -> LiteralTypeError {
    LiteralTypeError {
        message: message.into(),
    }
}

pub(crate) fn array_length_literal(node: &SyntaxNode) -> Option<u128> {
    node.children.iter().find_map(|child| match child {
        SyntaxElement::Token(Token {
            kind: TokenKind::IntegerLiteral { raw, radix, suffix },
            ..
        }) => parse_integer_magnitude(raw, *radix, *suffix).ok(),
        SyntaxElement::Node(child) => array_length_literal(child),
        SyntaxElement::Token(_) => None,
    })
}

fn abi_from_tokens(tokens: &[&Token]) -> Abi {
    if !tokens
        .iter()
        .any(|token| matches!(token.kind, TokenKind::Keyword(Keyword::Extern)))
    {
        return Abi::Elamite;
    }
    let name = tokens.iter().find_map(|token| match &token.kind {
        TokenKind::StringLiteral(name) => Some(name.clone()),
        _ => None,
    });
    match name.as_deref() {
        Some("C") => Abi::C,
        Some(name) => Abi::Unsupported(name.to_string()),
        None => Abi::Unsupported(String::new()),
    }
}
