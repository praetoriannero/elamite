# Elamite Compiler Implementation Plan

> Status: Active — Milestones 0 through 10 complete, Milestone 11 next
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
  `StableHash`, `Display`, `Close`, `Identity`, `ForeignRoot`, and `CVoid`.
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
  flow, `return`, `break`, `continue`, `pass`, `unsafe`, and single-call
  `defer`.
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
- Resolve absolute package/dependency paths and `root`, `self`, `super`, and
  `std` paths. Keep lexical lookup separate from module-path lookup.
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
  function references, trait-object references, and shared resource state
  retain identity.
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

**Goal:** Implement static trait selection, coherence, compiler capabilities,
and explicit trait objects.

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

**Goal:** Supply the compiler-known standard value types needed by ordinary
programs and the authoritative demonstration.

Implementation work:

- Implement immutable UTF-8 `str`, mutable independently copied `String`, and
  explicit conversion between them.
- Implement fixed arrays, `Vec`, `Map`, and `Set` with the specified APIs and
  trap behavior.
- Lower `@vec`, `@map`, and `@set` as compiler-known construction forms. Do not
  create a general user macro system.
- Enforce exact homogeneous element types, contextual typing of empty literals,
  left-to-right construction, duplicate map replacement, and duplicate set
  collapse.
- Enforce `StableHash` on map keys and set elements. Implement
  `Identity[&T]` and `Identity[&var T]` with managed target identity.
- Implement value-context indexed copies and mutable collection places without
  making interiors addressable.
- Lower `for` by copying its iterable once into hidden state and copying each
  yielded element. Preserve index order for arrays and vectors while leaving
  map/set order unspecified.
- Implement `Display`, `Formatter`, formatted-string left-to-right evaluation,
  and the single-value `print` and `println` functions.

Validation:

- Full API tests, empty and duplicate literals, bounds/key traps, and nested
  mutation through mutable collection paths.
- Copy independence after every structural collection operation.
- Rejection of unstable keys, floating keys, and collection-interior
  references.
- Formatting escapes, unmatched braces, evaluation order, and user `Display`
  impls.

### Milestone 15: `Result`, postfix `?`, `Close`, and `defer`

**Goal:** Complete recoverable error propagation and deterministic explicit
resource cleanup.

Implementation work:

- Implement `Result[T, E]` and `Option[T]` as standard generic enums with their
  compiler-recognized roles.
- Type-check postfix `?` only in a function returning `Result[U, E]` with the
  exact same error type. Evaluate its operand once and copy either payload.
- Lower `?` into explicit branching and early return in control-flow IR.
- Validate the compiler-known `Close.close(self: &Self) -> ()` signature while
  leaving its idempotence and shared-state laws as implementation contracts for
  manually written impls.
- Accept only a safe, unit-returning single call after `defer`; reject multiline
  bodies, `errdefer`, direct unsafe or foreign calls, and deferred expressions
  containing postfix `?`.
- Retain a deferred call expression tied to its lexical bindings rather than
  evaluating or capturing its values at registration. Evaluate its callee,
  receiver, and arguments at scope exit using their then-current values.
- Lower every fallthrough, `return`, `?`, `break`, and `continue` edge through
  the appropriate cleanup chain. Run inner-scope calls before outer-scope calls
  and calls in one scope in reverse registration order.
- Evaluate and copy a return value or propagated error before entering cleanup
  blocks.
- Keep bindings referenced by a deferred expression live until that call
  completes.
- Do not implement unwinding after traps, OOM, or a trap in a deferred call.
  Remaining deferred calls are not guaranteed in those cases.

Validation:

- Exact error-type propagation and explicit conversion of differing errors.
- Exactly-once `Result` evaluation and payload copying.
- Deferred calls on fallthrough and every non-trap exit edge.
- Reverse ordering, nested scopes, reassigned `var` bindings, and return-value
  copying before cleanup.
- A returned resource handle is observed closed when its close was
  unconditionally deferred in the returning scope.

## 9. Stage G: unsafe code and C interoperability

### Milestone 16: unsafe contexts and raw pointers

**Goal:** Isolate unverifiable pointer operations behind the specified lexical
unsafe boundary.

Implementation work:

- Type-check `*T`, `*var T`, `null`, raw dereference, safe-to-raw conversion,
  mutability downgrade, pointee casts, and raw-to-safe-reference conversion.
- Require explicit `unsafe:` context for dereference, raw-to-reference
  conversion, pointee-changing raw casts, and unsafe calls.
- Keep unsafe function caller contracts separate from unsafe operations in the
  body. An unsafe function body still needs nested `unsafe:` blocks.
- Preserve safety in unbound unsafe method and named unsafe function references.
- Permit writes only through `*var T` and forbid casts that upgrade a read-only
  raw pointer to any mutable raw pointer.
- Preserve pointer address, provenance metadata assumed by the language model,
  designated extent, and mutability permission through permitted conversions.
  Most provenance obligations remain an unsafe source contract rather than
  runtime metadata.
- Emit mandatory null and alignment checks for every executed raw dereference
  and raw-to-reference conversion.
- Implement the narrow expression-local constant evaluator used to reject a
  statically known null or misaligned operand. Do not propagate required
  pointer-validity errors through bindings, assignments, branches,
  reachability, or calls.
- Ensure a raw pointer alone does not keep managed storage alive and document
  the keep-alive patterns expected of safe wrappers.

Validation:

- Every permitted safe and unsafe conversion and every mutability failure.
- Runtime null and alignment traps.
- Compile-time rejection only for expression-local known-invalid operands,
  including tests proving that local and branch facts do not become required
  errors.
- Raw-pointer receiver calls with exact types and no implicit adaptation.
- Safe wrappers that establish all obligations internally.

### Milestone 17: C ABI, foreign roots, and callbacks

**Goal:** Interoperate with C without weakening Elamite's ordinary type and
reachability rules.

Implementation work:

- Parse and validate module-level `extern "C"` declarations, opaque types,
  foreign structs, imported functions, and exported module-level C-callable
  functions.
- Compute or obtain target C layout for foreign structs and validate the exact
  initial ABI-safe type set. Reject safe references, ordinary aggregates,
  `bool`, `char`, strings, trait objects, ordinary function references,
  collections, and 128-bit integers at the boundary.
- Map fixed-width integers, pointer-sized integers, floating types, C `void`,
  raw pointers, foreign structs, and `extern "C" fn` pointers to exact C ABI
  declarations.
- Treat every imported foreign function as unsafe and bodyless. Do not support C
  variadics or implicit marshalling.
- Consume native library and link options from the package graph without making
  source `import` execute code.
- Emit exported callback symbols with stable process-lifetime addresses and
  prevent ordinary Elamite function references from converting to C function
  pointers.
- Implement `ForeignRoot[T]` and `ForeignRootMut[T]` registrations, shared
  handle state, idempotent close, pointer access errors after close, and explicit
  unregistering. GC must never unregister them automatically.
- Preserve a strong source path across a non-retaining foreign call. Require an
  open foreign-root registration for managed storage retained after return.
- Define the runtime trap boundary so traps terminate rather than unwind through
  C. Document C++ exceptions, `longjmp`, and other foreign unwinding across
  Elamite frames as unsupported contract violations.
- Enforce the initial callback thread restriction: direct or nested reentry on
  an OS thread already executing Elamite is supported; foreign-created or
  concurrent callback threads are not.

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

**Goal:** Turn compiler mechanisms into a coherent, usable initial toolchain.

Implementation work:

- Provide the `std` package and prelude declarations corresponding exactly to
  compiler-known types, traits, functions, and capabilities.
- Keep compiler knowledge minimal and explicit. Prefer ordinary Elamite
  declarations and implementations when the semantics do not require an
  intrinsic.
- Complete executable and library artifact workflows, dependency compilation,
  native link configuration, and useful package diagnostics.
- Add commands or options to dump tokens, syntax, resolved declarations, typed
  IR, control-flow IR, monomorphization results, and generated C.
- Add a reproducible conformance-test runner and a way to retain temporary build
  artifacts after failures.
- Generate or expose documentation comments without making documentation
  generation a prerequisite for semantic analysis.
- Document supported targets, required C and GC toolchain components, unstable
  compiler interfaces, and known limitations.

Validation:

- The standard library can be compiled as an ordinary package except for a
  reviewed list of intrinsics.
- Separate packages can consume public APIs and re-exports from a built library.
- Every intermediate dump is deterministic and refers back to source spans.

### Milestone 19: conformance, hardening, and initial release gate

**Goal:** Demonstrate that the entire specified initial language works together.

Implementation work:

- Turn `examples/spec_demo.elx` into an end-to-end run-pass conformance test.
- Add one focused conformance file per specification section rather than relying
  on the large demonstration to localize regressions.
- Audit the feature ledger from Milestone 0. Every item must be implemented,
  deliberately deferred by the specification, or represented by a required
  diagnostic.
- Fuzz the lexer and parser, and fuzz semantic inputs through the compiler with
  the invariant that malformed source never crashes the compiler.
- Run generated C with strong C warnings and available memory/undefined-behavior
  instrumentation. Investigate compiler-generated warnings as backend defects.
- Test multiple optimization levels and supported pointer widths. Pay special
  attention to integer boundaries, root liveness, left-to-right evaluation,
  function identity, and C ABI layout.
- Measure compile time, generated C size, native code size, and runtime behavior
  on representative programs. Optimize only after establishing a regression
  benchmark.
- Review diagnostics for common mistakes and ensure errors identify Elamite
  source rather than exposing internal IR failures.

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

**Goal:** Improve implementation quality without changing language behavior.

Candidate work:

- Precise escape analysis to keep nonescaping referenced storage on the stack.
- Copy elision and move-like backend optimizations that remain unobservable at
  the source level.
- Copy-on-write storage for `String`, arrays where useful, and standard
  collections.
- Specialized monomorphized layouts and devirtualization of statically known
  trait-object calls.
- Incremental queries, dependency artifact caching, parallel package
  compilation, and deterministic cache keys.
- Better source maps, editor-facing diagnostics, and language-server support.
- Optional warnings for provable resource leaks and suspicious raw-pointer
  contracts.

Each optimization needs before-and-after conformance runs and targeted tests
that would expose a change in copy independence, evaluation order, identity,
root lifetime, or trap behavior.

## 11. Deferred concurrency design gate

Do not implement task syntax, schedulers, channels, synchronization traits, or
cross-thread callbacks until `I-015` defines:

- whether execution is only cooperative or may be parallel;
- which values may cross a task boundary;
- how safe references, mutable aliases, raw pointers, trait objects, function
  references, and `Close` handles behave across that boundary;
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
