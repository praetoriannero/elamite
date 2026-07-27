//! Trait implementation checking (`IMPL.md` Milestone 13).
//!
//! This pass validates declarations rather than bodies: whether each
//! `impl Trait for Type` supplies exactly the trait's methods with exact
//! signatures, whether the program's implementations are coherent, and whether
//! a trait may form a trait object. Body-level trait *use* — bound-call
//! selection and dispatch — belongs to the checker and the backend.

use std::collections::BTreeMap;

use crate::diagnostics::{Category, Diagnostic};
use crate::package::PackageId;
use crate::resolution::{DeclarationId, DeclarationKind, ImplId, ResolvedProgram, Visibility};
use crate::source::Span;
use crate::types::{FunctionSignature, TypeId, TypeKind, TypedProgram};

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
    let implementations = collect(resolved, typed);
    for implementation in &implementations {
        check_conformance(resolved, typed, *implementation, &mut output.diagnostics);
    }
    check_coherence(resolved, typed, &implementations, &mut output.diagnostics);
    output.implementations = implementations;
    output
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
            crate::types::direct_child(&data.syntax, crate::parser::SyntaxKind::Block).is_some();
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
                && typed.types.exactly_equal(*target, implementation.target)
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
#[derive(Debug, Clone, Copy)]
pub struct SelectedTraitMethod {
    /// The declaration to call: an implementation's method, or the trait's
    /// default body.
    pub declaration: DeclarationId,
    /// `None` for a compiler-known builtin trait, which has no declaration.
    pub trait_declaration: Option<DeclarationId>,
    /// `Some` when `declaration` is a default body, which is written against
    /// `Self` and must be specialized for this receiver.
    pub self_type: Option<TypeId>,
}

/// Selects a trait method named `name` for `target`.
///
/// This searches every implementation for the type, including implementations
/// of compiler-known traits such as `Close`, which are builtins rather than
/// user declarations. An implementation's own method is preferred over a
/// trait's default body. `Err` lists the competing trait names when more than
/// one supplies the name, which the caller reports as an ambiguity rather than
/// choosing arbitrarily.
pub fn select_trait_method(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    target: TypeId,
    name: &str,
) -> Result<Option<SelectedTraitMethod>, Vec<String>> {
    let mut found: Vec<(SelectedTraitMethod, String)> = Vec::new();
    for block in &resolved.impls {
        let Some(implemented) = typed.impl_target_types.get(&block.id).copied() else {
            continue;
        };
        if !typed.types.exactly_equal(implemented, target) {
            continue;
        }
        let Some(trait_type) = typed.impl_trait_types.get(&block.id).copied() else {
            continue;
        };
        let trait_declaration = trait_declaration_of(resolved, typed, trait_type);
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
                    self_type: Some(implemented),
                },
                trait_name,
            ));
        }
    }
    match found.len() {
        0 => Ok(None),
        1 => Ok(Some(found[0].0)),
        _ => Err(found.into_iter().map(|(_, name)| name).collect()),
    }
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

/// A display name for a trait type that is not a user declaration, such as a
/// compiler-known builtin trait.
fn trait_type_name(resolved: &ResolvedProgram, typed: &TypedProgram, trait_type: TypeId) -> String {
    match typed.types.kind(typed.types.resolve_inference(trait_type)) {
        TypeKind::Builtin { builtin, .. } => resolved.builtin_name(*builtin).to_string(),
        _ => "trait".to_string(),
    }
}

/// The vtable layout of a trait: every method reachable through an object,
/// ordered by name.
///
/// Ordering by name rather than declaration order keeps the layout stable
/// against source reordering, which `IMPL.md` 2.2 requires of anything that
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
        if !typed.types.exactly_equal(implemented, target) {
            continue;
        }
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
            });
        }
        if let Some((method, true)) = trait_methods(resolved, trait_declaration)
            .get(slot_name)
            .copied()
        {
            return Some(SelectedTraitMethod {
                declaration: method,
                trait_declaration: Some(trait_declaration),
                self_type: Some(implemented),
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
    let Some(list) = crate::types::direct_child(syntax, crate::parser::SyntaxKind::DeriveList)
    else {
        return false;
    };
    list.children.iter().any(|child| match child {
        crate::parser::SyntaxElement::Token(token) => {
            matches!(&token.kind, crate::lexer::TokenKind::Identifier(name) if name == trait_name)
        }
        crate::parser::SyntaxElement::Node(_) => false,
    })
}

/// Whether a type derives or manually implements `trait_name`, which is what
/// an operator or associated-function synthesis needs to know.
#[must_use]
pub fn provides(
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    ty: TypeId,
    trait_name: &str,
) -> bool {
    let resolved_type = typed.types.resolve_inference(ty);
    if let TypeKind::Nominal { identity, .. } = typed.types.kind(resolved_type)
        && derives(resolved, identity.declaration, trait_name)
    {
        return true;
    }
    resolved.impls.iter().any(|block| {
        typed
            .impl_target_types
            .get(&block.id)
            .is_some_and(|target| typed.types.exactly_equal(*target, resolved_type))
            && typed
                .impl_trait_types
                .get(&block.id)
                .is_some_and(|trait_type| {
                    match typed.types.kind(typed.types.resolve_inference(*trait_type)) {
                        TypeKind::Builtin { builtin, .. } => {
                            resolved.builtin_name(*builtin) == trait_name
                        }
                        TypeKind::Nominal { identity, .. } => {
                            declaration_name(resolved, identity.declaration) == trait_name
                        }
                        _ => false,
                    }
                })
    })
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
