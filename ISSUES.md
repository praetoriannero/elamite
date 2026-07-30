# Elamite Design Issues

> This document records only unresolved design questions for Elamite's managed,
> independent-value-copy direction. Resolved decisions belong in `SPEC.md` and
> the authoritative `examples/spec_demo.elx` demonstration. Implementation
> rationale that remains useful belongs in `LEDGER.md`.

## I-015: Concurrency and asynchronous execution

The standard-library native-thread design has been accepted for implementation
planning in `ROADMAP.md` **Standard-library concurrency**. This issue remains
open only until its **Normative concurrency contract** package moves the
`Transfer`, thread lifecycle, synchronization, memory-ordering, cleanup, GC,
and restricted callback rules into `SPEC.md` and `LEDGER.md`. Cooperative
tasks, `async`/`await`, cancellation, detached execution, relaxed atomics,
scoped references, and foreign-thread attachment remain deferred and must
receive separate design review before any later milestone admits them.
