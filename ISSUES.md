# Elamite Design Issues

> This document records open design questions for Elamite's managed,
> copy-by-default direction. It deliberately contains no proposed solutions or
> decisions.

## Issues to resolve before initial implementation begins

These issues determine the initial parser, type checker, and core runtime
model.

## I-005: Function, method, and closure values

The function type `fn(Args) -> Return` covers named functions, unbound methods,
and closures. Closures shallow-capture their free local bindings at creation;
captured `var` bindings are closure-owned mutable copies, and captured
references retain their targets. Copies of a capturing closure share one
managed closure environment. Function equality compares generated-function and
closure-environment identity. Named functions and contiguous local closure
bindings support direct and mutual recursion. Dynamically stored callables
remain open.

## I-006: Traits and method resolution

Traits, inherent struct methods, trait imports, and explicit trait-qualified
calls require one coherent lookup model. The unresolved details include default
methods, same-name collisions, visibility of trait members, associated types or
constants, static versus dynamic dispatch, and whether trait values exist.

The package-level coherence and overlap rules for generic trait implementations
also remain open.

## I-007: Generics and type inference

Generic declarations use square brackets, but the language must define generic
inference, bounds, variance of references and containers, generic recursion,
and runtime representation. It must also decide whether compilation uses
specialization, erased managed representations, or a mixture, especially for a
C backend.

## I-008: Equality, hashing, and ordering

The language must define equality for ordinary values, references, raw pointers,
and cyclic data. Hashing, ordering, user-defined comparison hooks, and the
comparison behavior of library collections remain open.

## I-012: Layout-sensitive grammar

The language uses `:` and indentation for declaration and control-flow bodies.
It still needs exact indentation, tab, blank-line, comment, multiline
continuation, empty-body, dedent, and error-recovery rules. The grammar must
also distinguish a body colon from type annotations, generic bounds, record
literals and patterns, and multiline `match` arms or closures.

## I-013: Core types, literals, and expressions

The distinction between `str` and `String`, numeric conversion and overflow,
tuples, record and collection literals, pattern grammar, operator coverage,
casts, and the unit/return conventions remain incomplete. The status of
`Default`, constructors named `new`, and any built-in traits also needs a single
definition.

## I-017: Specification migration and consistency

`SPEC.md` and the specification examples now define the managed,
copy-by-default direction. The existing lexer/parser and any older fixtures
still encode earlier grammar and semantic assumptions. Parser work must wait
until the unresolved design issues have been reviewed and the specification is
considered stable.

## I-018: Mixed body and return syntax

The authoritative demo uses indentation-and-colon bodies almost everywhere, but
`Point.new` uses a brace-delimited body. The grammar must decide whether brace
bodies are an alternative, restricted to specific contexts, or errors.

The demo consistently writes `-> Return` types but uses both explicit `return`
and tail expressions. The exact interaction between an omitted return type,
tail expressions, and `return` needs a single rule.

## I-019: Derivation and generic declaration syntax

Generic aliases and types use square brackets, for example `NameMap[V]` and
`Chain[T]`, while `Point(Default)` uses parentheses for trait derivation. The
language must define whether parentheses are reserved for derives, whether
multiple derives are allowed, and how derives compose with generic parameters,
visibility, trait bounds, and user-defined traits.

## I-022: Receiver adaptation and callable syntax

`Session.stop` declares a mutable-reference receiver, but the demo invokes the
unbound method as `fn_stop(session)` without an explicit `&var session`.
`Toggle.status` likewise declares `&Self` but is selected as
`Session.Toggle.status(observer)`. The language must define receiver
auto-reference rules and the type of an unbound or trait-selected method value.

## I-023: Strings, literals, and mutable fields

The demo calls `str` immutable, `String` a mutable UTF-8 vector, uses string
literals for both, and declares `Packet.body: str` before replacing that field.
It does not establish whether literals are `str` or `String`, whether values of
either type copy deeply, or whether field replacement differs from mutation of
a string's contents.

## I-017.2: Allocation-expression consistency

The updated demo uses `new Session.new("build")` and says that this creates a
heap-allocated pointer cleaned up by GC. The specification currently describes
`new` as an ordinary associated method name. The grammar, result type, lifetime,
and relationship between the allocation expression and a type's `new` method
need a single definition.

## Issues to refine after initial implementation begins

These issues remain visible in the design, but do not block the beginning of
the initial implementation.

## I-009: Collections and iteration

`Vec`, `Map`, `Set`, indexing, iteration, and literals need a concrete API and
copy/reference contract. Open questions include whether reading an element
copies it, iterator lifetime and allocation behavior, and iteration over cyclic
or shared structures.

## I-010: Errors, cleanup, and resource values

`Result`, `?`, and `defer` need rules that do not rely on move semantics. The
design must define error propagation across nested calls, defer ordering and
failure behavior, resource-handle copying, and whether copying a resource
duplicates a handle, shares it, or is invalid. Garbage collection must remain
separate from deterministic external-resource cleanup.

## I-011: Modules, packages, and visibility

Module files, package roots, import resolution, circular imports, re-exports,
and package identity remain unspecified. These rules determine `pub` behavior,
trait coherence, qualified symbol lookup, and public API type checking.

## I-014: Garbage collector and runtime contract

The language uses Boehm GC but needs an observable-runtime contract for
allocation, reachability, cycles, weak references, finalization policy,
out-of-memory behavior, stack roots, and debugging support. It must specify
what managed and raw references guarantee when values are copied.

## I-015: Concurrency and asynchronous execution

The interaction among copying, explicit references, mutable aliases, closures,
garbage collection, `defer`, and tasks is unaddressed. The language needs a
model for data races, synchronization, ownership or sharing across tasks,
cancellation, and task-local resource cleanup before concurrency is added.

## I-016: Foreign-function interface and unsafe code

A C-oriented implementation needs a boundary for scalar and aggregate
marshalling, references retained by foreign code, callbacks, native resource
handles, object movement, pinning, and foreign exceptions. The initial safe
surface and any later `unsafe`/raw-pointer model must be designed together.

## I-020: Null pointers and unsafe references

The demo initializes `ptr` to `null`, tests it for truthiness, and separately
returns `&x` from a local binding in an `unsafe` function named `maybe_null`.
Reference lifetime and nullability are separate questions.

The specification must decide pointer-truthiness rules, what an unsafe block
promises, whether managed references can ever be null, and when an unsafe
conversion or returned reference is checked.

## I-021: Default initialization and optionals

The demo treats `Default` as a derived trait that supplies `Self.default()` and
uses `new` as a user-defined convenience method. It says an `&T` field prevents
deriving `Default`, while `*T` defaults to `null` and `Option[T]` is preferred.

Default values for primitives, structs, enums, generic fields, collections,
references, and user-defined traits need definition. The language also needs a
rule for whether `new` has any convention beyond an ordinary method name.
