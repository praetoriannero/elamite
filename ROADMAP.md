# Elamite Compiler Roadmap

> Status: Active — completed legacy work is omitted from this forward-looking
> plan; remaining milestones and work packages use stable descriptive names
>
> Next required work package: **Compile-time AST and interpreter** —
> **Concatenation operator**
>
> Basis: `SPEC.md` version 0.9.0-draft and
> `examples/spec_demo.elx`

This document breaks the remaining initial Elamite compiler work and planned
post-conformance extensions into implementation milestones. Each milestone
carries its own status note; the header above records how far the sequence has
advanced. Elamite compiles to C, as required by the specification.

Detailed plans are removed from this file once every work package in a
milestone is complete. Normative behavior remains in `SPEC.md`, rule coverage
and implementation status remain in `LEDGER.md`, and implementation evidence
remains in tests and version history.

The plan was originally written without choosing the language in which the
compiler is written, a parser technology, or a build system. Those are now
settled: the compiler is an edition-2024 Rust package built with Cargo, and its
lexer and parser are hand-written. See `AGENTS.md` for the rules those choices
imply and `LEDGER.md` §18 for the third-party crate decisions behind them.

`SPEC.md` and the authoritative demonstration define the language. This
document defines an implementation order, not new language semantics. When it
conflicts with the specification, the specification wins and this plan must be
updated.

Concurrency is not part of the initial implementation plan. The
**Standard-library concurrency** milestone records the accepted post-closure
native-thread and synchronization design; its normative specification is
blocked by **Explicit-capture closures**.

## 1. Implementation strategy

The compiler should be built as a sequence of complete, testable layers:

```text
manifest and source files
    -> tokens
    -> parsed package syntax
    -> expanded package syntax (lossless token trees; rewriting still disabled)
    -> resolved declarations
    -> typed high-level IR
    -> explicit control-flow IR
    -> monomorphized program
    -> generated C and runtime support
    -> C compiler and linker
```

Each representation should have one clear responsibility:

- Tokens preserve source spans and indentation events.
- Parsed package syntax owns the token-preserving trees for every user and
  shipped standard-library module without performing semantic checks.
- Expanded package syntax owns the lossless token-tree view and forwards the
  parsed syntax unchanged until macro rewriting begins; it remains the sole
  input to ordinary name resolution.
- Resolution assigns stable identities to packages, modules, declarations,
  fields, variants, traits, and lexical bindings.
- Typed high-level IR records types, selected declarations, implicit receiver
  adaptations, value-versus-place classification, and required copies.
- Control-flow IR makes evaluation order, branches, traps, returns, propagated
  errors, and deferred cleanup explicit.
- Monomorphization replaces valid generic instantiations with concrete
  functions, types, and trait implementations.
- The C backend selects representations and emits strictly sequenced C.

The initial implementation should favor correctness and inspectability over
optimization. In particular, deep copying and conservative heap promotion are
acceptable first implementations. Copy-on-write values, precise escape
analysis, incremental compilation, and aggressive C output optimization should
come only after the relevant semantics have conformance tests.

## 2. Cross-cutting engineering rules

These rules apply to every milestone.

### 2.1 Stable identities and source locations

Assign internal IDs rather than using source names as identity. At minimum,
provide IDs for package instances, modules, declarations, generic parameters,
impls, fields, variants, and local bindings. A nominal type identity must
include its package instance, not merely the displayed package name.

Every token and syntax node should carry a source span. Later representations
should retain an originating span for diagnostics even after desugaring or
monomorphization. Generated C should contain enough source mapping information
to relate a C compiler or linker failure back to the generated unit and, where
possible, the Elamite declaration.

### 2.2 Deterministic behavior

Given identical source, dependency identities, compiler options, and target,
the compiler should produce the same diagnostics, symbol names, generated C,
and link inputs. Sort filesystem discoveries and unordered internal maps before
they affect observable output.

The unspecified iteration order of an Elamite `Map` or `Set` is a runtime
language property and does not justify nondeterministic compiler output.

### 2.3 Diagnostics

Diagnostics should have a stable category or code, a primary source span, a
plain-language explanation, and related spans when another declaration caused
the error. Semantic analysis should continue after locally recoverable errors
by using explicit error symbols and error types rather than cascading guesses.

Required compile-time errors belong in conformance tests. Optional warnings,
such as broader raw-pointer analysis or locally provable resource leaks, must
never be required for a program to compile.

### 2.4 Testing layers

Maintain four main kinds of tests:

- Parse tests compare source with token or syntax-tree snapshots.
- Compile-pass and compile-fail tests exercise name resolution and static
  semantics. Compile-fail tests assert the diagnostic category and important
  spans without depending on incidental wording.
- Run-pass tests compile through C, execute the native result, and compare
  output and exit status.
- Integration tests build multiple packages or link a small C harness to test
  package identity, visibility, ABI behavior, callbacks, and native libraries.

Add focused property or fuzz tests for indentation, literal parsing, parser
recovery, numeric boundaries, match exhaustiveness, generic instantiation, and
copy independence once those subsystems exist.

Each language feature should normally have at least:

1. One minimal successful example.
2. One interaction example with an already implemented feature.
3. One failure for every important static restriction.
4. One runtime boundary test when the feature can trap.

### 2.5 Definition of done

A milestone is complete only when:

- its accepted syntax and semantics are represented in the appropriate IR;
- successful and failing behavior has automated coverage;
- all earlier milestone tests still pass;
- generated artifacts and diagnostics are deterministic;
- temporary limitations are recorded explicitly rather than silently accepted;
- the next milestone does not need to bypass the established layer boundary.

### 2.6 Work-package format

Only milestones with outstanding or candidate work remain below. Within an
active milestone, completed work packages stay marked until the whole milestone
is complete. Outstanding work is divided into ordered, descriptively named
packages.

Each work package should normally fit in one focused change and must:

- leave the compiler building with all earlier tests passing;
- add the smallest representation, semantic rule, or runtime hook needed by
  that package, without partially enabling later syntax;
- include focused positive and negative tests, plus a runtime test when it can
  trap or has observable ordering;
- update `LEDGER.md` when completing it changes the recorded implementation or
  test status of a normative rule; and
- record any deliberately temporary boundary in the milestone status note.

Task order within a milestone is significant unless a task explicitly says it
may proceed in parallel. A work package is not a new language-design authority:
`SPEC.md` remains normative, and splitting work must not create observable
intermediate semantics that contradict it.

## 3. Active implementation roadmap

### Post-conformance optimization

> Status: Candidate work; select a measured problem before implementation.
>
> Blocked by: **None**.

**Goal:** Improve implementation quality without changing language behavior.

Candidate work packages are independent proposals, not a promise to implement
all of them in table order:

| Task | Candidate change | Required semantic guard |
| --- | --- | --- |
| **Optimization benchmark gate** | Choose a measured problem, record a reproducible before-state, and define the expected improvement before changing lowering. | The complete compiler-architecture refactor baseline remains available for comparison. |
| **Precise escape analysis** | Keep proven nonescaping address-taken storage on the stack while retaining conservative promotion as the fallback. | Escaping, interior, returned, trait-object, and deferred references preserve root lifetime and identity. |
| **Copy elision** | Elide backend copies or reuse storage when independence cannot become observable. This is not a source-level move operation. | Mutation, alias, evaluation-order, return, pattern, and cleanup tests remain unchanged. |
| **Copy-on-write text** | Replace eager `String` copies with COW storage behind the existing logical-copy interface. | First mutation detaches exactly once; independent copies, explicit aliases, and evaluation order retain specified behavior. |
| **Copy-on-write collections** | Add COW independently for `Vec`, `Map`, and `Set`, one collection at a time. | Mutation, indexing places, iteration snapshots, hashes, and returned removed values remain independent. |
| **Representation specialization** | Specialize concrete generic layouts/helpers where measurement justifies it. | Canonical type identity, ABI exclusion, deterministic symbols, and copy semantics do not change. |
| **Devirtualization** | Replace a trait-object call with static dispatch only when the concrete target is proven and evaluation is unchanged. | Vtable-visible behavior, target identity, object safety, and source-order evaluation remain intact. |
| **Incremental queries** | Introduce stable query boundaries for parsing, resolution, typing, and lowering. | Cache keys include all semantic inputs and clean/incremental outputs are byte-identical. |
| **Dependency artifact cache** | Reuse built package artifacts by deterministic package identity, target, options, compiler/spec revision, and dependency keys. | Stale artifacts cannot cross package instances or target widths. |
| **Parallel package compilation** | Schedule independent package nodes concurrently after deterministic dependency resolution. | Diagnostics, symbols, artifacts, and link-input ordering remain deterministic. |
| **Source maps and editor diagnostics** | Improve generated-source mapping and expose machine-readable diagnostics suitable for editor tooling. | CLI diagnostics retain stable categories/spans and no language-server dependency enters semantic layers. |
| **Optional analyses** | Add warnings for locally provable resource leaks and suspicious raw-pointer contracts. | Warnings are conservative, independently suppressible, and never become required compile errors. |

Every selected package requires before-and-after measurements, a full
conformance run, and targeted tests that would expose a change in copy
independence, evaluation order, identity, root lifetime, trap behavior, or
deterministic output.

**Post-conformance optimization** is optional work rather than a gate for the
test and macro programs below. Individual packages, especially source-map
infrastructure, may be pulled forward when a later milestone needs them.

## 5. Post-conformance compile-time syntax generation

Macro work begins after the completed initial-conformance milestone.
`SPEC.md` §12 owns the accepted interpreter-backed `macro`, `attr`, and
`derive` declarations, the versioned `std.ast` interface, `quote:` and `$`
interpolation, `++` concatenation, hygiene, scheduling, and deterministic
resource limits. Until implementation reaches each gated package, the existing
compiler-supported collection macros, FFI attributes, and compact built-in
derive syntax retain their behavior.

The implementation must preserve these boundaries throughout the transition:

- macro-free packages keep the same semantics, diagnostics, and deterministic
  C output;
- expansion is a distinct phase rather than behavior embedded in parsing,
  resolution, or type checking;
- expanded declarations pass through ordinary resolution, checking, coherence,
  lowering, and backend rules;
- expansion provenance is retained so diagnostics identify both generated code
  and the source invocation that caused it;
- compile-time execution has no ambient capabilities and cannot observe target
  state or mutable compiler internals; and
- resource limits and expansion ordering are deterministic and testable.

### Macro expansion foundations

> Status: Complete against the revised compile-time design in `SPEC.md`
> 0.9.0-draft.
>
> Blocked by: **None**. It is independent of **Package tests, typed traps, and
> runner**.

**Goal:** Add behavior-neutral representations and pipeline boundaries on which
user-defined macros, attributes, and derives can be implemented without
destabilizing the existing compiler.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Language-contract revision (done)** | Replace token matcher/transcriber rules with interpreter-backed `derive`, `attr`, and `macro` declarations over `std.ast`; settle `@` invocation and attachment forms, `quote:`, `$` interpolation, `++`, namespaces, execution order, hygiene, capabilities, and deterministic limits in `SPEC.md`. | Every implemented compile-time behavior is owned by a normative rule rather than this roadmap. |
| **Token-tree representation (done)** | Represent nested delimiters, indentation tokens, source text, and spans without prematurely parsing tokens as Elamite syntax. | Existing lexing behavior remains unchanged for macro-free source. |
| **Expansion identities and provenance (done)** | Introduce stable expansion identities and origin chains that distinguish physical source, invocation, definition, and generated spans without inventing physical file offsets. | Nested generated nodes can be traced deterministically to their invocation and definition. |
| **Fragment parser entry points (done)** | Parse complete expression, statement, pattern, type, and item fragments from token trees with full-consumption checks and ordinary parser recovery. | Each fragment role has positive, trailing-token, and malformed-input tests. |
| **Expansion pipeline boundary (done)** | Replace the compiler-architecture refactor's pass-through seam with expansion-owned unit identities, lossless token trees, provenance, and an owned package result consumed before name resolution. | Macro-free packages preserve their source inputs and the explicit parse → expand → resolve path is equivalent to the normal resolver entry point; the complete downstream suite preserves diagnostics, typed IR, runtime behavior, and generated C. |
| **Compile-time identities and namespace collection (done)** | Parse the minimal physical declaration/import surface and add stable macro, attribute, and derive declaration/import/module identities plus package, module, visibility, alias, re-export, and separate-namespace collection before ordinary resolution. Signature semantics and execution remain later packages. | Same-name declarations across ordinary/macro/attribute/derive namespaces, renamed nested-module imports, duplicates, private bindings, public re-exports, and cross-package declarations resolve or diagnose predictably. |
| **Deterministic expansion scheduler (done)** | Implement the structural fixed-point queue and dependency graph, including attribute-before-derive ordering, outermost-first function macros, generated-item re-entry, cycle diagnostics, and stable recovery nodes. The scheduler is execution-independent until the compile-time interpreter supplies output. | Repeated builds schedule and diagnose the same expansions in the same order. |
| **Resource accounting seam (done)** | Give the scheduler shared depth, execution, generated-node, interpreter-fuel, and live-value budgets before execution exists. Generated output is admitted atomically, and per-execution exhaustion remains sticky even when a driver ignores the immediate charge result. | Limit charging is stable and cannot be bypassed by nested or generated work. |
| **Experimental gate (done)** | Keep incomplete user-defined macro behavior behind the explicit `--unstable-macros` compiler gate across package and single-file check/build/run, semantic dumps, documentation, package tests, and conformance. Token/syntax dumps and formatting remain syntax-aware tooling; compiler macros, FFI attributes, and compact built-in derives remain ungated. | Stable invocations cannot accidentally depend on unfinished behavior, and library entry points default to the stable feature set. |
| **Foundation validation (done)** | Add directed and property coverage for token-tree losslessness, every fragment parser role, generated provenance chains, deterministic scheduler staging, resource limits, malformed input, the experimental gate, and macro-free equivalence across all shipped package examples. | Arbitrary lexer output recovers without panics or fabricated spans, generated chains terminate at their configured bounds, and the complete compiler suite preserves macro-free behavior. |

### Compile-time AST and interpreter

> Status: In progress — the versioned `std.ast` façade and typed quotation
> syntax are complete.
>
> Blocked by: **None** — **Macro expansion foundations** is complete.

**Goal:** Implement the target-independent `std.ast` façade, quotation, and a
bounded interpreter for ordinary safe Elamite compile-time code.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Versioned `std.ast` façade (done)** | Define opaque immutable structural syntax values, stable accessors and `with_` transforms, validating builders, persistent AST lists, origin handles, pattern variants, and contained `std.ast.error` failures without exposing compiler-owned nodes or tables. The exact `1.0` handshake and sorted intrinsic type inventory are carried by every expanded package; only expansion can mint origins, and generated failures retain invocation/definition context without fabricated spans. | Directed and property tests cover every admitted value family and variant, every published transform, invalid identifiers and paths, exact version skew, arbitrary persistent-list concatenation, physical diagnostics, and generated diagnostic context. |
| **Quote and interpolation syntax (done)** | Lex and parse role-neutral, indentation-delimited `quote:` templates and `$name`/`$(expression)` sites without parsing quoted source prematurely. Infer explicit binding and compile-time return roles for every admitted `std.ast` scalar, list, item, and definition type; distinguish scalar insertion from collection splicing; validate adapted bodies through the ordinary hand-written grammar; preserve physical spans; reject runtime quotation; and retain parameter-driven inference for compile-time signature checking. Hygiene context assignment and conversion to actual façade values remain interpreter-lowering work. | Lexer, parser, formatter, editor, expansion, directed role/malformed/wrong-role/nesting/indentation tests, and property-generated named/computed interpolation streams cover the complete syntax boundary. |
| **Concatenation operator** | Add binary `++` at additive precedence for strings, supported sequences, and AST lists while keeping numeric `+` separate and rejecting arbitrary AST-expression concatenation. | Lexer, parser, checker, runtime, formatter, and editor tests agree on the new operator. |
| **Compile-time checking and lowering** | Check compile-time signatures and bodies through the ordinary language front end, reject runtime-only and unsafe capabilities, and lower the admitted subset to a versioned interpreter representation. | Invalid signatures and operations fail before execution with ordinary source spans. |
| **Bounded interpreter** | Execute safe Elamite control flow, values, pattern matching, functions, and `std.ast` intrinsics deterministically with explicit fuel and live-value accounting. | Repeated execution is identical; recursion, loops, allocation, panics, and invalid intrinsics are contained diagnostics. |
| **Capability and host/target boundary** | Deny FFI and ambient filesystem, environment, process, network, clock, randomness, target, runtime, and compiler-internal access; keep compile-time execution independent of the selected output target. | Capability probes fail predictably and x86/x86-64 builds expand identically. |
| **Artifact and dependency identity** | Serialize or rebuild public compile-time bodies and `std.ast` ABI metadata with identities keyed by source, transitive compile-time dependencies, and compiler/spec/interface versions. | Clean, cached, local, and cross-package execution produce equivalent results and reject version skew. |
| **Interpreter validation** | Add unit, property, fuzz, adversarial, reproducibility, limit, recovery, host/target, version-skew, and cross-package suites. | The interpreter cannot hang, crash, escape its capability boundary, or mutate compiler state. |

### Interpreter-backed macros, attributes, and derives

> Status: Pending.
>
> Blocked by: **Compile-time AST and interpreter**.

**Goal:** Expose all three accepted declaration forms on the shared `std.ast`
and interpreter foundation with deterministic staging and ordinary semantic
validation.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Declaration syntax and lookup** | Parse `[pub] macro`, `[pub] attr`, and `[pub] derive` signatures and bodies, including one final homogeneous variadic AST parameter where §12 permits it; collect physical declarations/imports in separate namespaces with stable identities, visibility, aliases, and re-exports. | Local, duplicate, renamed, private, malformed, variadic-placement, and cross-package cases resolve or diagnose predictably. |
| **Function-like macros** | Parse `@path(...)` in expression, pattern, type, whole-statement, and module-item roles; construct fixed and variadic typed AST arguments, execute the declaration, and validate the declared return role. | Each role has successful, zero/many variadic, empty, wrong-role, trailing, nested, and recovery coverage. |
| **Structural attributes** | Attach `@attr(path)`/`@attr(path(...))` to accepted definitions, supply the typed target implicitly, pack any final variadic explicit AST arguments, run top-to-bottom, and admit same-kind replacement or validated `ItemList` output. | Field/method addition, fixed/variadic arguments, replacement, removal, sibling emission, bad target, and interacting attributes behave deterministically. |
| **Trait derives** | Run `@derive(...)` after attributes, validate the exact trait and target identity of each returned `Implementation`, and retain the original definition. | Struct/enum, generic, duplicate, bad-output, orphan, overlap, bound, and coherence cases use ordinary diagnostics. |
| **Quote hygiene and provenance** | Assign definition-site contexts to literal quote syntax, preserve interpolated contexts and origin chains, and deny fabricated physical locations or caller contexts. | Capture, shadowing, private helper, nested expansion, and diagnostic snapshots demonstrate the specified contexts. |
| **Fixed-point integration** | Re-enter generated ordinary items, imports, attachments, and invocations through the deterministic scheduler while forbidding generated compile-time declarations/imports. | Attribute/derive/macro nesting and cycles terminate in stable source/provenance order. |
| **Ordinary semantic integration** | Route all generated syntax through normal resolution, visibility, generics, trait conformance/coherence, safety, cleanup, checking, lowering, and C emission. | Generated code cannot bypass any handwritten-code restriction. |
| **Built-in compatibility and migration** | Preserve `@vec`/`@map`/`@set`, FFI attributes, and compact compiler derives; implement the attached form for built-in derives and migrate internals only after output and diagnostics are equivalent. | Existing fixtures remain unchanged and attached built-in derives gain equivalent coverage. |
| **Expansion conformance** | Add compile-pass/fail, run-pass, cross-module/package, hygiene, determinism, architecture, nesting, adversarial, capability, and resource-limit suites. | All three forms pass the Linux x86 and x86-64 matrix behind the gate. |

### Compile-time diagnostics, tooling, and stabilization

> Status: Pending.
>
> Blocked by: **Interpreter-backed macros, attributes, and derives**.

**Goal:** Make generated syntax diagnosable, inspectable, reproducible, and
ready to leave the experimental gate.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Expansion-aware diagnostics** | Render generated primary locations with attachment/invocation and definition spans, bounded execution backtraces, `std.ast.error` messages, and stable categories. | Nested snapshots identify both the failure and its complete source chain. |
| **Recovery across executions** | Contain invalid output and nested execution failures so independent diagnostics continue without duplicate cascades or partial syntax. | One failed execution does not suppress or multiply unrelated diagnostics. |
| **Expansion inspection** | Add a CLI mode showing expanded syntax, compile-time execution order, and origin information deterministically for tests and editor tooling. | Repeated dumps are byte-identical and make attribute/derive/macro staging visible. |
| **Dependency and cache identities** | Define stable hashes for declarations, invocations, attachments, imported metadata, interpreter artifacts, compiler/spec/AST versions, and every admitted input. | Input changes invalidate exactly the affected artifact or result. |
| **Robustness campaign** | Fuzz quotation, interpolation, AST transforms, interpreter execution, hygiene, span handling, nesting, and every limit; retain all crash, hang, escape, and nondeterminism regressions. | The retained corpus finishes without an internal failure or unbounded cascade. |
| **Compatibility audit** | Verify macro-free diagnostic, IR, runtime, and generated-C equivalence and built-in behavior on Linux x86 and x86-64. | Enabling the expansion pipeline changes only programs using the new surface. |
| **Stabilization** | Complete documentation and ledger coverage, freeze the initial `std.ast` ABI, decide the long-term compact-derive compatibility policy, and remove `--unstable-macros`. | Local and cross-package conformance suites pass before the gate is removed. |

## 6. Closures

### Explicit-capture closures

> Status: Implemented against `SPEC.md` 0.7.0-draft.
>
> Blocked by: **None**. **Standard-library concurrency** is blocked by this
> milestone because spawned thread bodies rely on closures.

**Goal:** Add locally defined, state-carrying safe callables with explicit
capture boundaries, ordinary Elamite copy and alias semantics, and no implicit
capture or caller-visible unsafe contract.

The accepted surface has these boundaries:

- a captureless closure is written `fn(parameters):` or
  `fn(parameters) -> Return:`; it has no capture brackets and cannot refer to
  an enclosing local binding;
- a capturing closure inserts a nonempty capture list before its parameter
  list, as in
  `fn[value, &shared, &var mutable, *pointer, *var write_pointer](parameters):`;
  every enclosing local used by the body must be named by exactly one capture,
  while module declarations, imports, types, and named functions need no
  capture;
- captures are constructed once, from left to right, when evaluation reaches
  the closure expression. A plain capture takes an independent logical copy,
  `&name` forms a shared reference to the binding's storage, and `&var name`
  forms a mutable reference to mutable binding storage. Captures may use an
  explicit local alias to avoid collisions with parameters or other bindings;
- `*pointer` copies a `*T` pointer and may deliberately downgrade a `*var T`
  pointer to `*T`; `*var pointer` requires and preserves `*var T`. Neither form
  dereferences the pointer or keeps its pointee alive, and a directly captured
  raw-pointer binding must use one of these explicit forms;
- raw-pointer capture, copying, storage, passing, and comparison, including
  comparison with `null`, remain safe. Postfix field access such as
  `pointer.value` automatically dereferences the pointer and therefore requires
  an ordinary `unsafe:` block, performs the required null and alignment checks,
  and permits a write only through `*var T`. A raw-pointer receiver method
  taking `self: *Self` or `self: *var Self` still receives the pointer without
  dereference;
- a captured binding cannot be rebound inside the closure. Mutation through
  `&var T` or `*var T` changes the referenced storage, while ordinary captured
  values remain the closure's private snapshots and copy independently when
  the closure value is copied;
- closure parameters retain explicit types. The return annotation is optional:
  an expected callable result and explicit `return` expressions determine the
  inferred result, reachable fallthrough contributes `()`, and there is still
  no implicit tail-expression return;
- all closures are safe callables. There is no `unsafe fn[...]` closure form;
  a closure may perform an unsafe-only operation only within an explicit
  `unsafe:` block whose author establishes the complete invariant internally.
  A caller-dependent unsafe contract remains a named `unsafe fn`;
- closure literals introduce no generic parameters and are not variadic.
  They may occur inside a generic declaration and may be passed to a generic
  higher-order function, but each resulting closure type is concrete after
  ordinary substitution and monomorphization;
- every closure expression has a distinct anonymous nominal type that
  implements the standard `Callable[Arguments, Return]` trait. Ordinary call
  syntax invokes that trait, arguments are represented by their exact tuple,
  and generic code may accept an inferred concrete callable or erase it behind
  `&Callable[Arguments, Return]`;
- named function references remain thin, stateless `&fn` or `*fn` values.
  Function references may participate in the same callable APIs, but closures,
  including captureless closures, never convert to function references or C
  callbacks; and
- a closure cannot capture its own initializing binding, perform anonymous
  recursion, redirect control into an enclosing function, or inherit an
  enclosing `unsafe:` or `defer` control context. `return`, postfix `?`, `!`
  termination, and `defer` otherwise apply within the closure's own function
  boundary under their ordinary rules.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Normative closure contract (done)** | Record closure syntax, capture forms and aliases, evaluation order, name visibility, inferred returns, callable behavior, copying, safety, escape, control-flow, and exclusion rules in `SPEC.md`; update `LEDGER.md`, `AGENTS.md`, and the authoritative example. | Every accepted and rejected closure form has one normative outcome before anonymous `fn` expressions are enabled. |
| **Syntax and editor support (done)** | Parse safe closure expressions with optional nonempty capture lists, typed parameters, optional return annotations, and ordinary indented bodies; update traversal and editor inventories without admitting generic, unsafe, variadic, or declaration-position forms. | Snapshots preserve capture-kind and body spans, and malformed lists, modifiers, parameters, aliases, and body indentation receive focused diagnostics. |
| **Capture resolution (done)** | Assign stable identities to closure expressions and environment bindings, resolve every outer-local use through one explicit capture, and distinguish declarations that require no capture. | Missing, duplicate, self-initializing, shadowing, alias-collision, and inaccessible captures are deterministic errors with related spans. |
| **Capture typing and construction (done)** | Type-check value, shared-reference, mutable-reference, const-pointer, and mutable-pointer captures; require addressability and mutability where appropriate; record exact-once left-to-right construction and any `*var T` to `*T` downgrade. | Capture types and evaluation order are explicit in typed facts, direct raw-pointer captures cannot bypass `*`/`*var`, and no capture operation itself enters an unsafe context. |
| **Callable types and return inference (done)** | Give each closure expression a unique anonymous nominal type, add the ordinary user-implementable `Callable[Arguments, Return]` contract and call-syntax selection, infer omitted closure results, and preserve exact annotated results including `!`. | Distinct expressions remain distinct types, all return paths agree, fallthrough is unit, generic callable parameters infer concrete closure types, and erased calls retain exact argument and result types. |
| **Body, safety, and control checking (done)** | Check a closure as its own safe function boundary with immutable capture bindings, explicit unsafe blocks, ordinary return/error/defer rules, and no inherited unsafe or escaping control context. | Raw-pointer comparisons compile safely; automatic raw-pointer field access requires `unsafe:`, mutable access requires `*var`, and unsafe closure declarations and anonymous recursion are rejected. |
| **Copy, alias, and escape semantics (done)** | Extend logical-copy recording and promotion analysis to anonymous environments so value fields copy independently, explicit references and pointers preserve identity, and escaping references to captured storage or closure values remain rooted where required. | Later outer rebinding affects only reference captures, copied closures expose independent value state, aliases remain aliases, and a raw pointer alone never roots its pointee. |
| **Typed and control-flow IR lowering (done)** | Represent closure construction, environment access, static callable invocation, erased callable dispatch, return flow, traps, and deferred cleanup without embedding syntax or name-resolution facts in later IR. | Construction and argument evaluation order are explicit, closure-local exits cannot target an outer body, and existing named-function lowering is unchanged. |
| **C99 environment and call emission (done)** | Emit deterministic private environment layouts and static body functions, pass an environment pointer on direct calls, and reuse ordinary trait-object vtables for erased calls while retaining GC-visible roots. | Capturing and captureless closures work on x86 and x86-64, generated C remains C99, symbols are deterministic, and no closure is emitted as a plain C function pointer. |
| **Cross-feature integration (done)** | Exercise closures with generics in enclosing declarations and higher-order APIs, traits, collections, managed and interior references, raw pointers, `Result`, `!`, `defer`, tests, and nested modules. | Closure support does not weaken coherence, copy independence, visibility, safety, cleanup, trap behavior, or production/test reachability. |
| **Conformance and tooling closure (done)** | Add parser snapshots, compile-pass/fail cases, run-pass and trap tests, debug/release and x86/x86-64 coverage, generated-C assertions, documentation, editor synchronization, and macro-produced closure cases when macros are available. | The pre-closure suite remains green and every normative closure rule is mapped to deterministic evidence in `LEDGER.md`. |

Private evolving captured state, implicit or default capture, arbitrary
initialized captures, generic closure literals, unsafe closures, variadic
closures, recursive anonymous closures, callable equality or hashing,
`CallableMut`/`CallableOnce`, captureless conversion to `&fn` or `*fn`, and C
callback conversion are outside this milestone. A later proposal must define
their interaction with logical copying, erasure, cleanup, and concurrency
before adding any of them.

## 7. Native threads and synchronization

### Standard-library concurrency

> Status: Accepted design; pending normative specification.
>
> Blocked by: **Explicit-capture closures**.
>
> This milestone adds no thread, task, `concurrent`, `async`, or `await`
> grammar. Closures supply executable bodies, and ordinary declarations in
> `std.thread` and `std.sync` expose every concurrency operation.

**Goal:** Add safe native parallelism using independent transfer copies and
explicit synchronized identity, without adding ownership, a borrow checker,
implicit shared aliases, catchable thread traps, or nondeterministic cleanup.

The accepted thread and transfer contract is:

- `std.thread.spawn` accepts a safe zero-argument callable, evaluates it once,
  copies its environment across the thread boundary, starts one native thread
  eagerly, and returns `Result[std.thread.Thread[R],
  std.thread.SpawnError]`. Operating-system thread-creation failure is
  recoverable; allocation failure retains the existing fatal OOM behavior;
- a spawned callable and its result must satisfy the compiler-recognized
  structural `Transfer` capability: an independent logical copy may be used on
  another thread while the source remains usable. Primitives, strings,
  ordinary aggregates, and collections satisfy `Transfer` recursively;
  function references satisfy it; and a closure satisfies it exactly when
  every capture does;
- `&T`, `&var T`, `*T`, `*var T`, and `&Trait` do not satisfy `Transfer`.
  Consequently, closures with reference or raw-pointer captures remain valid
  closures but cannot be spawned safely. Concurrency-aware standard handles
  satisfy `Transfer`, and an expert FFI wrapper may opt in only through an
  explicit unsafe `Transfer` implementation whose author guarantees that
  concurrent copies are sound;
- a transfer copy must be physically safe for independent concurrent use.
  The initial implementation deeply detaches ordinary value storage at the
  boundary; COW backing may remain shared only after its reference counts,
  reads, and detach-on-write operations are thread-safe;
- `Thread[R]` is a copyable identity handle. All copies name the same native
  thread and cached result; the runtime performs the OS join once, while each
  `join()` returns an independent logical copy. Joining oneself traps, and
  ordinary cyclic joins may deadlock. A thread handle is transferable when its
  result is transferable;
- threads are joinable and never implicitly detached. Losing every source
  handle does not stop a thread. After the program entry function returns
  normally and completes its deferred cleanup, runtime shutdown waits for all
  remaining Elamite-created threads. There is initially no cancellation,
  interruption, or detach operation;
- a thread body is its own safe function boundary. Its `defer` registrations
  run on ordinary completion, including postfix `?`; a returned `Result` is
  simply `Thread[Result[T, E]]`. A runtime trap, `std.panic`, or OOM reached on
  any thread terminates the complete process and is not converted into a join
  result or a catchable thread failure; and
- safe Elamite programs are data-race free. Scheduling, fairness, relative
  completion, and cross-thread output order are unspecified. Standard output
  calls are internally synchronized so one call is not corrupted by another,
  but call order remains nondeterministic. Unsynchronized concurrent access
  constructed through unsafe code or FFI is undefined behavior.

The accepted `std.sync` foundation is:

- `std.sync.channel[T](capacity: usize)` creates a bounded multi-producer,
  multi-consumer channel and returns a `Sender[T]` and `Receiver[T]`; capacity
  zero is a rendezvous channel, while a separately named
  `unbounded_channel[T]()` may allocate without a fixed capacity;
- sending makes a transfer copy and returns a recoverable closed-channel
  result. Blocking receive returns `Option[T]`, with `None` only after closure
  and draining; nonblocking send and receive report full, empty, and closed
  states distinctly. Sender and receiver copies share synchronized endpoint
  identity;
- channel closure is explicit and idempotent. Garbage collection or loss of
  the last visible sender does not close a channel, because Elamite has no
  deterministic destruction. Closing one endpoint state affects all copies
  according to the normative channel contract;
- `std.sync.Mutex[T]` is a copyable synchronized identity handle for a
  transferable value. It exposes copy-based `new`, `read`, `replace`, and
  atomic `update` operations; neither the mutex nor a guard exposes `&T` or
  `&var T`, so no reference into protected storage can escape after unlocking.
  The update callable receives an independent `T` and returns its replacement
  while the mutex remains locked;
- `std.sync.AtomicBool`, `AtomicI32`, and `AtomicUsize` are copyable handles to
  shared atomic cells rather than independent scalar values. All initial
  atomic, mutex, channel, spawn, completion, and join operations use a
  sequentially consistent memory model with the documented synchronization
  edges; weaker ordering arguments are not exposed; and
- blocking synchronization may deadlock and the runtime need not detect
  general cycles. Self-join is the only initially required deadlock trap.
  Mutex poisoning is unnecessary because an unrecoverable trap terminates the
  process rather than unwinding through another thread.

Every runtime-created thread is registered with the collector before executing
Elamite code, exposes its stack and active transfer environments as roots, and
unregisters only after publishing its result. C code may synchronously call
back into Elamite on the same registered thread that entered C, including a
spawned Elamite thread. A foreign-created thread still cannot enter Elamite,
and asynchronous or concurrent callbacks originating on such a thread remain
undefined behavior until a later design introduces an explicit attachment
trampoline and concurrency contract.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Normative concurrency contract** | Record the thread, transfer, memory-ordering, channel, mutex, atomic, shutdown, trap, cleanup, GC, and callback rules in `SPEC.md`; update `LEDGER.md`, `AGENTS.md`, the standard-library documentation, and close or split I-015. | Every concurrency operation and exclusion has one normative result before any standard declaration or runtime hook is enabled. |
| **Structural transfer capability** | Add canonical `Transfer` facts and generic bounds, structural derivation for ordinary values, conditional derivation for closures and standard handles, explicit exclusions for reference/pointer/trait-object aliases, and the accepted unsafe FFI opt-in. | Spawn inputs and results cannot hide an unapproved alias; diagnostics identify the exact nontransferable capture, field, element, or generic obligation. |
| **Standard concurrency declarations** | Add the accepted `std.thread` and `std.sync` modules and source declarations for thread handles, spawn errors, channels and endpoint outcomes, mutexes, and atomic cells, keeping only representation and lowering hooks intrinsic. | All APIs resolve and type-check through ordinary module, generic, trait, visibility, and standard-library paths. |
| **Transfer-copy lowering** | Lower cross-thread arguments, results, channel messages, and synchronized cell values through an explicit transfer-copy operation that recursively detaches ordinary backing storage while preserving approved synchronized handles. | Mutation on either side cannot observe shared ordinary storage, explicit synchronized identity remains shared, and evaluation occurs exactly once. |
| **Native thread lifecycle** | Implement eager Linux native-thread creation, runtime identity, recoverable creation failure, synchronized result publication, copyable handles, single OS join, repeated logical-result copies, self-join detection, and shutdown waiting on x86 and x86-64. | Successful, failed, nested, multiply joined, handle-discarded, self-joined, and entry-return cases match the accepted lifecycle without implicit detach or cancellation. |
| **Thread body and failure integration** | Lower spawned `Callable[(), R]` bodies as safe function boundaries with ordinary return, `Result`, `!`, trap, panic, and `defer` behavior; synchronize complete output calls without imposing an order. | Normal results and recoverable errors cross predictably, while a trap on any thread terminates the process and never becomes a join value. |
| **Channel implementation** | Implement bounded and unbounded synchronized queues, rendezvous behavior, transfer-copy sends, blocking and nonblocking operations, explicit idempotent closure, draining, and copyable endpoint identity. | MPMC stress, full/empty/closed distinctions, close races, wakeups, ordering within one sender, and abandoned-handle behavior are deterministic where specified and race-free. |
| **Mutex implementation** | Implement copyable `Mutex[T]` identity and copy-based `new`, `read`, `replace`, and callback-driven atomic `update` without exposing protected-storage references. | Concurrent updates do not lose changes, returned values are independent, callback traps remain process-fatal, recursive locking may deadlock, and no safe reference escapes. |
| **Sequentially consistent atomics** | Implement shared `AtomicBool`, `AtomicI32`, and target-width `AtomicUsize` cells with the accepted load, store, exchange, compare-exchange, and integer read-modify-write operations without emitting C11 `_Atomic` into the C99 backend. | Operations are sequentially consistent on both targets, copies retain cell identity, runtime/compiler hooks preserve C99 output, and target-width behavior never assumes 64-bit atomics on x86. |
| **Collector and root integration** | Register and unregister runtime-created threads, scan their stacks, queues, environments, synchronized handles, and unpublished/published results, and make shutdown cooperate with collection. | Stress collection cannot reclaim reachable cross-thread state, raw pointers acquire no rooting behavior, and completed thread state is reclaimable after all roots disappear. |
| **C callback boundary** | Permit synchronous same-registered-thread reentry from C on the initializer or an Elamite-created thread while retaining the prohibition on foreign-created-thread and asynchronous foreign entry. | Nested registered callbacks preserve roots and traps never unwind through C; unsupported foreign-thread entry remains explicitly documented and tested where a harness can detect it. |
| **Concurrency conformance** | Add compile-pass/fail transfer cases, runtime lifecycle and synchronization tests, trap-process tests, high-contention and repeated stress suites, sanitizer-capable native harnesses, debug/release coverage, and the Linux x86/x86-64 matrix. | The complete pre-concurrency suite remains green, safe suites show no races or hangs under their bounded contracts, and every normative concurrency rule is mapped in `LEDGER.md`. |

Cooperative tasks, executors, `async`/`await`, futures, detached execution,
cancellation, interruption, timeouts, thread-local storage, relaxed atomics,
guards exposing protected references, scoped reference transfer, parallel
iterators, scheduling or fairness guarantees, automatic deadlock detection,
and foreign-thread attachment are outside this milestone.

## 9. Recommended delivery checkpoints

Delivery checkpoints are:

- **Initial conformance:** unsafe code, C interoperability, the standard
  library, and the authoritative demonstration satisfy the full initial test
  matrix.
- **Compiler architecture refactor:** compiler phase ownership, package
  parsing, canonical type lowering, IR layering, target configuration, and C
  emission have explicit behavior-neutral boundaries suitable for the
  remaining language and tooling work.
- **Never-return type and explicit panic:** restricted `!` returns and
  `std.panic` provide typed, non-returning control flow without changing
  ordinary `-> T` signatures or introducing catchable traps.
- **Package tests, typed traps, and runner:** package tests run
  deterministically from the current directory or an explicit package path
  without changing production artifacts, and isolated expectations verify
  built-in and user-defined runtime traps.
- **Compile-time diagnostics, tooling, and stabilization:** interpreter-backed
  macros, attributes, and derives are bounded, reproducible, diagnosable, and
  stable across packages through the versioned `std.ast` interface.
- **Explicit-capture closures:** safe anonymous callables preserve explicit
  capture, logical-copy, alias, raw-pointer, escape, and function boundary
  rules across both supported targets.
- **Standard-library concurrency:** native threads exchange only transfer-safe
  copies or synchronized handles, retain process-wide trap and cleanup rules,
  and pass the race, GC, and target conformance matrices.
- **Tuple destructuring and positional fields:** local tuple bindings and
  positional fields preserve exact tuple shape, logical-copy, place,
  reference, and evaluation-order semantics on both targets.

These checkpoints are reporting boundaries, not substitutes for the exit
criteria of the individual milestones.
