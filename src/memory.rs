//! Collector-neutral managed-memory backend contract.
//!
//! Milestone 8 does not allocate managed storage, but this interface fixes the
//! boundary Milestone 10 will use instead of teaching semantic lowering about
//! a particular collector. Boehm is the default strategy; another non-moving
//! tracing collector, or a reference-counting strategy with cycle detection,
//! can implement the same operations while preserving the language semantics.

use std::fmt::{self, Write};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocationClass {
    /// The allocation may contain managed pointers and must be scanned.
    Scanned,
    /// The allocation contains no managed pointers.
    PointerFree,
}

/// Collector operations exposed to C runtime lowering.
///
/// Operands are already-validated C lvalues or expressions produced by the
/// backend, never user-provided source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedMemoryOperation<'a> {
    Initialize,
    Allocate {
        destination: &'a str,
        byte_count: &'a str,
        class: AllocationClass,
    },
    Collect,
    RegisterRoot {
        start: &'a str,
        byte_count: &'a str,
    },
    UnregisterRoot {
        start: &'a str,
        byte_count: &'a str,
    },
    KeepAlive {
        expression: &'a str,
    },
}

/// Internal, unstable interface between managed-memory lowering and a runtime
/// strategy. Implementations must be non-moving and reclaim unreachable cycles
/// to satisfy the initial Elamite language contract.
pub trait ManagedMemoryStrategy: Send + Sync {
    fn name(&self) -> &'static str;

    fn native_libraries(&self) -> &'static [&'static str];

    fn emit_c_prelude(&self, output: &mut dyn Write) -> fmt::Result;

    fn emit_c_operation(
        &self,
        operation: ManagedMemoryOperation<'_>,
        output: &mut dyn Write,
    ) -> fmt::Result;
}

#[derive(Debug, Default)]
pub struct BoehmGarbageCollector;

impl ManagedMemoryStrategy for BoehmGarbageCollector {
    fn name(&self) -> &'static str {
        "boehm"
    }

    fn native_libraries(&self) -> &'static [&'static str] {
        &["gc"]
    }

    fn emit_c_prelude(&self, output: &mut dyn Write) -> fmt::Result {
        output.write_str("#include <gc.h>\n")
    }

    fn emit_c_operation(
        &self,
        operation: ManagedMemoryOperation<'_>,
        output: &mut dyn Write,
    ) -> fmt::Result {
        match operation {
            ManagedMemoryOperation::Initialize => {
                // Interior pointers must be traced: a safe reference into an
                // aggregate points inside a managed allocation rather than at
                // its base (`LEDGER.md` 19). This is Boehm's default in most
                // builds, but the language requires it, so it is requested
                // explicitly and before `GC_INIT`.
                output.write_str(
                    "GC_set_all_interior_pointers(1);\n\
                     \x20\x20\x20\x20GC_INIT();\n",
                )
            }
            ManagedMemoryOperation::Allocate {
                destination,
                byte_count,
                class,
            } => {
                let allocator = match class {
                    AllocationClass::Scanned => "GC_MALLOC",
                    AllocationClass::PointerFree => "GC_MALLOC_ATOMIC",
                };
                writeln!(output, "{destination} = {allocator}({byte_count});")
            }
            ManagedMemoryOperation::Collect => output.write_str("GC_gcollect();\n"),
            ManagedMemoryOperation::RegisterRoot { start, byte_count } => {
                writeln!(
                    output,
                    "GC_add_roots((void *)({start}), (void *)((char *)({start}) + ({byte_count})));"
                )
            }
            ManagedMemoryOperation::UnregisterRoot { start, byte_count } => {
                writeln!(
                    output,
                    "GC_remove_roots((void *)({start}), (void *)((char *)({start}) + ({byte_count})));"
                )
            }
            ManagedMemoryOperation::KeepAlive { expression } => {
                writeln!(output, "GC_reachable_here({expression});")
            }
        }
    }
}

static BOEHM: BoehmGarbageCollector = BoehmGarbageCollector;

#[must_use]
pub fn default_managed_memory_strategy() -> &'static dyn ManagedMemoryStrategy {
    &BOEHM
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boehm_is_the_default_strategy() {
        let strategy = default_managed_memory_strategy();
        assert_eq!(strategy.name(), "boehm");
        assert_eq!(strategy.native_libraries(), ["gc"]);
    }

    #[test]
    fn boehm_operations_are_lowered_behind_the_strategy_boundary() {
        let strategy = default_managed_memory_strategy();
        let mut output = String::new();
        strategy
            .emit_c_operation(
                ManagedMemoryOperation::Allocate {
                    destination: "cell",
                    byte_count: "sizeof(*cell)",
                    class: AllocationClass::Scanned,
                },
                &mut output,
            )
            .expect("writing to a string succeeds");
        assert_eq!(output, "cell = GC_MALLOC(sizeof(*cell));\n");
    }
}
