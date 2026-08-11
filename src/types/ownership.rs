//! Canonical structural ownership-capability queries.

use super::*;
use crate::operations::{CapabilityState, OwnershipFacts};

/// Computes ownership facts from canonical source types, never from backend
/// size or layout. The query is coinductive for positive structural
/// capabilities and conservative for recovery and unresolved types.
#[must_use]
pub fn ownership_facts(
    resolved: &ResolvedProgram,
    typed: &mut TypedProgram,
    ty: TypeId,
) -> OwnershipFacts {
    let cache = std::mem::take(&mut typed.ownership_cache);
    let (facts, cache) = Query {
        resolved,
        typed,
        cache,
        visiting: BTreeSet::new(),
    }
    .run(ty);
    typed.ownership_cache = cache;
    facts
}

struct Query<'a> {
    resolved: &'a ResolvedProgram,
    typed: &'a mut TypedProgram,
    cache: BTreeMap<TypeId, OwnershipFacts>,
    visiting: BTreeSet<TypeId>,
}

impl Query<'_> {
    fn run(mut self, ty: TypeId) -> (OwnershipFacts, BTreeMap<TypeId, OwnershipFacts>) {
        let facts = self.facts(ty);
        (facts, self.cache)
    }

    fn facts(&mut self, ty: TypeId) -> OwnershipFacts {
        let ty = self.typed.types.resolve_inference(ty);
        if let Some(facts) = self.cache.get(&ty).copied() {
            return facts;
        }
        if !self.visiting.insert(ty) {
            if matches!(
                self.typed.types.kind(ty),
                TypeKind::Alias { .. } | TypeKind::Error | TypeKind::InferenceVariable(_)
            ) {
                return OwnershipFacts::ERROR;
            }
            // Recursive ownership equations are monotone. These identities
            // are the optimistic seeds for conjunction capabilities and the
            // empty seed for containment/destruction properties.
            return OwnershipFacts {
                copy: CapabilityState::Present,
                clone: CapabilityState::Present,
                needs_drop: CapabilityState::Absent,
                contains_borrow: CapabilityState::Absent,
                send: CapabilityState::Present,
                sync: CapabilityState::Present,
            };
        }
        let facts = match self.typed.types.kind(ty).clone() {
            TypeKind::Error | TypeKind::InferenceVariable(_) => OwnershipFacts::ERROR,
            TypeKind::Alias { target, .. } => self.facts(target),
            TypeKind::Never => plain(CapabilityState::Present),
            TypeKind::Primitive(primitive) => self.primitive(primitive),
            TypeKind::Tuple(elements) => self.aggregate(&elements, false, false),
            TypeKind::Array { element, .. } => self.aggregate(&[element], false, false),
            TypeKind::Slice {
                mutability,
                element,
            } => self.reference_like(mutability, element),
            TypeKind::Reference { mutability, target } => self.reference_like(mutability, target),
            TypeKind::RawPointer { .. } => OwnershipFacts {
                copy: CapabilityState::Present,
                clone: CapabilityState::Present,
                needs_drop: CapabilityState::Absent,
                contains_borrow: CapabilityState::Absent,
                // Raw pointers carry no proof that cross-thread access is
                // valid, so safe structural capabilities are not inferred.
                send: CapabilityState::Absent,
                sync: CapabilityState::Absent,
            },
            TypeKind::Function { .. } => plain(CapabilityState::Present),
            TypeKind::Closure { captures, .. } => self.aggregate(&captures, false, false),
            TypeKind::Nominal {
                identity,
                arguments,
            } => self.nominal(ty, identity.declaration, &arguments),
            TypeKind::Builtin { builtin, arguments } => {
                self.builtin(self.resolved.builtin_name(builtin), &arguments)
            }
            TypeKind::Foreign { identity, complete } => {
                if complete {
                    let fields = self
                        .typed
                        .foreign_fields
                        .get(&identity.declaration)
                        .cloned()
                        .unwrap_or_default();
                    self.aggregate(&fields, false, false)
                } else {
                    OwnershipFacts::ERROR
                }
            }
            TypeKind::TraitObject { .. } | TypeKind::SelfType(_) => conditional(),
            TypeKind::GenericParameter(parameter) => self.generic(parameter),
        };
        self.visiting.remove(&ty);
        self.cache.insert(ty, facts);
        facts
    }

    fn primitive(&self, primitive: PrimitiveType) -> OwnershipFacts {
        match primitive {
            PrimitiveType::String => OwnershipFacts {
                copy: CapabilityState::Absent,
                clone: CapabilityState::Present,
                needs_drop: CapabilityState::Present,
                contains_borrow: CapabilityState::Absent,
                send: CapabilityState::Present,
                sync: CapabilityState::Present,
            },
            PrimitiveType::Str => OwnershipFacts {
                copy: CapabilityState::Present,
                clone: CapabilityState::Present,
                needs_drop: CapabilityState::Absent,
                contains_borrow: CapabilityState::Present,
                send: CapabilityState::Present,
                sync: CapabilityState::Present,
            },
            _ => plain(CapabilityState::Present),
        }
    }

    fn reference_like(&mut self, mutability: Mutability, target: TypeId) -> OwnershipFacts {
        let target = self.facts(target);
        let shared = mutability == Mutability::Shared;
        OwnershipFacts {
            copy: if shared {
                CapabilityState::Present
            } else {
                CapabilityState::Absent
            },
            clone: if shared {
                CapabilityState::Present
            } else {
                CapabilityState::Absent
            },
            needs_drop: CapabilityState::Absent,
            contains_borrow: CapabilityState::Present,
            send: if shared { target.sync } else { target.send },
            sync: target.sync,
        }
    }

    fn nominal(
        &mut self,
        ty: TypeId,
        declaration: DeclarationId,
        arguments: &[TypeId],
    ) -> OwnershipFacts {
        if self.resolved.declarations[declaration.index()].kind == DeclarationKind::Trait {
            return conditional();
        }
        let templates = self
            .typed
            .nominal_fields
            .get(&declaration)
            .cloned()
            .unwrap_or_default();
        let parameters = self
            .typed
            .nominal_parameters
            .get(&declaration)
            .cloned()
            .unwrap_or_default();
        let mut substitution = Substitution::new();
        for (parameter, argument) in parameters.into_iter().zip(arguments.iter().copied()) {
            substitution.insert(parameter, argument);
        }
        let fields = templates
            .into_iter()
            .map(|field| self.typed.types.substitute(field, &substitution))
            .collect::<Vec<_>>();
        let custom_drop = self.has_trait_impl(ty, "Drop");
        let custom_clone = self.has_trait_impl(ty, "Clone");
        self.aggregate(&fields, custom_drop, custom_clone)
    }

    fn builtin(&mut self, name: &str, arguments: &[TypeId]) -> OwnershipFacts {
        match name {
            "Copy" | "Send" | "Sync" | "Default" | "PartialEq" | "Eq" | "PartialOrd" | "Ord"
            | "Hash" | "StableHash" | "Display" | "Formatter" => conditional(),
            "Vec" | "Map" | "Set" | "Box" => {
                let mut facts = self.aggregate(arguments, true, false);
                facts.copy = CapabilityState::Absent;
                facts.clone = conjunction(arguments.iter().map(|ty| self.facts(*ty).clone));
                facts
            }
            "ForeignRoot" | "ForeignRootMut" | "File" | "Directory" | "Thread" | "Sender"
            | "Receiver" | "Mutex" => OwnershipFacts {
                copy: CapabilityState::Absent,
                clone: CapabilityState::Absent,
                needs_drop: CapabilityState::Present,
                contains_borrow: disjunction(
                    arguments.iter().map(|ty| self.facts(*ty).contains_borrow),
                ),
                send: conjunction(arguments.iter().map(|ty| self.facts(*ty).send)),
                sync: conjunction(arguments.iter().map(|ty| self.facts(*ty).sync)),
            },
            "AtomicBool" | "AtomicI32" | "AtomicUsize" => OwnershipFacts {
                copy: CapabilityState::Absent,
                clone: CapabilityState::Absent,
                needs_drop: CapabilityState::Absent,
                contains_borrow: CapabilityState::Absent,
                send: CapabilityState::Present,
                sync: CapabilityState::Present,
            },
            // Unknown opaque runtime handles are conservatively move-only.
            _ => OwnershipFacts {
                copy: CapabilityState::Absent,
                clone: CapabilityState::Absent,
                needs_drop: CapabilityState::Conditional,
                contains_borrow: disjunction(
                    arguments.iter().map(|ty| self.facts(*ty).contains_borrow),
                ),
                send: CapabilityState::Conditional,
                sync: CapabilityState::Conditional,
            },
        }
    }

    fn aggregate(
        &mut self,
        fields: &[TypeId],
        custom_drop: bool,
        custom_clone: bool,
    ) -> OwnershipFacts {
        let children = fields
            .iter()
            .map(|field| self.facts(*field))
            .collect::<Vec<_>>();
        let mut copy = conjunction(children.iter().map(|facts| facts.copy));
        if custom_drop {
            copy = CapabilityState::Absent;
        }
        OwnershipFacts {
            copy,
            clone: if custom_clone || copy == CapabilityState::Present {
                CapabilityState::Present
            } else if copy == CapabilityState::Error {
                CapabilityState::Error
            } else {
                CapabilityState::Absent
            },
            needs_drop: if custom_drop {
                CapabilityState::Present
            } else {
                disjunction(children.iter().map(|facts| facts.needs_drop))
            },
            contains_borrow: disjunction(children.iter().map(|facts| facts.contains_borrow)),
            send: conjunction(children.iter().map(|facts| facts.send)),
            sync: conjunction(children.iter().map(|facts| facts.sync)),
        }
    }

    fn generic(&self, parameter: GenericParameterId) -> OwnershipFacts {
        let bound = |name| {
            self.typed.obligations.iter().any(|obligation| {
                obligation.parameter == parameter
                    && self.type_name(obligation.trait_type) == Some(name)
            })
        };
        OwnershipFacts {
            copy: state_if_bound(bound("Copy")),
            clone: state_if_bound(bound("Clone") || bound("Copy")),
            needs_drop: CapabilityState::Conditional,
            contains_borrow: CapabilityState::Conditional,
            send: state_if_bound(bound("Send")),
            sync: state_if_bound(bound("Sync")),
        }
    }

    fn has_trait_impl(&self, ty: TypeId, name: &str) -> bool {
        self.resolved.impls.iter().any(|implementation| {
            let Some(trait_type) = self.typed.impl_trait_types.get(&implementation.id).copied()
            else {
                return false;
            };
            let Some(target) = self
                .typed
                .impl_target_types
                .get(&implementation.id)
                .copied()
            else {
                return false;
            };
            self.type_name(trait_type) == Some(name) && self.typed.types.exactly_equal(target, ty)
        })
    }

    fn type_name(&self, mut ty: TypeId) -> Option<&str> {
        while let TypeKind::Alias { target, .. } = self.typed.types.kind(ty) {
            ty = *target;
        }
        match self.typed.types.kind(ty) {
            TypeKind::Nominal { identity, .. } => Some(
                self.resolved
                    .symbol_text(self.resolved.declarations[identity.declaration.index()].name),
            ),
            TypeKind::Builtin { builtin, .. } => Some(self.resolved.builtin_name(*builtin)),
            _ => None,
        }
    }
}

fn state_if_bound(bound: bool) -> CapabilityState {
    if bound {
        CapabilityState::Present
    } else {
        CapabilityState::Conditional
    }
}

fn plain(capability: CapabilityState) -> OwnershipFacts {
    OwnershipFacts {
        copy: capability,
        clone: capability,
        needs_drop: CapabilityState::Absent,
        contains_borrow: CapabilityState::Absent,
        send: capability,
        sync: capability,
    }
}

fn conditional() -> OwnershipFacts {
    OwnershipFacts {
        copy: CapabilityState::Conditional,
        clone: CapabilityState::Conditional,
        needs_drop: CapabilityState::Conditional,
        contains_borrow: CapabilityState::Conditional,
        send: CapabilityState::Conditional,
        sync: CapabilityState::Conditional,
    }
}

fn conjunction(values: impl IntoIterator<Item = CapabilityState>) -> CapabilityState {
    let mut result = CapabilityState::Present;
    for value in values {
        result = match (result, value) {
            (CapabilityState::Error, _) | (_, CapabilityState::Error) => CapabilityState::Error,
            (CapabilityState::Absent, _) | (_, CapabilityState::Absent) => CapabilityState::Absent,
            (CapabilityState::Conditional, _) | (_, CapabilityState::Conditional) => {
                CapabilityState::Conditional
            }
            _ => CapabilityState::Present,
        };
    }
    result
}

fn disjunction(values: impl IntoIterator<Item = CapabilityState>) -> CapabilityState {
    let mut result = CapabilityState::Absent;
    for value in values {
        result = match (result, value) {
            (CapabilityState::Error, _) | (_, CapabilityState::Error) => CapabilityState::Error,
            (CapabilityState::Present, _) | (_, CapabilityState::Present) => {
                CapabilityState::Present
            }
            (CapabilityState::Conditional, _) | (_, CapabilityState::Conditional) => {
                CapabilityState::Conditional
            }
            _ => CapabilityState::Absent,
        };
    }
    result
}
