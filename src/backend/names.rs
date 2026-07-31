//! Deterministic C symbol and literal spelling.

use std::fmt::Write as _;

use crate::operations::{NumericAlternative, NumericOutcome, StandardCall};
use crate::resolution::{DeclarationId, FieldId, LocalBindingId, VariantId};
use crate::types::{TypeId, TypedProgram};

use super::TemporaryId;

pub(super) fn struct_name(declaration: DeclarationId, ty: TypeId) -> String {
    format!("el_struct_d{}_t{}", declaration.index(), ty.index())
}

pub(super) fn enum_name(declaration: DeclarationId, ty: TypeId) -> String {
    format!("el_enum_d{}_t{}", declaration.index(), ty.index())
}

pub(super) fn slice_name(ty: TypeId) -> String {
    format!("el_slice_t{}", ty.index())
}

pub(super) fn function_type_name(ty: TypeId) -> String {
    format!("el_fn_t{}", ty.index())
}

pub(super) fn closure_name(ty: TypeId) -> String {
    format!("el_closure_t{}", ty.index())
}

pub(super) fn numeric_alternative_name(
    operation: NumericAlternative,
    operand: TypeId,
    result: TypeId,
) -> String {
    format!(
        "el_{}_t{}_t{}",
        operation.name,
        operand.index(),
        result.index()
    )
}

pub(super) fn pointer_check_name(pointee: TypeId) -> String {
    format!("el_check_ptr_t{}", pointee.index())
}

pub(super) fn numeric_conversion_name(
    outcome: NumericOutcome,
    source: TypeId,
    result: TypeId,
) -> String {
    let stem = match outcome {
        NumericOutcome::Checked => "try_from",
        NumericOutcome::Wrapping => "wrapping_from",
        NumericOutcome::Saturating => "saturating_from",
    };
    format!("el_{stem}_t{}_t{}", source.index(), result.index())
}

pub(super) fn standard_call_name(operation: StandardCall) -> String {
    use StandardCall::{
        ArrayGet, ArrayLen, MapClear, MapContainsKey, MapGet, MapInsert, MapIsEmpty, MapLen,
        MapNew, MapRemove, SetClear, SetContains, SetInsert, SetIsEmpty, SetLen, SetNew, SetRemove,
        StringFrom, VecAppend, VecClear, VecGet, VecInsert, VecIsEmpty, VecLen, VecNew, VecRemove,
    };
    let (name, ty) = match operation {
        StandardCall::Panic => return "el_panic".to_string(),
        StandardCall::Assert => return "el_assert".to_string(),
        StandardCall::Fail { .. } => return "el_assert_fail".to_string(),
        StandardCall::Trap { .. } => return "el_typed_trap".to_string(),
        StringFrom => return "el_string_from".to_string(),
        StandardCall::IdentityFrom { wrapper } => ("identity_from", wrapper),
        StandardCall::ForeignRootRetain { handle, .. } => ("foreign_root_retain", handle),
        StandardCall::ForeignRootPointer { handle, .. } => ("foreign_root_pointer", handle),
        StandardCall::ForeignRootClose { handle } => ("foreign_root_close", handle),
        StandardCall::FormatterWrite { formatter } => ("formatter_write", formatter),
        ArrayLen { collection } => ("array_len", collection),
        ArrayGet { collection } => ("array_get", collection),
        VecNew { collection } => ("vec_new", collection),
        VecLen { collection } => ("vec_len", collection),
        VecIsEmpty { collection } => ("vec_is_empty", collection),
        VecGet { collection } => ("vec_get", collection),
        VecAppend { collection } => ("vec_append", collection),
        VecInsert { collection } => ("vec_insert", collection),
        VecRemove { collection } => ("vec_remove", collection),
        VecClear { collection } => ("vec_clear", collection),
        MapNew { collection } => ("map_new", collection),
        MapLen { collection } => ("map_len", collection),
        MapIsEmpty { collection } => ("map_is_empty", collection),
        MapContainsKey { collection } => ("map_contains_key", collection),
        MapGet { collection } => ("map_get", collection),
        MapInsert { collection } => ("map_insert", collection),
        MapRemove { collection } => ("map_remove", collection),
        MapClear { collection } => ("map_clear", collection),
        SetNew { collection } => ("set_new", collection),
        SetLen { collection } => ("set_len", collection),
        SetIsEmpty { collection } => ("set_is_empty", collection),
        SetContains { collection } => ("set_contains", collection),
        SetInsert { collection } => ("set_insert", collection),
        SetRemove { collection } => ("set_remove", collection),
        SetClear { collection } => ("set_clear", collection),
    };
    format!("el_{name}_t{}", ty.index())
}

pub(super) fn variant_member_name(variant: VariantId) -> String {
    format!("v{}", variant.index())
}

pub(super) fn tuple_name(ty: TypeId) -> String {
    format!("el_tuple_t{}", ty.index())
}

pub(super) fn array_name(ty: TypeId) -> String {
    format!("el_array_t{}", ty.index())
}

pub(super) fn collection_type_name(ty: TypeId) -> String {
    format!("el_runtime_t{}", ty.index())
}

pub(super) fn field_name(field: FieldId) -> String {
    format!("f{}", field.index())
}

pub(super) fn copy_helper_name(ty: TypeId) -> String {
    format!("el_copy_t{}", ty.index())
}

pub(super) fn local_name(binding: LocalBindingId) -> String {
    format!("l{}", binding.index())
}

pub(super) fn cell_name(binding: LocalBindingId) -> String {
    format!("c{}", binding.index())
}

pub(super) fn equality_helper_name(ty: TypeId) -> String {
    format!("el_eq_t{}", ty.index())
}

pub(super) fn ordering_helper_name(ty: TypeId) -> String {
    format!("el_ord_t{}", ty.index())
}

pub(super) fn hash_helper_name(ty: TypeId) -> String {
    format!("el_hash_t{}", ty.index())
}

pub(super) fn default_helper_name(ty: TypeId) -> String {
    format!("el_default_t{}", ty.index())
}

pub(super) fn object_name(trait_declaration: DeclarationId, trait_type: TypeId) -> String {
    format!(
        "el_obj{}_t{}",
        trait_declaration.index(),
        trait_type.index()
    )
}

pub(super) fn vtable_type_name(trait_declaration: DeclarationId, trait_type: TypeId) -> String {
    format!("el_vt{}_t{}", trait_declaration.index(), trait_type.index())
}

pub(super) fn vtable_slot_name(slot: usize) -> String {
    format!("m{slot}")
}

pub(super) fn vtable_instance_name(
    typed: &TypedProgram,
    trait_declaration: DeclarationId,
    trait_type: TypeId,
    concrete: TypeId,
) -> String {
    let _ = typed;
    format!(
        "el_vtbl{}_{}_{}",
        trait_declaration.index(),
        trait_type.index(),
        concrete.index()
    )
}

pub(super) fn thunk_name(
    trait_declaration: DeclarationId,
    trait_type: TypeId,
    concrete: TypeId,
    slot: usize,
) -> String {
    format!(
        "el_thunk{}_{}_{}_{slot}",
        trait_declaration.index(),
        trait_type.index(),
        concrete.index()
    )
}

pub(super) fn temporary_name(temporary: TemporaryId) -> String {
    format!("t{}", temporary.index())
}

pub(super) fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub(super) fn c_string(value: &str) -> String {
    let mut escaped = String::from("\"");
    for byte in value.as_bytes() {
        match byte {
            b'\\' => escaped.push_str("\\\\"),
            b'"' => escaped.push_str("\\\""),
            b'\n' => escaped.push_str("\\n"),
            b'\r' => escaped.push_str("\\r"),
            b'\t' => escaped.push_str("\\t"),
            0x20..=0x7e => escaped.push(char::from(*byte)),
            _ => {
                let _ = write!(escaped, "\\{:03o}", byte);
            }
        }
    }
    escaped.push('"');
    escaped
}

pub(super) fn c_comment(value: &str) -> String {
    value.replace("*/", "* /").replace('\n', " ")
}
