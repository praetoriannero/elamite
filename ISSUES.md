# Elamite Design Issues

> This document records only unresolved design questions for Elamite's managed,
> independent-value-copy direction. Resolved decisions belong in `SPEC.md` and
> the authoritative `examples/spec_demo.elx` demonstration. Implementation
> rationale that remains useful belongs in `LEDGER.md`.

## I-015: Concurrency and asynchronous execution

The interaction among independent value copying, explicit references, mutable
aliases, function references, trait-object callbacks, garbage collection,
resource cleanup, and tasks is unaddressed. The language needs a model for data
races, synchronization, ownership or sharing across tasks, cancellation, and
task-local resource cleanup before concurrency is added.
