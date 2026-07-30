//! Trait implementation checking (`ROADMAP.md` Milestone 13).
//!
//! This pass validates declarations rather than bodies: whether each
//! `impl Trait for Type` supplies exactly the trait's methods with exact
//! signatures, whether the program's implementations are coherent, and whether
//! a trait may form a trait object. Body-level trait *use* — bound-call
//! selection and dispatch — belongs to the checker and the backend.

use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostics::{Category, Diagnostic};
use crate::package::PackageId;
use crate::resolution::{
    DeclarationId, DeclarationKind, GenericParameterId, ImplId, ModuleId, ResolvedProgram,
    VariantId, Visibility,
};
use crate::source::Span;
use crate::types::{FunctionSignature, PrimitiveType, TypeId, TypeKind, TypedProgram};

/// One implementation of a trait, resolved to its trait and target
/// declarations.
#[derive(Debug, Clone, Copy)]
pub struct TraitImplementation {
    pub implementation: ImplId,
    pub trait_declaration: DeclarationId,
    pub target: TypeId,
    pub span: Span,
}

/// Result of trait checking: the validated trait implementations, plus any
/// diagnostics.
#[derive(Debug, Default)]
pub struct TraitOutput {
    pub implementations: Vec<TraitImplementation>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Validates every trait implementation in the program.
#[must_use]
pub fn check_traits(resolved: &ResolvedProgram, typed: &mut TypedProgram) -> TraitOutput {
    let mut output = TraitOutput::default();
    check_derivations(resolved, typed, &mut output.diagnostics);
    let implementations = collect(resolved, typed);
    for implementation in &implementations {
        check_conformance(resolved, typed, *implementation, &mut output.diagnostics);
    }
    check_coherence(resolved, typed, &implementations, &mut output.diagnostics);
    output.implementations = implementations;
    output
}

fn check_derivations(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    diagnostics: &mut Vec<Diagnostic>,
) {
    const SUPPORTED: [&str; 6] = ["Default", "PartialEq", "Eq", "PartialOrd", "Ord", "Hash"];
    for declaration in &resolved.declarations {
        if !matches!(
            declaration.kind,
            DeclarationKind::Struct | DeclarationKind::Enum
        ) {
            continue;
        }
        let entries = derivation_entries(&declaration.syntax);
        let mut seen = BTreeMap::<String, Span>::new();
        for (name, span) in &entries {
            if let Some(previous) = seen.insert(name.clone(), *span) {
                diagnostics.push(
                    Diagnostic::new(
                        Category::TypeSystem,
                        format!("derived trait `{name}` is listed more than once"),
                    )
                    .with_primary(*span)
                    .with_related(previous, "the earlier entry is here"),
                );
                continue;
            }
            if !SUPPORTED.contains(&name.as_str()) {
                diagnostics.push(
                    Diagnostic::new(
                        Category::TypeSystem,
                        format!("`{name}` is not a compiler-supported derivable trait"),
                    )
                    .with_primary(*span),
                );
                continue;
            }
            if name == "Default" && declaration.kind == DeclarationKind::Enum {
                diagnostics.push(
                    Diagnostic::new(
                        Category::TypeSystem,
                        "`Default` cannot be derived for an enum; implement it manually",
                    )
                    .with_primary(*span),
                );
                continue;
            }
            let prerequisite = match name.as_str() {
                "Eq" if !derives(resolved, declaration.id, "PartialEq") => Some("PartialEq"),
                "Ord" if !derives(resolved, declaration.id, "Eq") => Some("Eq"),
                "Ord" if !derives(resolved, declaration.id, "PartialOrd") => Some("PartialOrd"),
                _ => None,
            };
            if let Some(prerequisite) = prerequisite {
                diagnostics.push(
                    Diagnostic::new(
                        Category::TypeSystem,
                        format!("deriving `{name}` also requires deriving `{prerequisite}`"),
                    )
                    .with_primary(*span),
                );
                continue;
            }
            let Some(ty) = typed.declaration_types.get(&declaration.id).copied() else {
                continue;
            };
            if !provides_inner(
                resolved,
                typed,
                ty,
                name,
                &BTreeMap::new(),
                true,
                &mut BTreeSet::new(),
            ) {
                diagnostics.push(
                    Diagnostic::new(
                        Category::TypeSystem,
                        format!(
                            "`{name}` cannot be derived because at least one field does not \
                             provide `{name}`"
                        ),
                    )
                    .with_primary(*span),
                );
            }
        }
    }
}

fn derivation_entries(syntax: &crate::syntax::SyntaxNode) -> Vec<(String, Span)> {
    let Some(list) = crate::syntax::direct_child(syntax, crate::syntax::SyntaxKind::DeriveList)
    else {
        return Vec::new();
    };
    list.children
        .iter()
        .filter_map(|child| match child {
            crate::syntax::SyntaxElement::Token(token) => match &token.kind {
                crate::lexer::TokenKind::Identifier(name) => Some((name.clone(), token.span)),
                _ => None,
            },
            crate::syntax::SyntaxElement::Node(_) => None,
        })
        .collect()
}

/// Trait implementations in declaration order, skipping inherent impls and
/// impls whose trait or target failed to canonicalize.
#[must_use]
pub fn implementations(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
) -> Vec<TraitImplementation> {
    collect(resolved, typed)
}

/// The trait a trait-object type names, if it is one.
#[must_use]
pub fn object_trait(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    ty: TypeId,
) -> Option<DeclarationId> {
    let resolved_type = typed.types.resolve_inference(ty);
    let target = match typed.types.kind(resolved_type) {
        TypeKind::Reference { target, .. } => *target,
        _ => resolved_type,
    };
    match typed.types.kind(typed.types.resolve_inference(target)) {
        TypeKind::TraitObject { trait_type } => trait_declaration_of(resolved, typed, *trait_type),
        _ => None,
    }
}

fn collect(resolved: &ResolvedProgram, typed: &TypedProgram) -> Vec<TraitImplementation> {
    let mut implementations = Vec::new();
    for block in &resolved.impls {
        let Some(trait_type) = typed.impl_trait_types.get(&block.id).copied() else {
            continue;
        };
        let Some(target) = typed.impl_target_types.get(&block.id).copied() else {
            continue;
        };
        let Some(trait_declaration) = trait_declaration_of(resolved, typed, trait_type) else {
            continue;
        };
        implementations.push(TraitImplementation {
            implementation: block.id,
            trait_declaration,
            target,
            span: block.span,
        });
    }
    implementations
}

fn trait_declaration_of(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    trait_type: TypeId,
) -> Option<DeclarationId> {
    match typed.types.kind(typed.types.resolve_inference(trait_type)) {
        TypeKind::Nominal { identity, .. }
            if resolved.declarations[identity.declaration.index()].kind
                == DeclarationKind::Trait =>
        {
            Some(identity.declaration)
        }
        _ => None,
    }
}

/// Required methods of a trait, paired with whether the trait supplies a
/// default body.
fn trait_methods(
    resolved: &ResolvedProgram,
    trait_declaration: DeclarationId,
) -> BTreeMap<String, (DeclarationId, bool)> {
    let mut methods = BTreeMap::new();
    let Some(members) = resolved.declaration_members.get(&trait_declaration) else {
        return methods;
    };
    for (symbol, member) in members {
        let crate::resolution::MemberId::Method(method) = member else {
            continue;
        };
        let data = &resolved.declarations[method.index()];
        let has_default =
            crate::syntax::direct_child(&data.syntax, crate::syntax::SyntaxKind::Block).is_some();
        methods.insert(
            resolved.symbol_text(*symbol).to_string(),
            (*method, has_default),
        );
    }
    methods
}

/// Checks that an implementation supplies exactly the trait's methods, each
/// with the trait's signature after `Self` is replaced by the target type.
fn check_conformance(
    resolved: &ResolvedProgram,
    typed: &mut TypedProgram,
    implementation: TraitImplementation,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let required = trait_methods(resolved, implementation.trait_declaration);
    let trait_name = declaration_name(resolved, implementation.trait_declaration);
    let provided = resolved
        .impl_members
        .get(&implementation.implementation)
        .cloned()
        .unwrap_or_default();
    let provided_names: BTreeMap<String, DeclarationId> = provided
        .iter()
        .map(|(symbol, declaration)| (resolved.symbol_text(*symbol).to_string(), *declaration))
        .collect();

    for (name, (trait_method, has_default)) in &required {
        match provided_names.get(name) {
            Some(method) => {
                compare_signatures(
                    resolved,
                    typed,
                    implementation,
                    *trait_method,
                    *method,
                    name,
                    diagnostics,
                );
            }
            None if *has_default => {}
            None => {
                diagnostics.push(
                    Diagnostic::new(
                        Category::TypeSystem,
                        format!(
                            "this implementation is missing required method `{name}` of trait \
                             `{trait_name}`"
                        ),
                    )
                    .with_primary(implementation.span),
                );
            }
        }
    }

    for (name, method) in &provided_names {
        if required.contains_key(name) {
            continue;
        }
        diagnostics.push(
            Diagnostic::new(
                Category::TypeSystem,
                format!("trait `{trait_name}` has no method named `{name}`"),
            )
            .with_primary(resolved.declarations[method.index()].span),
        );
    }
}

fn compare_signatures(
    resolved: &ResolvedProgram,
    typed: &mut TypedProgram,
    implementation: TraitImplementation,
    trait_method: DeclarationId,
    impl_method: DeclarationId,
    name: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let (Some(declared), Some(actual)) = (
        typed.function_signatures.get(&trait_method).cloned(),
        typed.function_signatures.get(&impl_method).cloned(),
    ) else {
        return;
    };
    let span = resolved.declarations[impl_method.index()].span;
    let expected = substitute_signature(typed, &declared, implementation.target);

    let mismatch = |diagnostics: &mut Vec<Diagnostic>, detail: &str| {
        diagnostics.push(
            Diagnostic::new(
                Category::TypeSystem,
                format!("method `{name}` does not match its trait declaration: {detail}"),
            )
            .with_primary(span),
        );
    };

    match (expected.receiver, actual.receiver) {
        (Some(expected_receiver), Some(actual_receiver)) => {
            if !typed
                .types
                .exactly_equal(expected_receiver, actual_receiver)
            {
                mismatch(diagnostics, "the receiver form differs");
                return;
            }
        }
        (None, None) => {}
        _ => {
            mismatch(diagnostics, "the receiver form differs");
            return;
        }
    }

    if expected.parameters.len() != actual.parameters.len() {
        mismatch(diagnostics, "the parameter count differs");
        return;
    }
    for (index, (expected_parameter, actual_parameter)) in expected
        .parameters
        .iter()
        .zip(&actual.parameters)
        .enumerate()
    {
        if expected_parameter.variadic != actual_parameter.variadic {
            mismatch(
                diagnostics,
                &format!("parameter {} differs in its variadic marker", index + 1),
            );
            return;
        }
        if !typed
            .types
            .exactly_equal(expected_parameter.ty, actual_parameter.ty)
        {
            mismatch(
                diagnostics,
                &format!("parameter {} has a different type", index + 1),
            );
            return;
        }
    }
    if !typed
        .types
        .exactly_equal(expected.return_type, actual.return_type)
    {
        mismatch(diagnostics, "the return type differs");
    }
}

fn substitute_signature(
    typed: &mut TypedProgram,
    signature: &FunctionSignature,
    target: TypeId,
) -> FunctionSignature {
    FunctionSignature {
        ty: signature.ty,
        receiver: signature
            .receiver
            .map(|receiver| typed.types.substitute_self(receiver, target)),
        parameters: signature
            .parameters
            .iter()
            .map(|parameter| crate::types::FunctionParameter {
                ty: typed.types.substitute_self(parameter.ty, target),
                variadic: parameter.variadic,
            })
            .collect(),
        return_type: typed.types.substitute_self(signature.return_type, target),
    }
}

/// Enforces the orphan rule and rejects overlapping implementations.
fn check_coherence(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    implementations: &[TraitImplementation],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut seen: Vec<(DeclarationId, TypeId, Span)> = Vec::new();
    for implementation in implementations {
        // Orphan rule: an implementation is permitted only where the trait or
        // the implemented type belongs to the implementing package, so two
        // packages cannot both supply the same implementation.
        let implementing = package_of(resolved, implementation.implementation);
        let trait_package = declaration_package(resolved, implementation.trait_declaration);
        let target_package = target_declaration(resolved, typed, implementation.target)
            .map(|declaration| declaration_package(resolved, declaration));
        let trait_is_local = trait_package == implementing;
        let target_is_local = target_package.is_some_and(|package| package == implementing);
        if !trait_is_local && !target_is_local {
            diagnostics.push(
                Diagnostic::new(
                    Category::TypeSystem,
                    format!(
                        "neither trait `{}` nor its implemented type belongs to this package, so \
                         this implementation is not permitted",
                        declaration_name(resolved, implementation.trait_declaration)
                    ),
                )
                .with_primary(implementation.span),
            );
        }

        if let Some((_, _, previous)) = seen.iter().find(|(trait_declaration, target, _)| {
            *trait_declaration == implementation.trait_declaration
                && type_patterns_overlap(typed, *target, implementation.target)
        }) {
            diagnostics.push(
                Diagnostic::new(
                    Category::TypeSystem,
                    format!(
                        "trait `{}` is already implemented for this type",
                        declaration_name(resolved, implementation.trait_declaration)
                    ),
                )
                .with_primary(implementation.span)
                .with_related(*previous, "the earlier implementation is here"),
            );
        } else {
            seen.push((
                implementation.trait_declaration,
                implementation.target,
                implementation.span,
            ));
        }
    }
}

fn match_type_pattern(
    typed: &TypedProgram,
    pattern: TypeId,
    actual: TypeId,
    bindings: &mut BTreeMap<GenericParameterId, TypeId>,
) -> bool {
    let pattern = typed.types.resolve_inference(pattern);
    let actual = typed.types.resolve_inference(actual);
    if let TypeKind::Alias { target, .. } = typed.types.kind(pattern) {
        return match_type_pattern(typed, *target, actual, bindings);
    }
    if let TypeKind::Alias { target, .. } = typed.types.kind(actual) {
        return match_type_pattern(typed, pattern, *target, bindings);
    }
    if let TypeKind::GenericParameter(parameter) = typed.types.kind(pattern) {
        if let Some(bound) = bindings.get(parameter) {
            return typed.types.exactly_equal(*bound, actual);
        }
        bindings.insert(*parameter, actual);
        return true;
    }
    match (typed.types.kind(pattern), typed.types.kind(actual)) {
        (TypeKind::Error, TypeKind::Error)
        | (TypeKind::Primitive(_), TypeKind::Primitive(_))
        | (TypeKind::SelfType(_), TypeKind::SelfType(_))
        | (TypeKind::InferenceVariable(_), TypeKind::InferenceVariable(_)) => {
            typed.types.exactly_equal(pattern, actual)
        }
        (
            TypeKind::Nominal {
                identity: left,
                arguments: left_arguments,
            },
            TypeKind::Nominal {
                identity: right,
                arguments: right_arguments,
            },
        ) => {
            left == right && match_type_arguments(typed, left_arguments, right_arguments, bindings)
        }
        (
            TypeKind::Builtin {
                builtin: left,
                arguments: left_arguments,
            },
            TypeKind::Builtin {
                builtin: right,
                arguments: right_arguments,
            },
        ) => {
            left == right && match_type_arguments(typed, left_arguments, right_arguments, bindings)
        }
        (TypeKind::Tuple(left), TypeKind::Tuple(right)) => {
            match_type_arguments(typed, left, right, bindings)
        }
        (
            TypeKind::Array {
                element: left,
                length: left_length,
            },
            TypeKind::Array {
                element: right,
                length: right_length,
            },
        ) => left_length == right_length && match_type_pattern(typed, *left, *right, bindings),
        (TypeKind::Slice(left), TypeKind::Slice(right)) => {
            match_type_pattern(typed, *left, *right, bindings)
        }
        (
            TypeKind::Reference {
                mutability: left_mutability,
                target: left,
            },
            TypeKind::Reference {
                mutability: right_mutability,
                target: right,
            },
        )
        | (
            TypeKind::RawPointer {
                mutability: left_mutability,
                target: left,
            },
            TypeKind::RawPointer {
                mutability: right_mutability,
                target: right,
            },
        ) => {
            left_mutability == right_mutability
                && match_type_pattern(typed, *left, *right, bindings)
        }
        (
            TypeKind::Function {
                safety: left_safety,
                abi: left_abi,
                receiver: left_receiver,
                parameters: left_parameters,
                return_type: left_return,
            },
            TypeKind::Function {
                safety: right_safety,
                abi: right_abi,
                receiver: right_receiver,
                parameters: right_parameters,
                return_type: right_return,
            },
        ) => {
            left_safety == right_safety
                && left_abi == right_abi
                && match (left_receiver, right_receiver) {
                    (Some(left), Some(right)) => match_type_pattern(typed, *left, *right, bindings),
                    (None, None) => true,
                    _ => false,
                }
                && left_parameters.len() == right_parameters.len()
                && left_parameters
                    .iter()
                    .zip(right_parameters)
                    .all(|(left, right)| {
                        left.variadic == right.variadic
                            && match_type_pattern(typed, left.ty, right.ty, bindings)
                    })
                && match_type_pattern(typed, *left_return, *right_return, bindings)
        }
        (
            TypeKind::TraitObject { trait_type: left },
            TypeKind::TraitObject { trait_type: right },
        ) => match_type_pattern(typed, *left, *right, bindings),
        (
            TypeKind::Foreign {
                identity: left,
                complete: left_complete,
            },
            TypeKind::Foreign {
                identity: right,
                complete: right_complete,
            },
        ) => left == right && left_complete == right_complete,
        _ => false,
    }
}

fn match_type_arguments(
    typed: &TypedProgram,
    patterns: &[TypeId],
    actuals: &[TypeId],
    bindings: &mut BTreeMap<GenericParameterId, TypeId>,
) -> bool {
    patterns.len() == actuals.len()
        && patterns
            .iter()
            .zip(actuals)
            .all(|(pattern, actual)| match_type_pattern(typed, *pattern, *actual, bindings))
}

fn type_patterns_overlap(typed: &TypedProgram, left: TypeId, right: TypeId) -> bool {
    let left = typed.types.resolve_inference(left);
    let right = typed.types.resolve_inference(right);
    if matches!(typed.types.kind(left), TypeKind::GenericParameter(_))
        || matches!(typed.types.kind(right), TypeKind::GenericParameter(_))
    {
        return true;
    }
    if let TypeKind::Alias { target, .. } = typed.types.kind(left) {
        return type_patterns_overlap(typed, *target, right);
    }
    if let TypeKind::Alias { target, .. } = typed.types.kind(right) {
        return type_patterns_overlap(typed, left, *target);
    }
    match (typed.types.kind(left), typed.types.kind(right)) {
        (
            TypeKind::Nominal {
                identity: left_identity,
                arguments: left_arguments,
            },
            TypeKind::Nominal {
                identity: right_identity,
                arguments: right_arguments,
            },
        ) => {
            left_identity == right_identity
                && overlapping_arguments(typed, left_arguments, right_arguments)
        }
        (
            TypeKind::Builtin {
                builtin: left_builtin,
                arguments: left_arguments,
            },
            TypeKind::Builtin {
                builtin: right_builtin,
                arguments: right_arguments,
            },
        ) => {
            left_builtin == right_builtin
                && overlapping_arguments(typed, left_arguments, right_arguments)
        }
        (TypeKind::Tuple(left), TypeKind::Tuple(right)) => {
            overlapping_arguments(typed, left, right)
        }
        (
            TypeKind::Array {
                element: left,
                length: left_length,
            },
            TypeKind::Array {
                element: right,
                length: right_length,
            },
        ) => left_length == right_length && type_patterns_overlap(typed, *left, *right),
        (TypeKind::Slice(left), TypeKind::Slice(right)) => {
            type_patterns_overlap(typed, *left, *right)
        }
        (
            TypeKind::Reference {
                mutability: left_mutability,
                target: left,
            },
            TypeKind::Reference {
                mutability: right_mutability,
                target: right,
            },
        )
        | (
            TypeKind::RawPointer {
                mutability: left_mutability,
                target: left,
            },
            TypeKind::RawPointer {
                mutability: right_mutability,
                target: right,
            },
        ) => left_mutability == right_mutability && type_patterns_overlap(typed, *left, *right),
        _ => typed.types.exactly_equal(left, right),
    }
}

fn overlapping_arguments(typed: &TypedProgram, left: &[TypeId], right: &[TypeId]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| type_patterns_overlap(typed, *left, *right))
}

fn target_declaration(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    target: TypeId,
) -> Option<DeclarationId> {
    let _ = resolved;
    match typed.types.kind(typed.types.resolve_inference(target)) {
        TypeKind::Nominal { identity, .. } | TypeKind::Foreign { identity, .. } => {
            Some(identity.declaration)
        }
        _ => None,
    }
}

fn declaration_package(
    resolved: &ResolvedProgram,
    declaration: DeclarationId,
) -> Option<PackageId> {
    let module = resolved.declarations[declaration.index()].module;
    resolved.modules[module.index()].package.clone()
}

fn package_of(resolved: &ResolvedProgram, implementation: ImplId) -> Option<PackageId> {
    let module = resolved.impls[implementation.index()].module;
    resolved.modules[module.index()].package.clone()
}

fn declaration_name(resolved: &ResolvedProgram, declaration: DeclarationId) -> String {
    resolved
        .symbol_text(resolved.declarations[declaration.index()].name)
        .to_string()
}

/// A trait method selected for a concrete receiver type.
#[derive(Debug, Clone)]
pub struct SelectedTraitMethod {
    /// The declaration to call: an implementation's method, or the trait's
    /// default body.
    pub declaration: DeclarationId,
    /// `None` for a compiler-known builtin trait, which has no declaration.
    pub trait_declaration: Option<DeclarationId>,
    /// `Some` when `declaration` is a default body, which is written against
    /// `Self` and must be specialized for this receiver.
    pub self_type: Option<TypeId>,
    /// Concrete arguments for an implementation block's generic parameters.
    pub arguments: Vec<TypeId>,
}

/// Selects a trait method named `name` for `target`.
///
/// This searches every implementation for the type, including implementations
/// of compiler-known traits, which are builtins rather than user declarations.
/// An implementation's own method is preferred over a trait's default body.
/// `Err` lists the competing trait names when more than one supplies the name,
/// which the caller reports as an ambiguity rather than choosing arbitrarily.
pub fn select_trait_method(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    target: TypeId,
    name: &str,
    module: Option<ModuleId>,
) -> Result<Option<SelectedTraitMethod>, Vec<String>> {
    let mut found: Vec<(SelectedTraitMethod, String)> = Vec::new();
    for block in &resolved.impls {
        let Some(implemented) = typed.impl_target_types.get(&block.id).copied() else {
            continue;
        };
        let mut bindings = BTreeMap::new();
        if !match_type_pattern(typed, implemented, target, &mut bindings) {
            continue;
        }
        if !implementation_bounds_hold(resolved, typed, block, &bindings) {
            continue;
        }
        let implementation_arguments = block
            .generic_parameters
            .iter()
            .filter_map(|parameter| bindings.get(parameter).copied())
            .collect::<Vec<_>>();
        let Some(trait_type) = typed.impl_trait_types.get(&block.id).copied() else {
            continue;
        };
        let trait_declaration = trait_declaration_of(resolved, typed, trait_type);
        if let (Some(module), Some(trait_declaration)) = (module, trait_declaration)
            && !resolved.declaration_in_scope(module, trait_declaration)
        {
            continue;
        }
        let trait_name = trait_declaration.map_or_else(
            || trait_type_name(resolved, typed, trait_type),
            |declaration| declaration_name(resolved, declaration),
        );
        let provided = resolved.impl_members.get(&block.id).and_then(|members| {
            members
                .iter()
                .find(|(symbol, _)| resolved.symbol_text(**symbol) == name)
                .map(|(_, declaration)| *declaration)
        });
        if let Some(declaration) = provided {
            found.push((
                SelectedTraitMethod {
                    declaration,
                    trait_declaration,
                    self_type: None,
                    arguments: implementation_arguments,
                },
                trait_name,
            ));
            continue;
        }
        // Only a user-declared trait can supply a default body.
        if let Some(trait_declaration) = trait_declaration
            && let Some((method, true)) = trait_methods(resolved, trait_declaration)
                .get(name)
                .copied()
        {
            found.push((
                SelectedTraitMethod {
                    declaration: method,
                    trait_declaration: Some(trait_declaration),
                    self_type: Some(target),
                    arguments: Vec::new(),
                },
                trait_name,
            ));
        }
    }
    match found.len() {
        0 => Ok(None),
        1 => Ok(found.pop().map(|(selected, _)| selected)),
        _ => Err(found.into_iter().map(|(_, name)| name).collect()),
    }
}

/// Selects one explicitly qualified implementation member for
/// `Type.Trait.method`.
#[must_use]
pub fn qualified_trait_method(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    target: TypeId,
    trait_name: &str,
    method_name: &str,
) -> Option<DeclarationId> {
    for block in &resolved.impls {
        let Some(implemented) = typed.impl_target_types.get(&block.id).copied() else {
            continue;
        };
        let Some(trait_type) = typed.impl_trait_types.get(&block.id).copied() else {
            continue;
        };
        if trait_type_name(resolved, typed, trait_type) != trait_name {
            continue;
        }
        let mut bindings = BTreeMap::new();
        let matched = match_type_pattern(typed, implemented, target, &mut bindings);
        let bounds = implementation_bounds_hold(resolved, typed, block, &bindings);
        if !matched || !bounds {
            continue;
        }
        if let Some(declaration) = resolved.impl_members.get(&block.id).and_then(|members| {
            members
                .iter()
                .find(|(symbol, _)| resolved.symbol_text(**symbol) == method_name)
                .map(|(_, declaration)| *declaration)
        }) {
            return Some(declaration);
        }
    }
    None
}

/// The trait declaration a *trait type* names (as opposed to a trait-object
/// type). Returns `None` for builtin traits, which have no declaration.
#[must_use]
pub fn object_trait_of_nominal(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    trait_type: TypeId,
) -> Option<DeclarationId> {
    trait_declaration_of(resolved, typed, trait_type)
}

/// Whether one concrete type is covered by an implementation of a
/// user-declared trait, including an applicable generic implementation.
#[must_use]
pub fn implements_trait(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    target: TypeId,
    trait_declaration: DeclarationId,
) -> bool {
    resolved.impls.iter().any(|block| {
        let Some(implemented) = typed.impl_target_types.get(&block.id).copied() else {
            return false;
        };
        let Some(trait_type) = typed.impl_trait_types.get(&block.id).copied() else {
            return false;
        };
        if trait_declaration_of(resolved, typed, trait_type) != Some(trait_declaration) {
            return false;
        }
        let mut bindings = BTreeMap::new();
        match_type_pattern(typed, implemented, target, &mut bindings)
            && implementation_bounds_hold(resolved, typed, block, &bindings)
    })
}

/// A display name for a trait type that is not a user declaration, such as a
/// compiler-known builtin trait.
fn trait_type_name(resolved: &ResolvedProgram, typed: &TypedProgram, trait_type: TypeId) -> String {
    match typed.types.kind(typed.types.resolve_inference(trait_type)) {
        TypeKind::Builtin { builtin, .. } => resolved.builtin_name(*builtin).to_string(),
        TypeKind::Nominal { identity, .. } => declaration_name(resolved, identity.declaration),
        _ => "trait".to_string(),
    }
}

/// A diagnostic name for a canonical trait type.
#[must_use]
pub fn trait_name(resolved: &ResolvedProgram, typed: &TypedProgram, trait_type: TypeId) -> String {
    trait_type_name(resolved, typed, trait_type)
}

/// The vtable layout of a trait: every method reachable through an object,
/// ordered by name.
///
/// Ordering by name rather than declaration order keeps the layout stable
/// against source reordering, which `ROADMAP.md` 2.2 requires of anything that
/// reaches generated output. Default methods participate (`SPEC.md` 6).
#[must_use]
pub fn vtable_slots(
    resolved: &ResolvedProgram,
    trait_declaration: DeclarationId,
) -> Vec<(String, DeclarationId)> {
    trait_methods(resolved, trait_declaration)
        .into_iter()
        .map(|(name, (method, _))| (name, method))
        .collect()
}

/// The declaration implementing `slot_name` for `target`, used to fill one
/// vtable slot. Falls back to the trait's default body.
#[must_use]
pub fn vtable_entry(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    trait_declaration: DeclarationId,
    target: TypeId,
    slot_name: &str,
) -> Option<SelectedTraitMethod> {
    for block in &resolved.impls {
        let Some(implemented) = typed.impl_target_types.get(&block.id).copied() else {
            continue;
        };
        let mut bindings = BTreeMap::new();
        if !match_type_pattern(typed, implemented, target, &mut bindings) {
            continue;
        }
        if !implementation_bounds_hold(resolved, typed, block, &bindings) {
            continue;
        }
        let implementation_arguments = block
            .generic_parameters
            .iter()
            .filter_map(|parameter| bindings.get(parameter).copied())
            .collect::<Vec<_>>();
        let Some(trait_type) = typed.impl_trait_types.get(&block.id).copied() else {
            continue;
        };
        if trait_declaration_of(resolved, typed, trait_type) != Some(trait_declaration) {
            continue;
        }
        if let Some(members) = resolved.impl_members.get(&block.id)
            && let Some((_, declaration)) = members
                .iter()
                .find(|(symbol, _)| resolved.symbol_text(**symbol) == slot_name)
        {
            return Some(SelectedTraitMethod {
                declaration: *declaration,
                trait_declaration: Some(trait_declaration),
                self_type: None,
                arguments: implementation_arguments,
            });
        }
        if let Some((method, true)) = trait_methods(resolved, trait_declaration)
            .get(slot_name)
            .copied()
        {
            return Some(SelectedTraitMethod {
                declaration: method,
                trait_declaration: Some(trait_declaration),
                self_type: Some(target),
                arguments: Vec::new(),
            });
        }
    }
    None
}

/// Whether a struct or enum declaration derives `trait_name`.
///
/// Derivations are a fixed, compiler-supported list written in the
/// declaration's parentheses (`SPEC.md` 4.3); user traits cannot define derive
/// behavior, so this is a syntactic lookup rather than an impl search.
#[must_use]
pub fn derives(resolved: &ResolvedProgram, declaration: DeclarationId, trait_name: &str) -> bool {
    let syntax = &resolved.declarations[declaration.index()].syntax;
    let Some(list) = crate::syntax::direct_child(syntax, crate::syntax::SyntaxKind::DeriveList)
    else {
        return false;
    };
    list.children.iter().any(|child| match child {
        crate::syntax::SyntaxElement::Token(token) => {
            matches!(&token.kind, crate::lexer::TokenKind::Identifier(name) if name == trait_name)
        }
        crate::syntax::SyntaxElement::Node(_) => false,
    })
}

/// Whether the compiler supplies `trait_name` for `declaration` without a
/// source derive list, because the specification gives that exact standard
/// declaration the capability directly.
///
/// The only such rule today is `SPEC.md` 4.3: `Option[T]` defaults to
/// `Option.None` *without* requiring `T` to implement `Default`. That is
/// unconditional, so unlike a derivation it imposes no obligation on the
/// declaration's field types. A user enum spelled `Option` is unaffected.
#[must_use]
pub fn intrinsic_derivation(
    resolved: &ResolvedProgram,
    declaration: DeclarationId,
    trait_name: &str,
) -> bool {
    trait_name == "Default" && resolved.is_standard_declaration(declaration, "Option")
}

/// Whether `declaration` derives `trait_name` from its source derive list or
/// receives it intrinsically. Use this wherever a pass asks "can this type
/// supply the trait", and [`derives`] only where the source list itself is the
/// question.
#[must_use]
pub fn derives_or_intrinsic(
    resolved: &ResolvedProgram,
    declaration: DeclarationId,
    trait_name: &str,
) -> bool {
    derives(resolved, declaration, trait_name)
        || intrinsic_derivation(resolved, declaration, trait_name)
}

/// The variant an intrinsically `Default` enum defaults to: `Option.None`
/// (`SPEC.md` 4.3). Ordinary enums derive no default, so this is the complete
/// list.
#[must_use]
pub fn intrinsic_default_variant(
    resolved: &ResolvedProgram,
    declaration: DeclarationId,
) -> Option<VariantId> {
    resolved
        .is_standard_declaration(declaration, "Option")
        .then(|| resolved.standard_variant("Option", "None"))
        .flatten()
}

/// Whether a type has a compiler-known capability or an applicable manual
/// implementation.
///
/// Derived implementations are conditional: a generic aggregate has the
/// capability only for instantiations whose participating fields have it.
/// `StableHash` is deliberately excluded from manual implementation lookup and
/// is inferred only from compiler-known leaves and compiler-derived `Eq` and
/// `Hash`.
#[must_use]
pub fn provides(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    ty: TypeId,
    trait_name: &str,
) -> bool {
    provides_inner(
        resolved,
        typed,
        ty,
        trait_name,
        &BTreeMap::new(),
        false,
        &mut BTreeSet::new(),
    )
}

fn provides_inner(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    ty: TypeId,
    trait_name: &str,
    substitution: &BTreeMap<GenericParameterId, TypeId>,
    assume_parameters: bool,
    visiting: &mut BTreeSet<(TypeId, String)>,
) -> bool {
    let ty = typed.types.resolve_inference(ty);
    if let TypeKind::GenericParameter(parameter) = typed.types.kind(ty) {
        if let Some(argument) = substitution.get(parameter)
            && !typed.types.exactly_equal(*argument, ty)
        {
            return provides_inner(
                resolved,
                typed,
                *argument,
                trait_name,
                substitution,
                assume_parameters,
                visiting,
            );
        }
        if assume_parameters {
            return true;
        }
        return typed.obligations_for(*parameter).any(|obligation| {
            capability_implies(
                &trait_type_name(resolved, typed, obligation.trait_type),
                trait_name,
            )
        });
    }
    if let TypeKind::Alias { target, .. } = typed.types.kind(ty) {
        return provides_inner(
            resolved,
            typed,
            *target,
            trait_name,
            substitution,
            assume_parameters,
            visiting,
        );
    }

    let visit = (ty, trait_name.to_string());
    if !visiting.insert(visit.clone()) {
        return false;
    }
    let result = match typed.types.kind(ty) {
        TypeKind::Primitive(primitive) => primitive_provides(*primitive, trait_name),
        TypeKind::Tuple(elements) => elements.iter().all(|element| {
            provides_inner(
                resolved,
                typed,
                *element,
                trait_name,
                substitution,
                assume_parameters,
                visiting,
            )
        }),
        TypeKind::Array { element, .. } => provides_inner(
            resolved,
            typed,
            *element,
            trait_name,
            substitution,
            assume_parameters,
            visiting,
        ),
        TypeKind::Reference { target, .. } => {
            matches!(trait_name, "PartialEq" | "Eq" | "Hash")
                || (trait_name == "Display"
                    && provides_inner(
                        resolved,
                        typed,
                        *target,
                        trait_name,
                        substitution,
                        assume_parameters,
                        visiting,
                    ))
        }
        TypeKind::TraitObject { trait_type } => {
            matches!(trait_name, "PartialEq" | "Eq" | "Hash")
                || (trait_name == "Display"
                    && matches!(
                        typed.types.kind(typed.types.resolve_inference(*trait_type)),
                        TypeKind::Nominal { identity, .. }
                            if resolved.is_standard_declaration(
                                identity.declaration,
                                "Display"
                            )
                    ))
        }
        TypeKind::RawPointer { .. } => {
            matches!(trait_name, "Default" | "PartialEq" | "Eq" | "Hash")
        }
        TypeKind::Function { .. } => matches!(trait_name, "PartialEq" | "Eq"),
        TypeKind::Builtin { builtin, arguments } => {
            let name = resolved.builtin_name(*builtin);
            match (name, trait_name) {
                ("Vec" | "Map" | "Set", "Default") => true,
                ("Vec" | "Map" | "Set", "Display") => arguments.iter().all(|argument| {
                    provides_inner(
                        resolved,
                        typed,
                        *argument,
                        trait_name,
                        substitution,
                        assume_parameters,
                        visiting,
                    )
                }),
                ("Vec", "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Hash") => {
                    arguments.first().is_some_and(|argument| {
                        provides_inner(
                            resolved,
                            typed,
                            *argument,
                            trait_name,
                            substitution,
                            assume_parameters,
                            visiting,
                        )
                    })
                }
                ("Map" | "Set", "PartialEq" | "Eq" | "Hash") => arguments.iter().all(|argument| {
                    provides_inner(
                        resolved,
                        typed,
                        *argument,
                        trait_name,
                        substitution,
                        assume_parameters,
                        visiting,
                    )
                }),
                ("Identity", "PartialEq" | "Eq" | "Hash" | "StableHash") => true,
                ("String" | "Vec" | "Map" | "Set", "StableHash") => false,
                (_, "StableHash") => arguments.iter().all(|argument| {
                    provides_inner(
                        resolved,
                        typed,
                        *argument,
                        trait_name,
                        substitution,
                        assume_parameters,
                        visiting,
                    )
                }),
                (_, "PartialEq" | "Eq" | "PartialOrd" | "Ord" | "Hash") => {
                    arguments.iter().all(|argument| {
                        provides_inner(
                            resolved,
                            typed,
                            *argument,
                            trait_name,
                            substitution,
                            assume_parameters,
                            visiting,
                        )
                    })
                }
                _ => false,
            }
        }
        TypeKind::Nominal {
            identity,
            arguments,
        } => {
            nominal_provides(
                resolved,
                typed,
                identity.declaration,
                arguments,
                trait_name,
                substitution,
                assume_parameters,
                visiting,
            ) || (trait_name != "StableHash"
                && manual_implementation_provides(
                    resolved,
                    typed,
                    ty,
                    trait_name,
                    assume_parameters,
                    visiting,
                ))
        }
        TypeKind::GenericParameter(_)
        | TypeKind::Alias { .. }
        | TypeKind::Error
        | TypeKind::Slice(_)
        | TypeKind::Foreign { .. }
        | TypeKind::SelfType(_)
        | TypeKind::InferenceVariable(_) => false,
    };
    visiting.remove(&visit);
    result
}

fn primitive_provides(primitive: PrimitiveType, trait_name: &str) -> bool {
    match trait_name {
        "Default" | "PartialEq" | "PartialOrd" | "Display" => true,
        "Eq" | "Ord" | "Hash" => !primitive.is_float(),
        "StableHash" => !primitive.is_float() && primitive != PrimitiveType::String,
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
fn nominal_provides(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    declaration: DeclarationId,
    arguments: &[TypeId],
    trait_name: &str,
    outer_substitution: &BTreeMap<GenericParameterId, TypeId>,
    assume_parameters: bool,
    visiting: &mut BTreeSet<(TypeId, String)>,
) -> bool {
    // An intrinsic capability is unconditional, so it is decided before the
    // field obligations a derivation would impose (`SPEC.md` 4.3).
    if intrinsic_derivation(resolved, declaration, trait_name) {
        return true;
    }
    let derivation = if trait_name == "StableHash" {
        derives(resolved, declaration, "Eq") && derives(resolved, declaration, "Hash")
    } else {
        derives(resolved, declaration, trait_name)
    };
    if !derivation {
        return false;
    }
    if trait_name == "Default"
        && resolved.declarations[declaration.index()].kind != DeclarationKind::Struct
    {
        return false;
    }
    let mut substitution = outer_substitution.clone();
    for (parameter, argument) in resolved.declarations[declaration.index()]
        .generic_parameters
        .iter()
        .zip(arguments)
    {
        substitution.insert(*parameter, *argument);
    }
    resolved
        .fields
        .iter()
        .filter(|field| field.parent_declaration == declaration)
        .all(|field| {
            typed.field_types.get(&field.id).is_some_and(|field_type| {
                provides_inner(
                    resolved,
                    typed,
                    *field_type,
                    trait_name,
                    &substitution,
                    assume_parameters,
                    visiting,
                )
            })
        })
}

fn manual_implementation_provides(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    ty: TypeId,
    trait_name: &str,
    assume_parameters: bool,
    visiting: &mut BTreeSet<(TypeId, String)>,
) -> bool {
    resolved.impls.iter().any(|block| {
        let Some(target) = typed.impl_target_types.get(&block.id).copied() else {
            return false;
        };
        let Some(trait_type) = typed.impl_trait_types.get(&block.id).copied() else {
            return false;
        };
        if trait_type_name(resolved, typed, trait_type) != trait_name {
            return false;
        }
        let mut bindings = BTreeMap::new();
        if !match_type_pattern(typed, target, ty, &mut bindings) {
            return false;
        }
        block.generic_parameters.iter().all(|parameter| {
            typed.obligations_for(*parameter).all(|obligation| {
                let required = trait_type_name(resolved, typed, obligation.trait_type);
                bindings.get(parameter).is_some_and(|argument| {
                    provides_inner(
                        resolved,
                        typed,
                        *argument,
                        &required,
                        &BTreeMap::new(),
                        assume_parameters,
                        visiting,
                    )
                })
            })
        })
    })
}

fn implementation_bounds_hold(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    block: &crate::resolution::ImplBlock,
    bindings: &BTreeMap<GenericParameterId, TypeId>,
) -> bool {
    block.generic_parameters.iter().all(|parameter| {
        typed.obligations_for(*parameter).all(|obligation| {
            let required = trait_type_name(resolved, typed, obligation.trait_type);
            bindings
                .get(parameter)
                .is_some_and(|argument| provides(resolved, typed, *argument, &required))
        })
    })
}

fn capability_implies(provided: &str, required: &str) -> bool {
    provided == required
        || matches!(
            (provided, required),
            ("Eq" | "Ord", "PartialEq") | ("Ord", "PartialOrd") | ("StableHash", "Eq" | "Hash")
        )
}

/// The name of a declaration, for diagnostics.
#[must_use]
pub fn name_of(resolved: &ResolvedProgram, declaration: DeclarationId) -> String {
    declaration_name(resolved, declaration)
}

/// Why a trait cannot form a trait object, or `None` when it can.
///
/// `SPEC.md` 6: every method reachable through the object needs an `&Self` or
/// `&var Self` receiver, no method-level generic parameters, and no other
/// mention of `Self` in its parameters or return type. A trait failing these
/// rules stays usable with static dispatch.
#[must_use]
pub fn object_safety_violation(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    trait_declaration: DeclarationId,
) -> Option<String> {
    for (name, (method, _)) in trait_methods(resolved, trait_declaration) {
        let data = &resolved.declarations[method.index()];
        if !data.generic_parameters.is_empty() {
            return Some(format!("method `{name}` has its own generic parameters"));
        }
        let Some(signature) = typed.function_signatures.get(&method) else {
            continue;
        };
        match signature.receiver {
            Some(receiver) if is_self_reference(typed, receiver) => {}
            Some(_) => {
                return Some(format!(
                    "method `{name}` does not take an `&Self` or `&var Self` receiver"
                ));
            }
            None => {
                return Some(format!("method `{name}` has no receiver"));
            }
        }
        for parameter in &signature.parameters {
            if mentions_self(typed, parameter.ty) {
                return Some(format!("method `{name}` mentions `Self` in a parameter"));
            }
        }
        if mentions_self(typed, signature.return_type) {
            return Some(format!("method `{name}` returns `Self`"));
        }
    }
    None
}

/// Whether a receiver type is exactly `&Self` or `&var Self`.
fn is_self_reference(typed: &TypedProgram, receiver: TypeId) -> bool {
    match typed.types.kind(typed.types.resolve_inference(receiver)) {
        TypeKind::Reference { target, .. } => {
            matches!(
                typed.types.kind(typed.types.resolve_inference(*target)),
                TypeKind::SelfType(_)
            )
        }
        _ => false,
    }
}

fn mentions_self(typed: &TypedProgram, ty: TypeId) -> bool {
    typed.types.mentions_self(ty)
}

/// Whether a declaration is publicly visible, used by derivation work.
#[must_use]
pub fn is_public(resolved: &ResolvedProgram, declaration: DeclarationId) -> bool {
    resolved.declarations[declaration.index()].visibility == Visibility::Public
}
