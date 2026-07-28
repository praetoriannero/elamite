# Elamite Design Issues

> This document records only unresolved design questions for Elamite's managed,
> independent-value-copy direction. Resolved decisions belong in `SPEC.md` and
> the authoritative `examples/spec_demo.elx` demonstration.

## Issues to resolve before initial implementation begins

These issues determine the initial parser, type checker, and core runtime
model. None are currently open: `I-017` through `I-021` are all resolved
(below). Two implementation/tooling decisions surfaced while resolving `I-017`
— the minimum C compiler requirement and the supported OS/architecture matrix —
are not
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

## I-018: Reference target model for nested aggregates

**Resolved.** `SPEC.md` §3.2 previously stated that a reference into a nested
aggregate targeted the *selected subvalue* and was not rebased by a later
replacement of its container, with a worked example printing the former value.
That rule has been replaced: a reference names storage, and any assignment
that overwrites that storage is observable through the reference — including
replacement of a containing aggregate.

Raised during Milestone 10 implementation. The rule had no analogue in either
reference language: in both C and Go, `&user.address.city` is a pointer into
the container's storage, and assigning a new value to the container is visible
through it. Implementing the former rule required boxing every address-taken
field into its own managed cell, which made the C layout of a nominal type
depend on a whole-program analysis and required `Milestone 9`'s copy helpers
to deep-copy through those boxes to preserve independence — a silent-aliasing
failure mode in exchange for behavior users of C or Go would not predict.

The replacement rule unifies the two reference cases into one statement,
keeps `&T` a single C representation (`T *`) as `Milestone 11`, `Milestone 13`,
and `Milestone 17` all require, leaves struct layouts flat, and needs no
whole-program analysis. Its consequences are recorded in
[`LEDGER.md`](LEDGER.md#19-reference-storage-model-m10): interior pointers must
be traced by the collector, and a reference into an aggregate keeps its whole
container reachable — matching Go.

## I-019: Trait-object syntax and the `std` path root

**Resolved.** Two keywords are removed from the language, leaving 32.

`dyn` is gone. A trait object is written `&Trait` or `&var Trait`, and forming
one is an explicit conversion — `reference as &Trait` — rather than an implicit
coercion. Raised while reviewing the keyword list. `dyn` was fully predictable
in Elamite in a way it is not in Rust: trait objects appear only behind a safe
reference, so every position where `dyn` could appear was a position where it
had to appear. The information it carried — that a fat reference is being
constructed — is now carried by the `as` conversion, at the point where the
construction actually happens, and `as` was already the language's universal
explicit-conversion operator for numeric, raw-pointer, and raw-to-reference
conversions.

Requiring the conversion also removes a contradiction. §6 states that
references introduce no subtype or variance conversions, and then granted
implicit concrete-to-trait-object coercion, which is exactly such a
conversion. The explicit form deletes the exception instead of documenting it.
The cost is accepted deliberately: trait objects are the only data-carrying
callback mechanism in a language without closures, so conversion sites are
common, and Elamite is now stricter here than either Go's implicit interface
satisfaction or Rust's implicit unsizing coercion.

`std` is gone as a keyword. `root`, `self`, and `super` remain keywords
because each names a position relative to the current module. `std` merely
names a package, and is now resolved by ordinary lookup — consulted after
lexical bindings, module declarations, imports, dependency aliases, and
prelude names, so a module that declares its own `std` shadows the
standard-library package. This matches the treatment of every other
compiler-known name (`Vec`, `Map`, `Set`, `String`, `Option`, `Result`), none
of which are keywords, and frees `std` for use as an ordinary identifier.

A bare trait name is now rejected in every value type position — field,
parameter, return, local annotation, type alias, and generic argument — along
with `*Trait`. `SPEC.md` §6 already declared bare trait-object values invalid,
but nothing enforced it, so an unsized type could reach lowering. Type
positions that legitimately name a trait (a safe-reference target, a generic or
impl bound, and the trait of an `impl Trait for Type`) are distinguished during
type lowering rather than by inspecting the resulting type, since a bound and a
reference target both produce the same nominal trait type.

Verifying that a conversion's source type implements the trait, and that the
trait is object-safe, remains Milestone 13 work; Milestone 6 checks the
reference shape and mutability of the conversion only.

## I-020: Multi-statement `defer` blocks

**Resolved.** `defer` gains a second form. `defer call` still defers one safe
unit-returning call; `defer:` defers an indented block of statements as a
single registration. This reverses the earlier direction, which explicitly
excluded a multiline `defer:` body, and `AGENTS.md` is updated accordingly.

The motivation is that cleanup frequently needs more than one call, and the
single-call form forced either a wrapper function per cleanup site or several
`defer` statements whose reverse-registration order is easy to misread. A
block form makes the grouping explicit and registers once.

The restrictions follow from when deferred code runs. Because the block
executes while its enclosing scope is already exiting, it cannot change where
control goes: `return`, `break`, `continue`, and postfix `?` are invalid
inside it, as is a nested `defer`. The prohibition on `?` extends the rule the
single-call form already had.

Two further restrictions keep unsafe code and deferred cleanup separate: a
`defer` statement is invalid inside an `unsafe` block, so unsafe scopes stay
straight-line, and an `unsafe` block is invalid inside a `defer:` block, so the
block form is not a way around the existing rule that a direct unsafe or
foreign call cannot be deferred.

A `defer:` body is an ordinary lexical scope, so a binding declared inside it
is local to the deferred block. Deferred *execution* remains Milestone 15;
Milestone 3 parses both forms and Milestone 7 enforces the placement rules.

## I-021: Remove the compiler-known `Close` trait

**Resolved.** Deterministic cleanup uses `defer` alone. The compiler-known
`Close` trait is removed because it neither caused cleanup nor let the compiler
prove that cleanup was registered. `defer` already accepts any safe
unit-returning call and does not need a distinguished method or trait.

The former trait also attached idempotence, shared-resource state, and
closed-handle behavior to an implementation that could not enforce any of
those laws. Resource types now expose ordinary methods such as `close` or
`release`; the concrete type's API specifies whether those methods are
idempotent, fallible, or shared across copied handles. Shared identity must be
represented explicitly, such as through a safe reference in the handle, rather
than being introduced by implementing a trait.

Libraries remain free to declare an ordinary `Close` trait when generic cleanup
polymorphism is useful. Such a trait has no compiler integration and no special
relationship with `defer`.

## Issues to refine after initial implementation begins

These questions remain visible in the language design but do not block the
initial parser and type checker.

## I-015: Concurrency and asynchronous execution

The interaction among independent value copying, explicit references, mutable
aliases, function references, trait-object callbacks, garbage collection,
resource cleanup, and tasks is unaddressed. The language needs a model for data
races, synchronization, ownership or sharing across tasks, cancellation, and
task-local resource cleanup before concurrency is added.
