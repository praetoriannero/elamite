//! Runtime prelude, helpers, managed-memory operations, and value operations.

use super::*;

impl<'a> CEmitter<'a> {
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
                                    | StandardCall::AtomicFetchAdd { .. },
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
                            let mut arguments = vec![format!("*(({concrete_type} *)el_self)")];
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
        self.output.push_str(
            "/* Generated by Elamite. C99; internal ABI is intentionally unstable. */\n\
             #include <stdbool.h>\n\
             #include <stddef.h>\n\
             #include <stdint.h>\n\
             #include <inttypes.h>\n\
             #include <limits.h>\n\
             #include <float.h>\n\
             #include <math.h>\n\
             #include <pthread.h>\n\
             #include <stdio.h>\n\
             #include <stdlib.h>\n\n\
             #include <string.h>\n\
             #include <sys/types.h>\n\
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
             \x20\x20\x20\x20result.bytes = (const char *)el_runtime_alloc_atomic(length + 1U);\n\
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

    pub(super) fn option_expression(
        &mut self,
        option: TypeId,
        value: Option<(&str, TypeId)>,
    ) -> String {
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
            return zero_value(result, &self.typed.types);
        };
        let Some(variant_id) = self.resolved.standard_variant("Result", variant_name) else {
            return zero_value(result, &self.typed.types);
        };
        let Some(variant) = enumeration
            .variants
            .iter()
            .find(|variant| variant.id == variant_id)
        else {
            return zero_value(result, &self.typed.types);
        };
        let Some((field, _, _)) = variant.fields.first() else {
            return zero_value(result, &self.typed.types);
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
        self.emit_thread_helpers(&calls);
        self.emit_channel_helpers(&calls);
        self.emit_mutex_helpers(&calls);
        self.emit_atomic_helpers(&calls);
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
        let scanned = if self.scanned_allocation(element) {
            AllocationClass::Scanned
        } else {
            AllocationClass::PointerFree
        };
        let new = standard_call_name(StandardCall::VecNew { collection });
        let _ = writeln!(
            self.output,
            "static {collection_type} {new}(void) {{ return ({collection_type}){{0}}; }}\n"
        );

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
            self.emit_runtime_allocate("result.values", &bytes, scanned);
            let _ = writeln!(
                self.output,
                "    }}\n    for (uintptr_t index = 0U; index < left.length; ++index) \
                 result.values[index] = left.values[index];\n    \
                 for (uintptr_t index = 0U; index < right.length; ++index) \
                 result.values[left.length + index] = right.values[index];\n    \
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
            let some = self.option_expression(*result, Some(("value.values[index]", element)));
            let _ = writeln!(
                self.output,
                "static {result_type} {}({collection_type} value, uintptr_t index) {{\n    \
                 if (index >= value.length) return {none};\n    return {some};\n}}\n",
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
                 while (capacity < needed) capacity *= 2U;"
            );
            let bytes = format!("capacity * sizeof({element_type})");
            self.emit_runtime_allocate("replacement", &bytes, scanned);
            self.output.push_str(
                "    if (value->length != 0U) el_cost_memcpy(replacement, value->values, \
                 value->length * sizeof(*replacement));\n\
                 \x20   value->values = replacement;\n\
                 \x20   value->capacity = capacity;\n}\n\n",
            );
            if calls.contains_key(&append) {
                let _ = writeln!(
                    self.output,
                    "static el_unit {}({collection_type} *value, {element_type} element) {{\n    \
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
        let clear = StandardCall::VecClear { collection };
        if calls.contains_key(&clear) {
            let _ = writeln!(
                self.output,
                "static el_unit {}({collection_type} *value) {{ value->length = 0U; \
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
                self.emit_runtime_allocate("result.values", &bytes, scanned);
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
            "    if (value->length != 0U) {\n        el_cost_memcpy(keys, value->keys, \
             value->length * sizeof(*keys));\n        el_cost_memcpy(values, value->values, \
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
            "    if (value->length != 0U) el_cost_memcpy(replacement, value->values, \
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
