//! Runtime prelude, helpers, managed-memory operations, and value operations.

use super::*;
use crate::ir::control_flow::ValueModel;
use crate::operations::{SystemOperation, TextOperation};

fn collect_ownership_drop_types(action: &crate::ir::DropAction, types: &mut BTreeSet<TypeId>) {
    use crate::ir::DropActionKind;
    match &action.kind {
        DropActionKind::OwnedShared { owner }
        | DropActionKind::OwnedWeak { owner }
        | DropActionKind::OwnedStore { owner } => {
            types.insert(*owner);
        }
        DropActionKind::OwnedVec { elements } | DropActionKind::OwnedSet { elements } => {
            for child in elements {
                collect_ownership_drop_types(child, types);
            }
        }
        DropActionKind::OwnedMap { keys, values } => {
            for child in keys.iter().chain(values) {
                collect_ownership_drop_types(child, types);
            }
        }
        DropActionKind::OwnedBox { value } => {
            for child in value {
                collect_ownership_drop_types(child, types);
            }
        }
        DropActionKind::StructuralLeaf
        | DropActionKind::Custom(_)
        | DropActionKind::OwnedString => {}
    }
}

impl<'a> CEmitter<'a> {
    pub(super) fn uses_process_arguments(&self) -> bool {
        self.program.functions.iter().any(|function| {
            function.blocks.iter().any(|block| {
                block.instructions.iter().any(|instruction| {
                    matches!(
                        instruction,
                        Instruction::Assign {
                            value: Rvalue::StandardCall {
                                operation: StandardCall::System {
                                    operation: SystemOperation::Args,
                                    ..
                                },
                                ..
                            },
                            ..
                        }
                    )
                })
            })
        })
    }
    pub(super) fn uses_synchronized_output(&self) -> bool {
        self.uses_native_threads()
            && self.program.functions.iter().any(|function| {
                function.blocks.iter().any(|block| {
                    block
                        .instructions
                        .iter()
                        .any(|instruction| matches!(instruction, Instruction::PrintValue { .. }))
                })
            })
    }

    pub(super) fn uses_thread_lifecycle(&self) -> bool {
        self.program.functions.iter().any(|function| {
            function.blocks.iter().any(|block| {
                block.instructions.iter().any(|instruction| {
                    matches!(
                        instruction,
                        Instruction::Assign {
                            value: Rvalue::StandardCall {
                                operation: StandardCall::ThreadSpawn { .. }
                                    | StandardCall::ThreadJoin { .. }
                                    | StandardCall::ThreadIsFinished { .. },
                                ..
                            },
                            ..
                        }
                    )
                }) || matches!(
                    block.terminator,
                    Terminator::NeverCall {
                        call: NeverCall::Standard {
                            operation: StandardCall::ThreadJoin { .. },
                            ..
                        },
                        ..
                    }
                )
            })
        })
    }

    pub(super) fn uses_native_threads(&self) -> bool {
        self.program.functions.iter().any(|function| {
            function.blocks.iter().any(|block| {
                block.instructions.iter().any(|instruction| {
                    matches!(
                        instruction,
                        Instruction::Assign {
                            value: Rvalue::StandardCall {
                                operation: StandardCall::ThreadSpawn { .. }
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
                                    | StandardCall::SharedNew { .. }
                                    | StandardCall::SharedGet { .. }
                                    | StandardCall::SharedDowngrade { .. }
                                    | StandardCall::WeakUpgrade { .. },
                                ..
                            },
                            ..
                        }
                    )
                }) || matches!(
                    block.terminator,
                    Terminator::NeverCall {
                        call: NeverCall::Standard {
                            operation: StandardCall::ThreadJoin { .. },
                            ..
                        },
                        ..
                    }
                )
            })
        })
    }

    pub(super) fn emit_foreign_root_runtime(&mut self, used_types: &BTreeSet<TypeId>) {
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
    pub(super) fn emit_object_types(&mut self) {
        let vtables = self.program.vtables.clone();
        if vtables.is_empty() {
            return;
        }
        let mut traits: BTreeMap<(DeclarationId, TypeId), Vec<&crate::ir::Vtable>> =
            BTreeMap::new();
        for vtable in &vtables {
            traits
                .entry((vtable.trait_declaration, vtable.trait_type))
                .or_default()
                .push(vtable);
        }
        for ((trait_declaration, trait_type), tables) in traits {
            let Some(table) = tables.first() else {
                continue;
            };
            for signature in &table.signatures {
                self.emit_type_definition(signature.return_type, None);
                for parameter in &signature.parameters {
                    self.emit_type_definition(parameter.ty, None);
                }
            }
            let vtable_type = vtable_type_name(trait_declaration, trait_type);
            let object = object_name(trait_declaration, trait_type);
            // The method-pointer struct and the fat-reference struct.
            let _ = writeln!(self.output, "typedef struct {vtable_type} {{");
            for (slot, signature) in table.signatures.iter().enumerate() {
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
    pub(super) fn emit_vtable_tables(&mut self) {
        let vtables = self.program.vtables.clone();
        if vtables.is_empty() {
            return;
        }
        let mut traits: BTreeMap<(DeclarationId, TypeId), Vec<&crate::ir::Vtable>> =
            BTreeMap::new();
        for vtable in &vtables {
            traits
                .entry((vtable.trait_declaration, vtable.trait_type))
                .or_default()
                .push(vtable);
        }
        for ((trait_declaration, trait_type), tables) in traits {
            let vtable_type = vtable_type_name(trait_declaration, trait_type);
            let _ = &tables;
            for table in tables {
                let Some(concrete_type) = self.c_type(table.concrete, None) else {
                    continue;
                };
                for (slot, method) in table.methods.iter().enumerate() {
                    let thunk = thunk_name(trait_declaration, trait_type, table.concrete, slot);
                    let Some(slot_signature) = table.signatures.get(slot).cloned() else {
                        continue;
                    };
                    let Some(return_type) =
                        self.c_function_return_type(slot_signature.return_type, None)
                    else {
                        continue;
                    };
                    let mut parameters = vec!["void *el_self".to_string()];
                    for (index, parameter) in slot_signature.parameters.iter().enumerate() {
                        let Some(ty) = self.c_type(parameter.ty, None) else {
                            return;
                        };
                        parameters.push(format!("{ty} a{index}"));
                    }
                    let call = match method {
                        VtableMethod::Function(instance) => {
                            let mut arguments = vec![format!("({concrete_type} *)el_self")];
                            arguments.extend(
                                slot_signature
                                    .parameters
                                    .iter()
                                    .enumerate()
                                    .map(|(index, _)| format!("a{index}")),
                            );
                            format!(
                                "{}({})",
                                self.function_symbol(instance),
                                arguments.join(", ")
                            )
                        }
                        VtableMethod::Closure(instance) => {
                            let tuple_arguments =
                                callable_tuple_arguments(&self.typed.types, &slot_signature);
                            let capture_free = matches!(
                                self.typed.types.kind(self.resolve_alias(table.concrete)),
                                TypeKind::Closure { captures, .. } if captures.is_empty()
                            );
                            let mut arguments = if self.program.value_model == ValueModel::Owned {
                                if capture_free {
                                    Vec::new()
                                } else {
                                    vec![format!("(({concrete_type} *)el_self)")]
                                }
                            } else {
                                vec![format!("*(({concrete_type} *)el_self)")]
                            };
                            arguments.extend(tuple_arguments);
                            format!(
                                "{}({})",
                                self.function_symbol(instance),
                                arguments.join(", ")
                            )
                        }
                        VtableMethod::FunctionReference => {
                            let arguments =
                                callable_tuple_arguments(&self.typed.types, &slot_signature);
                            format!("(*(({concrete_type} *)el_self))({})", arguments.join(", "))
                        }
                    };
                    let parameter_uses = slot_signature
                        .parameters
                        .iter()
                        .enumerate()
                        .map(|(index, _)| format!("    (void)a{index};\n"))
                        .collect::<String>();
                    if return_type == "void" {
                        let _ = writeln!(
                            self.output,
                            "static void {thunk}({}) {{\n{parameter_uses}    {call};\n}}\n",
                            parameters.join(", "),
                        );
                    } else {
                        let _ = writeln!(
                            self.output,
                            "static {return_type} {thunk}({}) {{\n{parameter_uses}    return {call};\n}}\n",
                            parameters.join(", "),
                        );
                    }
                }
                let entries = (0..table.methods.len())
                    .map(|slot| thunk_name(trait_declaration, trait_type, table.concrete, slot))
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(
                    self.output,
                    "static const {vtable_type} {} = {{ {entries} }};\n",
                    vtable_instance_name(self.typed, trait_declaration, trait_type, table.concrete,)
                );
            }
        }
    }

    /// Emits the collector's declarations behind the `ManagedMemoryStrategy`
    /// boundary. A program with no managed storage emits nothing, keeping the
    /// translation unit free of any collector dependency.
    pub(super) fn emit_managed_memory_prelude(&mut self) {
        if !self.program.requires_managed_memory {
            self.output.push_str(
                "void *el_runtime_alloc(size_t byte_count) {\n\
                 \x20\x20\x20\x20void *result = malloc(byte_count);\n\
                 \x20\x20\x20\x20if (result == NULL) exit(101);\n\
                 \x20\x20\x20\x20el_cost_allocation(byte_count, true);\n\
                 \x20\x20\x20\x20return result;\n\
                 }\n\
                 void *el_runtime_alloc_atomic(size_t byte_count) {\n\
                 \x20\x20\x20\x20void *result = malloc(byte_count);\n\
                 \x20\x20\x20\x20if (result == NULL) exit(101);\n\
                 \x20\x20\x20\x20el_cost_allocation(byte_count, false);\n\
                 \x20\x20\x20\x20return result;\n\
                 }\n\
                 void el_out_of_memory(void) { exit(101); }\n\n",
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
            let scanned = if class == AllocationClass::Scanned {
                "true"
            } else {
                "false"
            };
            let _ = writeln!(
                self.output,
                "        if (result == NULL) el_out_of_memory();\n    }}\n    \
                 el_cost_allocation(byte_count, {scanned});\n    return result;\n}}"
            );
        }
        // Every allocation wrapper has already attempted a full collection
        // and retried before reaching this terminal path (`docs/spec.md` 9).
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
    pub(super) fn emit_managed_operation(&mut self, operation: ManagedMemoryOperation<'_>) {
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

    pub(super) fn emit_prelude(&mut self) {
        let builtin_trap_type = self
            .resolved
            .standard_declaration("BuiltinTrap")
            .and_then(|declaration| self.typed.declaration_types.get(&declaration).copied())
            .map_or(u32::MAX, |ty| ty.index() as u32);
        let _ = writeln!(
            self.output,
            "#define EL_BUILTIN_TRAP_TYPE UINT32_C({builtin_trap_type})"
        );
        let _ = writeln!(
            self.output,
            "#define EL_OWNED_VALUES {}",
            u8::from(self.program.value_model == ValueModel::Owned)
        );
        self.output.push_str(
            "/* Generated by Elamite. C99; internal ABI is intentionally unstable. */\n\
             #ifndef _POSIX_C_SOURCE\n\
             #define _POSIX_C_SOURCE 200809L\n\
             #endif\n\
             #include <stdbool.h>\n\
             #include <stddef.h>\n\
             #include <stdint.h>\n\
             #include <inttypes.h>\n\
             #include <limits.h>\n\
             #include <float.h>\n\
             #include <math.h>\n\
             #include <dirent.h>\n\
             #include <errno.h>\n\
             #include <fcntl.h>\n\
             #include <pthread.h>\n\
             #include <stdio.h>\n\
             #include <stdlib.h>\n\n\
             #include <string.h>\n\
             #include <time.h>\n\
             #include <sys/types.h>\n\
             #include <sys/stat.h>\n\
             #include <sys/wait.h>\n\
             #include <unistd.h>\n\n\
             typedef struct { uint8_t _value; } el_unit;\n\
             typedef struct { uint64_t lo; int64_t hi; } el_i128;\n\
             typedef struct { uint64_t lo; uint64_t hi; } el_u128;\n\n\
             typedef struct { const char *bytes; size_t length; } el_str;\n\
             typedef struct { const char *bytes; size_t length; } el_string;\n\n\
             void el_out_of_memory(void);\n\
             void *el_runtime_alloc(size_t byte_count);\n\
             void *el_runtime_alloc_atomic(size_t byte_count);\n\n\
             #ifdef ELAMITE_COST_INSTRUMENTATION\n\
             static volatile uintptr_t el_cost_allocations = 0U;\n\
             static volatile uintptr_t el_cost_allocated_bytes = 0U;\n\
             static volatile uintptr_t el_cost_scanned_allocations = 0U;\n\
             static volatile uintptr_t el_cost_scanned_bytes = 0U;\n\
             static volatile uintptr_t el_cost_memcpy_calls = 0U;\n\
             static volatile uintptr_t el_cost_memcpy_bytes = 0U;\n\
             static void el_cost_add(volatile uintptr_t *counter, size_t amount) {\n\
             \x20\x20\x20\x20(void)__sync_fetch_and_add(counter, (uintptr_t)amount);\n\
             }\n\
             static void el_cost_allocation(size_t byte_count, bool scanned) {\n\
             \x20\x20\x20\x20el_cost_add(&el_cost_allocations, 1U);\n\
             \x20\x20\x20\x20el_cost_add(&el_cost_allocated_bytes, byte_count);\n\
             \x20\x20\x20\x20if (scanned) { el_cost_add(&el_cost_scanned_allocations, 1U); el_cost_add(&el_cost_scanned_bytes, byte_count); }\n\
             }\n\
             static void *el_cost_memcpy(void *destination, const void *source, size_t byte_count) {\n\
             \x20\x20\x20\x20el_cost_add(&el_cost_memcpy_calls, 1U);\n\
             \x20\x20\x20\x20el_cost_add(&el_cost_memcpy_bytes, byte_count);\n\
             \x20\x20\x20\x20return memcpy(destination, source, byte_count);\n\
             }\n\
             static void el_cost_report(void) {\n\
             \x20\x20\x20\x20fprintf(stderr, \"elamite-cost-v1\\tallocations=%\" PRIuPTR \"\\tallocated_bytes=%\" PRIuPTR \"\\tscanned_allocations=%\" PRIuPTR \"\\tscanned_bytes=%\" PRIuPTR \"\\tmemcpy_calls=%\" PRIuPTR \"\\tmemcpy_bytes=%\" PRIuPTR \"\\n\", el_cost_allocations, el_cost_allocated_bytes, el_cost_scanned_allocations, el_cost_scanned_bytes, el_cost_memcpy_calls, el_cost_memcpy_bytes);\n\
             }\n\
             static void el_cost_begin(void) { (void)atexit(el_cost_report); }\n\
             #else\n\
             #define el_cost_allocation(BYTE_COUNT, SCANNED) ((void)0)\n\
             #define el_cost_memcpy(DESTINATION, SOURCE, BYTE_COUNT) memcpy((DESTINATION), (SOURCE), (BYTE_COUNT))\n\
             #define el_cost_begin() ((void)0)\n\
             #endif\n\n\
             typedef struct { uint32_t type_id; uint64_t code_length; } el_trap_record;\n\
             static int el_expect_read_fd = -1;\n\
             static int el_expect_write_fd = -1;\n\
             static pid_t el_expect_child = (pid_t)-1;\n\
             static uint32_t el_expected_type = UINT32_MAX;\n\
             static el_str el_expected_code = {NULL, 0U};\n\
             static void el_write_all(int fd, const void *data, size_t length) {\n\
             \x20\x20\x20\x20const char *bytes = (const char *)data;\n\
             \x20\x20\x20\x20while (length != 0U) { ssize_t count = write(fd, bytes, length); if (count <= 0) _exit(118); bytes += count; length -= (size_t)count; }\n\
             }\n\
             static bool el_read_all(int fd, void *data, size_t length) {\n\
             \x20\x20\x20\x20char *bytes = (char *)data;\n\
             \x20\x20\x20\x20while (length != 0U) { ssize_t count = read(fd, bytes, length); if (count <= 0) return false; bytes += count; length -= (size_t)count; }\n\
             \x20\x20\x20\x20return true;\n\
             }\n\
             bool el_expect_begin(uint32_t type_id, el_str code, const char *path, uint32_t line, uint32_t column) {\n\
             \x20\x20\x20\x20int descriptors[2];\n\
             \x20\x20\x20\x20fflush(NULL);\n\
             \x20\x20\x20\x20if (pipe(descriptors) != 0) { fprintf(stderr, \"test isolation failed [E-TEST-PROCESS] at %s:%\" PRIu32 \":%\" PRIu32 \"\\n\", path, line, column); exit(103); }\n\
             \x20\x20\x20\x20el_expect_child = fork();\n\
             \x20\x20\x20\x20if (el_expect_child < 0) { close(descriptors[0]); close(descriptors[1]); fprintf(stderr, \"test isolation failed [E-TEST-PROCESS] at %s:%\" PRIu32 \":%\" PRIu32 \"\\n\", path, line, column); exit(103); }\n\
             \x20\x20\x20\x20if (el_expect_child == 0) { close(descriptors[0]); el_expect_write_fd = descriptors[1]; el_expect_read_fd = -1; return true; }\n\
             \x20\x20\x20\x20close(descriptors[1]); el_expect_read_fd = descriptors[0]; el_expect_write_fd = -1; el_expected_type = type_id; el_expected_code = code; return false;\n\
             }\n\
             void el_expect_complete(void) { fflush(NULL); _exit(0); }\n\
             void el_expect_wait(const char *path, uint32_t line, uint32_t column) {\n\
             \x20\x20\x20\x20el_trap_record record = {UINT32_MAX, 0U}; char *code = NULL; int status = 0; bool received;\n\
             \x20\x20\x20\x20received = el_read_all(el_expect_read_fd, &record, sizeof(record));\n\
             \x20\x20\x20\x20if (received && record.code_length <= SIZE_MAX && record.code_length != 0U) { code = (char *)malloc((size_t)record.code_length); if (code == NULL) received = false; }\n\
             \x20\x20\x20\x20if (received && record.code_length != 0U) received = el_read_all(el_expect_read_fd, code, (size_t)record.code_length);\n\
             \x20\x20\x20\x20close(el_expect_read_fd); el_expect_read_fd = -1;\n\
             \x20\x20\x20\x20if (waitpid(el_expect_child, &status, 0) < 0) { received = false; }\n\
             \x20\x20\x20\x20el_expect_child = (pid_t)-1;\n\
             \x20\x20\x20\x20if (!received || !WIFEXITED(status) || WEXITSTATUS(status) != 119 || record.type_id != el_expected_type || record.code_length != (uint64_t)el_expected_code.length || (record.code_length != 0U && memcmp(code, el_expected_code.bytes, (size_t)record.code_length) != 0)) {\n\
             \x20\x20\x20\x20\x20\x20\x20\x20fprintf(stderr, \"expected runtime trap did not occur [E-TEST-EXPECT] at %s:%\" PRIu32 \":%\" PRIu32 \"\\n\", path, line, column); fflush(stderr); exit(103);\n\
             \x20\x20\x20\x20}\n\
             \x20\x20\x20\x20free(code);\n\
             }\n\
             static void el_typed_trap(uint32_t type_id, el_str code, el_str message, const char *path, uint32_t line, uint32_t column) {\n\
             \x20\x20\x20\x20if (el_expect_write_fd >= 0) { el_trap_record record = {type_id, (uint64_t)code.length}; el_write_all(el_expect_write_fd, &record, sizeof(record)); el_write_all(el_expect_write_fd, code.bytes, code.length); close(el_expect_write_fd); fflush(NULL); _exit(119); }\n\
             \x20\x20\x20\x20(void)type_id;\n\
             \x20\x20\x20\x20fputs(\"elamite trap [\", stderr); fwrite(code.bytes, 1U, code.length, stderr);\n\
             \x20\x20\x20\x20fprintf(stderr, \"] at %s:%\" PRIu32 \":%\" PRIu32 \": \", path, line, column);\n\
             \x20\x20\x20\x20fwrite(message.bytes, 1U, message.length, stderr);\n\
             \x20\x20\x20\x20fputc('\\n', stderr);\n\
             \x20\x20\x20\x20fflush(stderr);\n\
             \x20\x20\x20\x20exit(101);\n\
             }\n\
             static void el_trap(const char *code, const char *path, uint32_t line, uint32_t column) {\n\
             \x20\x20\x20\x20el_str value = {code, strlen(code)};\n\
             \x20\x20\x20\x20el_typed_trap(EL_BUILTIN_TRAP_TYPE, value, value, path, line, column);\n\
             }\n\n\
             el_unit el_assert(bool condition, const char *path, uint32_t line, uint32_t column) {\n\
             \x20\x20\x20\x20if (!condition) {\n\
             \x20\x20\x20\x20\x20\x20\x20\x20fprintf(stderr, \"elamite assertion failed [E-TEST-ASSERT] at %s:%\" PRIu32 \":%\" PRIu32 \"\\n\", path, line, column);\n\
             \x20\x20\x20\x20\x20\x20\x20\x20fflush(stderr); exit(102);\n\
             \x20\x20\x20\x20}\n\
             \x20\x20\x20\x20return (el_unit){0};\n\
             }\n\
             void el_assert_fail(el_str message, const char *path, uint32_t line, uint32_t column) {\n\
             \x20\x20\x20\x20fprintf(stderr, \"elamite assertion failed [E-TEST-ASSERT] at %s:%\" PRIu32 \":%\" PRIu32 \": \", path, line, column);\n\
             \x20\x20\x20\x20fwrite(message.bytes, 1U, message.length, stderr); fputc('\\n', stderr);\n\
             \x20\x20\x20\x20fflush(stderr); exit(102);\n\
             }\n\n\
             el_string el_string_allocate(size_t length) {\n\
             \x20\x20\x20\x20el_string result = {NULL, length};\n\
             \x20\x20\x20\x20if (length == SIZE_MAX) el_out_of_memory();\n\
             #if EL_OWNED_VALUES\n\
             \x20\x20\x20\x20result.bytes = (const char *)malloc(length + 1U);\n\
             \x20\x20\x20\x20if (result.bytes == NULL) el_out_of_memory();\n\
             \x20\x20\x20\x20el_cost_allocation(length + 1U, false);\n\
             #else\n\
             \x20\x20\x20\x20result.bytes = (const char *)el_runtime_alloc_atomic(length + 1U);\n\
             #endif\n\
             \x20\x20\x20\x20return result;\n\
             }\n\
             char *el_string_make_mut(el_string *value) {\n\
             \x20\x20\x20\x20if (value->bytes == NULL) { *value = el_string_allocate(0U); ((char *)value->bytes)[0] = '\\0'; }\n\
             \x20\x20\x20\x20return (char *)value->bytes;\n\
             }\n\
             el_string el_string_from(el_str value) {\n\
             \x20\x20\x20\x20el_string owned = el_string_allocate(value.length);\n\
             \x20\x20\x20\x20if (value.length != 0U) el_cost_memcpy((char *)owned.bytes, value.bytes, value.length);\n\
             \x20\x20\x20\x20((char *)owned.bytes)[value.length] = '\\0';\n\
             \x20\x20\x20\x20return owned;\n\
             }\n\
             el_string el_string_clone(el_string value) {\n\
             \x20\x20\x20\x20return el_string_from((el_str){value.bytes, value.length});\n\
             }\n\
             void el_owned_string_drop(el_string *value) {\n\
             #if EL_OWNED_VALUES\n\
             \x20\x20\x20\x20free((void *)value->bytes);\n\
             #endif\n\
             \x20\x20\x20\x20value->bytes = NULL; value->length = 0U;\n\
             }\n\
             el_str el_concat_str(el_str left, el_str right) {\n\
             \x20\x20\x20\x20el_str result = {NULL, 0U};\n\
             \x20\x20\x20\x20if (left.length > SIZE_MAX - right.length) el_out_of_memory();\n\
             \x20\x20\x20\x20result.length = left.length + right.length;\n\
             \x20\x20\x20\x20result.bytes = (const char *)el_runtime_alloc_atomic(result.length == 0U ? 1U : result.length);\n\
             \x20\x20\x20\x20if (left.length != 0U) el_cost_memcpy((char *)result.bytes, left.bytes, left.length);\n\
             \x20\x20\x20\x20if (right.length != 0U) el_cost_memcpy((char *)result.bytes + left.length, right.bytes, right.length);\n\
             \x20\x20\x20\x20return result;\n\
             }\n\
             el_string el_concat_string(el_string left, el_string right) {\n\
             \x20\x20\x20\x20el_string result;\n\
             \x20\x20\x20\x20char *bytes;\n\
             \x20\x20\x20\x20if (left.length >= SIZE_MAX - right.length) el_out_of_memory();\n\
             \x20\x20\x20\x20result = el_string_allocate(left.length + right.length);\n\
             \x20\x20\x20\x20bytes = (char *)result.bytes;\n\
             \x20\x20\x20\x20if (left.length != 0U) el_cost_memcpy(bytes, left.bytes, left.length);\n\
             \x20\x20\x20\x20if (right.length != 0U) el_cost_memcpy(bytes + left.length, right.bytes, right.length);\n\
             \x20\x20\x20\x20bytes[result.length] = '\\0';\n\
             \x20\x20\x20\x20result.bytes = bytes;\n\
             #if EL_OWNED_VALUES\n\
             \x20\x20\x20\x20free((void *)left.bytes); free((void *)right.bytes);\n\
             #endif\n\
             \x20\x20\x20\x20return result;\n\
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
             \x20\x20\x20\x20size_t needed;\n\
             \x20\x20\x20\x20size_t capacity;\n\
             \x20\x20\x20\x20char *replacement;\n\
             \x20\x20\x20\x20if (extra >= (size_t)PTRDIFF_MAX || formatter->length > (size_t)PTRDIFF_MAX - extra - 1U) el_out_of_memory();\n\
             \x20\x20\x20\x20needed = formatter->length + extra + 1U;\n\
             \x20\x20\x20\x20if (needed <= formatter->capacity) return;\n\
             \x20\x20\x20\x20capacity = formatter->capacity == 0U ? 32U : formatter->capacity;\n\
             \x20\x20\x20\x20while (capacity < needed) {\n\
             \x20\x20\x20\x20\x20\x20\x20\x20if (capacity > (size_t)PTRDIFF_MAX / 2U) { capacity = needed; break; }\n\
             \x20\x20\x20\x20\x20\x20\x20\x20capacity *= 2U;\n\
             \x20\x20\x20\x20}\n\
             \x20\x20\x20\x20replacement = (char *)el_runtime_alloc_atomic(capacity);\n\
             \x20\x20\x20\x20if (formatter->length != 0U) el_cost_memcpy(replacement, formatter->bytes, formatter->length);\n\
             \x20\x20\x20\x20formatter->bytes = replacement;\n\
             \x20\x20\x20\x20formatter->capacity = capacity;\n\
             }\n\
             void el_fmt_append_n(el_formatter *formatter, const char *text, size_t length) {\n\
             \x20\x20\x20\x20el_fmt_reserve(formatter, length);\n\
             \x20\x20\x20\x20if (length != 0U) el_cost_memcpy(formatter->bytes + formatter->length, text, length);\n\
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
             \x20\x20\x20\x20if (formatter->bytes == NULL) { formatter->bytes = (char *)el_runtime_alloc_atomic(1U); formatter->bytes[0] = '\\0'; }\n\
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
        if self.program.functions.iter().any(|function| {
            let reachable = reachable_blocks(function);
            function.blocks.iter().any(|block| {
                reachable.contains(&block.id)
                    && matches!(
                        block.terminator,
                        Terminator::NeverCall {
                            call: NeverCall::Panic { .. },
                            ..
                        }
                    )
            })
        }) {
            self.output.push_str(
                 "static void el_panic(el_str message, const char *path, uint32_t line, uint32_t column) {\n\
                 \x20\x20\x20\x20el_str code = {\"E-RUN-PANIC\", sizeof(\"E-RUN-PANIC\") - 1U};\n\
                 \x20\x20\x20\x20el_typed_trap(EL_BUILTIN_TRAP_TYPE, code, message, path, line, column);\n\
                 }\n\n",
            );
        }
        if self.uses_synchronized_output() {
            self.output
                .push_str("static pthread_mutex_t el_stdout_lock = PTHREAD_MUTEX_INITIALIZER;\n\n");
        }
        self.emit_checked_integer_helpers();
    }

    pub(super) fn emit_checked_integer_helpers(&mut self) {
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
             TYPE el_shl_##NAME(TYPE a, uintmax_t b, const char *p, uint32_t l, uint32_t c) { uint32_t n; if (b >= (BITS)) { el_trap(\"E-RUN-SHIFT\", p, l, c); } n = (uint32_t)b; while (n-- != 0U) { if (a > MAXIMUM / 2 || a < MINIMUM / 2) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } a = (TYPE)(a * 2); } return a; } \\\n\
             TYPE el_shr_##NAME(TYPE a, uintmax_t b, const char *p, uint32_t l, uint32_t c) { uint32_t n; if (b >= (BITS)) el_trap(\"E-RUN-SHIFT\", p, l, c); n = (uint32_t)b; while (n-- != 0U) a = a >= 0 ? (TYPE)(a / 2) : (TYPE)(-1 - ((-1 - a) / 2)); return a; }\n\
             #define EL_UNSIGNED_ARITH(NAME, TYPE, MAXIMUM, BITS) \\\n\
             TYPE el_add_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { if (a > (TYPE)(MAXIMUM - b)) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } return (TYPE)(a + b); } \\\n\
             TYPE el_sub_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { if (a < b) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } return (TYPE)(a - b); } \\\n\
             TYPE el_mul_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { if (b != 0 && a > MAXIMUM / b) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } return (TYPE)(a * b); } \\\n\
             TYPE el_neg_##NAME(TYPE a, const char *p, uint32_t l, uint32_t c) { if (a != 0) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } return 0; } \\\n\
             TYPE el_div_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { if (b == 0) { el_trap(\"E-RUN-DIVZERO\", p, l, c); } return (TYPE)(a / b); } \\\n\
             TYPE el_rem_##NAME(TYPE a, TYPE b, const char *p, uint32_t l, uint32_t c) { if (b == 0) { el_trap(\"E-RUN-DIVZERO\", p, l, c); } return (TYPE)(a % b); } \\\n\
             TYPE el_shl_##NAME(TYPE a, uintmax_t b, const char *p, uint32_t l, uint32_t c) { if (b >= (BITS)) { el_trap(\"E-RUN-SHIFT\", p, l, c); } if (a > (TYPE)(MAXIMUM >> b)) { el_trap(\"E-RUN-OVERFLOW\", p, l, c); } return (TYPE)(a << b); } \\\n\
             TYPE el_shr_##NAME(TYPE a, uintmax_t b, const char *p, uint32_t l, uint32_t c) { if (b >= (BITS)) { el_trap(\"E-RUN-SHIFT\", p, l, c); } return (TYPE)(a >> b); }\n\
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
    /// (`docs/spec.md` 4.1) as two macros, instantiated per used integer type.
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
    pub(super) fn emit_numeric_alternative_macros(&mut self) {
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

    /// Operand types of every structural comparison in the program.
    pub(super) fn compared_types(&self) -> BTreeSet<TypeId> {
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
    pub(super) fn ordered_types(&self) -> BTreeSet<TypeId> {
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

    pub(super) fn defaulted_types(&self) -> BTreeSet<TypeId> {
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
    pub(super) fn checked_conversions(&self) -> BTreeSet<(NumericOutcome, TypeId, TypeId)> {
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

    /// Emits one standard numeric conversion helper (`docs/spec.md` 4.1).
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
    pub(super) fn emit_numeric_conversion_helper(
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
            // rounding and cannot fail (`docs/spec.md` 4.1).
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

    pub(super) fn standard_call_instances(&self) -> BTreeMap<StandardCall, (TypeId, Span)> {
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
                if let Terminator::NeverCall {
                    call: NeverCall::Standard { operation, .. },
                    span,
                } = &block.terminator
                {
                    let result = match operation {
                        StandardCall::ThreadJoin { return_type, .. } => *return_type,
                        _ => continue,
                    };
                    calls.entry(*operation).or_insert((result, *span));
                }
            }
        }
        calls
    }

    fn system_call_instances(&self) -> BTreeMap<StandardCall, (TypeId, Vec<TypeId>, Span)> {
        let mut calls = BTreeMap::new();
        for function in &self.program.functions {
            for block in &function.blocks {
                for instruction in &block.instructions {
                    if let Instruction::Assign {
                        destination,
                        value:
                            Rvalue::StandardCall {
                                operation,
                                arguments,
                                ..
                            },
                        span,
                    } = instruction
                        && matches!(operation, StandardCall::System { .. })
                    {
                        calls.entry(*operation).or_insert_with(|| {
                            (
                                function.temporary_types[destination.index()],
                                arguments
                                    .iter()
                                    .map(|argument| function.temporary_types[argument.index()])
                                    .collect(),
                                *span,
                            )
                        });
                    }
                }
            }
        }
        calls
    }

    pub(super) fn collection_literal_instances(
        &self,
    ) -> BTreeSet<(CollectionLiteralKind, TypeId, usize)> {
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
                            self.resolve_alias(function.temporary_types[destination.index()]),
                            elements.len(),
                        ));
                    }
                }
            }
        }
        literals
    }

    pub(super) fn variadic_slice_instances(&self) -> BTreeSet<(TypeId, TypeId, usize)> {
        let mut slices = BTreeSet::new();
        for function in &self.program.functions {
            for block in &function.blocks {
                for instruction in &block.instructions {
                    if let Instruction::Assign {
                        destination,
                        value:
                            Rvalue::VariadicSlice {
                                elements,
                                element_type,
                            },
                        ..
                    } = instruction
                    {
                        slices.insert((
                            self.resolve_alias(function.temporary_types[destination.index()]),
                            self.resolve_alias(*element_type),
                            elements.len(),
                        ));
                    }
                }
            }
        }
        slices
    }

    pub(super) fn emit_variadic_slice_helper(
        &mut self,
        slice: TypeId,
        element: TypeId,
        length: usize,
    ) {
        let (Some(slice_type), Some(element_type)) =
            (self.c_type(slice, None), self.c_type(element, None))
        else {
            return;
        };
        let name = variadic_slice_name(slice, length);
        let parameters = (0..length)
            .map(|index| format!("{element_type} value{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let parameters = if parameters.is_empty() {
            "void".to_string()
        } else {
            parameters
        };
        let _ = writeln!(
            self.output,
            "static {slice_type} {name}({parameters}) {{\n    {slice_type} result = {{ NULL, \
             (uintptr_t){length}U }};"
        );
        if length != 0 {
            self.emit_runtime_allocate(
                "result.values",
                &format!("(size_t){length}U * sizeof({element_type})"),
                if self.scanned_allocation(element) {
                    AllocationClass::Scanned
                } else {
                    AllocationClass::PointerFree
                },
            );
            for index in 0..length {
                let _ = writeln!(self.output, "    result.values[{index}] = value{index};");
            }
        }
        self.output.push_str("    return result;\n}\n\n");
    }

    pub(super) fn emit_runtime_allocate(
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

    fn emit_collection_allocate(
        &mut self,
        destination: &str,
        byte_count: &str,
        class: AllocationClass,
    ) {
        if self.program.value_model == ValueModel::Owned {
            let _ = writeln!(self.output, "    {destination} = malloc({byte_count});");
            let _ = writeln!(
                self.output,
                "    if ({destination} == NULL) el_out_of_memory();"
            );
            let _ = writeln!(self.output, "    el_cost_allocation({byte_count}, false);");
        } else {
            self.emit_runtime_allocate(destination, byte_count, class);
        }
    }

    pub(super) fn option_expression(
        &mut self,
        option: TypeId,
        value: Option<(&str, TypeId)>,
    ) -> String {
        let Some(enumeration) = self.enums.get(&option).copied() else {
            return zero_value(option, &self.typed.types, self.program.value_model);
        };
        let variant_name = if value.is_some() { "Some" } else { "None" };
        let Some(variant_id) = self.resolved.standard_variant("Option", variant_name) else {
            return zero_value(option, &self.typed.types, self.program.value_model);
        };
        let Some(variant) = enumeration
            .variants
            .iter()
            .find(|variant| variant.id == variant_id)
        else {
            return zero_value(option, &self.typed.types, self.program.value_model);
        };
        let c_type = self
            .c_type(option, None)
            .unwrap_or_else(|| "el_unit".to_string());
        match (value, variant.fields.first()) {
            (Some((expression, _)), Some((field, _, _))) => format!(
                "({c_type}){{ .tag = UINT32_C({}), .payload.{} = {{ .{} = {expression} }} }}",
                variant_id.index(),
                variant_member_name(variant_id),
                field_name(*field)
            ),
            // Omitted aggregate members are zero-initialized by C99. Leaving
            // the inactive payload out also avoids brace-depth diagnostics
            // whose required nesting depends on the concrete payload type.
            _ => format!("({c_type}){{ .tag = UINT32_C({}) }}", variant_id.index()),
        }
    }

    fn enum_unit_variant_expression(&mut self, ty: TypeId, variant: VariantId) -> String {
        let c_type = self
            .c_type(ty, None)
            .unwrap_or_else(|| "el_unit".to_string());
        format!("({c_type}){{ .tag = UINT32_C({}) }}", variant.index())
    }

    fn result_expression(
        &mut self,
        result: TypeId,
        variant_name: &str,
        value: (&str, TypeId),
    ) -> String {
        let Some(enumeration) = self.enums.get(&result).copied() else {
            return zero_value(result, &self.typed.types, self.program.value_model);
        };
        let Some(variant_id) = self.resolved.standard_variant("Result", variant_name) else {
            return zero_value(result, &self.typed.types, self.program.value_model);
        };
        let Some(variant) = enumeration
            .variants
            .iter()
            .find(|variant| variant.id == variant_id)
        else {
            return zero_value(result, &self.typed.types, self.program.value_model);
        };
        let Some((field, _, _)) = variant.fields.first() else {
            return zero_value(result, &self.typed.types, self.program.value_model);
        };
        let c_type = self
            .c_type(result, None)
            .unwrap_or_else(|| "el_unit".to_string());
        let copied = value.0.to_string();
        format!(
            "({c_type}){{ .tag = UINT32_C({}), .payload.{} = {{ .{} = {} }} }}",
            variant_id.index(),
            variant_member_name(variant_id),
            field_name(*field),
            copied
        )
    }

    pub(super) fn emit_standard_runtime_helpers(&mut self) {
        for (slice, element, length) in self.variadic_slice_instances() {
            self.emit_variadic_slice_helper(slice, element, length);
        }
        let calls = self.standard_call_instances();
        let clone_types = calls
            .keys()
            .filter_map(|operation| match operation {
                StandardCall::Clone { value } => Some(*value),
                _ => None,
            })
            .collect::<Vec<_>>();
        for ty in clone_types {
            self.emit_clone_helper(ty);
        }
        let mut ownership_drop_types = BTreeSet::new();
        for function in &self.program.functions {
            for block in &function.blocks {
                for instruction in &block.instructions {
                    match instruction {
                        Instruction::DropValue { action, .. } => {
                            collect_ownership_drop_types(action, &mut ownership_drop_types);
                        }
                        Instruction::DropIteration { actions, .. } => {
                            for action in actions {
                                collect_ownership_drop_types(action, &mut ownership_drop_types);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        for ty in ownership_drop_types {
            self.emit_drop_helper(ty);
        }
        for operation in calls.keys() {
            if let StandardCall::BoxNew { boxed, value } = operation {
                let (Some(boxed_c), Some(value_c)) =
                    (self.c_type(*boxed, None), self.c_type(*value, None))
                else {
                    continue;
                };
                let name = standard_call_name(*operation);
                let _ = writeln!(
                    self.output,
                    "static {boxed_c} {name}({value_c} value) {{\n    {boxed_c} result = ({boxed_c})malloc(sizeof({value_c}));\n    if (result == NULL) el_out_of_memory();\n    el_cost_allocation(sizeof({value_c}), false);\n    *result = value;\n    return result;\n}}\n"
                );
            }
        }
        let validates_host_text = self.system_call_instances().keys().any(|call| {
            matches!(
                call,
                StandardCall::System {
                    operation: SystemOperation::EnvGet
                        | SystemOperation::CurrentDir
                        | SystemOperation::DirectoryNext
                        | SystemOperation::Args,
                    ..
                }
            )
        });
        if validates_host_text {
            self.output.push_str(
                "static bool el_valid_utf8_cstr(const char *text) {\n\
                     size_t i = 0U;\n\
                     while (text[i] != '\\0') {\n\
                         unsigned char a = (unsigned char)text[i++], b, c, d;\n\
                         if (a < 0x80U) continue;\n\
                         b = (unsigned char)text[i++];\n\
                         if (a >= 0xC2U && a <= 0xDFU) { if ((b & 0xC0U) != 0x80U) return false; continue; }\n\
                         if ((b & 0xC0U) != 0x80U) return false;\n\
                         c = (unsigned char)text[i++];\n\
                         if (a >= 0xE0U && a <= 0xEFU) { if ((c & 0xC0U) != 0x80U || (a == 0xE0U && b < 0xA0U) || (a == 0xEDU && b >= 0xA0U)) return false; continue; }\n\
                         if ((c & 0xC0U) != 0x80U) return false;\n\
                         d = (unsigned char)text[i++];\n\
                         if (a < 0xF0U || a > 0xF4U || (d & 0xC0U) != 0x80U || (a == 0xF0U && b < 0x90U) || (a == 0xF4U && b >= 0x90U)) return false;\n\
                     }\n\
                     return true;\n\
                 }\n\n",
            );
        }
        if self.uses_process_arguments() {
            self.output.push_str(
                "static size_t el_utf8_prefix_width(const unsigned char *text, size_t length) {\n\
                     unsigned char a, b, c, d;\n\
                     if (length == 0U) { return 0U; } a = text[0]; if (a < 0x80U) { return 1U; }\n\
                     if (length < 2U) { return 0U; } b = text[1];\n\
                     if (a >= 0xC2U && a <= 0xDFU) return (b & 0xC0U) == 0x80U ? 2U : 0U;\n\
                     if (length < 3U || (b & 0xC0U) != 0x80U) { return 0U; } c = text[2];\n\
                     if (a >= 0xE0U && a <= 0xEFU) return (c & 0xC0U) == 0x80U && !(a == 0xE0U && b < 0xA0U) && !(a == 0xEDU && b >= 0xA0U) ? 3U : 0U;\n\
                     if (length < 4U || (c & 0xC0U) != 0x80U) { return 0U; } d = text[3];\n\
                     return a >= 0xF0U && a <= 0xF4U && (d & 0xC0U) == 0x80U && !(a == 0xF0U && b < 0x90U) && !(a == 0xF4U && b >= 0x90U) ? 4U : 0U;\n\
                 }\n\
                 static el_string el_string_from_host_text(const char *text) {\n\
                     size_t length = strlen(text), i = 0U, written = 0U, width; el_string out;\n\
                     if (el_valid_utf8_cstr(text)) return el_string_from((el_str){text, length});\n\
                     if (length > SIZE_MAX / 3U) { el_out_of_memory(); } out = el_string_allocate(length * 3U);\n\
                     while (i < length) { width = el_utf8_prefix_width((const unsigned char *)text + i, length - i); if (width != 0U) { memcpy((char *)out.bytes + written, text + i, width); i += width; written += width; } else { ((char *)out.bytes)[written++] = (char)0xEF; ((char *)out.bytes)[written++] = (char)0xBF; ((char *)out.bytes)[written++] = (char)0xBD; ++i; } }\n\
                     out.length = written; ((char *)out.bytes)[written] = '\\0'; return out;\n\
                 }\n\n",
            );
        }
        self.emit_system_helpers();
        self.emit_env_process_helpers();
        self.emit_text_helpers(&calls);
        self.emit_clock_helpers(&calls);
        self.emit_thread_helpers(&calls);
        self.emit_channel_helpers(&calls);
        self.emit_mutex_helpers(&calls);
        self.emit_atomic_helpers(&calls);
        self.emit_shared_store_helpers(&calls);
        let literals = self.collection_literal_instances();
        let mut concatenated_types = BTreeSet::new();
        for function in &self.program.functions {
            for block in &function.blocks {
                for instruction in &block.instructions {
                    if let Instruction::Assign {
                        destination,
                        value:
                            Rvalue::Binary {
                                operator: BinaryOperator::Concatenate,
                                ..
                            },
                        ..
                    } = instruction
                    {
                        concatenated_types.insert(
                            self.resolve_alias(function.temporary_types[destination.index()]),
                        );
                    }
                }
            }
        }
        let mut collections = BTreeSet::new();
        for operation in calls.keys() {
            if let Some(collection) = standard_collection_type(*operation) {
                collections.insert(self.resolve_alias(collection));
            }
        }
        collections.extend(literals.iter().map(|(_, ty, _)| self.resolve_alias(*ty)));
        collections.extend(
            self.used_types()
                .into_iter()
                .filter(|ty| match self.typed.types.kind(self.resolve_alias(*ty)) {
                    TypeKind::Builtin { builtin, .. } => {
                        matches!(self.resolved.builtin_name(*builtin), "Vec" | "Map" | "Set")
                    }
                    _ => false,
                })
                .map(|ty| self.resolve_alias(ty)),
        );

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
                    self.emit_vec_helpers(
                        collection,
                        *element,
                        &calls,
                        &literals,
                        concatenated_types.contains(&self.resolve_alias(collection)),
                    );
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

    fn emit_system_helpers(&mut self) {
        let calls = self.system_call_instances();
        if calls.keys().any(|call| {
            matches!(
                call,
                StandardCall::System {
                    operation: SystemOperation::Open
                        | SystemOperation::ReadDir
                        | SystemOperation::Metadata
                        | SystemOperation::CreateDir
                        | SystemOperation::RemoveDir
                        | SystemOperation::RemoveFile
                        | SystemOperation::Rename
                        | SystemOperation::FileReadToEnd
                        | SystemOperation::FileWriteAll
                        | SystemOperation::FileMetadata
                        | SystemOperation::DirectoryNext
                        | SystemOperation::CurrentDir,
                    ..
                }
            )
        }) {
            self.emit_io_error_mapper();
        }
        let mut emitted_file_state = false;
        let mut emitted_directory_state = false;
        for (call, (_, arguments, span)) in &calls {
            let StandardCall::System { operation, .. } = call else {
                continue;
            };
            if !emitted_file_state
                && matches!(
                    operation,
                    SystemOperation::FileReadToEnd
                        | SystemOperation::FileWriteAll
                        | SystemOperation::FileMetadata
                        | SystemOperation::FileClose
                )
                && let Some(file) = arguments.first().copied()
                && let Some(file_c) = self.c_type(file, Some(*span))
            {
                let _ = writeln!(
                    self.output,
                    "struct {file_c}_data {{ int fd; bool closed; }};\n"
                );
                emitted_file_state = true;
            }
            if !emitted_directory_state
                && matches!(
                    operation,
                    SystemOperation::DirectoryNext | SystemOperation::DirectoryClose
                )
                && let Some(directory) = arguments.first().copied()
                && let Some(directory_c) = self.c_type(directory, Some(*span))
            {
                let _ = writeln!(
                    self.output,
                    "struct {directory_c}_data {{ DIR *stream; bool closed; el_string path; }};\n"
                );
                emitted_directory_state = true;
            }
        }
        for (call, (result, arguments, span)) in calls {
            let StandardCall::System { operation, .. } = call else {
                continue;
            };
            let name = standard_call_name(call);
            let Some(result_c) = self.c_type(result, Some(span)) else {
                continue;
            };
            if operation == SystemOperation::Open {
                let Some(path) = arguments.first().copied() else {
                    continue;
                };
                let Some(mode) = arguments.get(1).copied() else {
                    continue;
                };
                let (Some(path_c), Some(mode_c), Some(path_field)) = (
                    self.c_type(path, Some(span)),
                    self.c_type(mode, Some(span)),
                    self.path_field(path),
                ) else {
                    continue;
                };
                let Some((file, _, _)) = self.result_variant_payload(result, "Ok") else {
                    continue;
                };
                let Some(file_c) = self.c_type(file, Some(span)) else {
                    continue;
                };
                let (Some(read_mode), Some(write_mode)) = (
                    self.resolved.standard_variant("OpenMode", "Read"),
                    self.resolved.standard_variant("OpenMode", "Write"),
                ) else {
                    continue;
                };
                if !emitted_file_state {
                    let _ = writeln!(
                        self.output,
                        "struct {file_c}_data {{ int fd; bool closed; }};\n"
                    );
                    emitted_file_state = true;
                }
                let Some((error, error_ty)) = self.io_error_value("errno") else {
                    continue;
                };
                let error = self.result_expression(result, "Err", (&error, error_ty));
                let ok = self.result_expression(result, "Ok", ("state", file));
                let path_bytes = format!("path->{}.bytes", field_name(path_field));
                let _ = writeln!(
                    self.output,
                    "static {result_c} {name}({path_c} path, {mode_c} mode) {{ int flags = mode.tag == {}U ? O_RDONLY : mode.tag == {}U ? (O_WRONLY|O_CREAT|O_TRUNC) : (O_WRONLY|O_CREAT|O_APPEND); int fd; {file_c} state; if(memchr({path_bytes},'\\0',path->{}.length)!=NULL){{errno=EINVAL;return {error};}}fd=open({path_bytes},flags,0666);if(fd<0)return {error};state=el_runtime_alloc(sizeof(*state));state->fd=fd;state->closed=false;return {ok};}}\n",
                    read_mode.index(),
                    write_mode.index(),
                    field_name(path_field)
                );
                continue;
            }
            if operation == SystemOperation::FileClose {
                let Some(file) = arguments.first().copied() else {
                    continue;
                };
                let Some(file_c) = self.c_type(file, Some(span)) else {
                    continue;
                };
                let _ = writeln!(
                    self.output,
                    "static {result_c} {name}({file_c} file) {{ if(file!=NULL&&!file->closed){{ file->closed=true; (void)close(file->fd); }} return (el_unit){{0}}; }}\n"
                );
                continue;
            }
            if operation == SystemOperation::FileWriteAll {
                let (Some(file), Some(bytes)) =
                    (arguments.first().copied(), arguments.get(1).copied())
                else {
                    continue;
                };
                let (Some(file_c), Some(bytes_c)) = (
                    self.c_type(file, Some(span)),
                    self.c_type(bytes, Some(span)),
                ) else {
                    continue;
                };
                let Some((error_value, error_ty)) = self.io_error_value("errno") else {
                    continue;
                };
                let error = self.result_expression(result, "Err", (&error_value, error_ty));
                let Some(ok_variant) = self.resolved.standard_variant("Result", "Ok") else {
                    continue;
                };
                let Some(unit) = self
                    .enums
                    .get(&result)
                    .and_then(|e| e.variants.iter().find(|v| v.id == ok_variant))
                    .and_then(|v| v.fields.first())
                    .map(|f| f.2)
                else {
                    continue;
                };
                let ok = self.result_expression(result, "Ok", ("(el_unit){0}", unit));
                let _ = writeln!(
                    self.output,
                    "static {result_c} {name}({file_c} file, {bytes_c} bytes) {{ uintptr_t done=0U; if(file==NULL||file->closed){{errno=EBADF;return {error};}} while(done<bytes.length){{ ssize_t n=write(file->fd,bytes.values+done,(size_t)(bytes.length-done)); if(n<0){{if(errno==EINTR)continue;return {error};}} if(n==0){{errno=EIO;return {error};}} done+=(uintptr_t)n; }} return {ok}; }}\n"
                );
                continue;
            }
            if matches!(
                operation,
                SystemOperation::Metadata | SystemOperation::FileMetadata
            ) {
                let Some((metadata, _, _)) = self.result_variant_payload(result, "Ok") else {
                    continue;
                };
                let Some(metadata_c) = self.c_type(metadata, Some(span)) else {
                    continue;
                };
                let Some(structure) = self.structs.get(&metadata).copied() else {
                    continue;
                };
                let [kind_field, len_field, readonly_field] = structure.fields.as_slice() else {
                    continue;
                };
                let kind_ty = kind_field.2;
                let Some(kind_c) = self.c_type(kind_ty, Some(span)) else {
                    continue;
                };
                let (Some(file_v), Some(dir_v), Some(link_v), Some(other_v)) = (
                    self.resolved.standard_variant("FileType", "File"),
                    self.resolved.standard_variant("FileType", "Directory"),
                    self.resolved.standard_variant("FileType", "SymbolicLink"),
                    self.resolved.standard_variant("FileType", "Other"),
                ) else {
                    continue;
                };
                let value = format!(
                    "({metadata_c}){{ .{} = ({kind_c}){{.tag=kind}}, .{} = (uint64_t)st.st_size, .{} = (st.st_mode&(S_IWUSR|S_IWGRP|S_IWOTH))==0 }}",
                    field_name(kind_field.0),
                    field_name(len_field.0),
                    field_name(readonly_field.0)
                );
                let ok = self.result_expression(result, "Ok", (&value, metadata));
                let Some((error_value, error_ty)) = self.io_error_value("errno") else {
                    continue;
                };
                let error = self.result_expression(result, "Err", (&error_value, error_ty));
                let kind_expr = format!(
                    "S_ISREG(st.st_mode)?{}U:S_ISDIR(st.st_mode)?{}U:S_ISLNK(st.st_mode)?{}U:{}U",
                    file_v.index(),
                    dir_v.index(),
                    link_v.index(),
                    other_v.index()
                );
                if operation == SystemOperation::Metadata {
                    let Some(path) = arguments.first().copied() else {
                        continue;
                    };
                    let (Some(path_c), Some(field)) =
                        (self.c_type(path, Some(span)), self.path_field(path))
                    else {
                        continue;
                    };
                    let _ = writeln!(
                        self.output,
                        "static {result_c} {name}({path_c} path) {{struct stat st;uint32_t kind;if(memchr(path->{}.bytes,'\\0',path->{}.length)!=NULL){{errno=EINVAL;return {error};}}if(lstat(path->{}.bytes,&st)!=0)return {error};kind={kind_expr};return {ok};}}\n",
                        field_name(field),
                        field_name(field),
                        field_name(field)
                    );
                } else {
                    let Some(file) = arguments.first().copied() else {
                        continue;
                    };
                    let Some(file_c) = self.c_type(file, Some(span)) else {
                        continue;
                    };
                    let _ = writeln!(
                        self.output,
                        "static {result_c} {name}({file_c} file) {{ struct stat st; uint32_t kind; if(file==NULL||file->closed){{errno=EBADF;return {error};}} if(fstat(file->fd,&st)!=0)return {error}; kind={kind_expr}; return {ok}; }}\n"
                    );
                }
                continue;
            }
            if operation == SystemOperation::FileReadToEnd {
                let Some(file) = arguments.first().copied() else {
                    continue;
                };
                let Some(file_c) = self.c_type(file, Some(span)) else {
                    continue;
                };
                let Some((bytes, _, _)) = self.result_variant_payload(result, "Ok") else {
                    continue;
                };
                let Some(bytes_c) = self.c_type(bytes, Some(span)) else {
                    continue;
                };
                let Some((error_value, error_ty)) = self.io_error_value("errno") else {
                    continue;
                };
                let error = self.result_expression(result, "Err", (&error_value, error_ty));
                let ok = self.result_expression(result, "Ok", ("out", bytes));
                let _ = writeln!(
                    self.output,
                    "static {result_c} {name}({file_c} file) {{ {bytes_c} out={{0}}; uint8_t chunk[4096]; size_t done=0U; if(file==NULL||file->closed){{errno=EBADF;return {error};}} for(;;){{ssize_t n=read(file->fd,chunk,sizeof(chunk));size_t required,capacity;uint8_t *grown;if(n<0){{if(errno==EINTR)continue;return {error};}}if(n==0)break;if(done>SIZE_MAX-(size_t)n){{errno=EFBIG;return {error};}}required=done+(size_t)n;capacity=(size_t)out.capacity;if(capacity<required){{capacity=capacity==0U?4096U:capacity;while(capacity<required){{if(capacity>SIZE_MAX/2U){{capacity=required;break;}}capacity*=2U;}}grown=el_runtime_alloc_atomic(capacity);if(done!=0U)memcpy(grown,out.values,done);out.values=grown;out.capacity=(uintptr_t)capacity;}}memcpy(out.values+done,chunk,(size_t)n);done=required;}}out.length=(uintptr_t)done;return {ok}; }}\n"
                );
                continue;
            }
            if operation == SystemOperation::ReadDir {
                let Some(path) = arguments.first().copied() else {
                    continue;
                };
                let (Some(path_c), Some(field)) =
                    (self.c_type(path, Some(span)), self.path_field(path))
                else {
                    continue;
                };
                let Some((directory, _, _)) = self.result_variant_payload(result, "Ok") else {
                    continue;
                };
                let Some(directory_c) = self.c_type(directory, Some(span)) else {
                    continue;
                };
                if !emitted_directory_state {
                    let _ = writeln!(
                        self.output,
                        "struct {directory_c}_data {{ DIR *stream; bool closed; el_string path; }};\n"
                    );
                    emitted_directory_state = true;
                }
                let Some((error_value, error_ty)) = self.io_error_value("errno") else {
                    continue;
                };
                let error = self.result_expression(result, "Err", (&error_value, error_ty));
                let ok = self.result_expression(result, "Ok", ("state", directory));
                let _ = writeln!(
                    self.output,
                    "static {result_c} {name}({path_c} path) {{{directory_c} state;DIR *stream;if(memchr(path->{}.bytes,'\\0',path->{}.length)!=NULL){{errno=EINVAL;return {error};}}stream=opendir(path->{}.bytes);if(stream==NULL)return {error};state=el_runtime_alloc(sizeof(*state));state->stream=stream;state->closed=false;state->path=path->{};return {ok};}}\n",
                    field_name(field),
                    field_name(field),
                    field_name(field),
                    field_name(field)
                );
                continue;
            }
            if operation == SystemOperation::DirectoryClose {
                let Some(directory) = arguments.first().copied() else {
                    continue;
                };
                let Some(directory_c) = self.c_type(directory, Some(span)) else {
                    continue;
                };
                let _ = writeln!(
                    self.output,
                    "static {result_c} {name}({directory_c} directory) {{if(directory!=NULL&&!directory->closed){{directory->closed=true;(void)closedir(directory->stream);}}return (el_unit){{0}};}}\n"
                );
                continue;
            }
            if operation == SystemOperation::DirectoryNext {
                let Some(directory) = arguments.first().copied() else {
                    continue;
                };
                let Some(directory_c) = self.c_type(directory, Some(span)) else {
                    continue;
                };
                let Some((optional, _, _)) = self.result_variant_payload(result, "Ok") else {
                    continue;
                };
                let Some(some_variant) = self.resolved.standard_variant("Option", "Some") else {
                    continue;
                };
                let Some(entry) = self
                    .enums
                    .get(&optional)
                    .and_then(|e| e.variants.iter().find(|v| v.id == some_variant))
                    .and_then(|v| v.fields.first())
                    .map(|f| f.2)
                else {
                    continue;
                };
                let Some(entry_c) = self.c_type(entry, Some(span)) else {
                    continue;
                };
                let Some(entry_struct) = self.structs.get(&entry).copied() else {
                    continue;
                };
                let [path_field, name_field, type_field] = entry_struct.fields.as_slice() else {
                    continue;
                };
                let path_ty = path_field.2;
                let Some(path_c) = self.c_type(path_ty, Some(span)) else {
                    continue;
                };
                let Some(path_value_field) = self
                    .structs
                    .get(&path_ty)
                    .and_then(|s| s.fields.first())
                    .map(|f| f.0)
                else {
                    continue;
                };
                let Some(file_type_c) = self.c_type(type_field.2, Some(span)) else {
                    continue;
                };
                let (Some(file_v), Some(dir_v), Some(link_v), Some(other_v)) = (
                    self.resolved.standard_variant("FileType", "File"),
                    self.resolved.standard_variant("FileType", "Directory"),
                    self.resolved.standard_variant("FileType", "SymbolicLink"),
                    self.resolved.standard_variant("FileType", "Other"),
                ) else {
                    continue;
                };
                let none = self.option_expression(optional, None);
                let some = self.option_expression(optional, Some(("entry", entry)));
                let ok_none = self.result_expression(result, "Ok", (&none, optional));
                let ok_some = self.result_expression(result, "Ok", (&some, optional));
                let Some((error_value, error_ty)) = self.io_error_value("errno") else {
                    continue;
                };
                let error = self.result_expression(result, "Err", (&error_value, error_ty));
                let _ = writeln!(
                    self.output,
                    "static {result_c} {name}({directory_c} directory) {{struct dirent *item;struct stat st;{entry_c} entry={{0}};{path_c} path={{0}};size_t n,b,total;bool slash;uint32_t kind;if(directory==NULL||directory->closed){{errno=EBADF;return {error};}}errno=0;do{{item=readdir(directory->stream);}}while(item!=NULL&&(!strcmp(item->d_name,\".\")||!strcmp(item->d_name,\"..\")));if(item==NULL){{if(errno!=0)return {error};return {ok_none};}}if(!el_valid_utf8_cstr(item->d_name)){{errno=EINVAL;return {error};}}n=strlen(item->d_name);entry.{}=el_string_from((el_str){{item->d_name,n}});b=directory->path.length;slash=b!=0U&&directory->path.bytes[b-1U]!='/';if(n>SIZE_MAX-b-(slash?1U:0U)){{errno=ENAMETOOLONG;return {error};}}total=b+(slash?1U:0U)+n;path.{}=el_string_allocate(total);memcpy((char*)path.{}.bytes,directory->path.bytes,b);if(slash)((char*)path.{}.bytes)[b++]='/';memcpy((char*)path.{}.bytes+b,item->d_name,n);((char*)path.{}.bytes)[total]='\\0';if(lstat(path.{}.bytes,&st)!=0)return {error};kind=S_ISREG(st.st_mode)?{}U:S_ISDIR(st.st_mode)?{}U:S_ISLNK(st.st_mode)?{}U:{}U;entry.{}=({file_type_c}){{.tag=kind}};entry.{}=path;return {ok_some};}}\n",
                    field_name(name_field.0),
                    field_name(path_value_field),
                    field_name(path_value_field),
                    field_name(path_value_field),
                    field_name(path_value_field),
                    field_name(path_value_field),
                    field_name(path_value_field),
                    file_v.index(),
                    dir_v.index(),
                    link_v.index(),
                    other_v.index(),
                    field_name(type_field.0),
                    field_name(path_field.0)
                );
                continue;
            }
            if operation == SystemOperation::PathView {
                let Some(path) = arguments.first().copied() else {
                    continue;
                };
                let (Some(path_c), Some(field)) =
                    (self.c_type(path, Some(span)), self.path_field(path))
                else {
                    continue;
                };
                let access = if matches!(
                    self.typed.types.kind(self.resolve_alias(path)),
                    TypeKind::Reference { .. }
                ) {
                    "->"
                } else {
                    "."
                };
                let member = field_name(field);
                let _ = writeln!(
                    self.output,
                    "static {result_c} {name}({path_c} path) {{ return (el_str){{path{access}{member}.bytes,path{access}{member}.length}}; }}\n"
                );
                continue;
            }
            if matches!(
                operation,
                SystemOperation::CreateDir
                    | SystemOperation::RemoveDir
                    | SystemOperation::RemoveFile
            ) {
                let Some(path) = arguments.first().copied() else {
                    continue;
                };
                let Some(path_c) = self.c_type(path, Some(span)) else {
                    continue;
                };
                let Some(path_field) = self.path_field(path) else {
                    continue;
                };
                let Some((error, error_ty)) = self.io_error_value("errno") else {
                    continue;
                };
                let error = self.result_expression(result, "Err", (&error, error_ty));
                let Some(ok_variant) = self.resolved.standard_variant("Result", "Ok") else {
                    continue;
                };
                let Some(unit) = self
                    .enums
                    .get(&result)
                    .and_then(|enumeration| {
                        enumeration
                            .variants
                            .iter()
                            .find(|variant| variant.id == ok_variant)
                    })
                    .and_then(|variant| variant.fields.first())
                    .map(|field| field.2)
                else {
                    continue;
                };
                let ok = self.result_expression(result, "Ok", ("(el_unit){0}", unit));
                let path_bytes = format!("path->{}.bytes", field_name(path_field));
                let syscall = match operation {
                    SystemOperation::CreateDir => format!("mkdir({path_bytes}, 0777)"),
                    SystemOperation::RemoveDir => format!("rmdir({path_bytes})"),
                    _ => format!("unlink({path_bytes})"),
                };
                let _ = writeln!(
                    self.output,
                    "static {result_c} {name}({path_c} path) {{if(memchr({path_bytes},'\\0',path->{}.length)!=NULL){{errno=EINVAL;return {error};}}if({syscall}!=0)return {error};return {ok};}}\n",
                    field_name(path_field)
                );
                continue;
            }
            if operation == SystemOperation::Rename {
                let (Some(from), Some(to)) =
                    (arguments.first().copied(), arguments.get(1).copied())
                else {
                    continue;
                };
                let (Some(from_c), Some(to_c), Some(from_field), Some(to_field)) = (
                    self.c_type(from, Some(span)),
                    self.c_type(to, Some(span)),
                    self.path_field(from),
                    self.path_field(to),
                ) else {
                    continue;
                };
                let Some((error_value, error_ty)) = self.io_error_value("errno") else {
                    continue;
                };
                let error = self.result_expression(result, "Err", (&error_value, error_ty));
                let Some(ok_variant) = self.resolved.standard_variant("Result", "Ok") else {
                    continue;
                };
                let Some(unit) = self
                    .enums
                    .get(&result)
                    .and_then(|e| e.variants.iter().find(|v| v.id == ok_variant))
                    .and_then(|v| v.fields.first())
                    .map(|f| f.2)
                else {
                    continue;
                };
                let ok = self.result_expression(result, "Ok", ("(el_unit){0}", unit));
                let _ = writeln!(
                    self.output,
                    "static {result_c} {name}({from_c} from,{to_c} to) {{if(memchr(from->{}.bytes,'\\0',from->{}.length)!=NULL||memchr(to->{}.bytes,'\\0',to->{}.length)!=NULL){{errno=EINVAL;return {error};}}if(rename(from->{}.bytes,to->{}.bytes)!=0)return {error};return {ok};}}\n",
                    field_name(from_field),
                    field_name(from_field),
                    field_name(to_field),
                    field_name(to_field),
                    field_name(from_field),
                    field_name(to_field)
                );
                continue;
            }
        }
    }

    fn result_variant_payload(
        &self,
        result: TypeId,
        name: &str,
    ) -> Option<(TypeId, FieldId, VariantId)> {
        let variant = self.resolved.standard_variant("Result", name)?;
        let field = self
            .enums
            .get(&result)?
            .variants
            .iter()
            .find(|item| item.id == variant)?
            .fields
            .first()?;
        Some((field.2, field.0, variant))
    }

    fn emit_env_process_helpers(&mut self) {
        let process_exit = self.program.functions.iter().find_map(|function| {
            function
                .blocks
                .iter()
                .find_map(|block| match &block.terminator {
                    Terminator::NeverCall {
                        call:
                            NeverCall::Standard {
                                operation,
                                arguments,
                            },
                        span,
                    } if matches!(
                        operation,
                        StandardCall::System {
                            operation: SystemOperation::ProcessExit,
                            ..
                        }
                    ) =>
                    {
                        arguments.first().map(|argument| {
                            (
                                *operation,
                                function.temporary_types[argument.index()],
                                *span,
                            )
                        })
                    }
                    _ => None,
                })
        });
        for (call, (result, arguments, span)) in self.system_call_instances() {
            let StandardCall::System { operation, .. } = call else {
                continue;
            };
            if !matches!(
                operation,
                SystemOperation::Args
                    | SystemOperation::EnvGet
                    | SystemOperation::CurrentDir
                    | SystemOperation::ProcessRun
                    | SystemOperation::ProcessExit
            ) {
                continue;
            }
            let name = standard_call_name(call);
            let Some(result_c) = self.c_type(result, Some(span)) else {
                continue;
            };
            match operation {
                SystemOperation::Args => {
                    let TypeKind::Builtin {
                        arguments: elements,
                        ..
                    } = self.typed.types.kind(self.resolve_alias(result)).clone()
                    else {
                        continue;
                    };
                    let Some(element) = elements.first().copied() else {
                        continue;
                    };
                    let Some(element_c) = self.c_type(element, Some(span)) else {
                        continue;
                    };
                    let _ = writeln!(
                        self.output,
                        "static int el_process_argc = 0;\nstatic char **el_process_argv = NULL;\nstatic {result_c} {name}(void) {{\n    {result_c} out = {{0}}; int i;\n    if (el_process_argc <= 0) return out;\n    if ((size_t)el_process_argc > SIZE_MAX / sizeof({element_c})) el_out_of_memory();\n    out.length = out.capacity = (uintptr_t)el_process_argc;\n    out.values = ({element_c} *)el_runtime_alloc((size_t)el_process_argc * sizeof({element_c}));\n    for (i = 0; i < el_process_argc; ++i) out.values[i] = el_string_from_host_text(el_process_argv[i]);\n    return out;\n}}\n"
                    );
                }
                SystemOperation::EnvGet => {
                    let TypeKind::Nominal {
                        arguments: result_args,
                        ..
                    } = self.typed.types.kind(self.resolve_alias(result)).clone()
                    else {
                        continue;
                    };
                    let Some(option) = result_args.first().copied() else {
                        continue;
                    };
                    let TypeKind::Nominal {
                        arguments: option_args,
                        ..
                    } = self.typed.types.kind(self.resolve_alias(option)).clone()
                    else {
                        continue;
                    };
                    let Some(string_ty) = option_args.first().copied() else {
                        continue;
                    };
                    let none = self.option_expression(option, None);
                    let some = self.option_expression(
                        option,
                        Some(("el_string_from((el_str){value, strlen(value)})", string_ty)),
                    );
                    let ok_none = self.result_expression(result, "Ok", (&none, option));
                    let ok_some = self.result_expression(result, "Ok", (&some, option));
                    let error_ty = result_args.get(1).copied().unwrap_or(result);
                    let invalid_name = self
                        .resolved
                        .standard_variant("EnvError", "InvalidName")
                        .map(|variant| self.enum_unit_variant_expression(error_ty, variant))
                        .unwrap_or_else(|| {
                            zero_value(error_ty, &self.typed.types, self.program.value_model)
                        });
                    let invalid_name =
                        self.result_expression(result, "Err", (&invalid_name, error_ty));
                    let invalid_text = self
                        .resolved
                        .standard_variant("EnvError", "InvalidText")
                        .map(|variant| self.enum_unit_variant_expression(error_ty, variant))
                        .unwrap_or_else(|| {
                            zero_value(error_ty, &self.typed.types, self.program.value_model)
                        });
                    let invalid_text =
                        self.result_expression(result, "Err", (&invalid_text, error_ty));
                    let Some(argument) = arguments.first().copied() else {
                        continue;
                    };
                    let Some(argument_c) = self.c_type(argument, Some(span)) else {
                        continue;
                    };
                    let _ = writeln!(
                        self.output,
                        "static {result_c} {name}({argument_c} key) {{\n    char *name_bytes; const char *value; size_t i;\n    if (key.length == 0U) return {invalid_name};\n    for (i = 0U; i < key.length; ++i) if (key.bytes[i] == '\\0' || key.bytes[i] == '=') return {invalid_name};\n    name_bytes = (char *)el_runtime_alloc_atomic(key.length + 1U); memcpy(name_bytes, key.bytes, key.length); name_bytes[key.length] = '\\0';\n    value = getenv(name_bytes);\n    if (value == NULL) return {ok_none};\n    if (!el_valid_utf8_cstr(value)) return {invalid_text};\n    return {ok_some};\n}}\n"
                    );
                }
                SystemOperation::CurrentDir => {
                    let TypeKind::Nominal {
                        arguments: result_args,
                        ..
                    } = self.typed.types.kind(self.resolve_alias(result)).clone()
                    else {
                        continue;
                    };
                    let Some(path_ty) = result_args.first().copied() else {
                        continue;
                    };
                    let Some(path_c) = self.c_type(path_ty, Some(span)) else {
                        continue;
                    };
                    let Some(path_structure) = self.structs.get(&path_ty).copied() else {
                        continue;
                    };
                    let Some((path_field, _, _)) = path_structure.fields.first() else {
                        continue;
                    };
                    let path_value =
                        format!("({path_c}){{ .{} = owned }}", field_name(*path_field));
                    let ok = self.result_expression(result, "Ok", (&path_value, path_ty));
                    let Some((io_error, error_ty)) = self.io_error_value("errno") else {
                        continue;
                    };
                    let error = self.result_expression(result, "Err", (&io_error, error_ty));
                    let _ = writeln!(
                        self.output,
                        "static {result_c} {name}(void) {{\n    size_t size = 256U; char *bytes; el_string owned;\n    for (;;) {{ bytes = (char *)el_runtime_alloc_atomic(size); if (getcwd(bytes, size) != NULL) break; if (errno != ERANGE || size > SIZE_MAX / 2U) return {error}; size *= 2U; }}\n    if (!el_valid_utf8_cstr(bytes)) {{ errno = EINVAL; return {error}; }}\n    owned = el_string_from((el_str){{bytes, strlen(bytes)}});\n    return {ok};\n}}\n"
                    );
                }
                SystemOperation::ProcessExit => {
                    let Some(argument) = arguments.first().copied() else {
                        continue;
                    };
                    let Some(argument_c) = self.c_type(argument, Some(span)) else {
                        continue;
                    };
                    let _ = writeln!(
                        self.output,
                        "static {result_c} {name}({argument_c} code) {{ fflush(NULL); exit((int)code); }}\n"
                    );
                }
                SystemOperation::ProcessRun => {
                    let TypeKind::Nominal {
                        arguments: result_args,
                        ..
                    } = self.typed.types.kind(self.resolve_alias(result)).clone()
                    else {
                        continue;
                    };
                    let (Some(output_ty), Some(error_ty)) =
                        (result_args.first().copied(), result_args.get(1).copied())
                    else {
                        continue;
                    };
                    let Some(output_structure) = self.structs.get(&output_ty).copied() else {
                        continue;
                    };
                    let output_fields = output_structure.fields.clone();
                    if output_fields.len() != 3 {
                        continue;
                    }
                    let status_ty = output_fields[0].2;
                    let bytes_ty = output_fields[1].2;
                    let Some(status_structure) = self.structs.get(&status_ty).copied() else {
                        continue;
                    };
                    let status_fields = status_structure.fields.clone();
                    if status_fields.len() != 2 {
                        continue;
                    }
                    let code_option = status_fields[0].2;
                    let code_ty = match self
                        .typed
                        .types
                        .kind(self.resolve_alias(code_option))
                        .clone()
                    {
                        TypeKind::Nominal { arguments, .. } => arguments.first().copied(),
                        _ => None,
                    }
                    .unwrap_or(code_option);
                    let none_code = self.option_expression(code_option, None);
                    let some_code = self.option_expression(
                        code_option,
                        Some(("(int32_t)WEXITSTATUS(status)", code_ty)),
                    );
                    let Some(output_c) = self.c_type(output_ty, Some(span)) else {
                        continue;
                    };
                    let Some(status_c) = self.c_type(status_ty, Some(span)) else {
                        continue;
                    };
                    let Some(bytes_c) = self.c_type(bytes_ty, Some(span)) else {
                        continue;
                    };
                    let output_value = format!(
                        "({output_c}){{ .{} = child_status, .{} = out_bytes, .{} = err_bytes }}",
                        field_name(output_fields[0].0),
                        field_name(output_fields[1].0),
                        field_name(output_fields[2].0)
                    );
                    let ok = self.result_expression(result, "Ok", (&output_value, output_ty));
                    let mut error_result = |variant_name: &str| {
                        let value = self
                            .resolved
                            .standard_variant("ProcessError", variant_name)
                            .map(|variant| self.enum_unit_variant_expression(error_ty, variant))
                            .unwrap_or_else(|| {
                                zero_value(error_ty, &self.typed.types, self.program.value_model)
                            });
                        self.result_expression(result, "Err", (&value, error_ty))
                    };
                    let not_found = error_result("NotFound");
                    let denied = error_result("PermissionDenied");
                    let invalid = error_result("InvalidInput");
                    let unavailable = error_result("Unavailable");
                    let other = error_result("Other");
                    let (Some(program_arg), Some(slice_arg)) =
                        (arguments.first().copied(), arguments.get(1).copied())
                    else {
                        continue;
                    };
                    let (Some(program_c), Some(slice_c)) = (
                        self.c_type(program_arg, Some(span)),
                        self.c_type(slice_arg, Some(span)),
                    ) else {
                        continue;
                    };
                    let TypeKind::Reference {
                        target: path_ty, ..
                    } = self
                        .typed
                        .types
                        .kind(self.resolve_alias(program_arg))
                        .clone()
                    else {
                        continue;
                    };
                    let Some(path_structure) = self.structs.get(&path_ty).copied() else {
                        continue;
                    };
                    let Some((path_field, _, _)) = path_structure.fields.first() else {
                        continue;
                    };
                    let _ = writeln!(self.output, "static {result_c} {name}({program_c} program, {slice_c} arguments) {{
    FILE *out_file = NULL, *err_file = NULL; int exec_pipe[2], child_errno = 0, status = 0; pid_t child; char *path = NULL; char **argv = NULL; uintptr_t i; long out_size, err_size; {bytes_c} out_bytes = {{0}}, err_bytes = {{0}}; {status_c} child_status;
    if (program == NULL || memchr(program->{path_field}.bytes, '\\0', program->{path_field}.length) != NULL) return {invalid};
    if (arguments.length > (uintptr_t)(SIZE_MAX / sizeof(char *)) - 2U) return {unavailable};
    path = (char *)malloc(program->{path_field}.length + 1U); argv = (char **)calloc((size_t)arguments.length + 2U, sizeof(char *));
    if (path == NULL || argv == NULL) goto unavailable_error;
    memcpy(path, program->{path_field}.bytes, program->{path_field}.length); path[program->{path_field}.length] = '\\0'; argv[0] = path;
    for (i = 0U; i < arguments.length; ++i) {{ if (memchr(arguments.values[i].bytes, '\\0', arguments.values[i].length) != NULL) goto invalid_error; argv[i + 1U] = (char *)malloc(arguments.values[i].length + 1U); if (argv[i + 1U] == NULL) goto unavailable_error; memcpy(argv[i + 1U], arguments.values[i].bytes, arguments.values[i].length); argv[i + 1U][arguments.values[i].length] = '\\0'; }}
    out_file = tmpfile(); err_file = tmpfile(); if (out_file == NULL || err_file == NULL || pipe(exec_pipe) != 0) goto unavailable_error; (void)fcntl(exec_pipe[1], F_SETFD, FD_CLOEXEC);
    child = fork(); if (child < 0) {{ close(exec_pipe[0]); close(exec_pipe[1]); goto unavailable_error; }}
    if (child == 0) {{ close(exec_pipe[0]); if (dup2(fileno(out_file), STDOUT_FILENO) < 0 || dup2(fileno(err_file), STDERR_FILENO) < 0) {{ child_errno = errno; if (write(exec_pipe[1], &child_errno, sizeof(child_errno)) < 0) child_errno = errno; _exit(126); }} execv(path, argv); child_errno = errno; if (write(exec_pipe[1], &child_errno, sizeof(child_errno)) < 0) child_errno = errno; _exit(127); }}
    close(exec_pipe[1]); {{ ssize_t count; do {{ count = read(exec_pipe[0], &child_errno, sizeof(child_errno)); }} while (count < 0 && errno == EINTR); if (count < 0 || (count != 0 && count != (ssize_t)sizeof(child_errno))) child_errno = EIO; else if (count == 0) child_errno = 0; }} close(exec_pipe[0]); {{ pid_t waited; do {{ waited = waitpid(child, &status, 0); }} while (waited < 0 && errno == EINTR); if (waited < 0 && child_errno == 0) child_errno = EIO; }}
    for (i = 0U; i < arguments.length; ++i) {{ free(argv[i + 1U]); }}
    free(argv); argv = NULL; free(path); path = NULL;
    if (child_errno != 0) {{ fclose(out_file); fclose(err_file); if (child_errno == ENOENT || child_errno == ENOTDIR) return {not_found}; if (child_errno == EACCES || child_errno == EPERM) return {denied}; if (child_errno == EINVAL || child_errno == ENAMETOOLONG || child_errno == ELOOP) return {invalid}; if (child_errno == EAGAIN || child_errno == ENOMEM || child_errno == EMFILE || child_errno == ENFILE) return {unavailable}; return {other}; }}
    if (fseek(out_file, 0L, SEEK_END) != 0 || (out_size = ftell(out_file)) < 0L || fseek(out_file, 0L, SEEK_SET) != 0 || fseek(err_file, 0L, SEEK_END) != 0 || (err_size = ftell(err_file)) < 0L || fseek(err_file, 0L, SEEK_SET) != 0) {{ fclose(out_file); fclose(err_file); return {other}; }}
    if (out_size != 0L) {{ out_bytes.values = (uint8_t *)el_runtime_alloc_atomic((size_t)out_size); if (fread(out_bytes.values, 1U, (size_t)out_size, out_file) != (size_t)out_size) {{ fclose(out_file); fclose(err_file); return {other}; }} out_bytes.length = out_bytes.capacity = (uintptr_t)out_size; }}
    if (err_size != 0L) {{ err_bytes.values = (uint8_t *)el_runtime_alloc_atomic((size_t)err_size); if (fread(err_bytes.values, 1U, (size_t)err_size, err_file) != (size_t)err_size) {{ fclose(out_file); fclose(err_file); return {other}; }} err_bytes.length = err_bytes.capacity = (uintptr_t)err_size; }} fclose(out_file); fclose(err_file);
    child_status = ({status_c}){{ .{code_field} = WIFEXITED(status) ? {some_code} : {none_code}, .{success_field} = WIFEXITED(status) && WEXITSTATUS(status) == 0 }}; return {ok};
invalid_error:
    if (argv != NULL) {{ for (i = 0U; i < arguments.length; ++i) free(argv[i + 1U]); }} free(argv); free(path); if (out_file != NULL) fclose(out_file); if (err_file != NULL) fclose(err_file); return {invalid};
unavailable_error:
    if (argv != NULL) {{ for (i = 0U; i < arguments.length; ++i) free(argv[i + 1U]); }} free(argv); free(path); if (out_file != NULL) fclose(out_file); if (err_file != NULL) fclose(err_file); return {unavailable};
}}
", path_field = field_name(*path_field), code_field = field_name(status_fields[0].0), success_field = field_name(status_fields[1].0));
                }
                _ => {}
            }
        }
        if let Some((call, argument, span)) = process_exit
            && let Some(argument_c) = self.c_type(argument, Some(span))
        {
            let _ = writeln!(
                self.output,
                "static void {}({argument_c} code) {{ fflush(NULL); exit((int)code); }}\n",
                standard_call_name(call)
            );
        }
    }

    fn path_field(&self, path: TypeId) -> Option<FieldId> {
        let path = match self.typed.types.kind(self.resolve_alias(path)) {
            TypeKind::Reference { target, .. } => self.resolve_alias(*target),
            _ => self.resolve_alias(path),
        };
        self.structs.get(&path)?.fields.first().map(|field| field.0)
    }

    fn io_error_value(&mut self, code: &str) -> Option<(String, TypeId)> {
        let declaration = self.resolved.standard_declaration("IoError")?;
        let ty = *self.typed.declaration_types.get(&declaration)?;
        Some((format!("el_io_error({code})"), ty))
    }

    fn emit_io_error_mapper(&mut self) {
        let Some(declaration) = self.resolved.standard_declaration("IoError") else {
            return;
        };
        let Some(ty) = self.typed.declaration_types.get(&declaration).copied() else {
            return;
        };
        let Some(c_type) = self.c_type(ty, None) else {
            return;
        };
        let variant = |name| {
            self.resolved
                .standard_variant("IoError", name)
                .map(|id| id.index())
        };
        let (
            Some(not_found),
            Some(permission),
            Some(exists),
            Some(invalid),
            Some(is_dir),
            Some(not_dir),
            Some(not_empty),
            Some(read_only),
            Some(pipe),
            Some(interrupted),
            Some(would_block),
            Some(timed_out),
            Some(storage),
            Some(resources),
            Some(unsupported),
            Some(other),
        ) = (
            variant("NotFound"),
            variant("PermissionDenied"),
            variant("AlreadyExists"),
            variant("InvalidInput"),
            variant("IsDirectory"),
            variant("NotDirectory"),
            variant("DirectoryNotEmpty"),
            variant("ReadOnly"),
            variant("BrokenPipe"),
            variant("Interrupted"),
            variant("WouldBlock"),
            variant("TimedOut"),
            variant("StorageFull"),
            variant("ResourceExhausted"),
            variant("Unsupported"),
            variant("Other"),
        )
        else {
            return;
        };
        let _ = writeln!(
            self.output,
            "static {c_type} el_io_error(int code) {{ uint32_t tag; switch(code) {{ case ENOENT: tag={not_found}U; break; case EACCES: case EPERM: tag={permission}U; break; case EEXIST: tag={exists}U; break; case EBADF: case EINVAL: case ENAMETOOLONG: case ELOOP: tag={invalid}U; break; case EISDIR: tag={is_dir}U; break; case ENOTDIR: tag={not_dir}U; break; case ENOTEMPTY: tag={not_empty}U; break; case EROFS: tag={read_only}U; break; case EPIPE: tag={pipe}U; break; case EINTR: tag={interrupted}U; break; case EAGAIN: tag={would_block}U; break; case ETIMEDOUT: tag={timed_out}U; break; case ENOSPC: case EDQUOT: case EFBIG: tag={storage}U; break; case EMFILE: case ENFILE: case ENOMEM: case ENOBUFS: tag={resources}U; break; case ENOSYS: case EOPNOTSUPP: case EXDEV: tag={unsupported}U; break; default: tag={other}U; break; }} return ({c_type}){{.tag=tag}}; }}\n"
        );
    }

    fn emit_text_helpers(&mut self, calls: &BTreeMap<StandardCall, (TypeId, Span)>) {
        let text_calls = calls
            .iter()
            .filter(|(call, _)| matches!(call, StandardCall::Text { .. }))
            .collect::<Vec<_>>();
        if text_calls.is_empty() {
            return;
        }
        if text_calls.iter().any(|(call, _)| {
            matches!(
                call,
                StandardCall::Text {
                    operation: TextOperation::NextScalar,
                    ..
                }
            )
        }) {
            self.output.push_str(
                "static uint32_t el_utf8_decode(const char *p, size_t n, size_t *width) { unsigned char a = (unsigned char)p[0]; if (a < 0x80U) { *width=1U; return a; } if (a < 0xE0U && n>=2U) { *width=2U; return ((uint32_t)(a&31U)<<6)|((unsigned char)p[1]&63U); } if (a < 0xF0U && n>=3U) { *width=3U; return ((uint32_t)(a&15U)<<12)|((uint32_t)((unsigned char)p[1]&63U)<<6)|((unsigned char)p[2]&63U); } *width=4U; return ((uint32_t)(a&7U)<<18)|((uint32_t)((unsigned char)p[1]&63U)<<12)|((uint32_t)((unsigned char)p[2]&63U)<<6)|((unsigned char)p[3]&63U); }\n\n",
            );
        }
        if text_calls.iter().any(|(call, _)| {
            matches!(
                call,
                StandardCall::Text {
                    operation: TextOperation::FromChars,
                    ..
                }
            )
        }) {
            self.output.push_str(
                "static size_t el_utf8_encoded_width(uint32_t value) { return value < UINT32_C(0x80) ? 1U : value < UINT32_C(0x800) ? 2U : value < UINT32_C(0x10000) ? 3U : 4U; }\n\
                 static size_t el_utf8_encode(char *out, uint32_t value) { if(value<UINT32_C(0x80)){out[0]=(char)value;return 1U;} if(value<UINT32_C(0x800)){out[0]=(char)(0xC0U|(value>>6));out[1]=(char)(0x80U|(value&0x3FU));return 2U;} if(value<UINT32_C(0x10000)){out[0]=(char)(0xE0U|(value>>12));out[1]=(char)(0x80U|((value>>6)&0x3FU));out[2]=(char)(0x80U|(value&0x3FU));return 3U;} out[0]=(char)(0xF0U|(value>>18));out[1]=(char)(0x80U|((value>>12)&0x3FU));out[2]=(char)(0x80U|((value>>6)&0x3FU));out[3]=(char)(0x80U|(value&0x3FU));return 4U;}\n\n",
            );
        }
        for (call, (result, _)) in text_calls {
            let StandardCall::Text {
                operation,
                result_type,
                input_type,
            } = call
            else {
                continue;
            };
            let name = standard_call_name(*call);
            let Some(result_c) = self.c_type(*result, None) else {
                continue;
            };
            match operation {
                TextOperation::ByteLen => {
                    let _ = writeln!(
                        self.output,
                        "static {result_c} {name}(el_str text) {{ return (uintptr_t)text.length; }}\n"
                    );
                }
                TextOperation::StringView => {
                    let _ = writeln!(
                        self.output,
                        "static {result_c} {name}(el_string text) {{ return (el_str){{text.bytes,text.length}}; }}\n"
                    );
                }
                TextOperation::SliceBytes => {
                    let none = self.option_expression(*result_type, None);
                    let str_type = self.typed.types.primitive_id(PrimitiveType::Str);
                    let some = self.option_expression(*result_type, Some(("view", str_type)));
                    let _ = writeln!(
                        self.output,
                        "static {result_c} {name}(el_str text, uintptr_t start, uintptr_t end) {{ el_str view; if(start>end||end>(uintptr_t)text.length) return {none}; if((start<(uintptr_t)text.length&&(((unsigned char)text.bytes[start]&0xC0U)==0x80U))||(end<(uintptr_t)text.length&&(((unsigned char)text.bytes[end]&0xC0U)==0x80U))) return {none}; view=(el_str){{text.bytes+start,(size_t)(end-start)}}; return {some}; }}\n"
                    );
                }
                TextOperation::NextScalar => {
                    let TypeKind::Nominal { arguments, .. } =
                        self.typed.types.kind(*result_type).clone()
                    else {
                        continue;
                    };
                    let [step_type] = arguments.as_slice() else {
                        continue;
                    };
                    let Some(fields) = self
                        .structs
                        .get(step_type)
                        .map(|structure| structure.fields.clone())
                    else {
                        continue;
                    };
                    let Some((value_field, _, _)) =
                        fields.iter().find(|(_, field, _)| field == "value")
                    else {
                        continue;
                    };
                    let Some((next_field, _, _)) =
                        fields.iter().find(|(_, field, _)| field == "next")
                    else {
                        continue;
                    };
                    let Some(step_c) = self.c_type(*step_type, None) else {
                        continue;
                    };
                    let step = format!(
                        "({step_c}){{ .{} = value, .{} = (uintptr_t)(start + width) }}",
                        field_name(*value_field),
                        field_name(*next_field)
                    );
                    let none = self.option_expression(*result_type, None);
                    let some = self.option_expression(*result_type, Some((&step, *step_type)));
                    let _ = writeln!(
                        self.output,
                        "static {result_c} {name}(el_str text, uintptr_t start) {{ size_t width; uint32_t value; if(start>=(uintptr_t)text.length||(((unsigned char)text.bytes[start]&0xC0U)==0x80U)) return {none}; value=el_utf8_decode(text.bytes+start,text.length-(size_t)start,&width); if(start>UINTPTR_MAX-width||start+width>(uintptr_t)text.length) return {none}; return {some}; }}\n"
                    );
                }
                TextOperation::FromChars => {
                    let Some(input_c) = self.c_type(*input_type, None) else {
                        continue;
                    };
                    let _ = writeln!(
                        self.output,
                        "static {result_c} {name}({input_c} chars) {{ uintptr_t i; size_t total=0U,written=0U,width; el_string out; for(i=0U;i<chars.length;++i){{width=el_utf8_encoded_width(chars.values[i]);if(width>SIZE_MAX-total)el_out_of_memory();total+=width;}} out=el_string_allocate(total);for(i=0U;i<chars.length;++i)written+=el_utf8_encode((char*)out.bytes+written,chars.values[i]);((char*)out.bytes)[written]='\\0';return out; }}\n"
                    );
                }
            }
        }
    }

    fn emit_clock_helpers(&mut self, calls: &BTreeMap<StandardCall, (TypeId, Span)>) {
        for (operation, (result, span)) in calls {
            let StandardCall::ClockNow {
                clock_type,
                monotonic,
            } = operation
            else {
                continue;
            };
            let clock_type = self.resolve_alias(*clock_type);
            let Some(structure) = self.structs.get(&clock_type) else {
                self.type_error(
                    clock_type,
                    Some(*span),
                    "a standard clock result must be a one-field source-backed struct",
                );
                continue;
            };
            let Some((field, _, field_type)) = structure.fields.first() else {
                self.type_error(
                    clock_type,
                    Some(*span),
                    "a standard clock result must contain its nanosecond field",
                );
                continue;
            };
            if structure.fields.len() != 1
                || self.typed.types.expanded_primitive(*field_type) != Some(PrimitiveType::U64)
            {
                self.type_error(
                    clock_type,
                    Some(*span),
                    "a standard clock result must contain exactly one `u64` field",
                );
                continue;
            }
            let Some(result_type) = self.c_type(*result, Some(*span)) else {
                continue;
            };
            let clock_id = if *monotonic {
                "CLOCK_MONOTONIC"
            } else {
                "CLOCK_REALTIME"
            };
            let name = standard_call_name(*operation);
            let member = field_name(*field);
            let _ = writeln!(
                self.output,
                "static {result_type} {name}(const char *path, uint32_t line, uint32_t column) {{\n\
                     struct timespec reading;\n\
                     uint64_t seconds;\n\
                     uint64_t nanoseconds;\n\
                     if (clock_gettime({clock_id}, &reading) != 0 || reading.tv_sec < 0) el_trap(\"E-RUN-CLOCK\", path, line, column);\n\
                     seconds = (uint64_t)reading.tv_sec;\n\
                     if (seconds > UINT64_MAX / UINT64_C(1000000000)) el_trap(\"E-RUN-OVERFLOW\", path, line, column);\n\
                     nanoseconds = seconds * UINT64_C(1000000000);\n\
                     if ((uint64_t)reading.tv_nsec > UINT64_MAX - nanoseconds) el_trap(\"E-RUN-OVERFLOW\", path, line, column);\n\
                     return ({result_type}){{ .{member} = nanoseconds + (uint64_t)reading.tv_nsec }};\n\
                 }}\n"
            );
        }
    }

    fn emit_thread_helpers(&mut self, calls: &BTreeMap<StandardCall, (TypeId, Span)>) {
        // POSIX thread creation and join are the concrete release/acquire
        // boundaries for the source-level start and completion edges in
        // `docs/spec.md` 10.4. Keep result publication before entry return and
        // result consumption after `pthread_join`.
        let thread_calls = calls
            .iter()
            .filter(|(operation, _)| {
                matches!(
                    operation,
                    StandardCall::ThreadSpawn { .. }
                        | StandardCall::ThreadJoin { .. }
                        | StandardCall::ThreadIsFinished { .. }
                )
            })
            .collect::<Vec<_>>();
        if thread_calls.is_empty() {
            return;
        }
        self.output.push_str(
            "typedef void (*el_thread_shutdown_join)(void *);\n\
             typedef struct el_thread_registry_node {\n\
             \x20\x20\x20\x20void *state;\n\
             \x20\x20\x20\x20el_thread_shutdown_join join;\n\
             \x20\x20\x20\x20struct el_thread_registry_node *next;\n\
             } el_thread_registry_node;\n\
             static pthread_mutex_t el_thread_registry_lock = PTHREAD_MUTEX_INITIALIZER;\n\
             static el_thread_registry_node *el_thread_registry = NULL;\n\
             static void el_thread_register(void *state, el_thread_shutdown_join join) {\n\
             \x20\x20\x20\x20el_thread_registry_node *node = (el_thread_registry_node *)GC_MALLOC(sizeof(*node));\n\
             \x20\x20\x20\x20if (node == NULL) el_out_of_memory();\n\
             \x20\x20\x20\x20node->state = state; node->join = join;\n\
             \x20\x20\x20\x20(void)pthread_mutex_lock(&el_thread_registry_lock);\n\
             \x20\x20\x20\x20node->next = el_thread_registry; el_thread_registry = node;\n\
             \x20\x20\x20\x20(void)pthread_mutex_unlock(&el_thread_registry_lock);\n\
             }\n\
             static void el_thread_unregister(void *state) {\n\
             \x20\x20\x20\x20el_thread_registry_node *node;\n\
             \x20\x20\x20\x20(void)pthread_mutex_lock(&el_thread_registry_lock);\n\
             \x20\x20\x20\x20for (node = el_thread_registry; node != NULL; node = node->next) { if (node->state == state) { node->state = NULL; node->join = NULL; break; } }\n\
             \x20\x20\x20\x20(void)pthread_mutex_unlock(&el_thread_registry_lock);\n\
             }\n\
             void el_thread_shutdown_all(void) {\n\
             \x20\x20\x20\x20for (;;) {\n\
             \x20\x20\x20\x20\x20\x20\x20\x20el_thread_registry_node *node; void *state = NULL; el_thread_shutdown_join join = NULL;\n\
             \x20\x20\x20\x20\x20\x20\x20\x20(void)pthread_mutex_lock(&el_thread_registry_lock);\n\
             \x20\x20\x20\x20\x20\x20\x20\x20for (node = el_thread_registry; node != NULL; node = node->next) { if (node->state != NULL) { state = node->state; join = node->join; break; } }\n\
             \x20\x20\x20\x20\x20\x20\x20\x20(void)pthread_mutex_unlock(&el_thread_registry_lock);\n\
             \x20\x20\x20\x20\x20\x20\x20\x20if (state == NULL) break;\n\
             \x20\x20\x20\x20\x20\x20\x20\x20join(state);\n\
             \x20\x20\x20\x20}\n\
             }\n\n",
        );
        let mut emitted = BTreeSet::new();
        for (operation, (_, span)) in &thread_calls {
            let thread = match operation {
                StandardCall::ThreadSpawn { thread, .. }
                | StandardCall::ThreadJoin { thread, .. }
                | StandardCall::ThreadIsFinished { thread } => *thread,
                _ => continue,
            };
            if !emitted.insert(thread) {
                continue;
            }
            let return_type = calls
                .keys()
                .find_map(|operation| match operation {
                    StandardCall::ThreadSpawn {
                        thread: candidate,
                        return_type,
                        ..
                    }
                    | StandardCall::ThreadJoin {
                        thread: candidate,
                        return_type,
                    } if *candidate == thread => Some(*return_type),
                    _ => None,
                })
                .or_else(|| {
                    let TypeKind::Builtin { arguments, .. } =
                        self.typed.types.kind(self.resolve_alias(thread))
                    else {
                        return None;
                    };
                    arguments.first().copied()
                });
            let Some(return_type) = return_type else {
                continue;
            };
            let needs_join = calls.keys().any(|operation| {
                matches!(operation, StandardCall::ThreadJoin { thread: candidate, .. } if *candidate == thread)
            });
            let needs_finished = calls.keys().any(|operation| {
                matches!(operation, StandardCall::ThreadIsFinished { thread: candidate } if *candidate == thread)
            });
            self.emit_thread_type_helpers(thread, return_type, *span, needs_join, needs_finished);
            let _ = span;
        }
        for (operation, (result, span)) in thread_calls {
            if matches!(operation, StandardCall::ThreadSpawn { .. }) {
                self.emit_thread_spawn_helper(*operation, *result, *span);
            }
        }
    }

    fn emit_thread_type_helpers(
        &mut self,
        thread: TypeId,
        return_type: TypeId,
        span: Span,
        needs_join: bool,
        needs_finished: bool,
    ) {
        let Some(thread_c) = self.c_type(thread, Some(span)) else {
            return;
        };
        let never = matches!(
            self.typed.types.kind(self.resolve_alias(return_type)),
            TypeKind::Never
        );
        let return_c = if never {
            "el_unit".to_string()
        } else {
            let Some(return_c) = self.c_type(return_type, Some(span)) else {
                return;
            };
            return_c
        };
        let state_name = format!("{}_data", collection_type_name(thread));
        let join_name = standard_call_name(StandardCall::ThreadJoin {
            thread,
            return_type,
        });
        let finished_name = standard_call_name(StandardCall::ThreadIsFinished { thread });
        let _ = writeln!(
            self.output,
            "struct {state_name} {{\n    pthread_t thread;\n    pthread_mutex_t join_lock;\n    pthread_mutex_t status_lock;\n    bool joined;\n    bool finished;\n    void *startup;\n    {return_c} result;\n}};\n\
             static void {join_name}_shutdown(void *raw) {{\n    {thread_c} state = ({thread_c})raw;\n    (void)pthread_mutex_lock(&state->join_lock);\n    if (!state->joined) {{ (void)pthread_join(state->thread, NULL); state->joined = true; }}\n    (void)pthread_mutex_unlock(&state->join_lock);\n    el_thread_unregister(state);\n}}\n"
        );
        if needs_join {
            if !never {
                let _ = writeln!(
                    self.output,
                    "static {return_c} {join_name}({thread_c} state, const char *path, uint32_t line, uint32_t column) {{\n    if (pthread_equal(pthread_self(), state->thread)) el_trap(\"E-RUN-SELF-JOIN\", path, line, column);\n    {join_name}_shutdown(state);\n    return state->result;\n}}\n"
                );
            } else {
                let _ = writeln!(
                    self.output,
                    "static void {join_name}({thread_c} state, const char *path, uint32_t line, uint32_t column) {{\n    if (pthread_equal(pthread_self(), state->thread)) el_trap(\"E-RUN-SELF-JOIN\", path, line, column);\n    {join_name}_shutdown(state);\n    abort();\n}}\n"
                );
            }
        }
        if needs_finished {
            let _ = writeln!(
                self.output,
                "static bool {finished_name}({thread_c} state) {{\n    bool finished; (void)pthread_mutex_lock(&state->status_lock); finished = state->finished; (void)pthread_mutex_unlock(&state->status_lock); return finished;\n}}\n"
            );
        }
    }

    fn emit_thread_spawn_helper(&mut self, operation: StandardCall, result: TypeId, span: Span) {
        let StandardCall::ThreadSpawn {
            thread,
            callable,
            entry,
            closure_entry,
            return_type,
        } = operation
        else {
            return;
        };
        let (Some(thread_c), Some(callable_c), Some(result_c)) = (
            self.c_type(thread, Some(span)),
            self.c_type(callable, Some(span)),
            self.c_type(result, Some(span)),
        ) else {
            return;
        };
        let entry_symbol = self.function_symbol(&FunctionInstance {
            declaration: entry,
            arguments: Vec::new(),
            self_type: None,
        });
        let call = if closure_entry {
            format!("{entry_symbol}(context->body)")
        } else {
            format!("{entry_symbol}()")
        };
        let publish_result = if matches!(
            self.typed.types.kind(self.resolve_alias(return_type)),
            TypeKind::Never
        ) {
            format!("{call}; abort();")
        } else if matches!(
            self.typed.types.kind(self.resolve_alias(return_type)),
            TypeKind::Primitive(PrimitiveType::Unit)
        ) {
            format!("{call}; state->result = (el_unit){{0}};")
        } else {
            format!("state->result = {call};")
        };
        let Some(ok_variant) = self.resolved.standard_variant("Result", "Ok") else {
            return;
        };
        let Some(err_variant) = self.resolved.standard_variant("Result", "Err") else {
            return;
        };
        let (Some(ok_field), Some(err_field)) = (
            self.resolved.variants[ok_variant.index()].fields.first(),
            self.resolved.variants[err_variant.index()].fields.first(),
        ) else {
            return;
        };
        let Some(error_decl) = self.resolved.standard_declaration("SpawnError") else {
            return;
        };
        let Some(error_ty) = self.typed.declaration_types.get(&error_decl).copied() else {
            return;
        };
        let Some(error_c) = self.c_type(error_ty, Some(span)) else {
            return;
        };
        let Some(unavailable) = self.resolved.standard_variant("SpawnError", "Unavailable") else {
            return;
        };
        let spawn_name = standard_call_name(operation);
        let context_name = format!("{spawn_name}_context");
        let join_name = standard_call_name(StandardCall::ThreadJoin {
            thread,
            return_type,
        });
        let _ = writeln!(
            self.output,
            "typedef struct {context_name} {{ {thread_c} state; {callable_c} body; }} {context_name};\n\
             static void *{spawn_name}_entry(void *raw) {{\n    {context_name} *context = ({context_name} *)raw;\n    {thread_c} state = context->state;\n    state->startup = NULL;\n    {publish_result}\n    (void)pthread_mutex_lock(&state->status_lock); state->finished = true; (void)pthread_mutex_unlock(&state->status_lock);\n    return NULL;\n}}\n\
             static {result_c} {spawn_name}({callable_c} body) {{\n    {thread_c} state; {context_name} *context; {result_c} outcome = {{0}}; int status;"
        );
        self.emit_runtime_allocate("state", "sizeof(*state)", AllocationClass::Scanned);
        self.emit_runtime_allocate("context", "sizeof(*context)", AllocationClass::Scanned);
        let _ = writeln!(
            self.output,
            "    state->joined = false; state->finished = false; context->state = state; context->body = body; state->startup = context;\n    (void)pthread_mutex_init(&state->join_lock, NULL); (void)pthread_mutex_init(&state->status_lock, NULL);\n    status = pthread_create(&state->thread, NULL, {spawn_name}_entry, context);\n    if (status != 0) {{\n        state->startup = NULL; (void)pthread_mutex_destroy(&state->join_lock); (void)pthread_mutex_destroy(&state->status_lock);\n        outcome.tag = UINT32_C({});\n        outcome.payload.{}.{} = ({error_c}){{ .tag = UINT32_C({}) }};\n        return outcome;\n    }}\n    el_thread_register(state, {join_name}_shutdown);\n    outcome.tag = UINT32_C({});\n    outcome.payload.{}.{} = state;\n    return outcome;\n}}\n",
            err_variant.index(),
            variant_member_name(err_variant),
            field_name(*err_field),
            unavailable.index(),
            ok_variant.index(),
            variant_member_name(ok_variant),
            field_name(*ok_field),
        );
    }

    fn channel_error_expression(
        &mut self,
        error_name: &str,
        variant_name: &str,
    ) -> Option<(String, TypeId)> {
        let declaration = self.resolved.standard_declaration(error_name)?;
        let ty = *self.typed.declaration_types.get(&declaration)?;
        let variant = self.resolved.standard_variant(error_name, variant_name)?;
        Some((self.enum_unit_variant_expression(ty, variant), ty))
    }

    fn emit_channel_helpers(&mut self, calls: &BTreeMap<StandardCall, (TypeId, Span)>) {
        // The queue mutex publishes writes sequenced before a successful send
        // to the matching receiver. Condition variables provide blocking and
        // wakeup only; the mutex unlock/lock pair owns the ordering edge.
        let mut elements = BTreeSet::new();
        for operation in calls.keys() {
            match operation {
                StandardCall::ChannelCreate { element, .. }
                | StandardCall::ChannelSend { element, .. }
                | StandardCall::ChannelReceive { element, .. } => {
                    elements.insert(self.resolve_alias(*element));
                }
                StandardCall::ChannelClose { handle, .. } => {
                    if let TypeKind::Builtin { arguments, .. } =
                        self.typed.types.kind(self.resolve_alias(*handle))
                        && let Some(element) = arguments.first()
                    {
                        elements.insert(self.resolve_alias(*element));
                    }
                }
                _ => {}
            }
        }
        for element in elements {
            self.emit_channel_type_helpers(element, calls);
        }
    }

    fn emit_channel_type_helpers(
        &mut self,
        element: TypeId,
        calls: &BTreeMap<StandardCall, (TypeId, Span)>,
    ) {
        let Some(element_c) = self.c_type(element, None) else {
            return;
        };
        let creates = calls
            .iter()
            .filter_map(|(operation, result)| match operation {
                StandardCall::ChannelCreate {
                    sender,
                    receiver,
                    element: candidate,
                    bounded,
                } if self.resolve_alias(*candidate) == element => {
                    Some((*operation, *sender, *receiver, *bounded, *result))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let sender = creates
            .first()
            .map(|(_, sender, _, _, _)| *sender)
            .or_else(|| {
                calls.keys().find_map(|operation| match operation {
                    StandardCall::ChannelSend { sender, .. } => Some(*sender),
                    StandardCall::ChannelClose {
                        handle,
                        sender: true,
                    } => Some(*handle),
                    _ => None,
                })
            });
        let receiver = creates
            .first()
            .map(|(_, _, receiver, _, _)| *receiver)
            .or_else(|| {
                calls.keys().find_map(|operation| match operation {
                    StandardCall::ChannelReceive { receiver, .. } => Some(*receiver),
                    StandardCall::ChannelClose {
                        handle,
                        sender: false,
                    } => Some(*handle),
                    _ => None,
                })
            });
        if sender.is_none() && receiver.is_none() {
            return;
        }
        let sender_c = sender.and_then(|sender| self.c_type(sender, None));
        let receiver_c = receiver.and_then(|receiver| self.c_type(receiver, None));
        let state = format!("el_channel_t{}_data", element.index());
        let node = format!("el_channel_t{}_node", element.index());
        let _ = writeln!(
            self.output,
            "typedef struct {node} {{ {element_c} value; struct {node} *next; }} {node};\n\
             struct {state} {{\n    pthread_mutex_t lock;\n    pthread_cond_t readable;\n    \
             pthread_cond_t writable;\n    uintptr_t capacity;\n    uintptr_t count;\n    \
             uintptr_t waiting_receivers;\n    bool unbounded;\n    bool sender_closed;\n    \
             bool receiver_closed;\n    {node} *head;\n    {node} *tail;\n}};\n"
        );

        for (operation, _, _, bounded, (result, _)) in creates {
            let (Some(sender_c), Some(receiver_c)) = (&sender_c, &receiver_c) else {
                continue;
            };
            let Some(result_c) = self.c_type(result, None) else {
                continue;
            };
            let name = standard_call_name(operation);
            let parameters = if bounded {
                "uintptr_t capacity"
            } else {
                "void"
            };
            let capacity = if bounded { "capacity" } else { "UINTPTR_MAX" };
            let unbounded = if bounded { "false" } else { "true" };
            let _ = writeln!(
                self.output,
                "static {result_c} {name}({parameters}) {{\n    {sender_c} channel;\n    \
                 {result_c} result = {{0}};"
            );
            self.emit_runtime_allocate("channel", "sizeof(*channel)", AllocationClass::Scanned);
            let _ = writeln!(
                self.output,
                "    (void)pthread_mutex_init(&channel->lock, NULL);\n    \
                 (void)pthread_cond_init(&channel->readable, NULL);\n    \
                 (void)pthread_cond_init(&channel->writable, NULL);\n    \
                 channel->capacity = {capacity}; channel->count = 0U; \
                 channel->waiting_receivers = 0U; channel->unbounded = {unbounded};\n    \
                 channel->sender_closed = false; channel->receiver_closed = false;\n    \
                 channel->head = NULL; channel->tail = NULL;\n    result.v0 = channel;\n    \
                 result.v1 = ({receiver_c})channel;\n    return result;\n}}\n"
            );
        }

        for (operation, (result, _)) in calls {
            let StandardCall::ChannelSend {
                sender: candidate,
                element: candidate_element,
                nonblocking,
            } = operation
            else {
                continue;
            };
            if self.resolve_alias(*candidate_element) != element {
                continue;
            }
            let Some(result_c) = self.c_type(*result, None) else {
                continue;
            };
            let Some((closed_value, closed_ty)) = self.channel_error_expression(
                if *nonblocking {
                    "TrySendError"
                } else {
                    "SendError"
                },
                "Closed",
            ) else {
                continue;
            };
            let closed = self.result_expression(*result, "Err", (&closed_value, closed_ty));
            let Some(unit) = self
                .typed
                .types
                .id_for_kind(&TypeKind::Primitive(PrimitiveType::Unit))
            else {
                continue;
            };
            let ok = self.result_expression(*result, "Ok", ("(el_unit){0}", unit));
            let full = if *nonblocking {
                let Some((full_value, full_ty)) =
                    self.channel_error_expression("TrySendError", "Full")
                else {
                    continue;
                };
                Some(self.result_expression(*result, "Err", (&full_value, full_ty)))
            } else {
                None
            };
            let name = standard_call_name(*operation);
            let sender_type = self
                .c_type(*candidate, None)
                .or_else(|| sender_c.clone())
                .unwrap_or_else(|| "void *".to_string());
            let _ = writeln!(
                self.output,
                "static {result_c} {name}({sender_type} channel, {element_c} value) {{\n    \
                 {node} *message;\n    (void)pthread_mutex_lock(&channel->lock);\n    \
                 if (channel->sender_closed || channel->receiver_closed) {{ \
                 (void)pthread_mutex_unlock(&channel->lock); return {closed}; }}"
            );
            if let Some(full) = full {
                let _ = writeln!(
                    self.output,
                    "    if (!channel->unbounded && ((channel->capacity == 0U && \
                     (channel->waiting_receivers == 0U || channel->count != 0U)) || \
                     (channel->capacity != 0U && channel->count >= channel->capacity))) {{ \
                     (void)pthread_mutex_unlock(&channel->lock); return {full}; }}"
                );
            } else {
                self.output.push_str(
                    "    while (!channel->sender_closed && !channel->receiver_closed && \
                     !channel->unbounded && ((channel->capacity == 0U && \
                     (channel->waiting_receivers == 0U || channel->count != 0U)) || \
                     (channel->capacity != 0U && channel->count >= channel->capacity))) \
                     (void)pthread_cond_wait(&channel->writable, &channel->lock);\n    \
                     if (channel->sender_closed || channel->receiver_closed) { \
                     (void)pthread_mutex_unlock(&channel->lock); return ",
                );
                self.output.push_str(&closed);
                self.output.push_str("; }\n");
            }
            let _ = writeln!(
                self.output,
                "    message = ({node} *)el_runtime_alloc(sizeof(*message));\n    \
                 if (message == NULL) el_out_of_memory();\n    message->value = value; \
                 message->next = NULL;\n    if (channel->tail == NULL) channel->head = message; \
                 else channel->tail->next = message;\n    channel->tail = message; \
                 ++channel->count;\n    (void)pthread_cond_signal(&channel->readable);"
            );
            if !*nonblocking {
                self.output.push_str(
                    "    if (channel->capacity == 0U) while (channel->count != 0U && \
                     !channel->receiver_closed) (void)pthread_cond_wait(&channel->writable, \
                     &channel->lock);\n",
                );
            }
            let _ = writeln!(
                self.output,
                "    (void)pthread_mutex_unlock(&channel->lock);\n    return {ok};\n}}\n"
            );
        }

        for (operation, (result, _)) in calls {
            let StandardCall::ChannelReceive {
                receiver: candidate,
                element: candidate_element,
                nonblocking,
            } = operation
            else {
                continue;
            };
            if self.resolve_alias(*candidate_element) != element {
                continue;
            }
            let Some(result_c) = self.c_type(*result, None) else {
                continue;
            };
            let name = standard_call_name(*operation);
            let receiver_type = self
                .c_type(*candidate, None)
                .or_else(|| receiver_c.clone())
                .unwrap_or_else(|| "void *".to_string());
            let _ = writeln!(
                self.output,
                "static {result_c} {name}({receiver_type} channel) {{\n    {node} *message; \
                 {element_c} value;\n    (void)pthread_mutex_lock(&channel->lock);"
            );
            if *nonblocking {
                let Some((empty_value, empty_ty)) =
                    self.channel_error_expression("TryReceiveError", "Empty")
                else {
                    continue;
                };
                let Some((closed_value, closed_ty)) =
                    self.channel_error_expression("TryReceiveError", "Closed")
                else {
                    continue;
                };
                let empty = self.result_expression(*result, "Err", (&empty_value, empty_ty));
                let closed = self.result_expression(*result, "Err", (&closed_value, closed_ty));
                let _ = writeln!(
                    self.output,
                    "    if (channel->receiver_closed || (channel->sender_closed && \
                     channel->count == 0U)) {{ (void)pthread_mutex_unlock(&channel->lock); \
                     return {closed}; }}\n    if (channel->count == 0U) {{ \
                     (void)pthread_mutex_unlock(&channel->lock); return {empty}; }}"
                );
            } else {
                let none = self.option_expression(*result, None);
                let _ = writeln!(
                    self.output,
                    "    if (channel->receiver_closed) {{ (void)pthread_mutex_unlock(&channel->lock); \
                     return {none}; }}\n    ++channel->waiting_receivers; \
                     (void)pthread_cond_broadcast(&channel->writable);\n    while \
                     (channel->count == 0U && !channel->sender_closed && \
                     !channel->receiver_closed) (void)pthread_cond_wait(&channel->readable, \
                     &channel->lock);\n    --channel->waiting_receivers;\n    if \
                     (channel->count == 0U) {{ (void)pthread_mutex_unlock(&channel->lock); \
                     return {none}; }}"
                );
            }
            let _ = writeln!(
                self.output,
                "    message = channel->head; channel->head = message->next; \
                 if (channel->head == NULL) channel->tail = NULL; --channel->count; \
                 value = message->value;\n    (void)pthread_cond_broadcast(&channel->writable); \
                 (void)pthread_mutex_unlock(&channel->lock);"
            );
            let outcome = if *nonblocking {
                self.result_expression(*result, "Ok", ("value", element))
            } else {
                self.option_expression(*result, Some(("value", element)))
            };
            let _ = writeln!(self.output, "    return {outcome};\n}}\n");
        }

        for operation in calls.keys() {
            let StandardCall::ChannelClose {
                handle,
                sender: closes_sender,
            } = operation
            else {
                continue;
            };
            let TypeKind::Builtin { arguments, .. } =
                self.typed.types.kind(self.resolve_alias(*handle))
            else {
                continue;
            };
            if arguments
                .first()
                .is_none_or(|candidate| self.resolve_alias(*candidate) != element)
            {
                continue;
            }
            let Some(handle_c) = self.c_type(*handle, None) else {
                continue;
            };
            let name = standard_call_name(*operation);
            let field = if *closes_sender {
                "sender_closed"
            } else {
                "receiver_closed"
            };
            let discard = if *closes_sender {
                ""
            } else {
                " channel->head = NULL; channel->tail = NULL; channel->count = 0U;"
            };
            let _ = writeln!(
                self.output,
                "static el_unit {name}({handle_c} channel) {{\n    \
                 (void)pthread_mutex_lock(&channel->lock); channel->{field} = true;{discard}\n    \
                 (void)pthread_cond_broadcast(&channel->readable); \
                 (void)pthread_cond_broadcast(&channel->writable);\n    \
                 (void)pthread_mutex_unlock(&channel->lock); return (el_unit){{0}};\n}}\n"
            );
        }
    }

    fn emit_mutex_helpers(&mut self, calls: &BTreeMap<StandardCall, (TypeId, Span)>) {
        // These critical sections also serve as programmer-visible ordering
        // tokens for ordinary shared storage outside `state->value`.
        let mut mutexes = BTreeMap::new();
        for operation in calls.keys() {
            match operation {
                StandardCall::MutexNew { mutex, value_type }
                | StandardCall::MutexRead { mutex, value_type }
                | StandardCall::MutexReplace { mutex, value_type }
                | StandardCall::MutexUpdate {
                    mutex, value_type, ..
                } => {
                    mutexes.insert(*mutex, *value_type);
                }
                _ => {}
            }
        }
        for (mutex, value_type) in mutexes {
            let (Some(mutex_c), Some(value_c)) =
                (self.c_type(mutex, None), self.c_type(value_type, None))
            else {
                continue;
            };
            let state = format!("{}_data", collection_type_name(mutex));
            let _ = writeln!(
                self.output,
                "struct {state} {{ pthread_mutex_t lock; {value_c} value; }};\n"
            );
            let operation = StandardCall::MutexNew { mutex, value_type };
            if calls.contains_key(&operation) {
                let name = standard_call_name(operation);
                let _ = writeln!(
                    self.output,
                    "static {mutex_c} {name}({value_c} value) {{\n    {mutex_c} state;"
                );
                self.emit_runtime_allocate("state", "sizeof(*state)", AllocationClass::Scanned);
                let _ = writeln!(
                    self.output,
                    "    (void)pthread_mutex_init(&state->lock, NULL); state->value = \
                     value; return state;\n}}\n"
                );
            }
            let operation = StandardCall::MutexRead { mutex, value_type };
            if calls.contains_key(&operation) {
                let _ = writeln!(
                    self.output,
                    "static {value_c} {}({mutex_c} state) {{\n    {value_c} value; \
                     (void)pthread_mutex_lock(&state->lock); value = state->value; \
                     (void)pthread_mutex_unlock(&state->lock); return value;\n}}\n",
                    standard_call_name(operation)
                );
            }
            let operation = StandardCall::MutexReplace { mutex, value_type };
            if calls.contains_key(&operation) {
                let _ = writeln!(
                    self.output,
                    "static {value_c} {}({mutex_c} state, {value_c} replacement) {{\n    \
                     {value_c} previous; (void)pthread_mutex_lock(&state->lock); \
                     previous = state->value; state->value = replacement; \
                     (void)pthread_mutex_unlock(&state->lock); return previous;\n}}\n",
                    standard_call_name(operation)
                );
            }
            for operation in calls.keys() {
                let StandardCall::MutexUpdate {
                    mutex: candidate,
                    value_type: candidate_value,
                    callable,
                    entry,
                    closure_entry,
                } = operation
                else {
                    continue;
                };
                if *candidate != mutex || *candidate_value != value_type {
                    continue;
                }
                let Some(callable_c) = self.c_type(*callable, None) else {
                    continue;
                };
                let symbol = self.function_symbol(&FunctionInstance {
                    declaration: *entry,
                    arguments: Vec::new(),
                    self_type: None,
                });
                let invocation = if *closure_entry {
                    format!("{symbol}(body, state->value)")
                } else {
                    format!("{symbol}(state->value)")
                };
                let _ = writeln!(
                    self.output,
                    "static {value_c} {}({mutex_c} state, {callable_c} body) {{\n    \
                     {value_c} replacement; (void)body; \
                     (void)pthread_mutex_lock(&state->lock); replacement = {invocation}; \
                     state->value = replacement; replacement = state->value; \
                     (void)pthread_mutex_unlock(&state->lock); return replacement;\n}}\n",
                    standard_call_name(*operation)
                );
            }
        }
    }

    fn emit_atomic_helpers(&mut self, calls: &BTreeMap<StandardCall, (TypeId, Span)>) {
        // Every operation is a synchronous mutex-protected linearization
        // point. This deliberately implements the source SC contract without
        // requiring C11 `_Atomic` or exposing weaker orderings.
        let mut atomics = BTreeMap::new();
        for operation in calls.keys() {
            match operation {
                StandardCall::AtomicNew { atomic, value_type }
                | StandardCall::AtomicLoad { atomic, value_type }
                | StandardCall::AtomicStore { atomic, value_type }
                | StandardCall::AtomicExchange { atomic, value_type }
                | StandardCall::AtomicCompareExchange { atomic, value_type }
                | StandardCall::AtomicFetchAdd {
                    atomic, value_type, ..
                } => {
                    atomics.insert(*atomic, *value_type);
                }
                _ => {}
            }
        }
        for (atomic, value_type) in atomics {
            let (Some(atomic_c), Some(value_c)) =
                (self.c_type(atomic, None), self.c_type(value_type, None))
            else {
                continue;
            };
            let state = format!("{}_data", collection_type_name(atomic));
            let _ = writeln!(
                self.output,
                "struct {state} {{ pthread_mutex_t lock; {value_c} value; }};\n"
            );
            let operation = StandardCall::AtomicNew { atomic, value_type };
            if calls.contains_key(&operation) {
                let name = standard_call_name(operation);
                let _ = writeln!(
                    self.output,
                    "static {atomic_c} {name}({value_c} value) {{\n    {atomic_c} state;"
                );
                self.emit_runtime_allocate("state", "sizeof(*state)", AllocationClass::PointerFree);
                let _ = writeln!(
                    self.output,
                    "    (void)pthread_mutex_init(&state->lock, NULL); state->value = value; \
                     return state;\n}}\n"
                );
            }
            let operation = StandardCall::AtomicLoad { atomic, value_type };
            if calls.contains_key(&operation) {
                let _ = writeln!(
                    self.output,
                    "static {value_c} {}({atomic_c} state) {{ {value_c} value; \
                     (void)pthread_mutex_lock(&state->lock); value = state->value; \
                     (void)pthread_mutex_unlock(&state->lock); return value; }}\n",
                    standard_call_name(operation)
                );
            }
            let operation = StandardCall::AtomicStore { atomic, value_type };
            if calls.contains_key(&operation) {
                let _ = writeln!(
                    self.output,
                    "static el_unit {}({atomic_c} state, {value_c} value) {{ \
                     (void)pthread_mutex_lock(&state->lock); state->value = value; \
                     (void)pthread_mutex_unlock(&state->lock); return (el_unit){{0}}; }}\n",
                    standard_call_name(operation)
                );
            }
            let operation = StandardCall::AtomicExchange { atomic, value_type };
            if calls.contains_key(&operation) {
                let _ = writeln!(
                    self.output,
                    "static {value_c} {}({atomic_c} state, {value_c} value) {{ \
                     {value_c} previous; (void)pthread_mutex_lock(&state->lock); \
                     previous = state->value; state->value = value; \
                     (void)pthread_mutex_unlock(&state->lock); return previous; }}\n",
                    standard_call_name(operation)
                );
            }
            let operation = StandardCall::AtomicCompareExchange { atomic, value_type };
            if calls.contains_key(&operation) {
                let _ = writeln!(
                    self.output,
                    "static bool {}({atomic_c} state, {value_c} expected, {value_c} replacement) {{ \
                     bool exchanged; (void)pthread_mutex_lock(&state->lock); \
                     exchanged = state->value == expected; if (exchanged) state->value = replacement; \
                     (void)pthread_mutex_unlock(&state->lock); return exchanged; }}\n",
                    standard_call_name(operation)
                );
            }
            for subtract in [false, true] {
                let operation = StandardCall::AtomicFetchAdd {
                    atomic,
                    value_type,
                    subtract,
                };
                if !calls.contains_key(&operation) {
                    continue;
                }
                let arithmetic = if subtract {
                    if matches!(
                        self.typed.types.kind(self.resolve_alias(value_type)),
                        TypeKind::Primitive(PrimitiveType::I32)
                    ) {
                        "(int32_t)((uint32_t)state->value - (uint32_t)value)"
                    } else {
                        "(uintptr_t)(state->value - value)"
                    }
                } else if matches!(
                    self.typed.types.kind(self.resolve_alias(value_type)),
                    TypeKind::Primitive(PrimitiveType::I32)
                ) {
                    "(int32_t)((uint32_t)state->value + (uint32_t)value)"
                } else {
                    "(uintptr_t)(state->value + value)"
                };
                let _ = writeln!(
                    self.output,
                    "static {value_c} {}({atomic_c} state, {value_c} value) {{ \
                     {value_c} previous; (void)pthread_mutex_lock(&state->lock); \
                     previous = state->value; state->value = {arithmetic}; \
                     (void)pthread_mutex_unlock(&state->lock); return previous; }}\n",
                    standard_call_name(operation)
                );
            }
        }
    }

    fn emit_shared_store_helpers(&mut self, calls: &BTreeMap<StandardCall, (TypeId, Span)>) {
        let store_new_used = calls
            .keys()
            .any(|call| matches!(call, StandardCall::StoreNew { .. }));
        if store_new_used {
            self.output
                .push_str("static uintptr_t el_next_store_identity = 1U;\n\n");
        }
        for (operation, (result, _)) in calls {
            match *operation {
                StandardCall::SharedNew { shared, value } => {
                    let (Some(shared_c), Some(value_c)) =
                        (self.c_type(shared, None), self.c_type(value, None))
                    else {
                        continue;
                    };
                    let _ = writeln!(
                        self.output,
                        "static {shared_c} {}({value_c} value) {{\n    {shared_c} result = ({shared_c})malloc(sizeof(*result));\n    if (result == NULL) {{ el_out_of_memory(); }}\n    el_cost_allocation(sizeof(*result), false);\n    if (pthread_mutex_init(&result->lock, NULL) != 0) {{ el_out_of_memory(); }}\n    result->strong = 1U; result->weak = 1U; result->value = value; return result;\n}}\n",
                        standard_call_name(*operation)
                    );
                }
                StandardCall::SharedGet { shared, value } => {
                    let (Some(shared_c), Some(value_c)) =
                        (self.c_type(shared, None), self.c_type(value, None))
                    else {
                        continue;
                    };
                    let _ = writeln!(
                        self.output,
                        "static {value_c} *{}({shared_c} value) {{ return &value->value; }}\n",
                        standard_call_name(*operation)
                    );
                }
                StandardCall::SharedDowngrade { shared, weak, .. } => {
                    let (Some(shared_c), Some(weak_c)) =
                        (self.c_type(shared, None), self.c_type(weak, None))
                    else {
                        continue;
                    };
                    let _ = writeln!(
                        self.output,
                        "static {weak_c} {}({shared_c} value) {{\n    (void)pthread_mutex_lock(&value->lock);\n    if (value->weak == UINTPTR_MAX) {{ (void)pthread_mutex_unlock(&value->lock); el_out_of_memory(); }}\n    ++value->weak; (void)pthread_mutex_unlock(&value->lock); return value;\n}}\n",
                        standard_call_name(*operation)
                    );
                }
                StandardCall::WeakUpgrade {
                    weak,
                    shared,
                    result: option,
                    value,
                } => {
                    let (Some(weak_c), Some(shared_c), Some(option_c)) = (
                        self.c_type(weak, None),
                        self.c_type(shared, None),
                        self.c_type(option, None),
                    ) else {
                        continue;
                    };
                    let none = self.option_expression(option, None);
                    let some = self.option_expression(option, Some(("owner", value)));
                    let _ = writeln!(
                        self.output,
                        "static {option_c} {}({weak_c} value) {{\n    {shared_c} owner;\n    (void)pthread_mutex_lock(&value->lock);\n    if (value->strong == 0U) {{ (void)pthread_mutex_unlock(&value->lock); return {none}; }}\n    if (value->strong == UINTPTR_MAX) {{ (void)pthread_mutex_unlock(&value->lock); el_out_of_memory(); }}\n    ++value->strong; owner = value; (void)pthread_mutex_unlock(&value->lock); return {some};\n}}\n",
                        standard_call_name(*operation)
                    );
                }
                StandardCall::StoreNew { store, .. } => {
                    let Some(store_c) = self.c_type(store, None) else {
                        continue;
                    };
                    let _ = writeln!(
                        self.output,
                        "static {store_c} {}(void) {{\n    {store_c} result = ({store_c})malloc(sizeof(*result));\n    if (result == NULL || el_next_store_identity == 0U) {{ el_out_of_memory(); }}\n    el_cost_allocation(sizeof(*result), false);\n    result->identity = el_next_store_identity++; result->length = 0U; result->capacity = 0U; result->slots = NULL; result->live_slots = NULL; return result;\n}}\n",
                        standard_call_name(*operation)
                    );
                }
                StandardCall::StoreLen { store } => {
                    let Some(store_c) = self.c_type(store, None) else {
                        continue;
                    };
                    let _ = writeln!(
                        self.output,
                        "static uintptr_t {}({store_c} value) {{ return value->length; }}\n",
                        standard_call_name(*operation)
                    );
                }
                StandardCall::StoreIsEmpty { store } => {
                    let Some(store_c) = self.c_type(store, None) else {
                        continue;
                    };
                    let _ = writeln!(
                        self.output,
                        "static bool {}({store_c} value) {{ return value->length == 0U; }}\n",
                        standard_call_name(*operation)
                    );
                }
                StandardCall::StoreInsert {
                    store,
                    handle,
                    value,
                } => {
                    let (Some(store_c), Some(handle_c), Some(value_c)) = (
                        self.c_type(store, None),
                        self.c_type(handle, None),
                        self.c_type(value, None),
                    ) else {
                        continue;
                    };
                    let _ = writeln!(
                        self.output,
                        "static {handle_c} {}({store_c} *value, {value_c} item) {{\n    uintptr_t index, old_capacity, capacity;\n    for (index = 0U; index < (*value)->capacity; ++index) if (!(*value)->slots[index].occupied && (*value)->slots[index].generation != UINTPTR_MAX) break;\n    if (index == (*value)->capacity) {{\n        if ((*value)->capacity == UINTPTR_MAX) {{ el_out_of_memory(); }}\n        old_capacity = (*value)->capacity; capacity = old_capacity == 0U ? 4U : old_capacity * 2U;\n        if (capacity <= old_capacity || capacity > SIZE_MAX / sizeof(*(*value)->slots) || capacity > SIZE_MAX / sizeof(uintptr_t)) {{ el_out_of_memory(); }}\n        (*value)->slots = realloc((*value)->slots, capacity * sizeof(*(*value)->slots));\n        (*value)->live_slots = realloc((*value)->live_slots, capacity * sizeof(uintptr_t));\n        if ((*value)->slots == NULL || (*value)->live_slots == NULL) {{ el_out_of_memory(); }}\n        el_cost_allocation(capacity * sizeof(*(*value)->slots), false); el_cost_allocation(capacity * sizeof(uintptr_t), false);\n        for (index = old_capacity; index < capacity; ++index) {{ (*value)->slots[index].generation = 1U; (*value)->slots[index].occupied = false; }}\n        (*value)->capacity = capacity; index = old_capacity;\n    }}\n    if ((*value)->length == UINTPTR_MAX) {{ el_out_of_memory(); }}\n    (*value)->slots[index].value = item; (*value)->slots[index].occupied = true; (*value)->slots[index].dense_index = (*value)->length; (*value)->live_slots[(*value)->length++] = index;\n    return ({handle_c}){{ (*value)->identity, index, (*value)->slots[index].generation }};\n}}\n",
                        standard_call_name(*operation)
                    );
                }
                StandardCall::StoreGet {
                    store,
                    handle,
                    value,
                    mutable,
                } => {
                    let (Some(store_c), Some(handle_c), Some(value_c)) = (
                        self.c_type(store, None),
                        self.c_type(handle, None),
                        self.c_type(value, None),
                    ) else {
                        continue;
                    };
                    let receiver = if mutable {
                        format!("{store_c} *value")
                    } else {
                        format!("{store_c} value")
                    };
                    let access = if mutable { "(*value)" } else { "value" };
                    let _ = writeln!(
                        self.output,
                        "static {value_c} *{}({receiver}, {handle_c} handle, const char *path, uint32_t line, uint32_t column) {{\n    if (handle.store != {access}->identity) el_trap(\"E-RUN-WRONG-STORE\", path, line, column);\n    if (handle.slot >= {access}->capacity || !{access}->slots[handle.slot].occupied || {access}->slots[handle.slot].generation != handle.generation) el_trap(\"E-RUN-STALE\", path, line, column);\n    return &{access}->slots[handle.slot].value;\n}}\n",
                        standard_call_name(*operation)
                    );
                }
                StandardCall::StoreRemove {
                    store,
                    handle,
                    value,
                } => {
                    let (Some(store_c), Some(handle_c), Some(value_c)) = (
                        self.c_type(store, None),
                        self.c_type(handle, None),
                        self.c_type(value, None),
                    ) else {
                        continue;
                    };
                    let _ = writeln!(
                        self.output,
                        "static {value_c} {}({store_c} *value, {handle_c} handle, const char *path, uint32_t line, uint32_t column) {{\n    {value_c} result; uintptr_t dense, index, moved;\n    if (handle.store != (*value)->identity) el_trap(\"E-RUN-WRONG-STORE\", path, line, column);\n    if (handle.slot >= (*value)->capacity || !(*value)->slots[handle.slot].occupied || (*value)->slots[handle.slot].generation != handle.generation) el_trap(\"E-RUN-STALE\", path, line, column);\n    result = (*value)->slots[handle.slot].value; dense = (*value)->slots[handle.slot].dense_index;\n    for (index = dense + 1U; index < (*value)->length; ++index) {{ moved = (*value)->live_slots[index]; (*value)->live_slots[index - 1U] = moved; (*value)->slots[moved].dense_index = index - 1U; }}\n    --(*value)->length; (*value)->slots[handle.slot].occupied = false;\n    if ((*value)->slots[handle.slot].generation != UINTPTR_MAX) {{ ++(*value)->slots[handle.slot].generation; }}\n    return result;\n}}\n",
                        standard_call_name(*operation)
                    );
                }
                StandardCall::StoreCompact { store } => {
                    let Some(store_c) = self.c_type(store, None) else {
                        continue;
                    };
                    let _ = writeln!(
                        self.output,
                        "static el_unit {}({store_c} *value) {{\n    void *slots, *live_slots; size_t slot_bytes, live_bytes;\n    if ((*value)->capacity == 0U) return (el_unit){{0}};\n    slot_bytes = (*value)->capacity * sizeof(*(*value)->slots); live_bytes = (*value)->capacity * sizeof(uintptr_t); slots = malloc(slot_bytes); live_slots = malloc(live_bytes);\n    if (slots == NULL || live_slots == NULL) {{ el_out_of_memory(); }}\n    el_cost_allocation(slot_bytes, false); el_cost_allocation(live_bytes, false); el_cost_memcpy(slots, (*value)->slots, slot_bytes); el_cost_memcpy(live_slots, (*value)->live_slots, live_bytes);\n    free((*value)->slots); free((*value)->live_slots); (*value)->slots = slots; (*value)->live_slots = live_slots; return (el_unit){{0}};\n}}\n",
                        standard_call_name(*operation)
                    );
                }
                StandardCall::StoreClear { store, value } => {
                    let Some(store_c) = self.c_type(store, None) else {
                        continue;
                    };
                    self.emit_drop_helper(value);
                    let drop = if self.type_has_drop_glue(value) {
                        format!("el_drop_t{}(&(*store)->slots[index].value);", value.index())
                    } else {
                        String::new()
                    };
                    let _ = writeln!(
                        self.output,
                        "static el_unit {}({store_c} *store) {{\n    uintptr_t index;\n    for (index = 0U; index < (*store)->capacity; ++index) if ((*store)->slots[index].occupied) {{ {drop} (*store)->slots[index].occupied = false; if ((*store)->slots[index].generation != UINTPTR_MAX) ++(*store)->slots[index].generation; }}\n    (*store)->length = 0U; return (el_unit){{0}};\n}}\n",
                        standard_call_name(*operation)
                    );
                }
                _ => {
                    let _ = result;
                }
            }
        }
    }

    fn emit_clone_helper(&mut self, ty: TypeId) {
        let ty = self.resolve_alias(ty);
        if !self.emitted_clone_helpers.insert(ty) {
            return;
        }
        let kind = self.typed.types.kind(ty).clone();
        let custom =
            crate::traits::select_trait_method(self.resolved, self.typed, ty, "clone", None)
                .ok()
                .flatten()
                .filter(|selected| {
                    selected.trait_declaration == self.resolved.standard_declaration("Clone")
                });
        let Some(c_type) = self.c_type(ty, None) else {
            return;
        };
        let name = standard_call_name(StandardCall::Clone { value: ty });
        if let Some(selected) = custom {
            let instance = FunctionInstance {
                declaration: selected.declaration,
                arguments: selected.arguments,
                self_type: selected.self_type,
            };
            let symbol = self.function_symbol(&instance);
            let _ = writeln!(
                self.output,
                "static {c_type} {name}({c_type} value) {{ return {symbol}(&value); }}\n"
            );
            return;
        }
        match kind {
            TypeKind::Primitive(PrimitiveType::String) => {
                let _ = writeln!(
                    self.output,
                    "static {c_type} {name}({c_type} value) {{ return el_string_clone(value); }}\n"
                );
            }
            TypeKind::Tuple(elements) => {
                for element in &elements {
                    self.emit_clone_helper(*element);
                }
                let _ = writeln!(self.output, "static {c_type} {name}({c_type} value) {{");
                let _ = writeln!(self.output, "    {c_type} result;");
                for (index, element) in elements.iter().enumerate() {
                    let child = standard_call_name(StandardCall::Clone { value: *element });
                    let _ = writeln!(
                        self.output,
                        "    result.v{index} = {child}(value.v{index});"
                    );
                }
                self.output.push_str("    return result;\n}\n\n");
            }
            TypeKind::Array { element, length } => {
                self.emit_clone_helper(element);
                let child = standard_call_name(StandardCall::Clone { value: element });
                let _ = writeln!(
                    self.output,
                    "static {c_type} {name}({c_type} value) {{\n    {c_type} result;\n    uintptr_t index;\n    for (index = 0U; index < {length}U; ++index) result.values[index] = {child}(value.values[index]);\n    return result;\n}}\n"
                );
            }
            TypeKind::Closure { captures, .. } if self.program.value_model == ValueModel::Owned => {
                for capture in &captures {
                    self.emit_clone_helper(*capture);
                }
                let _ = writeln!(self.output, "static {c_type} {name}({c_type} value) {{");
                let _ = writeln!(self.output, "    {c_type} result = {{0}};");
                for (index, capture) in captures.iter().enumerate() {
                    let child = standard_call_name(StandardCall::Clone { value: *capture });
                    let _ = writeln!(
                        self.output,
                        "    result.v{index} = {child}(value.v{index});"
                    );
                }
                self.output.push_str("    return result;\n}\n\n");
            }
            TypeKind::Nominal { .. } => {
                if let Some(structure) = self.structs.get(&ty).copied() {
                    for (_, _, field_ty) in &structure.fields {
                        self.emit_clone_helper(*field_ty);
                    }
                    let _ = writeln!(self.output, "static {c_type} {name}({c_type} value) {{");
                    let _ = writeln!(self.output, "    {c_type} result;");
                    for (field, _, field_ty) in &structure.fields {
                        let member = field_name(*field);
                        let child = standard_call_name(StandardCall::Clone { value: *field_ty });
                        let _ = writeln!(
                            self.output,
                            "    result.{member} = {child}(value.{member});"
                        );
                    }
                    self.output.push_str("    return result;\n}\n\n");
                } else {
                    // A structural enum clone is emitted variant by variant.
                    let Some(enumeration) = self.enums.get(&ty).copied() else {
                        let _ = writeln!(
                            self.output,
                            "static {c_type} {name}({c_type} value) {{ return value; }}\n"
                        );
                        return;
                    };
                    for variant in &enumeration.variants {
                        for (_, _, field_ty) in &variant.fields {
                            self.emit_clone_helper(*field_ty);
                        }
                    }
                    let _ = writeln!(self.output, "static {c_type} {name}({c_type} value) {{");
                    let _ = writeln!(self.output, "    {c_type} result; result.tag = value.tag;");
                    self.output.push_str("    switch (value.tag) {\n");
                    for variant in &enumeration.variants {
                        let variant_member = variant_member_name(variant.id);
                        let _ = writeln!(self.output, "    case UINT32_C({}):", variant.id.index());
                        for (field, _, field_ty) in &variant.fields {
                            let member = field_name(*field);
                            let child =
                                standard_call_name(StandardCall::Clone { value: *field_ty });
                            let _ = writeln!(
                                self.output,
                                "        result.payload.{variant_member}.{member} = {child}(value.payload.{variant_member}.{member});"
                            );
                        }
                        self.output.push_str("        break;\n");
                    }
                    self.output
                        .push_str("    default: break;\n    }\n    return result;\n}\n\n");
                }
            }
            TypeKind::Builtin { builtin, arguments } => {
                let builtin = self.resolved.builtin_name(builtin);
                if !matches!(builtin, "Shared" | "Weak") {
                    for argument in &arguments {
                        self.emit_clone_helper(*argument);
                    }
                }
                match (builtin, arguments.as_slice()) {
                    ("Vec", [element]) => {
                        let element_c = self
                            .c_type(*element, None)
                            .unwrap_or_else(|| "el_unit".into());
                        let child = standard_call_name(StandardCall::Clone { value: *element });
                        let _ = writeln!(
                            self.output,
                            "static {c_type} {name}({c_type} value) {{\n    {c_type} result = {{0}}; uintptr_t index;\n    result.length = value.length; result.capacity = value.length;\n    if (value.length > SIZE_MAX / sizeof({element_c})) el_out_of_memory();\n    if (value.length != 0U) {{ result.values = ({element_c} *)malloc(value.length * sizeof({element_c})); if (result.values == NULL) el_out_of_memory(); el_cost_allocation(value.length * sizeof({element_c}), false); }}\n    for (index = 0U; index < value.length; ++index) result.values[index] = {child}(value.values[index]);\n    return result;\n}}\n"
                        );
                    }
                    ("Map", [key, value]) => {
                        let key_c = self.c_type(*key, None).unwrap_or_else(|| "el_unit".into());
                        let value_c = self
                            .c_type(*value, None)
                            .unwrap_or_else(|| "el_unit".into());
                        let key_clone = standard_call_name(StandardCall::Clone { value: *key });
                        let value_clone = standard_call_name(StandardCall::Clone { value: *value });
                        let _ = writeln!(
                            self.output,
                            "static {c_type} {name}({c_type} value) {{\n    {c_type} result; uintptr_t index;\n    result = ({c_type})malloc(sizeof(*result)); if (result == NULL) el_out_of_memory(); el_cost_allocation(sizeof(*result), false);\n    result->length = value->length; result->capacity = value->length; result->keys = NULL; result->values = NULL;\n    if (value->length > SIZE_MAX / sizeof({key_c}) || value->length > SIZE_MAX / sizeof({value_c})) el_out_of_memory();\n    if (value->length != 0U) {{ result->keys = ({key_c} *)malloc(value->length * sizeof({key_c})); result->values = ({value_c} *)malloc(value->length * sizeof({value_c})); if (result->keys == NULL || result->values == NULL) el_out_of_memory(); el_cost_allocation(value->length * sizeof({key_c}), false); el_cost_allocation(value->length * sizeof({value_c}), false); }}\n    for (index = 0U; index < value->length; ++index) {{ result->keys[index] = {key_clone}(value->keys[index]); result->values[index] = {value_clone}(value->values[index]); }}\n    return result;\n}}\n"
                        );
                    }
                    ("Set", [element]) => {
                        let element_c = self
                            .c_type(*element, None)
                            .unwrap_or_else(|| "el_unit".into());
                        let child = standard_call_name(StandardCall::Clone { value: *element });
                        let _ = writeln!(
                            self.output,
                            "static {c_type} {name}({c_type} value) {{\n    {c_type} result; uintptr_t index;\n    result = ({c_type})malloc(sizeof(*result)); if (result == NULL) el_out_of_memory(); el_cost_allocation(sizeof(*result), false);\n    result->length = value->length; result->capacity = value->length; result->values = NULL;\n    if (value->length > SIZE_MAX / sizeof({element_c})) el_out_of_memory();\n    if (value->length != 0U) {{ result->values = ({element_c} *)malloc(value->length * sizeof({element_c})); if (result->values == NULL) el_out_of_memory(); el_cost_allocation(value->length * sizeof({element_c}), false); }}\n    for (index = 0U; index < value->length; ++index) result->values[index] = {child}(value->values[index]);\n    return result;\n}}\n"
                        );
                    }
                    ("Box", [value]) => {
                        let child = standard_call_name(StandardCall::Clone { value: *value });
                        let _ = writeln!(
                            self.output,
                            "static {c_type} {name}({c_type} value) {{\n    {c_type} result = ({c_type})malloc(sizeof(*result));\n    if (result == NULL) el_out_of_memory(); el_cost_allocation(sizeof(*result), false);\n    *result = {child}(*value);\n    return result;\n}}\n"
                        );
                    }
                    ("Shared", [_]) => {
                        let _ = writeln!(
                            self.output,
                            "static {c_type} {name}({c_type} value) {{\n    if (value == NULL) return NULL;\n    (void)pthread_mutex_lock(&value->lock);\n    if (value->strong == UINTPTR_MAX) {{ (void)pthread_mutex_unlock(&value->lock); el_out_of_memory(); }}\n    ++value->strong;\n    (void)pthread_mutex_unlock(&value->lock);\n    return value;\n}}\n"
                        );
                    }
                    ("Weak", [_]) => {
                        let _ = writeln!(
                            self.output,
                            "static {c_type} {name}({c_type} value) {{\n    if (value == NULL) return NULL;\n    (void)pthread_mutex_lock(&value->lock);\n    if (value->weak == UINTPTR_MAX) {{ (void)pthread_mutex_unlock(&value->lock); el_out_of_memory(); }}\n    ++value->weak;\n    (void)pthread_mutex_unlock(&value->lock);\n    return value;\n}}\n"
                        );
                    }
                    _ => {
                        let _ = writeln!(
                            self.output,
                            "static {c_type} {name}({c_type} value) {{ return value; }}\n"
                        );
                    }
                }
            }
            _ => {
                let _ = writeln!(
                    self.output,
                    "static {c_type} {name}({c_type} value) {{ return value; }}\n"
                );
            }
        }
    }

    fn emit_drop_helper(&mut self, ty: TypeId) -> bool {
        let ty = self.resolve_alias(ty);
        if self.emitted_drop_helpers.contains(&ty) {
            return true;
        }
        let kind = self.typed.types.kind(ty).clone();
        let custom =
            crate::traits::select_trait_method(self.resolved, self.typed, ty, "drop", None)
                .ok()
                .flatten()
                .filter(|selected| {
                    selected.trait_declaration == self.resolved.standard_declaration("Drop")
                });
        let needs_structural = match &kind {
            TypeKind::Primitive(PrimitiveType::String) => true,
            TypeKind::Tuple(elements) => {
                elements.iter().any(|child| self.type_has_drop_glue(*child))
            }
            TypeKind::Array { element, .. } => self.type_has_drop_glue(*element),
            TypeKind::Closure { captures, .. } if self.program.value_model == ValueModel::Owned => {
                captures
                    .iter()
                    .any(|capture| self.type_has_drop_glue(*capture))
            }
            TypeKind::Nominal { .. } => {
                self.structs.get(&ty).is_some_and(|structure| {
                    structure
                        .fields
                        .iter()
                        .any(|(_, _, child)| self.type_has_drop_glue(*child))
                }) || self.enums.get(&ty).is_some_and(|enumeration| {
                    enumeration.variants.iter().any(|variant| {
                        variant
                            .fields
                            .iter()
                            .any(|(_, _, child)| self.type_has_drop_glue(*child))
                    })
                })
            }
            TypeKind::Builtin { builtin, .. } => matches!(
                self.resolved.builtin_name(*builtin),
                "Vec" | "Map" | "Set" | "Box" | "Shared" | "Weak" | "Store"
            ),
            _ => false,
        };
        if custom.is_none() && !needs_structural {
            return false;
        }
        self.emitted_drop_helpers.insert(ty);
        let Some(c_type) = self.c_type(ty, None) else {
            return false;
        };
        let name = format!("el_drop_t{}", ty.index());

        // Dependencies must precede this helper in C99.
        match &kind {
            TypeKind::Tuple(elements) => {
                for child in elements {
                    self.emit_drop_helper(*child);
                }
            }
            TypeKind::Array { element, .. } => {
                self.emit_drop_helper(*element);
            }
            TypeKind::Closure { captures, .. } if self.program.value_model == ValueModel::Owned => {
                for capture in captures {
                    self.emit_drop_helper(*capture);
                }
            }
            TypeKind::Nominal { .. } => {
                let children = self.structs.get(&ty).map_or_else(
                    || {
                        self.enums.get(&ty).map_or_else(Vec::new, |enumeration| {
                            enumeration
                                .variants
                                .iter()
                                .flat_map(|variant| {
                                    variant.fields.iter().map(|(_, _, child)| *child)
                                })
                                .collect()
                        })
                    },
                    |structure| {
                        structure
                            .fields
                            .iter()
                            .map(|(_, _, child)| *child)
                            .collect()
                    },
                );
                for child in children {
                    self.emit_drop_helper(child);
                }
            }
            TypeKind::Builtin { arguments, .. } => {
                for child in arguments {
                    self.emit_drop_helper(*child);
                }
            }
            _ => {}
        }
        let _ = writeln!(self.output, "static void {name}({c_type} *value) {{");
        if let Some(selected) = custom {
            let instance = FunctionInstance {
                declaration: selected.declaration,
                arguments: selected.arguments,
                self_type: selected.self_type,
            };
            let _ = writeln!(
                self.output,
                "    {}(value);",
                self.function_symbol(&instance)
            );
        }
        match kind {
            TypeKind::Primitive(PrimitiveType::String) => {
                self.output.push_str("    el_owned_string_drop(value);\n")
            }
            TypeKind::Tuple(elements) => {
                for (index, child) in elements.iter().enumerate().rev() {
                    if self.type_has_drop_glue(*child) {
                        let _ = writeln!(
                            self.output,
                            "    el_drop_t{}(&value->v{index});",
                            child.index()
                        );
                    }
                }
            }
            TypeKind::Array { element, length } => {
                if self.type_has_drop_glue(element) {
                    let _ = writeln!(
                        self.output,
                        "    for (uintptr_t index = {length}U; index != 0U; --index) el_drop_t{}(&value->values[index - 1U]);",
                        element.index()
                    );
                }
            }
            TypeKind::Closure { captures, .. } if self.program.value_model == ValueModel::Owned => {
                for (index, capture) in captures.iter().enumerate().rev() {
                    if self.type_has_drop_glue(*capture) {
                        let _ = writeln!(
                            self.output,
                            "    el_drop_t{}(&value->v{index});",
                            capture.index()
                        );
                    }
                }
            }
            TypeKind::Nominal { .. } => {
                if let Some(structure) = self.structs.get(&ty) {
                    for (field, _, child) in structure.fields.iter().rev() {
                        if self.type_has_drop_glue(*child) {
                            let _ = writeln!(
                                self.output,
                                "    el_drop_t{}(&value->{});",
                                child.index(),
                                field_name(*field)
                            );
                        }
                    }
                } else if let Some(enumeration) = self.enums.get(&ty) {
                    self.output.push_str("    switch (value->tag) {\n");
                    for variant in &enumeration.variants {
                        let _ = writeln!(self.output, "    case UINT32_C({}):", variant.id.index());
                        for (field, _, child) in variant.fields.iter().rev() {
                            if self.type_has_drop_glue(*child) {
                                let _ = writeln!(
                                    self.output,
                                    "        el_drop_t{}(&value->payload.{}.{});",
                                    child.index(),
                                    variant_member_name(variant.id),
                                    field_name(*field)
                                );
                            }
                        }
                        self.output.push_str("        break;\n");
                    }
                    self.output.push_str("    default: break;\n    }\n");
                }
            }
            TypeKind::Builtin { builtin, arguments } => {
                match (self.resolved.builtin_name(builtin), arguments.as_slice()) {
                    ("Vec", [element]) => {
                        if self.type_has_drop_glue(*element) {
                            let _ = writeln!(
                                self.output,
                                "    for (uintptr_t index = value->length; index != 0U; --index) el_drop_t{}(&value->values[index - 1U]);",
                                element.index()
                            );
                        }
                        self.output.push_str("    free(value->values); value->values = NULL; value->length = 0U; value->capacity = 0U;\n");
                    }
                    ("Map", [key, item]) => {
                        self.output.push_str("    if (*value != NULL) {\n");
                        let _ = writeln!(
                            self.output,
                            "        for (uintptr_t index = (*value)->length; index != 0U; --index) {{"
                        );
                        if self.type_has_drop_glue(*item) {
                            let _ = writeln!(
                                self.output,
                                "            el_drop_t{}(&(*value)->values[index - 1U]);",
                                item.index()
                            );
                        }
                        if self.type_has_drop_glue(*key) {
                            let _ = writeln!(
                                self.output,
                                "            el_drop_t{}(&(*value)->keys[index - 1U]);",
                                key.index()
                            );
                        }
                        self.output.push_str("        }\n        free((*value)->keys); free((*value)->values); free(*value); *value = NULL;\n    }\n");
                    }
                    ("Set", [element]) => {
                        self.output.push_str("    if (*value != NULL) {\n");
                        if self.type_has_drop_glue(*element) {
                            let _ = writeln!(
                                self.output,
                                "        for (uintptr_t index = (*value)->length; index != 0U; --index) el_drop_t{}(&(*value)->values[index - 1U]);",
                                element.index()
                            );
                        }
                        self.output.push_str(
                            "        free((*value)->values); free(*value); *value = NULL;\n    }\n",
                        );
                    }
                    ("Box", [item]) => {
                        self.output.push_str("    if (*value != NULL) {\n");
                        if self.type_has_drop_glue(*item) {
                            let _ =
                                writeln!(self.output, "        el_drop_t{}(*value);", item.index());
                        }
                        self.output
                            .push_str("        free(*value); *value = NULL;\n    }\n");
                    }
                    ("Shared", [item]) => {
                        self.output.push_str("    if (*value != NULL) {\n        bool destroy_value = false, free_block = false;\n        (void)pthread_mutex_lock(&(*value)->lock);\n        if (--(*value)->strong == 0U) destroy_value = true;\n        (void)pthread_mutex_unlock(&(*value)->lock);\n        if (destroy_value) {\n");
                        if self.type_has_drop_glue(*item) {
                            let _ = writeln!(
                                self.output,
                                "            el_drop_t{}(&(*value)->value);",
                                item.index()
                            );
                        }
                        self.output.push_str("            (void)pthread_mutex_lock(&(*value)->lock);\n            if (--(*value)->weak == 0U) free_block = true;\n            (void)pthread_mutex_unlock(&(*value)->lock);\n            if (free_block) { (void)pthread_mutex_destroy(&(*value)->lock); free(*value); }\n        }\n        *value = NULL;\n    }\n");
                    }
                    ("Weak", [_]) => {
                        self.output.push_str("    if (*value != NULL) {\n        bool free_block = false;\n        (void)pthread_mutex_lock(&(*value)->lock);\n        if (--(*value)->weak == 0U) free_block = true;\n        (void)pthread_mutex_unlock(&(*value)->lock);\n        if (free_block) { (void)pthread_mutex_destroy(&(*value)->lock); free(*value); }\n        *value = NULL;\n    }\n");
                    }
                    ("Store", [item]) => {
                        self.output.push_str("    if (*value != NULL) {\n        uintptr_t index;\n        for (index = (*value)->capacity; index != 0U; --index) {\n            if (!(*value)->slots[index - 1U].occupied) continue;\n");
                        if self.type_has_drop_glue(*item) {
                            let _ = writeln!(
                                self.output,
                                "            el_drop_t{}(&(*value)->slots[index - 1U].value);",
                                item.index()
                            );
                        }
                        self.output.push_str("        }\n        free((*value)->slots); free((*value)->live_slots); free(*value); *value = NULL;\n    }\n");
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        self.output.push_str("}\n\n");
        true
    }

    fn type_has_drop_glue(&self, ty: TypeId) -> bool {
        let ty = self.resolve_alias(ty);
        self.emitted_drop_helpers.contains(&ty)
            || matches!(
                self.typed.types.kind(ty),
                TypeKind::Primitive(PrimitiveType::String)
            )
            || crate::traits::select_trait_method(self.resolved, self.typed, ty, "drop", None)
                .ok()
                .flatten()
                .is_some()
            || match self.typed.types.kind(ty) {
                TypeKind::Builtin { builtin, .. } => matches!(
                    self.resolved.builtin_name(*builtin),
                    "Vec" | "Map" | "Set" | "Box" | "Shared" | "Weak" | "Store"
                ),
                TypeKind::Tuple(elements) => {
                    elements.iter().any(|child| self.type_has_drop_glue(*child))
                }
                TypeKind::Array { element, .. } => self.type_has_drop_glue(*element),
                TypeKind::Closure { captures, .. }
                    if self.program.value_model == ValueModel::Owned =>
                {
                    captures
                        .iter()
                        .any(|capture| self.type_has_drop_glue(*capture))
                }
                TypeKind::Nominal { .. } => {
                    self.structs.get(&ty).is_some_and(|structure| {
                        structure
                            .fields
                            .iter()
                            .any(|(_, _, child)| self.type_has_drop_glue(*child))
                    }) || self.enums.get(&ty).is_some_and(|enumeration| {
                        enumeration.variants.iter().any(|variant| {
                            variant
                                .fields
                                .iter()
                                .any(|(_, _, child)| self.type_has_drop_glue(*child))
                        })
                    })
                }
                _ => false,
            }
    }

    pub(super) fn emit_vec_helpers(
        &mut self,
        collection: TypeId,
        element: TypeId,
        calls: &BTreeMap<StandardCall, (TypeId, Span)>,
        literals: &BTreeSet<(CollectionLiteralKind, TypeId, usize)>,
        concatenate_used: bool,
    ) {
        let Some(collection_type) = self.c_type(collection, None) else {
            return;
        };
        let Some(element_type) = self.c_type(element, None) else {
            return;
        };
        let clear = StandardCall::VecClear { collection };
        let element_drops = self.program.value_model == ValueModel::Owned
            && calls.contains_key(&clear)
            && self.emit_drop_helper(element);
        let scanned = if self.scanned_allocation(element) {
            AllocationClass::Scanned
        } else {
            AllocationClass::PointerFree
        };
        let new = standard_call_name(StandardCall::VecNew { collection });
        let new_used = calls.contains_key(&StandardCall::VecNew { collection })
            || concatenate_used
            || literals
                .iter()
                .any(|(kind, ty, _)| *kind == CollectionLiteralKind::Vec && *ty == collection);
        if new_used {
            let _ = writeln!(
                self.output,
                "static {collection_type} {new}(void) {{ return ({collection_type}){{0}}; }}\n"
            );
        }

        if concatenate_used {
            let concatenate = concatenate_name(collection);
            let _ = writeln!(
                self.output,
                "static {collection_type} {concatenate}({collection_type} left, {collection_type} right) {{\n    \
                 {collection_type} result = {new}();\n    uintptr_t length;\n    \
                 if (left.length > UINTPTR_MAX - right.length) el_out_of_memory();\n    \
                 length = left.length + right.length;\n    \
                 if (length > SIZE_MAX / sizeof({element_type})) el_out_of_memory();\n    \
                 result.length = length;\n    result.capacity = length;\n    \
                 if (length != 0U) {{"
            );
            let bytes = format!("length * sizeof({element_type})");
            self.emit_collection_allocate("result.values", &bytes, scanned);
            let _ = writeln!(
                self.output,
                "    }}\n    for (uintptr_t index = 0U; index < left.length; ++index) \
                 result.values[index] = left.values[index];\n    \
                 for (uintptr_t index = 0U; index < right.length; ++index) \
                 result.values[left.length + index] = right.values[index];\n    \
                 #if EL_OWNED_VALUES\n    free(left.values); free(right.values);\n    #endif\n    \
                 return result;\n}}\n"
            );
        }

        let operation = StandardCall::VecLen { collection };
        if calls.contains_key(&operation) {
            let _ = writeln!(
                self.output,
                "static uintptr_t {}({collection_type} value) {{ return value.length; }}\n",
                standard_call_name(operation)
            );
        }
        let operation = StandardCall::VecIsEmpty { collection };
        if calls.contains_key(&operation) {
            let _ = writeln!(
                self.output,
                "static bool {}({collection_type} value) {{ return value.length == 0U; }}\n",
                standard_call_name(operation)
            );
        }
        let operation = StandardCall::VecGet { collection };
        if let Some((result, _)) = calls.get(&operation) {
            let result_type = self
                .c_type(*result, None)
                .unwrap_or_else(|| "el_unit".to_string());
            let none = self.option_expression(*result, None);
            let value = if self.program.value_model == ValueModel::Owned {
                "&value.values[index]"
            } else {
                "value.values[index]"
            };
            let some = self.option_expression(*result, Some((value, element)));
            let _ = writeln!(
                self.output,
                "static {result_type} {}({collection_type} value, uintptr_t index) {{\n    \
                 if (index >= value.length) return {none};\n    return {some};\n}}\n",
                standard_call_name(operation)
            );
        }
        let operation = StandardCall::VecGetVar { collection };
        if let Some((result, _)) = calls.get(&operation) {
            let result_type = self
                .c_type(*result, None)
                .unwrap_or_else(|| "el_unit".to_string());
            let none = self.option_expression(*result, None);
            let some = self.option_expression(*result, Some(("&value->values[index]", element)));
            let _ = writeln!(
                self.output,
                "static {result_type} {}({collection_type} *value, uintptr_t index) {{\n    if (index >= value->length) return {none};\n    return {some};\n}}\n",
                standard_call_name(operation)
            );
        }
        let append = StandardCall::VecAppend { collection };
        let insert = StandardCall::VecInsert { collection };
        if calls.contains_key(&append) || calls.contains_key(&insert) {
            let reserve = format!("el_vec_reserve_t{}", collection.index());
            let _ = writeln!(
                self.output,
                "static void {reserve}({collection_type} *value, uintptr_t needed) {{\n    \
                 uintptr_t capacity;\n    {element_type} *replacement;\n    \
                 if (value->capacity >= needed) return;\n    \
                 capacity = value->capacity == 0U ? 4U : value->capacity;\n    \
                 while (capacity < needed) {{ if (capacity > UINTPTR_MAX / 2U) {{ capacity = needed; break; }} capacity *= 2U; }}\n    if (capacity > SIZE_MAX / sizeof({element_type})) el_out_of_memory();"
            );
            let bytes = format!("capacity * sizeof({element_type})");
            self.emit_collection_allocate("replacement", &bytes, scanned);
            self.output.push_str(
                "    if (value->length != 0U) el_cost_memcpy(replacement, value->values, \
                 value->length * sizeof(*replacement));\n\
                 #if EL_OWNED_VALUES\n\
                 \x20   free(value->values);\n\
                 #endif\n\
                 \x20   value->values = replacement;\n\
                 \x20   value->capacity = capacity;\n}\n\n",
            );
            if calls.contains_key(&append) {
                let _ = writeln!(
                    self.output,
                    "static el_unit {}({collection_type} *value, {element_type} element) {{\n    \
                     if (value->length == UINTPTR_MAX) el_out_of_memory();\n    \
                     {reserve}(value, value->length + 1U);\n    \
                     value->values[value->length++] = element;\n    return (el_unit){{0}};\n}}\n",
                    standard_call_name(append)
                );
            }
            if calls.contains_key(&insert) {
                let _ = writeln!(
                    self.output,
                    "static el_unit {}({collection_type} *value, uintptr_t index, \
                     {element_type} element, const char *path, uint32_t line, uint32_t column) {{\n    \
                     if (index > value->length) el_trap(\"E-RUN-INDEX\", path, line, column);\n    \
                     if (value->length == UINTPTR_MAX) el_out_of_memory();\n    \
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
                "static {element_type} {}({collection_type} *value, uintptr_t index, \
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
        let pop = StandardCall::VecPop { collection };
        if let Some((result, _)) = calls.get(&pop) {
            let result_type = self
                .c_type(*result, None)
                .unwrap_or_else(|| "el_unit".to_string());
            let none = self.option_expression(*result, None);
            let some =
                self.option_expression(*result, Some(("value->values[value->length]", element)));
            let _ = writeln!(
                self.output,
                "static {result_type} {}({collection_type} *value) {{\n    if (value->length == 0U) return {none};\n    --value->length;\n    return {some};\n}}\n",
                standard_call_name(pop)
            );
        }
        if calls.contains_key(&clear) {
            let drop_elements = if element_drops {
                format!(
                    "while (value->length != 0U) {{ --value->length; el_drop_t{}(&value->values[value->length]); }} ",
                    element.index()
                )
            } else {
                "value->length = 0U; ".to_string()
            };
            let _ = writeln!(
                self.output,
                "static el_unit {}({collection_type} *value) {{ {drop_elements}return (el_unit){{0}}; }}\n",
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
                self.emit_collection_allocate("result.values", &bytes, scanned);
                let _ = writeln!(
                    self.output,
                    "    result.length = {count}U;\n    result.capacity = {count}U;"
                );
                for index in 0..*count {
                    let _ = writeln!(self.output, "    result.values[{index}] = v{index};");
                }
            }
            self.output.push_str("    return result;\n}\n\n");
        }
    }

    pub(super) fn emit_map_helpers(
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
        let key_drop_operation = [
            StandardCall::MapInsert { collection },
            StandardCall::MapRemove { collection },
            StandardCall::MapClear { collection },
        ]
        .iter()
        .any(|operation| calls.contains_key(operation));
        let value_drop_operation = calls.contains_key(&StandardCall::MapClear { collection });
        let key_drops = self.program.value_model == ValueModel::Owned
            && key_drop_operation
            && self.emit_drop_helper(key);
        let value_drops = self.program.value_model == ValueModel::Owned
            && value_drop_operation
            && self.emit_drop_helper(value);
        if self.needs_equality_helper(key) {
            self.emit_equality_helper(key, None);
        }
        let query_key = if self.program.value_model == ValueModel::Owned {
            "(*key)"
        } else {
            "key"
        };
        let key_parameter = if self.program.value_model == ValueModel::Owned {
            format!("const {key_type} *key")
        } else {
            format!("{key_type} key")
        };
        let key_equal = self.component_equality(key, "value->keys[index]", query_key);
        let stored_hash = self.component_hash(key, "value->keys[index]");
        let target_hash = self.component_hash(key, query_key);
        let new = standard_call_name(StandardCall::MapNew { collection });
        let _ = writeln!(self.output, "static {collection_type} {new}(void) {{");
        let _ = writeln!(self.output, "    {collection_type} result;");
        self.emit_collection_allocate("result", "sizeof(*result)", AllocationClass::Scanned);
        self.output.push_str(
            "    result->length = 0U;\n    result->capacity = 0U;\n    result->keys = NULL;\n    \
             result->values = NULL;\n    return result;\n}\n\n",
        );
        let find = format!("el_map_find_t{}", collection.index());
        let _ = writeln!(
            self.output,
            "intptr_t {find}({collection_type} value, {key_parameter}) {{\n    \
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
             while (capacity < needed) {{ if (capacity > UINTPTR_MAX / 2U) {{ capacity = needed; break; }} capacity *= 2U; }}\n    if (capacity > SIZE_MAX / sizeof({key_type}) || capacity > SIZE_MAX / sizeof({value_type})) el_out_of_memory();"
        );
        let key_bytes = format!("capacity * sizeof({key_type})");
        self.emit_collection_allocate("keys", &key_bytes, key_class);
        let value_bytes = format!("capacity * sizeof({value_type})");
        self.emit_collection_allocate("values", &value_bytes, value_class);
        self.output.push_str(
            "    if (value->length != 0U) {\n        el_cost_memcpy(keys, value->keys, \
             value->length * sizeof(*keys));\n        el_cost_memcpy(values, value->values, \
             value->length * sizeof(*values));\n    }\n#if EL_OWNED_VALUES\n    free(value->keys); free(value->values);\n#endif\n    value->keys = keys;\n    \
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
                "static bool {}({collection_type} value, {key_parameter}) {{ \
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
            let get_value = if self.program.value_model == ValueModel::Owned {
                "&value->values[(uintptr_t)index]"
            } else {
                "value->values[(uintptr_t)index]"
            };
            let some = self.option_expression(*result, Some((get_value, value)));
            let _ = writeln!(
                self.output,
                "static {result_type} {}({collection_type} value, {key_parameter}) {{\n    \
                 intptr_t index = {find}(value, key);\n    if (index < 0) return {none};\n    \
                 return {some};\n}}\n",
                standard_call_name(get)
            );
        }
        let get_var = StandardCall::MapGetVar { collection };
        if let Some((result, _)) = calls.get(&get_var) {
            let result_type = self
                .c_type(*result, None)
                .unwrap_or_else(|| "el_unit".to_string());
            let none = self.option_expression(*result, None);
            let some =
                self.option_expression(*result, Some(("&value->values[(uintptr_t)index]", value)));
            let _ = writeln!(
                self.output,
                "static {result_type} {}({collection_type} value, {key_parameter}) {{\n    intptr_t index = {find}(value, key);\n    if (index < 0) return {none};\n    return {some};\n}}\n",
                standard_call_name(get_var)
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
            let drop_duplicate_key = if key_drops {
                format!("el_drop_t{}(&key); ", key.index())
            } else {
                String::new()
            };
            let _ = writeln!(
                self.output,
                "static {result_type} {}({collection_type} value, {key_type} key, \
                 {value_type} replacement) {{\n    intptr_t index = {find}(value, {}) ;\n    \
                 if (index >= 0) {{\n        {result_type} previous = {some};\n        \
                 {drop_duplicate_key}value->values[(uintptr_t)index] = replacement;\n        return previous;\n    }}\n    \
                 if (value->length == UINTPTR_MAX) el_out_of_memory();\n    {reserve}(value, value->length + 1U);\n    value->keys[value->length] = key;\n    \
                 value->values[value->length] = replacement;\n    ++value->length;\n    return {none};\n}}\n",
                standard_call_name(insert),
                if self.program.value_model == ValueModel::Owned {
                    "&key"
                } else {
                    "key"
                }
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
            let drop_stored_key = if key_drops {
                format!(
                    "el_drop_t{}(&value->keys[(uintptr_t)index]);\n        ",
                    key.index()
                )
            } else {
                String::new()
            };
            let _ = writeln!(
                self.output,
                "static {result_type} {}({collection_type} value, {key_parameter}) {{\n    \
                 intptr_t index = {find}(value, key);\n    if (index < 0) return {none};\n    {{\n        \
                 {result_type} removed = {some};\n        {drop_stored_key}uintptr_t tail = value->length - \
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
            let drop_value = if value_drops {
                format!(
                    "el_drop_t{}(&value->values[value->length]); ",
                    value.index()
                )
            } else {
                String::new()
            };
            let drop_key = if key_drops {
                format!("el_drop_t{}(&value->keys[value->length]); ", key.index())
            } else {
                String::new()
            };
            let _ = writeln!(
                self.output,
                "static el_unit {}({collection_type} value) {{ while (value->length != 0U) {{ --value->length; {drop_value}{drop_key}}} return (el_unit){{0}}; }}\n",
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
                    "    found = {find}(result, {}k{index});\n    if (found >= 0) {{\n        \
                     result->values[(uintptr_t)found] = v{index};\n    }} else {{\n        \
                     {reserve}(result, result->length + 1U);\n        \
                     result->keys[result->length] = k{index};\n        \
                     result->values[result->length] = v{index};\n        ++result->length;\n    }}",
                    if self.program.value_model == ValueModel::Owned {
                        "&"
                    } else {
                        ""
                    }
                );
            }
            self.output.push_str("    return result;\n}\n\n");
        }
    }

    pub(super) fn emit_set_helpers(
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
        let element_drop_operation = [
            StandardCall::SetInsert { collection },
            StandardCall::SetRemove { collection },
            StandardCall::SetClear { collection },
        ]
        .iter()
        .any(|operation| calls.contains_key(operation));
        let element_drops = self.program.value_model == ValueModel::Owned
            && element_drop_operation
            && self.emit_drop_helper(element);
        if self.needs_equality_helper(element) {
            self.emit_equality_helper(element, None);
        }
        let query_element = if self.program.value_model == ValueModel::Owned {
            "(*element)"
        } else {
            "element"
        };
        let element_parameter = if self.program.value_model == ValueModel::Owned {
            format!("const {element_type} *element")
        } else {
            format!("{element_type} element")
        };
        let equal = self.component_equality(element, "value->values[index]", query_element);
        let stored_hash = self.component_hash(element, "value->values[index]");
        let target_hash = self.component_hash(element, query_element);
        let new = standard_call_name(StandardCall::SetNew { collection });
        let _ = writeln!(self.output, "static {collection_type} {new}(void) {{");
        let _ = writeln!(self.output, "    {collection_type} result;");
        self.emit_collection_allocate("result", "sizeof(*result)", AllocationClass::Scanned);
        self.output.push_str(
            "    result->length = 0U;\n    result->capacity = 0U;\n    result->values = NULL;\n    \
             return result;\n}\n\n",
        );
        let find = format!("el_set_find_t{}", collection.index());
        let _ = writeln!(
            self.output,
            "intptr_t {find}({collection_type} value, {element_parameter}) {{\n    \
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
             while (capacity < needed) {{ if (capacity > UINTPTR_MAX / 2U) {{ capacity = needed; break; }} capacity *= 2U; }}\n    if (capacity > SIZE_MAX / sizeof({element_type})) el_out_of_memory();"
        );
        let bytes = format!("capacity * sizeof({element_type})");
        self.emit_collection_allocate("replacement", &bytes, class);
        self.output.push_str(
            "    if (value->length != 0U) el_cost_memcpy(replacement, value->values, \
             value->length * sizeof(*replacement));\n#if EL_OWNED_VALUES\n    free(value->values);\n#endif\n    value->values = replacement;\n    \
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
                "static bool {}({collection_type} value, {element_parameter}) {{ \
                 return {find}(value, element) >= 0; }}\n",
                standard_call_name(contains)
            );
        }
        let insert = StandardCall::SetInsert { collection };
        if calls.contains_key(&insert) {
            let duplicate = if element_drops {
                format!(
                    "{{ el_drop_t{}(&element); return false; }}",
                    element.index()
                )
            } else {
                "return false;".to_string()
            };
            let _ = writeln!(
                self.output,
                "static bool {}({collection_type} value, {element_type} element) {{\n    \
                 if ({find}(value, {}) >= 0) {duplicate}\n    \
                 if (value->length == UINTPTR_MAX) el_out_of_memory();\n    {reserve}(value, value->length + 1U);\n    \
                 value->values[value->length++] = element;\n    return true;\n}}\n",
                standard_call_name(insert),
                if self.program.value_model == ValueModel::Owned {
                    "&element"
                } else {
                    "element"
                }
            );
        }
        let remove = StandardCall::SetRemove { collection };
        if calls.contains_key(&remove) {
            let drop_removed = if element_drops {
                format!(
                    "el_drop_t{}(&value->values[(uintptr_t)index]);\n    ",
                    element.index()
                )
            } else {
                String::new()
            };
            let _ = writeln!(
                self.output,
                "static bool {}({collection_type} value, {element_parameter}) {{\n    \
                 intptr_t index = {find}(value, element);\n    if (index < 0) return false;\n    {drop_removed}\
                 memmove(&value->values[(uintptr_t)index], \
                 &value->values[(uintptr_t)index + 1U], \
                 (value->length - (uintptr_t)index - 1U) * sizeof(*value->values));\n    \
                 --value->length;\n    return true;\n}}\n",
                standard_call_name(remove)
            );
        }
        let clear = StandardCall::SetClear { collection };
        if calls.contains_key(&clear) {
            let drop_elements = if element_drops {
                format!(
                    "while (value->length != 0U) {{ --value->length; el_drop_t{}(&value->values[value->length]); }} ",
                    element.index()
                )
            } else {
                "value->length = 0U; ".to_string()
            };
            let _ = writeln!(
                self.output,
                "static el_unit {}({collection_type} value) {{ {drop_elements}return (el_unit){{0}}; }}\n",
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
                    "    found = {find}(result, {}v{index});\n    if (found < 0) {{\n        \
                     {reserve}(result, result->length + 1U);\n        \
                     result->values[result->length++] = v{index};\n    }}",
                    if self.program.value_model == ValueModel::Owned {
                        "&"
                    } else {
                        ""
                    }
                );
            }
            self.output.push_str("    return result;\n}\n\n");
        }
    }

    /// The variant and field identities a checked conversion's result value is
    /// built from, or `None` when the standard declarations are unavailable.
    pub(super) fn checked_conversion_parts(
        &self,
        result: TypeId,
    ) -> Option<CheckedConversionParts> {
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
    pub(super) fn checked_pointees(&self) -> BTreeSet<TypeId> {
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
    /// (`docs/spec.md` 3.3, Milestone 16.8). The alignment comes from the C99
    /// `offsetof` probe — a `char` followed by the pointee — because
    /// `_Alignof` is C11-only and the generated C must stay C99.
    pub(super) fn emit_pointer_check_helper(&mut self, pointee: TypeId) {
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
    pub(super) fn numeric_alternatives(&self) -> BTreeSet<(NumericAlternative, TypeId, TypeId)> {
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
    pub(super) fn emit_numeric_alternative_instances(&mut self) {
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
    pub(super) fn emit_numeric_alternative_helper(
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
    pub(super) fn option_parts(&self, option: TypeId) -> Option<OptionParts> {
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
    /// `docs/spec.md` 4.3: derived equality compares components. Reference-like
    /// components compare by identity, which is what `==` on a pointer already
    /// does.
    pub(super) fn emit_equality_helper(&mut self, ty: TypeId, span: Option<Span>) {
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
            TypeKind::Builtin { builtin, arguments }
                if !matches!(
                    self.resolved.builtin_name(*builtin),
                    "Handle" | "Shared" | "Weak"
                ) =>
            {
                components.extend(arguments.iter().copied());
            }
            TypeKind::Builtin { .. } => {}
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
                if *length != 0 {
                    body.push_str(&format!(
                        "    for (uintptr_t i = 0; i < {length}u; ++i) {{\n        if (!{}) return false;\n    }}\n",
                        self.component_equality(*element, "a.values[i]", "b.values[i]")
                    ));
                }
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
                        body.push_str("    if (a.length != b.length) return false;\n");
                        body.push_str(&format!(
                            "    for (uintptr_t i = 0U; i < a.length; ++i) {{\n        \
                             if (!{}) return false;\n    }}\n",
                            self.component_equality(*element, "a.values[i]", "b.values[i]")
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
                    ("Shared" | "Weak", [_]) => {
                        body.push_str("    return a == b;\n");
                    }
                    ("Handle", [_]) => {
                        body.push_str("    return a.store == b.store && a.slot == b.slot && a.generation == b.generation;\n");
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
    pub(super) fn component_equality(&mut self, ty: TypeId, left: &str, right: &str) -> String {
        let ty = self.resolve_alias(ty);
        if self.typed.types.expanded_primitive(ty) == Some(PrimitiveType::Unit) {
            return "true".to_string();
        }
        if let TypeKind::Reference { target, .. } = self.typed.types.kind(ty)
            && matches!(
                self.typed.types.kind(self.resolve_alias(*target)),
                TypeKind::TraitObject { .. }
            )
        {
            // Trait-object reference identity is the erased target address;
            // the dispatch table is metadata, not part of the referent.
            return format!("(({left}).data == ({right}).data)");
        }
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
    pub(super) fn needs_equality_helper(&self, ty: TypeId) -> bool {
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
    pub(super) fn emit_ordering_helper(&mut self, ty: TypeId, span: Option<Span>) {
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
                if *length != 0 {
                    body.push_str(&format!(
                        "    for (uintptr_t i = 0; i < {length}u; ++i) {{\n"
                    ));
                    let expression =
                        self.component_ordering(*element, "a.values[i]", "b.values[i]");
                    append_component(&mut body, expression, "        ");
                    body.push_str("    }\n");
                }
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
                        "    uintptr_t length = a.length < b.length ? a.length : b.length;\n    \
                         for (uintptr_t i = 0U; i < length; ++i) {\n",
                    );
                    let expression =
                        self.component_ordering(*element, "a.values[i]", "b.values[i]");
                    append_component(&mut body, expression, "        ");
                    body.push_str(
                        "    }\n    if (a.length < b.length) return -1;\n    \
                         if (a.length > b.length) return 1;\n",
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
    pub(super) fn component_ordering(&mut self, ty: TypeId, left: &str, right: &str) -> String {
        let ty = self.resolve_alias(ty);
        if self.typed.types.expanded_primitive(ty) == Some(PrimitiveType::Unit) {
            return "0".to_string();
        }
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

    pub(super) fn needs_ordering_helper(&self, ty: TypeId) -> bool {
        let ty = self.resolve_alias(ty);
        match self.typed.types.kind(ty) {
            TypeKind::Builtin { builtin, .. } => self.resolved.builtin_name(*builtin) == "Vec",
            _ => self.needs_equality_helper(ty),
        }
    }

    pub(super) fn stable_key_types(&self) -> BTreeSet<TypeId> {
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

    pub(super) fn needs_hash_helper(&self, ty: TypeId) -> bool {
        let ty = self.resolve_alias(ty);
        matches!(
            self.typed.types.kind(ty),
            TypeKind::Tuple(_)
                | TypeKind::Array { .. }
                | TypeKind::Nominal { .. }
                | TypeKind::Builtin { .. }
        )
    }

    pub(super) fn component_hash(&self, ty: TypeId, value: &str) -> String {
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

    pub(super) fn emit_hash_helper(&mut self, ty: TypeId, span: Option<Span>) {
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
            TypeKind::Builtin { builtin, arguments }
                if !matches!(
                    self.resolved.builtin_name(*builtin),
                    "Handle" | "Shared" | "Weak"
                ) =>
            {
                arguments.clone()
            }
            TypeKind::Builtin { .. } => Vec::new(),
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
                if *length != 0 {
                    let value = self.component_hash(*element, "value.values[index]");
                    let _ = writeln!(
                        body,
                        "    for (uintptr_t index = 0U; index < {length}U; ++index) \
                         hash = el_hash_combine(hash, {value});"
                    );
                }
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
            TypeKind::Builtin { builtin, arguments }
                if matches!(self.resolved.builtin_name(*builtin), "Shared" | "Weak")
                    && arguments.len() == 1 =>
            {
                body.push_str(
                    "    hash = el_hash_combine(hash, el_hash_u64((uint64_t)(uintptr_t)value));\n",
                );
            }
            TypeKind::Builtin { builtin, arguments }
                if self.resolved.builtin_name(*builtin) == "Handle" && arguments.len() == 1 =>
            {
                body.push_str("    hash = el_hash_combine(hash, el_hash_u64((uint64_t)value.store));\n    hash = el_hash_combine(hash, el_hash_u64((uint64_t)value.slot));\n    hash = el_hash_combine(hash, el_hash_u64((uint64_t)value.generation));\n");
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
}

fn callable_tuple_arguments(
    types: &TypeContext,
    signature: &crate::types::FunctionSignature,
) -> Vec<String> {
    let Some(parameter) = signature.parameters.first() else {
        return Vec::new();
    };
    match types.kind(types.resolve_inference(parameter.ty)) {
        TypeKind::Tuple(elements) => (0..elements.len())
            .map(|index| format!("a0.v{index}"))
            .collect(),
        TypeKind::Primitive(PrimitiveType::Unit) => Vec::new(),
        _ => vec!["a0".to_string()],
    }
}
