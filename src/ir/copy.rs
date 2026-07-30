//! Logical value-copy contract shared by lowering and the backend.

use crate::types::{PrimitiveType, TypeContext, TypeId, TypeKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalCopyStrategy {
    Trivial,
    PreserveIdentity,
    Recursive,
    OwnedString,
    RuntimeManaged,
}

#[must_use]
pub fn logical_copy_strategy(types: &TypeContext, mut ty: TypeId) -> LogicalCopyStrategy {
    loop {
        return match types.kind(ty) {
            TypeKind::Alias { target, .. } => {
                ty = *target;
                continue;
            }
            TypeKind::Never => LogicalCopyStrategy::PreserveIdentity,
            TypeKind::Primitive(PrimitiveType::String) => LogicalCopyStrategy::OwnedString,
            TypeKind::Primitive(_) => LogicalCopyStrategy::Trivial,
            TypeKind::Tuple(_) | TypeKind::Array { .. } | TypeKind::Nominal { .. } => {
                LogicalCopyStrategy::Recursive
            }
            TypeKind::Reference { .. }
            | TypeKind::RawPointer { .. }
            | TypeKind::Function { .. }
            | TypeKind::TraitObject { .. }
            | TypeKind::Slice(_) => LogicalCopyStrategy::PreserveIdentity,
            TypeKind::Foreign { complete: true, .. } => LogicalCopyStrategy::Trivial,
            TypeKind::Builtin { .. }
            | TypeKind::Foreign {
                complete: false, ..
            }
            | TypeKind::GenericParameter(_)
            | TypeKind::SelfType(_)
            | TypeKind::InferenceVariable(_)
            | TypeKind::Error => LogicalCopyStrategy::RuntimeManaged,
        };
    }
}
