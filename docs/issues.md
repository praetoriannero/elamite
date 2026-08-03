# Elamite Design Issues

> This document records only unresolved design questions for Elamite's managed,
> shallow-copy direction. Resolved decisions belong in `spec.md` and
> the authoritative `examples/spec_demo.elx` demonstration. Implementation
> rationale that remains useful belongs in `ledger.md`.

There are currently no unresolved design reviews. Specification 0.10 has
settled shallow ordinary copies, programmer-managed shared-memory concurrency,
data-race undefined behavior, and unsafe raw-pointer arithmetic, indexing,
subtraction, and null-low relational ordering. Implementation work is tracked
in `roadmap.md`; any further surface change requires a new issue entry.
