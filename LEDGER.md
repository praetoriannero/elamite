# Elamite Feature Ledger

> Status: Active — Milestone 0 deliverable, maintained as milestones complete
>
> Basis: `SPEC.md` version 0.4.0-draft and `examples/spec_demo.elx`
>
> Purpose: turn the specification into an implementable checklist, per the
> completed [`ROADMAP.md`](ROADMAP.md) Milestone 0. This document assigns no new
> semantics. When it conflicts with `SPEC.md`, the specification wins and this
> ledger must be corrected.

## 0. How to read this ledger

Each row maps one normative rule (or a small cluster of tightly related rules)
to:

- **Pass** — the `ROADMAP.md` milestone(s) expected to implement it, using the
  short tags below.
- **Runtime** — what the generated program depends on at run time, if
  anything (`—` means purely a compile-time rule with no runtime footprint).
- **Tests** — which `ROADMAP.md` §2.4 test layer(s) should cover it: *parse*,
  *compile-pass*, *compile-fail*, *run-pass*, or *integration*.

| Tag | Milestone |
| --- | --- |
| M0  | Specification migration and feature ledger (this document) |
| M1  | Compiler driver and package graph |
| M2  | Lexer and indentation engine |
| M3  | Complete surface parser |
| M4  | Declaration collection, imports, and visibility |
| M5  | Canonical type system and inference core |
| M6  | Core expression and function checking |
| M7  | Control flow, patterns, and flow analysis |
| M8  | Typed IR, control-flow IR, and C backend skeleton |
| M9  | Complete logical value-copy lowering |
| M10 | Safe references, storage promotion, and Boehm GC |
| M11 | Methods and function references |
| M12 | Generics and monomorphization |
| M13 | Traits, derivation, and dynamic dispatch |
| M14 | Strings, collections, iteration, and formatting |
| M15 | `Result`, postfix `?`, and `defer` |
| M16 | Unsafe contexts and raw pointers |
| M17 | C ABI, foreign roots, and callbacks |
| M18 | Prelude, standard library, and developer tooling |
| M19 | Conformance, hardening, and initial release gate |
| M20 | Post-conformance optimization |
| M21 | Never-return type and explicit panic |
| M22 | Package tests, typed traps, and runner |
| M23 | Macro expansion foundations |
| M24 | Hygienic declarative macros |
| M25 | Macro diagnostics, tooling, and stabilization |
| M26 | Declarative custom derive generators |
| M27 | Compile-time execution runtime |
| M28 | Procedural macros and attributes |

This ledger maps ownership of work rather than serving as the sole completion
tracker. Milestones 0 through 19 are complete (implemented together in one
frontend pass where applicable; see `ROADMAP.md`); later rows remain planned until
their milestone status changes in `ROADMAP.md`. The Milestone 19 closure evidence
is indexed by `docs/release.md` and the section-owned fixture map in
`tests/fixtures/conformance/README.md`.

## 0.1 Legacy artifact inventory

`ROADMAP.md` Milestone 0 calls for inventorying legacy grammar, compiler
components, examples, and fixtures, preserving useful test cases. As of this
ledger, **there is nothing to inventory**: the prior Python implementation
(`src/elamite/*.py`), both Lark grammars (`elamite.lark`, `elx.lark`), and
`examples/spec_alt_demo.elx` were fully removed in the same commit that
introduced the Rust skeleton and `ROADMAP.md`. No legacy test fixtures, parser
output, or grammar files survive to migrate or reconcile. This closes that
part of Milestone 0's exit criteria by observation rather than by migration
work.

A `notes.txt` at the repository root predated the current specification (it
used brace-delimited bodies, `mut`, printf-style calls, and `case`/`abstract`
keywords that contradict `SPEC.md`'s indentation-delimited, `var`-based
design). It was never an implementation input to this ledger and has been
removed.

---

## 1. Overview (§1)

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| Statically typed, GC'd, compiles to C; value types, explicit references, traits, generics, ADTs, `Result`, raw pointers behind `unsafe`, indentation-delimited control flow | M0 (identity only; detailed per section below) | — | — |
| Ordinary values copy by logical value copy; explicit-alias types (safe/raw ref, function ref, trait-object ref, shared resource handle) retain identity — detailed in §3.1 | M6, M9 | — | see §3.1 |
| `&value` / `&var value` explicit reference formation — detailed in §3.2 | M6, M10 | — | see §3.2 |
| No source lifetime parameters (no lifetime syntax exists) | M2, M3 (absence of grammar) | — | parse (rejects lifetime syntax as unrecognized) |
| The initial runtime uses Boehm GC behind a collector-neutral compiler/runtime interface; alternative strategies must preserve the same non-moving, cycle-reclaiming semantics, and programs must not depend on collection timing for cleanup — detailed in §9 | M8 (interface), M10 (integration) | Boehm GC by default | see §9 |

## 2. Program layout (§2)

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| Source files are UTF-8; identifiers match `[A-Za-z_][A-Za-z0-9_]*`; keywords are reserved; Unicode remains available in comments/docs and literal contents | M1 (UTF-8 source loading and shared identifier predicate), M2 (lexing) | — | parse, compile-fail (non-ASCII identifier) |

### 2.1 Comments and documentation

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| `//` line comment; consecutive `//` lines form a multiline comment | M2 | — | parse |
| `///` doc comment; contents are Markdown for the following declaration | M2 (capture), M3/M4 (attach to decl) | — | parse, compile-pass |
| Doc comments use the following declaration's indentation and do not open/close blocks | M2 | — | parse |

### 2.2 Indentation and bodies

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| Spaces-only leading indentation; a tab is an error | M2 | — | compile-fail |
| Exactly four spaces per nested block | M2 | — | compile-fail (wrong width) |
| Body-bearing construct: trailing `:` + newline + one deeper indent; body ends at dedent; dedent must return to an established level; EOF closes all open blocks | M2 (indent/dedent events), M3 (block parsing) | — | parse, compile-fail (bad dedent) |
| Body form applies to `mod`, `struct`, `enum`, `trait`, `impl`, `if`, `else`, `match`, `for`, `while`, `unsafe`, and function declarations with bodies | M3 | — | parse (one case per construct) |
| Brace-delimited bodies and same-line bodies are invalid everywhere | M3 | — | compile-fail |
| Empty body is invalid; `pass` is the explicit no-op | M3 (reject empty), M6 (`pass` is a valid statement) | — | compile-fail (empty body), compile-pass (`pass`) |
| Blank lines and comment-only lines do not affect indentation | M2 | — | parse |
| Statement continuation: lines indented exactly +4 from the statement's start join it; a body colon takes precedence over a continuation; continuation ends at the statement's starting indentation; unexpected indentation is an error | M2 (logical-line joining), M3 | — | parse, compile-fail |
| Inside `()`, `[]`, `{}`: newlines/indentation don't terminate the expression or open blocks; no backslash continuation; `{}` = record literals, `()` = tuples/grouping | M2, M3 | — | parse |

### 2.3 Modules, imports, and visibility

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| Package = compilation/dependency/nominal-identity/coherence unit; `elamite.toml` declares name, version, target kind (`lib`\|`exe`), dependencies, root file; default roots `src/lib.elx`/`src/main.elx` | M1 | — | compile-fail (malformed manifest), integration |
| Root file defines `root` module; other `.elx` files define file-backed modules from relative path; directories are namespace components; path components must be valid identifiers | M1 | — | integration, compile-fail (invalid component) |
| `mod name:` inline nested module; inline and file-backed modules cannot collide; file-backed modules need no bodyless `mod` declaration | M1 (discover file-backed paths), M3 (parse inline modules), M4 (reject collision during collection) | — | compile-fail (duplicate module path) |
| Path roots `root`, `self`, `super` (error at package root) are keywords; `std` and dependency aliases are ordinary names resolved after lexical bindings, module declarations, imports, and prelude names, so a module may shadow `std` | M4 | — | compile-fail (`super` at root) |
| Unqualified name lookup: lexical bindings, current-module decls/imports, prelude only — never unrelated modules | M4 | — | compile-fail |
| `use path` / `use path as name`; not inherited by nested modules; no wildcard/grouped `use` declarations; import order has no semantic effect | M3 (parse), M4 (resolve) | — | compile-pass, compile-fail |
| `pub` visibility on modules/fns/structs/enums/traits/aliases; fields and inherent methods package-private unless individually `pub`; all variants/payload fields of a `pub enum` are public; all methods of a `pub trait` are public | M4 | — | compile-fail (private access across packages) |
| `pub use path [as name]` re-export; module re-export exposes public contents only; reachability requires an unbroken re-export chain; re-export doesn't change nominal identity or defining package | M4 | — | compile-pass/fail (unreachable public decl) |
| Public signature may mention only publicly accessible types/traits/aliases/bounds; private members are not part of the public signature | M4, M5 | — | compile-fail |
| Shared module-item namespace (modules/types/traits/fns/module values/aliases/imports); duplicate declaration or import is an error even for the same target; explicit alias resolves it; local bindings may shadow module items | M4 | — | compile-fail (duplicate entry) |
| Circular imports within one package are permitted; declarations collected before import/body resolution; imports execute no code and establish no init order; the package dependency graph must be acyclic | M1 (dep-graph acyclicity), M4 (collect-before-resolve) | — | compile-pass (import cycle), compile-fail (package cycle) |
| Initial resolver accepts local path dependencies only; dependency aliases map to resolved package identities; canonical manifest-directory path defines identity, not display name/version; distinct identity ⇒ distinct nominal types/traits and feeds coherence/orphan rules (§6) | M1 (resolver, edge map, identity), M13 (consumes identity) | — | integration (alias edges; two instances, same display name) |

## 3. Values, mutability, and references (§3)

### 3.1 Copying values

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| `let` = non-rebindable, not itself a mutable place; `var` = rebindable mutable place | M6 | — | compile-fail (assign through `let`) |
| Field through an ordinary `let` aggregate cannot be assigned or `&var`-referenced, recursively; explicit aliases stored inside retain mutation capability | M6 (place classification) | — | compile-fail, compile-pass |
| Assignment/argument passing/return copy the source value; the source remains usable afterward | M6 (record copy op), M9 (lowering) | — | run-pass |
| Copying is a core value property, not trait-controlled | M5, M6 | — | — |
| Ordinary copy is recursively and observably independent; COW backing storage permitted if unobservable | M9 | — | run-pass (independence tests) |
| Explicit-alias types (safe ref, raw ptr, trait-object ref, function ref) retain identity when copied; copying an aggregate preserves these aliases while ordinary fields stay independent | M9 | — | run-pass |

### 3.2 References

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| `&T` shared / `&var T` mutable reference; `&value` / `&var value` form them; field/method access auto-dereferences | M5, M6, M10 | — | compile-pass, run-pass |
| Reference formation is explicit except a bound-call receiver; no other implicit `T` → `&T` conversion | M6 | — | compile-fail |
| `&var T` may update target fields; references are not exclusive — any number of shared/mutable refs may alias; sequential last-write-wins; no borrow/alias checking | M6, M10 | — | run-pass (aliasing example) |
| Go-style addressability: a binding or field of an addressable value; call results/computed expressions are not addressable; a referenced composite literal (`&Point{...}`) is the explicit exception, creating a GC target | M6 (addressability), M10 (composite-literal ref lowering) | Boehm GC | compile-fail (ref to call result), run-pass |
| Collection interiors are never addressable for reference formation; value-context collection access returns an independent copy | M6, M14 | — | compile-fail |
| Array/`Vec` element and `Map` value may be assignable places via a mutable collection path (replace/compound-assign/nested-mutate) without a reference escaping; `Map` keys/`Set` elements are never mutable places | M6, M14 | — | run-pass |
| References are valid struct fields, enum payloads, parameter types, and return types (no closure captures — closures do not exist) | M5 | — | compile-pass |
| A safe reference stays valid while reachable; an escaping local reference promotes required storage to GC-managed storage; escape analysis may keep non-escaping storage on the stack | M10 (done; promotion is conservative — every address-taken local is promoted, precise escape analysis is M20) | Boehm GC | run-pass (`a_reference_to_a_local_survives_its_frame`) |
| A reference formed directly from a binding targets that binding's storage cell and observes later assignment; promotion preserves this | M10 (done) | Boehm GC | run-pass (`references_observe_storage_through_binding_and_path`) |
| A reference into a nested aggregate points to that subvalue's storage within its container, so replacing the container is observable through the reference, and mutation through the reference is visible in the container (§19) | M10 (done) | Boehm GC (interior pointers) | run-pass (`city` example) |
| A reference into an aggregate keeps its whole container reachable | M10 (done; `GC_set_all_interior_pointers(1)` before `GC_INIT`) | Boehm GC (interior pointers) | run-pass (`an_interior_reference_keeps_its_whole_container_reachable`) |

### 3.3 Raw pointers and null

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| `*T`/`*var T`, nullable; `&T`/`&var T` always non-null; nullable safe ref = `Option[&T]`; conditions require `bool` (no pointer/reference truthiness); explicit `== null` test | M5, M6, M14.1 (`Option[&T]` executes), M16 | — | compile-pass/fail, run-pass (`option_of_a_safe_reference_keeps_a_recursive_graph_reachable`) |
| Provenance model: one storage instance + designated subobject; preserved by safe→raw conversion, copy, `*var T`→`*T`; comparison is address-only; `null` has no provenance; address reuse grants no provenance | M16.1 (done; representation preserves address, comparison compiles to `==`) | — | doc-contract (largely unenforceable), run-pass (`raw_pointers_read_write_and_compare_by_address`) |
| No pointer arithmetic or int↔pointer conversion initially; pointee-changing `as` cast only in `unsafe`; cast preserves address/provenance/extent/mutability without validating content; `*T` cannot cast to any `*var U` | M16.5 (done) | — | compile-fail (`raw_pointer_conversions_follow_the_exact_matrix`: upgrade, int↔ptr, pointee change outside `unsafe:`), run-pass |
| `&T`→`*T`, `&var T`→`*var T`/`*T` safe conversions; `*var T`→`*T` safe downgrade; dereference / raw→ref conversion require `unsafe`; write requires `*var T`; raw→ref conversion asserts non-null, aligned, valid, and remains valid for every use; `unsafe` never makes an ordinary reference nullable | M16.2/16.3/16.6/16.9 (done) | — | compile-fail (`unsafe_only_operations_require_a_lexical_unsafe_block`, `raw_dereference_places_permit_writes_but_never_safe_references`), run-pass |
| Dereference validity: storage alive, subobject initialized as pointee type, access within subobject (write needs writable storage); raw pointer is not itself a GC root, so managed-storage liveness needs a separate strong path; foreign storage lifetime is the foreign contract | M16, M9 (root doc) | — | doc-contract (largely unenforceable) |
| Every executed raw dereference/raw→ref conversion does a mandatory null+alignment check and traps on failure; compile-time error only for an expression-local constant-evaluable null/misaligned operand (literals/casts/operators within the operand expression — no propagation through bindings, assignment, branches, reachability, or calls); broader analysis may warn only | M16.7/16.8 (done; `E-RUN-NULL`/`E-RUN-ALIGN` via per-pointee C99 `offsetof` probes; only `null` is constructible as a constant pointer, so the misalignment half of the evaluator is vacuous until int→ptr conversion exists) | trap path | run-pass (`raw_pointer_null_and_alignment_checks_trap`), compile-fail/compile-pass (`pointer_validity_is_expression_local`) |
| Remaining obligations (provenance/liveness/bounds/init/pointee-type/write-permission) are UB if violated and generally unchecked; accidental GC retention or address reuse cannot validate a dangling pointer | M16 | — | doc-contract (not generally constructible as defined behavior), retained malformed-input corpus, and M19 UBSan gate (`generated_c_is_clean_under_address_and_undefined_behavior_sanitizers`) |
| Raw→safe-ref conversion asserts obligations hold for the reference's whole reachable lifetime; once valid it becomes a strong path for managed storage (§9); a reference to foreign/manual storage does not extend its lifetime; safe code alone cannot create UB through a raw pointer | M16.9 (done for managed targets), M17 (foreign contract) | Boehm GC | run-pass (`raw_to_reference_conversion_restores_a_strong_managed_path`), integration at M17 |

## 4. Types (§4)

### 4.1 Primitives, tuples, strings, collections

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| `bool`, `char`, `()`, signed/unsigned fixed-width integers, `f32`, `f64` | M5 | — | compile-pass |
| Integer literal bases (`0b`/`0o`/`0x`), `_` separators only between digits of one run, exact numeric suffixes | M2 | — | parse, compile-fail (bad digit/separator/suffix) |
| Unsuffixed integer literal materializes to the expected type or defaults `i32`; float literal (decimal point or exponent) materializes to the expected type or defaults `f64` | M5, M6 (contextual materialization) | — | compile-pass |
| Unary `-` is an operator, not part of the literal; range checking includes an immediately applied minus so signed-min is expressible | M6 | — | compile-pass (`i8::MIN`-equivalent literal) |
| Concrete numeric types never convert implicitly; arithmetic operands need compatible concrete types after materialization; `as` performs explicit numeric conversion | M6 | — | compile-fail (mixed-type arithmetic) |
| int→int cast traps out of range; float→int truncates toward zero and traps on NaN/inf/out-of-range; int→float/float→float use IEEE rounding; bool/char/enum excluded from numeric casts | M6 (typecheck), M8 (checked-cast codegen) | trap path | run-pass (trap), compile-fail |
| `Type.try_from`/`wrapping_from`/`saturating_from`; `try_from` returns `Result[Type, NumericError]` | M14.3, M14.4 (done) | — | run-pass (`checked_numeric_conversion_reports_instead_of_trapping`, `numeric_alternative_conversions_wrap_and_saturate`) |
| Integer arithmetic traps on overflow/div-by-zero/signed-min÷-1/bad shift count in every build; `checked_`/`wrapping_`/`saturating_` alternatives return `Option[T]`; float arithmetic follows IEEE 754; statically evident bad literal/conversion/arithmetic is a compile error; `isize`/`usize` use target pointer width | M8 (checked-arith helpers), M6 (static detection), M14.4 (alternatives; done) | trap path | run-pass (`numeric_alternatives_replace_the_trapping_operators_at_the_width_boundary`), compile-fail |
| Tuples via `()`; `()` = unit = empty tuple; `(v)` groups, `(v,)` is one-element | M3, M5, M6 | — | compile-pass |
| `str` immutable UTF-8; `String` mutable UTF-8, independently copied (COW allowed internally); `str` qualifies `StableHash`, `String` doesn't | M14 (done) | managed allocation | run-pass (`text_preserves_unicode_and_embedded_nul_bytes`) |
| Double-quoted strings and single-quoted characters contain direct Unicode scalars or `\\`/`\"`/`\'`/`\n`/`\r`/`\t`/`\0`/`\u{HEX}` escapes; character literals decode to exactly one scalar; physical newlines, unsupported escapes, invalid scalars, and unterminated literals are errors | M2 | — | parse, compile-fail |
| String literal materializes `str`/`String` from expected type, defaults `str`; no general implicit `str`→`String` (`String.from` is explicit); replacing a `str` field is valid, mutating existing `str` contents is not | M6, M14 | — | compile-fail, run-pass |
| Fixed array `[T; N]` (`N` compile-time `usize`); `[a, b]` literal; `@vec[...]`/`@map{...}`/`@set{...}` compiler macros are the only macro forms, `@name` namespace reserved, no user macros; macro names distinct from `Vec`/`Map`/`Set` type names | M3 (parse), M14 (lower) | — | compile-pass, run-pass |
| Literal elements/map entries evaluate left-to-right; must produce one exact type after materialization; empty literal needs an expected collection type; multiline trailing commas allowed; duplicate map key replaces, duplicate set element collapses | M6, M14 | — | compile-pass, run-pass |
| Arrays are ordinary fixed-size aggregates with recursive value copy; qualify `StableHash` when element does; `Vec.new()`/`Map.new()`/`Set.new()` for empty collections | M14 | — | run-pass |
| `Vec[T]` is the growable sequence type; `Vector` is not an alternate name | M14 | — | compile-fail (`Vector` usage) |
| `Map`/`Set` keys/elements require `StableHash` (structural inference: integrals/`bool`/`char`/`str`/`()` qualify; tuples/structs/enums qualify when every hashed field qualifies; `String`/`Vec`/`Map`/`Set`/`&T`/`&var T`/float never qualify) | M13 (inference), M14 (enforcement) | — | compile-fail (non-stable key) |
| `Map` values have no `StableHash` requirement; no collection API exposes an interior safe reference; `Identity[&T]`/`Identity[&var T]` are formed with `Identity[ReferenceType].from(reference)` and are compiler-known `StableHash` exceptions via managed target identity | M14 (done) | Boehm GC | run-pass (`identity_wrappers_are_stable_collection_keys`) |
| Array/`Vec` index type `usize`; value-context index copies independently; OOB traps, statically-known-OOB on an array is a compile error; mutable-path index is assignable (replace/compound/nested-mutate), never reference-formable; `Array.len()`/`.get()`, fixed length | M14 | trap path | run-pass, compile-fail |
| `Vec` API: `len`/`is_empty`/`get`/`append`/`insert`/`remove`/`clear`; `insert` index `0..=len`; `remove` requires index `< len`; invalid index traps; `remove` returns an owned copy | M14 | trap path | run-pass |
| `Map[K, V]` value-context index copies independently, traps if key absent; mutable-path index is assignable (needs existing key, traps if absent; `insert` for new keys); key args pass by ordinary copy; API: `len`/`is_empty`/`contains_key`/`get`/`insert`/`remove`/`clear` | M14 | trap path | run-pass |
| `Set` has no indexing; API: `len`/`is_empty`/`contains`/`insert`/`remove`/`clear`; ordinary copy value args; `insert` returns "newly added", `remove` returns "was present" | M14 | — | run-pass |

### 4.2 Structs

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| `struct` = aggregate value type; fields before methods in the body; inherent methods in the same body (no separate `impl` block); fields and inherent methods share one member namespace | M3 (parse), M4 (namespace) | — | compile-fail (name collision, field-after-method) |
| `Self` = enclosing type; five legal `self` forms — `Self`, `&Self`, `&var Self`, `*Self`, `*var Self` — are the *only* permitted types for a parameter named `self`; same forms available to trait methods | M11 | — | compile-fail (sixth form), compile-pass |
| Bound call adapts only its receiver: `self: Self` copies the evaluated-once receiver (need not be addressable, source stays valid); `self: &Self`/`&var Self` on a reference receiver auto-dereferences+copies | M11 | — | run-pass |
| `self: &Self`/`&var Self` on an addressable value auto-borrows `&value`/`&var value`; a matching reference receiver passes directly; never upgrades `&T`→`&var T`; never applies to non-receiver arguments | M11 | — | compile-fail (immutable receiver to `&var self`), run-pass |
| `value.name(args)`: field lookup first — if `name` is a field, call its value (must be callable) with no receiver adaptation; otherwise bound-method lookup; a field takes precedence over a same-named trait method in scope; explicit trait qualification reaches the shadowed trait method | M11 | — | run-pass (`IntTransform.apply`), compile-fail (non-callable field call) |
| `self: *Self`/`*var Self` bound call requires an already-exact-matching raw-pointer receiver, passed unchanged (no borrow/cast/downgrade/deref/null-check); calling doesn't itself access the pointee; body deref still needs `unsafe:`; calling an `unsafe` method still needs `unsafe:` at the call site; raw-pointer receiver never adapts to `self: Self` | M11, M16.3 (done) | — | compile-fail (mismatched pointer type), run-pass (`unsafe_methods_and_references_follow_the_demo_region`) |
| Struct literal `Type{field: expr, ...}`; any order, each field exactly once; `Type{field}` shorthand; multiline trailing comma; no record-update/spread | M3, M6 | — | compile-pass/fail |
| Recursive struct/enum containment must cross explicit indirection (`&T`/`&var T`/`*T`/`*var T`); generic wrappers and transparent aliases don't break a cycle; hidden managed/COW storage doesn't count | M5, M6 (containment check) | — | compile-fail (`Node{next: Option[Node]}`), compile-pass (`Chain[T]` via `&Chain[T]`) |

### 4.3 `Default` derivation and initializers

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| Derive list in parens after name+generics; nonempty comma list of compiler-supported traits; duplicate entries invalid; user traits implementable normally but no custom derive; derived impl has no separate visibility modifier | M3 (parse), M13 (derive) | — | compile-fail (dup derive entry) |
| `Default`: `fn default() -> Self`; struct derive supplies it per-field, valid only when every field implements `Default`; generic struct's derived impl is conditional on used field types and adds no bounds to the declaration; struct-only — enums implement manually, no implicit variant | M13 | — | compile-fail (field without `Default`), run-pass |
| `new` is an ordinary associated-function name, not a keyword or allocation expression; may call `default()` | M4, M11 | — | compile-pass |
| Standard defaults: zero numerics, `false`, U+0000 `char`, `()`, empty `str`/`String`/`Vec`/`Map`/`Set`, `null` for both raw-pointer types; tuples default fieldwise; `Option[T]` defaults `None` without requiring `T: Default` | M13, M14 (done) | — | run-pass (`option_defaults_to_none_through_an_explicit_discriminant`, `array_and_collection_empty_apis_return_typed_values`) |
| Safe references and function references don't implement `Default`; a struct with a direct safe-ref field can't derive it, while `Option[&T]` can (defaults `None`); ordinary enums don't derive `Default` | M13, M14.1 | — | compile-fail (`option_defaults_to_none_without_a_payload_default`) |

### 4.4 Enums, optionals, and aliases

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| Enums are tagged unions (unit/tuple/record-like variants); `Option[T]` = possibly-absent; no trailing optional-type syntax; struct+enum containment checked together under the same cross-indirection rule | M3, M5, M6, M14.1 (`Option` is an ordinary generic enum declaration) | — | compile-pass/fail, run-pass (`executes_option_construction_matching_and_payload_copies`) |
| Record-like variant `Variant{field: Type}`, constructed `Enum.Variant{field: value}`, same field rules as a struct literal | M3, M6 | — | compile-pass |
| Module-level `type` alias is transparent; generic params/args use `[]`; an alias declares only the parameters that remain variable, may fix others | M4, M5 (alias expansion) | — | compile-pass (`NameMap[V]`) |

### 4.5 Equality, ordering, and hashing

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| `PartialEq`/`Eq`/`PartialOrd`/`Ord`/`Hash` compiler-known, manual or derived; `==`/`!=` use `PartialEq`, `<`/`<=`/`>`/`>=` use `PartialOrd`; `Eq` = equivalence, `Ord` requires `Eq`+`PartialOrd`+total order; manual impls responsible for the laws | M13 | — | compile-fail (missing bound) |
| Derived comparison is structural: tuple/struct fields in declaration order; enum variants by declaration order then payload; `Vec` lexicographic; `Map`/`Set` equality ignores order, no relational order; `str`/`String` compare exact codepoints, no normalization | M13, M14 | — | run-pass |
| Float `PartialEq`/`PartialOrd` is IEEE (NaN unordered), no `Eq`/`Ord`/`StableHash`; integral/`bool`/`char`/unit/`str` give total eq+ord+hash; other aggregates are conditional on component capabilities | M13 | — | compile-fail (float as `Map` key) |
| Safe refs compare storage identity; trait-object refs compare concrete target identity; raw pointers compare address (incl. `null`); refs/trait-object-refs/raw-pointers have no relational order; function refs compare target-function identity (§5); content comparison is explicit (`*left == *right`); indirection-crossing + identity-compare makes derived structural equality terminate for recursive values | M11, M13 | — | run-pass |
| `StableHash` = compiler-proven stable structure + built-in/derived `Eq`+`Hash`; manually-implemented-equality/hashing types don't qualify initially; `Identity[&T]`/`Identity[&var T]` give `Eq`/`Hash`/`StableHash` via managed address | M13, M14 | Boehm GC | compile-fail, run-pass |

## 5. Functions and function references (§5)

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| Named function params need name+type; return type after `->`, omit for unit; non-unit function needs explicit `return expr` on every reachable path; no implicit tail-expression return; bare `return`/fallthrough only for unit functions | M6, M7 (return-path analysis) | — | compile-fail (missing return), compile-pass |
| No overloading: one function per name per namespace regardless of signature; generics/distinct names are the alternative; doesn't decide inherent-vs-trait collisions | M4 | — | compile-fail (duplicate name) |
| Safe/unsafe function references are `&fn`/`&unsafe fn`; general raw function pointers are `*fn`/`*unsafe fn`; both are directly callable, but every raw call requires `unsafe:` and traps on null | M11 (references), M17 (raw function pointers, done) | null trap | run-pass (`raw_function_pointers_are_general_and_directly_callable`), compile-fail |
| Exact function references explicitly convert to matching raw function pointers; function and data pointer domains never cast between each other | M17 (done) | — | compile-pass/fail (`rejects_invalid_c_contracts_and_unsafe_calls`) |
| No default parameter values; every non-variadic call needs the exact declared arity | M6 | — | compile-fail (arity mismatch) |
| Variadic final parameter `name: ...T`: 0+ trailing `T` args bound as `[T]`; homogeneous, only once, final position; type marker preserved (`&fn(i32, ...String) -> ()`); lowered as a slice argument, not C's variadic ABI | M3, M6, M8 | — | compile-pass, run-pass |
| Function value = *function reference*: safe `&fn(P) -> R`, unsafe `&unsafe fn(P) -> R`; bare `fn(...)`/`unsafe fn(...)` types are inhabited only behind a reference (like a trait); no `&var fn`; **no closures, anonymous function literals, captured environments, or bound-method values**; a reference is produced only by naming a named function or unbound method, with the matching safety qualifier | M5 (types), M11 (semantics) | — | compile-fail (bare `fn` type as a value) |
| Referencing a named function (`let bump = increment`) produces an `&fn` value; call syntax auto-dereferences it | M11 | — | run-pass |
| Referencing an `unsafe` function/method produces `&unsafe fn`; taking/storing/copying/passing/returning/comparing it is safe (no invocation); calling requires `unsafe:` | M11, M16.4 (done; the one call gate covers direct, bound, unbound, dynamic, and indirect forms) | — | compile-fail (call without `unsafe:`, `&unsafe fn` to `&fn`), run-pass |
| Function reference is storable in a binding/field/enum payload/collection/parameter/return; two references are compatible only on exact match of params+return+arity+variadic marker+safety qualifier; safe↔unsafe never converts implicitly; no variance; collections homogeneous by exact type | M11, M12 | — | compile-fail (safe/unsafe mismatch) |
| A named function has a stable whole-program address, so its function reference is always valid and never needs escape promotion; it carries no captured environment | M10 (no promotion path needed), M11 | — | run-pass |
| A generic function becomes a function reference only once all type arguments are determined; no erased any-callable type, dynamic call-operator, runtime signature inspection, or heterogeneous function-value collection; trait-object dispatch is separate and not directly callable | M12 | — | compile-fail |
| Selecting a method from a *type* yields an unbound function reference; selecting from an *instance* does not (`session.stop` alone is invalid; direct call is fine); an unbound method retains its receiver parameter and safety qualifier; trait-qualified selection follows the same rule | M11 | — | compile-fail (`session.stop` without a call), run-pass |
| A function reference carries no state, so a stateful callback uses `&Trait` instead (data in a struct, trait impl, dispatch through the reference) | M13 | — | run-pass (`Transform`/`AddOffset` demo) |
| Named functions may call themselves and other named functions in the same lexical scope — this is how recursion is expressed, since there are no closures | M4 (predeclare for recursion) | — | run-pass (`is_even`/`is_odd`) |
| Function-reference `==`/`!=` is reference identity (equal iff naming the same function); no behavior comparison; no relational order | M11 | — | run-pass |

## 6. Generics and traits (§6)

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| Generic declarations use `[]`; inline trait/capability bounds combined with `+`; `StableHash` usable as a bound though unimplementable by users; no `where` clauses, default type args, const generics, or HKTs initially | M5, M12 | — | compile-pass/fail |
| A call infers all generic arguments from argument types + expected result type only when unique; otherwise every argument must be explicit (no partial explicit lists); struct/enum literals infer the same way | M12 | — | compile-fail (ambiguous/partial inference), compile-pass |
| A generic body is type-checked once against its bounds only; constructed generic types have exact identity, no subtype/variance; the C backend monomorphizes per concrete instantiation; finite mutually-recursive instantiation sets are valid, unbounded expansion (`T`, `Vec[T]`, `Vec[Vec[T]]`, …) is rejected | M12 | — | compile-fail (unbounded expansion), run-pass |
| `trait`/`impl Trait for Type`; bodyless method = required, with-body = default; an impl must supply every required method with the exact signature, may override defaults, may not add extra methods; traits initially have methods only (no associated types/constants) | M13 | — | compile-fail (missing/extra method, signature mismatch) |
| Concrete/monomorphized calls use static dispatch; a trait object is `&Trait`/`&var Trait` and appears only behind a safe reference; bare trait-object values/raw pointers to trait objects are invalid; where that exact trait-object type is expected, a concrete safe reference of matching mutability converts automatically when its target implements the object-safe trait; `reference as &Trait` remains available explicitly; this conversion introduces no general reference subtyping or variance; the object is a fat reference (target + vtable) | M6 (reference shape + mutability), M13 (implements + object safety and coercion) | — | compile-fail (bare trait value, mutability mismatch, non-reference source, missing impl), run-pass |
| A trait has no value representation: a trait name is a type only as a safe-reference target, a generic/impl bound, or the trait of an `impl Trait for Type`; a bare trait name in a field, parameter, return, local annotation, alias, or generic argument is an error, as is `*Trait` | M5 | — | compile-fail (bare trait in each value position) |
| Object safety: every object-reachable method needs `&Self`/`&var Self`, no method-level generics, no other `Self` mention; a failing trait remains usable with static dispatch only; a generic trait needs concrete type arguments to form an object; default methods participate in the vtable | M13 | — | compile-fail (non-object-safe trait as a trait object) |
| Trait-object calls dispatch through the vtable; heterogeneous concrete types coexist in e.g. `Vec[&Trait]`, with each concrete reference converted against the expected element type; no downcasting/runtime-type-inspection/multi-trait objects initially; safe-reference reachability/escape promotion applies to concrete targets | M13 | Boehm GC | run-pass |
| `pub trait` exposes all methods where the trait is accessible; trait method declarations and impl methods carry no separate `pub` modifiers | M4, M13 | — | compile-fail (`pub` on a trait method) |
| Bound-call lookup considers inherent methods + in-scope-trait methods; inherent wins over a same-named trait method; multiple matching in-scope traits are ambiguous | M11, M13 | — | compile-fail (ambiguity) |
| `Type.Trait.method` unconditionally selects the named impl member, bypassing field selection, inherent-method lookup, and bound trait lookup; valid only when accessible and the trait is implemented for the type; unbound, retains its receiver parameter; selecting without calling yields an unbound function reference; also selects receiverless trait functions | M11, M13 | — | run-pass (`Session.Toggle.status`), compile-fail (trait not implemented) |
| Orphan rule: an impl is declared only in the package defining the trait or the outermost nominal target type; only one impl of an instantiated trait per concrete target type program-wide; generic impls (`impl[T: Bound] Trait for Wrapper[T]:`) are invalid when any concrete substitution could satisfy both; no specialization or negative impls | M13 | — | compile-fail (orphan violation, overlapping impls) |
| `Self` in a trait declaration = implementing type; in `impl Trait for Type` = `Type`; invalid outside a struct body, trait declaration, or trait impl | M4, M5 | — | compile-fail |

## 7. Expressions and control flow (§7)

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| `if`/`else`/`match`/`for`/`while` use indentation bodies, condition after keyword; `match` evaluates the scrutinee, chooses the first matching arm | M3 (parse), M7 (static checks), M8 (`if`/`while` lowering), M9 (`match` lowering), M14 (`for` lowering) | — | compile-pass, run-pass |
| Patterns (match-arm only): `_`, immutable binding names, primitive/`str` literals, tuples, structs, unit/tuple/record enum variants, `\|` alternatives; struct/record-variant patterns use named fields; `Point{x, ..}` shorthand binds `x` and ignores the rest, without `..` every field is required; alternative patterns must bind identical names+types | M3 (parse), M7 (typecheck), M9 (lowering) | — | compile-fail (mismatched alt bindings, missing field without `..`), run-pass |
| Guarded arm `Pattern if cond:`; bindings in scope for the guard; a failed guard proceeds to the next arm; guarded arms don't count toward exhaustiveness; pattern bindings are independent copies behaving as `let`; matching a reference does not auto-dereference (`*reference` for content matching) | M7 (static checks), M9 (lowering) | — | compile-fail/run-pass |
| Every `match` is exhaustive; infinite-domain patterns need a catch-all/`_`; arms test in source order with no fallthrough; a statically unreachable arm is a compile error | M7 (exhaustiveness/usefulness/reachability) | — | compile-fail (non-exhaustive, unreachable arm) |
| Control-flow constructs/`unsafe` blocks/indented bodies are statements, not expressions; assignment/compound assignment are statements, cannot nest in expressions; no `++`/`--`; compound assignment evaluates its destination exactly once | M6, M7, M8 | — | compile-fail (`if` used as an expression), run-pass (destination evaluated once) |
| Left-to-right expression evaluation; a call evaluates callee/receiver then arguments in source order; `&&`/`\|\|` require `bool` and short-circuit; `!` boolean negation; unary `+`/`-` (numeric/signed+float), `~` (integer); arithmetic/bitwise ops are built-in, not overloadable; comparisons use §4.5 traits; `%` integer-only; shift count must be an unsigned integer smaller than the left operand's bit width; chained comparisons (`a < b < c`) are invalid | M6, M7, M8 | — | compile-fail (chained comparison), run-pass (evaluation order) |
| 14-level operator precedence table (postfix highest … assignment lowest) | M3 (precedence-climbing parser) | — | parse (one test per level) |

### 7.1 Collection iteration

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| `for` initially supports only arrays/`Vec`/`Map`/`Set`, no user iteration protocol yet; the iterable evaluates once and copies into hidden loop state via ordinary value semantics; later source mutation cannot affect the active loop; COW allowed if unobservable | M7 (CFG for `for`), M14 (lowering + hidden state) | — | run-pass |
| Arrays/vectors iterate index order; maps yield `(K, V)` pairs, sets yield elements, order unspecified and may vary; each yielded item is independently copied into a non-rebindable binding; no interior references exposed; visits only direct elements, no recursive traversal through reference-like values | M14 | — | run-pass |

### 7.2 Formatted strings and display

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| `Display` compiler-recognized prelude trait with required `fmt(self: &Self, formatter: &var Formatter)`; `Formatter.write(str)` appends text; user-implementable; primitives/`str`/`String`/references-to-displayable/std collections-of-displayable provide impls | M13, M14 (done) | managed allocation | compile-fail (non-`Display` in `f"..."`), run-pass (`user_display_implementations_write_through_formatter`, `display_trait_objects_dispatch_through_the_formatter`) |
| `f"..."` produces immutable `str`; each `{expr}` evaluates once left-to-right, must implement `Display`; `{{`/`}}` are literal braces; unmatched braces are a compile error; no width/precision/positional/debug specifiers initially; braces in ordinary strings aren't special | M2 (lex), M6 (typecheck interpolations), M8 (lower to `Formatter` calls) | — | compile-fail (unmatched brace, non-`Display` value), run-pass |
| Prelude `print`/`println` are single generic `Display`-value functions, not heterogeneous variadics; combine multiple values via a formatted literal first | M14, M18 (done) | — | run-pass |

## 8. Errors and resource cleanup (§8)

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| `Result[T, E]`; postfix `?` valid only in a function returning `Result[U, E]` with the *exact* same error type; operand evaluates once; `Ok(value)` copies value as the postfix expression's value; `Err(error)` copies error and immediately returns `Result.Err(error)` | M15.1–15.3 (done) | — | compile-fail (`postfix_propagation_requires_matching_standard_result_types`), run-pass (`propagation_branches_evaluate_once_and_copy_payloads`) |
| `?` is the explicit exception to "return uses `return`"; no implicit error conversion (caller converts explicitly, e.g. via `match`); `Option[T]` uses `match`, not `?` | M15.2 (done) | — | compile-fail (`Option` operand, differing errors, non-`Result` operand and function) |
| No implicit destruction protocol; GC manages memory only; deterministic external cleanup uses lexical `defer` | M15 (done) | — | run-pass (defer suite) |
| There is no compiler-known cleanup trait or privileged method name; resource types expose ordinary safe unit-returning cleanup methods, and each API specifies its own idempotence, sharing, and error behavior; shared handle identity must be represented explicitly | M15 (done) | — | run-pass (`spec_demo_error_and_cleanup_regions_build_and_run`, `a_returned_shared_handle_observes_its_deferred_close`) |
| `defer call` registers one safe unit-returning call for block exit; `defer:` defers an indented block of statements as one registration, its body an ordinary lexical scope; both only in an executable body; registration occurs only when control reaches it; not a function value/closure, no captured environment, cannot escape its block | M3 (parse both forms), M15.4–15.5 (done) | — | compile-fail (`deferred_calls_must_be_safe_and_unit_returning`), run-pass (`deferred_execution_is_static_and_constructs_no_callable`: static per-edge expansion, no callable/environment value) |
| The deferred call evaluates at block exit using the callee/receiver/argument values *at that time* (not at registration); referenced bindings stay alive until the call finishes; reassigning a `var` after registration affects the later call | M15.10 (done) | — | run-pass (`deferred_bodies_read_execution_time_values`) |
| Deferred calls run on fallthrough/`return`/`?`-propagation/`break`/`continue`; one block's calls run in reverse registration order; an inner block's calls run before an enclosing block's; a return value/propagated error is evaluated and copied *before* deferred calls begin (so an unconditionally deferred `close()` on a returned resource closes the returned handle too); no `errdefer` | M15.6–15.9 (done; static per-scope cleanup plans expanded at every exit edge) | — | run-pass (`deferred_registrations_run_in_reverse_on_every_exit_edge`, `propagation_runs_cleanup_for_every_exited_scope`, `a_returned_shared_handle_observes_its_deferred_close`) |
| No `errdefer` and no conditional error-only deferral; a deferred block cannot redirect control, so `return`/`break`/`continue`/postfix `?`/nested `defer` are invalid inside it; a `defer` statement is invalid inside an `unsafe` block and an `unsafe` block is invalid inside a `defer:` block; a direct unsafe/foreign call cannot be deferred (wrap it in a safe unit method); an unrecoverable trap (including during deferred execution) or OOM doesn't guarantee remaining deferred statements run | M7 (placement rules), M15.5/15.11 (done; M17 re-applies the recorded rule to foreign calls) | trap path | compile-fail (each control-flow escape, nested `defer`, `defer` in `unsafe`, `unsafe` in `defer`, unsafe deferred call), run-pass (`traps_terminate_without_promising_remaining_cleanup`) |
| Leaving a scope does not implicitly call a resource-cleanup method; only an explicit `defer`/direct call runs cleanup; GC never calls cleanup methods; an un-deferred resource may leak; an implementation may warn on provable local leaks (not required to be complete) | M15 (done; no leak warning implemented, none required) | — | run-pass (leak is not a requirement) |

## 9. Garbage collection (§9)

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| Boehm non-moving GC for managed memory; stack-vs-heap placement unobservable in safe code; escape promotion preserves safe-ref behavior + `Identity` identity; a managed allocation never moves once created | M10 | Boehm GC | run-pass |
| Strong roots: every local binding until scope end, function parameters for the complete call, temporaries until their full expression finishes, module-level values, safe references, managed handles in structs/enums/collections/hidden loop state; assigning a new value to a `var` removes the binding's strong path to the old value (other strong paths remain effective) | M10 (root tracking) | Boehm GC | run-pass (liveness tests) |
| Reachable safe references are roots for managed targets; a reference constructed from a raw pointer roots the target only if managed; it doesn't extend foreign/manual storage lifetime | M10, M16 | Boehm GC | run-pass, doc-contract |
| Raw pointers are *not* language roots; code retaining one is responsible for a separate managed path; Boehm may conservatively over-retain via bit-pattern resemblance but a program cannot rely on it | M10, M16 | Boehm GC | doc-contract, best-effort test |
| Cycles without a strong-root path are unreachable and collectible; collection timing is unspecified and not guaranteed before exit; unreachable storage may persist indefinitely; collection timing/memory usage are not deterministic program behavior | M10 | Boehm GC | run-pass (`runtime_stress_is_stable_across_repeated_debug_and_release_runs` constructs a managed cycle); reclamation timing remains intentionally unasserted |
| No `Weak` type, GC finalizers, implicit destruction, or user collection callbacks; GC never invokes resource-cleanup methods; internal runtime reclamation must invoke no user code and cause no observable cleanup behavior | M10, M15 | Boehm GC | design constraint (no such syntax to test) |
| Managed allocation failure is unrecoverable (copy/COW-mutation/escape-promotion may allocate implicitly); OOM path attempts a full collection, retries the allocation, then terminates with an OOM diagnostic only if the retry fails; OOM is not `Result`-represented, uncatchable, and runs no cleanup; safe allocation never produces `null` | M10 (OOM path; retry closure audited at M19) | Boehm GC | generated-C integration (`managed_storage_engages_the_collector_prelude_and_link_inputs` asserts collect → retry → terminal ordering); forced host OOM remains nondeterministic |
| No portable collection-control/heap-stats API initially; an implementation may offer nonportable diagnostic flags without establishing stronger guarantees or changing language-visible values | M18, M20 | Boehm GC (nonportable extensions) | optional tooling, not required |

## 10. Unsafe operations and C interoperability (§10)

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| `unsafe` function declaration means every caller must satisfy documented preconditions, requiring `unsafe:` at the call site; the body is not implicitly an unsafe context — internal unsafe-only operations still need nested `unsafe:` blocks; referencing (not calling) an unsafe function/method is safe and yields `&unsafe fn` preserving the call-site requirement | M16.3 (done; lexical `unsafe:` depth is the only unsafe context) | — | compile-fail (unsafe call without a block, including inside an `unsafe fn` body), compile-pass (safe reference to an unsafe fn) |
| Unsafe-ness depends only on the caller contract, not on whether the body contains `unsafe:` or where `return` appears; a safe function may use unsafe-only operations internally if it establishes every obligation itself; a function *must* be `unsafe` whenever sound use requires the caller to establish an obligation the signature doesn't express | M16 | — | documentation-only obligation on authors — largely unenforceable by the compiler |
| `unsafe:` block permits raw dereference/raw→ref conversion/unsafe-or-foreign calls; doesn't disable type checking or prove validity; the author asserts the documented preconditions and §3.3 obligations hold | M16.3 (done) | — | compile-fail (`unsafe_only_operations_require_a_lexical_unsafe_block`) |
| An unsafe-only op outside the required context is a compile-time error; §3.3's expression-local constant rule is the *only* mandatory value analysis for raw-pointer access; broader-analysis violations are warnings only; inability to prove valid foreign input is not itself an error; these diagnostics don't fire merely because a safely-formed local reference escapes (it's promoted instead) | M16.3/16.7 (done; no broader analysis implemented, none required) | — | compile-fail/compile-pass (`pointer_validity_is_expression_local`) |

### 10.1 Foreign declarations and ABI types

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| C ABI only initially; `@importc("c_name", "header.h")` marks a module-level bodyless function, opaque type, or foreign struct; generated C includes the authoritative header and uses the exact C spelling while the local Elamite name may differ; manifest include/library paths, libraries, and link options feed deterministic native commands; import has no runtime effect | M17 (done) | C toolchain | compile-pass, integration (`imports_c_symbols_through_attributes`, callback include-path harness) |
| Opaque foreign type: unknown size/align, usable only behind a raw pointer; foreign struct matches the header type's target C layout, is an ordinary copyable value with ABI-safe fields, can't be generic/derive/have methods/contain an incomplete opaque type directly; mismatch is an unsafe contract violation (UB at the boundary) | M17 (done) | C toolchain | compile-fail (generic foreign struct), integration (`imported_c_structs_use_header_layout_and_field_names`) |
| ABI-safe scalars: `i8`–`i64`, `u8`–`u64`, `isize`/`usize`, `f32`/`f64`; also raw data pointers, foreign structs of ABI-safe fields, and `*fn`/`*unsafe fn` pointers with ABI-safe signatures; `()` only as a return type (lowers to `void`); fixed-width ints use `stdint.h`; `isize`/`usize` use `intptr_t`/`uintptr_t`; `std.ffi.CVoid` is raw-pointer-only C `void` | M17 (done) | C toolchain | compile-pass/fail, callback integration |
| Not ABI-safe: `bool`/`char`/`str`/`String`/safe refs/function refs/bare function types/tuples/arrays/ordinary structs+enums/trait objects/std collections/`i128`/`u128`; no implicit marshalling — wrappers explicitly encode text+terminator, pass raw pointer+length, and keep/register backing storage for the foreign-access duration | M17 (done) | — | compile-fail (`rejects_invalid_c_contracts_and_unsafe_calls`) |
| Every imported foreign function is unsafe to call even with scalar-only signatures, is bodyless, and cannot use Elamite variadic syntax; no C variadics initially; every raw-pointer parameter/result needs a documented foreign contract that the compiler does not infer from a C header | M17 (done) | — | compile-fail and integration |

### 10.2 Ownership, retention, and managed roots

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| A raw pointer never owns its target; receiving/passing one never schedules or transfers cleanup by itself; an owning C API is wrapped in an Elamite handle whose ordinary methods enforce the API state and may include an idempotent `close()` that invokes native release; shared copies represent their identity explicitly; borrowed foreign pointers are valid only per the foreign contract's duration | M17 (done) | — | integration |
| Safe references are not ABI-safe and cross the boundary only as an explicit raw-pointer conversion; a non-retaining foreign call keeps raw pointer arguments alive for the call; retained-after-return storage needs `std.ffi.ForeignRoot[T]`/`ForeignRootMut[T]` registration | M17 (done) | Boehm GC | integration |
| `ForeignRoot.retain`/`ForeignRootMut.retain` promote the target if needed and create a runtime root registration; `.pointer()` returns `*T`/`*var T`; copies share one explicit registration; `.close()` is idempotent, closing any copy unregisters, and `.pointer()` on a closed handle traps with `E-RUN-CLOSED`; unreachable-without-close handles may leak | M17 (done) | Boehm GC root registration | run-pass (`foreign_root_copies_share_one_idempotent_registration`, `a_closed_foreign_root_reports_a_stable_runtime_error`) |
| Closing a registration is valid only once the foreign contract says no later access will occur; a raw pointer returned *by* foreign code does not root foreign storage, and converting it to a safe reference does not extend the foreign lifetime | M17 (done) | — | doc-contract, integration |

### 10.3 Callbacks and foreign control flow

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| `@exportc("c_name")` gives an ABI-safe module-level function definition an exact unmangled link symbol without creating a separate function kind; any exact ordinary `&fn`/`&unsafe fn` can explicitly become a raw callback pointer | M17 (done) | — | C harness (`a_c_harness_calls_an_exported_elamite_function`), callback run-pass |
| Foreign code may retain a callback pointer indefinitely; retained managed callback state passes through a raw context pointer backed by an open `ForeignRoot` registration; the callback recovers a reference only within `unsafe:`; the registration must stay open until both callback and context pointer are released per the foreign API's contract | M17 (done) | Boehm GC | integration (`a_foreign_callback_can_use_registered_managed_context`) |
| Until concurrency is specified, C callback entry is supported only on the OS thread that initialized the runtime; direct/nested and later same-thread callbacks are allowed; foreign-created-thread/concurrent invocation is UB and is not generally compiler-detectable; broader threading is deferred to [I-015](ISSUES.md#i-015-concurrency-and-asynchronous-execution) | M17 (documented) + I-015 gate | — | integration, doc |
| Recoverable Elamite errors don't cross the ABI automatically (wrapper translates to a status code/out-params); `errno`/foreign error channels are observed only through explicit wrapper ops; a trap during foreign code/callback terminates the process without unwinding through C; foreign unwinding (C++ exceptions, `longjmp`) across an Elamite frame is forbidden and is UB | M17 (done) | trap path | integration/documentation |

## 11. Conformance example (§11)

| Rule | Pass | Runtime | Tests |
| --- | --- | --- | --- |
| The `Counter`/`main` example (value-copy vs. `&var`-mutated alias) must build and run with the specified output once the compiler exists | M19 (done) | Boehm GC, C toolchain | run-pass (`11_example`; authoritative `examples/spec_demo` package) |

---

## 12. Compiler-known entity catalog

Entities the compiler must recognize intrinsically (structural rules, derive
support, or lowering hooks), versus entities that can be ordinary library code
once the intrinsic hooks exist. The completed Milestone 18 inventory is
enforced exactly by `src/standard.rs` and its resolution tests; compiler
knowledge should stay minimal, and this column records why each entity needs
any compiler awareness at all.

| Entity | Role | Why the compiler must know it | Pass |
| --- | --- | --- | --- |
| `bool`, `char`, integer and floating-point primitives | scalar values | fixed C representation, contextual literal materialization, checked operators, target-width behavior for `isize`/`usize`, and compiler-supplied trait capabilities | M5–M18 (done; exact inventory in `src/standard.rs`) |
| `Option[T]` | possibly-absent value | `Default` special-cases it (§4.3). Otherwise ordinary: M14.1 collects it from compiler-supplied source into the `std` root module, so construction, matching, copying, exhaustiveness, and monomorphization are the generic-enum rules and the only intrinsic left is the unconditional `Default` | M13, M14.1 (done) |
| `Result[T, E]` | recoverable error | postfix `?` and `defer`/return-copy ordering are compiler-level control flow (§8). Otherwise ordinary: M14.2 collects it from compiler-supplied source, so construction, matching, and copying are the generic-enum rules and it carries no intrinsic trait capability. The propagation role keys on the standard declaration's identity (`standard_result_payloads`), so a shadowing user `Result` receives nothing | M14.2 (values), M15.1 (`?` role) — done |
| `Vec[T]`, `Map[K, V]`, `Set[T]` | standard collections | `@vec`/`@map`/`@set` macro lowering, `StableHash` enforcement, mutable-place vs. reference rules (§3.2, §4.1) | M14 |
| `String`, `str` | text types | literal contextual materialization, `StableHash` asymmetry, ABI-unsafety | M14 |
| `Default` | derivable trait | derive-list special case (§4.3), per-type standard defaults | M13 |
| `PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Hash` | comparison traits | operator desugaring (`==`, `<`, …), structural derivation | M13 |
| `StableHash` | compiler-controlled capability | structurally inferred, not implementable via ordinary `impl` (§4.1, §4.5) | M13 |
| `Display`, `Formatter` | formatting | `f"..."` string lowering, `print`/`println` bound | M13, M14 |
| `print`, `println` | standard output | backend output hook after ordinary `Display` checking; exported through both the exact prelude and `std.io` surfaces | M14, M18 (done) |
| `Identity[&T]`, `Identity[&var T]` | identity-keyed wrapper | compiler-known `StableHash` exception via managed address (§4.1, §4.5) | M14 |
| `std.ffi.ForeignRoot[T]`, `ForeignRootMut[T]` | GC root registration | runtime root-table integration and explicit shared registration state (§10.2) | M17 |
| `std.ffi.CVoid` | opaque FFI type | fixed correspondence to C `void`, raw-pointer-only usage (§10.1) | M17 |
| `NumericError` | checked-conversion error | return type of `Type.try_from` (§4.1). An ordinary compiler-supplied enum since M14.3; `OutOfRange`/`NotANumber` are chosen by the implementation, since the specification fixes the role but not the variants | M14.3 (done) |

## 13. Target assumptions

Per `ROADMAP.md` Milestone 0, the initial supported-target assumptions:

| Assumption | Value | Status |
| --- | --- | --- |
| Supported OS/architecture matrix | **Decided: Linux, two architectures — x86-64 (primary) and x86 (32-bit).** Distro version is intentionally left unpinned; both architectures are in scope from M1 onward, not just the primary one | fixed — M1 target-kind validation and M19's target matrix cover both |
| Pointer width | `isize`/`usize` match the selected C target's pointer width (§4.1). With two supported architectures this means **two** pointer widths in scope (64-bit on x86-64, 32-bit on x86) — the backend must not hardcode a single width | fixed by the target-matrix decision above; M8/M16 numeric-boundary code must be tested at both widths |
| Integer representations | Fixed-width types map to `stdint.h` types; `isize`/`usize` map to `intptr_t`/`uintptr_t` (§10.1) | fixed by spec |
| C compiler requirement | **Decided: C99.** `stdint.h` fixed-width types and `stdbool.h` (§10.1, §4.1) are both available. The C backend (M8) and foreign declarations (M10.1) must not emit or require C11-only features (`_Static_assert`, `_Generic`, anonymous struct/union members) — tagged unions for enum lowering must be hand-rolled with a discriminant field rather than relying on C11 anonymous unions | fixed — M8/M10.1 must conform |
| Boehm GC availability | Assumed available on Linux via a distro-provided Boehm GC development package (e.g. `libgc-dev` on Debian/Ubuntu-family systems), for both x86-64 and x86; the manifest's native-library/link-option mechanism (§2.3, §10.1) is the same path used to supply it | target OS/arch fixed above. **Decided (M10): the dependency is demand-driven** — `ManagedMemoryStrategy` contributes its native libraries, and the backend engages the collector prelude, entry-shim initialization, and link inputs only when lowering produced managed storage, so programs needing no managed storage keep a collector-free translation unit. System package vs. bundled build remains open |
| Native library supply | Declared in `elamite.toml` (include/library paths, native libraries, link options); consumed without executing code (§2.3, §10.1) | implemented in M17 |

## 14. Command-level outcomes

Per `ROADMAP.md` Milestone 0, exact command spelling is a tooling decision; these
are the outcomes the initial driver must support, independent of spelling:

| Outcome | Description | Pass |
| --- | --- | --- |
| Check a package | Run resolution + type checking without generating C or linking; report diagnostics | M4–M7 |
| Build a package | Full pipeline through C generation, C-compiler invocation, and linking; produce an executable or library artifact | M8 (initial), M17–M18 (complete) |
| Initialize a package | Create a non-destructive hello-world executable skeleton by default (`src/main.elx`) or a library skeleton with `--lib` (`src/lib.elx`) | M18.6 (done) |
| Print diagnostics | Stable diagnostic category, primary span, plain-language explanation, related spans (§2.3 of `ROADMAP.md`) | M4 onward |
| Select a target | Choose the target architecture/OS the generated C is compiled for | M1 (manifest), M8 (backend) — depends on the open target-matrix decision in §13 above |
| Select an output directory | Choose where build artifacts (generated C, object files, linked binary) land | M8, M18 (done) |
| Dump intermediate representations | Tokens, syntax tree, resolved declarations, typed IR, control-flow IR, monomorphization results, generated C | M18 (done) |
| Extract public documentation | Emit attached Markdown, public signatures, and source links without requiring unrelated private bodies to check | M18 (done) |
| Run conformance fixtures | Select fixtures and target/optimization matrices, compare stable output/status expectations, isolate builds, and retain failure artifacts | M18 (done) |

## 15. Concurrency: explicitly unsupported

Per `ROADMAP.md` §7 and [I-015](ISSUES.md#i-015-concurrency-and-asynchronous-execution),
no task syntax, scheduler, channel, synchronization trait, or cross-thread
callback exists in the initial language, and none should be implemented until
I-015 resolves the questions listed there. Concretely:

- No lexical or grammatical form in `SPEC.md` reserves task/async/await
  syntax — there is nothing to parse and nothing to mark "unsupported" in the
  grammar; the absence is the marker.
- §10.3's callback thread restriction (foreign invocation only on a thread
  already executing Elamite code) is the *only* concurrency-adjacent rule in
  the initial language, and it is a restriction, not a capability — see the
  §10.3 table above.
- When I-015 is resolved, concurrency is added as a new vertical slice per
  `ROADMAP.md` §7 (syntax/resolution → static transfer checks → CFG lowering →
  runtime scheduler/synchronization → GC integration → cancellation cleanup →
  foreign-thread entry → stress/race testing), not as incremental patches to
  the milestones above.

## 16. Demonstration coverage

Every construct in `examples/spec_demo.elx` maps to a section of this ledger:

| `spec_demo.elx` region | Ledger section(s) |
| --- | --- |
| `use`, `mod`, `pub`, re-exports, `type` alias | §2.3, §4.4 |
| `Point(Default, PartialEq)`, `MyType`/`MyBetterType` derive examples | §4.2, §4.3 |
| `Session` — five `self` receiver forms and `Toggle` trait | §4.2, §6 |
| `Packet`, `DemoResourceState`, `DemoResource`, `use_demo_resource` (`defer call`), `use_demo_resource_block` (`defer:` block) | §4.2, §8 |
| `Address`, `Account` — reference into an aggregate observing container replacement and caller-visible mutation | §3.2, §19 |
| `IntTransform`, `apply_offset`, `increment`, `Transform`/`AddOffset`, `apply_transform` | §5, §6 |
| `Chain[T]`, `chain_length` | §4.2, §6, §9 |
| `State` enum | §4.4 |
| `equivalent[T: PartialEq]`, `is_even`/`is_odd` | §4.5, §5, §6 |
| `propagate_io` (`?`) | §8 |
| `main`: raw pointer round-trip (`*Self`/`*var Self`, unsafe receiver), `ptr == null` unreachable-branch example, arrays/`Vec`/`@map`/`@set`, `for`, `while`, f-strings | §3.3, §4.1, §7, §7.1, §7.2 |

Every top-level and `main`-body construct in the demonstration has at least
one corresponding row above; there is no construct in the demonstration
without a ledger entry.

## 17. Milestone 0 exit criteria

Per `ROADMAP.md`:

- [x] **Every construct in the authoritative demonstration has a ledger
  entry.** See §16 above.
- [x] **Known disagreements between old implementation artifacts and the
  current specification are either removed from planned behavior or recorded
  as a test-migration task.** There are no old implementation artifacts left
  to disagree (§0.1) — the prior Python/Lark implementation was deleted, not
  reconciled.
- [x] **The specification can be mapped without claiming the compiler is
  implemented.** When these criteria were met no row claimed anything was
  built—every row's status was "planned." Rows now reach "complete" only as
  their owning milestone's status changes in `ROADMAP.md`; see §0.

Both tooling gaps surfaced by this exercise are now decided (§13): the C
compiler requirement is **C99**, and the supported OS/architecture matrix is
**Linux, x86-64 and x86**. Neither was a pre-existing `ISSUES.md` design
question — both were implementation/tooling decisions recorded here so
Milestone 1 didn't have to silently invent an answer.

### 17.1 Milestone 1 exit criteria

- [x] `elamite.toml` loads package metadata, target kind, default or custom
  root, local path dependencies, native libraries, and link options.
- [x] Canonical manifest-directory paths provide stable initial package
  identities, including distinct identities for same-named packages in
  different directories.
- [x] File-backed modules are discovered deterministically and invalid path
  components are rejected.
- [x] The resolved graph retains manifest-alias-to-package-identity edges and
  rejects dependency cycles.
- [x] `SourceManager` owns decoded text, file IDs, line indexes, and
  span-to-line/column conversion.
- [x] Integration coverage includes every validation item in the revised
  Milestone 1 list.

Inline/file-backed module collisions remain a language requirement, but the
check belongs to Milestone 4 because inline module paths do not exist before
Milestone 3 parses source declarations.

### 17.2 Milestone 2 exit criteria

- [x] The hand-written lexer emits span-preserving identifiers, reserved
  keywords, punctuation, operators, numeric literals, decoded ordinary and
  character literals, structured formatted strings, documentation comments,
  and layout tokens.
- [x] ASCII identifier syntax is shared with manifest aliases and file-backed
  module paths; non-ASCII identifier characters are diagnosed.
- [x] Physical lines produce logical `Newline`, `Indent`, and `Dedent` events
  with four-space blocks, exact continuation indentation, body-colon
  precedence, delimiter-aware multiline expressions, and EOF dedents.
- [x] Ordinary comments and blank lines do not affect layout; documentation
  text and spans are preserved for Milestone 3.
- [x] Numeric bases, separator placement, suffix shapes, string/character
  escapes, formatted-string braces, and grouping delimiters are validated.
- [x] Lexical diagnostics recover sufficiently to retain later valid tokens.
- [x] Golden snapshots cover blocks, continuations, grouped multiline
  expressions, documentation, literals, and operators; negative tests cover
  every required failure class; a property test checks balanced layout; and
  the authoritative demonstration lexes without diagnostics.

### 17.3 Milestone 3 exit criteria

- [x] The hand-written parser produces a deterministic, token-preserving
  syntax tree whose nodes and tokens retain exact source spans; the public
  tree dump provides a stable debugging representation.
- [x] Declarations cover inline modules, imports and re-exports, aliases,
  structs, enums, traits, implementations, functions, foreign blocks and
  members, generics, derives, and attached documentation nodes.
- [x] Type syntax covers paths and applications, tuples, arrays, slices,
  safe references, raw pointers, safe and unsafe function references, foreign
  function pointers, and trait objects.
- [x] Executable syntax covers bindings, assignment and compound assignment,
  expression statements, every control-flow statement, unsafe blocks, and the
  single-call `defer` form.
- [x] The Pratt expression parser implements every precedence level, postfix
  forms, casts, literals, the three compiler-known collection macros, record
  construction, and parsed formatted-string interpolations while leaving name
  and type interpretation to later phases.
- [x] Match syntax covers bindings, primitive and string literals, tuples,
  record and variant patterns, alternatives, guards, field shorthand, and
  rest markers.
- [x] Recovery tests cover same-line, brace, and empty bodies, malformed
  generic lists and patterns, unknown macro forms, non-call `defer`, and every
  relational and mixed comparison chains with primary source spans.
- [x] Isolated syntax tests, two syntax-tree snapshots, and a malformed-input
  property test pass; the complete authoritative demonstration parses without
  diagnostics, and all 61 project tests pass under formatting, checking, and
  warning-denied Clippy validation.

The parser deliberately represents postfix square brackets structurally as
`BracketExpression`; Milestones 5 and 12 will distinguish indexing from
explicit generic arguments using resolved names and type context. This is a
phase boundary, not an accepted ambiguity in typed programs.

### 17.4 Milestone 4 exit criteria

- [x] `lasso`-interned symbols and owned tables assign stable IDs to modules,
  declarations and methods, impls, fields, variants, generic parameters,
  imports, compiler-provided names, parameters, locals, loop bindings, and
  pattern-binding candidates.
- [x] All file-backed, directory-namespace, and inline modules are created
  before declaration collection; inline/file-backed collisions and shared
  module/member namespace conflicts carry primary and related spans.
- [x] Module items and import slots are predeclared before paths or bodies are
  resolved, permitting declaration-order-independent direct and mutual
  recursion and legal circular imports within a package.
- [x] Absolute dependency paths and `root`, `self`, `super`, and `std` paths
  resolve independently from lexical lookup. Lexical scopes cover parameters,
  locals, nested bodies, loops, guards, and pattern bindings; nested modules do
  not inherit imports, while locals may shadow module items.
- [x] Imports, aliases, public re-exports, file-module re-exports, package
  privacy, externally reachable module paths, and unbroken public re-export
  chains are represented without changing the target's original identity.
  Resolved name occurrences retain the import/re-export spans that supplied
  their names.
- [x] Public functions, aliases, types, traits, public fields and methods, and
  public enum payloads are checked for less-visible types and bounds. Private
  fields and hidden public declarations do not become externally reachable
  accidentally.
- [x] Struct field-before-method ordering, shared field/method namespaces,
  duplicate functions regardless of signature, duplicate imports even to one
  target, trait-method visibility syntax, and `Self` scope are diagnosed.
- [x] Integration coverage exercises circular imports, re-exported modules and
  declarations, private cross-package access, hidden public declarations,
  inline/file collisions, illegal root-level `super`, duplicate namespaces,
  lexical shadowing, pattern/loop scopes, public API leakage, deterministic
  identity dumps, and direct/mutual recursion. The authoritative demonstration
  resolves without diagnostics; all 73 project tests pass under formatting,
  checking, and warning-denied Clippy validation.

Milestone 4 resolves lexical names, module paths, and the module portion of
member chains. Selection that depends on an expression's type—fields, inherent
or trait methods, enum variants, and associated functions—remains with
Milestones 5, 7, 11, and 13. Unqualified identifier patterns retain explicit
`PatternCandidate` binding identities until Milestone 7 can distinguish a
binding from a unit variant using the scrutinee type. Compiler-provided prelude
and `std` names have stable identities; Milestone 18 moved every expressible
standard declaration into `stdlib/` source and records the remaining intrinsic
boundary in `src/standard.rs`.

### 17.5 Milestone 5 exit criteria

- [x] One owned interning arena canonicalizes the error type, primitives,
  nominal and compiler-provided applications, tuples, arrays, slices, safe
  references, raw pointers, function signatures, trait objects, foreign
  opaque/complete types, generic parameters, `Self`, aliases, and inference
  variables.
- [x] Nominal identity includes the defining package instance, stable
  declaration identity, and exact canonical arguments. Generic substitutions
  recursively rebuild canonical compound types without introducing
  source-order or reference identity.
- [x] Transparent aliases retain their declaration and arguments for
  diagnostics while recursively expanded targets drive exact equivalence,
  substitution, and cycle detection. Alias cycles and wrong generic arity
  produce stable type-system diagnostics.
- [x] Strict structural unification binds inference variables and performs
  alias expansion, but provides no numeric conversion, subtyping, reference
  variance, function variance, safety conversion, or ABI conversion.
- [x] Function identity includes safety, Elamite-versus-C ABI, receiver type,
  parameter order, homogeneous variadic markers, and return type. Explicit and
  omitted unit spellings share one canonical unit type.
- [x] Integer, floating-point, and string literal materialization honors
  suffixes and expected types before the specified `i32`, `f64`, and `str`
  defaults. Integer minima, signed/unsigned limits, float range, and
  target-dependent `isize`/`usize` limits are checked for both 32- and 64-bit
  targets.
- [x] Reusable queries expose target-aware layout availability, place
  addressability and mutability, generic trait obligations, C ABI safety, and
  recursive explicit-alias, managed-reference, and mutable-indirection
  containment.
- [x] Integration coverage checks generic alias equivalence and cycles,
  distinct same-named nominals from separate package instances, inference,
  concrete numeric invariance, reference/function invariance, all identity
  markers, target-width literal boundaries, foreign ABI/layout properties,
  trait and impl bounds, and place classification. The authoritative
  demonstration canonicalizes without diagnostics; all 80 project tests pass
  under formatting, checking, and warning-denied Clippy validation.

Milestone 5 canonicalizes declarations and signatures but deliberately does not
type-check executable expressions. Expression categories, contextual expected
types at call/binding/return sites, place propagation, operator checking, and
copy recording begin in Milestone 6.

### 17.6 Milestone 6 exit criteria

- [x] Plain non-generic module-level functions are checked for direct calls,
  arity, contextual argument and return types, and unit fallthrough.
- [x] Expressions retain canonical types, value/place classification, and
  explicit logical-copy decisions for later lowering.
- [x] Bindings, assignments, compound assignments, fields, indexing, tuples,
  arrays, structs, enum construction, primitive operators, and explicit
  numeric casts are checked with source diagnostics.
- [x] Recursive value containment is rejected unless a cycle crosses an
  explicit safe reference or raw pointer.
- [x] Integration coverage includes successful programs and stable diagnostics
  for each supported expression category. Method, trait, and generic body
  checking remains assigned to Milestones 11 through 13.

### 17.7 Milestone 7 exit criteria

- [x] `if`, `while`, `for`, `match`, `return`, `break`, `continue`, and
  short-circuit conditions are checked in the same body pass as Milestone 6.
- [x] Reachable-path analysis requires every reachable path of a non-unit
  function to return and diagnoses misplaced loop control.
- [x] Pattern typing records immutable bindings, checks alternative binding
  consistency, requires explicit dereference for safe-reference content
  matching, and implements conservative usefulness and exhaustiveness for the
  documented Milestone 7 scope.
- [x] Compound-assignment destinations and expression operands retain the
  information needed for exact-once, left-to-right lowering.
- [x] The authoritative demonstration checks without false positives and all
  105 frontend tests pass.

### 17.8 Milestone 8 exit criteria

- [x] Owned typed high-level IR records selected declarations, canonical types,
  places, copy decisions, casts, and source spans.
- [x] Control-flow lowering produces basic blocks with explicit temporaries,
  loads/stores, calls, branches, returns, short-circuit paths, and trap
  annotations.
- [x] Deterministic package/declaration-based mangling and an internal Elamite
  calling convention are separated from the public C entry shim.
- [x] Deterministic C99 emission covers primitives, unit, tuples, fixed arrays,
  and initial non-recursive structs without relying on unspecified C
  evaluation order.
- [x] Checked arithmetic, division, shifts, indexing, and numeric conversions
  trap with stable codes and source locations; `isize`/`usize` checking follows
  the selected 32- or 64-bit target.
- [x] The driver invokes a selectable C compiler with debug or release
  optimization, forwards native link inputs, retains generated C on request or
  toolchain failure, and reports failures as toolchain diagnostics.
- [x] Executable packages receive an entry shim and can be built and run;
  library packages produce relocatable objects without an entry shim.
- [x] A collector-neutral managed-memory backend contract exposes
  initialization, scanned/pointer-free allocation, collection, root
  registration, keep-alive, and native-library operations with Boehm selected
  by default; actual managed allocation remains Milestone 10.
- [x] Backend integration tests execute at multiple C optimization levels,
  verify left-to-right and short-circuit behavior, exercise all trap classes,
  inspect deterministic C, exercise target-aware checking and both target
  selections through the native driver, exercise the command-line driver, and
  bring the complete suite to 125 passing tests
  under formatting, checking, and warning-denied Clippy validation.

### 17.9 Milestone 9 exit criteria

- [x] Every C-representable canonical type is routed through one explicit
  logical-copy strategy and a deterministic generated copy helper.
- [x] Tuples, fixed arrays, nested structs, and explicit-discriminant enums
  recursively copy ordinary payloads; mutable `String` buffers are eagerly
  duplicated; scalar values and explicit aliases retain their required direct
  or identity-preserving behavior.
- [x] Assignment, argument, return, aggregate read, aggregate construction,
  and pattern binding sites consume explicit copy operations in control-flow
  IR.
- [x] Source-ordered `match` control flow lowers literals, alternatives,
  guards, tuple/struct patterns, and unit/tuple/record enum variants. Payload
  bindings are immutable independent copies.
- [x] Run-pass tests cover nested assignment/argument/return independence,
  aggregate-read independence, copied match payloads, eager string copy
  helpers, source order, guards, alternatives, structural patterns, enum
  representation, and both debug/release optimization.
- [x] Runtime representations not yet present cannot silently receive a
  shallow copy: safe-reference execution remains M10 and collection
  representations/copy hooks remain M14.
- [x] The complete suite contains 133 passing tests under formatting,
  checking, and warning-denied Clippy validation.

### 17.10 Milestone 10 exit criteria

- [x] Safe references lower to one non-moving pointer representation, and every
  address-taken local is conservatively promoted to Boehm-managed storage.
- [x] Binding, interior, mutable, escaping, and referenced-composite-literal
  behavior reaches executable C99 with demand-driven collector linkage.
- [x] Interior pointers keep their complete containing allocation reachable;
  root retention and the deferred cycle-collection test are documented in
  §19 and §9 respectively.

### 17.11 Milestone 11 exit criteria

- [x] Non-generic inherent method bodies are checked and lowered with `Self`
  and all five legal receiver forms.
- [x] Bound calls adapt only their receiver: value copying, safe-reference
  dereference/copy, shared or mutable auto-borrow, exact reference passing, and
  exact raw-pointer passing are represented explicitly in typed IR.
- [x] Associated and unbound method selection produces stable named function
  references; instance selection cannot construct a bound-method value.
- [x] Safe and unsafe function references have exact invariant signatures,
  support storage, parameters, returns, indirect calls, and identity equality,
  and lower to typed C99 function pointers without closures or captures.
- [x] Field-first postfix calls bypass receiver adaptation, and homogeneous
  variadic tails are packed into a slice with runtime length for checked
  indexing.
- [x] Compile-fail coverage checks invalid receiver adaptation, bound-method
  values, bare function value types, safety mismatch, variadic mismatch, and
  raw-pointer mismatch; debug and release run-pass coverage exercises methods,
  indirect calls, fields, returns, equality, unbound calls, and variadics.

### 17.12 Milestone 12 exit criteria

- [x] Generic function and inherent-method bodies are checked once using
  canonical generic-parameter types and their declared obligations, without
  being rechecked against call-site-specific capabilities.
- [x] Calls and generic function references infer a unique complete argument
  list from ordinary arguments and expected results; explicit lists must be
  complete. The same rule applies to struct and enum literals.
- [x] Generic aggregate fields, selections, patterns, receiver adaptation, and
  local annotations substitute their enclosing nominal arguments.
- [x] Concrete function signatures are cached by declaration identity plus
  canonical type arguments. Typed IR discovers reachable function instances
  to a fixed point and presents concrete types to control-flow lowering.
- [x] Concrete generic structs and enums receive distinct backend layouts,
  copy helpers, and type-bearing symbol identities. Generic function symbols
  likewise include every concrete type argument.
- [x] Direct and mutual finite recursion deduplicate successfully, while
  recursive structural growth is rejected deterministically before C
  emission.
- [x] Compile-pass/fail coverage exercises explicit, inferred,
  expected-result, ambiguous, and partial generic arguments. Run-pass coverage
  exercises generic functions, function references, structs, enums, methods,
  recursive types through explicit indirection, and finite mutual recursion.

### 17.13 Milestone 13 exit criteria

- [x] Trait implementations are checked for required/default/extra methods and
  exact signatures after `Self` substitution; orphan and overlapping concrete
  or generic implementations are rejected.
- [x] Static calls prefer inherent members, diagnose trait ambiguity, support
  unconditional `Type.Trait.method` selection, and enforce declared generic
  capabilities for functions and implementations.
- [x] Object safety is validated before explicit concrete-reference conversion;
  trait references lower to data-plus-vtable fat references with typed C99
  thunks, default slots, overrides, and generic implementation specialization.
- [x] Derive lists reject duplicates, unsupported traits, invalid enum
  `Default`, and permanently incapable fields. Generic derivations remain
  conditional on their concrete field instantiations.
- [x] Fieldwise `Default`, structural equality, and lexicographic structural
  ordering execute for structs, arrays, tuples, and enums; nested floating
  values preserve IEEE unordered relational behavior.
- [x] `Eq`, `Ord`, and `Hash` capabilities are component-conditional, while
  `StableHash` is structurally inferred only from compiler-known stable leaves
  and compiler-derived `Eq` plus `Hash`; ordinary manual impls cannot claim it.
- [x] Compile-pass/fail and run-pass coverage exercises defaults, overrides,
  signature failures, ambiguity, qualified selection, generic overlap and
  bounds, conditional derivation, structural comparison, object-safety
  restrictions, multiple concrete vtables, and generic static/dynamic
  dispatch. Collection-backed consumers remain Milestone 14.

## 18. Third-party crate decisions

Another implementation/tooling decision, not a language-semantic one, on the
same footing as §13/§14. Precedent considered: rustc hand-writes its own
lexer and parser, and rust-analyzer — despite building `salsa` (incremental
queries) and `rowan` (lossless syntax trees) into its architecture — still
uses, in its own words, "a hand-written recursive descent parser." Neither
project reaches for a parser-generator crate for the core grammar; crates are
reserved for cross-cutting infrastructure around a hand-written core. This
ledger follows the same split.

### 18.1 Adopted

| Concern | Crate | Reasoning | Milestone |
| --- | --- | --- | --- |
| Manifest parsing | `toml` + `serde` | Already adopted at M1; TOML parsing is infrastructure, not language-compiler work | M1 (done) |
| Diagnostic rendering | `codespan-reporting` | Our `Diagnostic`/`Category`/`Span` shape (§0 legend; `ROADMAP.md` §2.3) already matches its `Diagnostic`/`Label`/`Files` model directly. Retrofitted into M1 immediately (`SourceManager` implements `Files`, `manifest.rs` captures real spans via `toml::Spanned` and `toml::de::Error::span()`, `main.rs` renders with `term::emit_to_write_style`) rather than left as a future intention — this is real span-based rustc-style output today, not just a plan | M1 (retrofitted), all later milestones |
| Snapshot testing | `insta` | `ROADMAP.md` explicitly asks for "golden token streams," "a syntax-tree dump... stable enough to serve as a debugging tool," and snapshot-compared parse tests (§2.4) — exactly insta's purpose, and the standard choice in this niche (rust-analyzer, ruff, and biome all use it for the same purpose) | M2 onward |
| Property testing | `proptest` | `ROADMAP.md` §2.4 already calls for "property or fuzz tests for indentation, literal parsing, parser recovery, numeric boundaries, match exhaustiveness, generic instantiation, and copy independence" | M2 onward |
| Retained parser fuzz corpus | `proptest` plus checked-in malformed inputs | M19 combines the existing arbitrary-Unicode parser property test with seeded indentation, delimiter, escape, formatted-string, and recovery inputs; this keeps failures reproducible without making libFuzzer a normal build dependency | M19 (done; `tests/robustness.rs`) |
| Symbol interning | `lasso` | Backs the existing cross-cutting rule "assign internal IDs rather than using source names as identity" (`ROADMAP.md` §2.1) once identifiers flow through resolution; cheap `Copy` symbol keys instead of cloning `String` everywhere | M4 (done) |
| CLI surface | `clap` (derive) | `ROADMAP.md` Milestone 18's command surface (check/build/dump options, target/output selection — §14 here) is ordinary CLI-flag parsing, not compiler-specific work | M18.6 (done) |

### 18.2 Deferred, not rejected

| Concern | Crate | Why not yet | Reconsider at |
| --- | --- | --- | --- |
| Incremental/query-based compilation | `salsa` | `ROADMAP.md` §1 and Milestone 20 both explicitly defer incremental compilation until after conformance tests exist; adopting it now would be premature architecture. The existing "stable IDs and owned tables instead of long-lived references" rule (§2.1) is already salsa-shaped, so adopting it later should not require restructuring the ID system — only wrapping it in salsa's query/tracked-struct machinery | M20 |
| Lossless CST / IDE-grade syntax trees | `rowan` | Built for exactly the use case rust-analyzer needs it for (incremental reparsing, trivia-preserving trees for refactoring). Elamite's goal through M19 is "compiles to C," not powering an editor — `ROADMAP.md`'s own Milestone 20 candidate list ("language-server support") is the only place this becomes relevant | M20, only if language-server support is pursued |
| Semantic-version-aware manifest validation | `semver` | `Manifest` currently only checks the `version` field is non-empty (`SPEC.md` §2.3 doesn't mandate a version grammar); version comparison matters only if a future resolver adds version selection beyond the normative initial local-path model | when such a resolver is designed |

### 18.3 Explicitly not adopted for the core grammar

- **Lexer generators** (`logos` for the *whole* lexer) — `logos` may still be
  useful for matching the "flat" token shapes (identifiers, keywords,
  punctuation, operators, numeric/string literal bodies) inside one logical
  line, but the indentation stack, four-space statement-continuation rule, and
  logical newline/indent/dedent events (`SPEC.md` §2.2) are inherently
  stateful in a way no lexer-generator expresses — that layer stays
  hand-written regardless of whether `logos` handles token shapes underneath
  it. Spike both before committing to either.
- **Parser generators** (`pest`, `lalrpop`, `chumsky`) — rejected for the core
  grammar. Every diagnostic-quality requirement in `ROADMAP.md` §2.3
  ("Compile-fail tests should assert stable diagnostic categories and
  meaningful spans") and Milestone 3's parser-recovery validation list is
  easier to hit with a hand-written recursive-descent/Pratt parser that
  controls its own error recovery — matching rustc's and rust-analyzer's own
  choice (confirmed above) and `SPEC.md` §7's explicit 14-level precedence
  table, which maps directly onto precedence climbing.
- **`cc` crate for invoking the C backend's compiler** — a likely-looking but
  wrong fit. `cc::Build` is designed for `build.rs`-time compilation of C
  source into a Cargo build (`OUT_DIR`, `TARGET`, `CC` env conventions set by
  Cargo). Milestone 8/17's job is different: the *`elamc` compiler binary*,
  once already built, spawns a C compiler as a subprocess against *user*
  generated C at the user's build time — that's plain `std::process::Command`
  with computed arguments, not a Cargo build-script concern. The one place
  `cc` legitimately fits is compiling Elamite's *own* bundled C runtime-support
  library (checked-arithmetic helpers, trap/panic handlers, GC init shims)
  into the shipped compiler binary at M17/M18 — that genuinely is `build.rs`
  -time C compilation, so `cc` is the right tool there specifically, not for
  the general "invoke a C compiler" step.
- **Arena-of-references AST** (`bumpalo`, `typed-arena`) — would introduce
  `&'arena T` lifetime parameters threading through every IR type, directly
  contradicting the already-established cross-cutting rule "Represent compiler
  relationships with stable IDs and owned tables instead of long-lived
  references between phase data" (`ROADMAP.md` §2.1, `AGENTS.md`). A plain
  `Vec`-backed store with a newtype index (`NodeId(u32)`) needs no dependency
  at all and matches that rule directly; `slotmap` or `id-arena` remain
  available if generational-key safety is ever worth the dependency, but
  aren't required.

## 19. Reference storage model (M10)

An implementation decision on the same footing as §13/§14/§18. `SPEC.md` §3.2
fixes the observable behavior; this section records how the backend achieves
it and why an earlier, more elaborate model was abandoned.

### 19.1 The rule

A reference names storage. Every assignment that overwrites that storage is
observable through the reference, whether the reference was formed from a
binding (`&point`) or from a path into an aggregate (`&user.address.city`).
Mutation through a reference into an aggregate is visible in the container for
the same reason: both name the same storage.

This is exactly the C and Go model. `&user.address.city` is a pointer into the
container's storage, and replacing the container writes through it.

### 19.2 Implementation

- `&T` and `&var T` both lower to `T *`. `Mutability` is compile-time only.
  A single C representation is required by function references (M11),
  `&Trait` dispatch (M13), and the public C ABI and callbacks (M17), all of
  which need a `&T` parameter to accept any `&T` regardless of provenance.
- Struct layouts stay flat. No field is indirected to satisfy reference
  semantics, so a nominal type's C layout does not depend on what any other
  part of the program does with it.
- Promotion is per-function and conservative: a local whose address is taken
  is promoted to managed storage, whole. Taking a reference into an aggregate
  promotes the containing local. Precise escape analysis stays an M20
  optimization, per `ROADMAP.md` Milestone 10.
- The collector must trace **interior pointers**, since a reference into an
  aggregate points inside a managed allocation rather than at its base. Boehm
  provides this (`GC_ALL_INTERIOR_POINTERS`, enabled by default in most
  builds; the backend sets it explicitly rather than relying on the build).
- A reference into an aggregate therefore keeps its **whole container**
  reachable, not merely the selected subvalue — the same reachability behavior
  as Go.
- Milestone 9's copy helpers are unaffected. Flat layouts mean an ordinary
  aggregate copy stays a recursive value copy, and references inside a copied
  aggregate keep their identity exactly as M9 specified.

### 19.3 Root retention

Boehm scans the C stack and machine registers conservatively, so a promoted
cell's pointer — an ordinary C local in the generated function — is already a
root for as long as its frame is live. The implementation therefore emits no
explicit root registration for locals, parameters, or temporaries, and no
keep-alive barriers: there is no language-level strong path that the C
compiler could shorten below what the conservative scan already covers.

`RegisterRoot`, `UnregisterRoot`, and `KeepAlive` remain defined on
`ManagedMemoryStrategy` and unused. They are needed when storage outlives a
scanned frame or is reachable only from foreign code — Milestone 17's foreign
roots and callbacks — and a strategy that is not conservatively stack-scanning
would need them earlier. Emitting no-op barriers now would add noise without
adding a guarantee.

Allocation failure calls `el_out_of_memory`, which requests a full collection
through the strategy and then terminates, without running deferred cleanup
(`SPEC.md` §9).

### 19.4 Rejected: subvalue targeting with field boxing

`SPEC.md` §3.2 originally stated that a reference into a nested aggregate
targeted the selected subvalue and was *not* rebased by a later replacement of
its container, with a worked example printing the former value. Satisfying that
alongside "a binding reference observes reassignment" is impossible for a
single contiguous cell, and the only resolution preserving one C representation
for `&T` was to box every address-taken field into its own managed cell.

That rule was rejected in favor of the storage model specified by `SPEC.md`
§3.2. Boxing would have made a nominal type's C layout depend on a
whole-program analysis, and would have required M9's copy helpers to deep-copy
through each box to preserve independence—a silent-aliasing failure mode,
since a copy that duplicated the pointer would produce two values that are
supposed to be independent but share a subvalue. The cost bought behavior that
no C or Go programmer would predict.

Two alternatives were considered and rejected on their own terms: *generation
cells* (allocate a fresh cell per assignment, with binding references pointing
at the binding slot) split `&T` into two incompatible C representations
(`T **` and `T *`), breaking M11/M13/M17; and *copy on reference formation*
broke `&var`'s caller-visible mutation, since writes would land in a private
copy the container never sees.
