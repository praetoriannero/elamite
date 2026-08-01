//! Type/layout declarations and C type selection.

use super::*;

impl<'a> CEmitter<'a> {
    pub(super) fn emit_forward_structs(&mut self) {
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

    pub(super) fn used_types(&self) -> BTreeSet<TypeId> {
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
            if let Some(closure) = &function.closure {
                types.insert(closure.ty);
            }
            if !matches!(
                self.typed
                    .types
                    .kind(self.typed.types.resolve_inference(function.return_type)),
                TypeKind::Never
            ) {
                types.insert(function.return_type);
            }
            types.extend(function.parameters.iter().map(|parameter| parameter.ty));
            types.extend(function.local_types.values().copied());
            types.extend(function.temporary_types.iter().copied().filter(|ty| {
                !matches!(
                    self.typed
                        .types
                        .kind(self.typed.types.resolve_inference(*ty)),
                    TypeKind::Never
                )
            }));
        }
        types
    }

    pub(super) fn emit_type_definition(&mut self, ty: TypeId, span: Option<Span>) {
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
                    // ISO C99 has no zero-length arrays. The backing slot is
                    // never observable through Elamite's length-zero type.
                    let storage_length = (*length).max(1);
                    let _ = writeln!(
                        self.output,
                        "typedef struct {name} {{ {c_type} values[{storage_length}]; }} {name};\n"
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
            TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => {
                // A pointee can require its own concrete C declaration even
                // when it is reachable only through a pointer (for example,
                // `*(i32, i32)` followed by `.0`). Recursive nominal graphs
                // remain safe because `emitting_types` breaks the cycle and
                // nominal types already have forward declarations.
                self.emit_type_definition(*target, span);
            }
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
            TypeKind::Closure { captures, .. } => {
                for capture in captures {
                    self.emit_type_definition(*capture, span);
                }
                let name = closure_name(ty);
                let _ = writeln!(self.output, "typedef struct {name}_data {{");
                if captures.is_empty() {
                    self.output.push_str("    uint8_t _value;\n");
                } else {
                    for (index, capture) in captures.iter().enumerate() {
                        if let Some(c_type) = self.c_type(*capture, span) {
                            let _ = writeln!(self.output, "    {c_type} v{index};");
                        }
                    }
                }
                let _ = writeln!(self.output, "}} *{name};\n");
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

    pub(super) fn emit_default_helper(&mut self, ty: TypeId, span: Option<Span>) {
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
                if length != 0
                    && let Some(value) = self.component_default(element, span)
                {
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

    pub(super) fn needs_default_helper(&self, ty: TypeId) -> bool {
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

    pub(super) fn component_default(&mut self, ty: TypeId, span: Option<Span>) -> Option<String> {
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
    pub(super) fn default_expression(&mut self, ty: TypeId, span: Span) -> Option<String> {
        self.component_default(ty, Some(span))
    }

    /// Whether an allocation of `ty` may contain managed pointers and must
    /// therefore be scanned by the collector. Conservative: only types proven
    /// free of references, pointers, and owned buffers are left unscanned.
    pub(super) fn scanned_allocation(&self, ty: TypeId) -> bool {
        fn walk(types: &TypeContext, ty: TypeId, depth: u32) -> bool {
            if depth == 0 {
                return true;
            }
            match types.kind(types.resolve_inference(ty)) {
                TypeKind::Never => false,
                TypeKind::Primitive(primitive) => {
                    // A `String` owns a heap buffer; every other primitive is
                    // a plain scalar.
                    matches!(primitive, PrimitiveType::String)
                }
                TypeKind::Reference { .. }
                | TypeKind::RawPointer { .. }
                | TypeKind::Function { .. }
                | TypeKind::TraitObject { .. }
                | TypeKind::Closure { .. }
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

    pub(super) fn c_type(&mut self, ty: TypeId, span: Option<Span>) -> Option<String> {
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
                object_name(trait_declaration, trait_type)
            }
            TypeKind::Reference { target, .. } | TypeKind::RawPointer { target, .. } => {
                format!("{} *", self.c_type(*target, span)?)
            }
            TypeKind::Function { .. } => function_type_name(ty),
            TypeKind::Closure { .. } => closure_name(ty),
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

    pub(super) fn c_function_return_type(
        &mut self,
        ty: TypeId,
        span: Option<Span>,
    ) -> Option<String> {
        if matches!(
            self.typed
                .types
                .kind(self.typed.types.resolve_inference(ty)),
            TypeKind::Never
        ) || matches!(
            self.typed.types.expanded_primitive(ty),
            Some(PrimitiveType::Unit)
        ) {
            Some("void".to_string())
        } else {
            self.c_type(ty, span)
        }
    }

    pub(super) fn call_rvalue(&self, call: String, destination_type: TypeId) -> String {
        if matches!(
            self.typed.types.expanded_primitive(destination_type),
            Some(PrimitiveType::Unit)
        ) {
            format!("({call}, (el_unit){{0}})")
        } else {
            call
        }
    }

    pub(super) fn resolve_alias(&self, mut ty: TypeId) -> TypeId {
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

    pub(super) fn type_error(&mut self, ty: TypeId, span: Option<Span>, message: &str) {
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

    pub(super) fn trap_arguments(&self, span: Span) -> String {
        let location = self.location(span);
        format!(
            "{}, UINT32_C({}), UINT32_C({})",
            c_string(&location.path),
            location.line,
            location.column
        )
    }

    pub(super) fn location(&self, span: Span) -> SourceLocation {
        let position = self.sources.line_col(span.file, span.start);
        SourceLocation {
            path: self.sources.path(span.file).display().to_string(),
            line: position.line,
            column: position.column,
        }
    }
}
