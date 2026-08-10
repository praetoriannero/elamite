# Compiler architecture

Milestone 20 established explicit, owned boundaries without changing Elamite
language behavior or the public compiler commands.

The descriptions below reflect the executable compiler targeting Specification
0.10 plus the temporary 0.11 frontend seam.
Ordinary shallow-copy lowering and collection representations are implemented;
direct collection state snapshots its shallow iterable and bound once, while a
user-defined iterator is selected statically and promoted into managed hidden
state for repeated `next` calls. Threads, channels, and mutex operations use
shallow values. Pointer arithmetic and indexing are implemented, and pointer
ordering lowers null cases through explicit equality guards before any C
relational operator. POSIX start,
channel, mutex, join, and atomic operations own the concurrency ordering
boundaries. Current
reuse and borrowing facts remain documented here until their implementation is
replaced.

## Pipeline and ownership

| Boundary | Owner |
| --- | --- |
| Tokens and token-preserving syntax | `src/syntax.rs` |
| Hand-written lexing and parsing | `src/lexer.rs`, `src/parser.rs` |
| Loaded and parsed package units | `src/parsed.rs` |
| Expansion between parsing and resolution | `src/expansion.rs` |
| Stable names, IDs, and resolution tables | `src/resolution/model.rs` |
| Collection, imports, bodies, visibility | matching modules in `src/resolution/` |
| Canonical types and interning | `src/types/context.rs` |
| Canonical ownership capabilities | `src/types/ownership.rs` |
| Typed-program facts | `src/types/model.rs` |
| Source-type lowering | `src/types/lower.rs` |
| Checked output and pure checker analyses | `src/check/model.rs`, `coverage.rs`, `containment.rs` |
| Body-checking orchestration | `src/check/mod.rs` |
| Typed IR and lowering | `src/ir/typed/model.rs`, `lower.rs` |
| Internal read-only call analysis and ABI selection | `src/ir/borrowing.rs` |
| Temporary and return storage-reuse selection | `src/ir/reuse.rs` |
| Control-flow IR and lowering | `src/ir/control_flow/model.rs`, `lower.rs` |
| Shared ownership/use operations, logical-copy facts, and traps | `src/operations.rs`, `src/ir/traps.rs` |
| Target and optimization policy | `src/config.rs` |
| C naming, types, runtime, functions, entry | matching modules in `src/backend/` |

The lexer, parser, resolution, types, checker, IR, and backend façades retain
their established public paths. Compatibility re-exports for `Target` from
`backend` and `Optimization` from `driver` are retained because integration
users already import them there; new compiler code should use `config`
directly.

`src/config.rs::SemanticRevision` is chosen once while the package graph is
constructed. Every package on a dependency edge must carry the same revision;
parsed, expanded, resolved, and typed results retain it explicitly. The 0.11
path admits the accepted slice, array, closure-capture, and borrow syntax,
canonicalizes its source types, validates trait declarations, and constructs
ownership-aware checked facts. It then stops at
`check::semantic_revision_boundary` before move-state enforcement. Focused
tests lower those checked facts through both IR levels, but compiling driver
entry points do not yet send an owned-model value to the backend. Consequently
the backend has no revision switch and cannot infer a semantic model from
syntax.

`src/types/ownership.rs` computes `Copy`, clone availability, destruction,
borrow containment, and provisional `Send`/`Sync` from canonical types and
declared traits. Recovery types grant nothing, generic facts remain explicit,
and recursive queries use monotone seeds rather than layout recursion.
`OwnershipPlace` gives later dataflow a stable root plus field, tuple, index,
dereference, and receiver projections with a conservative overlap query.
Checking assigns an ordered operation sequence to every expression because one
expression may both form a borrow and copy or move the resulting reference.
Typed IR substitutes concrete facts and attaches projected places;
control-flow IR assigns stable per-function operation IDs after source
evaluation. Move, `Copy`, clone, borrow, reborrow, and future drop operations
therefore remain distinct without backend inference.

Logical-copy intent is selected before backend lowering. Checking records the
copy kind and source and destination lifetime classes; typed IR adds the
type-selected allocation class; control-flow IR assigns a stable per-function
copy identity. Debug builds audit that inventory for exact-once emission. The C
backend lowers every copy, including thread, channel, and mutex values, as an
immediate representation assignment. There is no recursive per-type copy-helper
family.

Logical copies are now explicitly the compatibility operation
`OwnershipUseKind::LegacyCopy`. Only that variant enters the 0.10 copy
materialization path. Owned moves and bitwise copies lower as explicit
ownership uses whose C representation is an ordinary temporary transfer;
neither operation can select recursive cloning or shallow managed sharing.

Typed IR distinguishes numeric arithmetic from raw-pointer offset and distance
operations. Pointer indexing adapts to an element-scaled offset followed by a
raw dereference; control-flow lowering materializes the pointer and signed
index once in left-to-right order and emits the existing null/alignment check
before the load or store. The backend therefore emits ordinary C99 pointer
operations without introducing a parallel indexing or trap mechanism.

After concrete typed functions and vtables exist, `src/ir/borrowing.rs`
conservatively identifies costly internal direct-call parameters whose storage
cannot be mutated or address-exposed. It records an explicit passing mode on
parameters and arguments before control-flow lowering. The backend implements
that mode with a hidden `const` pointer ABI; indirect, closure, vtable, and
foreign-visible callables retain the ordinary owned ABI.

`src/ir/reuse.rs` retains each remaining semantic copy record while selecting
an explicit `ReuseSource` physical mode for fresh temporaries and conservatively
dead local returns. Ordinary `Materialize` and `ReuseSource` copies now both
preserve the shallow representation; the distinction remains an optimizer seam
for future inline movement.

The typed and control-flow IR classify ordinary `String` copies as
shared-backing operations. Ordinary copies directly preserve the two-word
descriptor, whose pointer names writable managed bytes; mutable access does not
detach. Thread environments, channel messages, and join results preserve that
pointer directly, as do mutex storage and returned values. `Vec` uses an inline
pointer/length/capacity descriptor and mutating calls carry the evaluated
receiver place through control-flow IR so only that descriptor's length,
capacity, and growth pointer change. `Map` and `Set` retain shared table
handles. None of these representations is exposed through the foreign ABI.

The expansion pass owns a lossless nested token-tree view containing exact
source spellings, layout tokens, and stable origin identities. Parsed units are
consumed into expansion-owned identities, user-defined compile-time code runs,
generated syntax re-enters the fixed-point expansion queue, and resolution
accepts only the completed expansion result. The append-only provenance table
distinguishes physical spans from spanless generated output and records
definition, invocation, and nested-expansion chains. Strict expression,
statement, pattern, type, member, and item fragment entry points reuse the
ordinary parser through `src/expansion/fragment.rs`, including full-consumption
checks and existing recovery.

The compile-time layer exposes an opaque, versioned, pre-resolution `std.ast`
façade to a bounded interpreter for ordinary safe compile-time Elamite code.
`macro`, `attr`, and `derive` share that runtime and the same provenance model;
`quote:` plus `$` interpolation creates syntax with definition-site hygiene,
while interpolated values retain their context. The interpreter and AST façade
must remain outside compiler-private parsed, resolved, and typed data models and
must not gain ambient host or target capabilities.

`src/expansion/ast.rs` owns that façade. Expanded packages carry an exact
`std.ast` 2.0 interface handshake for 0.10 or the selected 3.0 owned-surface
handshake for 0.11, plus a stable intrinsic
type inventory. Its opaque, immutable values cover definitions, items,
expressions, statements, patterns, written types, metadata, fields, variants,
parameters, and implementations; persistent typed lists and `with_` methods
return logical copies without mutating their inputs. Validating constructors
receive only expansion-minted origin handles. Contained `std.ast.error` failures
therefore resolve physical origins directly and retain generated invocation and
definition locations as related context without inventing a physical span.
These values share no `SyntaxNode`, resolver identity, inferred type, target
layout, runtime value, or mutable compiler table. Quotation follows the same
separation while preparing source structure for later interpreter lowering.

The quote syntax boundary now lives in `src/expansion/quote.rs`. The lexer emits
`$` as ordinary punctuation, while the hand-written parser records a
role-neutral `QuoteExpression`/`QuoteBody` and replaces only `$name` and
`$(expression)` sites with interpolation nodes. Expansion recognizes explicit
binding and compile-time return annotations for every admitted `std.ast` role,
classifies sites as scalar insertion or collection splicing, and adapts the
physical body back through the same expression, pattern, type, statement,
member, and item grammars used by handwritten source. Definition-specific
roles additionally require the matching struct, enum, function, field, or
implementation node. Parameter-driven roles remain for compile-time signature
checking. The interpreter constructs façade values and assigns hygiene through
the execution context rather than through the parser.

`src/expansion/namespace.rs` collects physical macro, attribute, and derive
declarations and their explicit imports into stable, separate module namespaces
before ordinary resolution. It resolves aliases, public re-exports, package
privacy, inline modules, and dependency roots without adding those bindings to
the ordinary value/type namespace.
`src/expansion/scheduler.rs` is the deterministic fixed-point engine driven by
the bounded interpreter. It orders ready work by package, module, and
provenance; represents attributes-before-derives and generated-output
dependencies explicitly; re-enters generated invocations and definitions; and
turns structurally repeated active-chain requests into stable recovery nodes.
It also owns the normative graph-wide depth, execution, and generated-node
budgets plus the per-execution interpreter-step and live-value meters.
Generated-node charging is atomic, while per-execution exhaustion is sticky,
so failed work cannot leak partial syntax even through an incorrect driver.
Macro bodies execute through `src/expansion/interpreter.rs`, while
`src/expansion/engine.rs` connects typed arguments and returned syntax to the
scheduler and ordinary semantic pipeline. User-defined forms are stable; the
former experimental gate and CLI option were removed after conformance.
The completed compile-time system is guarded by directed integration tests and property
tests over arbitrary token streams, every fragment role, deep generated
provenance, randomized expansion depth, atomic limit recovery, and explicit
macro-free equivalence for every shipped package example. These tests preserve
the phase boundary while generated syntax flows into ordinary semantic passes.
Native-language test discovery and execution lives in `src/testing.rs` and
remains separate from the conformance fixture runner.

## Behavior-neutral baseline

Before structural edits, `examples/spec_demo.elx` produced these SHA-256
digests:

| Artifact | SHA-256 |
| --- | --- |
| tokens | `137620c0ed56528b5feaa0d727a41010f078aacbd1cad8e682371460d3552bd8` |
| syntax | `9b2e37d5ee01fb3542483bbfd27e8b28a3fc283c895d154eb148b8c87b8b5b0f` |
| resolution | `41d851cf30e179fc314a8ae493bca17f8e9def8013996e08cd9d9db6fac0e681` |
| types | `fb690e8edc5c058667beb0fef0e3049ddb3c885866980c31035979fc32b2c5e4` |
| typed IR | `d82585bd4efbdeab5426c6360535530be78b2f5048ef500eab5828a01ddcbd9c` |
| control flow | `34705be3f70bda830cfc289f840b6532c871eeb83901d46ccb83c4ed2562540a` |
| monomorphized program | `e8aa64cce32a48a748ad79ab7e380d8a682b0d7b08f6af71fc52e4f5e0ba40a2` |
| generated C | `a8d738b059d929f5f7f0a1d96470a6748a5d921639c03a64ab9ad8976cd7c660` |

Diagnostic categories, spans, and ordering remain guarded by the compile-fail
tests. Public library entry points are exercised by integration tests. Native
runtime behavior, target widths, and debug/release modes are guarded by the
complete conformance matrix. Debug C compilation remains `-O0`; release
compilation remains `-O3`.
