# Elamite Design Issues

> This document records only unresolved design questions for Elamite's managed,
> independent-value-copy direction. Resolved decisions belong in `SPEC.md` and
> the authoritative `examples/spec_demo.elx` demonstration.

## Issues to resolve before initial implementation begins

These issues determine the initial parser, type checker, and core runtime
model.

## I-017: Specification migration and consistency

`SPEC.md` and the authoritative demonstration define the managed,
independent-value-copy direction, but the existing lexer, parser, analyzer, and
older fixtures still encode earlier grammar and semantic assumptions.
Implementation work must reconcile those components after the remaining
blocking design issues are resolved and the specification is considered stable.

## Issues to refine after initial implementation begins

These questions remain visible in the language design but do not block the
initial parser and type checker.

## I-015: Concurrency and asynchronous execution

The interaction among independent value copying, explicit references, mutable
aliases, function references, trait-object callbacks, garbage collection,
resource cleanup, and tasks is unaddressed. The language needs a model for data
races, synchronization, ownership or sharing across tasks, cancellation, and
task-local resource cleanup before concurrency is added.
