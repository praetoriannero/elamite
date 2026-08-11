//! Deterministic C99 backend façade and emission orchestration.
//!
//! This is the first executable backend (`docs/roadmap.md` Milestones 8-9). It consumes
//! explicit control-flow IR, uses an internal (unstable) calling convention,
//! emits one strictly sequenced C statement per IR instruction, and routes
//! ordinary copies through direct shallow C representations. Synchronized
//! runtime helpers own only the storage and publication mechanics they need.

mod entry;
mod functions;
mod names;
mod runtime;
mod types;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

pub use crate::config::Target;
use crate::diagnostics::{Category, Diagnostic};
use crate::ir::{
    AggregateValue, BinaryOperator, BlockId, CollectionLiteralKind, ControlFlowFunction,
    ControlFlowPlace, ControlFlowProgram, IndexKind, Instruction, IterationKind, NeverCall,
    RuntimeFormattedPart, Rvalue, TemporaryId, Terminator, TypedEnum, UnaryOperator, ValueModel,
    VtableMethod,
};
use crate::memory::{
    AllocationClass, ManagedMemoryOperation, ManagedMemoryStrategy, default_managed_memory_strategy,
};
use crate::operations::{
    NumericAlternative, NumericOperator, NumericOutcome, StandardCall, ValuePassingMode,
};
use crate::resolution::{DeclarationId, FieldId, ResolvedProgram, VariantId};
use crate::source::{SourceManager, Span};
use crate::types::{FunctionInstance, PrimitiveType, TypeContext, TypeId, TypeKind, TypedProgram};
use names::*;

#[derive(Debug, Clone)]
pub struct COptions {
    pub target: Target,
    /// `Some` emits the C runtime entry shim; `None` emits a library unit.
    pub entry: Option<DeclarationId>,
    /// `Some` emits a test-selection entry shim, including for an empty list.
    pub test_entries: Option<Vec<(DeclarationId, String)>>,
}

impl Default for COptions {
    fn default() -> Self {
        Self {
            target: Target::host(),
            entry: None,
            test_entries: None,
        }
    }
}

pub struct COutput {
    pub source: String,
    pub diagnostics: Vec<Diagnostic>,
    /// Native libraries the generated unit requires, contributed by the
    /// managed-memory strategy. Empty when the program needs no collector, so
    /// the backend rather than the driver decides when the runtime is linked.
    pub native_libraries: Vec<String>,
}

/// Deterministically mangles an Elamite declaration. The package-instance
/// hash and declaration identity prevent collisions across dependency units;
/// concrete generic arguments are appended by [`mangle_function_instance`].
#[must_use]
pub fn mangle_declaration(resolved: &ResolvedProgram, declaration: DeclarationId) -> String {
    let data = &resolved.declarations[declaration.index()];
    let package = resolved.modules[data.module.index()]
        .package
        .as_ref()
        .map_or_else(
            || "std".to_string(),
            |package| package.display().to_string(),
        );
    let hash = fnv1a64(package.as_bytes());
    let name = resolved
        .symbol_text(data.name)
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("el_p{hash:016x}_d{}_{}", declaration.index(), name)
}

#[must_use]
pub fn mangle_function_instance(resolved: &ResolvedProgram, instance: &FunctionInstance) -> String {
    let mut symbol = mangle_declaration(resolved, instance.declaration);
    for argument in &instance.arguments {
        let _ = write!(symbol, "_t{}", argument.index());
    }
    symbol
}

#[must_use]
pub fn emit_c(
    program: &ControlFlowProgram,
    resolved: &ResolvedProgram,
    typed: &TypedProgram,
    sources: &SourceManager,
    options: &COptions,
) -> COutput {
    CEmitter::new(program, resolved, typed, sources, options).run()
}

struct CEmitter<'a> {
    program: &'a ControlFlowProgram,
    resolved: &'a ResolvedProgram,
    typed: &'a TypedProgram,
    sources: &'a SourceManager,
    options: &'a COptions,
    output: String,
    diagnostics: Vec<Diagnostic>,
    emitted_types: BTreeSet<TypeId>,
    emitting_types: BTreeSet<TypeId>,
    structs: BTreeMap<TypeId, &'a crate::ir::TypedStruct>,
    enums: BTreeMap<TypeId, &'a TypedEnum>,
    emitted_equality_helpers: BTreeSet<TypeId>,
    emitting_equality_helpers: BTreeSet<TypeId>,
    emitted_ordering_helpers: BTreeSet<TypeId>,
    emitting_ordering_helpers: BTreeSet<TypeId>,
    emitted_hash_helpers: BTreeSet<TypeId>,
    emitting_hash_helpers: BTreeSet<TypeId>,
    emitted_default_helpers: BTreeSet<TypeId>,
    emitting_default_helpers: BTreeSet<TypeId>,
    strategy: &'static dyn ManagedMemoryStrategy,
    /// Promoted locals of the function currently being emitted. A promoted
    /// local lives in a managed cell, so every place naming it dereferences
    /// that cell.
    promoted: BTreeSet<crate::resolution::LocalBindingId>,
    /// Parameters using the compiler-only read-only borrowing ABI in the
    /// function currently being emitted.
    borrowed_parameters: BTreeSet<crate::resolution::LocalBindingId>,
    next_drop_loop: u32,
    emitted_clone_helpers: BTreeSet<TypeId>,
    emitted_drop_helpers: BTreeSet<TypeId>,
}

impl<'a> CEmitter<'a> {
    fn new(
        program: &'a ControlFlowProgram,
        resolved: &'a ResolvedProgram,
        typed: &'a TypedProgram,
        sources: &'a SourceManager,
        options: &'a COptions,
    ) -> Self {
        Self {
            program,
            resolved,
            typed,
            sources,
            options,
            output: String::new(),
            diagnostics: Vec::new(),
            emitted_types: BTreeSet::new(),
            emitting_types: BTreeSet::new(),
            structs: program
                .structs
                .iter()
                .map(|structure| (structure.ty, structure))
                .collect(),
            enums: program
                .enums
                .iter()
                .map(|enumeration| (enumeration.ty, enumeration))
                .collect(),
            emitted_equality_helpers: BTreeSet::new(),
            emitting_equality_helpers: BTreeSet::new(),
            emitted_ordering_helpers: BTreeSet::new(),
            emitting_ordering_helpers: BTreeSet::new(),
            emitted_hash_helpers: BTreeSet::new(),
            emitting_hash_helpers: BTreeSet::new(),
            emitted_default_helpers: BTreeSet::new(),
            emitting_default_helpers: BTreeSet::new(),
            strategy: default_managed_memory_strategy(),
            promoted: BTreeSet::new(),
            borrowed_parameters: BTreeSet::new(),
            next_drop_loop: 0,
            emitted_clone_helpers: BTreeSet::new(),
            emitted_drop_helpers: BTreeSet::new(),
        }
    }

    fn run(mut self) -> COutput {
        self.emit_prelude();
        // The collector's feature macros must be visible before any foreign
        // header can include `gc.h`; otherwise that header's include guard
        // suppresses the thread-registration declarations needed below.
        self.emit_managed_memory_prelude();
        self.emit_foreign_headers();
        self.emit_forward_structs();
        self.emit_object_types();
        let used_types = self.used_types();
        self.emit_foreign_root_runtime(&used_types);
        for ty in &used_types {
            self.emit_type_definition(*ty, None);
        }
        // Only types the program actually compares get a helper; emitting one
        // per aggregate would leave unused static functions behind.
        for ty in self.compared_types() {
            if self.needs_equality_helper(ty) {
                self.emit_equality_helper(ty, None);
            }
        }
        for ty in self.ordered_types() {
            if self.needs_ordering_helper(ty) {
                self.emit_ordering_helper(ty, None);
            }
        }
        for ty in self.stable_key_types() {
            if self.needs_hash_helper(ty) {
                self.emit_hash_helper(ty, None);
            }
        }
        self.emit_prototypes();
        self.emit_standard_runtime_helpers();
        for ty in self.defaulted_types() {
            if self.needs_default_helper(ty) {
                self.emit_default_helper(ty, None);
            }
        }
        for (outcome, source, result) in self.checked_conversions() {
            self.emit_numeric_conversion_helper(outcome, source, result);
        }
        self.emit_numeric_alternative_instances();
        for (operation, operand, result) in self.numeric_alternatives() {
            self.emit_numeric_alternative_helper(operation, operand, result);
        }
        for pointee in self.checked_pointees() {
            self.emit_pointer_check_helper(pointee);
        }
        self.emit_vtable_tables();
        for function in &self.program.functions {
            self.emit_function(function);
        }
        self.emit_entry();
        let mut native_libraries = if self.program.requires_managed_memory {
            self.strategy
                .native_libraries()
                .iter()
                .map(|library| (*library).to_string())
                .collect()
        } else {
            Vec::new()
        };
        if self.uses_native_threads() {
            native_libraries.push("pthread".to_string());
        }
        COutput {
            source: self.output,
            diagnostics: self.diagnostics,
            native_libraries,
        }
    }

    fn emit_foreign_headers(&mut self) {
        let headers = self
            .resolved
            .declarations
            .iter()
            .filter_map(|declaration| declaration.foreign_binding.as_ref())
            .filter_map(|binding| binding.header.as_ref())
            .collect::<BTreeSet<_>>();
        for header in headers {
            let _ = writeln!(self.output, "#include <{header}>");
        }
        if self
            .resolved
            .declarations
            .iter()
            .any(|declaration| declaration.foreign_binding.is_some())
        {
            self.output.push('\n');
        }
    }

    fn function_symbol(&self, instance: &FunctionInstance) -> String {
        self.resolved.declarations[instance.declaration.index()]
            .foreign_binding
            .as_ref()
            .map_or_else(
                || mangle_function_instance(self.resolved, instance),
                |binding| binding.c_name.clone(),
            )
    }

    fn c_field_name(&self, field: crate::resolution::FieldId) -> String {
        let data = &self.resolved.fields[field.index()];
        if self.resolved.declarations[data.parent_declaration.index()].kind
            == crate::resolution::DeclarationKind::ForeignStruct
        {
            self.resolved.symbol_text(data.name).to_string()
        } else {
            field_name(field)
        }
    }
}

struct SourceLocation {
    path: String,
    line: u32,
    column: u32,
}

fn value_place(value: &Rvalue) -> Option<&ControlFlowPlace> {
    match value {
        Rvalue::Load(place) => Some(place),
        _ => None,
    }
}

/// The identities an `Option[T]` value is built from.
struct OptionParts {
    some_variant: VariantId,
    some_field: FieldId,
    none_variant: VariantId,
}

fn numeric_operator_stem(operator: NumericOperator) -> &'static str {
    match operator {
        NumericOperator::Add => "add",
        NumericOperator::Subtract => "sub",
        NumericOperator::Multiply => "mul",
        NumericOperator::Divide => "div",
        NumericOperator::Remainder => "rem",
        NumericOperator::Negate => "neg",
        NumericOperator::ShiftLeft => "shl",
        NumericOperator::ShiftRight => "shr",
    }
}

/// Whether an alternative can still trap. Only the wrapping division and
/// remainder can, and only on a zero divisor, which has no wrapped answer.
fn numeric_alternative_traps(operation: NumericAlternative) -> bool {
    operation.outcome == NumericOutcome::Wrapping
        && matches!(
            operation.operator,
            NumericOperator::Divide | NumericOperator::Remainder
        )
}

/// The identities a checked numeric conversion's `Result` value is built from.
struct CheckedConversionParts {
    target: TypeId,
    error: TypeId,
    ok_variant: VariantId,
    ok_field: FieldId,
    err_variant: VariantId,
    err_field: FieldId,
    out_of_range: VariantId,
    not_a_number: VariantId,
}

/// The blocks reachable from `function`'s entry, as a fixpoint over
/// terminators. Unreachable blocks are not emitted at all, so no label
/// survives without a `goto` naming it.
fn reachable_blocks(function: &ControlFlowFunction) -> BTreeSet<BlockId> {
    let mut reachable = BTreeSet::new();
    let mut pending = vec![function.entry];
    while let Some(block_id) = pending.pop() {
        if !reachable.insert(block_id) {
            continue;
        }
        let Some(block) = function
            .blocks
            .iter()
            .find(|candidate| candidate.id == block_id)
        else {
            continue;
        };
        match block.terminator {
            Terminator::Goto(target) => pending.push(target),
            Terminator::Branch {
                then_block,
                else_block,
                ..
            } => {
                pending.push(then_block);
                pending.push(else_block);
            }
            Terminator::Return(_)
            | Terminator::Trap { .. }
            | Terminator::NeverCall { .. }
            | Terminator::Unreachable => {}
        }
    }
    reachable
}

fn standard_collection_type(operation: StandardCall) -> Option<TypeId> {
    use StandardCall::{
        ArrayGet, ArrayLen, MapClear, MapContainsKey, MapGet, MapInsert, MapIsEmpty, MapLen,
        MapNew, MapRemove, SetClear, SetContains, SetInsert, SetIsEmpty, SetLen, SetNew, SetRemove,
        SliceLen, StringFrom, VecAppend, VecClear, VecGet, VecGetVar, VecInsert, VecIsEmpty,
        VecLen, VecNew, VecPop, VecRemove,
    };
    Some(match operation {
        StandardCall::Panic
        | StandardCall::Assert
        | StandardCall::Fail { .. }
        | StandardCall::Trap { .. }
        | StandardCall::ClockNow { .. }
        | StandardCall::Clone { .. }
        | StandardCall::IntegerMax { .. }
        | StandardCall::BoxNew { .. }
        | StandardCall::SharedNew { .. }
        | StandardCall::SharedGet { .. }
        | StandardCall::SharedDowngrade { .. }
        | StandardCall::WeakUpgrade { .. }
        | StandardCall::StoreNew { .. }
        | StandardCall::StoreLen { .. }
        | StandardCall::StoreIsEmpty { .. }
        | StandardCall::StoreInsert { .. }
        | StandardCall::StoreGet { .. }
        | StandardCall::StoreRemove { .. }
        | StandardCall::StoreCompact { .. }
        | StandardCall::StoreClear { .. }
        | StandardCall::Text { .. }
        | StandardCall::System { .. }
        | StringFrom
        | StandardCall::IdentityFrom { .. }
        | StandardCall::ForeignRootRetain { .. }
        | StandardCall::ForeignRootPointer { .. }
        | StandardCall::ForeignRootClose { .. }
        | StandardCall::ThreadSpawn { .. }
        | StandardCall::ThreadJoin { .. }
        | StandardCall::ThreadIsFinished { .. }
        | StandardCall::ChannelCreate { .. }
        | StandardCall::ChannelSend { .. }
        | StandardCall::ChannelReceive { .. }
        | StandardCall::ChannelClose { .. }
        | StandardCall::MutexNew { .. }
        | StandardCall::MutexRead { .. }
        | StandardCall::MutexReplace { .. }
        | StandardCall::MutexUpdate { .. }
        | StandardCall::AtomicNew { .. }
        | StandardCall::AtomicLoad { .. }
        | StandardCall::AtomicStore { .. }
        | StandardCall::AtomicExchange { .. }
        | StandardCall::AtomicCompareExchange { .. }
        | StandardCall::AtomicFetchAdd { .. }
        | StandardCall::FormatterWrite { .. } => {
            return None;
        }
        SliceLen { .. } => return None,
        ArrayLen { collection }
        | ArrayGet { collection }
        | VecNew { collection }
        | VecLen { collection }
        | VecIsEmpty { collection }
        | VecGet { collection }
        | VecGetVar { collection }
        | VecAppend { collection }
        | VecInsert { collection }
        | VecRemove { collection }
        | VecPop { collection }
        | VecClear { collection }
        | MapNew { collection }
        | MapLen { collection }
        | MapIsEmpty { collection }
        | MapContainsKey { collection }
        | MapGet { collection }
        | StandardCall::MapGetVar { collection }
        | MapInsert { collection }
        | MapRemove { collection }
        | MapClear { collection }
        | SetNew { collection }
        | SetLen { collection }
        | SetIsEmpty { collection }
        | SetContains { collection }
        | SetInsert { collection }
        | SetRemove { collection }
        | SetClear { collection } => collection,
    })
}

fn strip_numeric_suffix(value: &str) -> &str {
    const SUFFIXES: [&str; 14] = [
        "isize", "usize", "i128", "u128", "i64", "u64", "i32", "u32", "i16", "u16", "i8", "u8",
        "f32", "f64",
    ];
    SUFFIXES
        .iter()
        .find_map(|suffix| value.strip_suffix(suffix))
        .unwrap_or(value)
}

fn integer_helper_name(primitive: PrimitiveType) -> Option<&'static str> {
    Some(match primitive {
        PrimitiveType::I8 => "i8",
        PrimitiveType::I16 => "i16",
        PrimitiveType::I32 => "i32",
        PrimitiveType::I64 => "i64",
        PrimitiveType::Isize => "isize",
        PrimitiveType::U8 => "u8",
        PrimitiveType::U16 => "u16",
        PrimitiveType::U32 => "u32",
        PrimitiveType::U64 => "u64",
        PrimitiveType::Usize => "usize",
        _ => return None,
    })
}

fn checked_binary_name(operator: BinaryOperator) -> Option<&'static str> {
    Some(match operator {
        BinaryOperator::Add => "add",
        BinaryOperator::Subtract => "sub",
        BinaryOperator::Multiply => "mul",
        BinaryOperator::Divide => "div",
        BinaryOperator::Remainder => "rem",
        BinaryOperator::ShiftLeft => "shl",
        BinaryOperator::ShiftRight => "shr",
        _ => return None,
    })
}

fn c_binary_operator(operator: BinaryOperator) -> Option<&'static str> {
    Some(match operator {
        BinaryOperator::Add | BinaryOperator::PointerOffsetAdd => "+",
        BinaryOperator::Concatenate => return None,
        BinaryOperator::Subtract
        | BinaryOperator::PointerOffsetSubtract
        | BinaryOperator::PointerDistance => "-",
        BinaryOperator::Multiply => "*",
        BinaryOperator::Divide => "/",
        BinaryOperator::Remainder => "%",
        BinaryOperator::BitAnd => "&",
        BinaryOperator::BitOr => "|",
        BinaryOperator::BitXor => "^",
        BinaryOperator::ShiftLeft => "<<",
        BinaryOperator::ShiftRight => ">>",
        BinaryOperator::Equal => "==",
        BinaryOperator::NotEqual => "!=",
        BinaryOperator::Less => "<",
        BinaryOperator::LessEqual => "<=",
        BinaryOperator::Greater => ">",
        BinaryOperator::GreaterEqual => ">=",
        BinaryOperator::PointerLess
        | BinaryOperator::PointerLessEqual
        | BinaryOperator::PointerGreater
        | BinaryOperator::PointerGreaterEqual => return None,
        BinaryOperator::LogicalAnd => "&&",
        BinaryOperator::LogicalOr => "||",
    })
}

fn primitive_bounds(
    primitive: PrimitiveType,
    target: Target,
) -> Option<(&'static str, &'static str)> {
    Some(match primitive {
        PrimitiveType::I8 => ("INT8_MIN", "INT8_MAX"),
        PrimitiveType::I16 => ("INT16_MIN", "INT16_MAX"),
        PrimitiveType::I32 => ("INT32_MIN", "INT32_MAX"),
        PrimitiveType::I64 => ("INT64_MIN", "INT64_MAX"),
        PrimitiveType::Isize => match target {
            Target::X86 => ("INT32_MIN", "INT32_MAX"),
            Target::X86_64 => ("INT64_MIN", "INT64_MAX"),
        },
        PrimitiveType::U8 => ("0", "UINT8_MAX"),
        PrimitiveType::U16 => ("0", "UINT16_MAX"),
        PrimitiveType::U32 => ("0", "UINT32_MAX"),
        PrimitiveType::U64 => ("0", "UINT64_MAX"),
        PrimitiveType::Usize => match target {
            Target::X86 => ("0", "UINT32_MAX"),
            Target::X86_64 => ("0", "UINT64_MAX"),
        },
        _ => return None,
    })
}

fn zero_value(ty: TypeId, types: &TypeContext, value_model: ValueModel) -> String {
    let mut ty = ty;
    loop {
        match types.kind(ty) {
            TypeKind::Alias { target, .. } => ty = *target,
            TypeKind::Primitive(PrimitiveType::Unit)
            | TypeKind::Tuple(_)
            | TypeKind::Array { .. }
            | TypeKind::Slice { .. }
            | TypeKind::Nominal { .. }
            | TypeKind::Builtin { .. }
            | TypeKind::Foreign { .. } => return "{0}".to_string(),
            // A trait object is a struct, not a pointer, so it zeroes with an
            // initializer rather than `NULL`.
            TypeKind::Reference { target, .. }
                if matches!(types.kind(*target), TypeKind::TraitObject { .. }) =>
            {
                return "{0}".to_string();
            }
            TypeKind::Primitive(PrimitiveType::Str | PrimitiveType::String) => {
                return "{0}".to_string();
            }
            TypeKind::Reference { .. }
            | TypeKind::RawPointer { .. }
            | TypeKind::Function { .. }
            | TypeKind::TraitObject { .. } => return "NULL".to_string(),
            TypeKind::Closure { .. } => {
                return if value_model == ValueModel::Owned {
                    "{0}".to_string()
                } else {
                    "NULL".to_string()
                };
            }
            _ => return "0".to_string(),
        }
    }
}
