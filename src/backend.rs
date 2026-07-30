//! Deterministic C99 emission and native-toolchain invocation support.
//!
//! This is the first executable backend (`ROADMAP.md` Milestones 8-9). It consumes
//! explicit control-flow IR, uses an internal (unstable) calling convention,
//! emits one strictly sequenced C statement per IR instruction, and routes
//! every supported value copy through a generated per-type helper.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::check::{NumericAlternative, NumericOperator, NumericOutcome, StandardCall};
use crate::diagnostics::{Category, Diagnostic};
use crate::ir::{
    AggregateValue, BinaryOperator, BlockId, CollectionLiteralKind, ControlFlowFunction,
    ControlFlowPlace, ControlFlowProgram, IndexKind, Instruction, IterationKind,
    LogicalCopyStrategy, RuntimeFormattedPart, Rvalue, TemporaryId, Terminator, TypedEnum,
    UnaryOperator, logical_copy_strategy,
};
use crate::memory::{
    AllocationClass, ManagedMemoryOperation, ManagedMemoryStrategy, default_managed_memory_strategy,
};
use crate::resolution::{DeclarationId, FieldId, ResolvedProgram, VariantId};
use crate::source::{SourceManager, Span};
use crate::types::{FunctionInstance, PrimitiveType, TypeContext, TypeId, TypeKind, TypedProgram};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    X86,
    X86_64,
}

impl Target {
    #[must_use]
    pub fn pointer_bits(self) -> u8 {
        match self {
            Self::X86 => 32,
            Self::X86_64 => 64,
        }
    }

    #[must_use]
    pub fn compiler_flag(self) -> &'static str {
        match self {
            Self::X86 => "-m32",
            Self::X86_64 => "-m64",
        }
    }

    #[must_use]
    pub fn host() -> Self {
        if cfg!(target_pointer_width = "32") {
            Self::X86
        } else {
            Self::X86_64
        }
    }
}

#[derive(Debug, Clone)]
pub struct COptions {
    pub target: Target,
    /// `Some` emits the C runtime entry shim; `None` emits a library unit.
    pub entry: Option<DeclarationId>,
}

impl Default for COptions {
    fn default() -> Self {
        Self {
            target: Target::host(),
            entry: None,
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
    emitted_copy_helpers: BTreeSet<TypeId>,
    emitting_copy_helpers: BTreeSet<TypeId>,
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
            emitted_copy_helpers: BTreeSet::new(),
            emitting_copy_helpers: BTreeSet::new(),
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
        }
    }

    fn run(mut self) -> COutput {
        self.emit_prelude();
        self.emit_foreign_headers();
        self.emit_managed_memory_prelude();
        self.emit_forward_structs();
        self.emit_object_types();
        let used_types = self.used_types();
        self.emit_foreign_root_runtime(&used_types);
        for ty in &used_types {
            self.emit_type_definition(*ty, None);
        }
        for ty in &used_types {
            self.emit_copy_helper(*ty, None);
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
        self.emit_standard_runtime_helpers();
        self.emit_prototypes();
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
        let native_libraries = if self.program.requires_managed_memory {
            self.strategy
                .native_libraries()
                .iter()
                .map(|library| (*library).to_string())
                .collect()
        } else {
            Vec::new()
        };
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

    fn emit_foreign_root_runtime(&mut self, used_types: &BTreeSet<TypeId>) {
        let needed = used_types.iter().any(|ty| {
            matches!(
                self.typed.types.kind(self.resolve_alias(*ty)),
                TypeKind::Builtin { builtin, .. }
                    if matches!(
                        self.resolved.builtin_name(*builtin),
                        "ForeignRoot" | "ForeignRootMut"
                    )
            )
        });
        if !needed {
            return;
        }
        self.output.push_str(
            "typedef struct el_foreign_root_state {\n\
             \x20\x20\x20\x20void *target;\n\
             \x20\x20\x20\x20bool open;\n\
             } el_foreign_root_state;\n\n\
             static el_foreign_root_state *el_foreign_root_retain(void *target) {\n\
             \x20\x20\x20\x20el_foreign_root_state *state = \
             (el_foreign_root_state *)malloc(sizeof(*state));\n\
             \x20\x20\x20\x20if (state == NULL) el_out_of_memory();\n\
             \x20\x20\x20\x20state->target = target;\n\
             \x20\x20\x20\x20state->open = true;\n",
        );
        self.emit_managed_operation(ManagedMemoryOperation::RegisterRoot {
            start: "&state->target",
            byte_count: "sizeof(state->target)",
        });
        self.output.push_str(
            "    return state;\n\
             }\n\n\
             static void *el_foreign_root_pointer(el_foreign_root_state *state, \
             const char *path, uint32_t line, uint32_t column) {\n\
             \x20\x20\x20\x20if (state == NULL || !state->open) \
             el_trap(\"E-RUN-CLOSED\", path, line, column);\n\
             \x20\x20\x20\x20return state->target;\n\
             }\n\n\
             static el_unit el_foreign_root_close(el_foreign_root_state *state) {\n\
             \x20\x20\x20\x20if (state != NULL && state->open) {\n",
        );
        self.emit_managed_operation(ManagedMemoryOperation::UnregisterRoot {
            start: "&state->target",
            byte_count: "sizeof(state->target)",
        });
        self.output.push_str(
            "        state->target = NULL;\n\
             \x20\x20\x20\x20\x20\x20\x20\x20state->open = false;\n\
             \x20\x20\x20\x20}\n\
             \x20\x20\x20\x20return (el_unit){0};\n\
             }\n\n",
        );
    }

    /// Emits one method-pointer struct per trait, a `void *`-receiver thunk per
    /// slot, and one static table per implementing type.
    ///
    /// Thunks exist so no function pointer is ever cast between incompatible
    /// types: each thunk has exactly the slot's signature and casts only the
    /// receiver, which is a plain object-pointer conversion.
    fn emit_object_types(&mut self) {
        let vtables = self.program.vtables.clone();
        if vtables.is_empty() {
            return;
        }
        let mut traits: BTreeMap<DeclarationId, Vec<&crate::ir::Vtable>> = BTreeMap::new();
        for vtable in &vtables {
            traits
                .entry(vtable.trait_declaration)
                .or_default()
                .push(vtable);
        }
        for (trait_declaration, _tables) in traits {
            let slots = crate::traits::vtable_slots(self.resolved, trait_declaration);
            let vtable_type = vtable_type_name(trait_declaration);
            let object = object_name(trait_declaration);
            // The method-pointer struct and the fat-reference struct.
            let _ = writeln!(self.output, "typedef struct {vtable_type} {{");
            for (slot, (_, method)) in slots.iter().enumerate() {
                let Some(signature) = self.typed.function_signatures.get(method).cloned() else {
                    continue;
                };
                let Some(return_type) = self.c_function_return_type(signature.return_type, None)
                else {
                    continue;
                };
                let mut parameters = vec!["void *".to_string()];
                for parameter in &signature.parameters {
                    match self.c_type(parameter.ty, None) {
                        Some(ty) => parameters.push(ty),
                        None => return,
                    }
                }
                let _ = writeln!(
                    self.output,
                    "    {return_type} (*{})({});",
                    vtable_slot_name(slot),
                    parameters.join(", ")
                );
            }
            let _ = writeln!(self.output, "}} {vtable_type};\n");
            let _ = writeln!(
                self.output,
                "typedef struct {object} {{\n    void *data;\n    const {vtable_type} *vtable;\n}} {object};\n"
            );
        }
    }

    /// Emits the `void *`-receiver thunks and the static tables. Separate from
    /// the type emission above because a thunk calls a mangled function and so
    /// must follow the prototypes.
    fn emit_vtable_tables(&mut self) {
        let vtables = self.program.vtables.clone();
        if vtables.is_empty() {
            return;
        }
        let mut traits: BTreeMap<DeclarationId, Vec<&crate::ir::Vtable>> = BTreeMap::new();
        for vtable in &vtables {
            traits
                .entry(vtable.trait_declaration)
                .or_default()
                .push(vtable);
        }
        for (trait_declaration, tables) in traits {
            let vtable_type = vtable_type_name(trait_declaration);
            let _ = &tables;
            for table in tables {
                let Some(concrete_type) = self.c_type(table.concrete, None) else {
                    continue;
                };
                for (slot, instance) in table.methods.iter().enumerate() {
                    // Instance signatures were cached during lowering; the
                    // backend only reads them.
                    let Some(signature) = self
                        .typed
                        .function_instance_signatures
                        .get(instance)
                        .or_else(|| self.typed.function_signatures.get(&instance.declaration))
                        .cloned()
                    else {
                        continue;
                    };
                    let Some(return_type) =
                        self.c_function_return_type(signature.return_type, None)
                    else {
                        continue;
                    };
                    let thunk = thunk_name(trait_declaration, table.concrete, slot);
                    let mut parameters = vec!["void *el_self".to_string()];
                    let mut arguments = vec![format!("({concrete_type} *)el_self")];
                    for (index, parameter) in signature.parameters.iter().enumerate() {
                        let Some(ty) = self.c_type(parameter.ty, None) else {
                            return;
                        };
                        parameters.push(format!("{ty} a{index}"));
                        arguments.push(format!("a{index}"));
                    }
                    let symbol = self.function_symbol(instance);
                    let call = format!("{symbol}({})", arguments.join(", "));
                    if return_type == "void" {
                        let _ = writeln!(
                            self.output,
                            "static void {thunk}({}) {{\n    {call};\n}}\n",
                            parameters.join(", ")
                        );
                    } else {
                        let _ = writeln!(
                            self.output,
                            "static {return_type} {thunk}({}) {{\n    return {call};\n}}\n",
                            parameters.join(", ")
                        );
                    }
                }
                let entries = (0..table.methods.len())
                    .map(|slot| thunk_name(trait_declaration, table.concrete, slot))
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(
                    self.output,
                    "static const {vtable_type} {} = {{ {entries} }};\n",
                    vtable_instance_name(self.typed, trait_declaration, table.concrete)
                );
            }
        }
    }

    /// Emits the collector's declarations behind the `ManagedMemoryStrategy`
    /// boundary. A program with no managed storage emits nothing, keeping the
    /// translation unit free of any collector dependency.
    fn emit_managed_memory_prelude(&mut self) {
        if !self.program.requires_managed_memory {
            self.output.push_str(
                "void *el_runtime_alloc(size_t byte_count) {\n\
                 \x20\x20\x20\x20void *result = malloc(byte_count);\n\
                 \x20\x20\x20\x20if (result == NULL) exit(101);\n\
                 \x20\x20\x20\x20return result;\n\
                 }\n\n",
            );
            return;
        }
        let strategy = self.strategy;
        if strategy.emit_c_prelude(&mut self.output).is_err() {
            self.diagnostics.push(Diagnostic::new(
                Category::CodeGeneration,
                format!(
                    "failed to emit the `{}` managed-memory prelude",
                    strategy.name()
                ),
            ));
            return;
        }
        self.output.push_str("\nvoid el_out_of_memory(void);\n");
        for (name, class) in [
            ("el_runtime_alloc", AllocationClass::Scanned),
            ("el_runtime_alloc_atomic", AllocationClass::PointerFree),
        ] {
            let _ = writeln!(
                self.output,
                "\nvoid *{name}(size_t byte_count) {{\n    void *result;"
            );
            if strategy
                .emit_c_operation(
                    ManagedMemoryOperation::Allocate {
                        destination: "result",
                        byte_count: "byte_count",
                        class,
                    },
                    &mut self.output,
                )
                .is_err()
            {
                self.diagnostics.push(Diagnostic::new(
                    Category::CodeGeneration,
                    format!(
                        "failed to emit the `{}` allocation operation",
                        strategy.name()
                    ),
                ));
                return;
            }
            self.output.push_str("    if (result == NULL) {\n        ");
            if strategy
                .emit_c_operation(ManagedMemoryOperation::Collect, &mut self.output)
                .is_err()
            {
                self.diagnostics.push(Diagnostic::new(
                    Category::CodeGeneration,
                    format!(
                        "failed to emit the `{}` collection operation",
                        strategy.name()
                    ),
                ));
                return;
            }
            self.output.push_str("        ");
            if strategy
                .emit_c_operation(
                    ManagedMemoryOperation::Allocate {
                        destination: "result",
                        byte_count: "byte_count",
                        class,
                    },
                    &mut self.output,
                )
                .is_err()
            {
                self.diagnostics.push(Diagnostic::new(
                    Category::CodeGeneration,
                    format!("failed to emit the `{}` allocation retry", strategy.name()),
                ));
                return;
            }
            self.output.push_str(
                "        if (result == NULL) el_out_of_memory();\n\
                 \x20   }\n\
                 \x20   return result;\n\
                 }\n",
            );
        }
        // Every allocation wrapper has already attempted a full collection
        // and retried before reaching this terminal path (`SPEC.md` 9).
        self.output.push_str("\nvoid el_out_of_memory(void) {\n");
        self.output.push_str(
            "\x20\x20\x20\x20fputs(\"elamite: out of memory\\n\", stderr);\n\
             \x20\x20\x20\x20fflush(stderr);\n\
             \x20\x20\x20\x20exit(101);\n\
             }\n\n",
        );
    }

    /// Emits one managed-memory operation behind the strategy boundary,
    /// indented as a statement inside the current function body.
    fn emit_managed_operation(&mut self, operation: ManagedMemoryOperation<'_>) {
        if let ManagedMemoryOperation::Allocate {
            destination,
            byte_count,
            class,
        } = operation
        {
            let allocator = match class {
                AllocationClass::Scanned => "el_runtime_alloc",
                AllocationClass::PointerFree => "el_runtime_alloc_atomic",
            };
            let _ = writeln!(self.output, "{destination} = {allocator}({byte_count});");
            return;
        }
        let strategy = self.strategy;
        self.output.push_str("    ");
        if strategy
            .emit_c_operation(operation, &mut self.output)
            .is_err()
        {
            self.diagnostics.push(Diagnostic::new(
                Category::CodeGeneration,
                format!(
                    "failed to emit a `{}` managed-memory operation",
                    strategy.name()
                ),
            ));
        }
    }

    fn emit_prelude(&mut self) {
        self.output.push_str(
            "/* Generated by Elamite. C99; internal ABI is intentionally unstable. */\n\
             #include <stdbool.h>\n\
             #include <stddef.h>\n\
             #include <stdint.h>\n\
             #include <inttypes.h>\n\
             #include <limits.h>\n\
             #include <float.h>\n\
             #include <math.h>\n\
             #include <stdio.h>\n\
             #include <stdlib.h>\n\n\
             #include <string.h>\n\n\
             typedef struct { uint8_t _value; } el_unit;\n\
             typedef struct { uint64_t lo; int64_t hi; } el_i128;\n\
             typedef struct { uint64_t lo; uint64_t hi; } el_u128;\n\n\
             typedef struct { const char *bytes; size_t length; } el_str;\n\
             typedef struct { char *bytes; size_t length; } el_string;\n\n\
             void el_out_of_memory(void);\n\
             void *el_runtime_alloc(size_t byte_count);\n\n\
             static void el_trap(const char *code, const char *path, uint32_t line, uint32_t column) {\n\
             \x20\x20\x20\x20fprintf(stderr, \"elamite trap [%s] at %s:%\" PRIu32 \":%\" PRIu32 \"\\n\", code, path, line, column);\n\
             \x20\x20\x20\x20fflush(stderr);\n\
             \x20\x20\x20\x20exit(101);\n\
             }\n\n\
             el_string el_copy_string(el_string value) {\n\
             \x20\x20\x20\x20el_string copy = {NULL, value.length};\n\
             \x20\x20\x20\x20copy.bytes = (char *)el_runtime_alloc(value.length + 1U);\n\
             \x20\x20\x20\x20if (value.length != 0U) memcpy(copy.bytes, value.bytes, value.length);\n\
             \x20\x20\x20\x20copy.bytes[value.length] = '\\0';\n\
             \x20\x20\x20\x20return copy;\n\
             }\n\
             el_string el_string_from(el_str value) {\n\
             \x20\x20\x20\x20el_string owned = {NULL, value.length};\n\
             \x20\x20\x20\x20owned.bytes = (char *)el_runtime_alloc(value.length + 1U);\n\
             \x20\x20\x20\x20if (value.length != 0U) memcpy(owned.bytes, value.bytes, value.length);\n\
             \x20\x20\x20\x20owned.bytes[value.length] = '\\0';\n\
             \x20\x20\x20\x20return owned;\n\
             }\n\n\
             bool el_text_equal(const char *a, size_t a_length, const char *b, size_t b_length) {\n\
             \x20\x20\x20\x20return a_length == b_length && (a_length == 0U || memcmp(a, b, a_length) == 0);\n\
             }\n\
             int el_text_order(const char *a, size_t a_length, const char *b, size_t b_length) {\n\
             \x20\x20\x20\x20size_t length = a_length < b_length ? a_length : b_length;\n\
             \x20\x20\x20\x20int order = length == 0U ? 0 : memcmp(a, b, length);\n\
             \x20\x20\x20\x20if (order < 0) return -1;\n\
             \x20\x20\x20\x20if (order > 0) return 1;\n\
             \x20\x20\x20\x20return a_length < b_length ? -1 : (a_length > b_length ? 1 : 0);\n\
             }\n\n\
             uint64_t el_hash_bytes(const char *bytes, size_t length) {\n\
             \x20\x20\x20\x20uint64_t hash = UINT64_C(14695981039346656037);\n\
             \x20\x20\x20\x20size_t index;\n\
             \x20\x20\x20\x20for (index = 0U; index < length; ++index) { hash ^= (uint8_t)bytes[index]; hash *= UINT64_C(1099511628211); }\n\
             \x20\x20\x20\x20return hash;\n\
             }\n\
             uint64_t el_hash_u64(uint64_t value) {\n\
             \x20\x20\x20\x20value ^= value >> 30; value *= UINT64_C(0xbf58476d1ce4e5b9);\n\
             \x20\x20\x20\x20value ^= value >> 27; value *= UINT64_C(0x94d049bb133111eb);\n\
             \x20\x20\x20\x20return value ^ (value >> 31);\n\
             }\n\
             uint64_t el_hash_combine(uint64_t state, uint64_t value) {\n\
             \x20\x20\x20\x20return (state ^ (value + UINT64_C(0x9e3779b97f4a7c15) + (state << 6) + (state >> 2)));\n\
             }\n\n\
             typedef struct el_formatter {\n\
             \x20\x20\x20\x20char *bytes;\n\
             \x20\x20\x20\x20size_t length;\n\
             \x20\x20\x20\x20size_t capacity;\n\
             } el_formatter;\n\
             void el_fmt_reserve(el_formatter *formatter, size_t extra) {\n\
             \x20\x20\x20\x20size_t needed = formatter->length + extra + 1U;\n\
             \x20\x20\x20\x20size_t capacity;\n\
             \x20\x20\x20\x20char *replacement;\n\
             \x20\x20\x20\x20if (needed <= formatter->capacity) return;\n\
             \x20\x20\x20\x20capacity = formatter->capacity == 0U ? 32U : formatter->capacity;\n\
             \x20\x20\x20\x20while (capacity < needed) capacity *= 2U;\n\
             \x20\x20\x20\x20replacement = (char *)el_runtime_alloc(capacity);\n\
             \x20\x20\x20\x20if (formatter->length != 0U) memcpy(replacement, formatter->bytes, formatter->length);\n\
             \x20\x20\x20\x20formatter->bytes = replacement;\n\
             \x20\x20\x20\x20formatter->capacity = capacity;\n\
             }\n\
             void el_fmt_append_n(el_formatter *formatter, const char *text, size_t length) {\n\
             \x20\x20\x20\x20el_fmt_reserve(formatter, length);\n\
             \x20\x20\x20\x20if (length != 0U) memcpy(formatter->bytes + formatter->length, text, length);\n\
             \x20\x20\x20\x20formatter->length += length;\n\
             \x20\x20\x20\x20formatter->bytes[formatter->length] = '\\0';\n\
             }\n\
             void el_fmt_append(el_formatter *formatter, const char *text) {\n\
             \x20\x20\x20\x20el_fmt_append_n(formatter, text, strlen(text));\n\
             }\n\
             void el_fmt_signed(el_formatter *formatter, intmax_t value) {\n\
             \x20\x20\x20\x20char buffer[64];\n\
             \x20\x20\x20\x20int length = snprintf(buffer, sizeof(buffer), \"%\" PRIdMAX, value);\n\
             \x20\x20\x20\x20if (length > 0) el_fmt_append_n(formatter, buffer, (size_t)length);\n\
             }\n\
             void el_fmt_unsigned(el_formatter *formatter, uintmax_t value) {\n\
             \x20\x20\x20\x20char buffer[64];\n\
             \x20\x20\x20\x20int length = snprintf(buffer, sizeof(buffer), \"%\" PRIuMAX, value);\n\
             \x20\x20\x20\x20if (length > 0) el_fmt_append_n(formatter, buffer, (size_t)length);\n\
             }\n\
             void el_fmt_float(el_formatter *formatter, double value) {\n\
             \x20\x20\x20\x20char buffer[64];\n\
             \x20\x20\x20\x20int length = snprintf(buffer, sizeof(buffer), \"%.17g\", value);\n\
             \x20\x20\x20\x20if (length > 0) el_fmt_append_n(formatter, buffer, (size_t)length);\n\
             }\n\
             void el_fmt_char(el_formatter *formatter, uint32_t value) {\n\
             \x20\x20\x20\x20char bytes[4]; size_t length;\n\
             \x20\x20\x20\x20if (value <= 0x7fU) { bytes[0] = (char)value; length = 1U; }\n\
             \x20\x20\x20\x20else if (value <= 0x7ffU) { bytes[0] = (char)(0xc0U | (value >> 6)); bytes[1] = (char)(0x80U | (value & 0x3fU)); length = 2U; }\n\
             \x20\x20\x20\x20else if (value <= 0xffffU) { bytes[0] = (char)(0xe0U | (value >> 12)); bytes[1] = (char)(0x80U | ((value >> 6) & 0x3fU)); bytes[2] = (char)(0x80U | (value & 0x3fU)); length = 3U; }\n\
             \x20\x20\x20\x20else { bytes[0] = (char)(0xf0U | (value >> 18)); bytes[1] = (char)(0x80U | ((value >> 12) & 0x3fU)); bytes[2] = (char)(0x80U | ((value >> 6) & 0x3fU)); bytes[3] = (char)(0x80U | (value & 0x3fU)); length = 4U; }\n\
             \x20\x20\x20\x20el_fmt_append_n(formatter, bytes, length);\n\
             }\n\
             el_str el_fmt_finish(el_formatter *formatter) {\n\
             \x20\x20\x20\x20if (formatter->bytes == NULL) { formatter->bytes = (char *)el_runtime_alloc(1U); formatter->bytes[0] = '\\0'; }\n\
             \x20\x20\x20\x20return (el_str){formatter->bytes, formatter->length};\n\
             }\n\n\
             void el_print_char(uint32_t value) {\n\
             \x20\x20\x20\x20if (value <= 0x7fU) { fputc((int)value, stdout); return; }\n\
             \x20\x20\x20\x20if (value <= 0x7ffU) {\n\
             \x20\x20\x20\x20\x20\x20\x20\x20fputc((int)(0xc0U | (value >> 6)), stdout);\n\
             \x20\x20\x20\x20\x20\x20\x20\x20fputc((int)(0x80U | (value & 0x3fU)), stdout); return;\n\
             \x20\x20\x20\x20}\n\
             \x20\x20\x20\x20if (value <= 0xffffU) {\n\
             \x20\x20\x20\x20\x20\x20\x20\x20fputc((int)(0xe0U | (value >> 12)), stdout);\n\
             \x20\x20\x20\x20\x20\x20\x20\x20fputc((int)(0x80U | ((value >> 6) & 0x3fU)), stdout);\n\
             \x20\x20\x20\x20\x20\x20\x20\x20fputc((int)(0x80U | (value & 0x3fU)), stdout); return;\n\
             \x20\x20\x20\x20}\n\
             \x20\x20\x20\x20fputc((int)(0xf0U | (value >> 18)), stdout);\n\
             \x20\x20\x20\x20fputc((int)(0x80U | ((value >> 12) & 0x3fU)), stdout);\n\
             \x20\x20\x20\x20fputc((int)(0x80U | ((value >> 6) & 0x3fU)), stdout);\n\
             \x20\x20\x20\x20fputc((int)(0x80U | (value & 0x3fU)), stdout);\n\
             }\n\
             #define el_cast_integer(VALUE, MINIMUM, MAXIMUM, PATH, LINE, COLUMN, CONVERTED) \\\n\
             \x20 (((VALUE) < (MINIMUM) || (VALUE) > (MAXIMUM) || !isfinite((double)(VALUE))) \\\n\
             \x20 ? (el_trap(\"E-RUN-CAST\", (PATH), (LINE), (COLUMN)), (CONVERTED)) : (CONVERTED))\n\n",
        );
        self.emit_checked_integer_helpers();
    }

    fn emit_checked_integer_helpers(&mut self) {
        self.output.push_str(
            "#define EL_SIGNED_ARITH(NAME, TYPE, MINIMUM, MAXIMUM, BITS) \\\n\
             TYPE el_add_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { \\\n\
             \x20 if ((b > 0 && a > (TYPE)(MAXIMUM - b)) || (b < 0 && a < (TYPE)(MINIMUM - b))) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } return (TYPE)(a + b); } \\\n\
             TYPE el_sub_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { \\\n\
             \x20 if ((b < 0 && a > (TYPE)(MAXIMUM + b)) || (b > 0 && a < (TYPE)(MINIMUM + b))) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } return (TYPE)(a - b); } \\\n\
             TYPE el_mul_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { \\\n\
             \x20 if (a == 0 || b == 0) { return 0; } \\\n\
             \x20 if ((a == -1 && b == MINIMUM) || (b == -1 && a == MINIMUM)) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } \\\n\
             \x20 if (a > 0 ? (b > 0 ? a > MAXIMUM / b : b < MINIMUM / a) : (b > 0 ? a < MINIMUM / b : b < MAXIMUM / a)) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } \\\n\
             \x20 return (TYPE)(a * b); } \\\n\
             TYPE el_neg_##NAME(TYPE a, const char *p, uint32_t l, uint32_t c) { if (a == MINIMUM) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } return (TYPE)-a; } \\\n\
             TYPE el_div_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { if (b == 0) { el_trap(\"E-RUN-DIVZERO\", p, l, c); } if (a == MINIMUM && b == -1) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } return (TYPE)(a / b); } \\\n\
             TYPE el_rem_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { if (b == 0) { el_trap(\"E-RUN-DIVZERO\", p, l, c); } if (a == MINIMUM && b == -1) { return 0; } return (TYPE)(a % b); } \\\n\
             TYPE el_shl_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { uint32_t n; if (b < 0 || (uintmax_t)b >= (BITS)) { el_trap(\"E-RUN-SHIFT\", p, l, c); } n = (uint32_t)b; while (n-- != 0U) { if (a > MAXIMUM / 2 || a < MINIMUM / 2) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } a = (TYPE)(a * 2); } return a; } \\\n\
             TYPE el_shr_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { uint32_t n; if (b < 0 || (uintmax_t)b >= (BITS)) el_trap(\"E-RUN-SHIFT\", p, l, c); n = (uint32_t)b; while (n-- != 0U) a = a >= 0 ? (TYPE)(a / 2) : (TYPE)(-1 - ((-1 - a) / 2)); return a; }\n\
             #define EL_UNSIGNED_ARITH(NAME, TYPE, MAXIMUM, BITS) \\\n\
             TYPE el_add_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { if (a > (TYPE)(MAXIMUM - b)) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } return (TYPE)(a + b); } \\\n\
             TYPE el_sub_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { if (a < b) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } return (TYPE)(a - b); } \\\n\
             TYPE el_mul_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { if (b != 0 && a > MAXIMUM / b) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } return (TYPE)(a * b); } \\\n\
             TYPE el_neg_##NAME(TYPE a, const char *p, uint32_t l, uint32_t c) { if (a != 0) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } return 0; } \\\n\
             TYPE el_div_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { if (b == 0) { el_trap(\"E-RUN-DIVZERO\", p, l, c); } return (TYPE)(a / b); } \\\n\
             TYPE el_rem_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { if (b == 0) { el_trap(\"E-RUN-DIVZERO\", p, l, c); } return (TYPE)(a % b); } \\\n\
             TYPE el_shl_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { if ((uintmax_t)b >= (BITS)) { el_trap(\"E-RUN-SHIFT\", p, l, c); } if (a > (TYPE)(MAXIMUM >> b)) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } return (TYPE)(a << b); } \\\n\
             TYPE el_shr_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { if ((uintmax_t)b >= (BITS)) { el_trap(\"E-RUN-SHIFT\", p, l, c); } return (TYPE)(a >> b); }\n\
             EL_SIGNED_ARITH(i8, int8_t, INT8_MIN, INT8_MAX, 8)\n\
             EL_SIGNED_ARITH(i16, int16_t, INT16_MIN, INT16_MAX, 16)\n\
             EL_SIGNED_ARITH(i32, int32_t, INT32_MIN, INT32_MAX, 32)\n\
             EL_SIGNED_ARITH(i64, int64_t, INT64_MIN, INT64_MAX, 64)\n\
             EL_SIGNED_ARITH(isize, intptr_t, INTPTR_MIN, INTPTR_MAX, (sizeof(intptr_t) * CHAR_BIT))\n\
             EL_UNSIGNED_ARITH(u8, uint8_t, UINT8_MAX, 8)\n\
             EL_UNSIGNED_ARITH(u16, uint16_t, UINT16_MAX, 16)\n\
             EL_UNSIGNED_ARITH(u32, uint32_t, UINT32_MAX, 32)\n\
             EL_UNSIGNED_ARITH(u64, uint64_t, UINT64_MAX, 64)\n\
             EL_UNSIGNED_ARITH(usize, uintptr_t, UINTPTR_MAX, (sizeof(uintptr_t) * CHAR_BIT))\n\n",
        );
        self.emit_numeric_alternative_macros();
    }

    /// Defines the standard alternatives to the trapping arithmetic operators
    /// (`SPEC.md` 4.1) as two macros, instantiated per used integer type.
    ///
    /// Each operation is expressed as an overflow *predicate* plus a wrapping
    /// result, so `checked_X` is exactly `ovf_X ? None : Some(wrap_X)` and the
    /// predicate mirrors the trapping helper's own condition. The two can
    /// therefore never disagree about which operations overflow.
    ///
    /// Wrapping arithmetic is performed in the unsigned counterpart type and
    /// converted back, which is well defined in C99 (modular in, and
    /// implementation-defined out — two's complement on both supported
    /// targets) rather than the signed-overflow undefined behavior that the
    /// direct expression would have.
    ///
    /// Division and remainder have no wrapped answer for a zero divisor, so
    /// `wrapping_div`/`wrapping_rem` still trap there exactly as `/` and `%`
    /// do; their wrapping behavior covers only the signed-minimum overflow.
    fn emit_numeric_alternative_macros(&mut self) {
        self.output.push_str(
            "#define EL_SIGNED_ALT(NAME, TYPE, UTYPE, MINIMUM, MAXIMUM, BITS) \\\n\
             bool el_ovf_add_##NAME(TYPE a, TYPE b) { return (b > 0 && a > (TYPE)(MAXIMUM - b)) || (b < 0 && a < (TYPE)(MINIMUM - b)); } \\\n\
             TYPE el_wrap_add_##NAME(TYPE a, TYPE b) { return (TYPE)(UTYPE)((UTYPE)a + (UTYPE)b); } \\\n\
             TYPE el_sat_add_##NAME(TYPE a, TYPE b) { if (!el_ovf_add_##NAME(a, b)) return (TYPE)(a + b); return b > 0 ? (TYPE)(MAXIMUM) : (TYPE)(MINIMUM); } \\\n\
             bool el_ovf_sub_##NAME(TYPE a, TYPE b) { return (b < 0 && a > (TYPE)(MAXIMUM + b)) || (b > 0 && a < (TYPE)(MINIMUM + b)); } \\\n\
             TYPE el_wrap_sub_##NAME(TYPE a, TYPE b) { return (TYPE)(UTYPE)((UTYPE)a - (UTYPE)b); } \\\n\
             TYPE el_sat_sub_##NAME(TYPE a, TYPE b) { if (!el_ovf_sub_##NAME(a, b)) return (TYPE)(a - b); return b < 0 ? (TYPE)(MAXIMUM) : (TYPE)(MINIMUM); } \\\n\
             bool el_ovf_mul_##NAME(TYPE a, TYPE b) { if (a == 0 || b == 0) return false; if ((a == -1 && b == (TYPE)(MINIMUM)) || (b == -1 && a == (TYPE)(MINIMUM))) return true; return a > 0 ? (b > 0 ? a > (TYPE)(MAXIMUM) / b : b < (TYPE)(MINIMUM) / a) : (b > 0 ? a < (TYPE)(MINIMUM) / b : b < (TYPE)(MAXIMUM) / a); } \\\n\
             TYPE el_wrap_mul_##NAME(TYPE a, TYPE b) { return (TYPE)(UTYPE)((UTYPE)a * (UTYPE)b); } \\\n\
             TYPE el_sat_mul_##NAME(TYPE a, TYPE b) { if (!el_ovf_mul_##NAME(a, b)) return (TYPE)(a * b); return ((a > 0) == (b > 0)) ? (TYPE)(MAXIMUM) : (TYPE)(MINIMUM); } \\\n\
             bool el_ovf_div_##NAME(TYPE a, TYPE b) { return b == 0 || (a == (TYPE)(MINIMUM) && b == -1); } \\\n\
             TYPE el_wrap_div_##NAME(TYPE a, TYPE b) { if (b == 0) return 0; if (a == (TYPE)(MINIMUM) && b == -1) return (TYPE)(MINIMUM); return (TYPE)(a / b); } \\\n\
             bool el_ovf_rem_##NAME(TYPE a, TYPE b) { (void)a; return b == 0; } \\\n\
             TYPE el_wrap_rem_##NAME(TYPE a, TYPE b) { if (b == 0) return 0; if (a == (TYPE)(MINIMUM) && b == -1) return 0; return (TYPE)(a % b); } \\\n\
             bool el_ovf_neg_##NAME(TYPE a) { return a == (TYPE)(MINIMUM); } \\\n\
             TYPE el_wrap_neg_##NAME(TYPE a) { return (TYPE)(UTYPE)(0U - (UTYPE)a); } \\\n\
             bool el_ovf_shl_##NAME(TYPE a, TYPE b) { uint32_t n; if (b < 0 || (uintmax_t)b >= (BITS)) return true; n = (uint32_t)b; while (n-- != 0U) { if (a > (TYPE)(MAXIMUM) / 2 || a < (TYPE)(MINIMUM) / 2) return true; a = (TYPE)(a * 2); } return false; } \\\n\
             TYPE el_wrap_shl_##NAME(TYPE a, TYPE b) { uint32_t n = (uint32_t)((uintmax_t)b & (uintmax_t)((BITS) - 1U)); return (TYPE)(UTYPE)((UTYPE)a << n); } \\\n\
             bool el_ovf_shr_##NAME(TYPE a, TYPE b) { (void)a; return b < 0 || (uintmax_t)b >= (BITS); } \\\n\
             TYPE el_wrap_shr_##NAME(TYPE a, TYPE b) { uint32_t n = (uint32_t)((uintmax_t)b & (uintmax_t)((BITS) - 1U)); while (n-- != 0U) a = a >= 0 ? (TYPE)(a / 2) : (TYPE)(-1 - ((-1 - a) / 2)); return a; }\n\
             #define EL_UNSIGNED_ALT(NAME, TYPE, MAXIMUM, BITS) \\\n\
             bool el_ovf_add_##NAME(TYPE a, TYPE b) { return a > (TYPE)(MAXIMUM - b); } \\\n\
             TYPE el_wrap_add_##NAME(TYPE a, TYPE b) { return (TYPE)(a + b); } \\\n\
             TYPE el_sat_add_##NAME(TYPE a, TYPE b) { return el_ovf_add_##NAME(a, b) ? (TYPE)(MAXIMUM) : (TYPE)(a + b); } \\\n\
             bool el_ovf_sub_##NAME(TYPE a, TYPE b) { return a < b; } \\\n\
             TYPE el_wrap_sub_##NAME(TYPE a, TYPE b) { return (TYPE)(a - b); } \\\n\
             TYPE el_sat_sub_##NAME(TYPE a, TYPE b) { return a < b ? (TYPE)0 : (TYPE)(a - b); } \\\n\
             bool el_ovf_mul_##NAME(TYPE a, TYPE b) { return b != 0 && a > (TYPE)(MAXIMUM) / b; } \\\n\
             TYPE el_wrap_mul_##NAME(TYPE a, TYPE b) { return (TYPE)(a * b); } \\\n\
             TYPE el_sat_mul_##NAME(TYPE a, TYPE b) { return el_ovf_mul_##NAME(a, b) ? (TYPE)(MAXIMUM) : (TYPE)(a * b); } \\\n\
             bool el_ovf_div_##NAME(TYPE a, TYPE b) { (void)a; return b == 0; } \\\n\
             TYPE el_wrap_div_##NAME(TYPE a, TYPE b) { if (b == 0) return 0; return (TYPE)(a / b); } \\\n\
             bool el_ovf_rem_##NAME(TYPE a, TYPE b) { (void)a; return b == 0; } \\\n\
             TYPE el_wrap_rem_##NAME(TYPE a, TYPE b) { if (b == 0) return 0; return (TYPE)(a % b); } \\\n\
             bool el_ovf_neg_##NAME(TYPE a) { return a != 0; } \\\n\
             TYPE el_wrap_neg_##NAME(TYPE a) { return (TYPE)(0U - a); } \\\n\
             bool el_ovf_shl_##NAME(TYPE a, TYPE b) { if ((uintmax_t)b >= (BITS)) return true; return a > (TYPE)((TYPE)(MAXIMUM) >> b); } \\\n\
             TYPE el_wrap_shl_##NAME(TYPE a, TYPE b) { uint32_t n = (uint32_t)((uintmax_t)b & (uintmax_t)((BITS) - 1U)); return (TYPE)(a << n); } \\\n\
             bool el_ovf_shr_##NAME(TYPE a, TYPE b) { (void)a; return (uintmax_t)b >= (BITS); } \\\n\
             TYPE el_wrap_shr_##NAME(TYPE a, TYPE b) { uint32_t n = (uint32_t)((uintmax_t)b & (uintmax_t)((BITS) - 1U)); return (TYPE)(a >> n); }\n\n",
        );
    }

    fn emit_forward_structs(&mut self) {
        for structure in &self.program.structs {
            let name = struct_name(structure.declaration, structure.ty);
            let _ = writeln!(self.output, "typedef struct {name} {name};");
        }
        for enumeration in &self.program.enums {
            let name = enum_name(enumeration.declaration, enumeration.ty);
            let _ = writeln!(self.output, "typedef struct {name} {name};");
        }
        if !self.program.structs.is_empty() || !self.program.enums.is_empty() {
            self.output.push('\n');
        }
    }

    fn used_types(&self) -> BTreeSet<TypeId> {
        let mut types = BTreeSet::new();
        for structure in &self.program.structs {
            for (_, _, ty) in &structure.fields {
                types.insert(*ty);
            }
        }
        for enumeration in &self.program.enums {
            for variant in &enumeration.variants {
                for (_, _, ty) in &variant.fields {
                    types.insert(*ty);
                }
            }
        }
        for function in &self.program.functions {
            types.insert(function.return_type);
            types.extend(function.parameters.iter().map(|parameter| parameter.ty));
            types.extend(function.local_types.values().copied());
            types.extend(function.temporary_types.iter().copied());
        }
        types
    }

    fn emit_type_definition(&mut self, ty: TypeId, span: Option<Span>) {
        let ty = self.resolve_alias(ty);
        if self.emitted_types.contains(&ty) || !self.emitting_types.insert(ty) {
            return;
        }
        match self.typed.types.kind(ty) {
            TypeKind::Tuple(elements) => {
                for element in elements {
                    self.emit_type_definition(*element, span);
                }
                let name = tuple_name(ty);
                let _ = writeln!(self.output, "typedef struct {name} {{");
                if elements.is_empty() {
                    self.output.push_str("    uint8_t _value;\n");
                } else {
                    for (index, element) in elements.iter().enumerate() {
                        if let Some(c_type) = self.c_type(*element, span) {
                            let _ = writeln!(self.output, "    {c_type} v{index};");
                        }
                    }
                }
                let _ = writeln!(self.output, "}} {name};\n");
            }
            TypeKind::Array { element, length } => {
                self.emit_type_definition(*element, span);
                let name = array_name(ty);
                if let Some(c_type) = self.c_type(*element, span) {
                    let _ = writeln!(
                        self.output,
                        "typedef struct {name} {{ {c_type} values[{length}]; }} {name};\n"
                    );
                }
            }
            TypeKind::Slice(element) => {
                self.emit_type_definition(*element, span);
                let name = slice_name(ty);
                if let Some(c_type) = self.c_type(*element, span) {
                    let _ = writeln!(
                        self.output,
                        "typedef struct {name} {{ {c_type} *values; uintptr_t length; }} {name};\n"
                    );
                }
            }
            TypeKind::Builtin { builtin, arguments } => {
                let builtin_name = self.resolved.builtin_name(*builtin);
                for argument in arguments {
                    self.emit_type_definition(*argument, span);
                }
                let name = collection_type_name(ty);
                match (builtin_name, arguments.as_slice()) {
                    ("Vec" | "Set", [element]) => {
                        if let Some(element_type) = self.c_type(*element, span) {
                            let _ = writeln!(
                                self.output,
                                "typedef struct {name}_data {{\n    uintptr_t length;\n    \
                                 uintptr_t capacity;\n    {element_type} *values;\n}} *{name};\n"
                            );
                        }
                    }
                    ("Map", [key, value]) => {
                        if let (Some(key_type), Some(value_type)) =
                            (self.c_type(*key, span), self.c_type(*value, span))
                        {
                            let _ = writeln!(
                                self.output,
                                "typedef struct {name}_data {{\n    uintptr_t length;\n    \
                                 uintptr_t capacity;\n    {key_type} *keys;\n    \
                                 {value_type} *values;\n}} *{name};\n"
                            );
                        }
                    }
                    ("Formatter", []) => {}
                    ("Identity", [_]) => {
                        let _ = writeln!(
                            self.output,
                            "typedef struct {name} {{ void *target; }} {name};\n"
                        );
                    }
                    _ => {}
                }
            }
            TypeKind::Reference { target, .. }
                if matches!(self.typed.types.kind(*target), TypeKind::Function { .. }) =>
            {
                self.emit_type_definition(*target, span);
            }
            TypeKind::RawPointer { target, .. }
                if matches!(self.typed.types.kind(*target), TypeKind::Function { .. }) =>
            {
                self.emit_type_definition(*target, span);
            }
            // A trait object's struct is emitted with its vtable, not here.
            TypeKind::TraitObject { .. } => {}
            TypeKind::Reference { target, .. }
                if matches!(
                    self.typed.types.kind(self.resolve_alias(*target)),
                    TypeKind::TraitObject { .. }
                ) => {}
            TypeKind::Function {
                receiver,
                parameters,
                return_type,
                ..
            } => {
                self.emit_type_definition(*return_type, span);
                if let Some(receiver) = receiver {
                    self.emit_type_definition(*receiver, span);
                }
                for parameter in parameters {
                    self.emit_type_definition(parameter.ty, span);
                    if parameter.variadic
                        && let Some(slice) =
                            self.typed.types.id_for_kind(&TypeKind::Slice(parameter.ty))
                    {
                        self.emit_type_definition(slice, span);
                    }
                }
                let Some(result) = self.c_function_return_type(*return_type, span) else {
                    self.emitting_types.remove(&ty);
                    return;
                };
                let mut c_parameters = Vec::new();
                if let Some(receiver) = receiver
                    && let Some(receiver) = self.c_type(*receiver, span)
                {
                    c_parameters.push(receiver);
                }
                for parameter in parameters {
                    let parameter_type = if parameter.variadic {
                        self.typed
                            .types
                            .id_for_kind(&TypeKind::Slice(parameter.ty))
                            .and_then(|slice| self.c_type(slice, span))
                    } else {
                        self.c_type(parameter.ty, span)
                    };
                    if let Some(parameter_type) = parameter_type {
                        c_parameters.push(parameter_type);
                    }
                }
                let parameters = if c_parameters.is_empty() {
                    "void".to_string()
                } else {
                    c_parameters.join(", ")
                };
                let _ = writeln!(
                    self.output,
                    "typedef {result} (*{})({parameters});\n",
                    function_type_name(ty)
                );
            }
            TypeKind::Nominal { .. } => {
                if let Some(structure) = self.structs.get(&ty).copied() {
                    for (_, _, field_type) in &structure.fields {
                        self.emit_type_definition(*field_type, span);
                    }
                    let name = struct_name(structure.declaration, ty);
                    let _ = writeln!(self.output, "struct {name} {{");
                    if structure.fields.is_empty() {
                        self.output.push_str("    uint8_t _value;\n");
                    } else {
                        for (field, _, field_type) in &structure.fields {
                            if let Some(c_type) = self.c_type(*field_type, span) {
                                let _ =
                                    writeln!(self.output, "    {c_type} {};", field_name(*field));
                            }
                        }
                    }
                    self.output.push_str("};\n\n");
                } else if let Some(enumeration) = self.enums.get(&ty).copied() {
                    for variant in &enumeration.variants {
                        for (_, _, field_type) in &variant.fields {
                            self.emit_type_definition(*field_type, span);
                        }
                    }
                    let name = enum_name(enumeration.declaration, ty);
                    let _ = writeln!(self.output, "struct {name} {{");
                    self.output.push_str("    uint32_t tag;\n    union {\n");
                    let mut has_payload = false;
                    for variant in &enumeration.variants {
                        if variant.fields.is_empty() {
                            continue;
                        }
                        has_payload = true;
                        self.output.push_str("        struct {\n");
                        for (field, _, field_type) in &variant.fields {
                            if let Some(c_type) = self.c_type(*field_type, span) {
                                let _ = writeln!(
                                    self.output,
                                    "            {c_type} {};",
                                    field_name(*field)
                                );
                            }
                        }
                        let _ = writeln!(
                            self.output,
                            "        }} {};",
                            variant_member_name(variant.id)
                        );
                    }
                    if !has_payload {
                        self.output.push_str("        uint8_t _empty;\n");
                    }
                    self.output.push_str("    } payload;\n};\n\n");
                } else {
                    self.type_error(
                        ty,
                        span,
                        "this nominal type has no concrete C representation",
                    );
                }
            }
            _ => {}
        }
        self.emitting_types.remove(&ty);
        self.emitted_types.insert(ty);
    }

    /// Operand types of every structural comparison in the program.
    fn compared_types(&self) -> BTreeSet<TypeId> {
        let mut types = BTreeSet::new();
        for function in &self.program.functions {
            for block in &function.blocks {
                for instruction in &block.instructions {
                    if let Instruction::Assign {
                        value: Rvalue::CompareEqual { operand_type, .. },
                        ..
                    } = instruction
                    {
                        types.insert(*operand_type);
                    }
                }
            }
        }
        types
    }

    /// Operand types of every structural ordering operation in the program.
    fn ordered_types(&self) -> BTreeSet<TypeId> {
        let mut types = BTreeSet::new();
        for function in &self.program.functions {
            for block in &function.blocks {
                for instruction in &block.instructions {
                    if let Instruction::Assign {
                        value: Rvalue::CompareOrder { operand_type, .. },
                        ..
                    } = instruction
                    {
                        types.insert(*operand_type);
                    }
                }
            }
        }
        types
    }

    fn defaulted_types(&self) -> BTreeSet<TypeId> {
        let mut types = BTreeSet::new();
        for function in &self.program.functions {
            for block in &function.blocks {
                for instruction in &block.instructions {
                    if let Instruction::Assign {
                        value: Rvalue::DefaultValue(ty),
                        ..
                    } = instruction
                    {
                        types.insert(*ty);
                    }
                }
            }
        }
        types
    }

    /// Every `(source type, result type)` pair a `Target.try_from(value)` call
    /// needs a helper for. The result type carries the target, so the pair is
    /// the helper's complete identity.
    fn checked_conversions(&self) -> BTreeSet<(NumericOutcome, TypeId, TypeId)> {
        let mut pairs = BTreeSet::new();
        for function in &self.program.functions {
            for block in &function.blocks {
                for instruction in &block.instructions {
                    if let Instruction::Assign {
                        destination,
                        value:
                            Rvalue::NumericConversion {
                                outcome,
                                source_type,
                                ..
                            },
                        ..
                    } = instruction
                    {
                        pairs.insert((
                            *outcome,
                            *source_type,
                            function.temporary_types[destination.index()],
                        ));
                    }
                }
            }
        }
        pairs
    }

    /// Emits one standard numeric conversion helper (`SPEC.md` 4.1).
    ///
    /// The checked form's range test uses the same boundaries as the trapping
    /// `as` conversion, so `value as Target` and `Target.try_from(value)`
    /// never disagree about which values are representable — the only
    /// difference is that one traps and the other reports. A non-finite source
    /// is separated out: NaN is `NotANumber`, an infinity is `OutOfRange`.
    ///
    /// The wrapping form converts modulo the target's range, and the
    /// saturating form clamps to its nearest bound. Both are integer-only, so
    /// neither has a non-finite case.
    fn emit_numeric_conversion_helper(
        &mut self,
        outcome: NumericOutcome,
        source: TypeId,
        result: TypeId,
    ) {
        let (Some(source_c_type), Some(result_c_type)) =
            (self.c_type(source, None), self.c_type(result, None))
        else {
            return;
        };
        let Some(source_primitive) = self.typed.types.expanded_primitive(source) else {
            return;
        };
        let name = numeric_conversion_name(outcome, source, result);

        if outcome != NumericOutcome::Checked {
            let Some(target_primitive) = self.typed.types.expanded_primitive(result) else {
                return;
            };
            let Some((minimum, maximum)) = primitive_bounds(target_primitive, self.options.target)
            else {
                self.type_error(
                    result,
                    None,
                    "this target has no conversion range in the Milestone 8 backend",
                );
                return;
            };
            let body = if outcome == NumericOutcome::Wrapping {
                // Converting through the widest unsigned type is modular in
                // C99, so a narrowing wrap needs no explicit arithmetic.
                format!("    return ({result_c_type})(uintmax_t)value;\n")
            } else {
                let _ = source_primitive;
                format!(
                    "    if ((long double)value < (long double)({minimum})) \
                     return ({result_c_type})({minimum});\n\
                     \x20   if ((long double)value > (long double)({maximum})) \
                     return ({result_c_type})({maximum});\n\
                     \x20   return ({result_c_type})value;\n"
                )
            };
            let _ = writeln!(
                self.output,
                "static {result_c_type} {name}({source_c_type} value) {{\n{body}}}\n"
            );
            return;
        }

        let Some(parts) = self.checked_conversion_parts(result) else {
            return;
        };
        let Some(target_primitive) = self.typed.types.expanded_primitive(parts.target) else {
            return;
        };
        let Some(target_c_type) = self.c_type(parts.target, None) else {
            return;
        };
        let ok = format!(
            "({result_c_type}){{ .tag = UINT32_C({}), .payload.{}\
             = {{ .{} = ({target_c_type})value }} }}",
            parts.ok_variant.index(),
            variant_member_name(parts.ok_variant),
            field_name(parts.ok_field),
        );
        let Some(error_c_type) = self.c_type(parts.error, None) else {
            return;
        };
        let error = |variant: VariantId| {
            format!(
                "({result_c_type}){{ .tag = UINT32_C({}), .payload.{} = {{ .{} = \
                 ({error_c_type}){{ .tag = UINT32_C({}), .payload._empty = 0 }} }} }}",
                parts.err_variant.index(),
                variant_member_name(parts.err_variant),
                field_name(parts.err_field),
                variant.index(),
            )
        };
        let out_of_range = error(parts.out_of_range);
        let not_a_number = error(parts.not_a_number);

        let mut body = String::new();
        if target_primitive.is_float() {
            // Integer-to-float and float-to-float conversions use IEEE
            // rounding and cannot fail (`SPEC.md` 4.1).
            let _ = writeln!(body, "    return {ok};");
        } else {
            let Some((minimum, maximum)) = primitive_bounds(target_primitive, self.options.target)
            else {
                self.type_error(
                    parts.target,
                    None,
                    "this target has no checked-conversion range in the Milestone 8 backend",
                );
                return;
            };
            if source_primitive.is_float() {
                let _ = writeln!(body, "    if (value != value) return {not_a_number};");
                let _ = writeln!(
                    body,
                    "    if (!isfinite((double)value)) return {out_of_range};"
                );
            }
            let _ = writeln!(
                body,
                "    if ((long double)value < (long double)({minimum}) || \
                 (long double)value > (long double)({maximum})) return {out_of_range};"
            );
            let _ = writeln!(body, "    return {ok};");
        }
        let _ = writeln!(
            self.output,
            "static {result_c_type} {name}({source_c_type} value) {{\n{body}}}\n"
        );
    }

    fn standard_call_instances(&self) -> BTreeMap<StandardCall, (TypeId, Span)> {
        let mut calls = BTreeMap::new();
        for function in &self.program.functions {
            for block in &function.blocks {
                for instruction in &block.instructions {
                    if let Instruction::Assign {
                        destination,
                        value: Rvalue::StandardCall { operation, .. },
                        span,
                    } = instruction
                    {
                        calls
                            .entry(*operation)
                            .or_insert((function.temporary_types[destination.index()], *span));
                    }
                }
            }
        }
        calls
    }

    fn collection_literal_instances(&self) -> BTreeSet<(CollectionLiteralKind, TypeId, usize)> {
        let mut literals = BTreeSet::new();
        for function in &self.program.functions {
            for block in &function.blocks {
                for instruction in &block.instructions {
                    if let Instruction::Assign {
                        destination,
                        value: Rvalue::CollectionLiteral { kind, elements },
                        ..
                    } = instruction
                    {
                        literals.insert((
                            *kind,
                            function.temporary_types[destination.index()],
                            elements.len(),
                        ));
                    }
                }
            }
        }
        literals
    }

    fn emit_runtime_allocate(
        &mut self,
        destination: &str,
        byte_count: &str,
        class: AllocationClass,
    ) {
        self.emit_managed_operation(ManagedMemoryOperation::Allocate {
            destination,
            byte_count,
            class,
        });
        let _ = writeln!(
            self.output,
            "    if ({destination} == NULL) el_out_of_memory();"
        );
    }

    fn option_expression(&mut self, option: TypeId, value: Option<(&str, TypeId)>) -> String {
        let Some(enumeration) = self.enums.get(&option).copied() else {
            return zero_value(option, &self.typed.types);
        };
        let variant_name = if value.is_some() { "Some" } else { "None" };
        let Some(variant_id) = self.resolved.standard_variant("Option", variant_name) else {
            return zero_value(option, &self.typed.types);
        };
        let Some(variant) = enumeration
            .variants
            .iter()
            .find(|variant| variant.id == variant_id)
        else {
            return zero_value(option, &self.typed.types);
        };
        let c_type = self
            .c_type(option, None)
            .unwrap_or_else(|| "el_unit".to_string());
        match (value, variant.fields.first()) {
            (Some((expression, value_type)), Some((field, _, _))) => format!(
                "({c_type}){{ .tag = UINT32_C({}), .payload.{} = {{ .{} = {}({expression}) }} }}",
                variant_id.index(),
                variant_member_name(variant_id),
                field_name(*field),
                copy_helper_name(self.resolve_alias(value_type))
            ),
            _ => {
                let payload = enumeration
                    .variants
                    .iter()
                    .find(|candidate| !candidate.fields.is_empty())
                    .map_or_else(
                        || ".payload._empty = 0".to_string(),
                        |candidate| {
                            format!(".payload.{} = {{0}}", variant_member_name(candidate.id))
                        },
                    );
                format!(
                    "({c_type}){{ .tag = UINT32_C({}), {payload} }}",
                    variant_id.index()
                )
            }
        }
    }

    fn emit_standard_runtime_helpers(&mut self) {
        let calls = self.standard_call_instances();
        let literals = self.collection_literal_instances();
        let mut collections = BTreeSet::new();
        for operation in calls.keys() {
            if let Some(collection) = standard_collection_type(*operation) {
                collections.insert(collection);
            }
        }
        collections.extend(literals.iter().map(|(_, ty, _)| *ty));
        collections.extend(self.used_types().into_iter().filter(|ty| {
            match self.typed.types.kind(self.resolve_alias(*ty)) {
                TypeKind::Builtin { builtin, .. } => {
                    matches!(self.resolved.builtin_name(*builtin), "Vec" | "Map" | "Set")
                }
                _ => false,
            }
        }));

        for collection in collections {
            let TypeKind::Builtin { builtin, arguments } = self
                .typed
                .types
                .kind(self.resolve_alias(collection))
                .clone()
            else {
                continue;
            };
            match (self.resolved.builtin_name(builtin), arguments.as_slice()) {
                ("Vec", [element]) => {
                    self.emit_vec_helpers(collection, *element, &calls, &literals);
                }
                ("Map", [key, value]) => {
                    self.emit_map_helpers(collection, *key, *value, &calls, &literals);
                }
                ("Set", [element]) => {
                    self.emit_set_helpers(collection, *element, &calls, &literals);
                }
                _ => {}
            }
        }

        for (operation, (result, _)) in &calls {
            let (StandardCall::ArrayLen { collection } | StandardCall::ArrayGet { collection }) =
                operation
            else {
                continue;
            };
            let TypeKind::Array { element, length } = self
                .typed
                .types
                .kind(self.resolve_alias(*collection))
                .clone()
            else {
                continue;
            };
            let Some(array_type) = self.c_type(*collection, None) else {
                continue;
            };
            match operation {
                StandardCall::ArrayLen { .. } => {
                    let _ = writeln!(
                        self.output,
                        "static uintptr_t {}({array_type} value) {{\n    (void)value;\n    \
                         return (uintptr_t){length}U;\n}}\n",
                        standard_call_name(*operation)
                    );
                }
                StandardCall::ArrayGet { .. } => {
                    let none = self.option_expression(*result, None);
                    let some =
                        self.option_expression(*result, Some(("value.values[index]", element)));
                    let result_type = self
                        .c_type(*result, None)
                        .unwrap_or_else(|| "el_unit".to_string());
                    let _ = writeln!(
                        self.output,
                        "static {result_type} {}({array_type} value, uintptr_t index) {{\n    \
                         if (index >= (uintptr_t){length}U) return {none};\n    return {some};\n}}\n",
                        standard_call_name(*operation)
                    );
                }
                _ => {}
            }
        }
    }

    fn emit_vec_helpers(
        &mut self,
        collection: TypeId,
        element: TypeId,
        calls: &BTreeMap<StandardCall, (TypeId, Span)>,
        literals: &BTreeSet<(CollectionLiteralKind, TypeId, usize)>,
    ) {
        let Some(collection_type) = self.c_type(collection, None) else {
            return;
        };
        let Some(element_type) = self.c_type(element, None) else {
            return;
        };
        let scanned = if self.scanned_allocation(element) {
            AllocationClass::Scanned
        } else {
            AllocationClass::PointerFree
        };
        let new = standard_call_name(StandardCall::VecNew { collection });
        let _ = writeln!(self.output, "static {collection_type} {new}(void) {{");
        let _ = writeln!(self.output, "    {collection_type} result;");
        self.emit_runtime_allocate("result", "sizeof(*result)", AllocationClass::Scanned);
        self.output.push_str(
            "    result->length = 0U;\n    result->capacity = 0U;\n    result->values = NULL;\n    \
             return result;\n}\n\n",
        );

        let operation = StandardCall::VecLen { collection };
        if calls.contains_key(&operation) {
            let _ = writeln!(
                self.output,
                "static uintptr_t {}({collection_type} value) {{ return value->length; }}\n",
                standard_call_name(operation)
            );
        }
        let operation = StandardCall::VecIsEmpty { collection };
        if calls.contains_key(&operation) {
            let _ = writeln!(
                self.output,
                "static bool {}({collection_type} value) {{ return value->length == 0U; }}\n",
                standard_call_name(operation)
            );
        }
        let operation = StandardCall::VecGet { collection };
        if let Some((result, _)) = calls.get(&operation) {
            let result_type = self
                .c_type(*result, None)
                .unwrap_or_else(|| "el_unit".to_string());
            let none = self.option_expression(*result, None);
            let some = self.option_expression(*result, Some(("value->values[index]", element)));
            let _ = writeln!(
                self.output,
                "static {result_type} {}({collection_type} value, uintptr_t index) {{\n    \
                 if (index >= value->length) return {none};\n    return {some};\n}}\n",
                standard_call_name(operation)
            );
        }
        let append = StandardCall::VecAppend { collection };
        let insert = StandardCall::VecInsert { collection };
        if calls.contains_key(&append) || calls.contains_key(&insert) {
            let reserve = format!("el_vec_reserve_t{}", collection.index());
            let _ = writeln!(
                self.output,
                "static void {reserve}({collection_type} value, uintptr_t needed) {{\n    \
                 uintptr_t capacity;\n    {element_type} *replacement;\n    \
                 if (value->capacity >= needed) return;\n    \
                 capacity = value->capacity == 0U ? 4U : value->capacity;\n    \
                 while (capacity < needed) capacity *= 2U;"
            );
            let bytes = format!("capacity * sizeof({element_type})");
            self.emit_runtime_allocate("replacement", &bytes, scanned);
            self.output.push_str(
                "    if (value->length != 0U) memcpy(replacement, value->values, \
                 value->length * sizeof(*replacement));\n\
                 \x20   value->values = replacement;\n\
                 \x20   value->capacity = capacity;\n}\n\n",
            );
            if calls.contains_key(&append) {
                let _ = writeln!(
                    self.output,
                    "static el_unit {}({collection_type} value, {element_type} element) {{\n    \
                     {reserve}(value, value->length + 1U);\n    \
                     value->values[value->length++] = element;\n    return (el_unit){{0}};\n}}\n",
                    standard_call_name(append)
                );
            }
            if calls.contains_key(&insert) {
                let _ = writeln!(
                    self.output,
                    "static el_unit {}({collection_type} value, uintptr_t index, \
                     {element_type} element, const char *path, uint32_t line, uint32_t column) {{\n    \
                     if (index > value->length) el_trap(\"E-RUN-INDEX\", path, line, column);\n    \
                     {reserve}(value, value->length + 1U);\n    \
                     memmove(&value->values[index + 1U], &value->values[index], \
                     (value->length - index) * sizeof(*value->values));\n    \
                     value->values[index] = element;\n    ++value->length;\n    \
                     return (el_unit){{0}};\n}}\n",
                    standard_call_name(insert)
                );
            }
        }
        let remove = StandardCall::VecRemove { collection };
        if calls.contains_key(&remove) {
            let _ = writeln!(
                self.output,
                "static {element_type} {}({collection_type} value, uintptr_t index, \
                 const char *path, uint32_t line, uint32_t column) {{\n    \
                 {element_type} removed;\n    \
                 if (index >= value->length) el_trap(\"E-RUN-INDEX\", path, line, column);\n    \
                 removed = value->values[index];\n    \
                 memmove(&value->values[index], &value->values[index + 1U], \
                 (value->length - index - 1U) * sizeof(*value->values));\n    \
                 --value->length;\n    return removed;\n}}\n",
                standard_call_name(remove)
            );
        }
        let clear = StandardCall::VecClear { collection };
        if calls.contains_key(&clear) {
            let _ = writeln!(
                self.output,
                "static el_unit {}({collection_type} value) {{ value->length = 0U; \
                 return (el_unit){{0}}; }}\n",
                standard_call_name(clear)
            );
        }

        for (_, _, count) in literals
            .iter()
            .filter(|(kind, ty, _)| *kind == CollectionLiteralKind::Vec && *ty == collection)
        {
            let parameters = (0..*count)
                .map(|index| format!("{element_type} v{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let literal_name = format!("el_vec_literal_t{}_n{count}", collection.index());
            let _ = writeln!(
                self.output,
                "static {collection_type} {literal_name}({}) {{\n    \
                 {collection_type} result = {new}();",
                if parameters.is_empty() {
                    "void"
                } else {
                    &parameters
                }
            );
            if *count != 0 {
                let bytes = format!("{count}U * sizeof({element_type})");
                self.emit_runtime_allocate("result->values", &bytes, scanned);
                let _ = writeln!(
                    self.output,
                    "    result->length = {count}U;\n    result->capacity = {count}U;"
                );
                for index in 0..*count {
                    let _ = writeln!(self.output, "    result->values[{index}] = v{index};");
                }
            }
            self.output.push_str("    return result;\n}\n\n");
        }
    }

    fn emit_map_helpers(
        &mut self,
        collection: TypeId,
        key: TypeId,
        value: TypeId,
        calls: &BTreeMap<StandardCall, (TypeId, Span)>,
        literals: &BTreeSet<(CollectionLiteralKind, TypeId, usize)>,
    ) {
        let (Some(collection_type), Some(key_type), Some(value_type)) = (
            self.c_type(collection, None),
            self.c_type(key, None),
            self.c_type(value, None),
        ) else {
            return;
        };
        if self.needs_equality_helper(key) {
            self.emit_equality_helper(key, None);
        }
        let key_equal = self.component_equality(key, "value->keys[index]", "key");
        let stored_hash = self.component_hash(key, "value->keys[index]");
        let target_hash = self.component_hash(key, "key");
        let new = standard_call_name(StandardCall::MapNew { collection });
        let _ = writeln!(self.output, "static {collection_type} {new}(void) {{");
        let _ = writeln!(self.output, "    {collection_type} result;");
        self.emit_runtime_allocate("result", "sizeof(*result)", AllocationClass::Scanned);
        self.output.push_str(
            "    result->length = 0U;\n    result->capacity = 0U;\n    result->keys = NULL;\n    \
             result->values = NULL;\n    return result;\n}\n\n",
        );
        let find = format!("el_map_find_t{}", collection.index());
        let _ = writeln!(
            self.output,
            "intptr_t {find}({collection_type} value, {key_type} key) {{\n    \
             uintptr_t index;\n    uint64_t hash = {target_hash};\n    \
             for (index = 0U; index < value->length; ++index) {{\n        \
             if ({stored_hash} == hash && {key_equal}) return (intptr_t)index;\n    }}\n    \
             return (intptr_t)-1;\n}}\n"
        );
        let reserve = format!("el_map_reserve_t{}", collection.index());
        let key_class = if self.scanned_allocation(key) {
            AllocationClass::Scanned
        } else {
            AllocationClass::PointerFree
        };
        let value_class = if self.scanned_allocation(value) {
            AllocationClass::Scanned
        } else {
            AllocationClass::PointerFree
        };
        let _ = writeln!(
            self.output,
            "void {reserve}({collection_type} value, uintptr_t needed) {{\n    \
             uintptr_t capacity;\n    {key_type} *keys;\n    {value_type} *values;\n    \
             if (value->capacity >= needed) return;\n    \
             capacity = value->capacity == 0U ? 4U : value->capacity;\n    \
             while (capacity < needed) capacity *= 2U;"
        );
        let key_bytes = format!("capacity * sizeof({key_type})");
        self.emit_runtime_allocate("keys", &key_bytes, key_class);
        let value_bytes = format!("capacity * sizeof({value_type})");
        self.emit_runtime_allocate("values", &value_bytes, value_class);
        self.output.push_str(
            "    if (value->length != 0U) {\n        memcpy(keys, value->keys, \
             value->length * sizeof(*keys));\n        memcpy(values, value->values, \
             value->length * sizeof(*values));\n    }\n    value->keys = keys;\n    \
             value->values = values;\n    value->capacity = capacity;\n}\n\n",
        );

        let len = StandardCall::MapLen { collection };
        if calls.contains_key(&len) {
            let _ = writeln!(
                self.output,
                "static uintptr_t {}({collection_type} value) {{ return value->length; }}\n",
                standard_call_name(len)
            );
        }
        let empty = StandardCall::MapIsEmpty { collection };
        if calls.contains_key(&empty) {
            let _ = writeln!(
                self.output,
                "static bool {}({collection_type} value) {{ return value->length == 0U; }}\n",
                standard_call_name(empty)
            );
        }
        let contains = StandardCall::MapContainsKey { collection };
        if calls.contains_key(&contains) {
            let _ = writeln!(
                self.output,
                "static bool {}({collection_type} value, {key_type} key) {{ \
                 return {find}(value, key) >= 0; }}\n",
                standard_call_name(contains)
            );
        }
        let get = StandardCall::MapGet { collection };
        if let Some((result, _)) = calls.get(&get) {
            let result_type = self
                .c_type(*result, None)
                .unwrap_or_else(|| "el_unit".to_string());
            let none = self.option_expression(*result, None);
            let some =
                self.option_expression(*result, Some(("value->values[(uintptr_t)index]", value)));
            let _ = writeln!(
                self.output,
                "static {result_type} {}({collection_type} value, {key_type} key) {{\n    \
                 intptr_t index = {find}(value, key);\n    if (index < 0) return {none};\n    \
                 return {some};\n}}\n",
                standard_call_name(get)
            );
        }
        let insert = StandardCall::MapInsert { collection };
        if let Some((result, _)) = calls.get(&insert) {
            let result_type = self
                .c_type(*result, None)
                .unwrap_or_else(|| "el_unit".to_string());
            let none = self.option_expression(*result, None);
            let some =
                self.option_expression(*result, Some(("value->values[(uintptr_t)index]", value)));
            let _ = writeln!(
                self.output,
                "static {result_type} {}({collection_type} value, {key_type} key, \
                 {value_type} replacement) {{\n    intptr_t index = {find}(value, key);\n    \
                 if (index >= 0) {{\n        {result_type} previous = {some};\n        \
                 value->values[(uintptr_t)index] = replacement;\n        return previous;\n    }}\n    \
                 {reserve}(value, value->length + 1U);\n    value->keys[value->length] = key;\n    \
                 value->values[value->length] = replacement;\n    ++value->length;\n    return {none};\n}}\n",
                standard_call_name(insert)
            );
        }
        let remove = StandardCall::MapRemove { collection };
        if let Some((result, _)) = calls.get(&remove) {
            let result_type = self
                .c_type(*result, None)
                .unwrap_or_else(|| "el_unit".to_string());
            let none = self.option_expression(*result, None);
            let some =
                self.option_expression(*result, Some(("value->values[(uintptr_t)index]", value)));
            let _ = writeln!(
                self.output,
                "static {result_type} {}({collection_type} value, {key_type} key) {{\n    \
                 intptr_t index = {find}(value, key);\n    if (index < 0) return {none};\n    {{\n        \
                 {result_type} removed = {some};\n        uintptr_t tail = value->length - \
                 (uintptr_t)index - 1U;\n        memmove(&value->keys[(uintptr_t)index], \
                 &value->keys[(uintptr_t)index + 1U], tail * sizeof(*value->keys));\n        \
                 memmove(&value->values[(uintptr_t)index], \
                 &value->values[(uintptr_t)index + 1U], tail * sizeof(*value->values));\n        \
                 --value->length;\n        return removed;\n    }}\n}}\n",
                standard_call_name(remove)
            );
        }
        let clear = StandardCall::MapClear { collection };
        if calls.contains_key(&clear) {
            let _ = writeln!(
                self.output,
                "static el_unit {}({collection_type} value) {{ value->length = 0U; \
                 return (el_unit){{0}}; }}\n",
                standard_call_name(clear)
            );
        }

        for (_, _, flat_count) in literals
            .iter()
            .filter(|(kind, ty, _)| *kind == CollectionLiteralKind::Map && *ty == collection)
        {
            let entry_count = flat_count / 2;
            let parameters = (0..entry_count)
                .flat_map(|index| {
                    [
                        format!("{key_type} k{index}"),
                        format!("{value_type} v{index}"),
                    ]
                })
                .collect::<Vec<_>>()
                .join(", ");
            let literal_name = format!("el_map_literal_t{}_n{flat_count}", collection.index());
            let _ = writeln!(
                self.output,
                "static {collection_type} {literal_name}({}) {{\n    \
                 {collection_type} result = {new}();\n    intptr_t found;",
                if parameters.is_empty() {
                    "void"
                } else {
                    &parameters
                }
            );
            for index in 0..entry_count {
                let _ = writeln!(
                    self.output,
                    "    found = {find}(result, k{index});\n    if (found >= 0) {{\n        \
                     result->values[(uintptr_t)found] = v{index};\n    }} else {{\n        \
                     {reserve}(result, result->length + 1U);\n        \
                     result->keys[result->length] = k{index};\n        \
                     result->values[result->length] = v{index};\n        ++result->length;\n    }}"
                );
            }
            self.output.push_str("    return result;\n}\n\n");
        }
    }

    fn emit_set_helpers(
        &mut self,
        collection: TypeId,
        element: TypeId,
        calls: &BTreeMap<StandardCall, (TypeId, Span)>,
        literals: &BTreeSet<(CollectionLiteralKind, TypeId, usize)>,
    ) {
        let (Some(collection_type), Some(element_type)) =
            (self.c_type(collection, None), self.c_type(element, None))
        else {
            return;
        };
        if self.needs_equality_helper(element) {
            self.emit_equality_helper(element, None);
        }
        let equal = self.component_equality(element, "value->values[index]", "element");
        let stored_hash = self.component_hash(element, "value->values[index]");
        let target_hash = self.component_hash(element, "element");
        let new = standard_call_name(StandardCall::SetNew { collection });
        let _ = writeln!(self.output, "static {collection_type} {new}(void) {{");
        let _ = writeln!(self.output, "    {collection_type} result;");
        self.emit_runtime_allocate("result", "sizeof(*result)", AllocationClass::Scanned);
        self.output.push_str(
            "    result->length = 0U;\n    result->capacity = 0U;\n    result->values = NULL;\n    \
             return result;\n}\n\n",
        );
        let find = format!("el_set_find_t{}", collection.index());
        let _ = writeln!(
            self.output,
            "intptr_t {find}({collection_type} value, {element_type} element) {{\n    \
             uintptr_t index;\n    uint64_t hash = {target_hash};\n    \
             for (index = 0U; index < value->length; ++index) {{\n        \
             if ({stored_hash} == hash && {equal}) return (intptr_t)index;\n    }}\n    \
             return (intptr_t)-1;\n}}\n"
        );
        let reserve = format!("el_set_reserve_t{}", collection.index());
        let class = if self.scanned_allocation(element) {
            AllocationClass::Scanned
        } else {
            AllocationClass::PointerFree
        };
        let _ = writeln!(
            self.output,
            "void {reserve}({collection_type} value, uintptr_t needed) {{\n    \
             uintptr_t capacity;\n    {element_type} *replacement;\n    \
             if (value->capacity >= needed) return;\n    \
             capacity = value->capacity == 0U ? 4U : value->capacity;\n    \
             while (capacity < needed) capacity *= 2U;"
        );
        let bytes = format!("capacity * sizeof({element_type})");
        self.emit_runtime_allocate("replacement", &bytes, class);
        self.output.push_str(
            "    if (value->length != 0U) memcpy(replacement, value->values, \
             value->length * sizeof(*replacement));\n    value->values = replacement;\n    \
             value->capacity = capacity;\n}\n\n",
        );
        let len = StandardCall::SetLen { collection };
        if calls.contains_key(&len) {
            let _ = writeln!(
                self.output,
                "static uintptr_t {}({collection_type} value) {{ return value->length; }}\n",
                standard_call_name(len)
            );
        }
        let empty = StandardCall::SetIsEmpty { collection };
        if calls.contains_key(&empty) {
            let _ = writeln!(
                self.output,
                "static bool {}({collection_type} value) {{ return value->length == 0U; }}\n",
                standard_call_name(empty)
            );
        }
        let contains = StandardCall::SetContains { collection };
        if calls.contains_key(&contains) {
            let _ = writeln!(
                self.output,
                "static bool {}({collection_type} value, {element_type} element) {{ \
                 return {find}(value, element) >= 0; }}\n",
                standard_call_name(contains)
            );
        }
        let insert = StandardCall::SetInsert { collection };
        if calls.contains_key(&insert) {
            let _ = writeln!(
                self.output,
                "static bool {}({collection_type} value, {element_type} element) {{\n    \
                 if ({find}(value, element) >= 0) return false;\n    \
                 {reserve}(value, value->length + 1U);\n    \
                 value->values[value->length++] = element;\n    return true;\n}}\n",
                standard_call_name(insert)
            );
        }
        let remove = StandardCall::SetRemove { collection };
        if calls.contains_key(&remove) {
            let _ = writeln!(
                self.output,
                "static bool {}({collection_type} value, {element_type} element) {{\n    \
                 intptr_t index = {find}(value, element);\n    if (index < 0) return false;\n    \
                 memmove(&value->values[(uintptr_t)index], \
                 &value->values[(uintptr_t)index + 1U], \
                 (value->length - (uintptr_t)index - 1U) * sizeof(*value->values));\n    \
                 --value->length;\n    return true;\n}}\n",
                standard_call_name(remove)
            );
        }
        let clear = StandardCall::SetClear { collection };
        if calls.contains_key(&clear) {
            let _ = writeln!(
                self.output,
                "static el_unit {}({collection_type} value) {{ value->length = 0U; \
                 return (el_unit){{0}}; }}\n",
                standard_call_name(clear)
            );
        }

        for (_, _, count) in literals
            .iter()
            .filter(|(kind, ty, _)| *kind == CollectionLiteralKind::Set && *ty == collection)
        {
            let parameters = (0..*count)
                .map(|index| format!("{element_type} v{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            let literal_name = format!("el_set_literal_t{}_n{count}", collection.index());
            let _ = writeln!(
                self.output,
                "static {collection_type} {literal_name}({}) {{\n    \
                 {collection_type} result = {new}();\n    intptr_t found;",
                if parameters.is_empty() {
                    "void"
                } else {
                    &parameters
                }
            );
            for index in 0..*count {
                let _ = writeln!(
                    self.output,
                    "    found = {find}(result, v{index});\n    if (found < 0) {{\n        \
                     {reserve}(result, result->length + 1U);\n        \
                     result->values[result->length++] = v{index};\n    }}"
                );
            }
            self.output.push_str("    return result;\n}\n\n");
        }
    }

    /// The variant and field identities a checked conversion's result value is
    /// built from, or `None` when the standard declarations are unavailable.
    fn checked_conversion_parts(&self, result: TypeId) -> Option<CheckedConversionParts> {
        let TypeKind::Nominal { arguments, .. } = self.typed.types.kind(self.resolve_alias(result))
        else {
            return None;
        };
        let [target, error] = arguments.as_slice() else {
            return None;
        };
        let ok_variant = self.resolved.standard_variant("Result", "Ok")?;
        let err_variant = self.resolved.standard_variant("Result", "Err")?;
        Some(CheckedConversionParts {
            target: *target,
            error: *error,
            ok_variant,
            ok_field: *self.resolved.variants[ok_variant.index()].fields.first()?,
            err_variant,
            err_field: *self.resolved.variants[err_variant.index()].fields.first()?,
            out_of_range: self
                .resolved
                .standard_variant("NumericError", "OutOfRange")?,
            not_a_number: self
                .resolved
                .standard_variant("NumericError", "NotANumber")?,
        })
    }

    /// Every pointee type whose raw pointers are null/alignment-checked.
    fn checked_pointees(&self) -> BTreeSet<TypeId> {
        let mut pointees = BTreeSet::new();
        for function in &self.program.functions {
            for block in &function.blocks {
                for instruction in &block.instructions {
                    if let Instruction::CheckPointer { pointee, .. } = instruction {
                        pointees.insert(self.resolve_alias(*pointee));
                    }
                }
            }
        }
        pointees
    }

    /// Emits the mandatory null/alignment check helper for one pointee type
    /// (`SPEC.md` 3.3, Milestone 16.8). The alignment comes from the C99
    /// `offsetof` probe — a `char` followed by the pointee — because
    /// `_Alignof` is C11-only and the generated C must stay C99.
    fn emit_pointer_check_helper(&mut self, pointee: TypeId) {
        let Some(c_type) = self.c_type(pointee, None) else {
            return;
        };
        let name = pointer_check_name(pointee);
        let _ = writeln!(
            self.output,
            "static void {name}(const void *pointer, const char *path, uint32_t line, \
             uint32_t column) {{\n\
             \x20   struct el_align_probe_t{index} {{ char lead; {c_type} value; }};\n\
             \x20   if (pointer == NULL) el_trap(\"E-RUN-NULL\", path, line, column);\n\
             \x20   if (((uintptr_t)pointer % \
             (uintptr_t)offsetof(struct el_align_probe_t{index}, value)) != 0U) \
             el_trap(\"E-RUN-ALIGN\", path, line, column);\n\
             }}\n",
            index = pointee.index()
        );
    }

    /// Every `(operation, operand type, result type)` a numeric alternative
    /// needs a helper for.
    fn numeric_alternatives(&self) -> BTreeSet<(NumericAlternative, TypeId, TypeId)> {
        let mut used = BTreeSet::new();
        for function in &self.program.functions {
            for block in &function.blocks {
                for instruction in &block.instructions {
                    if let Instruction::Assign {
                        destination,
                        value:
                            Rvalue::NumericAlternative {
                                operation,
                                operand_type,
                                ..
                            },
                        ..
                    } = instruction
                    {
                        used.insert((
                            *operation,
                            *operand_type,
                            function.temporary_types[destination.index()],
                        ));
                    }
                }
            }
        }
        used
    }

    /// Instantiates `EL_SIGNED_ALT`/`EL_UNSIGNED_ALT` for exactly the integer
    /// types the program uses an alternative on, so an unused type contributes
    /// no code.
    fn emit_numeric_alternative_instances(&mut self) {
        let primitives = self
            .numeric_alternatives()
            .into_iter()
            .filter_map(|(_, operand, _)| self.typed.types.expanded_primitive(operand))
            .collect::<BTreeSet<_>>();
        for primitive in primitives {
            let Some(name) = crate::types::primitive_name_for_symbol(primitive) else {
                continue;
            };
            let line = match primitive {
                PrimitiveType::I8 => "EL_SIGNED_ALT(i8, int8_t, uint8_t, INT8_MIN, INT8_MAX, 8)",
                PrimitiveType::I16 => {
                    "EL_SIGNED_ALT(i16, int16_t, uint16_t, INT16_MIN, INT16_MAX, 16)"
                }
                PrimitiveType::I32 => {
                    "EL_SIGNED_ALT(i32, int32_t, uint32_t, INT32_MIN, INT32_MAX, 32)"
                }
                PrimitiveType::I64 => {
                    "EL_SIGNED_ALT(i64, int64_t, uint64_t, INT64_MIN, INT64_MAX, 64)"
                }
                PrimitiveType::Isize => {
                    "EL_SIGNED_ALT(isize, intptr_t, uintptr_t, INTPTR_MIN, INTPTR_MAX, \
                     (sizeof(intptr_t) * CHAR_BIT))"
                }
                PrimitiveType::U8 => "EL_UNSIGNED_ALT(u8, uint8_t, UINT8_MAX, 8)",
                PrimitiveType::U16 => "EL_UNSIGNED_ALT(u16, uint16_t, UINT16_MAX, 16)",
                PrimitiveType::U32 => "EL_UNSIGNED_ALT(u32, uint32_t, UINT32_MAX, 32)",
                PrimitiveType::U64 => "EL_UNSIGNED_ALT(u64, uint64_t, UINT64_MAX, 64)",
                PrimitiveType::Usize => {
                    "EL_UNSIGNED_ALT(usize, uintptr_t, UINTPTR_MAX, (sizeof(uintptr_t) * CHAR_BIT))"
                }
                _ => continue,
            };
            let _ = name;
            let _ = writeln!(self.output, "{line}");
        }
        self.output.push('\n');
    }

    /// Emits one alternative helper. A checked operation pairs the overflow
    /// predicate with the wrapping result to build `Option[T]`; a wrapping or
    /// saturating operation forwards directly to its runtime helper.
    fn emit_numeric_alternative_helper(
        &mut self,
        operation: NumericAlternative,
        operand: TypeId,
        result: TypeId,
    ) {
        let Some(primitive) = self.typed.types.expanded_primitive(operand) else {
            return;
        };
        let Some(suffix) = crate::types::primitive_name_for_symbol(primitive) else {
            return;
        };
        let Some(operand_c_type) = self.c_type(operand, None) else {
            return;
        };
        let Some(result_c_type) = self.c_type(result, None) else {
            return;
        };
        let binary = operation.operator != NumericOperator::Negate;
        let stem = numeric_operator_stem(operation.operator);
        let traps = numeric_alternative_traps(operation);
        let mut parameters = format!("{operand_c_type} a");
        let mut forwarded = "a".to_string();
        if binary {
            let _ = write!(parameters, ", {operand_c_type} b");
            forwarded.push_str(", b");
        }
        if traps {
            parameters.push_str(", const char *p, uint32_t l, uint32_t c");
        }
        let name = numeric_alternative_name(operation, operand, result);
        let body = match operation.outcome {
            NumericOutcome::Checked => {
                let Some(parts) = self.option_parts(result) else {
                    return;
                };
                let none = format!(
                    "({result_c_type}){{ .tag = UINT32_C({}), .payload.{} = {{0}} }}",
                    parts.none_variant.index(),
                    variant_member_name(parts.some_variant),
                );
                let some = format!(
                    "({result_c_type}){{ .tag = UINT32_C({}), .payload.{} = {{ .{} = \
                     el_wrap_{stem}_{suffix}({forwarded}) }} }}",
                    parts.some_variant.index(),
                    variant_member_name(parts.some_variant),
                    field_name(parts.some_field),
                );
                format!(
                    "    if (el_ovf_{stem}_{suffix}({}))\n        return {none};\n    return {some};\n",
                    if binary { "a, b" } else { "a" },
                )
            }
            NumericOutcome::Wrapping if traps => format!(
                "    if (b == 0) el_trap(\"E-RUN-DIVZERO\", p, l, c);\n    \
                 return el_wrap_{stem}_{suffix}(a, b);\n"
            ),
            NumericOutcome::Wrapping => {
                format!("    return el_wrap_{stem}_{suffix}({forwarded});\n")
            }
            NumericOutcome::Saturating => {
                format!("    return el_sat_{stem}_{suffix}({forwarded});\n")
            }
        };
        let _ = writeln!(
            self.output,
            "static {result_c_type} {name}({parameters}) {{\n{body}}}\n"
        );
    }

    /// The variant and field identities an `Option[T]` value is built from.
    fn option_parts(&self, option: TypeId) -> Option<OptionParts> {
        let some_variant = self.resolved.standard_variant("Option", "Some")?;
        Some(OptionParts {
            some_variant,
            some_field: *self.resolved.variants[some_variant.index()]
                .fields
                .first()?,
            none_variant: self.resolved.standard_variant("Option", "None")?,
        })
        .filter(|_| self.enums.contains_key(&self.resolve_alias(option)))
    }

    /// Emits `bool el_eq_T(T, T)` for an aggregate, comparing structurally.
    ///
    /// `SPEC.md` 4.3: derived equality compares components. Reference-like
    /// components compare by identity, which is what `==` on a pointer already
    /// does.
    fn emit_equality_helper(&mut self, ty: TypeId, span: Option<Span>) {
        let ty = self.resolve_alias(ty);
        if self.emitted_equality_helpers.contains(&ty) || !self.emitting_equality_helpers.insert(ty)
        {
            return;
        }
        let kind = self.typed.types.kind(ty).clone();
        // Only components that compare structurally need their own helper.
        let mut components: Vec<TypeId> = Vec::new();
        match &kind {
            TypeKind::Tuple(elements) => components.extend(elements.iter().copied()),
            TypeKind::Array { element, .. } => components.push(*element),
            TypeKind::Nominal { .. } => {
                if let Some(structure) = self.structs.get(&ty).copied() {
                    components.extend(structure.fields.iter().map(|(_, _, ty)| *ty));
                } else if let Some(enumeration) = self.enums.get(&ty).copied() {
                    for variant in &enumeration.variants {
                        components.extend(variant.fields.iter().map(|(_, _, ty)| *ty));
                    }
                }
            }
            TypeKind::Builtin { arguments, .. } => components.extend(arguments.iter().copied()),
            _ => {}
        }
        for component in components {
            if self.needs_equality_helper(component) {
                self.emit_equality_helper(component, span);
            }
        }
        let Some(c_type) = self.c_type(ty, span) else {
            self.emitting_equality_helpers.remove(&ty);
            return;
        };
        let name = equality_helper_name(ty);
        let mut body = String::new();
        match &kind {
            TypeKind::Tuple(elements) => {
                for (index, element) in elements.iter().enumerate() {
                    body.push_str(&format!(
                        "    if (!{}) return false;\n",
                        self.component_equality(
                            *element,
                            &format!("a.v{index}"),
                            &format!("b.v{index}")
                        )
                    ));
                }
            }
            TypeKind::Array { element, length } => {
                body.push_str(&format!(
                    "    for (uintptr_t i = 0; i < {length}u; ++i) {{\n        if (!{}) return false;\n    }}\n",
                    self.component_equality(*element, "a.values[i]", "b.values[i]")
                ));
            }
            TypeKind::Nominal { .. } => {
                if let Some(structure) = self.structs.get(&ty).copied() {
                    for (field, _, field_type) in &structure.fields {
                        let member = field_name(*field);
                        body.push_str(&format!(
                            "    if (!{}) return false;\n",
                            self.component_equality(
                                *field_type,
                                &format!("a.{member}"),
                                &format!("b.{member}")
                            )
                        ));
                    }
                } else if let Some(enumeration) = self.enums.get(&ty).copied() {
                    body.push_str("    if (a.tag != b.tag) return false;\n");
                    for variant in &enumeration.variants {
                        if variant.fields.is_empty() {
                            continue;
                        }
                        let member = variant_member_name(variant.id);
                        let mut arm = String::new();
                        for (field, _, field_type) in &variant.fields {
                            let field_member = field_name(*field);
                            arm.push_str(&format!(
                                "        if (!{}) return false;\n",
                                self.component_equality(
                                    *field_type,
                                    &format!("a.payload.{member}.{field_member}"),
                                    &format!("b.payload.{member}.{field_member}")
                                )
                            ));
                        }
                        body.push_str(&format!(
                            "    if (a.tag == UINT32_C({})) {{\n{arm}    }}\n",
                            variant.id.index()
                        ));
                    }
                }
            }
            TypeKind::Builtin { builtin, arguments } => {
                match (self.resolved.builtin_name(*builtin), arguments.as_slice()) {
                    ("Vec", [element]) => {
                        body.push_str("    if (a->length != b->length) return false;\n");
                        body.push_str(&format!(
                            "    for (uintptr_t i = 0U; i < a->length; ++i) {{\n        \
                             if (!{}) return false;\n    }}\n",
                            self.component_equality(*element, "a->values[i]", "b->values[i]")
                        ));
                    }
                    ("Map", [key, value]) => {
                        let key_equal = self.component_equality(*key, "a->keys[i]", "b->keys[j]");
                        let value_equal =
                            self.component_equality(*value, "a->values[i]", "b->values[j]");
                        body.push_str(
                            "    if (a->length != b->length) return false;\n    \
                             for (uintptr_t i = 0U; i < a->length; ++i) {\n        bool found = false;\n        \
                             for (uintptr_t j = 0U; j < b->length; ++j) {\n",
                        );
                        body.push_str(&format!(
                            "            if ({key_equal}) {{\n                \
                             if (!{value_equal}) return false;\n                found = true;\n                break;\n            }}\n"
                        ));
                        body.push_str("        }\n        if (!found) return false;\n    }\n");
                    }
                    ("Set", [element]) => {
                        let equal =
                            self.component_equality(*element, "a->values[i]", "b->values[j]");
                        body.push_str(
                            "    if (a->length != b->length) return false;\n    \
                             for (uintptr_t i = 0U; i < a->length; ++i) {\n        bool found = false;\n        \
                             for (uintptr_t j = 0U; j < b->length; ++j) {\n",
                        );
                        body.push_str(&format!(
                            "            if ({equal}) {{ found = true; break; }}\n"
                        ));
                        body.push_str("        }\n        if (!found) return false;\n    }\n");
                    }
                    ("Identity", [_]) => {
                        body.push_str("    return a.target == b.target;\n");
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        let _ = writeln!(
            self.output,
            "static bool {name}({c_type} a, {c_type} b) {{\n{body}    (void)a;\n    (void)b;\n    return true;\n}}\n"
        );
        self.emitting_equality_helpers.remove(&ty);
        self.emitted_equality_helpers.insert(ty);
    }

    /// A boolean C expression comparing one component.
    fn component_equality(&mut self, ty: TypeId, left: &str, right: &str) -> String {
        let ty = self.resolve_alias(ty);
        if matches!(
            self.typed.types.expanded_primitive(ty),
            Some(PrimitiveType::Str | PrimitiveType::String)
        ) {
            return format!(
                "el_text_equal(({left}).bytes, ({left}).length, \
                 ({right}).bytes, ({right}).length)"
            );
        }
        if self.needs_equality_helper(ty) {
            format!("{}({left}, {right})", equality_helper_name(ty))
        } else {
            format!("({left} == {right})")
        }
    }

    /// Whether a type compares structurally rather than with C `==`.
    fn needs_equality_helper(&self, ty: TypeId) -> bool {
        let ty = self.resolve_alias(ty);
        matches!(
            self.typed.types.kind(ty),
            TypeKind::Tuple(_) | TypeKind::Array { .. } | TypeKind::Builtin { .. }
        ) || self.structs.contains_key(&ty)
            || self.enums.contains_key(&ty)
    }

    /// Emits a comparison helper returning -1, 0, 1, or 2 for unordered.
    ///
    /// The fourth result preserves IEEE partial ordering when a derived
    /// aggregate contains a floating-point field. Call sites translate it to
    /// `false` for all four relational operators.
    fn emit_ordering_helper(&mut self, ty: TypeId, span: Option<Span>) {
        let ty = self.resolve_alias(ty);
        if self.emitted_ordering_helpers.contains(&ty) || !self.emitting_ordering_helpers.insert(ty)
        {
            return;
        }
        let kind = self.typed.types.kind(ty).clone();
        let mut components = Vec::new();
        match &kind {
            TypeKind::Tuple(elements) => components.extend(elements.iter().copied()),
            TypeKind::Array { element, .. } => components.push(*element),
            TypeKind::Nominal { .. } => {
                if let Some(structure) = self.structs.get(&ty).copied() {
                    components.extend(structure.fields.iter().map(|(_, _, ty)| *ty));
                } else if let Some(enumeration) = self.enums.get(&ty).copied() {
                    for variant in &enumeration.variants {
                        components.extend(variant.fields.iter().map(|(_, _, ty)| *ty));
                    }
                }
            }
            TypeKind::Builtin { arguments, .. } => components.extend(arguments.iter().copied()),
            _ => {}
        }
        for component in components {
            if self.needs_ordering_helper(component) {
                self.emit_ordering_helper(component, span);
            }
        }
        let Some(c_type) = self.c_type(ty, span) else {
            self.emitting_ordering_helpers.remove(&ty);
            return;
        };
        let name = ordering_helper_name(ty);
        let mut body = String::from("    int order;\n");
        let append_component = |body: &mut String, expression: String, indentation: &str| {
            body.push_str(&format!(
                "{indentation}order = {expression};\n\
                     {indentation}if (order != 0) return order;\n"
            ));
        };
        match &kind {
            TypeKind::Tuple(elements) => {
                for (index, element) in elements.iter().enumerate() {
                    let expression = self.component_ordering(
                        *element,
                        &format!("a.v{index}"),
                        &format!("b.v{index}"),
                    );
                    append_component(&mut body, expression, "    ");
                }
            }
            TypeKind::Array { element, length } => {
                body.push_str(&format!(
                    "    for (uintptr_t i = 0; i < {length}u; ++i) {{\n"
                ));
                let expression = self.component_ordering(*element, "a.values[i]", "b.values[i]");
                append_component(&mut body, expression, "        ");
                body.push_str("    }\n");
            }
            TypeKind::Nominal { .. } => {
                if let Some(structure) = self.structs.get(&ty).copied() {
                    for (field, _, field_type) in &structure.fields {
                        let member = field_name(*field);
                        let expression = self.component_ordering(
                            *field_type,
                            &format!("a.{member}"),
                            &format!("b.{member}"),
                        );
                        append_component(&mut body, expression, "    ");
                    }
                } else if let Some(enumeration) = self.enums.get(&ty).copied() {
                    body.push_str("    if (a.tag != b.tag) return a.tag < b.tag ? -1 : 1;\n");
                    for variant in &enumeration.variants {
                        if variant.fields.is_empty() {
                            continue;
                        }
                        let member = variant_member_name(variant.id);
                        body.push_str(&format!(
                            "    if (a.tag == UINT32_C({})) {{\n",
                            variant.id.index()
                        ));
                        for (field, _, field_type) in &variant.fields {
                            let field_member = field_name(*field);
                            let expression = self.component_ordering(
                                *field_type,
                                &format!("a.payload.{member}.{field_member}"),
                                &format!("b.payload.{member}.{field_member}"),
                            );
                            append_component(&mut body, expression, "        ");
                        }
                        body.push_str("    }\n");
                    }
                }
            }
            TypeKind::Builtin { builtin, arguments }
                if self.resolved.builtin_name(*builtin) == "Vec" =>
            {
                if let [element] = arguments.as_slice() {
                    body.push_str(
                        "    uintptr_t length = a->length < b->length ? a->length : b->length;\n    \
                         for (uintptr_t i = 0U; i < length; ++i) {\n",
                    );
                    let expression =
                        self.component_ordering(*element, "a->values[i]", "b->values[i]");
                    append_component(&mut body, expression, "        ");
                    body.push_str(
                        "    }\n    if (a->length < b->length) return -1;\n    \
                         if (a->length > b->length) return 1;\n",
                    );
                }
            }
            _ => {}
        }
        let _ = writeln!(
            self.output,
            "static int {name}({c_type} a, {c_type} b) {{\n{body}    (void)a;\n    \
             (void)b;\n    return 0;\n}}\n"
        );
        self.emitting_ordering_helpers.remove(&ty);
        self.emitted_ordering_helpers.insert(ty);
    }

    /// A normalized structural comparison expression: -1, 0, 1, or 2 when
    /// unordered.
    fn component_ordering(&mut self, ty: TypeId, left: &str, right: &str) -> String {
        let ty = self.resolve_alias(ty);
        if matches!(
            self.typed.types.expanded_primitive(ty),
            Some(PrimitiveType::Str | PrimitiveType::String)
        ) {
            return format!(
                "el_text_order(({left}).bytes, ({left}).length, \
                 ({right}).bytes, ({right}).length)"
            );
        }
        if self.needs_ordering_helper(ty) {
            return format!("{}({left}, {right})", ordering_helper_name(ty));
        }
        format!(
            "(({left}) < ({right}) ? -1 : (({left}) > ({right}) ? 1 : \
             (({left}) == ({right}) ? 0 : 2)))"
        )
    }

    fn needs_ordering_helper(&self, ty: TypeId) -> bool {
        let ty = self.resolve_alias(ty);
        match self.typed.types.kind(ty) {
            TypeKind::Builtin { builtin, .. } => self.resolved.builtin_name(*builtin) == "Vec",
            _ => self.needs_equality_helper(ty),
        }
    }

    fn stable_key_types(&self) -> BTreeSet<TypeId> {
        let mut keys = BTreeSet::new();
        for ty in self.used_types() {
            let ty = self.resolve_alias(ty);
            let TypeKind::Builtin { builtin, arguments } = self.typed.types.kind(ty) else {
                continue;
            };
            match (self.resolved.builtin_name(*builtin), arguments.as_slice()) {
                ("Map", [key, _]) | ("Set", [key]) => {
                    keys.insert(*key);
                }
                _ => {}
            }
        }
        keys
    }

    fn needs_hash_helper(&self, ty: TypeId) -> bool {
        let ty = self.resolve_alias(ty);
        matches!(
            self.typed.types.kind(ty),
            TypeKind::Tuple(_)
                | TypeKind::Array { .. }
                | TypeKind::Nominal { .. }
                | TypeKind::Builtin { .. }
        )
    }

    fn component_hash(&self, ty: TypeId, value: &str) -> String {
        let ty = self.resolve_alias(ty);
        if let Some(primitive) = self.typed.types.expanded_primitive(ty) {
            return match primitive {
                PrimitiveType::Unit => "UINT64_C(0)".to_string(),
                PrimitiveType::Str | PrimitiveType::String => {
                    format!("el_hash_bytes(({value}).bytes, ({value}).length)")
                }
                PrimitiveType::I128 | PrimitiveType::U128 => format!(
                    "el_hash_combine(el_hash_u64(({value}).lo), el_hash_u64((uint64_t)({value}).hi))"
                ),
                _ => format!("el_hash_u64((uint64_t)({value}))"),
            };
        }
        if self.needs_hash_helper(ty) {
            format!("{}({value})", hash_helper_name(ty))
        } else {
            format!("el_hash_u64((uint64_t)(uintptr_t)({value}))")
        }
    }

    fn emit_hash_helper(&mut self, ty: TypeId, span: Option<Span>) {
        let ty = self.resolve_alias(ty);
        if self.emitted_hash_helpers.contains(&ty) || !self.emitting_hash_helpers.insert(ty) {
            return;
        }
        let kind = self.typed.types.kind(ty).clone();
        let components = match &kind {
            TypeKind::Tuple(elements) => elements.clone(),
            TypeKind::Array { element, .. } => vec![*element],
            TypeKind::Nominal { .. } => {
                if let Some(structure) = self.structs.get(&ty).copied() {
                    structure
                        .fields
                        .iter()
                        .map(|(_, _, field_type)| *field_type)
                        .collect()
                } else if let Some(enumeration) = self.enums.get(&ty).copied() {
                    enumeration
                        .variants
                        .iter()
                        .flat_map(|variant| {
                            variant.fields.iter().map(|(_, _, field_type)| *field_type)
                        })
                        .collect()
                } else {
                    Vec::new()
                }
            }
            TypeKind::Builtin { arguments, .. } => arguments.clone(),
            _ => Vec::new(),
        };
        for component in components {
            if self.needs_hash_helper(component) {
                self.emit_hash_helper(component, span);
            }
        }
        let Some(c_type) = self.c_type(ty, span) else {
            self.emitting_hash_helpers.remove(&ty);
            return;
        };
        let mut body = String::from("    uint64_t hash = UINT64_C(14695981039346656037);\n");
        match &kind {
            TypeKind::Tuple(elements) => {
                for (index, element) in elements.iter().enumerate() {
                    let value = self.component_hash(*element, &format!("value.v{index}"));
                    let _ = writeln!(body, "    hash = el_hash_combine(hash, {value});");
                }
            }
            TypeKind::Array { element, length } => {
                let value = self.component_hash(*element, "value.values[index]");
                let _ = writeln!(
                    body,
                    "    for (uintptr_t index = 0U; index < {length}U; ++index) \
                     hash = el_hash_combine(hash, {value});"
                );
            }
            TypeKind::Nominal { .. } => {
                if let Some(structure) = self.structs.get(&ty).copied() {
                    for (field, _, field_type) in &structure.fields {
                        let value = self
                            .component_hash(*field_type, &format!("value.{}", field_name(*field)));
                        let _ = writeln!(body, "    hash = el_hash_combine(hash, {value});");
                    }
                } else if let Some(enumeration) = self.enums.get(&ty).copied() {
                    body.push_str(
                        "    hash = el_hash_combine(hash, el_hash_u64(value.tag));\n    \
                         switch (value.tag) {\n",
                    );
                    for variant in &enumeration.variants {
                        let _ = writeln!(body, "    case UINT32_C({}):", variant.id.index());
                        for (field, _, field_type) in &variant.fields {
                            let expression = format!(
                                "value.payload.{}.{}",
                                variant_member_name(variant.id),
                                field_name(*field)
                            );
                            let value = self.component_hash(*field_type, &expression);
                            let _ =
                                writeln!(body, "        hash = el_hash_combine(hash, {value});");
                        }
                        body.push_str("        break;\n");
                    }
                    body.push_str("    default: abort();\n    }\n");
                }
            }
            TypeKind::Builtin { builtin, arguments }
                if self.resolved.builtin_name(*builtin) == "Identity" && arguments.len() == 1 =>
            {
                body.push_str(
                    "    hash = el_hash_combine(hash, \
                     el_hash_u64((uint64_t)(uintptr_t)value.target));\n",
                );
            }
            _ => {}
        }
        let _ = writeln!(
            self.output,
            "static uint64_t {}({c_type} value) {{\n{body}    return hash;\n}}\n",
            hash_helper_name(ty)
        );
        self.emitting_hash_helpers.remove(&ty);
        self.emitted_hash_helpers.insert(ty);
    }

    fn emit_copy_helper(&mut self, ty: TypeId, span: Option<Span>) {
        let ty = self.resolve_alias(ty);
        if self.emitted_copy_helpers.contains(&ty) || !self.emitting_copy_helpers.insert(ty) {
            return;
        }
        let kind = self.typed.types.kind(ty).clone();
        let strategy = logical_copy_strategy(&self.typed.types, ty);
        match &kind {
            TypeKind::Tuple(elements) => {
                for element in elements {
                    self.emit_copy_helper(*element, span);
                }
            }
            TypeKind::Array { element, .. } => self.emit_copy_helper(*element, span),
            TypeKind::Nominal { .. } => {
                if let Some(structure) = self.structs.get(&ty).copied() {
                    for (_, _, field_type) in &structure.fields {
                        self.emit_copy_helper(*field_type, span);
                    }
                } else if let Some(enumeration) = self.enums.get(&ty).copied() {
                    for variant in &enumeration.variants {
                        for (_, _, field_type) in &variant.fields {
                            self.emit_copy_helper(*field_type, span);
                        }
                    }
                }
            }
            TypeKind::Builtin { arguments, .. } => {
                for argument in arguments {
                    self.emit_copy_helper(*argument, span);
                }
            }
            _ => {}
        }
        let Some(c_type) = self.c_type(ty, span) else {
            self.emitting_copy_helpers.remove(&ty);
            return;
        };
        let name = copy_helper_name(ty);
        let _ = writeln!(self.output, "{c_type} {name}({c_type} value) {{");
        match kind {
            TypeKind::Primitive(PrimitiveType::String) => {
                self.output.push_str("    return el_copy_string(value);\n");
            }
            TypeKind::Tuple(elements) => {
                let _ = writeln!(self.output, "    {c_type} result = {{0}};");
                for (index, element) in elements.iter().enumerate() {
                    let helper = copy_helper_name(self.resolve_alias(*element));
                    let _ = writeln!(
                        self.output,
                        "    result.v{index} = {helper}(value.v{index});"
                    );
                }
                self.output.push_str("    return result;\n");
            }
            TypeKind::Array { element, length } => {
                let helper = copy_helper_name(self.resolve_alias(element));
                let _ = writeln!(
                    self.output,
                    "    {c_type} result = {{0}};\n    size_t index;"
                );
                let _ = writeln!(
                    self.output,
                    "    for (index = 0U; index < {length}U; ++index) {{"
                );
                let _ = writeln!(
                    self.output,
                    "        result.values[index] = {helper}(value.values[index]);"
                );
                self.output.push_str("    }\n    return result;\n");
            }
            TypeKind::Nominal { .. } => {
                if let Some(structure) = self.structs.get(&ty).copied() {
                    let _ = writeln!(self.output, "    {c_type} result = {{0}};");
                    for (field, _, field_type) in &structure.fields {
                        let helper = copy_helper_name(self.resolve_alias(*field_type));
                        let field = field_name(*field);
                        let _ =
                            writeln!(self.output, "    result.{field} = {helper}(value.{field});");
                    }
                    self.output.push_str("    return result;\n");
                } else if let Some(enumeration) = self.enums.get(&ty).copied() {
                    let _ = writeln!(
                        self.output,
                        "    {c_type} result = {{0}};\n    result.tag = value.tag;\n    switch (value.tag) {{"
                    );
                    for variant in &enumeration.variants {
                        let _ = writeln!(self.output, "    case UINT32_C({}):", variant.id.index());
                        for (field, _, field_type) in &variant.fields {
                            let helper = copy_helper_name(self.resolve_alias(*field_type));
                            let member = variant_member_name(variant.id);
                            let field = field_name(*field);
                            let _ = writeln!(
                                self.output,
                                "        result.payload.{member}.{field} = \
                                 {helper}(value.payload.{member}.{field});"
                            );
                        }
                        self.output.push_str("        break;\n");
                    }
                    self.output
                        .push_str("    default: abort();\n    }\n    return result;\n");
                } else {
                    self.output.push_str("    return value;\n");
                }
            }
            TypeKind::Builtin { builtin, arguments } => {
                match (self.resolved.builtin_name(builtin), arguments.as_slice()) {
                    ("Vec" | "Set", [element]) => {
                        let element_type = self
                            .c_type(*element, span)
                            .unwrap_or_else(|| "uint8_t".to_string());
                        let helper = copy_helper_name(self.resolve_alias(*element));
                        let _ = writeln!(self.output, "    {c_type} result;");
                        self.emit_managed_operation(ManagedMemoryOperation::Allocate {
                            destination: "result",
                            byte_count: "sizeof(*result)",
                            class: AllocationClass::Scanned,
                        });
                        self.output.push_str(
                            "    if (result == NULL) el_out_of_memory();\n\
                             \x20   result->length = value->length;\n\
                             \x20   result->capacity = value->length;\n\
                             \x20   result->values = NULL;\n\
                             \x20   if (value->length != 0U) {\n",
                        );
                        let bytes = format!("value->length * sizeof({element_type})");
                        self.emit_managed_operation(ManagedMemoryOperation::Allocate {
                            destination: "result->values",
                            byte_count: &bytes,
                            class: if self.scanned_allocation(*element) {
                                AllocationClass::Scanned
                            } else {
                                AllocationClass::PointerFree
                            },
                        });
                        let _ = writeln!(
                            self.output,
                            "        if (result->values == NULL) el_out_of_memory();\n\
                             \x20       for (uintptr_t index = 0U; index < value->length; ++index) \
                             result->values[index] = {helper}(value->values[index]);\n\
                             \x20   }}\n    return result;"
                        );
                    }
                    ("Map", [key, value_type]) => {
                        let key_c_type = self
                            .c_type(*key, span)
                            .unwrap_or_else(|| "uint8_t".to_string());
                        let value_c_type = self
                            .c_type(*value_type, span)
                            .unwrap_or_else(|| "uint8_t".to_string());
                        let key_helper = copy_helper_name(self.resolve_alias(*key));
                        let value_helper = copy_helper_name(self.resolve_alias(*value_type));
                        let _ = writeln!(self.output, "    {c_type} result;");
                        self.emit_managed_operation(ManagedMemoryOperation::Allocate {
                            destination: "result",
                            byte_count: "sizeof(*result)",
                            class: AllocationClass::Scanned,
                        });
                        self.output.push_str(
                            "    if (result == NULL) el_out_of_memory();\n\
                             \x20   result->length = value->length;\n\
                             \x20   result->capacity = value->length;\n\
                             \x20   result->keys = NULL;\n\
                             \x20   result->values = NULL;\n\
                             \x20   if (value->length != 0U) {\n",
                        );
                        let key_bytes = format!("value->length * sizeof({key_c_type})");
                        self.emit_managed_operation(ManagedMemoryOperation::Allocate {
                            destination: "result->keys",
                            byte_count: &key_bytes,
                            class: if self.scanned_allocation(*key) {
                                AllocationClass::Scanned
                            } else {
                                AllocationClass::PointerFree
                            },
                        });
                        let value_bytes = format!("value->length * sizeof({value_c_type})");
                        self.emit_managed_operation(ManagedMemoryOperation::Allocate {
                            destination: "result->values",
                            byte_count: &value_bytes,
                            class: if self.scanned_allocation(*value_type) {
                                AllocationClass::Scanned
                            } else {
                                AllocationClass::PointerFree
                            },
                        });
                        let _ = writeln!(
                            self.output,
                            "        if (result->keys == NULL || result->values == NULL) \
                             el_out_of_memory();\n\
                             \x20       for (uintptr_t index = 0U; index < value->length; ++index) \
                             {{\n\
                             \x20           result->keys[index] = \
                             {key_helper}(value->keys[index]);\n\
                             \x20           result->values[index] = \
                             {value_helper}(value->values[index]);\n\
                             \x20       }}\n\
                             \x20   }}\n    return result;"
                        );
                    }
                    ("Identity" | "ForeignRoot" | "ForeignRootMut", [_]) => {
                        self.output.push_str("    return value;\n")
                    }
                    ("Formatter", []) => {
                        let _ = writeln!(self.output, "    {c_type} result;");
                        self.emit_managed_operation(ManagedMemoryOperation::Allocate {
                            destination: "result",
                            byte_count: "sizeof(*result)",
                            class: AllocationClass::Scanned,
                        });
                        self.output.push_str(
                            "    if (result == NULL) el_out_of_memory();\n\
                             \x20   result->length = value->length;\n\
                             \x20   result->capacity = value->length;\n\
                             \x20   result->bytes = NULL;\n\
                             \x20   if (value->length != 0U) {\n",
                        );
                        self.emit_managed_operation(ManagedMemoryOperation::Allocate {
                            destination: "result->bytes",
                            byte_count: "value->length + 1U",
                            class: AllocationClass::PointerFree,
                        });
                        self.output.push_str(
                            "        if (result->bytes == NULL) el_out_of_memory();\n\
                             \x20       memcpy(result->bytes, value->bytes, value->length);\n\
                             \x20       result->bytes[value->length] = '\\0';\n\
                             \x20   }\n\
                             \x20   return result;\n",
                        );
                    }
                    _ => {
                        self.type_error(
                            ty,
                            span,
                            "this builtin runtime type has no logical-copy operation",
                        );
                        self.output.push_str("    abort();\n");
                    }
                }
            }
            _ if matches!(
                strategy,
                LogicalCopyStrategy::Trivial | LogicalCopyStrategy::PreserveIdentity
            ) =>
            {
                self.output.push_str("    return value;\n")
            }
            _ => {
                self.type_error(
                    ty,
                    span,
                    "this runtime representation has no logical-copy operation",
                );
                self.output.push_str("    abort();\n");
            }
        }
        self.output.push_str("}\n\n");
        self.emitting_copy_helpers.remove(&ty);
        self.emitted_copy_helpers.insert(ty);
    }

    fn emit_prototypes(&mut self) {
        for function in &self.program.functions {
            let Some(return_type) =
                self.c_function_return_type(function.return_type, Some(function.span))
            else {
                continue;
            };
            let symbol = self.function_symbol(&function.instance);
            let parameters = self.parameter_list(function);
            let _ = writeln!(self.output, "{return_type} {symbol}({parameters});");
        }
        self.output.push('\n');
    }

    fn emit_function(&mut self, function: &ControlFlowFunction) {
        self.promoted = function.promoted_locals.clone();
        let Some(return_type) =
            self.c_function_return_type(function.return_type, Some(function.span))
        else {
            self.promoted.clear();
            return;
        };
        let symbol = self.function_symbol(&function.instance);
        let parameters = self.parameter_list(function);
        let location = self.location(function.span);
        let _ = writeln!(
            self.output,
            "/* {}:{}:{} — {} */",
            c_comment(&location.path),
            location.line,
            location.column,
            c_comment(&function.name)
        );
        let _ = writeln!(self.output, "{return_type} {symbol}({parameters}) {{");
        let parameter_bindings = function
            .parameters
            .iter()
            .map(|parameter| parameter.binding)
            .collect::<BTreeSet<_>>();
        for (binding, ty) in &function.local_types {
            if parameter_bindings.contains(binding) || self.promoted.contains(binding) {
                continue;
            }
            if let Some(c_type) = self.c_type(*ty, Some(function.span)) {
                let _ = writeln!(
                    self.output,
                    "    {c_type} {} = {};",
                    local_name(*binding),
                    zero_value(*ty, &self.typed.types)
                );
            }
        }
        for parameter in &function.parameters {
            let _ = writeln!(self.output, "    (void){};", local_name(parameter.binding));
        }
        // A promoted local lives in a managed cell so a reference to it stays
        // valid after the frame ends. The cell pointer itself is an ordinary
        // stack variable, which Boehm's conservative stack scan treats as a
        // root for as long as the frame is live.
        let promoted = self.promoted.clone();
        for binding in &promoted {
            let Some(ty) = function.local_types.get(binding).copied() else {
                continue;
            };
            let Some(c_type) = self.c_type(ty, Some(function.span)) else {
                continue;
            };
            let cell = cell_name(*binding);
            let class = if self.scanned_allocation(ty) {
                AllocationClass::Scanned
            } else {
                AllocationClass::PointerFree
            };
            let _ = writeln!(self.output, "    {c_type} *{cell} = NULL;");
            let byte_count = format!("sizeof({c_type})");
            self.emit_managed_operation(ManagedMemoryOperation::Allocate {
                destination: &cell,
                byte_count: &byte_count,
                class,
            });
            let _ = writeln!(self.output, "    if ({cell} == NULL) el_out_of_memory();");
            let initial = if parameter_bindings.contains(binding) {
                local_name(*binding)
            } else {
                // A braced zero initializer is only valid in a declaration, so
                // an assignment into the cell needs a C99 compound literal.
                let zero = zero_value(ty, &self.typed.types);
                if zero.starts_with('{') {
                    format!("({c_type}){zero}")
                } else {
                    zero
                }
            };
            let _ = writeln!(self.output, "    *{cell} = {initial};");
        }
        for (index, ty) in function.temporary_types.iter().enumerate() {
            if let Some(c_type) = self.c_type(*ty, Some(function.span)) {
                let _ = writeln!(
                    self.output,
                    "    {c_type} t{index} = {};",
                    zero_value(*ty, &self.typed.types)
                );
            }
        }
        let _ = writeln!(self.output, "    goto b{};", function.entry.index());
        // Only blocks reachable from the entry are emitted. Reachability must
        // be transitive: a block reached solely from an unreachable block
        // would otherwise emit a label with no `goto` naming it, which C
        // compilers reject under `-Werror=unused-label`. Lowering produces
        // unreachable blocks routinely — a `match` whose every arm returns
        // leaves its join block with no live predecessor.
        let reachable_blocks = reachable_blocks(function);
        for block in &function.blocks {
            if !reachable_blocks.contains(&block.id) {
                continue;
            }
            let _ = writeln!(self.output, "b{}:", block.id.index());
            for instruction in &block.instructions {
                self.emit_instruction(function, instruction);
            }
            self.emit_terminator(function, &block.terminator);
        }
        self.output.push_str("}\n\n");
        self.promoted.clear();
    }

    fn parameter_list(&mut self, function: &ControlFlowFunction) -> String {
        if function.parameters.is_empty() {
            return "void".to_string();
        }
        function
            .parameters
            .iter()
            .filter_map(|parameter| {
                self.c_type(parameter.ty, Some(parameter.span))
                    .map(|ty| format!("{ty} {}", local_name(parameter.binding)))
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn emit_instruction(&mut self, function: &ControlFlowFunction, instruction: &Instruction) {
        match instruction {
            Instruction::CheckPointer {
                pointer,
                pointee,
                span,
            } => {
                let arguments = self.trap_arguments(*span);
                let _ = writeln!(
                    self.output,
                    "    {}((const void *){}, {arguments});",
                    pointer_check_name(self.resolve_alias(*pointee)),
                    temporary_name(*pointer)
                );
            }
            Instruction::CheckFunctionPointer { pointer, span } => {
                let arguments = self.trap_arguments(*span);
                let _ = writeln!(
                    self.output,
                    "    if ({} == NULL) el_trap(\"E-RUN-NULL\", {arguments});",
                    temporary_name(*pointer)
                );
            }
            Instruction::Assign {
                destination,
                value,
                span,
            } => {
                self.emit_place_checks(value_place(value), *span);
                if let Some(expression) = self.rvalue(function, *destination, value, *span) {
                    let destination_name = temporary_name(*destination);
                    let _ = writeln!(
                        self.output,
                        "    {destination_name} = {expression};\n    (void){destination_name};"
                    );
                    if self.program.requires_managed_memory
                        && let Rvalue::Call {
                            instance,
                            arguments,
                        } = value
                        && self.resolved.declarations[instance.declaration.index()].kind
                            == crate::resolution::DeclarationKind::ForeignFunction
                    {
                        for argument in arguments {
                            if matches!(
                                self.typed.types.kind(
                                    self.typed.types.resolve_inference(
                                        function.temporary_types[argument.index()]
                                    )
                                ),
                                TypeKind::RawPointer { .. }
                            ) {
                                self.emit_managed_operation(ManagedMemoryOperation::KeepAlive {
                                    expression: &temporary_name(*argument),
                                });
                            }
                        }
                    }
                }
            }
            Instruction::Store { place, value, span } => {
                self.emit_place_checks(Some(place), *span);
                let expression = self.place_expression(place);
                let _ = writeln!(
                    self.output,
                    "    {expression} = {};\n    (void){expression};",
                    temporary_name(*value),
                );
            }
            Instruction::PrintValue { value, ty, span } => {
                self.emit_print(*value, *ty, *span);
            }
            Instruction::PrintNewline { .. } => {
                self.output.push_str("    fputc('\\n', stdout);\n");
            }
        }
    }

    fn rvalue(
        &mut self,
        function: &ControlFlowFunction,
        destination: TemporaryId,
        value: &Rvalue,
        span: Span,
    ) -> Option<String> {
        let destination_type = function.temporary_types[destination.index()];
        Some(match value {
            Rvalue::Constant(constant) => {
                self.constant_expression(constant, destination_type, span)?
            }
            Rvalue::FunctionReference(instance) => self.function_symbol(instance),
            Rvalue::Load(place) => self.place_expression(place),
            Rvalue::AddressOf(place) => format!("&{}", self.place_expression(place)),
            Rvalue::DefaultValue(ty) => self.default_expression(*ty, span)?,
            Rvalue::NumericConversion {
                outcome,
                value,
                source_type,
                ..
            } => format!(
                "{}({})",
                numeric_conversion_name(*outcome, *source_type, destination_type),
                temporary_name(*value)
            ),
            Rvalue::NumericAlternative {
                operation,
                receiver,
                operand,
                operand_type,
            } => {
                let mut call = format!(
                    "{}({}",
                    numeric_alternative_name(*operation, *operand_type, destination_type),
                    temporary_name(*receiver)
                );
                if let Some(operand) = operand {
                    let _ = write!(call, ", {}", temporary_name(*operand));
                }
                // Only the wrapping division and remainder helpers can trap,
                // and only on a zero divisor, so only they need a location.
                if numeric_alternative_traps(*operation) {
                    let _ = write!(call, ", {}", self.trap_arguments(span));
                }
                call.push(')');
                call
            }
            Rvalue::StandardCall {
                operation,
                arguments,
            } => {
                if matches!(operation, StandardCall::ForeignRootRetain { .. }) {
                    let value = arguments
                        .first()
                        .map_or_else(|| "NULL".to_string(), |value| temporary_name(*value));
                    return Some(format!("el_foreign_root_retain((void *){value})"));
                }
                if matches!(operation, StandardCall::ForeignRootPointer { .. }) {
                    let receiver = arguments
                        .first()
                        .map_or_else(|| "NULL".to_string(), |value| temporary_name(*value));
                    return Some(format!(
                        "({})el_foreign_root_pointer({receiver}, {})",
                        self.c_type(destination_type, Some(span))?,
                        self.trap_arguments(span)
                    ));
                }
                if matches!(operation, StandardCall::ForeignRootClose { .. }) {
                    let receiver = arguments
                        .first()
                        .map_or_else(|| "NULL".to_string(), |value| temporary_name(*value));
                    return Some(format!("el_foreign_root_close({receiver})"));
                }
                if matches!(operation, StandardCall::FormatterWrite { .. }) {
                    let receiver = arguments
                        .first()
                        .map_or_else(|| "NULL".to_string(), |value| temporary_name(*value));
                    let text = arguments
                        .get(1)
                        .map_or_else(|| "\"\"".to_string(), |value| temporary_name(*value));
                    return Some(format!(
                        "(el_fmt_append_n(*{receiver}, ({text}).bytes, ({text}).length), \
                         (el_unit){{0}})"
                    ));
                }
                if matches!(operation, StandardCall::IdentityFrom { .. }) {
                    let c_type = self.c_type(destination_type, Some(span))?;
                    let value = arguments
                        .first()
                        .map_or_else(|| "NULL".to_string(), |value| temporary_name(*value));
                    return Some(format!("({c_type}){{ .target = (void *){value} }}"));
                }
                let mut arguments = arguments
                    .iter()
                    .map(|argument| temporary_name(*argument))
                    .collect::<Vec<_>>();
                if matches!(
                    operation,
                    StandardCall::VecInsert { .. } | StandardCall::VecRemove { .. }
                ) {
                    arguments.push(self.trap_arguments(span));
                }
                format!(
                    "{}({})",
                    standard_call_name(*operation),
                    arguments.join(", ")
                )
            }
            Rvalue::CollectionLiteral { kind, elements } => {
                let name = match kind {
                    CollectionLiteralKind::Vec => "vec_literal",
                    CollectionLiteralKind::Map => "map_literal",
                    CollectionLiteralKind::Set => "set_literal",
                };
                format!(
                    "el_{name}_t{}_n{}({})",
                    destination_type.index(),
                    elements.len(),
                    elements
                        .iter()
                        .map(|element| temporary_name(*element))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            Rvalue::CollectionLength { collection, kind } => match kind {
                IterationKind::Array { length, .. } => format!("(uintptr_t){length}U"),
                IterationKind::Vec { .. }
                | IterationKind::Map { .. }
                | IterationKind::Set { .. } => {
                    format!("{}->length", temporary_name(*collection))
                }
            },
            Rvalue::IterationElement {
                collection,
                index,
                kind,
            } => {
                let collection_name = temporary_name(*collection);
                let index_name = temporary_name(*index);
                match kind {
                    IterationKind::Array { element, .. } => format!(
                        "{}({collection_name}.values[{index_name}])",
                        copy_helper_name(self.resolve_alias(*element))
                    ),
                    IterationKind::Vec { element, .. } | IterationKind::Set { element, .. } => {
                        format!(
                            "{}({collection_name}->values[{index_name}])",
                            copy_helper_name(self.resolve_alias(*element))
                        )
                    }
                    IterationKind::Map {
                        key, value, pair, ..
                    } => {
                        let pair_type = self.c_type(*pair, Some(span))?;
                        format!(
                            "({pair_type}){{ .v0 = {}({collection_name}->keys[{index_name}]), \
                             .v1 = {}({collection_name}->values[{index_name}]) }}",
                            copy_helper_name(self.resolve_alias(*key)),
                            copy_helper_name(self.resolve_alias(*value))
                        )
                    }
                }
            }
            Rvalue::FormattedString(parts) => {
                let formatter = format!("el_fmt_{}", destination.index());
                let _ = writeln!(
                    self.output,
                    "    el_formatter {formatter} = {{NULL, 0U, 0U}};"
                );
                for part in parts {
                    match part {
                        RuntimeFormattedPart::Text(text) => {
                            let _ = writeln!(
                                self.output,
                                "    el_fmt_append_n(&{formatter}, {}, (size_t){}U);",
                                c_string(text),
                                text.len()
                            );
                        }
                        RuntimeFormattedPart::Value { value, ty } => {
                            self.emit_formatter_value(
                                &formatter,
                                &temporary_name(*value),
                                *ty,
                                span,
                            );
                        }
                    }
                }
                format!("el_fmt_finish(&{formatter})")
            }
            Rvalue::MakeTraitObject {
                trait_declaration,
                concrete,
                value,
            } => {
                let object = object_name(*trait_declaration);
                let table = vtable_instance_name(self.typed, *trait_declaration, *concrete);
                format!(
                    "({object}){{ (void *){}, &{table} }}",
                    temporary_name(*value)
                )
            }
            Rvalue::DynamicCall {
                receiver,
                trait_declaration,
                slot,
                arguments,
            } => {
                let receiver_name = temporary_name(*receiver);
                let mut call = format!(
                    "{receiver_name}.vtable->{}({receiver_name}.data",
                    vtable_slot_name(*slot)
                );
                for argument in arguments {
                    call.push_str(&format!(", {}", temporary_name(*argument)));
                }
                let _ = trait_declaration;
                call.push(')');
                self.call_rvalue(call, destination_type)
            }
            Rvalue::AllocateManaged { value, value_type } => {
                // A referenced composite literal needs its own managed cell.
                // Allocation is a statement, so it is emitted before the
                // assignment that consumes the resulting address.
                let cell_type = self.c_type(*value_type, Some(span))?;
                let destination_name = temporary_name(destination);
                let class = if self.scanned_allocation(*value_type) {
                    AllocationClass::Scanned
                } else {
                    AllocationClass::PointerFree
                };
                let byte_count = format!("sizeof({cell_type})");
                self.emit_managed_operation(ManagedMemoryOperation::Allocate {
                    destination: &destination_name,
                    byte_count: &byte_count,
                    class,
                });
                let _ = writeln!(
                    self.output,
                    "    if ({destination_name} == NULL) el_out_of_memory();"
                );
                let _ = writeln!(
                    self.output,
                    "    *{destination_name} = {};",
                    temporary_name(*value)
                );
                destination_name
            }
            Rvalue::Copy(source) => format!(
                "{}({})",
                copy_helper_name(self.resolve_alias(destination_type)),
                temporary_name(*source)
            ),
            Rvalue::Discriminant(source) => format!("{}.tag", temporary_name(*source)),
            Rvalue::CompareEqual {
                left,
                right,
                operand_type,
            } => {
                let left = temporary_name(*left);
                let right = temporary_name(*right);
                self.component_equality(*operand_type, &left, &right)
            }
            Rvalue::CompareOrder {
                operator,
                left,
                right,
                operand_type,
            } => {
                let left = temporary_name(*left);
                let right = temporary_name(*right);
                let comparison = self.component_ordering(*operand_type, &left, &right);
                match operator {
                    BinaryOperator::Less => format!("(({comparison}) == -1)"),
                    BinaryOperator::LessEqual => {
                        format!("((({comparison}) == -1) || (({comparison}) == 0))")
                    }
                    BinaryOperator::Greater => format!("(({comparison}) == 1)"),
                    BinaryOperator::GreaterEqual => {
                        format!("((({comparison}) == 0) || (({comparison}) == 1))")
                    }
                    _ => "false".to_string(),
                }
            }
            Rvalue::Unary {
                operator, operand, ..
            } => self.unary_expression(*operator, *operand, destination_type, span)?,
            Rvalue::Binary {
                operator,
                left,
                right,
                ..
            } => self.binary_expression(*operator, *left, *right, destination_type, span)?,
            Rvalue::Cast {
                value, source_type, ..
            } => self.cast_expression(*value, *source_type, destination_type, span)?,
            Rvalue::Call {
                instance,
                arguments,
            } => {
                let call = format!(
                    "{}({})",
                    self.function_symbol(instance),
                    arguments
                        .iter()
                        .map(|argument| temporary_name(*argument))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                self.call_rvalue(call, destination_type)
            }
            Rvalue::IndirectCall { callee, arguments } => {
                let call = format!(
                    "{}({})",
                    temporary_name(*callee),
                    arguments
                        .iter()
                        .map(|argument| temporary_name(*argument))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                self.call_rvalue(call, destination_type)
            }
            Rvalue::VariadicSlice {
                elements,
                element_type,
            } => {
                let slice = self.c_type(destination_type, Some(span))?;
                if elements.is_empty() {
                    format!("({slice}){{NULL, (uintptr_t)0}}")
                } else {
                    let element = self.c_type(*element_type, Some(span))?;
                    let values = elements
                        .iter()
                        .map(|element| temporary_name(*element))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "({slice}){{({element}[]){{{values}}}, (uintptr_t){}}}",
                        elements.len()
                    )
                }
            }
            Rvalue::Aggregate(aggregate) => {
                self.aggregate_expression(aggregate, destination_type, span)?
            }
        })
    }

    fn constant_expression(
        &mut self,
        constant: &crate::ir::Constant,
        ty: TypeId,
        span: Span,
    ) -> Option<String> {
        let c_type = self.c_type(ty, Some(span))?;
        Some(match constant {
            crate::ir::Constant::Unit => "(el_unit){0}".to_string(),
            crate::ir::Constant::Bool(value) => if *value { "true" } else { "false" }.to_string(),
            crate::ir::Constant::Integer {
                magnitude,
                negative,
            } => {
                let primitive = self.typed.types.expanded_primitive(ty)?;
                if matches!(primitive, PrimitiveType::I128 | PrimitiveType::U128) {
                    self.type_error(
                        ty,
                        Some(span),
                        "128-bit integer constants are deferred past the Milestone 8 backend skeleton",
                    );
                    return None;
                }
                let magnitude = *magnitude;
                let sign = if *negative { "-" } else { "" };
                match primitive {
                    PrimitiveType::I64 if *negative && magnitude == (1_u128 << 63) => {
                        "INT64_MIN".to_string()
                    }
                    PrimitiveType::I64 => format!("({c_type})INT64_C({sign}{magnitude})"),
                    PrimitiveType::U64 => format!("({c_type})UINT64_C({magnitude})"),
                    PrimitiveType::Isize if self.options.target == Target::X86_64 => {
                        if *negative && magnitude == (1_u128 << 63) {
                            "INT64_MIN".to_string()
                        } else {
                            format!("({c_type})INT64_C({sign}{magnitude})")
                        }
                    }
                    PrimitiveType::Usize if self.options.target == Target::X86_64 => {
                        format!("({c_type})UINT64_C({magnitude})")
                    }
                    _ => format!("({c_type})({sign}{magnitude})"),
                }
            }
            crate::ir::Constant::Float(source) => {
                let number = strip_numeric_suffix(source).replace('_', "");
                format!("({c_type})({number})")
            }
            crate::ir::Constant::Character(value) => {
                format!("({c_type})UINT32_C({})", u32::from(*value))
            }
            crate::ir::Constant::String(value) => {
                let literal = format!(
                    "(el_str){{ {}, (size_t){}U }}",
                    c_string(value),
                    value.len()
                );
                if self.typed.types.expanded_primitive(ty) == Some(PrimitiveType::String) {
                    format!("el_string_from({literal})")
                } else {
                    literal
                }
            }
            crate::ir::Constant::Null => "NULL".to_string(),
        })
    }

    fn unary_expression(
        &mut self,
        operator: UnaryOperator,
        operand: TemporaryId,
        ty: TypeId,
        span: Span,
    ) -> Option<String> {
        let operand = temporary_name(operand);
        match operator {
            UnaryOperator::Positive => Some(operand),
            UnaryOperator::LogicalNot => Some(format!("(!{operand})")),
            UnaryOperator::BitwiseNot => Some(format!("(~{operand})")),
            UnaryOperator::Negative => {
                if let Some(primitive) = self.typed.types.expanded_primitive(ty)
                    && primitive.is_integer()
                {
                    let helper = integer_helper_name(primitive)?;
                    let arguments = self.trap_arguments(span);
                    return Some(format!("el_neg_{helper}({operand}, {arguments})"));
                }
                Some(format!("(-{operand})"))
            }
        }
    }

    fn binary_expression(
        &mut self,
        operator: BinaryOperator,
        left: TemporaryId,
        right: TemporaryId,
        ty: TypeId,
        span: Span,
    ) -> Option<String> {
        let left = temporary_name(left);
        let right = temporary_name(right);
        if let Some(primitive) = self.typed.types.expanded_primitive(ty)
            && primitive.is_integer()
            && let Some(operation) = checked_binary_name(operator)
        {
            let helper = integer_helper_name(primitive).or_else(|| {
                self.type_error(
                    ty,
                    Some(span),
                    "128-bit arithmetic is deferred past the Milestone 8 backend skeleton",
                );
                None
            })?;
            let arguments = self.trap_arguments(span);
            return Some(format!(
                "el_{operation}_{helper}({left}, {right}, {arguments})"
            ));
        }
        let operator = c_binary_operator(operator)?;
        Some(format!("({left} {operator} {right})"))
    }

    fn cast_expression(
        &mut self,
        value: TemporaryId,
        source: TypeId,
        target: TypeId,
        span: Span,
    ) -> Option<String> {
        let source_kind = self
            .typed
            .types
            .kind(self.typed.types.resolve_inference(source));
        let target_kind = self
            .typed
            .types
            .kind(self.typed.types.resolve_inference(target));
        if matches!(
            (source_kind, target_kind),
            (TypeKind::Reference { .. }, TypeKind::RawPointer { .. })
                | (TypeKind::RawPointer { .. }, TypeKind::Reference { .. })
                | (TypeKind::RawPointer { .. }, TypeKind::RawPointer { .. })
        ) {
            let target_type = self.c_type(target, Some(span))?;
            return Some(format!("({target_type}){}", temporary_name(value)));
        }
        let source_primitive = self.typed.types.expanded_primitive(source)?;
        let target_primitive = self.typed.types.expanded_primitive(target)?;
        let target_type = self.c_type(target, Some(span))?;
        let value = temporary_name(value);
        if target_primitive.is_integer() {
            let (minimum, maximum) = primitive_bounds(target_primitive, self.options.target)?;
            let location = self.trap_arguments(span);
            return Some(format!(
                "el_cast_integer((long double){value}, (long double)({minimum}), \
                 (long double)({maximum}), {location}, ({target_type}){value})"
            ));
        }
        if source_primitive.is_integer() || source_primitive.is_float() {
            return Some(format!("({target_type})({value})"));
        }
        None
    }

    fn aggregate_expression(
        &mut self,
        aggregate: &AggregateValue,
        ty: TypeId,
        span: Span,
    ) -> Option<String> {
        let c_type = self.c_type(ty, Some(span))?;
        Some(match aggregate {
            AggregateValue::Tuple(values) => format!(
                "({c_type}){{ {} }}",
                values
                    .iter()
                    .enumerate()
                    .map(|(index, value)| format!(".v{index} = {}", temporary_name(*value)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            AggregateValue::Array(values) => format!(
                "({c_type}){{ .values = {{ {} }} }}",
                values
                    .iter()
                    .map(|value| temporary_name(*value))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            AggregateValue::Struct { fields, .. } => format!(
                "({c_type}){{ {} }}",
                fields
                    .iter()
                    .map(|(field, value)| {
                        format!(
                            ".{} = {}",
                            self.c_field_name(*field),
                            temporary_name(*value)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            AggregateValue::Enum {
                declaration: _,
                variant,
                fields,
            } => {
                let member = variant_member_name(*variant);
                if fields.is_empty() {
                    let payload = self
                        .enums
                        .get(&ty)
                        .and_then(|enumeration| {
                            enumeration
                                .variants
                                .iter()
                                .find(|variant| !variant.fields.is_empty())
                        })
                        .map_or_else(
                            || ".payload._empty = 0".to_string(),
                            |variant| {
                                format!(".payload.{} = {{0}}", variant_member_name(variant.id))
                            },
                        );
                    format!(
                        "({c_type}){{ .tag = UINT32_C({}), {payload} }}",
                        variant.index(),
                    )
                } else {
                    format!(
                        "({c_type}){{ .tag = UINT32_C({}), .payload.{member} = {{ {} }} }}",
                        variant.index(),
                        fields
                            .iter()
                            .map(|(field, value)| {
                                format!(".{} = {}", field_name(*field), temporary_name(*value))
                            })
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            }
        })
    }

    fn emit_formatter_text(&mut self, formatter: &str, text: &str) {
        let _ = writeln!(
            self.output,
            "    el_fmt_append_n(&{formatter}, {}, (size_t){}U);",
            c_string(text),
            text.len()
        );
    }

    fn emit_formatter_value(&mut self, formatter: &str, value: &str, ty: TypeId, span: Span) {
        let ty = self.resolve_alias(ty);
        if let Some(trait_declaration) = crate::traits::object_trait(self.resolved, self.typed, ty)
            && self
                .resolved
                .is_standard_declaration(trait_declaration, "Display")
        {
            let slot = crate::traits::vtable_slots(self.resolved, trait_declaration)
                .iter()
                .position(|(name, _)| name == "fmt")
                .unwrap_or(0);
            let _ = writeln!(
                self.output,
                "    {{ el_formatter *el_user_formatter = &{formatter}; \
                 (void){value}.vtable->{}({value}.data, &el_user_formatter); }}",
                vtable_slot_name(slot)
            );
            return;
        }
        match self.typed.types.kind(ty).clone() {
            TypeKind::Primitive(primitive) => match primitive {
                PrimitiveType::Unit => self.emit_formatter_text(formatter, "()"),
                PrimitiveType::Bool => {
                    let _ = writeln!(
                        self.output,
                        "    el_fmt_append(&{formatter}, {value} ? \"true\" : \"false\");"
                    );
                }
                PrimitiveType::Char => {
                    let _ = writeln!(self.output, "    el_fmt_char(&{formatter}, {value});");
                }
                PrimitiveType::I8
                | PrimitiveType::I16
                | PrimitiveType::I32
                | PrimitiveType::I64
                | PrimitiveType::Isize => {
                    let _ = writeln!(
                        self.output,
                        "    el_fmt_signed(&{formatter}, (intmax_t){value});"
                    );
                }
                PrimitiveType::U8
                | PrimitiveType::U16
                | PrimitiveType::U32
                | PrimitiveType::U64
                | PrimitiveType::Usize => {
                    let _ = writeln!(
                        self.output,
                        "    el_fmt_unsigned(&{formatter}, (uintmax_t){value});"
                    );
                }
                PrimitiveType::F32 | PrimitiveType::F64 => {
                    let _ = writeln!(
                        self.output,
                        "    el_fmt_float(&{formatter}, (double){value});"
                    );
                }
                PrimitiveType::Str | PrimitiveType::String => {
                    let _ = writeln!(
                        self.output,
                        "    el_fmt_append_n(&{formatter}, ({value}).bytes, ({value}).length);"
                    );
                }
                PrimitiveType::I128 | PrimitiveType::U128 => {
                    self.type_error(ty, Some(span), "128-bit values cannot yet be displayed");
                }
            },
            TypeKind::Reference { target, .. } => {
                self.emit_formatter_value(formatter, &format!("(*{value})"), target, span);
            }
            TypeKind::Tuple(elements) => {
                self.emit_formatter_text(formatter, "(");
                for (index, element) in elements.iter().enumerate() {
                    if index != 0 {
                        self.emit_formatter_text(formatter, ", ");
                    }
                    self.emit_formatter_value(
                        formatter,
                        &format!("{value}.v{index}"),
                        *element,
                        span,
                    );
                }
                if elements.len() == 1 {
                    self.emit_formatter_text(formatter, ",");
                }
                self.emit_formatter_text(formatter, ")");
            }
            TypeKind::Array { element, length } => {
                self.emit_formatter_text(formatter, "[");
                for index in 0..length {
                    if index != 0 {
                        self.emit_formatter_text(formatter, ", ");
                    }
                    self.emit_formatter_value(
                        formatter,
                        &format!("{value}.values[{index}U]"),
                        element,
                        span,
                    );
                }
                self.emit_formatter_text(formatter, "]");
            }
            TypeKind::Builtin { builtin, arguments } => {
                match (self.resolved.builtin_name(builtin), arguments.as_slice()) {
                    ("Vec", [element]) => {
                        let index = format!("el_i_t{}", ty.index());
                        self.emit_formatter_text(formatter, "[");
                        let _ = writeln!(
                            self.output,
                            "    for (uintptr_t {index} = 0U; {index} < {value}->length; ++{index}) {{"
                        );
                        let _ = writeln!(
                            self.output,
                            "        if ({index} != 0U) el_fmt_append(&{formatter}, \", \");"
                        );
                        self.emit_formatter_value(
                            formatter,
                            &format!("{value}->values[{index}]"),
                            *element,
                            span,
                        );
                        self.output.push_str("    }\n");
                        self.emit_formatter_text(formatter, "]");
                    }
                    ("Set", [element]) => {
                        let index = format!("el_i_t{}", ty.index());
                        self.emit_formatter_text(formatter, "{");
                        let _ = writeln!(
                            self.output,
                            "    for (uintptr_t {index} = 0U; {index} < {value}->length; ++{index}) {{"
                        );
                        let _ = writeln!(
                            self.output,
                            "        if ({index} != 0U) el_fmt_append(&{formatter}, \", \");"
                        );
                        self.emit_formatter_value(
                            formatter,
                            &format!("{value}->values[{index}]"),
                            *element,
                            span,
                        );
                        self.output.push_str("    }\n");
                        self.emit_formatter_text(formatter, "}");
                    }
                    ("Map", [key, item]) => {
                        let index = format!("el_i_t{}", ty.index());
                        self.emit_formatter_text(formatter, "{");
                        let _ = writeln!(
                            self.output,
                            "    for (uintptr_t {index} = 0U; {index} < {value}->length; ++{index}) {{"
                        );
                        let _ = writeln!(
                            self.output,
                            "        if ({index} != 0U) el_fmt_append(&{formatter}, \", \");"
                        );
                        self.emit_formatter_value(
                            formatter,
                            &format!("{value}->keys[{index}]"),
                            *key,
                            span,
                        );
                        self.emit_formatter_text(formatter, ": ");
                        self.emit_formatter_value(
                            formatter,
                            &format!("{value}->values[{index}]"),
                            *item,
                            span,
                        );
                        self.output.push_str("    }\n");
                        self.emit_formatter_text(formatter, "}");
                    }
                    _ => self.type_error(
                        ty,
                        Some(span),
                        "this builtin type has no `Display` implementation",
                    ),
                }
            }
            TypeKind::Nominal { .. } => {
                match crate::traits::select_trait_method(self.resolved, self.typed, ty, "fmt", None)
                    .ok()
                    .flatten()
                {
                    Some(selected) => {
                        let instance = FunctionInstance {
                            declaration: selected.declaration,
                            arguments: selected.arguments,
                            self_type: selected.self_type,
                        };
                        let symbol = self.function_symbol(&instance);
                        let _ = writeln!(
                            self.output,
                            "    {{ el_formatter *el_user_formatter = &{formatter}; \
                             (void){symbol}(&({value}), &el_user_formatter); }}"
                        );
                    }
                    None => self.type_error(
                        ty,
                        Some(span),
                        "this nominal type has no `Display.fmt` implementation",
                    ),
                }
            }
            _ => self.type_error(ty, Some(span), "this type cannot be displayed"),
        }
    }

    fn emit_place_checks(&mut self, place: Option<&ControlFlowPlace>, span: Span) {
        let Some(place) = place else { return };
        match place {
            ControlFlowPlace::Local(_) => {}
            ControlFlowPlace::Temporary(_) => {}
            ControlFlowPlace::Field { base, .. }
            | ControlFlowPlace::TupleField { base, .. }
            | ControlFlowPlace::VariantField { base, .. }
            | ControlFlowPlace::Dereference { base } => {
                self.emit_place_checks(Some(base), span);
            }
            ControlFlowPlace::Index {
                base,
                index,
                kind,
                trap,
            } => {
                self.emit_place_checks(Some(base), span);
                let arguments = self.trap_arguments(span);
                match kind {
                    IndexKind::Array { length } => {
                        let _ = writeln!(
                            self.output,
                            "    if ((uintmax_t){} >= UINTMAX_C({length})) \
                             el_trap(\"{}\", {arguments});",
                            temporary_name(*index),
                            trap.code()
                        );
                    }
                    IndexKind::Slice => {
                        let bound = format!("(uintmax_t){}.length", self.place_expression(base));
                        let _ = writeln!(
                            self.output,
                            "    if ((uintmax_t){} >= {bound}) el_trap(\"{}\", {arguments});",
                            temporary_name(*index),
                            trap.code()
                        );
                    }
                    IndexKind::Vec { .. } => {
                        let bound = format!("(uintmax_t){}->length", self.place_expression(base));
                        let _ = writeln!(
                            self.output,
                            "    if ((uintmax_t){} >= {bound}) el_trap(\"{}\", {arguments});",
                            temporary_name(*index),
                            trap.code()
                        );
                    }
                    IndexKind::Map { collection } => {
                        let find = format!("el_map_find_t{}", collection.index());
                        let _ = writeln!(
                            self.output,
                            "    if ({find}({}, {}) < 0) el_trap(\"{}\", {arguments});",
                            self.place_expression(base),
                            temporary_name(*index),
                            trap.code()
                        );
                    }
                }
            }
        }
    }

    fn place_expression(&self, place: &ControlFlowPlace) -> String {
        match place {
            ControlFlowPlace::Local(binding) => {
                if self.promoted.contains(binding) {
                    format!("(*{})", cell_name(*binding))
                } else {
                    local_name(*binding)
                }
            }
            ControlFlowPlace::Temporary(temporary) => temporary_name(*temporary),
            ControlFlowPlace::Field { base, field } => {
                format!(
                    "{}.{}",
                    self.place_expression(base),
                    self.c_field_name(*field)
                )
            }
            ControlFlowPlace::TupleField { base, index } => {
                format!("{}.v{index}", self.place_expression(base))
            }
            ControlFlowPlace::VariantField {
                base,
                variant,
                field,
            } => format!(
                "{}.payload.{}.{}",
                self.place_expression(base),
                variant_member_name(*variant),
                field_name(*field)
            ),
            ControlFlowPlace::Dereference { base } => {
                format!("(*{})", self.place_expression(base))
            }
            ControlFlowPlace::Index {
                base, index, kind, ..
            } => match kind {
                IndexKind::Array { .. } | IndexKind::Slice => format!(
                    "{}.values[{}]",
                    self.place_expression(base),
                    temporary_name(*index)
                ),
                IndexKind::Vec { .. } => format!(
                    "{}->values[{}]",
                    self.place_expression(base),
                    temporary_name(*index)
                ),
                IndexKind::Map { collection } => format!(
                    "{}->values[(uintptr_t)el_map_find_t{}({}, {})]",
                    self.place_expression(base),
                    collection.index(),
                    self.place_expression(base),
                    temporary_name(*index)
                ),
            },
        }
    }

    fn emit_print(&mut self, value: TemporaryId, ty: TypeId, span: Span) {
        let value_id = value;
        let value = temporary_name(value);
        match self.typed.types.expanded_primitive(ty) {
            Some(PrimitiveType::Unit) => {
                self.output.push_str("    fputs(\"()\", stdout);\n");
            }
            Some(PrimitiveType::Bool) => {
                let _ = writeln!(
                    self.output,
                    "    fputs({value} ? \"true\" : \"false\", stdout);"
                );
            }
            Some(PrimitiveType::Char) => {
                let _ = writeln!(self.output, "    el_print_char({value});");
            }
            Some(
                PrimitiveType::I8
                | PrimitiveType::I16
                | PrimitiveType::I32
                | PrimitiveType::I64
                | PrimitiveType::Isize,
            ) => {
                let _ = writeln!(
                    self.output,
                    "    fprintf(stdout, \"%\" PRIdMAX, (intmax_t){value});"
                );
            }
            Some(
                PrimitiveType::U8
                | PrimitiveType::U16
                | PrimitiveType::U32
                | PrimitiveType::U64
                | PrimitiveType::Usize,
            ) => {
                let _ = writeln!(
                    self.output,
                    "    fprintf(stdout, \"%\" PRIuMAX, (uintmax_t){value});"
                );
            }
            Some(PrimitiveType::F32) => {
                let _ = writeln!(
                    self.output,
                    "    fprintf(stdout, \"%.9g\", (double){value});"
                );
            }
            Some(PrimitiveType::F64) => {
                let _ = writeln!(self.output, "    fprintf(stdout, \"%.17g\", {value});");
            }
            Some(PrimitiveType::Str | PrimitiveType::String) => {
                let _ = writeln!(
                    self.output,
                    "    fwrite(({value}).bytes, 1U, ({value}).length, stdout);"
                );
            }
            _ => {
                let formatter = format!("el_print_fmt_{}", value_id.index());
                let _ = writeln!(
                    self.output,
                    "    el_formatter {formatter} = {{NULL, 0U, 0U}};"
                );
                self.emit_formatter_value(&formatter, &value, ty, span);
                let rendered = format!("el_print_text_{}", value_id.index());
                let _ = writeln!(
                    self.output,
                    "    el_str {rendered} = el_fmt_finish(&{formatter});\n    \
                     fwrite({rendered}.bytes, 1U, {rendered}.length, stdout);"
                );
            }
        }
    }

    fn emit_terminator(&mut self, function: &ControlFlowFunction, terminator: &Terminator) {
        match terminator {
            Terminator::Goto(block) => {
                let _ = writeln!(self.output, "    goto b{};", block.index());
            }
            Terminator::Branch {
                condition,
                then_block,
                else_block,
            } => {
                let _ = writeln!(
                    self.output,
                    "    if ({}) goto b{}; else goto b{};",
                    temporary_name(*condition),
                    then_block.index(),
                    else_block.index()
                );
            }
            Terminator::Return(Some(value)) => {
                if matches!(
                    self.typed.types.expanded_primitive(function.return_type),
                    Some(PrimitiveType::Unit)
                ) {
                    let _ = writeln!(
                        self.output,
                        "    (void){};\n    return;",
                        temporary_name(*value)
                    );
                } else {
                    let _ = writeln!(self.output, "    return {};", temporary_name(*value));
                }
            }
            Terminator::Return(None) => {
                if matches!(
                    self.typed.types.expanded_primitive(function.return_type),
                    Some(PrimitiveType::Unit)
                ) {
                    self.output.push_str("    return;\n");
                } else {
                    self.output.push_str("    abort();\n");
                }
            }
            Terminator::Trap { kind, span } => {
                let arguments = self.trap_arguments(*span);
                let _ = writeln!(
                    self.output,
                    "    el_trap(\"{}\", {arguments});",
                    kind.code()
                );
            }
            Terminator::Unreachable => self.output.push_str("    abort();\n"),
        }
    }

    fn emit_entry(&mut self) {
        let Some(entry) = self.options.entry else {
            return;
        };
        let Some(function) = self
            .program
            .functions
            .iter()
            .find(|function| function.declaration == entry)
        else {
            self.diagnostics.push(Diagnostic::new(
                Category::CodeGeneration,
                "the executable entry function was not selected for Milestone 8 lowering",
            ));
            return;
        };
        let unit = matches!(
            self.typed.types.expanded_primitive(function.return_type),
            Some(PrimitiveType::Unit)
        );
        let result_unit = match self
            .typed
            .types
            .kind(self.typed.types.resolve_inference(function.return_type))
        {
            TypeKind::Nominal {
                identity,
                arguments,
            } if self
                .resolved
                .is_standard_declaration(identity.declaration, "Result") =>
            {
                matches!(
                    arguments.as_slice(),
                    [ok, _]
                        if matches!(
                            self.typed.types.expanded_primitive(*ok),
                            Some(PrimitiveType::Unit)
                        )
                )
            }
            _ => false,
        };
        if !function.parameters.is_empty() || (!unit && !result_unit) {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::CodeGeneration,
                    "an executable entry must have signature `fn main() -> ()` or \
                     `fn main() -> Result[(), E]`",
                )
                .with_primary(function.span),
            );
            return;
        }
        let symbol = self.function_symbol(&FunctionInstance {
            declaration: entry,
            arguments: Vec::new(),
            self_type: None,
        });
        // The collector is initialized before any Elamite code runs. A library
        // package emits no shim, so initialization is the linking
        // executable's responsibility.
        self.output.push_str("int main(void) {\n");
        if self.program.requires_managed_memory {
            self.emit_managed_operation(ManagedMemoryOperation::Initialize);
        }
        let _ = writeln!(self.output, "    (void){symbol}();\n    return 0;\n}}");
    }

    fn emit_default_helper(&mut self, ty: TypeId, span: Option<Span>) {
        let ty = self.resolve_alias(ty);
        if self.emitted_default_helpers.contains(&ty) || !self.emitting_default_helpers.insert(ty) {
            return;
        }
        let kind = self.typed.types.kind(ty).clone();
        let components = match &kind {
            TypeKind::Tuple(elements) => elements.clone(),
            TypeKind::Array { element, .. } => vec![*element],
            TypeKind::Nominal { .. } => self
                .structs
                .get(&ty)
                .map(|structure| {
                    structure
                        .fields
                        .iter()
                        .map(|(_, _, field_type)| *field_type)
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        for component in components {
            if self.needs_default_helper(component) {
                self.emit_default_helper(component, span);
            }
        }
        let Some(c_type) = self.c_type(ty, span) else {
            self.emitting_default_helpers.remove(&ty);
            return;
        };
        let name = default_helper_name(ty);
        let mut body = format!("    {c_type} value = {{0}};\n");
        match kind {
            TypeKind::Tuple(elements) => {
                for (index, element) in elements.into_iter().enumerate() {
                    if let Some(value) = self.component_default(element, span) {
                        let _ = writeln!(body, "    value.v{index} = {value};");
                    }
                }
            }
            TypeKind::Array { element, length } => {
                if let Some(value) = self.component_default(element, span) {
                    let _ = writeln!(
                        body,
                        "    for (uintptr_t i = 0; i < {length}u; ++i) value.values[i] = {value};"
                    );
                }
            }
            TypeKind::Nominal { identity, .. } => {
                if let Some(structure) = self.structs.get(&ty).copied() {
                    let fields = structure.fields.clone();
                    for (field, _, field_type) in fields {
                        if let Some(value) = self.component_default(field_type, span) {
                            let _ = writeln!(body, "    value.{} = {value};", field_name(field));
                        }
                    }
                } else if let Some(variant) =
                    crate::traits::intrinsic_default_variant(self.resolved, identity.declaration)
                {
                    // The discriminant is the variant's own identity, not its
                    // ordinal, so a zero-initialized value is not the default
                    // variant and the tag must be written explicitly.
                    let _ = writeln!(body, "    value.tag = UINT32_C({});", variant.index());
                }
            }
            _ => {}
        }
        body.push_str("    return value;\n");
        let _ = writeln!(self.output, "static {c_type} {name}(void) {{\n{body}}}\n");
        self.emitting_default_helpers.remove(&ty);
        self.emitted_default_helpers.insert(ty);
    }

    fn needs_default_helper(&self, ty: TypeId) -> bool {
        let resolved = self.resolve_alias(ty);
        matches!(
            self.typed.types.kind(resolved),
            TypeKind::Tuple(_) | TypeKind::Array { .. }
        ) || match self.typed.types.kind(resolved) {
            TypeKind::Nominal { identity, .. } => {
                (self.structs.contains_key(&resolved)
                    && crate::traits::derives(self.resolved, identity.declaration, "Default"))
                    || (self.enums.contains_key(&resolved)
                        && crate::traits::intrinsic_derivation(
                            self.resolved,
                            identity.declaration,
                            "Default",
                        ))
            }
            _ => false,
        }
    }

    fn component_default(&mut self, ty: TypeId, span: Option<Span>) -> Option<String> {
        let resolved = self.resolve_alias(ty);
        if self.needs_default_helper(resolved) {
            return Some(format!("{}()", default_helper_name(resolved)));
        }
        match self.typed.types.kind(resolved).clone() {
            TypeKind::Primitive(PrimitiveType::Str) => {
                Some("(el_str){\"\", (size_t)0U}".to_string())
            }
            TypeKind::Primitive(PrimitiveType::String) => {
                Some("el_string_from((el_str){\"\", (size_t)0U})".to_string())
            }
            TypeKind::Builtin { builtin, arguments }
                if matches!(
                    (self.resolved.builtin_name(builtin), arguments.len()),
                    ("Vec", 1) | ("Map", 2) | ("Set", 1)
                ) =>
            {
                let operation = match self.resolved.builtin_name(builtin) {
                    "Vec" => StandardCall::VecNew {
                        collection: resolved,
                    },
                    "Map" => StandardCall::MapNew {
                        collection: resolved,
                    },
                    _ => StandardCall::SetNew {
                        collection: resolved,
                    },
                };
                Some(format!("{}()", standard_call_name(operation)))
            }
            TypeKind::Reference { .. } | TypeKind::Function { .. } => {
                self.type_error(
                    ty,
                    span,
                    "a safe reference or function reference has no default",
                );
                None
            }
            TypeKind::Nominal { .. } => {
                let selected = crate::traits::select_trait_method(
                    self.resolved,
                    self.typed,
                    resolved,
                    "default",
                    None,
                )
                .ok()
                .flatten()?;
                let instance = FunctionInstance {
                    declaration: selected.declaration,
                    arguments: selected.arguments,
                    self_type: selected.self_type,
                };
                Some(format!("{}()", self.function_symbol(&instance)))
            }
            _ => {
                let c_type = self.c_type(resolved, span)?;
                let zero = zero_value(resolved, &self.typed.types);
                if zero.starts_with('{') {
                    Some(format!("({c_type}){zero}"))
                } else {
                    Some(zero)
                }
            }
        }
    }

    /// The structural default of `ty` (`SPEC.md` 4.3): zero for numerics,
    /// `false` for `bool`, U+0000 for `char`, empty text for `str`/`String`,
    /// `null` for raw pointers, and fieldwise defaults for aggregates.
    fn default_expression(&mut self, ty: TypeId, span: Span) -> Option<String> {
        self.component_default(ty, Some(span))
    }

    /// Whether an allocation of `ty` may contain managed pointers and must
    /// therefore be scanned by the collector. Conservative: only types proven
    /// free of references, pointers, and owned buffers are left unscanned.
    fn scanned_allocation(&self, ty: TypeId) -> bool {
        fn walk(types: &TypeContext, ty: TypeId, depth: u32) -> bool {
            if depth == 0 {
                return true;
            }
            match types.kind(types.resolve_inference(ty)) {
                TypeKind::Primitive(primitive) => {
                    // A `String` owns a heap buffer; every other primitive is
                    // a plain scalar.
                    matches!(primitive, PrimitiveType::String)
                }
                TypeKind::Reference { .. }
                | TypeKind::RawPointer { .. }
                | TypeKind::Function { .. }
                | TypeKind::TraitObject { .. }
                | TypeKind::Builtin { .. }
                | TypeKind::Foreign { .. }
                | TypeKind::GenericParameter(_)
                | TypeKind::Error => true,
                TypeKind::Alias { target, .. } => walk(types, *target, depth - 1),
                TypeKind::Array { element, .. } => walk(types, *element, depth - 1),
                TypeKind::Slice(element) => walk(types, *element, depth - 1),
                TypeKind::Tuple(elements) => elements
                    .iter()
                    .any(|element| walk(types, *element, depth - 1)),
                TypeKind::Nominal { .. }
                | TypeKind::SelfType(_)
                | TypeKind::InferenceVariable(_) => true,
            }
        }
        walk(&self.typed.types, ty, 16)
    }

    fn c_type(&mut self, ty: TypeId, span: Option<Span>) -> Option<String> {
        let ty = self.resolve_alias(ty);
        Some(match self.typed.types.kind(ty) {
            TypeKind::Primitive(primitive) => match primitive {
                PrimitiveType::Unit => "el_unit",
                PrimitiveType::Bool => "bool",
                PrimitiveType::Char => "uint32_t",
                PrimitiveType::I8 => "int8_t",
                PrimitiveType::I16 => "int16_t",
                PrimitiveType::I32 => "int32_t",
                PrimitiveType::I64 => "int64_t",
                PrimitiveType::I128 => "el_i128",
                PrimitiveType::Isize => "intptr_t",
                PrimitiveType::U8 => "uint8_t",
                PrimitiveType::U16 => "uint16_t",
                PrimitiveType::U32 => "uint32_t",
                PrimitiveType::U64 => "uint64_t",
                PrimitiveType::U128 => "el_u128",
                PrimitiveType::Usize => "uintptr_t",
                PrimitiveType::F32 => "float",
                PrimitiveType::F64 => "double",
                PrimitiveType::Str => "el_str",
                PrimitiveType::String => "el_string",
            }
            .to_string(),
            TypeKind::Tuple(_) => tuple_name(ty),
            TypeKind::Array { .. } => array_name(ty),
            TypeKind::Slice(_) => slice_name(ty),
            TypeKind::Nominal { .. } if self.structs.contains_key(&ty) => {
                struct_name(self.structs[&ty].declaration, ty)
            }
            TypeKind::Nominal { .. } if self.enums.contains_key(&ty) => {
                enum_name(self.enums[&ty].declaration, ty)
            }
            TypeKind::Foreign { identity, .. } => self.resolved.declarations
                [identity.declaration.index()]
            .foreign_binding
            .as_ref()
            .map(|binding| binding.c_name.clone())
            .unwrap_or_else(|| {
                self.type_error(ty, span, "a foreign type is missing `@importc` metadata");
                "void".to_string()
            }),
            // `&T`, `&var T`, `*T`, and `*var T` are all `T *`; mutability is
            // compile-time only (LEDGER 19).
            TypeKind::Reference { target, .. }
                if matches!(self.typed.types.kind(*target), TypeKind::Function { .. }) =>
            {
                function_type_name(*target)
            }
            TypeKind::RawPointer { target, .. }
                if matches!(self.typed.types.kind(*target), TypeKind::Function { .. }) =>
            {
                function_type_name(*target)
            }
            // A trait object is a fat reference: target plus vtable
            // (`SPEC.md` 6). It is the one reference whose C type is not `T *`.
            TypeKind::Reference { target, .. }
                if matches!(
                    self.typed.types.kind(self.resolve_alias(*target)),
                    TypeKind::TraitObject { .. }
                ) =>
            {
                let TypeKind::TraitObject { trait_type } =
                    self.typed.types.kind(self.resolve_alias(*target)).clone()
                else {
                    return None;
                };
                let Some(trait_declaration) =
                    crate::traits::object_trait_of_nominal(self.resolved, self.typed, trait_type)
                else {
                    self.type_error(ty, span, "this trait object has no trait declaration");
                    return None;
                };
                object_name(trait_declaration)
            }
            TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => {
                format!("{} *", self.c_type(*target, span)?)
            }
            TypeKind::Function { .. } => function_type_name(ty),
            TypeKind::Builtin { builtin, arguments }
                if self.resolved.builtin_name(*builtin) == "CVoid" && arguments.is_empty() =>
            {
                "void".to_string()
            }
            TypeKind::Builtin { builtin, arguments }
                if matches!(
                    (self.resolved.builtin_name(*builtin), arguments.len()),
                    ("ForeignRoot" | "ForeignRootMut", 1)
                ) =>
            {
                "el_foreign_root_state *".to_string()
            }
            TypeKind::Builtin { builtin, arguments }
                if self.resolved.builtin_name(*builtin) == "Formatter" && arguments.is_empty() =>
            {
                "el_formatter *".to_string()
            }
            TypeKind::Builtin { builtin, arguments }
                if matches!(
                    (self.resolved.builtin_name(*builtin), arguments.len()),
                    ("Vec" | "Set" | "Identity", 1) | ("Map", 2) | ("Formatter", 0)
                ) =>
            {
                collection_type_name(ty)
            }
            TypeKind::Error => {
                self.type_error(ty, span, "the explicit error type reached C generation");
                return None;
            }
            _ => {
                self.type_error(
                    ty,
                    span,
                    "this type has no representation in the Milestone 8 C backend",
                );
                return None;
            }
        })
    }

    fn c_function_return_type(&mut self, ty: TypeId, span: Option<Span>) -> Option<String> {
        if matches!(
            self.typed.types.expanded_primitive(ty),
            Some(PrimitiveType::Unit)
        ) {
            Some("void".to_string())
        } else {
            self.c_type(ty, span)
        }
    }

    fn call_rvalue(&self, call: String, destination_type: TypeId) -> String {
        if matches!(
            self.typed.types.expanded_primitive(destination_type),
            Some(PrimitiveType::Unit)
        ) {
            format!("({call}, (el_unit){{0}})")
        } else {
            call
        }
    }

    fn resolve_alias(&self, mut ty: TypeId) -> TypeId {
        ty = self.typed.types.resolve_inference(ty);
        loop {
            match self.typed.types.kind(ty) {
                TypeKind::Alias { target, .. } => {
                    ty = self.typed.types.resolve_inference(*target);
                }
                _ => return ty,
            }
        }
    }

    fn type_error(&mut self, ty: TypeId, span: Option<Span>, message: &str) {
        let mut diagnostic = Diagnostic::new(
            Category::CodeGeneration,
            format!(
                "{message} (canonical type {}: {:?})",
                ty.index(),
                self.typed.types.kind(ty)
            ),
        );
        if let Some(span) = span {
            diagnostic = diagnostic.with_primary(span);
        }
        self.diagnostics.push(diagnostic);
    }

    fn trap_arguments(&self, span: Span) -> String {
        let location = self.location(span);
        format!(
            "{}, UINT32_C({}), UINT32_C({})",
            c_string(&location.path),
            location.line,
            location.column
        )
    }

    fn location(&self, span: Span) -> SourceLocation {
        let position = self.sources.line_col(span.file, span.start);
        SourceLocation {
            path: self.sources.path(span.file).display().to_string(),
            line: position.line,
            column: position.column,
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

fn struct_name(declaration: DeclarationId, ty: TypeId) -> String {
    format!("el_struct_d{}_t{}", declaration.index(), ty.index())
}

fn enum_name(declaration: DeclarationId, ty: TypeId) -> String {
    format!("el_enum_d{}_t{}", declaration.index(), ty.index())
}

fn slice_name(ty: TypeId) -> String {
    format!("el_slice_t{}", ty.index())
}

fn function_type_name(ty: TypeId) -> String {
    format!("el_fn_t{}", ty.index())
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

fn numeric_alternative_name(
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
            Terminator::Return(_) | Terminator::Trap { .. } | Terminator::Unreachable => {}
        }
    }
    reachable
}

fn pointer_check_name(pointee: TypeId) -> String {
    format!("el_check_ptr_t{}", pointee.index())
}

fn numeric_conversion_name(outcome: NumericOutcome, source: TypeId, result: TypeId) -> String {
    let stem = match outcome {
        NumericOutcome::Checked => "try_from",
        NumericOutcome::Wrapping => "wrapping_from",
        NumericOutcome::Saturating => "saturating_from",
    };
    format!("el_{stem}_t{}_t{}", source.index(), result.index())
}

fn standard_call_name(operation: StandardCall) -> String {
    use StandardCall::{
        ArrayGet, ArrayLen, MapClear, MapContainsKey, MapGet, MapInsert, MapIsEmpty, MapLen,
        MapNew, MapRemove, SetClear, SetContains, SetInsert, SetIsEmpty, SetLen, SetNew, SetRemove,
        StringFrom, VecAppend, VecClear, VecGet, VecInsert, VecIsEmpty, VecLen, VecNew, VecRemove,
    };
    let (name, ty) = match operation {
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

fn standard_collection_type(operation: StandardCall) -> Option<TypeId> {
    use StandardCall::{
        ArrayGet, ArrayLen, MapClear, MapContainsKey, MapGet, MapInsert, MapIsEmpty, MapLen,
        MapNew, MapRemove, SetClear, SetContains, SetInsert, SetIsEmpty, SetLen, SetNew, SetRemove,
        StringFrom, VecAppend, VecClear, VecGet, VecInsert, VecIsEmpty, VecLen, VecNew, VecRemove,
    };
    Some(match operation {
        StringFrom
        | StandardCall::IdentityFrom { .. }
        | StandardCall::ForeignRootRetain { .. }
        | StandardCall::ForeignRootPointer { .. }
        | StandardCall::ForeignRootClose { .. }
        | StandardCall::FormatterWrite { .. } => {
            return None;
        }
        ArrayLen { collection }
        | ArrayGet { collection }
        | VecNew { collection }
        | VecLen { collection }
        | VecIsEmpty { collection }
        | VecGet { collection }
        | VecAppend { collection }
        | VecInsert { collection }
        | VecRemove { collection }
        | VecClear { collection }
        | MapNew { collection }
        | MapLen { collection }
        | MapIsEmpty { collection }
        | MapContainsKey { collection }
        | MapGet { collection }
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

fn variant_member_name(variant: VariantId) -> String {
    format!("v{}", variant.index())
}

fn tuple_name(ty: TypeId) -> String {
    format!("el_tuple_t{}", ty.index())
}

fn array_name(ty: TypeId) -> String {
    format!("el_array_t{}", ty.index())
}

fn collection_type_name(ty: TypeId) -> String {
    format!("el_runtime_t{}", ty.index())
}

fn field_name(field: crate::resolution::FieldId) -> String {
    format!("f{}", field.index())
}

fn copy_helper_name(ty: TypeId) -> String {
    format!("el_copy_t{}", ty.index())
}

fn local_name(binding: crate::resolution::LocalBindingId) -> String {
    format!("l{}", binding.index())
}

/// The managed cell backing a promoted local. Distinct from [`local_name`] so
/// a promoted parameter keeps its incoming C parameter alongside its cell.
fn cell_name(binding: crate::resolution::LocalBindingId) -> String {
    format!("c{}", binding.index())
}

/// The C struct representing a trait object of one trait.
fn equality_helper_name(ty: TypeId) -> String {
    format!("el_eq_t{}", ty.index())
}

fn ordering_helper_name(ty: TypeId) -> String {
    format!("el_ord_t{}", ty.index())
}

fn hash_helper_name(ty: TypeId) -> String {
    format!("el_hash_t{}", ty.index())
}

fn default_helper_name(ty: TypeId) -> String {
    format!("el_default_t{}", ty.index())
}

fn object_name(trait_declaration: DeclarationId) -> String {
    format!("el_obj{}", trait_declaration.index())
}

/// The C struct holding one trait's method pointers.
fn vtable_type_name(trait_declaration: DeclarationId) -> String {
    format!("el_vt{}", trait_declaration.index())
}

fn vtable_slot_name(slot: usize) -> String {
    format!("m{slot}")
}

/// The static vtable for one (trait, implementing type) pair. The type is
/// identified by its canonical id, which is stable for a given program.
fn vtable_instance_name(
    typed: &TypedProgram,
    trait_declaration: DeclarationId,
    concrete: TypeId,
) -> String {
    let _ = typed;
    format!("el_vtbl{}_{}", trait_declaration.index(), concrete.index())
}

/// The `void *`-receiver thunk that adapts a concrete method to a vtable slot.
fn thunk_name(trait_declaration: DeclarationId, concrete: TypeId, slot: usize) -> String {
    format!(
        "el_thunk{}_{}_{slot}",
        trait_declaration.index(),
        concrete.index()
    )
}

fn temporary_name(temporary: TemporaryId) -> String {
    format!("t{}", temporary.index())
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn c_string(value: &str) -> String {
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

fn c_comment(value: &str) -> String {
    value.replace("*/", "* /").replace('\n', " ")
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
        BinaryOperator::Add => "+",
        BinaryOperator::Subtract => "-",
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

fn zero_value(ty: TypeId, types: &TypeContext) -> String {
    let mut ty = ty;
    loop {
        match types.kind(ty) {
            TypeKind::Alias { target, .. } => ty = *target,
            TypeKind::Primitive(PrimitiveType::Unit)
            | TypeKind::Tuple(_)
            | TypeKind::Array { .. }
            | TypeKind::Slice(_)
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
            _ => return "0".to_string(),
        }
    }
}
