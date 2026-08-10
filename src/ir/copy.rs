//! Logical value-copy contract shared by lowering and the backend.

use crate::operations::LogicalCopyAllocation;
use crate::types::{PrimitiveType, TypeContext, TypeId, TypeKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalCopyStrategy {
    Trivial,
    PreserveIdentity,
    Recursive,
    MutableString,
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
            TypeKind::Primitive(PrimitiveType::String) => LogicalCopyStrategy::MutableString,
            TypeKind::Primitive(_) => LogicalCopyStrategy::Trivial,
            TypeKind::Tuple(_)
            | TypeKind::Array { .. }
            | TypeKind::Nominal { .. }
            | TypeKind::Closure { .. } => LogicalCopyStrategy::Recursive,
            TypeKind::Reference { .. }
            | TypeKind::RawPointer { .. }
            | TypeKind::Function { .. }
            | TypeKind::TraitObject { .. }
            | TypeKind::Slice { .. } => LogicalCopyStrategy::PreserveIdentity,
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

/// Classify the physical work required by one recorded copy.
///
/// Ordinary 0.10 copies never recursively duplicate reachable backing.
#[must_use]
pub fn logical_copy_allocation(types: &TypeContext, mut ty: TypeId) -> LogicalCopyAllocation {
    loop {
        return match types.kind(ty) {
            TypeKind::Alias { target, .. } => {
                ty = *target;
                continue;
            }
            TypeKind::Never
            | TypeKind::Reference { .. }
            | TypeKind::RawPointer { .. }
            | TypeKind::Function { .. }
            | TypeKind::TraitObject { .. }
            | TypeKind::Slice { .. }
            | TypeKind::Closure { .. }
            | TypeKind::Builtin { .. } => LogicalCopyAllocation::PreserveIdentity,
            TypeKind::Primitive(PrimitiveType::String) => LogicalCopyAllocation::SharedBacking,
            TypeKind::Primitive(_) | TypeKind::Foreign { complete: true, .. } => {
                LogicalCopyAllocation::None
            }
            TypeKind::Tuple(_) | TypeKind::Array { .. } | TypeKind::Nominal { .. } => {
                LogicalCopyAllocation::Shallow
            }
            TypeKind::Foreign {
                complete: false, ..
            }
            | TypeKind::GenericParameter(_)
            | TypeKind::SelfType(_)
            | TypeKind::InferenceVariable(_)
            | TypeKind::Error => LogicalCopyAllocation::RuntimeManaged,
        };
    }
}
