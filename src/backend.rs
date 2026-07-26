//! Deterministic C99 emission and native-toolchain invocation support.
//!
//! This is the first executable backend (`IMPL.md` Milestones 8-9). It consumes
//! explicit control-flow IR, uses an internal (unstable) calling convention,
//! emits one strictly sequenced C statement per IR instruction, and routes
//! every supported value copy through a generated per-type helper.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::diagnostics::{Category, Diagnostic};
use crate::ir::{
    AggregateValue, BinaryOperator, ControlFlowFunction, ControlFlowPlace, ControlFlowProgram,
    Instruction, LogicalCopyStrategy, Rvalue, TemporaryId, Terminator, TypedEnum, UnaryOperator,
    logical_copy_strategy,
};
use crate::memory::{
    AllocationClass, ManagedMemoryOperation, ManagedMemoryStrategy, default_managed_memory_strategy,
};
use crate::resolution::{DeclarationId, ResolvedProgram, VariantId};
use crate::source::{SourceManager, Span};
use crate::types::{PrimitiveType, TypeContext, TypeId, TypeKind, TypedProgram};

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
/// concrete generic arguments can be appended by Milestone 12.
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
    structs: BTreeMap<DeclarationId, &'a crate::ir::TypedStruct>,
    enums: BTreeMap<DeclarationId, &'a TypedEnum>,
    emitted_copy_helpers: BTreeSet<TypeId>,
    emitting_copy_helpers: BTreeSet<TypeId>,
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
                .map(|structure| (structure.declaration, structure))
                .collect(),
            enums: program
                .enums
                .iter()
                .map(|enumeration| (enumeration.declaration, enumeration))
                .collect(),
            emitted_copy_helpers: BTreeSet::new(),
            emitting_copy_helpers: BTreeSet::new(),
            strategy: default_managed_memory_strategy(),
            promoted: BTreeSet::new(),
        }
    }

    fn run(mut self) -> COutput {
        self.emit_prelude();
        self.emit_managed_memory_prelude();
        self.emit_forward_structs();
        let used_types = self.used_types();
        for ty in &used_types {
            self.emit_type_definition(*ty, None);
        }
        for ty in used_types {
            self.emit_copy_helper(ty, None);
        }
        self.emit_prototypes();
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

    /// Emits the collector's declarations behind the `ManagedMemoryStrategy`
    /// boundary. A program with no managed storage emits nothing, keeping the
    /// translation unit free of any collector dependency.
    fn emit_managed_memory_prelude(&mut self) {
        if !self.program.requires_managed_memory {
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
        // Allocation failure attempts a full collection and then terminates
        // without running deferred cleanup (`SPEC.md` 9).
        self.output.push_str("\nvoid el_out_of_memory(void) {\n");
        self.emit_managed_operation(ManagedMemoryOperation::Collect);
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
             static void el_trap(const char *code, const char *path, uint32_t line, uint32_t column) {\n\
             \x20\x20\x20\x20fprintf(stderr, \"elamite trap [%s] at %s:%\" PRIu32 \":%\" PRIu32 \"\\n\", code, path, line, column);\n\
             \x20\x20\x20\x20fflush(stderr);\n\
             \x20\x20\x20\x20exit(101);\n\
             }\n\n\
             char *el_copy_string(const char *value) {\n\
             \x20\x20\x20\x20size_t length;\n\
             \x20\x20\x20\x20char *copy;\n\
             \x20\x20\x20\x20if (value == NULL) return NULL;\n\
             \x20\x20\x20\x20length = strlen(value) + 1U;\n\
             \x20\x20\x20\x20copy = (char *)malloc(length);\n\
             \x20\x20\x20\x20if (copy == NULL) exit(101);\n\
             \x20\x20\x20\x20memcpy(copy, value, length);\n\
             \x20\x20\x20\x20return copy;\n\
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
    }

    fn emit_forward_structs(&mut self) {
        for structure in &self.program.structs {
            let name = struct_name(structure.declaration);
            let _ = writeln!(self.output, "typedef struct {name} {name};");
        }
        for enumeration in &self.program.enums {
            let name = enum_name(enumeration.declaration);
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
            TypeKind::Nominal { identity, .. } => {
                if let Some(structure) = self.structs.get(&identity.declaration).copied() {
                    for (_, _, field_type) in &structure.fields {
                        self.emit_type_definition(*field_type, span);
                    }
                    let name = struct_name(identity.declaration);
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
                } else if let Some(enumeration) = self.enums.get(&identity.declaration).copied() {
                    for variant in &enumeration.variants {
                        for (_, _, field_type) in &variant.fields {
                            self.emit_type_definition(*field_type, span);
                        }
                    }
                    let name = enum_name(identity.declaration);
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
                        "only non-generic structs and enums have a C representation",
                    );
                }
            }
            _ => {}
        }
        self.emitting_types.remove(&ty);
        self.emitted_types.insert(ty);
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
            TypeKind::Nominal { identity, .. } => {
                if let Some(structure) = self.structs.get(&identity.declaration).copied() {
                    for (_, _, field_type) in &structure.fields {
                        self.emit_copy_helper(*field_type, span);
                    }
                } else if let Some(enumeration) = self.enums.get(&identity.declaration).copied() {
                    for variant in &enumeration.variants {
                        for (_, _, field_type) in &variant.fields {
                            self.emit_copy_helper(*field_type, span);
                        }
                    }
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
            TypeKind::Nominal { identity, .. } => {
                if let Some(structure) = self.structs.get(&identity.declaration).copied() {
                    let _ = writeln!(self.output, "    {c_type} result = {{0}};");
                    for (field, _, field_type) in &structure.fields {
                        let helper = copy_helper_name(self.resolve_alias(*field_type));
                        let field = field_name(*field);
                        let _ =
                            writeln!(self.output, "    result.{field} = {helper}(value.{field});");
                    }
                    self.output.push_str("    return result;\n");
                } else if let Some(enumeration) = self.enums.get(&identity.declaration).copied() {
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
            let Some(return_type) = self.c_type(function.return_type, Some(function.span)) else {
                continue;
            };
            let symbol = mangle_declaration(self.resolved, function.declaration);
            let parameters = self.parameter_list(function);
            let _ = writeln!(self.output, "{return_type} {symbol}({parameters});");
        }
        self.output.push('\n');
    }

    fn emit_function(&mut self, function: &ControlFlowFunction) {
        self.promoted = function.promoted_locals.clone();
        let Some(return_type) = self.c_type(function.return_type, Some(function.span)) else {
            self.promoted.clear();
            return;
        };
        let symbol = mangle_declaration(self.resolved, function.declaration);
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
        let mut referenced_blocks = BTreeSet::from([function.entry]);
        for block in &function.blocks {
            match block.terminator {
                Terminator::Goto(target) => {
                    referenced_blocks.insert(target);
                }
                Terminator::Branch {
                    then_block,
                    else_block,
                    ..
                } => {
                    referenced_blocks.insert(then_block);
                    referenced_blocks.insert(else_block);
                }
                Terminator::Return(_) | Terminator::Trap { .. } | Terminator::Unreachable => {}
            }
        }
        for block in &function.blocks {
            if !referenced_blocks.contains(&block.id) {
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
            Instruction::PrintText { text, .. } => {
                let _ = writeln!(self.output, "    fputs({}, stdout);", c_string(text));
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
            Rvalue::Load(place) => self.place_expression(place),
            Rvalue::AddressOf(place) => format!("&{}", self.place_expression(place)),
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
                if matches!(
                    self.typed.types.expanded_primitive(*operand_type),
                    Some(PrimitiveType::Str | PrimitiveType::String)
                ) {
                    format!("(strcmp({left}, {right}) == 0)")
                } else {
                    format!("({left} == {right})")
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
                declaration,
                arguments,
            } => format!(
                "{}({})",
                mangle_declaration(self.resolved, *declaration),
                arguments
                    .iter()
                    .map(|argument| temporary_name(*argument))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
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
            crate::ir::Constant::String(value) => c_string(value),
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
                        format!(".{} = {}", field_name(*field), temporary_name(*value))
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            AggregateValue::Enum {
                declaration,
                variant,
                fields,
            } => {
                let member = variant_member_name(*variant);
                if fields.is_empty() {
                    let payload = self
                        .enums
                        .get(declaration)
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
                length,
                trap,
            } => {
                self.emit_place_checks(Some(base), span);
                let arguments = self.trap_arguments(span);
                let _ = writeln!(
                    self.output,
                    "    if ((uintmax_t){} >= UINTMAX_C({length})) el_trap(\"{}\", {arguments});",
                    temporary_name(*index),
                    trap.code()
                );
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
                format!("{}.{}", self.place_expression(base), field_name(*field))
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
            ControlFlowPlace::Index { base, index, .. } => format!(
                "{}.values[{}]",
                self.place_expression(base),
                temporary_name(*index)
            ),
        }
    }

    fn emit_print(&mut self, value: TemporaryId, ty: TypeId, span: Span) {
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
                let _ = writeln!(self.output, "    fputs({value}, stdout);");
            }
            _ => self.type_error(
                ty,
                Some(span),
                "printing this type requires the Milestone 13 `Display` implementation",
            ),
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
                let _ = writeln!(self.output, "    return {};", temporary_name(*value));
            }
            Terminator::Return(None) => {
                if matches!(
                    self.typed.types.expanded_primitive(function.return_type),
                    Some(PrimitiveType::Unit)
                ) {
                    self.output.push_str("    return (el_unit){0};\n");
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
        if !function.parameters.is_empty() || !unit {
            self.diagnostics.push(
                Diagnostic::new(
                    Category::CodeGeneration,
                    "the Milestone 8 executable entry must have signature `fn main() -> ()`",
                )
                .with_primary(function.span),
            );
            return;
        }
        let symbol = mangle_declaration(self.resolved, entry);
        // The collector is initialized before any Elamite code runs. A library
        // package emits no shim, so initialization is the linking
        // executable's responsibility.
        self.output.push_str("int main(void) {\n");
        if self.program.requires_managed_memory {
            self.emit_managed_operation(ManagedMemoryOperation::Initialize);
        }
        let _ = writeln!(self.output, "    (void){symbol}();\n    return 0;\n}}");
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
                PrimitiveType::Str => "const char *",
                PrimitiveType::String => "char *",
            }
            .to_string(),
            TypeKind::Tuple(_) => tuple_name(ty),
            TypeKind::Array { .. } => array_name(ty),
            TypeKind::Nominal { identity, .. }
                if self.structs.contains_key(&identity.declaration) =>
            {
                struct_name(identity.declaration)
            }
            TypeKind::Nominal { identity, .. }
                if self.enums.contains_key(&identity.declaration) =>
            {
                enum_name(identity.declaration)
            }
            // `&T`, `&var T`, `*T`, and `*var T` are all `T *`; mutability is
            // compile-time only (LEDGER 19).
            TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => {
                format!("{} *", self.c_type(*target, span)?)
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

    fn resolve_alias(&self, mut ty: TypeId) -> TypeId {
        loop {
            match self.typed.types.kind(ty) {
                TypeKind::Alias { target, .. } => ty = *target,
                _ => return ty,
            }
        }
    }

    fn type_error(&mut self, ty: TypeId, span: Option<Span>, message: &str) {
        let mut diagnostic = Diagnostic::new(
            Category::CodeGeneration,
            format!("{message} (canonical type {})", ty.index()),
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

fn struct_name(declaration: DeclarationId) -> String {
    format!("el_struct_d{}", declaration.index())
}

fn enum_name(declaration: DeclarationId) -> String {
    format!("el_enum_d{}", declaration.index())
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
            | TypeKind::Nominal { .. }
            | TypeKind::Foreign { .. } => return "{0}".to_string(),
            TypeKind::Primitive(PrimitiveType::Str | PrimitiveType::String)
            | TypeKind::Reference { .. }
            | TypeKind::RawPointer { .. }
            | TypeKind::Function { .. }
            | TypeKind::TraitObject { .. } => return "NULL".to_string(),
            _ => return "0".to_string(),
        }
    }
}
