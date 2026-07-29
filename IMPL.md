# Elamite Compiler Implementation Plan

> Status: Active — Milestones 0 through 19 are complete and omitted from this
> forward-looking plan; Milestone 20 is next
>
> Next work package: select and measure an M20 optimization candidate
>
> Basis: `SPEC.md` version 0.4.0-draft and
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

Concurrency is not part of the initial implementation plan. It has a separate
design gate at the end of this document because its memory and task model
remains unresolved in `ISSUES.md`.

## 1. Implementation strategy

The compiler should be built as a sequence of complete, testable layers:

```text
manifest and source files
    -> tokens
    -> syntax tree
    -> resolved declarations
    -> typed high-level IR
    -> explicit control-flow IR
    -> monomorphized program
    -> generated C and runtime support
    -> C compiler and linker
```

Each representation should have one clear responsibility:

- Tokens preserve source spans and indentation events.
- The syntax tree represents what was written without performing semantic
  checks.
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
is complete. Outstanding work is divided into ordered packages named
`M<N>.<task>`.

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

### Milestone 20: post-conformance optimization

> Status: Candidate work; select a measured problem before implementation.

**Goal:** Improve implementation quality without changing language behavior.

Candidate work packages are independent proposals, not a promise to implement
all of them in numeric order:

| Task | Candidate change | Required semantic guard |
| --- | --- | --- |
| **M20.1 — Optimization benchmark gate** | Choose a measured problem, record a reproducible before-state, and define the expected improvement before changing lowering. | Full M19 conformance baseline remains available for comparison. |
| **M20.2 — Precise escape analysis** | Keep proven nonescaping address-taken storage on the stack while retaining conservative promotion as the fallback. | Escaping, interior, returned, trait-object, and deferred references preserve root lifetime and identity. |
| **M20.3 — Copy elision** | Elide backend copies or reuse storage when independence cannot become observable. This is not a source-level move operation. | Mutation, alias, evaluation-order, return, pattern, and cleanup tests remain unchanged. |
| **M20.4 — Copy-on-write text** | Replace eager `String` copies with COW storage behind the existing logical-copy interface. | First mutation detaches exactly once; independent copies, explicit aliases, and evaluation order retain specified behavior. |
| **M20.5 — Copy-on-write collections** | Add COW independently for `Vec`, `Map`, and `Set`, one collection at a time. | Mutation, indexing places, iteration snapshots, hashes, and returned removed values remain independent. |
| **M20.6 — Representation specialization** | Specialize concrete generic layouts/helpers where measurement justifies it. | Canonical type identity, ABI exclusion, deterministic symbols, and copy semantics do not change. |
| **M20.7 — Devirtualization** | Replace a trait-object call with static dispatch only when the concrete target is proven and evaluation is unchanged. | Vtable-visible behavior, target identity, object safety, and source-order evaluation remain intact. |
| **M20.8 — Incremental queries** | Introduce stable query boundaries for parsing, resolution, typing, and lowering. | Cache keys include all semantic inputs and clean/incremental outputs are byte-identical. |
| **M20.9 — Dependency artifact cache** | Reuse built package artifacts by deterministic package identity, target, options, compiler/spec revision, and dependency keys. | Stale artifacts cannot cross package instances or target widths. |
| **M20.10 — Parallel package compilation** | Schedule independent package nodes concurrently after deterministic dependency resolution. | Diagnostics, symbols, artifacts, and link-input ordering remain deterministic. |
| **M20.11 — Source maps and editor diagnostics** | Improve generated-source mapping and expose machine-readable diagnostics suitable for editor tooling. | CLI diagnostics retain stable categories/spans and no language-server dependency enters semantic layers. |
| **M20.12 — Optional analyses** | Add warnings for locally provable resource leaks and suspicious raw-pointer contracts. | Warnings are conservative, independently suppressible, and never become required compile errors. |

Every selected package requires before-and-after measurements, a full
conformance run, and targeted tests that would expose a change in copy
independence, evaluation order, identity, root lifetime, trap behavior, or
deterministic output.

Milestone 20 is optional optimization work rather than a gate for the macro
program below. Individual packages, especially source-map infrastructure, may
be pulled forward when a macro milestone needs them.

## 4. Post-conformance macro system

Macro work begins after Milestone 19 reaches initial conformance and after the
corresponding language design is accepted into `SPEC.md`. This roadmap does not
itself define stable macro syntax. Until those design changes land, the
currently reserved `@name` space and compiler-supported built-in macros retain
their existing behavior.

The implementation must preserve these boundaries throughout the transition:

- macro-free packages keep the same semantics, diagnostics, and deterministic
  C output;
- expansion is a distinct phase rather than behavior embedded in parsing,
  resolution, or type checking;
- expanded declarations pass through ordinary resolution, checking, coherence,
  lowering, and backend rules;
- expansion provenance is retained so diagnostics identify both generated code
  and the source invocation that caused it;
- resource limits and expansion ordering are deterministic and testable.

### Milestone 21: macro expansion foundations

> Status: Pending Milestone 19 and normative macro design.

**Goal:** Add behavior-neutral representations and pipeline boundaries on which
user-defined macros can be implemented without destabilizing the existing
compiler.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **M21.1 — Language-contract closure** | Settle definition and invocation forms, delimiter and indentation behavior, expansion roles, macro namespaces, visibility/import rules, expansion order, and deterministic limits in `SPEC.md`; update `LEDGER.md` and close or split the corresponding open issues. | Every implemented macro behavior is owned by a normative rule rather than this roadmap. |
| **M21.2 — Token-tree representation** | Represent nested delimiters, indentation tokens, source text, and spans without prematurely parsing tokens as Elamite syntax. | Existing lexing behavior remains unchanged for macro-free source. |
| **M21.3 — Expansion identities and provenance** | Introduce stable expansion identities and origin chains that distinguish physical source, invocation, definition, and generated spans without inventing physical file offsets. | Nested generated nodes can be traced deterministically to their invocation and definition. |
| **M21.4 — Fragment parser entry points** | Parse complete expression, statement, pattern, type, and item fragments from token trees with full-consumption checks and ordinary parser recovery. | Each fragment role has positive, trailing-token, and malformed-input tests. |
| **M21.5 — Expansion pipeline boundary** | Insert a pass-through expansion phase before name resolution. | Macro-free packages produce equivalent syntax, resolved programs, diagnostics, typed IR, and generated C. |
| **M21.6 — Macro identities and namespace collection** | Add stable macro declaration identities and the accepted package, module, import, visibility, and namespace rules without folding macro lookup into value or type lookup. | Same-name and cross-package cases resolve according to the accepted namespace model. |
| **M21.7 — Deterministic expansion scheduler** | Implement the accepted ordering and fixed-point rules for nested and item-producing expansions, including explicit dependency and cycle diagnostics. | Repeated builds schedule and diagnose the same expansions in the same order. |
| **M21.8 — Resource accounting** | Enforce deterministic recursion, expansion-step, nesting, and generated-output limits. | Limit exhaustion is a stable diagnostic, never a panic or hang. |
| **M21.9 — Experimental gate** | Keep incomplete user-defined macro behavior behind an explicit unstable compiler gate. | Stable invocations cannot accidentally depend on unfinished behavior. |
| **M21.10 — Foundation validation** | Add token-tree, fragment-parser, provenance, scheduler, limit, fuzz, malformed-input, and macro-free equivalence tests. | The foundation is robust before user-defined expansion is enabled. |

### Milestone 22: hygienic declarative macros

> Status: Pending Milestone 21.

**Goal:** Implement ordinary syntax macros with deterministic matching,
transcription, hygiene, and cross-package use.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **M22.1 — Declarative definitions** | Parse and validate the accepted rule, matcher, metavariable, fragment-specifier, and transcription syntax while keeping macro definitions out of ordinary runtime declarations. | Valid definitions enter the macro namespace and invalid definitions fail before expansion. |
| **M22.2 — Matching and repetition** | Match token trees with deterministic rule selection, nested repetition, separators, and useful ambiguity/failure diagnostics under the Milestone 21 resource limits. | Matcher success, failure, ambiguity, and nested repetition have focused tests. |
| **M22.3 — Hygienic transcription** | Attach syntax contexts to generated identifiers and implement the accepted call-site and definition-site lookup behavior; ordinary source identifiers remain in the root context. | Capture and intentional-reference tests demonstrate the specified lookup behavior. |
| **M22.4 — Fragment-role expansion** | Expand expression, statement, pattern, type, and item macros only in roles permitted by `SPEC.md`, then parse the result through the corresponding fragment entry point. | Every supported role has positive and wrong-role diagnostics. |
| **M22.5 — Item fixed point** | Support generated declarations, imports, implementations, and further macro invocations using the accepted deterministic collection order. | Duplicate, dependency, nesting, and cycle cases terminate with stable results. |
| **M22.6 — Modules and packages** | Serialize or otherwise expose the stable macro metadata needed for imports, visibility, aliases, and cross-package expansion without exposing unstable compiler internals. | A downstream package can import and expand a public macro while private macros stay inaccessible. |
| **M22.7 — Semantic integration** | Route generated syntax through ordinary resolution, generic checking, trait conformance/coherence, safety checking, lowering, and C emission. | Interaction tests cover `defer`, `unsafe`, FFI, generics, traits, and managed storage. |
| **M22.8 — Built-in compatibility** | Prove behavioral and diagnostic compatibility for the existing built-in macros before optionally reimplementing any of them on the general expansion path. | Existing built-in fixtures pass unchanged; migration is not required. |
| **M22.9 — Declarative macro conformance** | Add positive, compile-fail, cross-module, cross-package, hygiene, determinism, architecture, nesting, adversarial, and resource-limit suites. | Declarative expansion passes the supported target matrix. |

### Milestone 23: macro diagnostics, tooling, and stabilization

> Status: Pending Milestone 22.

**Goal:** Make declarative macros diagnosable, inspectable, reproducible, and
ready to leave the experimental gate.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **M23.1 — Expansion-aware diagnostics** | Render primary generated spans with related invocation and definition spans, bounded expansion backtraces, and stable diagnostic categories. | Nested-expansion snapshots identify both the failure and its source chain. |
| **M23.2 — Recovery across expansions** | Contain malformed output and nested expansion failures so later independent diagnostics can still be emitted without duplicate cascades. | One failed invocation does not suppress or multiply independent diagnostics. |
| **M23.3 — Expansion inspection** | Add a CLI inspection mode that shows expanded source or syntax with origin information and deterministic ordering suitable for tests and editor tooling. | Repeated dumps are byte-identical and expose useful provenance. |
| **M23.4 — Dependency and cache identities** | Define stable hashes for macro definitions, invocations, imported macro metadata, compiler/spec versions, and relevant inputs. | Future incremental compilation can invalidate expansions precisely without changing current clean builds. |
| **M23.5 — Robustness campaign** | Fuzz matching, transcription, fragment parsing, hygiene, span projection, nesting, and limits; retain regressions for every discovered crash, hang, or nondeterministic result. | The retained corpus completes without an internal failure or unbounded diagnostic cascade. |
| **M23.6 — Compatibility audit** | Re-run the complete compiler suite with expansion enabled and verify macro-free diagnostic, IR, runtime, and generated-C equivalence on Linux x86 and x86-64. | Existing programs remain unchanged by enabling the expansion pipeline. |
| **M23.7 — Declarative stabilization** | Document the supported surface and limits, complete `SPEC.md`/`LEDGER.md` coverage, and remove the experimental gate. | Local and cross-package conformance suites pass before the gate is removed. |

### Milestone 24: declarative custom derive generators

> Status: Pending Milestone 23 and accepted derive design.

**Goal:** Generate ordinary implementations from type structure using bounded
declarative matching and templates, without granting generators arbitrary
execution or access to compiler internals.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **M24.1 — Derive contract** | Specify derive attachment, ordering, namespace, visibility, allowed targets, structural matching/template rules, duplicate behavior, and interaction with built-in derives in `SPEC.md`. | Each accepted and rejected definition and attachment case is normative and tested. |
| **M24.2 — Stable structural input** | Expose a versioned structural representation of names, fields, variants, generic parameters, bounds, visibility, and source spans sufficient for derives but independent of internal AST and semantic table layouts. | Generators need no compiler-private representation. |
| **M24.3 — Derive definitions and lookup** | Parse bounded structural matcher/template definitions and resolve derive generators through the accepted module/package rules with stable identities and deterministic ordering. | Local, imported, renamed, private, malformed, and duplicate generators resolve or diagnose predictably. |
| **M24.4 — Derive expansion phase** | Match structural input and transcribe ordinary items after input declarations are known and before implementation collection/coherence, preserving provenance and hygiene. | Generated implementations participate in the same collection order on repeated builds without executing arbitrary code. |
| **M24.5 — Ordinary semantic validation** | Resolve and check generated implementations exactly like handwritten implementations, including orphan, overlap, generic-bound, safety, and object-safety diagnostics. | Generated code cannot bypass an ordinary semantic restriction. |
| **M24.6 — Built-in derive compatibility** | Retain compiler-supported derives until a general generator matches their semantics, diagnostics, determinism, and cross-package behavior. | Migration remains optional and cannot regress existing derives. |
| **M24.7 — Derive conformance** | Test generic and nongeneric records/enums, visibility, renamed imports, cross-package generators, duplicate derives, malformed output, hygiene, coherence conflicts, and both target architectures. | Custom derives pass the supported target matrix. |

### Milestone 25: compile-time execution runtime

> Status: Pending Milestone 24 and accepted execution/security design.

**Goal:** Provide a bounded and reproducible host-side execution environment
for procedural macro code without confusing host and target compilation.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **M25.1 — Execution model** | Implement the interpreter, bytecode, native-host, or other model accepted by `SPEC.md`, with an explicit versioned interface rather than direct access to compiler data structures. | Macro programs execute through only the accepted interface. |
| **M25.2 — Host/target separation** | Compile and identify macro artifacts for the build host while preserving the selected x86 or x86-64 target for the user's package. | Incompatible artifacts are rejected clearly and never linked as target code. |
| **M25.3 — Token and diagnostic API** | Expose only the versioned token-tree, span, provenance, and diagnostic operations required by procedural macros. | API compatibility has version-skew tests. |
| **M25.4 — Capability boundary** | Deny filesystem, environment, process, clock, randomness, and network access by default; implement only capabilities explicitly accepted by the language/build design and include them in reproducibility metadata. | Undeclared external access fails as a contained diagnostic. |
| **M25.5 — Failure isolation and limits** | Contain panics, crashes, invalid results, recursion, excessive work, memory growth, and output growth as bounded diagnostics without corrupting compiler state. | A failed macro cannot crash or hang the compiling process. |
| **M25.6 — Artifact and cache identity** | Key macro artifacts and results by source, transitive dependencies, host platform, interface/compiler/spec versions, declared capabilities, and all accepted external inputs. | Input changes invalidate exactly the affected artifact or result. |
| **M25.7 — Runtime validation** | Add reproducibility, isolation, malformed-artifact, version-skew, resource-limit, cache-invalidation, host/target, and concurrent-build tests. | The runtime is deterministic and isolated across repeated builds. |

### Milestone 26: procedural macros and attributes

> Status: Pending Milestone 25 and accepted procedural-macro design.

**Goal:** Support function-like procedural expansion, procedural derives, and
item attributes on the bounded compile-time runtime.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **M26.1 — Function-like procedural macros** | Register, resolve, execute, and parse token-tree transformations in every accepted fragment role with the same provenance, hygiene, scheduling, and limits as declarative macros. | Function-like procedural macros satisfy the declarative interaction matrix. |
| **M26.2 — Procedural derives** | Feed the stable Milestone 24 structural input to procedural generators and validate their output through ordinary implementation collection and coherence. | Procedural derives cannot observe private compiler structures or bypass coherence. |
| **M26.3 — Attribute attachment and input** | Parse the accepted attribute grammar, attach attributes to permitted items, preserve raw token input and spans, and reject invalid placement before execution. | Attachment and input are deterministic and independently diagnosable. |
| **M26.4 — Attribute expansion semantics** | Apply the accepted ordering relative to declarative expansion and derives; allow attributes to replace, remove, or produce items only as specified, then return all output to ordinary collection and checking. | Nested and interacting attributes reach a deterministic fixed point. |
| **M26.5 — Procedural span and hygiene API** | Support the accepted call-site, definition-site, and generated identifier operations without allowing fabricated physical locations or bypassing visibility. | Span and capture tests exercise every exposed context operation. |
| **M26.6 — Failure behavior and tooling** | Surface execution failures with bounded expansion backtraces, preserve compiler recovery, and provide useful inspection output when a macro cannot execute. | Failures remain local and explain the invocation/definition chain. |
| **M26.7 — Cross-package distribution** | Build, discover, version, and cache procedural macro artifacts reproducibly across packages without loading target artifacts into the host compiler. | Clean and cached cross-package builds are equivalent. |
| **M26.8 — Procedural conformance and stabilization** | Test ordering, nesting, hygiene, generated imports/items/impls, capability denial, crashes, limits, version skew, cross-package use, deterministic rebuilds, and both target architectures. | Procedural macros and attributes leave the experimental surface only after the complete matrix passes. |

## 5. Deferred concurrency design gate

Do not implement task syntax, schedulers, channels, synchronization traits, or
cross-thread callbacks until `I-015` defines:

- whether execution is only cooperative or may be parallel;
- which values may cross a task boundary;
- how safe references, mutable aliases, raw pointers, trait objects, function
  references, and resource handles behave across that boundary;
- the data-race and memory-ordering model;
- structured task lifetime, cancellation, and the cleanup behavior of `defer`;
- task failure and recoverable error propagation;
- GC registration and root scanning for runtime-created and foreign-created
  threads;
- synchronization primitives and any compiler-recognized transfer capability;
- callback rules when C invokes Elamite from another or concurrent thread.

After those rules are normative, concurrency should be added as a new vertical
slice: syntax and resolution, static transfer checks, control-flow lowering,
runtime scheduler and synchronization, GC integration, cancellation cleanup,
foreign-thread entry, and stress/race testing. It should not require weakening
the sequential semantics established by the initial milestones.

## 6. Recommended delivery checkpoints

Completed checkpoints are omitted with their milestones. The remaining
delivery checkpoints are:

1. **Initial conformance checkpoint — Milestone 19:** unsafe code, C
   interoperability, the standard library, and the authoritative demonstration
   satisfy the full initial test matrix.
2. **Declarative macro checkpoint — Milestone 23:** hygienic declarative
   macros are stable, diagnosable, and usable across packages.
3. **Extensible macro checkpoint — Milestone 26:** procedural macros, derives,
   and attributes are bounded, reproducible, and stable across packages.

These checkpoints are reporting boundaries, not substitutes for the exit
criteria of the individual milestones.
