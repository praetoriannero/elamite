//! Canonical type identities, interning, equivalence, and substitution.

use super::*;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub(super) u32);

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
    pub fn is_unsigned_integer(self) -> bool {
        matches!(
            self,
            Self::U8 | Self::U16 | Self::U32 | Self::U64 | Self::U128 | Self::Usize
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
    Never,
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
    Slice {
        mutability: Mutability,
        element: TypeId,
    },
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
    Closure {
        declaration: DeclarationId,
        captures: Vec<TypeId>,
        parameters: Vec<TypeId>,
        return_type: TypeId,
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
        context.intern(TypeKind::Never);
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

    #[must_use]
    pub fn never(&self) -> TypeId {
        *self
            .interned
            .get(&TypeKind::Never)
            .expect("the never type is pre-interned")
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

    /// Whether `ty` contains one particular declared generic parameter.
    #[must_use]
    pub fn mentions_generic_parameter(&self, ty: TypeId, parameter: GenericParameterId) -> bool {
        self.any_type(
            ty,
            &mut BTreeSet::new(),
            &|kind| matches!(kind, TypeKind::GenericParameter(found) if *found == parameter),
        )
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

fn same_outer_shape(left: &TypeKind, right: &TypeKind) -> bool {
    match (left, right) {
        (TypeKind::Error, TypeKind::Error)
        | (TypeKind::Never, TypeKind::Never)
        | (TypeKind::Primitive(_), TypeKind::Primitive(_))
        | (TypeKind::Nominal { .. }, TypeKind::Nominal { .. })
        | (TypeKind::Builtin { .. }, TypeKind::Builtin { .. })
        | (TypeKind::Tuple(_), TypeKind::Tuple(_))
        | (TypeKind::Array { .. }, TypeKind::Array { .. })
        | (TypeKind::Slice { .. }, TypeKind::Slice { .. })
        | (TypeKind::Reference { .. }, TypeKind::Reference { .. })
        | (TypeKind::RawPointer { .. }, TypeKind::RawPointer { .. })
        | (TypeKind::Function { .. }, TypeKind::Function { .. })
        | (TypeKind::TraitObject { .. }, TypeKind::TraitObject { .. })
        | (TypeKind::Closure { .. }, TypeKind::Closure { .. })
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
        (
            TypeKind::Slice {
                mutability: left, ..
            },
            TypeKind::Slice {
                mutability: right, ..
            },
        ) => left == right,
        (TypeKind::TraitObject { .. }, TypeKind::TraitObject { .. })
        | (TypeKind::Error, TypeKind::Error)
        | (TypeKind::Never, TypeKind::Never) => true,
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
            TypeKind::Closure {
                declaration: left, ..
            },
            TypeKind::Closure {
                declaration: right, ..
            },
        ) => left == right,
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
        | TypeKind::Slice { element, .. }
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
        TypeKind::Closure {
            captures,
            parameters,
            return_type,
            ..
        } => captures
            .iter()
            .chain(parameters)
            .copied()
            .chain(std::iter::once(*return_type))
            .collect(),
        TypeKind::Error
        | TypeKind::Never
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
        | TypeKind::Slice { element, .. }
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
        TypeKind::Closure {
            captures,
            parameters,
            return_type,
            ..
        } => {
            for capture in captures {
                *capture = map(*capture);
            }
            for parameter in parameters {
                *parameter = map(*parameter);
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
        | TypeKind::Never
        | TypeKind::Primitive(_)
        | TypeKind::Foreign { .. }
        | TypeKind::GenericParameter(_)
        | TypeKind::SelfType(_)
        | TypeKind::InferenceVariable(_) => {}
    }
    kind
}

pub(super) fn is_type_declaration(kind: DeclarationKind) -> bool {
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

pub(super) fn alias_target_node(node: &SyntaxNode) -> Option<&SyntaxNode> {
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

pub(super) fn builtin_has_layout(name: &str) -> bool {
    matches!(
        name,
        "Vec"
            | "Map"
            | "Set"
            | "Box"
            | "Shared"
            | "Weak"
            | "Store"
            | "Handle"
            | "Formatter"
            | "Identity"
            | "ForeignRoot"
            | "ForeignRootMut"
            | "Thread"
            | "Sender"
            | "Receiver"
            | "Mutex"
            | "AtomicBool"
            | "AtomicI32"
            | "AtomicUsize"
            | "File"
            | "Directory"
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

/// The suffix the generated runtime helpers use for a primitive integer type
/// (`i8`, `usize`, …), or `None` for a type with no such helpers.
#[must_use]
pub(crate) fn primitive_name_for_symbol(primitive: PrimitiveType) -> Option<&'static str> {
    primitive.is_integer().then(|| primitive_name(primitive))
}

/// The source spelling of a primitive type, for diagnostics.
pub(crate) fn primitive_name(primitive: PrimitiveType) -> &'static str {
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

pub(crate) fn integer_fits(
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

pub(super) fn abi_from_tokens(tokens: &[&Token]) -> Abi {
    let _ = tokens;
    Abi::Elamite
}
