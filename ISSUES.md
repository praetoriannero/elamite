# Elamite Design Issues

> This document records only unresolved design questions for Elamite's managed,
> independent-value-copy direction. Resolved decisions belong in `SPEC.md` and
> the authoritative `examples/spec_demo.elx` demonstration.

## Issues to resolve before initial implementation begins

These issues determine the initial parser, type checker, and core runtime
model. None are currently open: `I-017` is resolved (below). Two
implementation/tooling decisions surfaced while resolving it — the minimum C
compiler requirement and the supported OS/architecture matrix — are not
language-design questions and are tracked in [`LEDGER.md`](LEDGER.md#13-target-assumptions)
instead of here.

## I-017: Specification migration and consistency

**Resolved.** `SPEC.md` and the authoritative demonstration define the managed,
independent-value-copy direction. The prior lexer, parser, analyzer, and
fixtures that encoded earlier grammar and semantic assumptions have been
removed entirely rather than reconciled; there is no legacy implementation
left to migrate. [`LEDGER.md`](LEDGER.md) maps every normative rule in
`SPEC.md` to an implementation milestone, closing this issue per `IMPL.md`
Milestone 0's exit criteria.

## Issues to refine after initial implementation begins

These questions remain visible in the language design but do not block the
initial parser and type checker.

## I-015: Concurrency and asynchronous execution

The interaction among independent value copying, explicit references, mutable
aliases, function references, trait-object callbacks, garbage collection,
resource cleanup, and tasks is unaddressed. The language needs a model for data
races, synchronization, ownership or sharing across tasks, cancellation, and
task-local resource cleanup before concurrency is added.
