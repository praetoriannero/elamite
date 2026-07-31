//! Executable entry-shim emission.

use super::*;

impl<'a> CEmitter<'a> {
    pub(super) fn emit_entry(&mut self) {
        if let Some(entries) = &self.options.test_entries {
            self.output.push_str("int main(void) {\n");
            if self.program.requires_managed_memory {
                self.emit_managed_operation(ManagedMemoryOperation::Initialize);
            }
            self.output
                .push_str("    const char *selected = getenv(\"ELAMITE_TEST\");\n");
            if entries.is_empty() {
                self.output
                    .push_str("    (void)selected;\n    return 0;\n}\n");
                return;
            }
            self.output
                .push_str("    if (selected == NULL) return 2;\n");
            for (declaration, name) in entries {
                let instance = FunctionInstance {
                    declaration: *declaration,
                    arguments: Vec::new(),
                    self_type: None,
                };
                let symbol = self.function_symbol(&instance);
                let _ = writeln!(
                    self.output,
                    "    if (strcmp(selected, {}) == 0) {{ (void){symbol}(); return 0; }}",
                    c_string(name)
                );
            }
            self.output.push_str("    return 2;\n}\n");
            return;
        }
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
}
