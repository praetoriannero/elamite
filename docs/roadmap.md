# Elamite Compiler Roadmap

> Status: Active — older completed work is summarized in the ledger; active,
> candidate, and recently completed milestones use stable descriptive names
>
> Next planned milestone: **Source-level debugging**
>
> Basis: implemented `spec.md` version 0.10.0-draft and the authoritative
> `examples/spec_demo.elx` demonstration

## Milestone summary

Keep this table synchronized with the detailed status blocks below.

| Milestone | Status | Current state or next action |
| --- | --- | --- |
| [Memory cost model documentation](#memory-cost-model-documentation) | Complete | The versioned cost model, instrumentation, fixed workloads, baseline, and maintenance contract are in place. |
| [Shallow-copy and systems-concurrency migration](#shallow-copy-and-systems-concurrency-migration) | Complete | Shallow values, systems concurrency, unsafe pointer traversal, final cost evidence, release identity, and the authoritative 0.10 demonstration are complete. |
| [Post-conformance optimization](#post-conformance-optimization) | Candidate | Optional measured work includes specialization, devirtualization, incremental queries, artifact caching, parallel packages, source maps, and warnings. |
| [Macro expansion foundations](#macro-expansion-foundations) | Complete | Token trees, provenance, fragment parsing, expansion identities, scheduling, resource accounting, and validation are complete. |
| [Compile-time AST and interpreter](#compile-time-ast-and-interpreter) | Complete | The versioned `std.ast` façade, quotation, checking, bounded interpreter, capability boundary, and artifact identities are complete; inherent blocks advance its exact ABI to 2.0. |
| [Interpreter-backed macros, attributes, and derives](#interpreter-backed-macros-attributes-and-derives) | Complete | All three stable compile-time declaration forms expand through the ordinary semantic pipeline. |
| [Compile-time diagnostics, tooling, and stabilization](#compile-time-diagnostics-tooling-and-stabilization) | Complete | Expansion diagnostics, recovery, inspection, reproducibility, robustness, compatibility, and stabilization are complete. |
| [Explicit-capture closures](#explicit-capture-closures) | Complete | Closure syntax, typing, capture semantics, IR/backend lowering, cross-feature behavior, and conformance are complete. |
| [Standard-library concurrency](#standard-library-concurrency) | Complete | The 0.10 shallow shared-memory contract, ordering edges, runtime lifecycle, and sanitizer-backed conformance matrix are implemented. |
| [User-defined iteration](#user-defined-iteration) | Complete | The ordinary `Iterator[Element]` protocol, static checking/lowering, managed hidden state, and unchanged direct collection behavior are implemented. |
| [Inherent implementation blocks](#inherent-implementation-blocks) | Complete | Field-only structs, local coherent generic inherent blocks, selection/lowering, `std.ast` 2.0, and repository migration are complete. |
| [Deferred specified surface](#deferred-specified-surface) | Candidate | Close or permanently document 128-bit integers, wildcard and grouped imports, and the foreign ABI surface. |
| [Standard-library expansion](#standard-library-expansion) | Complete | The accepted filesystem, process/environment, time, ordering/search, text, and deterministic-randomness surfaces are implemented and documented; source hosting is the follow-up milestone below. |
| [Source-hosted standard library](#source-hosted-standard-library) | Complete | An exact native inventory, explicit intrinsic declarations, a minimal UTF-8 kernel, source-hosted text/path algorithms, and demand-driven standard reachability are implemented. |
| [Source-level debugging](#source-level-debugging) | Planned | Map generated C back to `.elx` locations and preserve source-level names so native debuggers are usable. |
| [Language server](#language-server) | Candidate | Requires a scope decision and the **Incremental queries** package before implementation. |
| [API documentation generation](#api-documentation-generation) | Planned | The `doc` command exists but does not yet render API content, cross-links, or a distributable format. |
| [Additional platform targets](#additional-platform-targets) | Candidate | Begin with C-toolchain portability, then 64-bit ARM, then a non-ELF platform. |
| [Package distribution](#package-distribution) | Candidate | Requires an accepted distribution model before a resolver or lockfile is implemented. |
| [Distribution and installation](#distribution-and-installation) | Planned | Produce release artifacts, a tested installation path, and a maintained changelog. |
| [Learning material](#learning-material) | Planned | Write an introductory guide and tested worked examples separate from the specification. |
| [Project governance and contribution](#project-governance-and-contribution) | Planned | Add contribution, conduct, and security infrastructure, a design-change process, and a stability policy. |

This document organizes current compiler work, candidate optimizations,
recently completed language extensions, and the remaining language-surface,
standard-library, toolchain, and distribution work into implementation
milestones. Each milestone carries its own status note; the header above
records the next required package. Elamite compiles to C, as required by the
specification.

Sections 7 through 10 record work that is planned or candidate rather than
started. They exist so that no known gap is tracked only informally; their
ordering within a section is significant, but the sections themselves are not
ordered against each other and do not define a release gate.

Detailed plans are removed from this file once every work package in a
milestone is complete. Normative behavior remains in `spec.md`, rule coverage
and implementation status remain in `ledger.md`, and implementation evidence
remains in tests and version history.

The plan was originally written without choosing the language in which the
compiler is written, a parser technology, or a build system. Those are now
settled: the compiler is an edition-2024 Rust package built with Cargo, and its
lexer and parser are hand-written. See [`AGENTS.md`](../AGENTS.md) for the rules those choices
imply and `ledger.md` §18 for the third-party crate decisions behind them.

`spec.md` and the authoritative demonstration define the language. This
document defines an implementation order, not new language semantics. When it
conflicts with the specification, the specification wins and this plan must be
updated.

Concurrency is a post-closure extension to the initial implementation plan.
The **Standard-library concurrency** milestone records the implemented 0.9
runtime baseline; the completed migration milestone records its replacement by
the normative 0.10 shallow shared-memory revision.

## 1. Implementation strategy

The compiler should be built as a sequence of complete, testable layers:

```text
manifest and source files
    -> tokens
    -> parsed package syntax
    -> expanded package syntax (macro-expanded and origin-aware)
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
- Expanded package syntax owns lossless token trees, generated syntax, stable
  origin chains, and the completed fixed-point result consumed as the sole
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

The 0.9 implementation favored correctness and inspectability through deep
copying and conservative promotion. Specification 0.10 deliberately changes
ordinary copying to shallow representation copying and adopts programmer-managed
shared-memory concurrency. The completed migration preserves explicit IR facts
while every ordinary, thread, channel, mutex, closure, pattern, and collection
copy site follows the revised contract; precise escape analysis, incremental
compilation, and aggressive C output optimization remain later work.

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
copy and alias behavior once those subsystems exist.

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

Active, candidate, and recently completed milestones remain below. Within an
active milestone, completed work packages stay marked until the whole milestone
is complete. Outstanding work is divided into ordered, descriptively named
packages.

Each work package should normally fit in one focused change and must:

- leave the compiler building with all earlier tests passing;
- add the smallest representation, semantic rule, or runtime hook needed by
  that package, without partially enabling later syntax;
- include focused positive and negative tests, plus a runtime test when it can
  trap or has observable ordering;
- update `ledger.md` when completing it changes the recorded implementation or
  test status of a normative rule; and
- record any deliberately temporary boundary in the milestone status note.

Task order within a milestone is significant unless a task explicitly says it
may proceed in parallel. A work package is not a new language-design authority:
`spec.md` remains normative, and splitting work must not create observable
intermediate semantics that contradict it.

## 3. Current implementation roadmap

### Memory cost model documentation

> Status: Complete. `cost_model.md` inventories the current implementation,
> the opt-in `elamite-cost-v1` counters measure requested allocations and
> explicit copied bytes, and fixed release workloads establish the baseline.
>
> Blocked by: **None**. Its measured baseline should precede
> **Shallow-copy and systems-concurrency migration**.

**Goal:** Give programmers a usable, versioned account of where Elamite copies,
allocates, promotes storage, retains memory, and synchronizes, while keeping
semantic guarantees distinct from current implementation costs and future
targets.

The initial cost model is non-normative. `spec.md` continues to own observable
value behavior; the cost document describes the shipped compiler and clearly
labels intended bounds that are not yet achieved. A bound moves into `spec.md`
only after its implementation, measurements, target coverage, and compatibility
costs have been reviewed separately.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Copy and allocation inventory (done)** | Inventory binding, assignment, argument, return, pattern, closure-capture, iteration-snapshot, transfer, concatenation, collection mutation, reference-promotion, and synchronization costs by type family. | Each operation distinguishes semantic copying, physical copying, allocation, retained storage, and implementation freedom; x86 and x86-64 differences are explicit. |
| **Reproducible memory baseline (done)** | Add release-mode microbenchmarks and allocation/byte-copy instrumentation for representative `String`, `Vec`, `Map`, `Set`, nested aggregate, closure, function-call, loop, and cross-thread workloads. | Results are reproducible enough to compare revisions, record input sizes and toolchain/target identity, and avoid timing or allocation thresholds in ordinary conformance tests. |
| **Published cost model (done)** | Add a non-normative `cost_model.md` describing current asymptotic behavior, likely allocation sites, GC retention and nondeterminism, conservative promotion, explicit alias and synchronized-handle costs, and the intended optimized model. | A programmer can predict which source operations may allocate or copy proportionally to data size and can tell a guarantee from an implementation note or optimization target. |
| **Cost-document maintenance contract (done)** | Define the review rule for representation, lowering, runtime, and optimizer changes that alter documented costs, including required before/after measurements and release-note updates. | A cost-changing implementation cannot be marked complete while `cost_model.md` still describes the previous behavior. |

### Shallow-copy and systems-concurrency migration

> Status: Complete. The normative 0.10 documentation revision is complete;
> ordinary assignment, calls, returns, captures, patterns, indexing, and
> propagation now copy shallowly, and standard collections use their accepted
> shallow representations, hidden iteration state snapshots its bound once,
> threads/channels publish ordinary shallow values without `Transfer`, and
> mutex operations shallow-copy stored and returned values. Raw data pointers
> support typed arithmetic, subtraction, compound offsets, indexing, and
> null-low relational ordering. The synchronization-edge audit, final measured
> cost baseline, release identity, and authoritative demonstration are complete.
>
> Depends on: **Memory cost model documentation** and **Standard-library
> concurrency** (both complete).

**Goal:** Implement Go-like shallow ordinary values and C-like shared-memory
concurrency without weakening existing type, bounds, managed-lifetime, raw
pointer, trap, evaluation-order, or C99 guarantees beyond the explicitly
revised copy and data-race contracts.

Every old deep-copy and isolation boundary has migrated, and the compiler,
cost model, release evidence, and demonstration now identify the implemented
0.10.0-draft revision consistently.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Normative shallow-memory revision (done)** | Revise `spec.md`, `ledger.md`, the roadmap, issues/history notes, and current-versus-planned cost documentation for shallow fieldwise copies, shared collection backing, programmer-managed races, and unsafe pointer traversal. | One normative rule owns assignment, calls, captures, patterns, collections, threads, channels, mutexes, pointer arithmetic/indexing/ordering, null ordering, and undefined behavior. |
| **Retain the measurement seam (done)** | Keep the existing logical-copy inventory, read-only call analysis, temporary reuse facts, and pre-revision benchmark as migration evidence rather than deleting phase boundaries prematurely. | Tests can distinguish old materialized copies from new shallow copies and benchmark observations remain comparable without becoming semantic thresholds. |
| **Shallow ordinary-copy lowering (done)** | Change assignment, arguments, returns, tuple destructuring, pattern binding, closure capture/copy, indexing results, postfix `?`, and aggregate operations to copy only immediate representations; retain explicit IR inventory while ordinary C lowering bypasses recursive transfer helpers. | Scalars and inline slots remain independent, while descriptor-bearing nested fields and copied closure environments observably preserve identity through calls, returns, captures, patterns, and repeated copies; the focused alias matrix passes in debug and release. |
| **Shallow standard collection representations (done)** | Replace String COW detachment and eager ordinary collection duplication with the specified mutable `String` and Go-like `Vec` descriptors plus identity-preserving `Map` and `Set` tables. | String byte mutation aliases; vector element writes alias while length/capacity stay descriptor-local and growth may diverge; map/set structural mutation is visible through every copy. |
| **Iteration and mutation invalidation (done)** | Lower hidden loop state as a shallow copy and enforce or document the specified invalidation boundary for length-changing vector and structural map/set mutation during iteration. | Element/value replacement visibility, vector growth divergence, invalid mutation cases, and left-to-right evaluation match Section 7.1 without dangling generated-C storage. |
| **C-like thread and channel publication (done)** | Remove structural `Transfer` checking and recursive transfer helpers; shallow-copy spawn environments, cached join results, repeated joins, and channel messages while retaining native publication and GC rooting. | References, pointers, slices, trait objects, and collection backing can cross threads; pre-publication writes are observed; unsynchronized conflicting access is treated as UB rather than rejected or detached. |
| **Programmer-managed mutex contract (done)** | Update `Mutex[T]` operations to shallow values and verify that locking serializes callers without claiming that external aliases are isolated or compiler-associated with the mutex. | Tests demonstrate correct synchronized sharing and deliberately racy fixtures remain compile-only negative evidence rather than ordinary executions. |
| **Unsafe pointer arithmetic and indexing (done)** | Add `*T`/`*var T` element-scaled `+`, `-`, `+=`, `-=`, same-extent subtraction to `isize`, and `pointer[index]` places for complete nonzero-sized data pointees. | Precedence, inference, mixed mutability, one-past construction, assignment, single evaluation, null/alignment traps, incomplete types, both pointer widths, and generated C99 are covered. |
| **Unsafe pointer relational ordering (done)** | Add `<`, `<=`, `>`, and `>=` as primitive unsafe raw-data-pointer operations without `PartialOrd`/`Ord`; define null below all non-null pointers and require common live extent for two non-null operands. | Null cases lower without invalid C null ordering, same-extent loops work, statically evident invalid forms diagnose, and unrelated non-null ordering remains documented UB. |
| **Concurrent memory-model conformance (done)** | Test thread-start, mutex, channel, join, and sequentially consistent atomic ordering; document that ordinary shared access needs no `unsafe` and that conflicting unordered C99 accesses are UB. | Correctly synchronized stress remains TSan-clean, race-producing samples are never run as conformance programs, and no documentation claims safe code is data-race-free. |
| **Cost-model and release migration (done)** | Run comparable before/after memory baselines once lowering changes, replace the current 0.9 cost tables with achieved shallow-copy costs, and record the semantic break in release documentation and version output. | `elamc --version`, README, demo, cost model, fixtures, x86/x86-64 matrices, and release evidence all identify one implemented specification revision. |

### Post-conformance optimization

> Status: Candidate work; select a measured problem before implementation.
>
> Blocked by: **None**.

**Goal:** Improve implementation quality without changing language behavior.

The memory-cost and value-copy packages above are planned work rather than
members of this optional pool. The remaining candidate packages are independent
proposals, not a promise to implement all of them in table order:

| Task | Candidate change | Required semantic guard |
| --- | --- | --- |
| **Optimization benchmark gate** | Choose a measured problem, record a reproducible before-state, and define the expected improvement before changing lowering. | The complete compiler-architecture refactor baseline remains available for comparison. |
| **Representation specialization** | Specialize concrete generic layouts/helpers where measurement justifies it. | Canonical type identity, ABI exclusion, deterministic symbols, and copy semantics do not change. |
| **Precise escape analysis** | Keep proven nonescaping address-taken locals and referenced temporaries on the stack while retaining conservative managed promotion for returned, stored, captured, deferred, foreign, or otherwise uncertain references. | Root lifetime and identity match the managed-lifetime contract while measured nonescaping cases allocate no managed cell. |
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

## 4. Post-conformance compile-time syntax generation

The compile-time syntax-generation milestone is complete. `spec.md` §12 owns
the stable interpreter-backed `macro`, `attr`, and `derive` declarations, the
versioned `std.ast` interface, `quote:` and `$` interpolation, `++`
concatenation, hygiene, scheduling, and deterministic resource limits.

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

> Status: Complete. The compile-time surface is unchanged by `spec.md`
> 0.10.0-draft.
>
> Blocked by: **None**. It is independent of **Package tests, typed traps, and
> runner**.

**Goal:** Add behavior-neutral representations and pipeline boundaries on which
user-defined macros, attributes, and derives can be implemented without
destabilizing the existing compiler.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Language-contract revision (done)** | Replace token matcher/transcriber rules with interpreter-backed `derive`, `attr`, and `macro` declarations over `std.ast`; settle `@` invocation and attachment forms, `quote:`, `$` interpolation, `++`, namespaces, execution order, hygiene, capabilities, and deterministic limits in `spec.md`. | Every implemented compile-time behavior is owned by a normative rule rather than this roadmap. |
| **Token-tree representation (done)** | Represent nested delimiters, indentation tokens, source text, and spans without prematurely parsing tokens as Elamite syntax. | Existing lexing behavior remains unchanged for macro-free source. |
| **Expansion identities and provenance (done)** | Introduce stable expansion identities and origin chains that distinguish physical source, invocation, definition, and generated spans without inventing physical file offsets. | Nested generated nodes can be traced deterministically to their invocation and definition. |
| **Fragment parser entry points (done)** | Parse complete expression, statement, pattern, type, and item fragments from token trees with full-consumption checks and ordinary parser recovery. | Each fragment role has positive, trailing-token, and malformed-input tests. |
| **Expansion pipeline boundary (done)** | Replace the compiler-architecture refactor's pass-through seam with expansion-owned unit identities, lossless token trees, provenance, and an owned package result consumed before name resolution. | Macro-free packages preserve their source inputs and the explicit parse → expand → resolve path is equivalent to the normal resolver entry point; the complete downstream suite preserves diagnostics, typed IR, runtime behavior, and generated C. |
| **Compile-time identities and namespace collection (done)** | Parse the minimal physical declaration/import surface and add stable macro, attribute, and derive declaration/import/module identities plus package, module, visibility, alias, re-export, and separate-namespace collection before ordinary resolution. Signature semantics and execution remain later packages. | Same-name declarations across ordinary/macro/attribute/derive namespaces, renamed nested-module imports, duplicates, private bindings, public re-exports, and cross-package declarations resolve or diagnose predictably. |
| **Deterministic expansion scheduler (done)** | Implement the structural fixed-point queue and dependency graph, including attribute-before-derive ordering, outermost-first function macros, generated-item re-entry, cycle diagnostics, and stable recovery nodes. The scheduler is execution-independent until the compile-time interpreter supplies output. | Repeated builds schedule and diagnose the same expansions in the same order. |
| **Resource accounting seam (done)** | Give the scheduler shared depth, execution, generated-node, interpreter-fuel, and live-value budgets before execution exists. Generated output is admitted atomically, and per-execution exhaustion remains sticky even when a driver ignores the immediate charge result. | Limit charging is stable and cannot be bypassed by nested or generated work. |
| **Experimental gate (done, retired)** | The explicit `--unstable-macros` gate covered every compiler entry point while execution was incomplete. Stabilization removed it after the conformance audit. | No stable build accidentally depended on partial behavior during development. |
| **Foundation validation (done)** | Add directed and property coverage for token-tree losslessness, every fragment parser role, generated provenance chains, deterministic scheduler staging, resource limits, malformed input, the experimental gate, and macro-free equivalence across all shipped package examples. | Arbitrary lexer output recovers without panics or fabricated spans, generated chains terminate at their configured bounds, and the complete compiler suite preserves macro-free behavior. |

### Compile-time AST and interpreter

> Status: Complete. The versioned `std.ast` façade, typed quotation, checked lowering,
> bounded interpreter, capability boundary, identities, and validation are
> implemented.
>
> Blocked by: **None** — **Macro expansion foundations** is complete.

**Goal:** Implement the target-independent `std.ast` façade, quotation, and a
bounded interpreter for ordinary safe Elamite compile-time code.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Versioned `std.ast` façade (done)** | Define opaque immutable structural syntax values, stable accessors and `with_` transforms, validating builders, persistent AST lists, origin handles, pattern variants, and contained `std.ast.error` failures without exposing compiler-owned nodes or tables. Expanded packages carry an exact handshake and sorted intrinsic type inventory; the frozen 1.0 surface transitions explicitly to 2.0 for inherent blocks. Only expansion can mint origins, and generated failures retain invocation/definition context without fabricated spans. | Directed and property tests cover every admitted value family and variant, every published transform, invalid identifiers and paths, exact version skew, arbitrary persistent-list concatenation, physical diagnostics, and generated diagnostic context. |
| **Quote and interpolation syntax (done)** | Lex and parse role-neutral, indentation-delimited `quote:` templates and `$name`/`$(expression)` sites without parsing quoted source prematurely. Infer explicit binding and compile-time return roles for every admitted `std.ast` scalar, list, item, and definition type; distinguish scalar insertion from collection splicing; validate adapted bodies through the ordinary hand-written grammar; preserve physical spans; reject runtime quotation; and retain parameter-driven inference for compile-time signature checking. Hygiene context assignment and conversion to actual façade values remain interpreter-lowering work. | Lexer, parser, formatter, editor, expansion, directed role/malformed/wrong-role/nesting/indentation tests, and property-generated named/computed interpolation streams cover the complete syntax boundary. |
| **Concatenation operator (done)** | Add binary `++` at additive precedence for strings, supported sequences, and AST lists while keeping numeric `+` separate and rejecting arbitrary AST-expression concatenation. | Lexer, parser, checker, runtime, formatter, and editor tests agree on the new operator. |
| **Compile-time checking and lowering (done)** | Check compile-time signatures and bodies through the ordinary language front end, reject runtime-only and unsafe capabilities, and lower the admitted subset to a versioned interpreter representation. | Invalid signatures and operations fail before execution with ordinary source spans. |
| **Bounded interpreter (done)** | Execute safe Elamite control flow, values, pattern matching, functions, and `std.ast` intrinsics deterministically with explicit fuel and live-value accounting. | Repeated execution is identical; recursion, loops, allocation, panics, and invalid intrinsics are contained diagnostics. |
| **Capability and host/target boundary (done)** | Deny FFI and ambient filesystem, environment, process, network, clock, randomness, target, runtime, and compiler-internal access; keep compile-time execution independent of the selected output target. | Capability probes fail predictably and x86/x86-64 builds expand identically. |
| **Artifact and dependency identity (done)** | Serialize or rebuild public compile-time bodies and `std.ast` ABI metadata with identities keyed by source, transitive compile-time dependencies, and compiler/spec/interface versions. | Clean, cached, local, and cross-package execution produce equivalent results and reject version skew. |
| **Interpreter validation (done)** | Add unit, property, fuzz, adversarial, reproducibility, limit, recovery, host/target, version-skew, and cross-package suites. | The interpreter cannot hang, crash, escape its capability boundary, or mutate compiler state. |

### Interpreter-backed macros, attributes, and derives

> Status: Complete.
>
> Blocked by: **Compile-time AST and interpreter**.

**Goal:** Expose all three accepted declaration forms on the shared `std.ast`
and interpreter foundation with deterministic staging and ordinary semantic
validation.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Declaration syntax and lookup (done)** | Parse `[pub] macro`, `[pub] attr`, and `[pub] derive` signatures and bodies, including one final homogeneous variadic AST parameter where §12 permits it; collect physical declarations/imports in separate namespaces with stable identities, visibility, aliases, and re-exports. | Local, duplicate, renamed, private, malformed, variadic-placement, and cross-package cases resolve or diagnose predictably. |
| **Function-like macros (done)** | Parse `@path(...)` in expression, pattern, type, whole-statement, and module-item roles; construct fixed and variadic typed AST arguments, execute the declaration, and validate the declared return role. | Each role has successful, zero/many variadic, empty, wrong-role, trailing, nested, and recovery coverage. |
| **Structural attributes (done)** | Attach `@attr(path)`/`@attr(path(...))` to accepted definitions, supply the typed target implicitly, pack any final variadic explicit AST arguments, run top-to-bottom, and admit same-kind replacement or validated `ItemList` output. | Field/method addition, fixed/variadic arguments, replacement, removal, sibling emission, bad target, and interacting attributes behave deterministically. |
| **Trait derives (done)** | Run `@derive(...)` after attributes, validate the exact trait and target identity of each returned `Implementation`, and retain the original definition. | Struct/enum, generic, duplicate, bad-output, orphan, overlap, bound, and coherence cases use ordinary diagnostics. |
| **Quote hygiene and provenance (done)** | Assign definition-site contexts to literal quote syntax, preserve interpolated contexts and origin chains, and deny fabricated physical locations or caller contexts. | Capture, shadowing, private helper, nested expansion, and diagnostic snapshots demonstrate the specified contexts. |
| **Fixed-point integration (done)** | Re-enter generated ordinary items, imports, attachments, and invocations through the deterministic scheduler while forbidding generated compile-time declarations/imports. | Attribute/derive/macro nesting and cycles terminate in stable source/provenance order. |
| **Ordinary semantic integration (done)** | Route all generated syntax through normal resolution, visibility, generics, trait conformance/coherence, safety, cleanup, checking, lowering, and C emission. | Generated code cannot bypass any handwritten-code restriction. |
| **Built-in compatibility and migration (done)** | Preserve `@vec`/`@map`/`@set`, FFI attributes, and compact compiler derives; implement the attached form for built-in derives and migrate internals only after output and diagnostics are equivalent. | Existing fixtures remain unchanged and attached built-in derives gain equivalent coverage. |
| **Expansion conformance (done)** | Add compile-pass/fail, run-pass, cross-module/package, hygiene, determinism, architecture, nesting, adversarial, capability, and resource-limit suites. | All three forms pass the Linux x86 and x86-64 matrix as stable language features. |

### Compile-time diagnostics, tooling, and stabilization

> Status: Complete. User compile-time forms are stable and the former
> `--unstable-macros` gate has been removed.
>
> Blocked by: **Interpreter-backed macros, attributes, and derives**.

**Goal:** Make generated syntax diagnosable, inspectable, reproducible, and
stable without an experimental gate.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Expansion-aware diagnostics (done)** | Render generated primary locations with attachment/invocation and definition spans, bounded execution backtraces, `std.ast.error` messages, and stable categories. | Nested snapshots identify both the failure and its complete source chain. |
| **Recovery across executions (done)** | Contain invalid output and nested execution failures so independent diagnostics continue without duplicate cascades or partial syntax. | One failed execution does not suppress or multiply unrelated diagnostics. |
| **Expansion inspection (done)** | Add a CLI mode showing expanded syntax, compile-time execution order, and origin information deterministically for tests and editor tooling. | Repeated dumps are byte-identical and make attribute/derive/macro staging visible. |
| **Dependency and cache identities (done)** | Define stable hashes for declarations, invocations, attachments, imported metadata, interpreter artifacts, compiler/spec/AST versions, and every admitted input. | Input changes invalidate exactly the affected artifact or result. |
| **Robustness campaign (done)** | Fuzz quotation, interpolation, AST transforms, interpreter execution, hygiene, span handling, nesting, and every limit; retain all crash, hang, escape, and nondeterminism regressions. | The retained corpus finishes without an internal failure or unbounded cascade. |
| **Compatibility audit (done)** | Verify macro-free diagnostic, IR, runtime, and generated-C equivalence and built-in behavior on Linux x86 and x86-64. | Enabling the expansion pipeline changes only programs using the new surface. |
| **Stabilization (done)** | Complete documentation and ledger coverage, freeze the initial `std.ast` ABI, retain compact built-in derives as compatibility syntax alongside attached `@derive(...)`, and remove `--unstable-macros`. | Local and cross-package conformance suites pass with the stable surface. |

## 5. Closures

### Explicit-capture closures

> Status: Complete; 0.10 shallow capture construction and closure-copy identity
> are implemented, including cross-thread closure publication through the
> completed **Shallow-copy and systems-concurrency migration**.
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
  the closure expression. A plain capture takes an ordinary shallow copy,
  `&name` forms a shared reference to the binding's storage, and `&var name`
  forms a mutable reference to mutable binding storage. Captures may use an
  explicit local alias to avoid collisions with parameters or other bindings;
- `*pointer` copies a `*T` pointer and may deliberately downgrade a `*var T`
  pointer to `*T`; `*var pointer` requires and preserves `*var T`. Neither form
  dereferences the pointer or keeps its pointee alive, and a directly captured
  raw-pointer binding must use one of these explicit forms;
- raw-pointer capture, copying, storage, passing, equality, and comparison with
  `null` through `==`/`!=` remain safe. Arithmetic, indexing, and relational
  ordering require `unsafe`. Postfix field access such as
  `pointer.value` automatically dereferences the pointer and therefore requires
  an ordinary `unsafe:` block, performs the required null and alignment checks,
  and permits a write only through `*var T`. A raw-pointer receiver method
  taking `self: *Self` or `self: *var Self` still receives the pointer without
  dereference;
- a captured binding cannot be rebound inside the closure. Mutation through
  `&var T` or `*var T` changes the referenced storage; a plain capture owns its
  inline environment slot while descriptors and handles preserve shallow
  backing identity when the closure value is copied;
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
| **Normative closure contract (done)** | Record closure syntax, capture forms and aliases, evaluation order, name visibility, inferred returns, callable behavior, copying, safety, escape, control-flow, and exclusion rules in `spec.md`; update `ledger.md`, `AGENTS.md`, and the authoritative example. | Every accepted and rejected closure form has one normative outcome before anonymous `fn` expressions are enabled. |
| **Syntax and editor support (done)** | Parse safe closure expressions with optional nonempty capture lists, typed parameters, optional return annotations, and ordinary indented bodies; update traversal and editor inventories without admitting generic, unsafe, variadic, or declaration-position forms. | Snapshots preserve capture-kind and body spans, and malformed lists, modifiers, parameters, aliases, and body indentation receive focused diagnostics. |
| **Capture resolution (done)** | Assign stable identities to closure expressions and environment bindings, resolve every outer-local use through one explicit capture, and distinguish declarations that require no capture. | Missing, duplicate, self-initializing, shadowing, alias-collision, and inaccessible captures are deterministic errors with related spans. |
| **Capture typing and construction (done)** | Type-check value, shared-reference, mutable-reference, const-pointer, and mutable-pointer captures; require addressability and mutability where appropriate; record exact-once left-to-right construction and any `*var T` to `*T` downgrade. | Capture types and evaluation order are explicit in typed facts, direct raw-pointer captures cannot bypass `*`/`*var`, and no capture operation itself enters an unsafe context. |
| **Callable types and return inference (done)** | Give each closure expression a unique anonymous nominal type, add the ordinary user-implementable `Callable[Arguments, Return]` contract and call-syntax selection, infer omitted closure results, and preserve exact annotated results including `!`. | Distinct expressions remain distinct types, all return paths agree, fallthrough is unit, generic callable parameters infer concrete closure types, and erased calls retain exact argument and result types. |
| **Body, safety, and control checking (done)** | Check a closure as its own safe function boundary with immutable capture bindings, explicit unsafe blocks, ordinary return/error/defer rules, and no inherited unsafe or escaping control context. | Raw-pointer comparisons compile safely; automatic raw-pointer field access requires `unsafe:`, mutable access requires `*var`, and unsafe closure declarations and anonymous recursion are rejected. |
| **Copy, alias, and escape semantics (done, including 0.10 revision)** | Retain logical-copy recording and promotion analysis for anonymous environments, shallow-copy plain captures into one new environment, and preserve that environment identity when the resulting callable is copied. | Inline environment slots remain distinct, descriptor backing aliases, copied callables share their environment, explicit references/pointers preserve identity, and a raw pointer alone never roots its pointee. |
| **Typed and control-flow IR lowering (done)** | Represent closure construction, environment access, static callable invocation, erased callable dispatch, return flow, traps, and deferred cleanup without embedding syntax or name-resolution facts in later IR. | Construction and argument evaluation order are explicit, closure-local exits cannot target an outer body, and existing named-function lowering is unchanged. |
| **C99 environment and call emission (done)** | Emit deterministic private environment layouts and static body functions, pass an environment pointer on direct calls, and reuse ordinary trait-object vtables for erased calls while retaining GC-visible roots. | Capturing and captureless closures work on x86 and x86-64, generated C remains C99, symbols are deterministic, and no closure is emitted as a plain C function pointer. |
| **Cross-feature integration (0.10 shallow captures and thread publication done)** | Exercise closures with generics in enclosing declarations and higher-order APIs, traits, collections, managed and interior references, raw pointers, `Result`, `!`, `defer`, tests, and nested modules. | Shared-backing capture/copy and unrestricted cross-thread capture cases pass without weakening coherence, visibility, cleanup, trap behavior, or production/test reachability. |
| **Conformance and tooling closure (done)** | Add parser snapshots, compile-pass/fail cases, run-pass and trap tests, debug/release and x86/x86-64 coverage, generated-C assertions, documentation, editor synchronization, and macro-produced closure cases when macros are available. | The pre-closure suite remains green and every normative closure rule is mapped to deterministic evidence in `ledger.md`. |

Private evolving captured state, implicit or default capture, arbitrary
initialized captures, generic closure literals, unsafe closures, variadic
closures, recursive anonymous closures, callable equality or hashing,
`CallableMut`/`CallableOnce`, captureless conversion to `&fn` or `*fn`, and C
callback conversion are outside this milestone. A later proposal must define
their interaction with logical copying, erasure, cleanup, and concurrency
before adding any of them.

## 6. Native threads and synchronization

### Standard-library concurrency

> Status: Complete. The 0.9 structural-transfer baseline and its replacement by
> the 0.10 shallow shared-memory contract are both implemented and recorded.
> The historical bullets below explain the superseded baseline; current
> authority remains `spec.md` Section 10.4 and the migration table above.
>
> Depends on: **Explicit-capture closures** (complete).
>
> This milestone adds no thread, task, `concurrent`, `async`, or `await`
> grammar. Closures supply executable bodies, and ordinary declarations in
> `std.thread` and `std.sync` expose every concurrency operation.

**Historical 0.9 goal:** Add data-race-free native parallelism using independent
transfer copies and explicit synchronized identity. The bullets and completed
tasks below record the implemented baseline being replaced; they are not
authority for the 0.10 language semantics in `spec.md` Section 10.4.

The implemented 0.9 thread and transfer contract was:

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

The implemented 0.9 `std.sync` foundation was:

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
| **Normative concurrency contract (0.9 done; superseded by 0.10)** | Record the original thread, transfer, memory-ordering, channel, mutex, atomic, shutdown, trap, cleanup, GC, and callback rules. | Historical evidence remains available while the migration milestone owns the revised normative behavior. |
| **Structural transfer capability (done)** | Add canonical `Transfer` facts and generic bounds, structural derivation for ordinary values, conditional derivation for closures and standard handles, explicit exclusions for reference/pointer/trait-object aliases, and the accepted unsafe FFI opt-in. | Spawn inputs and results cannot hide an unapproved alias; diagnostics identify the exact nontransferable capture, field, element, or generic obligation. |
| **Standard concurrency declarations (done)** | Add the accepted `std.thread` and `std.sync` modules and source declarations for thread handles, spawn errors, channels and endpoint outcomes, mutexes, and atomic cells, keeping only representation and lowering hooks intrinsic. | All APIs resolve and type-check through ordinary module, generic, trait, visibility, and standard-library paths. |
| **Transfer-copy lowering (done)** | Lower cross-thread arguments, results, channel messages, and synchronized cell values through an explicit transfer-copy operation that recursively detaches ordinary backing storage while preserving approved synchronized handles. | Mutation on either side cannot observe shared ordinary storage, explicit synchronized identity remains shared, and evaluation occurs exactly once. |
| **Native thread lifecycle (done)** | Implement eager Linux native-thread creation, runtime identity, recoverable creation failure, synchronized result publication, copyable handles, single OS join, repeated logical-result copies, self-join detection, and shutdown waiting on x86 and x86-64. | Successful, failed, nested, multiply joined, handle-discarded, self-joined, and entry-return cases match the accepted lifecycle without implicit detach or cancellation. |
| **Thread body and failure integration (done)** | Lower spawned `Callable[(), R]` bodies as safe function boundaries with ordinary return, `Result`, `!`, trap, panic, and `defer` behavior; synchronize complete output calls without imposing an order. | Normal results and recoverable errors cross predictably, while a trap on any thread terminates the process and never becomes a join value. |
| **Channel implementation (done)** | Implement bounded and unbounded synchronized queues, rendezvous behavior, transfer-copy sends, blocking and nonblocking operations, explicit idempotent closure, draining, and copyable endpoint identity. | MPMC stress, full/empty/closed distinctions, close races, wakeups, ordering within one sender, and abandoned-handle behavior are deterministic where specified and race-free. |
| **Mutex implementation (done)** | Implement copyable `Mutex[T]` identity and copy-based `new`, `read`, `replace`, and callback-driven atomic `update` without exposing protected-storage references. | Concurrent updates do not lose changes, returned values are independent, callback traps remain process-fatal, recursive locking may deadlock, and no safe reference escapes. |
| **Sequentially consistent atomics (done)** | Implement shared `AtomicBool`, `AtomicI32`, and target-width `AtomicUsize` cells with the accepted load, store, exchange, compare-exchange, and integer read-modify-write operations without emitting C11 `_Atomic` into the C99 backend. | Operations are sequentially consistent on both targets, copies retain cell identity, runtime/compiler hooks preserve C99 output, and target-width behavior never assumes 64-bit atomics on x86. |
| **Collector and root integration (done)** | Register and unregister runtime-created threads, scan their stacks, queues, environments, synchronized handles, and unpublished/published results, and make shutdown cooperate with collection. | Stress collection cannot reclaim reachable cross-thread state, raw pointers acquire no rooting behavior, and completed thread state is reclaimable after all roots disappear. |
| **C callback boundary (done)** | Permit synchronous same-registered-thread reentry from C on the initializer or an Elamite-created thread while retaining the prohibition on foreign-created-thread and asynchronous foreign entry. | Nested registered callbacks preserve roots and traps never unwind through C; unsupported foreign-thread entry remains explicitly documented and tested where a harness can detect it. |
| **Concurrency conformance (done)** | Add compile-pass/fail transfer cases, runtime lifecycle and synchronization tests, trap-process tests, high-contention and repeated stress suites, sanitizer-capable native harnesses, debug/release coverage, and the Linux x86/x86-64 matrix. | The complete pre-concurrency suite remains green, safe suites show no races or hangs under their bounded contracts, and every normative concurrency rule is mapped in `ledger.md`. |

Cooperative tasks, executors, `async`/`await`, futures, detached execution,
cancellation, interruption, timeouts, thread-local storage, relaxed atomics,
guards exposing protected references, scoped reference transfer, parallel
iterators, scheduling or fairness guarantees, automatic deadlock detection,
and foreign-thread attachment are outside this milestone.

## 7. Language surface completion

### User-defined iteration

> Status: Complete. `spec.md` 7.1 defines the ordinary source-backed
> `Iterator[Element]` protocol. Static concrete and generic-bound selection,
> managed hidden state, shallow yielded values, diagnostics, both target
> widths, closure interaction, and cost evidence are implemented.
>
> Blocked by: **None**. Should precede **Standard-library expansion**,
> because every collection API added before it is either compiler-privileged
> or unusable with `for`.

**Goal:** Let an ordinary user type participate in `for` through a normal
trait while preserving the 0.10 shallow iterable copy, yielded-element copy,
and mutation-invalidation rules.

The accepted protocol uses `next(self: &var Self) -> Option[Element]`. It
shallow-copies the source iterator once into managed hidden state, so a yielded
safe reference can remain valid after the loop. Slices, arrays, `Vec`, `Map`,
and `Set` retain their direct lowering and established mutation-invalidation
rules; trait-object iteration and an implicit into-iterator conversion remain
outside the milestone.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Accepted iteration contract (done)** | Decide and record the protocol shape, shallow iterable copy, element copy behavior, mutation invalidation, and interaction with closures. | Section 7.1 owns one normative result; unspecified `Map`/`Set` order and existing direct collection rules are preserved. |
| **Protocol declaration and checking (done)** | Add the standard trait, its bounds, and the checking rules that admit a user type in loop-header position. | Concrete and generic-bound iterators are accepted; a nonconforming loop names the missing `std.Iterator[Element]` obligation. |
| **Loop lowering through the protocol (done)** | Lower `for` over a user type through the protocol while retaining the specified shallow-copy behavior and evaluation order. | The iterable evaluates exactly once, `next` controls every step, hidden-state references remain valid, and cleanup and `break`/`continue` edges are unchanged. |
| **Privileged collection reconciliation (done)** | Retain direct lowering for `Vec`, `Map`, `Set`, arrays, and slices and document the distinct user-iterator cost. | Existing collection output, ordering, invalidation, allocation counters, and performance characteristics remain unchanged. |

### Inherent implementation blocks

> Status: Complete. The compatibility review is closed: source syntax transitions
> directly to field-only structs, and `std.ast` advances from frozen 1.0 to an
> exact 2.0 interface with a distinct inherent-implementation value.
>
> Blocked by: **None**. This milestone should precede
> **Standard-library expansion**, so new nominal APIs are not added in syntax
> that is immediately due for migration. It does not block
> the completed **User-defined iteration** protocol, which uses trait
> implementations independently of this syntax change.

**Goal:** Separate nominal storage declarations from inherent behavior through
Rust-like `impl Type` blocks while preserving static lookup, coherent generic
applicability, exact nominal identity, layout, visibility, and copying
semantics.

The accepted language direction is:

- struct bodies become field-only, and inherent methods move to module-level
  `impl Type` blocks;
- implementation generic parameters and bounds are explicit, as in
  `impl[T: Display] Wrapper[T]`;
- an inherent block may add methods only, never fields or a type-dependent
  representation;
- multiple blocks may contribute methods when their target and bounds apply;
  fields and applicable inherent methods retain one member namespace;
- two blocks may reuse a method name only when their target sets are provably
  disjoint; an exact block never overrides an overlapping generic block; and
- this feature does not add implementation specialization. Trait-implementation
  overlap remains invalid, and inherent lookup continues to beat trait-method
  lookup.

Initially, an inherent implementation must be declared in the same module as
the outermost nominal target type. Every implementation parameter must be
constrained by that target, aliases are compared through their canonical
targets, `Self` denotes the complete target type, and method visibility remains
declared on each method. These restrictions avoid import-sensitive method sets,
unconstrained monomorphization, and downstream extension of an upstream type.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Normative and compatibility revision (done)** | Close I-1; revise Sections 4.2, 6, and 12; define the source migration; and publish the required `std.ast` interface transition without silently changing ABI 1.0. | The specification has one unambiguous inherent-implementation grammar and method-set rule; existing compile-time packages either remain compatible by an explicitly documented path or fail with an exact interface-version diagnostic. |
| **Syntax and identity boundary (done)** | Parse field-only structs and module-level `impl Type` blocks; collect stable block and member identities separately from trait implementations. | Inline inherent methods receive the migration diagnostic, fields in an inherent block are rejected, malformed targets recover, and clean versus generated syntax produces deterministic identities. |
| **Generic applicability and coherence (done)** | Canonicalize targets, bind every explicit implementation parameter, evaluate bounds, enforce local ownership, and reject field/method or potentially overlapping method-name collisions. | Generic, bounded, exact, disjoint, alias-equivalent, unconstrained, foreign-target, and overlapping cases have focused pass/fail coverage with diagnostics pointing to both declarations when relevant. |
| **Checking, selection, and lowering (done)** | Type-check methods under the block's substitutions, retain all five receiver forms and `Self`, select one applicable inherent member before trait lookup, and reuse ordinary monomorphization and C emission. | Static and bound calls preserve receiver adaptation, evaluation order, visibility, unsafe rules, function-reference behavior, deterministic symbols, and equivalent generated C on x86 and x86-64. |
| **Compile-time surface migration (done)** | Add the versioned inherent-implementation AST value and item/quote roles; migrate attributes that add methods and ensure derives retain their documented scheduling and observation rules. | Handwritten and generated blocks undergo identical parsing, resolution, coherence, safety, and provenance checks; version-skew, attribute sibling output, derive interaction, and recovery have directed and property coverage. |
| **Repository migration and conformance (done)** | Move shipped sources, examples, fixtures, and documentation to the accepted syntax and extend editor-grammar synchronization where its structural patterns change. | Formatting, check, test, clippy, both target matrices, the authoritative demonstration, compile-time compatibility fixtures, and documentation links agree with one implemented surface. |

### Deferred specified surface

> Status: Candidate work; each item is independently schedulable.
>
> Blocked by: **None**.

**Goal:** Close or permanently document the gaps where `spec.md` specifies a
construct that the implementation does not provide, so that no normative rule
remains silently unimplemented.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **128-bit integer lowering** | Implement `i128`/`u128` constants, arithmetic, conversion, and display in the C backend, or move the exclusion into `spec.md` as a permanent restriction. | `docs/toolchain.md` and the `int128_support.elx` regression fixture agree with the implementation, and the fixture's pinned phase behavior is updated with it. |
| **Wildcard and grouped imports** | Implement the unsupported `use` forms, or record their absence as a deliberate permanent restriction. | Accepted forms follow the ordinary Section 2.3 visibility, reachability, and duplicate-binding rules; rejected forms keep a diagnostic naming the restriction. |
| **Foreign ABI surface** | Decide whether C variadic functions and non-`C` foreign ABIs enter the language, and implement or permanently document the result. | The ABI-type rules in `spec.md` 10.1 and the foreign declaration checking agree with the shipped surface. |

## 8. Standard library

### Standard-library expansion

> Status: Complete as a public-surface and behavior milestone. The shipped
> modules provide the accepted filesystem, environment, process, time,
> ordering, text, and deterministic-randomness APIs. The follow-up
> **Source-hosted standard library** milestone owns the current overuse of
> native hooks and placeholder bodies without reopening these reviewed APIs.
>
> Blocked by: **None**. Nominal APIs use the completed **Inherent implementation
> blocks**, and iterable APIs use the completed **User-defined iteration** protocol.
> Every collection-shaped signature added here inherits the implemented 0.10
> alias and invalidation behavior.

**Goal:** Provide the ordinary capabilities a program needs to be useful,
without adding compiler privilege, and while treating every public signature
as a long-lived compatibility commitment.

Each module is a separate work package so that its API can be reviewed
independently. Prefer a small, complete surface over a broad, provisional one:
a missing function can be added later, while a published one cannot easily
change. Every module records its allocation and copying behavior in
`cost_model.md` as it lands.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Filesystem and path (done)** | Add reading, writing, metadata, directory traversal, and path manipulation behind `IoError`, with explicit resource handles and idempotent cleanup. | Handles follow the Section 8 cleanup contract, no operation exposes a reference into managed storage, and failure categories are exhaustive and portable. |
| **Process and environment (done)** | Add environment variable access, command-line arguments, exit status, and process invocation. | Values copy under ordinary semantics, the surface is target-portable, and no operation introduces implicit global mutable state visible across threads. |
| **Time and duration (done)** | Add monotonic and wall-clock reading and a duration type with checked arithmetic. | Monotonic and wall-clock sources are distinct types, arithmetic follows the ordinary overflow contract, and no operation is specified as a synchronization edge. |
| **Ordering and search utilities (done)** | Add sorting, binary search, and comparison helpers over slices and `Vec`. | Sorting is deterministic for equal keys or documented as unstable, comparison uses the Section 4.5 ordering rules, and no helper introduces a hidden allocation not recorded in `cost_model.md`. |
| **Text surface and behavior (done)** | Add string search, split, trim, case, and parsing operations with their reviewed allocation and Unicode contracts. | `str` and `String` results follow the specified materialization and shallow-backing rules, and Unicode behavior matches the Section 4.1 contract. |
| **Randomness (done)** | Add a seedable generator with an explicitly documented distribution and reproducibility contract. | Determinism under a fixed seed is guaranteed and tested; no operation silently seeds from the environment. |

### Source-hosted standard library

> Status: Complete. Public text algorithms, lexical path behavior, and portable
> host wrappers are ordinary Elamite. Exact bodyless intrinsic declarations now
> expose only representation, allocation, host, resource, synchronization, and
> caller-location capabilities that source cannot implement.
>
> Blocked by: **None**. It builds on the completed **Inherent implementation
> blocks** and **User-defined iteration** milestones and preserves the public
> behavior accepted by **Standard-library expansion**.

**Goal:** Implement standard-library policy and algorithms in Elamite whenever
they can be expressed safely, while reducing compiler/backend knowledge to the
smallest private kernel required for opaque representations, managed
allocation, traps, synchronization, and host operating-system interaction.

The native boundary is capability-based. A native primitive may expose one
operation that Elamite cannot implement because the representation is opaque
or the operation crosses into the runtime or host. It must not implement a
complete public algorithm merely because direct C is convenient. Public
standard-library functions remain ordinary source declarations and wrap those
private capabilities where a native boundary is unavoidable, except for
`panic`, typed traps, and test failures whose required caller location makes
the public call boundary itself intrinsic. No public API was added or changed.

For text, the private kernel must be sufficient to write search, splitting,
trimming, case conversion, and parsing in ordinary Elamite without exposing
raw pointers or permitting invalid UTF-8. It must provide the capabilities to
measure a text view, advance through one Unicode scalar while retaining byte
boundaries, construct a checked substring view, and efficiently materialize
owned text from validated scalars or pieces. The exact private declarations
are implementation details rather than stable user APIs.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Native-boundary inventory (done)** | Classify every compiler-known standard call and every bodyless or `pass`-bodied shipped function as an opaque-representation primitive, managed-runtime primitive, trap/synchronization primitive, host operation, or source-migration candidate. Record the reason for every retained native entry in one exact inventory. | Every current entry has one owner and reason; an inventory test fails when a native hook, `StandardCall`, or shipped placeholder is added without classification. Convenience and performance alone are not accepted reasons for native ownership. |
| **Explicit private intrinsic boundary (done)** | Replace executable `pass` placeholders with bodyless declarations accepted only for the exact compiler-owned standard inventory and unavailable to user packages. Public standard declarations call these capabilities through ordinary checked Elamite bodies unless caller-location semantics require the public boundary itself. | `stdlib/src/` contains no executable function whose `pass` body is silently replaced by the checker; copying or spelling an intrinsic declaration in a user package is rejected; missing intrinsic lowering is a compiler error rather than a generated abort or fallthrough. |
| **Text traversal and construction kernel (done)** | Add private, UTF-8-preserving primitives for text length, scalar advancement with byte boundaries, checked substring views, and amortized owned-text construction. Keep backing reachability, shallow `String` sharing, overflow behavior, and allocation accounting explicit. | Arbitrary valid UTF-8 can be traversed and sliced without unsafe code or invalid views; malformed boundaries are rejected; empty, ASCII, multibyte, maximum-scalar, and allocation-overflow cases have x86 and x86-64 coverage. No raw address or mutable text backing enters the public language surface. |
| **Elamite text algorithms (done)** | Reimplement `std.text` search, contains, borrowed and owned split, trim, case conversion, and boolean/integer parsing as ordinary Elamite functions over the private kernel. Remove their algorithm-level `TextOperation` call classification and generated C helpers. | Typed and control-flow IR contain ordinary monomorphized Elamite functions for every text algorithm; existing Unicode, error, aliasing, allocation, and result-materialization tests remain unchanged; generated C contains only the minimal text primitives, not native copies of the migrated algorithms. |
| **Wider source migration (done)** | Move lexical path manipulation, portable host wrappers, validation, and all other expressible standard-library policy into Elamite. Retain only actual file/process/environment/clock, resource-handle, collection-representation, trap, thread, synchronization, GC, formatter, and caller-location capabilities that require native access. | Every public standard function either has an ordinary checked Elamite body or is documented by the exact inventory as inseparable from a native capability. `Path` operations and other host-independent helpers lower as ordinary Elamite calls. Runtime and compiler tests prove that shadowing or copying a standard name grants no privilege. |
| **Cost, conformance, and cleanup (done)** | Remove superseded checker, IR, naming, and backend special cases; update the cost model and release evidence for changed allocation or copying; and compare the source-hosted implementation on both supported targets. | Formatting, check, test, and clippy pass; x86 and x86-64 generated-C and run-pass matrices agree; standard-library behavior is unchanged; comparable memory-cost baselines accompany material cost changes; an exact test proves that no source-migration candidate remains native. |

## 9. Toolchain and developer experience

### Source-level debugging

> Status: Planned. Because the compiler emits C, a native debugger currently
> shows generated identifiers and temporaries rather than Elamite source.
>
> Blocked by: **None**. Shares its span infrastructure with
> **Language server**.

**Goal:** Let a programmer debug an Elamite program in Elamite terms, using
ordinary native tooling.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Generated-source mapping** | Emit `#line` directives or an equivalent mapping so C-compiler diagnostics and debugger line tables refer to `.elx` locations. | A C-compiler warning and a debugger breakpoint both resolve to the originating Elamite line on both targets. |
| **Debuggable names and locals** | Preserve source-level function and binding names in the generated C so that stack frames and locals are recognizable. | A stack trace names Elamite functions, and deterministic-symbol and ABI-exclusion rules are unchanged. |
| **Debugging documentation** | Document the supported debugger workflow, including what remains visible as generated C. | A programmer can set a breakpoint, inspect a local, and step through a function following documented steps alone. |

### Language server

> Status: Candidate work; scope and protocol coverage require a decision
> before implementation. The shipped editor support is a TextMate grammar,
> which cannot resolve names.
>
> Blocked by: **Incremental queries** in **Post-conformance optimization**,
> which owns the stable query boundaries a responsive server needs.

**Goal:** Provide name resolution, diagnostics, and navigation over the real
compiler rather than a heuristic grammar, without a language-server dependency
entering the semantic layers.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Query-backed analysis surface** | Expose resolution, type, and diagnostic queries over an unsaved buffer through a stable library interface. | The interface is reusable by the CLI and the server, and no semantic layer gains an editor dependency. |
| **Core protocol support** | Implement diagnostics, hover, go-to-definition, and document symbols. | Results match ordinary `check` behavior for the same source, including compile-time expansion and macro provenance. |
| **Editing support** | Implement completion, find-references, rename, and formatter integration. | Rename respects the module, compile-time, and shadowing namespace rules; formatting reuses the existing formatter. |
| **Editor integration** | Ship the server with the existing extension and replace grammar-based classification where the server is available. | The extension degrades to grammar highlighting when the server is unavailable, and `tests/editor_grammar.rs` remains valid for that fallback. |

### API documentation generation

> Status: Planned. The `doc` command exists but currently emits a heading
> without rendered API content.
>
> Blocked by: **None**.

**Goal:** Generate readable, navigable reference documentation from
documentation comments and public signatures.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Rendered API model** | Render modules, types, traits, implementations, functions, and compile-time declarations with signatures, visibility, and documentation comments. | Output covers every public item a package exposes, and package-private items are excluded. |
| **Cross-linking and navigation** | Resolve documentation references to other items and emit a navigable module tree. | A reference to an item in the same package or a dependency resolves to its rendered location, and unresolved references are diagnosed rather than silently dropped. |
| **Output formats** | Emit at least one distributable format suitable for publishing, in addition to the existing text output. | Generated output is deterministic for identical input, target, and compiler revision. |
| **Documentation examples** | Decide whether examples in documentation comments are compiled or checked, and implement the accepted result. | Either examples are verified by the ordinary test layers, or their unverified status is documented explicitly. |

### Additional platform targets

> Status: Candidate work; target selection requires a decision. Supported
> targets are currently Linux x86 and x86-64.
>
> Blocked by: **None**. The C-toolchain portability tasks may proceed in
> parallel with target selection.

**Goal:** Widen the set of hosts and targets the compiler supports, without
weakening the deterministic-output or target-width guarantees.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **C-toolchain portability** | Run the full suite under a second C compiler, and review the driver's fixed flag set — including `-Werror` on generated code — for portability across compilers and versions. | The suite passes under both compilers, and any flag whose failure mode depends on the C compiler's version is documented or removed from shipped builds. |
| **64-bit ARM target** | Add an `aarch64` target with its pointer width, C flag selection, and layout behavior, and extend the conformance matrix to it. | The full matrix passes on 64-bit ARM, including the concurrency stress suites, whose weaker hardware memory model exercises the sequential-consistency contract that x86 cannot. |
| **Non-ELF platform support** | Decide the next operating-system target and implement its object format, linking, collector discovery, and prerequisite documentation. | Library and executable packages link on the new platform, and `docs/toolchain.md` records its prerequisites. |

## 10. Distribution and project infrastructure

### Package distribution

> Status: Candidate work; requires a design decision before implementation.
> Package dependencies are currently local paths only.
>
> Blocked by: **None**.

**Goal:** Let packages depend on packages they do not vendor, with
reproducible resolution.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Accepted distribution model** | Decide between a registry, a source resolver, or vendored-only distribution, including naming, versioning, and trust. | The decision records its compatibility, reproducibility, and security implications before any resolver is implemented. |
| **Version resolution and lockfile** | Implement the accepted resolver and a lockfile that pins an exact dependency graph. | A locked build is byte-reproducible for the same compiler revision and target, and resolution failure is diagnosed rather than silently resolved. |
| **Compile-time metadata compatibility** | Define how versioned compile-time metadata and the `std.ast` interface version interact with dependency resolution. | A dependency built against an incompatible interface version is rejected with a diagnostic naming both versions. |

### Distribution and installation

> Status: Planned. Cargo publishing is disabled and the only supported
> installation path is building the compiler from source.
>
> Blocked by: **Additional platform targets** for any platform it ships.

**Goal:** Let a programmer install and run the toolchain without building it.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Release artifacts** | Produce versioned toolchain archives per supported platform from a reproducible release process. | Artifacts report a compiler version and `spec.md` revision matching their source revision. |
| **Installation path** | Document and test installation, including the C-toolchain and collector prerequisites. | A programmer can install and compile a package following documented steps on a clean supported system. |
| **Release notes and changelog** | Add a maintained changelog covering language, library, and toolchain changes per release. | Every release records its behavior changes, and a cost-affecting change references its `cost_model.md` update. |

### Learning material

> Status: Planned. `spec.md` is a reference and is not a tutorial.
>
> Blocked by: **None**, though examples should follow **Standard-library
> expansion** to avoid teaching around missing capability.

**Goal:** Give a newcomer a path from installation to a working program that
does not require reading the specification.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Introductory guide** | Write a task-ordered guide covering installation, packages, values and references, errors and cleanup, generics and traits, and concurrency. | A programmer unfamiliar with the language can build a nontrivial program using the guide alone. |
| **Worked examples** | Maintain example programs that are compiled and run by the ordinary test layers. | Every example in published material is covered by an automated test, so documentation cannot drift from behavior. |
| **Migration and comparison notes** | Explain the shallow-copy, alias, pointer, and shared-memory model to readers arriving from Go, Rust, or C. | The notes state where intuition from each language transfers and where it does not, particularly for descriptor aliasing, data-race UB, and the absence of borrow checking. |

### Project governance and contribution

> Status: Planned. The repository has no contribution, conduct, security, or
> issue-template infrastructure.
>
> Blocked by: **None**. Should precede any public invitation to contribute.

**Goal:** Give outside participants a defined way to report, propose, and
contribute, and give maintainers a defined way to decide.

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **Contribution infrastructure** | Add contribution guidance, a code of conduct, a security reporting policy, and issue and pull-request templates. | A first-time contributor can find the build, test, and review expectations without reading the compiler sources. |
| **Design change process** | Document how a language change is proposed, reviewed, accepted, and recorded across `issues.md`, `proposals.md`, `spec.md`, and `ledger.md`. | An outside proposal has a defined entry point and a defined outcome, and the existing document ownership rules are preserved. |
| **Stability and versioning policy** | Define what a released language version guarantees, how a breaking change is introduced, and whether an edition or epoch mechanism is adopted. | The policy states its compatibility promise explicitly, and the specification revision reported by `elamc --version` is tied to it. |

## 11. Delivery checkpoints

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
- **Standard-library concurrency:** native threads and channels publish ordinary
  shallow values, retain process-wide trap and cleanup rules, and pass the GC,
  lifecycle, contention, and target conformance matrices.
- **Memory cost model:** the current shallow-copy, allocation, promotion,
  retention, and synchronization costs are documented separately from
  semantics and backed by reproducible release-mode counters and workloads.
- **Tuple destructuring and positional fields:** local tuple bindings and
  positional fields preserve exact tuple shape, logical-copy, place,
  reference, and evaluation-order semantics on both targets.

These checkpoints are reporting boundaries, not substitutes for the exit
criteria of the individual milestones.
