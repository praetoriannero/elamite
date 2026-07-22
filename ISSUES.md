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
aliases, closures, garbage collection, resource cleanup, and tasks is
unaddressed. The language needs a model for data races, synchronization,
ownership or sharing across tasks, cancellation, and task-local resource
cleanup before concurrency is added.

## I-016: Foreign-function interface and unsafe code

A C-oriented implementation needs rules for scalar and aggregate marshalling,
references retained by foreign code, callbacks, native resource handles,
promotion and root registration, pointer ownership, callback retention, and
foreign exceptions. It must also define whether an `unsafe fn` body still
requires explicit `unsafe:` blocks for individual unsafe operations.

## I-020: Raw-pointer provenance and violations

Raw-pointer provenance remains undefined. The language must decide the
consequence of violating raw dereference or raw-to-reference obligations, such
as a mandatory trap versus undefined behavior, and which violations require a
compile-time error, warning, runtime check, or no diagnostic.
