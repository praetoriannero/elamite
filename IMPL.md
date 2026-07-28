# Elamite Compiler Implementation Plan

> Status: Active — Milestones 0 through 16 complete, Milestone 17 next
>
> Next work package: M17.1 — foreign declaration validation
>
> Basis: `SPEC.md` version 0.4.0-draft and
> `examples/spec_demo.elx`

This document breaks the initial Elamite compiler into implementation
milestones. Each milestone carries its own status note; the header above
records how far the sequence has advanced. Elamite compiles to C, as required
by the specification.

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

Completed Milestones 0 through 13 retain concise implementation summaries
because they describe established layer boundaries rather than active backlog.
Starting with Milestone 14, remaining work is divided into ordered work packages
named `M<N>.<task>`.

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

## 3. Stage A: establish the language contract

### Milestone 0: specification migration and feature ledger

**Goal:** Resolve `I-017` by turning the current specification into an
implementable checklist before relying on legacy compiler behavior.

Implementation work:

- Create a feature ledger mapping each normative rule in `SPEC.md` to its
  syntax node, semantic pass, lowering rule, runtime dependency, and tests.
- Inventory the legacy grammar, compiler components, examples, and fixtures.
  Preserve useful test cases, but treat the current specification and
  `examples/spec_demo.elx` as authoritative when behavior differs.
- Record all compiler-known entities in one catalog: primitive types, numeric
  operations, `Option`, `Result`, collections, `Default`, comparison traits,
  `StableHash`, `Display`, `Identity`, `ForeignRoot`, and `CVoid`.
- Define the initial supported target assumptions, including pointer width,
  integer representations, C compiler requirements, Boehm GC availability,
  and how native libraries are supplied.
- Define command-level outcomes for at least checking a package, building a
  package, printing diagnostics, and selecting a target or output directory.
  Exact command spelling may remain a tooling decision.
- Mark concurrency syntax, task runtime behavior, and cross-thread callbacks as
  unsupported rather than leaving partial hooks in the initial compiler.

Exit criteria:

- Every construct in the authoritative demonstration has a ledger entry.
- Known disagreements between old implementation artifacts and the current
  specification are either removed from the planned behavior or recorded as a
  test migration task.
- `I-017` can be closed without claiming that the compiler is implemented.

## 4. Stage B: source, packages, and syntax

### Milestone 1: compiler driver and package graph

> Status: Complete for the initial local-path resolver.

**Goal:** Reliably identify the complete compilation unit before parsing
declaration bodies.

Implementation work:

- Load `elamite.toml` and validate package name, version, target kind,
  local path dependencies, selected root source file, native libraries, and
  link options.
- Resolve dependency paths relative to the depending manifest and use each
  canonical manifest-directory path as the initial package identity. Paths to
  the same canonical directory identify one instance; different directories
  remain distinct even when their displayed names and versions match.
- Discover `.elx` files beneath the package source directory in deterministic
  order. Convert relative file paths into `root`-based module paths and reject
  invalid path components.
- Detect cycles in the package dependency graph. Do not reject legal import
  cycles within one package.
- Preserve every resolved dependency edge from its manifest alias to the
  dependency's package identity.
- Introduce a source manager that owns decoded source text, file IDs, line
  indexes, and span-to-line/column conversion.
- Separate dependency resolution from compilation. The compiler pipeline
  consumes a resolved package graph; a future registry, Git, or lockfile-aware
  resolver can replace local-path resolution without changing semantic
  analysis.

Validation:

- Single-file executable and library packages.
- Nested file-backed modules.
- Custom root paths.
- Missing roots, malformed manifests, invalid module components, and cyclic
  package dependencies.
- Native-library and link-option manifest data.
- Resolved dependency aliases and transitive package edges.
- Two dependency instances with the same displayed package name but different
  identities.

### Milestone 2: lexer and indentation engine

> Status: Complete.

**Goal:** Convert source text into an exact, span-preserving token stream.

Implementation work:

- Recognize identifiers, keywords, punctuation, operators, numeric literals,
  ordinary strings, formatted strings, comments, and documentation comments.
- Track grouping delimiters so physical newlines inside `()`, `[]`, and `{}` do
  not end statements or affect the indentation stack.
- Emit logical newline, indent, and dedent events outside delimiters. Enforce
  spaces-only indentation, exactly four spaces per nested block, dedents to a
  previously established level, and EOF closure of open blocks.
- Implement statement continuations exactly four spaces beyond the statement's
  starting indentation without confusing a body-opening colon for a
  continuation.
- Preserve documentation comments for later attachment while discarding
  ordinary comments semantically.
- Validate literal spelling independently from type checking: digit separators,
  bases, suffix shapes, escapes, formatting braces, and malformed delimiters.
- Recover at a physical or logical line boundary where practical so one lexical
  error does not hide every later error.

Validation:

- Golden token streams for nested bodies, blank/comment-only lines,
  continuation lines, grouped multiline expressions, and EOF dedents.
- Negative tests for tabs, unexpected indentation, bad dedents, unterminated
  strings, mismatched delimiters, and malformed formatted-string braces.
- Property tests that indent/dedent streams are balanced for accepted files.

### Milestone 3: complete surface parser

> Status: Complete.

**Goal:** Parse the full initial surface language before implementing all of its
semantics.

Implementation work:

- Define syntax nodes for modules, imports and re-exports, aliases, structs,
  enums, traits, impls, functions, foreign blocks, fields, variants, generic
  parameters, derive lists, and documentation.
- Define type syntax for primitives, paths, generic applications, tuples,
  arrays, slices, safe references, raw pointers, function references, unsafe
  function references, foreign function pointers, and trait objects.
- Define statements for bindings, assignment, expression statements, control
  flow, `return`, `break`, `continue`, `pass`, `unsafe`, single-call
  `defer call`, and block-form `defer:`.
- Implement expression precedence and associativity exactly as specified,
  including postfix calls, fields, indexing, `?`, unary operators, `as`, and
  assignment separation.
- Parse struct and enum literals, tuples, arrays, the three compiler-known
  collection macro forms, and formatted-string interpolation.
- Parse all match pattern forms, alternatives, guards, field shorthand, and
  rest markers.
- Keep parser actions structural. Do not resolve names, infer types, enforce
  visibility, check receiver legality, or decide whether a call is safe in the
  parser.
- Attach documentation comments to the following declaration and retain all
  source spans.

Validation:

- The entire authoritative demonstration parses.
- Every syntax form in `SPEC.md` has an isolated parse test.
- Invalid same-line bodies, brace bodies, empty bodies, chained comparisons,
  malformed generic lists, and malformed patterns fail at useful spans.
- A syntax-tree dump is stable enough to serve as a debugging tool.

## 5. Stage C: names and foundational static semantics

### Milestone 4: declaration collection, imports, and visibility

> Status: Complete.

**Goal:** Resolve every name to a stable identity without depending on source
order.

Implementation work:

- Collect module-level declarations for all files and inline modules before
  resolving imports or bodies.
- Reject an inline module whose complete path is already defined by a
  file-backed module. This check occurs here because file-backed paths are
  discovered in Milestone 1 while inline paths do not exist until parsing and
  declaration collection.
- Maintain the shared module-item namespace and reject duplicate declarations
  or imports even if duplicate imports identify the same target.
- Resolve absolute package/dependency paths and `root`, `self`, and `super`
  paths. Resolve `std` as the ordinary standard-package name after lexical,
  module, import, and dependency-alias lookup, so a user declaration may shadow
  it. Keep lexical lookup separate from module-path lookup.
- Implement imports, aliases, public re-exports, externally reachable module
  paths, and package-private versus public access.
- Build lexical scopes for parameters, local bindings, pattern bindings, and
  nested bodies. Permit local bindings to shadow module items.
- Predeclare named functions and types so direct and mutual recursion work.
- Check that public signatures mention only externally accessible types,
  traits, aliases, and bounds.
- Retain provenance for imported names so diagnostics can show both the use and
  the declaration or re-export that supplied it.

Validation:

- Circular imports within one package.
- Re-export chains and inaccessible public declarations hidden behind an
  unre-exported module.
- Inline/file-backed module collisions, duplicate namespace entries, illegal
  `super`, private access across packages, and private types in public
  signatures.
- Direct and mutual function recursion independent of declaration order.

### Milestone 5: canonical type system and inference core

> Status: Complete.

**Goal:** Give later passes one exact representation of every type and generic
substitution.

Implementation work:

- Intern or otherwise canonicalize primitive, nominal, tuple, array, slice,
  reference, raw-pointer, function, trait-object, and foreign types.
- Key nominal types by package identity, declaration identity, and canonical
  type arguments.
- Expand transparent aliases for equivalence and cycle checking while retaining
  alias names for diagnostics.
- Represent inference variables, generic parameters, substitutions, expected
  types, and an explicit error type.
- Implement exact type equality. Do not introduce implicit numeric conversion,
  subtyping, reference variance, or function variance.
- Materialize numeric and string literals from expected types, followed by the
  specified defaults when no expected type exists.
- Make safety, variadic markers, receiver types, and C ABI markers part of a
  function type's identity.
- Provide reusable queries for size/layout availability, addressability,
  mutability, trait obligations, ABI safety, and whether a type contains an
  explicit alias or managed reference.

Validation:

- Exact equality across aliases and generic applications.
- Distinct nominal types from different package instances.
- Literal defaulting and contextual materialization at numeric boundaries.
- Rejection of accidental variance or implicit concrete numeric conversion.

### Milestone 6: core expression and function checking

> Status: Complete for plain (non-generic, non-method, non-trait) module-level
> named functions. Bound-method and receiver checking is Milestone 11,
> generic instantiation is Milestone 12, and trait-bound dispatch (including
> trait-object conversion and `PartialEq`/`PartialOrd`/`Display` obligations) is
> Milestone 13; expressions in those areas are still walked for nested
> diagnostics but are not misdiagnosed. Pattern and `for`-binding typing and
> reachable-path return analysis remain Milestone 7.

**Goal:** Type-check a useful non-generic, non-trait subset containing named
functions and ordinary value operations.

Implementation work:

- Check function signatures, parameters, direct named calls, arity, explicit
  returns, and unit-return fallthrough.
- Classify each expression as a value, addressable place, mutable place, or
  collection-interior place. Carry that classification into typed IR.
- Check `let`, `var`, plain assignment, compound assignment, field selection,
  indexing, struct construction, tuple construction, arrays, and enum
  construction.
- Enforce struct field ordering rules in declarations and exact-once field
  initialization in literals.
- Check primitive operators, boolean-only conditions, comparison capability,
  explicit numeric casts, literal range, array length constants, and statically
  evident numeric traps.
- Reject recursive struct/enum containment unless every cycle crosses an
  explicit safe reference or raw pointer. Treat aliases and value containers as
  transparent for this check.
- Record explicit copy operations for bindings, arguments, returns, aggregate
  construction, and value-context field or index access.
- Check standard assignment restrictions: immutable binding paths cannot be
  mutated, while explicit mutable aliases stored inside immutable values retain
  their mutation capability.

Validation:

- Compile-pass and compile-fail suites for every primitive operator and cast.
- Struct, tuple, array, and enum construction with exact diagnostic spans.
- Recursive type graphs through direct fields, aliases, `Option`, collections,
  safe references, and raw pointers.
- Independent source use after assignment, argument passing, and return.

### Milestone 7: control flow, patterns, and flow analysis

> Status: Complete for the same scope as Milestone 6 (plain module-level
> named functions; see its status note), and implemented together with it in
> `src/check.rs` in one pass rather than as a second tree-walk — see that
> module's doc comment for why. Reachable-path return analysis does not
> specially recognize `while true:`/`for` as always executing, so a loop
> body alone never satisfies a non-unit function's return requirement. Match
> exhaustiveness and redundancy are reasoned about precisely for `bool` and
> `enum` scrutinees and for any unconditionally irrefutable pattern
> (`_`/a binding, or a tuple/struct built entirely from those); every other
> shape (a tuple/struct with a refutable field, or a literal of an unbounded
> domain) conservatively requires an explicit catch-all arm rather than
> risking a false "exhaustive" claim. Nested field-value refutability inside
> an already-matched enum variant is not tracked. A bare, unqualified
> identifier pattern is always treated as a binding, never as an
> unqualified unit-variant reference, matching every pattern example in
> `SPEC.md` and `examples/spec_demo.elx`, which always qualify variant
> patterns as `Enum.Variant`. Explicit `*pattern` dereference is checked for
> safe-reference scrutinees and participates in usefulness/exhaustiveness
> analysis over the referenced value.

**Goal:** Make all safe control flow statically well-formed before lowering it.

Implementation work:

- Check the structured control-flow semantics of `if`, `else`, `while`, `for`,
  `match`, `return`, `break`, `continue`, and short-circuit operators.
  Record selected binding types and copy requirements for consumption by
  lowering, while checking expressions in source order.
  Postfix `?` may remain semantically disabled until Milestone 15.
- Enforce valid placement of `break` and `continue`, boolean conditions, and
  explicit return values on every reachable path of a non-unit function.
- Type-check patterns and create immutable copied bindings for their payloads.
- Implement pattern usefulness, source-order reachability, and exhaustiveness.
  Guarded arms do not contribute to exhaustiveness.
- Enforce identical binding names and types across alternative patterns.
- Enforce the structural placement rules for deferred blocks: no `return`,
  `break`, `continue`, postfix `?`, nested `defer`, or `unsafe:` inside
  `defer:`, and no `defer` inside `unsafe:`.
- Preserve the type, place, copy, and source-span metadata needed for
  Milestone 8 to record left-to-right evaluation explicitly.

Validation:

- Exhaustive and non-exhaustive matches over booleans, enums, tuples, structs,
  literals, and infinite domains.
- Unreachable arms, guards, alternative bindings, and explicit reference
  dereference before content matching.
- Return-path analysis for nested branches and loops.
- Exactly-once destination evaluation for compound assignment.

## 6. Stage D: first executable compiler

### Milestone 8: typed IR, control-flow IR, and C backend skeleton

> Status: Complete for the plain, non-generic module-level function scope
> established by Milestones 6 and 7. `src/ir.rs` owns the typed high-level and
> control-flow representations, `src/backend.rs` emits deterministic C99, and
> `src/driver.rs` owns C compiler invocation and artifacts. The backend
> deliberately diagnoses constructs outside this executable skeleton—managed
> references and collections, methods and traits, generics, `for`, `match`,
> postfix `?`, and `defer`—instead of silently miscompiling them. `match`
> execution is completed with pattern-copy lowering in Milestone 9, while
> collection-backed `for` execution remains Milestone 14. The generated runtime
> support is isolated from language lowering so Milestones 10 and 17 can
> introduce the previously selected Boehm collector behind a collector-neutral
> managed-memory interface.

**Goal:** Compile and run a small, safe, non-generic Elamite program end to end.

Implementation work:

- Lower resolved syntax into typed high-level IR with all selected declarations,
  types, places, copies, casts, and evaluation order made explicit.
- Lower typed bodies into control-flow IR made of basic blocks, explicit
  temporaries, branches, calls, returns, and trap operations.
- Keep temporaries live through their full expression so later GC and cleanup
  lowering can preserve roots correctly.
- Define an internal calling convention for generated Elamite functions. It is
  separate from the public C ABI and may evolve until explicitly stabilized.
- Define deterministic symbol mangling that includes package and declaration
  identity and can later include concrete generic arguments.
- Choose C representations for primitives, unit, tuples, fixed arrays, and the
  initial non-recursive structs.
- Emit strictly sequenced C statements and temporaries. Never translate a
  multi-part Elamite expression directly into C when C leaves operand or
  argument order unspecified.
- Emit checked helpers for integer arithmetic, division, shifts, indexing, and
  numeric conversions, including compile-time target width for `isize` and
  `usize`.
- Generate C translation units, invoke the selected C compiler, pass link
  inputs, and report C compiler or linker failures as toolchain diagnostics.
- Provide a runtime entry shim for executable packages and an artifact form for
  library packages.

Exit criteria:

- A package containing primitive calculations, local bindings, direct function
  calls, branches, loops, and output can be built and run.
- Runtime traps have stable categories and useful source locations.
- Evaluation-order tests continue to pass at multiple C optimization levels.
- Generated C can be retained and inspected with a diagnostic compiler option.

### Milestone 9: complete logical value-copy lowering

> Status: Complete for every representation available through Milestone 9.
> `src/ir.rs` classifies canonical types behind one logical-copy contract, and
> the C backend emits one deterministic copy helper per used canonical type.
> Tuples, fixed arrays, structs, and explicit-discriminant enums copy
> recursively; mutable `String` buffers copy eagerly; scalar values copy
> directly; and explicit reference-like types are classified to preserve
> identity when their executable representations arrive. Source-ordered
> `match` lowering covers literals, alternatives, guards, tuples, structs, and
> unit/tuple/record enum variants with independently copied bindings.
> Safe-reference dereference execution remains Milestone 10, while `Vec`,
> `Map`, and `Set` acquire runtime representations and copy hooks in Milestone
> 14 rather than being partially represented here.

**Goal:** Make independent value copying a backend invariant rather than a
front-end assumption.

Implementation work:

- Define generated or runtime copy operations for every value representation.
  Ordinary nested values copy recursively; explicit references, raw pointers,
  function references, and trait-object references retain identity. A resource
  handle shares state only when its representation contains an explicit alias
  or another compiler-known identity-bearing value; implementing a trait never
  changes copy behavior.
- Initially use eager deep copies for mutable strings and collections if that
  reduces implementation risk. Add copy-on-write only behind the same copy
  interface and only after independence tests exist.
- Apply copy operations consistently to assignment, argument passing, return,
  pattern binding, collection reads, aggregate reads, hidden loop state, and
  `Result`/`Option` payload extraction.
- Complete `match` control-flow lowering together with its source-ordered
  pattern tests and independently copied payload bindings.
- Ensure a compound assignment mutates only its selected destination and that a
  copied aggregate cannot observe ordinary nested mutation in another copy.
- Define equality separately from copying. Reference-like values compare
  identity even though ordinary aggregates compare structurally when the
  required traits are available.
- Avoid a destructor-based lowering model. Managed values and ordinary copies
  do not require implicit user-visible destruction.

Validation:

- Nested structs, arrays, strings, and collections remain independent after
  copying.
- Aggregates containing explicit references preserve those aliases while their
  other fields remain independent.
- Argument and return copies behave the same as assignment copies.
- Optimization of copy operations does not change observable behavior.

### Milestone 10: safe references, storage promotion, and Boehm GC

> Status: Complete for the non-generic, non-method, non-trait scope this stage
> covers. `&T` and `&var T` lower to `T *` with flat struct layouts;
> `src/promotion.rs` promotes every address-taken local to a managed cell;
> referenced composite literals allocate their own cell; dereference is an
> assignable place; and field access through a reference dereferences
> automatically. Boehm is engaged only when a program actually needs managed
> storage, with `GC_set_all_interior_pointers(1)` requested before `GC_INIT`
> so a reference into an aggregate keeps its container reachable. Root
> retention relies on Boehm's conservative stack and register scan rather than
> explicit registration; see `LEDGER.md` §19.3 for why no keep-alive barriers
> are emitted yet. Cycle collection is untested pending recursive `Option`
> payloads, and precise escape analysis stays a Milestone 20 optimization.

**Goal:** Implement non-null safe references and managed reachability without
source lifetime parameters.

Implementation work:

- Model a reference as naming storage, so every assignment that overwrites that
  storage is observable through the reference. This covers a reference formed
  from a binding and a reference formed through a path into an aggregate
  alike; replacing a container is observable through a reference into it, and
  mutation through such a reference is visible in the container.
- Keep `&T` and `&var T` in one C representation (`T *`) with flat struct
  layouts, and trace interior pointers in the collector. See `LEDGER.md` §19.
- Allow references only to addressable places, with referenced composite
  literals as the explicit exception. Reject references to calls, computed
  expressions, and every collection interior.
- Enforce `&var` formation from mutable places while allowing any number of
  shared and mutable aliases; do not introduce borrow exclusivity.
- Promote escaping binding cells and selected subvalues to managed storage.
  A correct first implementation may conservatively promote every local whose
  address is taken; precise escape analysis is a later optimization.
- Route initialization, allocation classes, explicit collection, root
  registration, keep-alive barriers, and native link inputs through the
  collector-neutral `ManagedMemoryStrategy` backend interface introduced in
  Milestone 8. Boehm is the default strategy; alternatives must preserve the
  same non-moving, cycle-reclaiming language semantics.
- Integrate non-moving Boehm-managed allocation and preserve strong roots for
  bindings, parameters, temporaries, safe references, managed handles, and
  hidden loop state for their specified lifetimes.
- Ensure raw pointers are not intentionally registered as language roots.
- Add keep-alive barriers or equivalent backend mechanisms where optimization
  by the C compiler could otherwise shorten the observable lifetime of a strong
  language path.
- Make OOM attempt a full collection and then terminate without running deferred
  cleanup.

Validation:

- References returned from functions and references to composite literals.
- A binding reference observes reassignment; a nested-field reference observes
  replacement of its container, and mutation through it is visible in that
  container.
- A reference into an aggregate keeps its whole container reachable.
- Mutably aliased sequential writes have the specified last-write behavior.
- Collection interiors cannot form safe references.
- Cyclic managed structures are collectible in best-effort runtime tests
  without making collection timing a conformance requirement.

## 7. Stage E: the callable and abstraction model

### Milestone 11: methods and function references

> Status: Complete for non-generic inherent methods and named function
> references. All five receiver forms are checked and lowered; bound calls
> perform only the specified receiver adaptation; associated and unbound
> selection, field-first indirect calls, safe/unsafe signature identity,
> function-reference equality, and homogeneous variadic slice packing reach
> executable C99. Generic selection is completed in Milestone 12; trait method
> lookup remains Milestone 13, collection storage remains Milestone 14, and
> unsafe invocation-context enforcement remains Milestone 16.

**Goal:** Complete callable resolution without introducing closures.

Implementation work:

- Check inherent methods and the five legal receiver forms: `Self`, `&Self`,
  `&var Self`, `*Self`, and `*var Self`.
- Implement bound receiver adaptation only. Value receivers copy; safe-reference
  receivers may be automatically borrowed under the mutability and
  addressability rules; matching safe references pass directly.
- Require an exact raw-pointer receiver type. Do not implicitly borrow, cast,
  downgrade, dereference, or validate a raw pointer during method lookup.
- Implement associated function selection and unbound method selection from a
  type. Reject bound-method values selected from an instance.
- Represent safe and unsafe function references as exact, distinct types.
  Function references contain only a stable named-function identity; there are
  no closures, anonymous function literals, captured environments, or mutable
  function references.
- Resolve postfix member-call syntax field first. If the field exists, call its
  function-reference value normally and perform no receiver adaptation;
  otherwise perform bound-method lookup.
- Implement homogeneous variadic parameters by packing trailing arguments into
  the language slice representation, not the C variadic ABI.
- Implement function-reference identity equality and generic function-reference
  inference once generics are available.

Validation:

- All receiver forms, including calls on computed value receivers.
- Mutable receiver failures from immutable or non-addressable values.
- Function-reference storage in fields, enum payloads, collections, parameters,
  and returns.
- Safe/unsafe signature mismatch, arity, variadic marker, and exact type checks.
- Explicit proof that no syntax path constructs a closure or bound-method value.

### Milestone 12: generics and monomorphization

> Status: Complete. Generic bodies are checked once against declared
> comparison bounds; calls, function references, structs, enums, and inherent
> methods use unique all-or-nothing inference; concrete signatures and nominal
> layouts are cached; typed IR discovers reachable instances to a fixed point;
> unbounded structural growth is rejected; and concrete type arguments are
> retained in backend identities.

**Goal:** Type-check generic code once and emit a finite set of concrete
instantiations.

Implementation work:

- Check generic bodies against declared bounds rather than against accidental
  capabilities of call-site types.
- Infer all generic arguments from ordinary arguments and expected result types
  only when the solution is unique. Require a complete explicit argument list
  otherwise.
- Apply the same all-or-nothing inference rule to generic struct and enum
  literals.
- Instantiate canonical concrete types and functions on demand. Cache work by
  declaration identity plus canonical type arguments.
- Discover reachable instantiations to a fixed point before final C emission.
- Detect unbounded recursive expansion such as repeatedly nesting `Vec` in a
  recursive generic call, while permitting finite mutually recursive
  instantiation sets.
- Include concrete type arguments in symbol identity and generated helper
  identity.
- Keep the monomorphization boundary independent from the C emitter so another
  backend would see the same concrete program.

Validation:

- Explicit and inferred calls, expected-result inference, ambiguity, and partial
  explicit argument rejection.
- Generic recursive types that use explicit indirection.
- Finite direct and mutual recursive instantiations.
- Deterministic rejection of unbounded instantiation growth.

### Milestone 13: traits, derivation, and dynamic dispatch

> Status: Complete. `src/traits.rs` validates exact implementation
> conformance, orphan ownership, overlapping concrete or generic
> implementations, generic implementation bounds, derivation lists, and object
> safety. Bound calls prefer inherent members, report trait ambiguity, enforce
> generic capabilities at instantiation, and specialize implementation and
> default bodies. `Type.Trait.method` performs unconditional unbound selection,
> including for generic implementations.
>
> `&Trait` is a fat reference — target pointer plus vtable pointer — and is the
> one reference whose C type is not `T *`. Each trait emits a method-pointer
> struct; each implementing type emits a static table whose slots are filled by
> `void *`-receiver thunks, so no function pointer is cast between incompatible
> types. Slots are ordered by method name for deterministic layout. Default
> methods participate and may be overridden. An exact expected trait-object
> reference context automatically converts a concrete safe reference of
> matching mutability when its target implements the object-safe trait;
> explicit `as &Trait` conversion remains available.
>
> Compiler-supported derivations are conditional on their instantiated field
> capabilities. `Default` is synthesized fieldwise; equality and ordering use
> structural C99 helpers, with declaration-order enum comparison and IEEE
> unordered propagation; `Eq`, `Ord`, and `Hash` capabilities are checked
> structurally. `StableHash` is inferred only from stable compiler-known leaves
> and compiler-derived `Eq` plus `Hash`, never from ordinary manual impls.
> Collection hashing and the heterogeneous `Vec[&Trait]` validation remain
> Milestone 14 consumers of these completed Milestone 13 facilities.

**Goal:** Implement static trait selection, coherence, compiler capabilities,
and contextual trait-object conversion and dispatch.

Implementation work:

- Collect required methods and default bodies, and verify that each impl has
  exact signatures, no missing required methods, and no extra methods.
- Enforce the orphan rule using package identity and reject overlapping concrete
  or generic impls. Do not add specialization or negative impls.
- Resolve bound calls with inherent methods preferred over in-scope traits and
  report ambiguity between otherwise matching traits.
- Implement `Type.Trait.method` as unconditional, unbound selection of the named
  impl member, bypassing fields, inherent methods, and bound trait lookup.
- Support static dispatch for concrete calls and generic bounds.
- Validate object safety, construct vtables for concrete implementations, and
  represent `&Trait` and `&var Trait` as a managed target reference plus
  vtable identity.
- Permit mutability-preserving coercion from a concrete safe reference to a
  matching trait-object reference. Do not support bare objects, raw object
  pointers, downcasting, multi-trait objects, or runtime type inspection.
- Implement compiler-supported derivations for `Default`, comparison traits,
  and hashing using conditional component obligations.
- Infer the compiler-controlled `StableHash` capability structurally. Keep it
  unavailable to manual impls except for specified compiler-known wrappers.

Validation:

- Default methods, overrides, missing methods, signature mismatch, orphan-rule
  failures, and overlapping generic impls.
- Inherent/trait ambiguity, field shadowing, and fully qualified selection.
- Static versus vtable dispatch with several concrete types in one
  `Vec[&Trait]`.
- Every object-safety restriction.
- Structural derivation and conditional generic obligations.

## 8. Stage F: standard values, errors, and cleanup

### Milestone 14: strings, collections, iteration, and formatting

> Status: Complete.
>
> Boundary: generic enums and structural `StableHash` capability inference
> already exist. This milestone makes the standard `Option[T]` and
> `Result[T, E]` declarations executable as ordinary generic enum values before
> APIs that return them. Milestone 15 still owns the compiler-recognized
> `Result` propagation role, postfix `?`, and its interaction with cleanup.
>
> M14.1 resolved how a standard type that the specification writes as ordinary
> Elamite source becomes executable. `Option[T]` is no longer a name-only
> builtin: `src/resolution.rs` lexes, parses, and collects a compiler-supplied
> source unit into the `std` root module, so `Option` carries real declaration,
> variant, field, and generic-parameter identities and reaches typing,
> monomorphization, and the C backend through the unmodified generic-enum path.
> Construction, inference, matching, exhaustiveness, payload copying, and
> recursive `Option[&T]` graphs therefore needed no `Option`-specific code, and
> the builtin-`Option` pattern and coverage special cases were removed.
>
> Exactly one intrinsic remains, and it is the one `SPEC.md` 4.3 states:
> `Option[T]` defaults to `Option.None` without a `T: Default` obligation.
> `crate::traits::intrinsic_derivation` names it, keyed on the standard
> declaration's identity rather than the spelling, so a user enum spelled
> `Option` shadows the prelude name and receives nothing. Because an enum's C
> discriminant is the variant's identity and not its ordinal, the backend's
> default helper writes the `None` tag explicitly instead of relying on
> zero-initialization.
>
> M14.2 gave `Result[T, E]` and M14.3 gave `NumericError` the same treatment.
> `Result`'s compiler-known role is postfix `?` propagation, which is control
> flow rather than a trait, so it carries no intrinsic capability at all; only
> the *standard* `Result` propagates, and Milestone 15.1 still owns that role.
> These declarations are not a `std` *package*; Milestone 18.2 still owns that,
> and M18.1's intrinsic-inventory audit should confirm the list has not grown.
>
> M14.3 and M14.4 added the numeric associated functions and methods. A
> primitive has no declaration, so `Target.try_from`/`wrapping_from`/
> `saturating_from` and the `checked_`/`wrapping_`/`saturating_` operator
> alternatives are recognized in `src/check.rs` beside nominal member
> selection rather than through it. Each alternative is emitted as an overflow
> *predicate* plus a wrapping result, so `checked_X` is exactly
> `ovf_X ? None : Some(wrap_X)` and the predicate mirrors the trapping
> helper's own condition — the two cannot disagree about which operations
> overflow. Wrapping arithmetic goes through the unsigned counterpart type,
> avoiding C's signed-overflow undefined behavior. `wrapping_div`/
> `wrapping_rem` still trap on a zero divisor, which has no wrapped answer;
> every other alternative leaves the trapping path entirely. Division,
> remainder, and shifts have no saturating form, because their failures have
> no nearest representable answer.
>
> A numeric primitive's associated-function surface is now complete, so an
> unrecognized member on one is reported. `str` and `String` deliberately are
> not: their surfaces arrive in M14.5 and M14.6, so an unknown member there
> still falls through silently until those packages close it.
>
> One pre-existing defect became visible here and is deliberately left where it
> belongs: selecting an unknown enum member (`Option.Nope`, `Choice.Nope`)
> yields the error type with no diagnostic. It is common to every enum, not
> specific to `Option`, and belongs to enum member selection in `src/check.rs`;
> Milestone 19.9's diagnostic review owns closing it.

**Goal:** Supply the compiler-known standard value types needed by ordinary
programs and the authoritative demonstration.

Implementation tasks, in order:

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **M14.1 — Executable `Option`** *(complete)* | Lower `Option.Some`/`Option.None` through the existing generic-enum path; support construction, matching, payload copying, and `Default` to `None` without a `T: Default` obligation. | Run-pass construction/match/default tests, including `Option[&T]`; recursive `Option` graphs exercise the existing GC path. |
| **M14.2 — Ordinary `Result` values** *(complete)* | Make standard `Result.Ok`/`Result.Err` construction, matching, and copying use the existing generic-enum path without giving `Result` its postfix `?` role yet. | Generic `Ok`/`Err` construction and match tests pass while `?` remains rejected. |
| **M14.3 — Checked numeric conversions** *(complete)* | Add `NumericError` and the specified `Type.try_from` methods on concrete numeric types, reusing the exact M6/M8 conversion boundaries. | Successful/failing conversions cover every source/target family and both pointer widths without trapping. |
| **M14.4 — Numeric arithmetic alternatives** *(complete)* | Add the specified checked arithmetic methods returning `Option[T]` and wrapping/saturating arithmetic and conversions. Reuse M8 helpers rather than duplicating arithmetic rules. | Integer-width boundary tests prove checked failure returns `None` and wrapping/saturating operations never take the trapping path. |
| **M14.5 — Immutable `str`** *(complete)* | Finish immutable UTF-8 storage, literal default/contextual materialization, default/equality/order/hash/`StableHash`, and immutable copy behavior. | Unicode and escape run-pass tests, exact codepoint comparison, stable hashing, and rejected content mutation. |
| **M14.6 — Mutable `String`** *(complete)* | Finish mutable UTF-8 storage, `String.from(str)`, independent logical copying, and default/equality/order/hash. Do not provide `StableHash` or an implicit conversion from an existing `str` value. | Copy-independence, explicit-conversion, comparison, and `StableHash` rejection tests. |
| **M14.7 — Collection runtime boundary** *(complete)* | Define target-independent layouts and helper interfaces for `Vec`, `Map`, and `Set`; integrate allocation, empty defaults, and recursive copy hooks with `ManagedMemoryStrategy`; invoke no user cleanup. | Default/empty construction and nested-copy tests for all three collections with deterministic generated C. |
| **M14.8 — Array APIs** *(complete)* | Complete fixed-array `len` and `get`, exact `usize` indexing, static and runtime bounds behavior, and conditional `StableHash`. Arrays remain fixed-size ordinary aggregates. | Length/get/index tests, statically known and runtime OOB cases, and conditional stable hashing. |
| **M14.9 — `Vec` APIs** *(complete)* | Implement `Vec.new`, `len`, `is_empty`, `get`, `append`, `insert`, `remove`, and `clear`; add conditional lexicographic equality, ordering, and hashing. Do not introduce a `Vector` alias. | API-by-API run-pass tests, insert/remove trap boundaries, copied return values, comparisons, and rejection of `Vector`. |
| **M14.10 — Runtime stable-key hashing** *(complete)* | Connect M13 `StableHash` capability proofs to matching equality/hash helpers used by hashed collections. Reject floats, mutable values, ordinary safe references, and manually claimed stability without promising map/set iteration order or cross-process hash values. | Equal stable values hash equally within a collection; each forbidden key category has a compile-fail test. |
| **M14.11 — `Map` APIs** *(complete)* | Implement `Map.new`, `len`, `is_empty`, `contains_key`, `get`, `insert`, `remove`, `clear`, and trapping value indexing. Keys and values copy normally; conditional equality/hash ignore iteration order, and maps have no relational order. | Replacement/removal results, absent-key behavior, copy independence, stable keys, and order-independent equality/hash. |
| **M14.12 — `Set` APIs** *(complete)* | Implement `Set.new`, `len`, `is_empty`, `contains`, `insert`, `remove`, and `clear`, plus conditional order-independent equality/hash. Sets have no indexing or relational order. | Duplicate collapse, boolean insert/remove results, copy independence, and order-independent equality/hash. |
| **M14.13 — Identity-key wrappers** *(complete)* | Implement `Identity[&T]` and `Identity[&var T]` equality, hashing, and `StableHash` using managed target identity rather than pointee contents. | Aliases to one target compare/hash equally and work as map keys/set elements despite pointee mutation. |
| **M14.14 — Collection literal typing** *(complete)* | Type-check `@vec`, `@map`, and `@set` without introducing general macros; require exact homogeneous types, contextual typing for empty literals, and `StableHash` where required. | Compile-pass contextual literals and compile-fail heterogeneous, ambiguous-empty, and unstable-key cases. |
| **M14.15 — Collection literal lowering** *(complete)* | Evaluate entries left-to-right into explicit temporaries, then build the collection; later duplicate map keys replace values and duplicate set entries collapse. | Side-effect ordering and duplicate tests use independently copied inputs. |
| **M14.16 — Collection places** *(complete)* | Lower value-context indexing as a copy and mutable array/`Vec`/`Map` paths as assignable, non-addressable places. Evaluate a compound-assignment destination once and expose no safe interior reference. | Replacement, compound assignment, nested-field mutation, absent-key traps, and reference-formation rejection. |
| **M14.17 — Collection iteration** *(complete)* | Lower `for` over arrays, `Vec`, `Map`, and `Set`: evaluate and copy the iterable once, keep hidden state rooted, and copy each yielded binding. Arrays/vectors preserve index order; map/set order remains unspecified. | Source-mutation independence, yielded-copy independence, early `break`/`continue`, and pair typing for maps. |
| **M14.18 — Formatter runtime** *(complete)* | Define the mutable `Formatter` representation and primitive append/write operations used by `Display` lowering. Keep sequencing explicit and allocation behind managed runtime hooks. | Direct formatter tests cover text growth, Unicode, allocation, and left-to-right writes. |
| **M14.19 — `Display` implementations** *(complete)* | Add built-in `Display` for primitives, text, references to displayable values, and displayable collections; connect ordinary user impls through M13 static and dynamic dispatch. | One focused test per family plus user static/trait-object impls; map/set tests assume no order. |
| **M14.20 — Formatted strings** *(complete)* | Lower formatted literals to left-to-right formatter operations with exactly-once interpolation, `{{`/`}}` escapes, immutable `str` results, and no width/precision/debug extensions. | Evaluation-order, escape, unmatched-brace, non-`Display`, and nested-formatting tests. |
| **M14.21 — `print` and `println`** *(complete)* | Replace the early output skeleton with the single-value generic `Display` functions, keeping `std.io` and prelude entry points consistent. | Primitive, string, user-display, formatted-string, and newline behavior. |
| **M14.22 — Standard-value integration** *(complete)* | Run the complete value/copy/place/formatting matrix through generic instantiation and C emission; remove M8 boundary diagnostics only for completed constructs. | Collection, `for`, numeric-alternative, and formatting regions of `spec_demo.elx` run without enabling M15 behavior. |

Validation:

- Every specified array, string, numeric-alternative, `Vec`, `Map`, and `Set`
  API has a focused run-pass test and every documented trap has a runtime test.
- Copy independence holds after construction, lookup, insertion, removal,
  iteration, assignment, argument passing, and return.
- Empty and duplicate literals, unstable keys, floating keys, and
  collection-interior reference attempts have focused coverage.
- Formatting covers escapes, unmatched braces, left-to-right exactly-once
  evaluation, built-in values, and user `Display` impls.

### Milestone 15: `Result`, postfix `?`, and `defer`

> Status: Complete.
>
> The propagation role keys on the standard `Result` declaration's identity
> through `check::standard_result_payloads` (aliases included), so a shadowing
> user `Result` receives nothing. `check_try` validates the operand and the
> enclosing function's return type against one exact `E`; the checker also
> rejects a deferred non-unit or unsafe/foreign call (`call_is_unsafe`
> records the rule Milestone 17 re-applies to executable foreign calls).
>
> Cleanup lowering is *static*: registration is per-lexical-block and a
> block's statement list is straight-line at the statement level, so at any
> exit edge the reached registrations are exactly the `defer` statements
> lexically preceding that edge in each exited scope. `src/ir.rs` therefore
> keeps one cleanup plan per open scope and re-lowers the registered bodies at
> every edge — fallthrough and `return` (M15.7), `break`/`continue` down to
> the loop body's recorded scope depth (M15.8), and `?` propagation across
> every open scope (M15.9) — innermost scope first, reverse registration
> order within a scope, a `defer:` body forward as one unit. No runtime
> registration list, callable, or environment value exists
> (`deferred_execution_is_static_and_constructs_no_callable` proves the
> per-edge expansion in generated C), and re-lowering at the edge is what
> gives deferred expressions their execution-time values (M15.10).
>
> Postfix `?` lowers to a discriminant branch reusing the match machinery:
> the `Err` path copies the error payload, builds the enclosing return type's
> `Result.Err`, runs every open scope's cleanup, and returns; the `Ok` path
> copies the payload as the expression's value. Return values are evaluated
> and copied before cleanup begins, so an unconditionally deferred `close()`
> on a returned shared handle closes the returned copy through its alias.
> Traps and OOM keep terminating through the existing non-unwinding runtime
> path (M15.11).
>
> `io.IoError`, which the authoritative demonstration propagates, became an
> ordinary compiler-supplied declaration in the `std.io` module (the M14
> pattern); like `NumericError`, its variants are implementation-chosen
> because no normative text names them — M18.2's standard-package skeleton
> owns refining that surface. Cleanup remains explicit: there is no
> compiler-known cleanup trait, privileged method name, implicit destruction,
> GC finalizer, `with`, or `errdefer`.

**Goal:** Complete recoverable error propagation and deterministic explicit
resource cleanup.

Implementation tasks, in order:

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **M15.1 — Compiler-known `Result` role** *(complete)* | Resolve the exact standard `Result[T, E]` identity for propagation and expose typed/IR queries for its tag and payloads. Do not recognize a user type merely because it is also named `Result`. | The standard type is recognized under aliases/imports; a shadowing user type receives no special behavior. |
| **M15.2 — Postfix `?` typing** *(complete)* | Accept `?` only for `Result[T, E]` inside a function returning `Result[U, E]` with exactly the same `E`; reject `Option`, differing errors, and non-result operands without implicit conversion. | One compile-pass case and focused compile-fail cases for every rejected shape. |
| **M15.3 — Postfix `?` control flow** *(complete)* | Evaluate the operand once, branch explicitly on its tag, copy an `Ok` payload into the expression result, and copy an `Err` payload into an early `Result.Err` return. | Side-effect-counting and copy-independence tests for both branches. |
| **M15.4 — Deferred-registration IR** *(complete)* | Represent each reached `defer call` or `defer:` as one registration owned by its lexical scope. Retain syntax/typed IR tied to lexical binding identities; do not create a closure, capture values, or evaluate deferred expressions at registration. | IR tests distinguish registration from execution and show no callable/environment value is constructed. |
| **M15.5 — Deferred-body checking** *(complete)* | Require the single-call form to be a safe unit-returning call with no postfix `?`. Type-check a block form as its existing lexical scope while preserving the M7 bans on control redirection, nested `defer`, and `unsafe:`. Reject a deferred call whose existing function signature is unsafe; M17 applies the same recorded rule to foreign calls. | Compile-fail tests for non-call, non-unit, `?`, each forbidden statement, and a direct unsafe call. |
| **M15.6 — Scope cleanup plans** *(complete)* | Build one cleanup plan per lexical scope. Preserve source registration order in IR and execute registrations in reverse order; execute a `defer:` body forward as one registration. | IR and run-pass tests for multiple calls, multiple blocks, and mixed call/block registration. |
| **M15.7 — Fallthrough and return edges** *(complete)* | Route normal block fallthrough and explicit `return` through the required inner-to-outer cleanup chain. Evaluate and independently copy the return value before cleanup begins. | Nested-scope ordering tests and a returned shared handle whose deferred mutation is observable in the returned copy. |
| **M15.8 — Loop-exit edges** *(complete)* | Route `break` and `continue` through exactly the scopes exited by that edge, without running cleanup for scopes that remain active. | Nested loop/block tests for both exits and repeated registrations across iterations. |
| **M15.9 — Propagation edges** *(complete)* | Compose M15.3 with cleanup plans so an `Err` propagation copies its error before running every exited scope's cleanup. | Nested-scope `?` tests proving exactly-once evaluation, copy-before-cleanup, and ordering. |
| **M15.10 — Deferred-value liveness** *(complete)* | Keep bindings and managed targets referenced by deferred syntax alive until that registration finishes. Rebinding a `var` changes the value observed at exit; a `let` continues to identify the original value. | Reassigned-variable, managed-allocation, and nested aggregate liveness tests. |
| **M15.11 — Non-unwinding termination boundary** *(complete)* | Ensure traps, OOM, and traps during deferred execution terminate through the existing runtime path without promising remaining cleanup. Do not synthesize exception unwinding. | Subprocess tests confirm termination and avoid asserting execution of remaining registrations. |
| **M15.12 — Error/cleanup integration** *(complete)* | Remove lowering diagnostics only for completed `Result`, `?`, and `defer` forms; run them together with methods, generics, traits, collections, and formatting. | The error and resource-cleanup regions of `spec_demo.elx` build and run. |

Validation:

- Exact error-type propagation and explicit conversion of differing errors.
- Exactly-once `Result` evaluation and payload copying.
- Deferred calls and blocks on fallthrough and every non-trap exit edge.
- Reverse registration order, forward execution within one deferred block,
  nested scopes, reassigned `var` bindings, and return-value copying before
  cleanup.
- A returned explicitly shared resource handle is observed closed when its
  ordinary `close()` call was unconditionally deferred in the returning scope.

## 9. Stage G: unsafe code and C interoperability

### Milestone 16: unsafe contexts and raw pointers

> Status: Complete.
>
> The lexical `unsafe:` depth the checker already tracked is the only unsafe
> context; an `unsafe` function's body is deliberately not one. One call gate
> in `check_expr` (via the `call_is_unsafe` query M15.5 introduced) covers
> direct, bound, unbound, trait-dispatched, and indirect `&unsafe fn` calls.
> The complete cast matrix lives in `check_cast`: reference-to-pointer and
> pointer downgrades are safe, pointee-changing raw casts require `unsafe:`,
> nothing upgrades `*T` to any `*var U`, and pointers never convert to or
> from integers. A raw dereference is a place with its own classification
> (`PlaceKind::RawPointerTarget`): assignable through `*var T`, read-only
> through `*T`, and never the source of a safe reference — the sanctioned
> raw-to-reference path is the explicit `as` conversion, which requires
> `unsafe:`, exact pointee and mutability, and the same runtime check as a
> dereference.
>
> Runtime checks are one `Instruction::CheckPointer` per executed dereference
> or raw-to-reference conversion, emitted as per-pointee helpers that trap
> `E-RUN-NULL`/`E-RUN-ALIGN` with source locations; alignment comes from a
> C99 `offsetof` probe because `_Alignof` is C11-only. The mandatory
> expression-local validity rule is implemented exactly at its specified
> ceiling: the evaluator looks through grouping and casts within the operand
> and nothing else — and since the initial language has no integer-to-pointer
> conversion, `null` is the only provable-invalid constant, making the
> misalignment half vacuous until such an expression exists. A recovered
> reference to a promoted managed target restores a strong path
> (`raw_to_reference_conversion_restores_a_strong_managed_path`); raw
> pointers themselves are never registered as roots. With this milestone the
> authoritative demonstration passes the complete frontend check; full
> execution still awaits M17's foreign declarations and M18's `std`
> surface.

**Goal:** Isolate unverifiable pointer operations behind the specified lexical
unsafe boundary.

Implementation tasks, in order:

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **M16.1 — Pointer values and equality** *(complete)* | Lower `null`, `*T`, and `*var T` values and address-only equality. Preserve exact pointee type and mutability in typed IR and C representations. | Null/non-null equality tests for both pointer mutabilities and both targets. |
| **M16.2 — Safe pointer conversions** *(complete)* | Implement `&T` to `*T`, `&var T` to `*var T` or `*T`, and `*var T` to `*T`. Preserve address and never infer a pointee-changing or mutability-upgrading conversion. | Compile-pass conversion matrix and compile-fail implicit/upgrade cases. |
| **M16.3 — Unsafe-context tracking** *(complete)* | Track lexical `unsafe:` depth independently from whether the enclosing function is declared `unsafe`. Require the context for raw access, raw-to-reference conversion, pointee-changing casts, and unsafe calls. | Safe wrapper passes; unsafe function body without a nested block fails; nested blocks recover normally. |
| **M16.4 — Unsafe function references** *(complete)* | Preserve safety in named and unbound function-reference types. Taking, copying, and comparing `&unsafe fn` remains safe; invocation requires `unsafe:` and never converts to or from `&fn`. | Reference-use run-pass tests and invocation/signature compile-fail tests. |
| **M16.5 — Pointee-changing casts** *(complete)* | Lower permitted raw-pointer casts while preserving address, source provenance/extent obligations, and mutability permission. Reject every `*T` to `*var U` upgrade and all pointer/integer arithmetic or conversion. | Complete cast matrix with explicit rejection tests. |
| **M16.6 — Dereference places** *(complete)* | Type-check raw reads and writes as places; writes require `*var T`. Raw-pointer method receivers still require an exact type and receive no implicit borrow, cast, downgrade, or dereference. | Read/write and raw-receiver tests, including immutable-pointer write failure. |
| **M16.7 — Expression-local validity evaluator** *(complete)* | Evaluate only literals, casts, and operators inside the pointer operand needed to prove null or misalignment. Do not propagate required-error facts through bindings, assignment, branches, reachability, or calls. | Known-invalid expressions fail; equivalent values reached through locals or branches remain accepted. |
| **M16.8 — Runtime access checks** *(complete)* | Emit mandatory null and target-alignment checks immediately before every executed dereference and raw-to-reference conversion, with stable trap categories and source locations. | Subprocess trap tests for null/misaligned read, write, and conversion. |
| **M16.9 — Raw-to-reference conversion** *(complete)* | Convert to `&T`/`&var T` only in `unsafe:`, after runtime checks, with exact mutability. A valid managed target becomes strongly reachable through the resulting reference; a foreign/manual target receives no lifetime extension. | Managed-target liveness test and documented foreign-lifetime contract fixture. |
| **M16.10 — Root and cleanup integration** *(complete)* | Ensure raw pointers alone are not intentionally registered as language roots. Preserve explicit strong paths across operations and finish rejection of unsafe deferred calls without weakening `defer:` restrictions. | Best-effort raw-nonroot test, safe-wrapper keep-alive test, and deferred-unsafe-call compile failure. |

Validation:

- Every permitted safe and unsafe conversion and every mutability failure.
- Runtime null and alignment traps.
- Compile-time rejection only for expression-local known-invalid operands,
  including tests proving that local and branch facts do not become required
  errors.
- Raw-pointer receiver calls with exact types and no implicit adaptation.
- Safe wrappers that establish all obligations internally.

### Milestone 17: C ABI, foreign roots, and callbacks

> Status: Next.
>
> Boundary: the backend remains C99. Elamite's internal calling convention is
> not the public C ABI, ordinary function references never convert to C
> function pointers, and safe references never cross the boundary directly.

**Goal:** Interoperate with C without weakening Elamite's ordinary type and
reachability rules.

Implementation tasks, in order:

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **M17.1 — Foreign declaration validation** | Validate parsed module-level `extern "C"` blocks, bodyless imports, opaque types, foreign structs, and exported module-level C-callable functions. Reject generics, derive lists, methods, bodies where forbidden, and C variadics. | One compile-pass declaration of each kind and focused compile-fail cases for every structural restriction. |
| **M17.2 — ABI-safety query** | Centralize the exact recursive ABI-safe type query. Accept the specified scalars, raw pointers, foreign structs, `std.ffi.CVoid` behind a pointer, and exact `extern "C" fn` signatures; reject every listed Elamite-only representation. | Table-driven tests for all accepted/rejected types on x86 and x86-64. |
| **M17.3 — Foreign layout and declarations** | Emit named C99 declarations for opaque and foreign-struct types, preserving field order and target layout assumptions without C11 anonymous members or `_Static_assert`. | C harness confirms size/alignment/field offsets for nested ABI-safe foreign structs. |
| **M17.4 — Imported functions** | Emit exact unmangled C symbol declarations and unsafe call sites. Map fixed-width/pointer-sized integers, floats, unit return, raw pointers, foreign structs, and C function pointers without implicit marshalling. | Scalar/pointer/struct calls link and run; unsafe-context, deferred-direct-call, and non-ABI-safe signatures fail. |
| **M17.5 — Native link inputs** | Feed manifest native libraries and link options from the resolved package graph into deterministic build commands. Source `import` remains compile-time lookup only and never executes initialization. | Fake-toolchain argument test and a real small-library link test. |
| **M17.6 — Exported C entry points** | Emit stable process-lifetime callback symbols for module-level `extern "C" fn`; keep their identity and type distinct from ordinary Elamite functions. | C calls exported safe and unsafe-signature functions; ordinary-function conversion is rejected. |
| **M17.7 — Non-retaining call liveness** | Keep each source binding/reference strongly reachable for the complete foreign call when a raw pointer is not retained. Document borrowed foreign-pointer duration without inventing runtime ownership. | A C harness triggers collection during a call and still reads the live buffer. |
| **M17.8 — Foreign-root registration core** | Implement runtime registration/unregistration and `ForeignRoot[T]`/`ForeignRootMut[T]` construction and pointer access. Retaining promotes storage when needed. | Retained buffers survive collection and yield exact raw pointer mutability. |
| **M17.9 — Foreign-root shared handle API** | Represent one registration as explicit shared handle state. Make `.close()` idempotent, close through any copy, return a closed-handle error from later pointer access, and never unregister from GC reachability alone. | Copy/close/error tests and a best-effort leak test that asserts no finalizer behavior. |
| **M17.10 — Callback context pattern** | Support a retained raw context pointer backed by an open foreign-root registration; recover a temporary safe reference only inside `unsafe:` and keep it within the foreign contract. | Stateful callback round trip with explicit registration lifetime. |
| **M17.11 — Error and trap boundary** | Require wrappers to translate `Result`, `errno`, status codes, and out-parameters explicitly. Make traps terminate rather than unwind through C and document C++ exceptions/`longjmp` across Elamite frames as contract violations. | Status-translation tests and trap-in-foreign/callback subprocess tests. |
| **M17.12 — Callback thread restriction** | Track enough runtime entry state to support direct/nested reentry on an OS thread already executing Elamite and reject or terminate checkable foreign-thread entry. Document unchecked foreign-created or concurrent invocation as undefined behavior pending I-015. | Same-thread nested reentry harness and a separately isolated forbidden-thread test. |

Validation:

- Compile and link against small C libraries covering every ABI-safe scalar,
  raw pointers, foreign structs by value, opaque handles, and callbacks.
- Deliberately mismatched or non-ABI-safe declarations fail when statically
  detectable.
- Retained managed buffers remain alive while registered and are unavailable
  through a closed registration.
- Safe wrappers explicitly encode strings and translate C error channels.
- Callback reentry and trap termination are exercised with a C harness.

## 10. Stage H: complete the initial implementation

### Milestone 18: prelude, standard library, and developer tooling

> Status: Pending Milestone 17.
>
> Boundary: compiler-known names are ordinary declarations from the user's
> perspective. Keep the intrinsic list identical to `LEDGER.md` §12; in
> particular, there is no compiler-known cleanup trait and `std` remains an
> ordinary, shadowable package name.

**Goal:** Turn compiler mechanisms into a coherent, usable initial toolchain.

Implementation tasks, in order:

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **M18.1 — Intrinsic inventory audit** | Compare compiler-known IDs, lowering hooks, prelude entries, and `LEDGER.md` §12. Remove accidental intrinsics and document why each remaining entity cannot yet be ordinary Elamite code. | A single inventory test fails on missing, extra, or multiply defined compiler-known entities. |
| **M18.2 — Standard package skeleton** | Create the ordinary `std` package/module tree, including `std.io` and `std.ffi`, with source declarations for every non-intrinsic public API. Keep module initialization nonexistent. | The package resolves and type-checks through the ordinary package graph. |
| **M18.3 — Prelude assembly** | Re-export exactly the specified prelude surface from `std`; preserve normal lookup and allow local/module/dependency declarations to shadow the name `std`. | Positive lookup tests and negative tests for non-prelude standard APIs. |
| **M18.4 — Standard implementations** | Move trait impls, associated functions, and wrappers that need no intrinsic behavior into Elamite source. Keep compiler hooks behind the smallest reviewed declarations. | The standard library compiles as ordinary source except for the recorded intrinsic boundary. |
| **M18.5 — Dependency artifact workflow** | Complete deterministic library compilation, dependency ordering, artifact reuse within one build, public metadata consumption, and native link propagation. | Multi-package executable links against a built Elamite library and uses re-exported generic APIs. |
| **M18.6 — CLI surface** | Stabilize `check`, `build`, and `run` argument parsing for target, release mode, output directory, C compiler, native inputs, and generated-C retention; provide consistent exit statuses and diagnostics. | Command-level golden tests cover valid combinations, invalid options, and tool failures. |
| **M18.7 — Intermediate dumps** | Add deterministic dumps for tokens, syntax, resolution, typed IR, control-flow IR, monomorphized instances, and generated C. Every entry retains useful source identity/span information. | Snapshot each dump twice and compare byte-for-byte. |
| **M18.8 — Test-runner support** | Provide a reproducible conformance runner, temporary-directory isolation, artifact retention after failure, target/optimization selection, and stable expected-output handling. | The runner can select one fixture, one target, or the full matrix and preserves a failing build. |
| **M18.9 — Documentation extraction** | Expose attached documentation comments and public API signatures without coupling documentation generation to semantic success of unrelated private bodies. | Public/private filtering and source-link tests. |
| **M18.10 — Toolchain documentation** | Document supported Linux targets, C99 compiler requirements, Boehm installation/linking, unstable compiler interfaces, foreign-thread limits, and all deliberate language limitations. | Documentation commands and examples match the implemented CLI and CI matrix. |

Validation:

- The standard library can be compiled as an ordinary package except for a
  reviewed list of intrinsics.
- Separate packages can consume public APIs and re-exports from a built library.
- Every intermediate dump is deterministic and refers back to source spans.

### Milestone 19: conformance, hardening, and initial release gate

> Status: Pending Milestone 18.
>
> Boundary: this milestone adds no language features. A failing normative rule
> returns ownership to the milestone named in `LEDGER.md`; it is not patched
> around in the conformance harness.

**Goal:** Demonstrate that the entire specified initial language works together.

Implementation tasks, in order:

| Task | Deliverable | Focused acceptance |
| --- | --- | --- |
| **M19.1 — Authoritative demo fixture** | Turn `examples/spec_demo.elx` and its package files into a deterministic end-to-end fixture with explicit expected output on both supported targets. | Debug and release builds produce the same specified behavior. |
| **M19.2 — Section conformance fixtures** | Add focused positive, negative, trap, and interaction fixtures for each `SPEC.md` section so failures localize without the large demo. | Every section has an owned fixture directory and a documented test layer. |
| **M19.3 — Ledger closure audit** | Review every normative `LEDGER.md` row against implementation code and a named test. Mark it implemented, specified as unsupported, or return it to its owning milestone. | No row remains with an ambiguous pass, runtime dependency, or test status. |
| **M19.4 — Target/optimization matrix** | Run x86 and x86-64 across debug/release C optimization. Concentrate explicit cases on integer width, layout, root liveness, evaluation order, and function identity. | All supported matrix cells pass or the target claim is revised before release. |
| **M19.5 — Strict generated-C gate** | Compile generated C99 with strong warnings treated as errors. Exercise available undefined-behavior and memory instrumentation without allowing backend-generated suppressions to hide defects. | The conformance suite is warning-clean and sanitizer-clean in supported configurations. |
| **M19.6 — Lexer/parser robustness** | Add seeded property/fuzz corpora for indentation, delimiters, escapes, formatted strings, recovery, and arbitrary malformed token streams. | No panic, hang, or unbounded diagnostic cascade across the retained corpus. |
| **M19.7 — Semantic robustness** | Feed parsed malformed and type-invalid programs through resolution, checking, lowering boundaries, and diagnostic rendering with explicit error symbols/types. | No user input reaches an internal-invariant panic or malformed C emission. |
| **M19.8 — Runtime stress** | Stress recursive copying, generic instantiation, collection churn, cycles, reference promotion, cleanup chains, traps, and callback reentry. | Stable output/status and no sanitizer failures under repeated runs. |
| **M19.9 — Diagnostic review** | Curate common mistakes for modules, types, traits, generics, collections, unsafe code, and FFI. Ensure primary/related spans point to Elamite source and categories remain stable. | Snapshot review contains no raw internal IDs or unexplained IR/backend failures. |
| **M19.10 — Performance baseline** | Record compile time, peak memory, generated-C size, native size, and runtime for representative programs without changing semantics to meet arbitrary targets. | Reproducible benchmark inputs and baseline results are checked in or published with the release process. |
| **M19.11 — Release audit** | Verify version/spec reporting, supported-toolchain documentation, license/notices, known limitations, unsupported concurrency, and foreign-thread restrictions. | Every initial release criterion below has linked evidence. |

Initial release criteria:

- The authoritative demonstration builds and produces its expected output.
- All required positive, negative, runtime-trap, multi-package, and C ABI tests
  pass on every declared supported target.
- There are no known violations of independent value copying, managed-reference
  reachability, explicit unsafe boundaries, or cleanup ordering.
- Unsupported concurrency and foreign-thread callback behavior is documented.
- The compiler can report its version and the specification revision it
  targets.

### Milestone 20: post-conformance optimization

> Status: Candidate work after Milestone 19.

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

## 11. Deferred concurrency design gate

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

## 12. Recommended delivery checkpoints

The milestone sequence has four useful external checkpoints:

1. **Frontend checkpoint — Milestone 7:** every initial source form parses, and
   the safe non-generic core is resolved and type-checked.
2. **Executable-core checkpoint — Milestone 10:** safe programs with ordinary
   values and escaping references compile through C and run under Boehm GC.
3. **Language-feature checkpoint — Milestone 15:** methods, function
   references, generics, traits, collections, errors, and `defer` work together.
4. **Initial conformance checkpoint — Milestone 19:** unsafe code, C
   interoperability, the standard library, and the authoritative demonstration
   satisfy the full initial test matrix.

These checkpoints are reporting boundaries, not substitutes for the exit
criteria of the individual milestones.
