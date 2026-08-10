# Repository Guidelines

## Project Structure & Module Organization

- `src/lib.rs` is the compiler library root. Add compiler phases as focused
  modules beneath `src/`, following the representation boundaries in
  `docs/roadmap.md`.
- `src/main.rs` is the command-line entry point. Keep language behavior in the
  library so it can be exercised directly by tests and other tools.
- `src/syntax.rs` owns phase-neutral tokens, syntax trees, and generic traversal.
  `src/parsed.rs` owns package-wide loading and parsing, and `src/expansion.rs`
  is the parsed-to-resolved expansion boundary. Keep lexing and parsing
  hand-written and separate.
- `src/resolution/model.rs` owns stable module, declaration, member, generic,
  impl, import, and lexical-binding identities and result tables.
  `src/resolution/collect.rs`, `imports.rs`, `bodies.rs`, and `visibility.rs`
  own their corresponding algorithms behind the `resolution` façade. Keep
  type-dependent member selection in later semantic passes rather than
  folding it into name lookup.
  `src/standard.rs` owns the exact intrinsic inventory and includes the shipped
  sources from `stdlib/`; resolution loads those declarations through the
  ordinary lexer/parser/collection path. Prefer adding a standard type in
  `stdlib/src/` over adding a compiler-known builtin: source declarations reach
  every later pass through the ordinary path, so no pass needs a parallel
  representation. A type belongs in the intrinsic list only while it still
  needs a representation or lowering hook that Elamite source cannot express.
- `src/traits.rs` validates trait implementations: conformance, coherence, and
  object safety. It checks declarations, not bodies — bound-call selection and
  dispatch belong to `src/check/` and `src/backend/`.
- `src/types/context.rs`, `model.rs`, and `lower.rs` separately own canonical
  type storage, typed-program facts, and source-type lowering behind the
  `types` façade. Add new source type forms to the single lowering path.
- `src/check/` owns body-checking orchestration and checked facts; keep mutually
  dependent expression and statement walking together. Pure containment and
  exhaustiveness analyses belong in focused child modules.
- `src/ir/typed/` and `src/ir/control_flow/` each own their data model and
  lowering. Shared operation and trap vocabulary belongs in
  `src/operations.rs` and `src/ir/traps.rs`, not in the checker or backend.
- `src/config.rs` owns target and optimization policy. `src/backend/` owns only
  C naming, type/layout emission, runtime helpers, function/control-flow
  emission, and the executable entry shim.
- `src/expansion/ast.rs` owns the versioned, compile-time-only `std.ast`
  façade. Keep its values immutable and detached from `SyntaxNode`, resolution,
  inferred types, layout, runtime state, and mutable compiler tables. Only the
  expansion layer may mint origin handles; builders and transforms must retain
  provenance without fabricating physical spans.
- `src/expansion/quote.rs` owns typed quote-role inference, interpolation
  position classification, and quote-body adaptation back into the ordinary
  parser. Keep quote templates role-neutral in `src/parser.rs`; do not parse
  quoted source during lexing or let quote validation grow a second Elamite
  grammar. Hygiene and façade-value construction belong to interpreter
  lowering, not this syntax adapter.
- `src/promotion.rs` decides which locals need managed storage. It answers only
  "is this local's address taken", deliberately conservatively; precise escape
  analysis belongs to the **Post-conformance optimization** milestone, not
  here.
- `docs/cost_model.md` is the versioned, non-normative account of current copy,
  allocation, retention, promotion, and synchronization costs. Any change to
  representation, logical-copy lowering, collection growth, formatting,
  promotion, managed memory, transfer, or synchronized storage that changes a
  documented cost must update it in the same change. Material cost changes
  also require comparable before/after runs of
  `benchmarks/memory-cost-baseline.sh` and a note in `docs/release.md`; never
  turn benchmark observations into semantic conformance thresholds.
- `README.md` is the project and documentation index. All other project
  Markdown except this contributor-facing `AGENTS.md` belongs under `docs/`;
  keep links and source comments pointed at that directory so the specification
  and planning documents remain discoverable after future changes.
- `examples/` holds Elamite source examples. `docs/spec.md` is the language
  design; `docs/roadmap.md` is the compiler roadmap; `docs/ledger.md` maps every
  normative `docs/spec.md` rule to an implementation milestone; and
  `docs/issues.md` records unresolved design work.
- `docs/proposals.md` and `docs/critiques.md` are non-normative design history.
  Keep resolved material there clearly labeled and never treat either as
  authorization to diverge from `docs/spec.md`.
- `editors/vscode/` is a declarative VS Code extension providing `.elx` syntax
  highlighting: a TextMate grammar and a language configuration, with no
  compiled code. Its grammar necessarily duplicates the keyword, numeric-suffix,
  builtin, attribute, and macro lists owned by `src/`, so `tests/editor_grammar.rs`
  asserts the copies still agree. Adding any of those to the compiler means
  updating `editors/vscode/syntaxes/elamite.tmLanguage.json` in the same change.
- Add integration tests under `tests/` and Elamite fixtures beneath a focused
  fixture directory such as `tests/fixtures/parser/`.

## Build, Test, and Development Commands

Use a stable Rust toolchain that supports edition 2024:

```sh
cargo fmt --check
cargo check --all-targets
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

Check, build, or run a package with:

```sh
cargo run -- check path/to/package
cargo run -- build path/to/package
cargo run -- run path/to/package
```

All three commands accept `--target=x86` or `--target=x86_64`. `build` and
`run` also accept `--release`, `--out-dir=PATH`, `--cc=PATH`, and `--keep-c`.
Keep this summary and `docs/toolchain.md` synchronized when the driver changes.

## Coding Style & Naming Conventions

Use `rustfmt` defaults. Name modules, functions, and variables with
`snake_case`; types and traits with `UpperCamelCase`; and constants with
`SCREAMING_SNAKE_CASE`. Prefer small modules with explicit public boundaries.

Represent compiler relationships with stable IDs and owned tables instead of
long-lived references between phase data. Keep lexing, parsing, resolution,
type checking, lowering, and C emission separate. Parser code constructs syntax
only; it must not perform name resolution or semantic checks.

Use exhaustive matches for compiler IR where practical. Treat unexpected
user input as a diagnostic, not a panic. Reserve panics for violated internal
invariants and state those invariants near the assertion.

## Testing Guidelines

Place focused unit tests beside their modules and end-to-end or multi-module
tests under `tests/`. Name tests after behavior rather than implementation
details. Use `.elx` fixtures when source text is clearer than an inline string.

Cover successful behavior, important diagnostics, and runtime trap boundaries.
Compile-fail tests should assert stable diagnostic categories and meaningful
spans without depending on incidental prose. Follow the test layers and
milestone exit criteria in `docs/roadmap.md`.

## Design, Commits, and Pull Requests

Treat `docs/spec.md` 0.11.0-draft as the normative design target and follow the
strict implementation order in `docs/roadmap.md`. The current compiler remains
the 0.10 shallow/GC implementation baseline in `examples/spec_demo.elx`;
`examples/owned_spec_demo.elx` is the target-only 0.11 demonstration and must
not be added to current compile-pass tests prematurely. The inactive split
target corpus lives under `tests/fixtures/owned_model_design/`.

During migration, select one complete semantic revision per package. Preserve
0.10 behavior unless the active work package explicitly introduces its 0.11
replacement, and stop unsupported 0.11 constructs at the declared migration
boundary rather than lowering them with shallow semantics. Do not implement a
later ownership milestone around a missing earlier checker or IR fact.

The function-pointer model remains exact: safe named functions use
`&fn(P) -> R` or `&unsafe fn(P) -> R`, raw counterparts use `*fn(P) -> R` or
`*unsafe fn(P) -> R`, raw calls require `unsafe:` plus a null check, and function
and data pointer domains never cast between each other.

The accepted closure target is `fn[captures](parameters):`, including `fn[]`
for capture-free closures. Capture lists are exhaustive; plain captures move or
`Copy`, `&` captures shared provenance, and `&var` captures exclusive
provenance. Environments are nominal inline first-class objects with one shared
`Callable[Arguments, Return]` call contract. There are no implicit captures,
generic or variadic closure literals, unsafe closures, trailing-closure syntax,
or anonymous recursion. Only capture-free closures explicitly convert to exact
function references; stateful C callbacks use a separate owned raw context.

`docs/spec.md` §§3–9 own move-by-default values, explicit `Clone`, structural
`Copy`, inferred borrow provenance, deterministic `Drop`, uniquely owned
collections, explicit `Box`/`Shared`/`Weak`/`Store`/`Handle` ownership, and
collector removal. Ordinary borrow formation must not allocate. `&var` is
exclusive and move-only. Do not insert hidden clones, retain shallow mutable
backing aliases, or use promotion/GC to legalize a reference escape in the
0.11 path.

`docs/spec.md` §10.4 owns race-safe concurrency. It adds no syntax:
`std.thread.spawn` consumes an owned `Send` closure, `std.thread.scope` admits
inferred scoped borrows, channels move messages, mutexes expose guarded
borrows, and atomic cells are non-`Copy` and sequentially consistent. `Send`
and `Sync` are structural capabilities; raw pointers receive neither
automatically. Safe code must not retain the 0.10 programmer-managed data-race
contract.

`docs/spec.md` §3.3 continues to own raw data-pointer traversal.
Element-scaled `+`, `-`, `+=`, `-=`, same-extent subtraction to `isize`,
indexing, and relational ordering require `unsafe` and complete nonzero-sized
data pointees. Indexing performs null/alignment checks but no bounds check.
Null orders below non-null; two non-null pointers order only within one live
extent. Equality remains safe, raw pointers implement neither `PartialOrd` nor
`Ord`, and integer-pointer conversion and function-pointer arithmetic remain
absent. Raw pointers never own or retain storage.

Local tuple patterns remain irrefutable and limited to nested tuples,
identifiers, and `_`. Numeric postfix selectors remain canonical unsuffixed
in-range decimal indices. In the 0.11 path, a consuming projection moves a
non-`Copy` field, copies a `Copy` field, and participates in partial-move and
borrow-place checking.

`Drop.drop(&var self)` is the sole compiler-invoked user destruction hook and is
not directly callable. `Drop` types are non-`Copy` and cannot be partially
moved. `defer call` and `defer:` remain lexical non-closure registrations; in
one scope they run LIFO before automatic reverse-initialization destruction.
Deferred code cannot use `return`, `break`, `continue`, postfix `?`, nested
`defer`, or `unsafe`, and traps remain non-unwinding.
The C backend targets **C99**. Generated C and foreign declarations must not
rely on C11-only features (`_Static_assert`, `_Generic`, anonymous
struct/union members); lower tagged unions (enums) with an explicit
discriminant field instead of an anonymous union.

The initial supported target matrix is **Linux, x86-64 and x86 (32-bit)**.
Both architectures are in scope from the compiler driver onward — do not
hardcode a 64-bit pointer width; `isize`/`usize` and any layout-sensitive code
must work at both widths. See `docs/ledger.md` §13 for the full target-assumption
list.

The lexer and parser are **hand-written**, not generated by a parser-generator
crate (`pest`, `lalrpop`, `chumsky`) — matching rustc's and rust-analyzer's own
choice, and needed for the custom error recovery and precise spans `docs/roadmap.md`
§2.3 requires. Crates are for cross-cutting infrastructure around that
hand-written core, not the core grammar itself: `codespan-reporting` for
diagnostic rendering (already wired into `SourceManager`/`Diagnostic`), `insta`
for token/syntax-tree snapshot tests, `proptest` for property tests, `lasso`
for symbol interning, and `clap` for the implemented CLI.
`salsa` (incremental compilation) and `rowan` (lossless CST) are deliberately
deferred past initial conformance, not rejected — see `docs/ledger.md` §18 for the
full adopted/deferred/rejected list and reasoning, including why the `cc`
crate is *not* the right tool for invoking the C backend's compiler.

`docs/spec.md` §12 owns the accepted compile-time syntax-generation contract.
The three documented module-level declaration forms are `[pub] macro`, `[pub]
attr`, and `[pub] derive`; their ordinary safe Elamite bodies execute in the
bounded compile-time interpreter over the versioned, opaque, pre-resolution
`std.ast` façade. Function-like calls use `@path(...)`, attached transforms use
`@attr(path(...))`, and derivation uses `@derive(...)`. Quotation is `quote:`;
`$name`/`$(expression)` interpolate AST values, and `++` concatenates strings,
supported sequences, and AST lists rather than arbitrary expression nodes.
Macros may use one final homogeneous variadic AST parameter; attributes may do
so after their implicit target and fixed explicit parameters; derives remain
fixed at one target parameter.
Attributes run before derives on one definition, then generated ordinary items
and function-like macros follow the deterministic fixed-point scheduler. The
surface is stable; do not reintroduce the retired `--unstable-macros` gate.

Do not fold compile-time execution or expansion into the lexer, ordinary
parser, resolver, checker, or backend. Token trees and provenance belong at the
parsed-to-expanded boundary in `src/expansion.rs`,
`src/expansion/token_tree.rs`, and `src/expansion/provenance.rs`; stable
compile-time declarations, imports, module identities, and separate namespaces
belong in `src/expansion/namespace.rs`. Deterministic work ordering,
dependency staging, generated-output re-entry, active-chain cycle detection,
recovery nodes, and the shared/per-execution resource meters belong in
`src/expansion/scheduler.rs`. Resource exhaustion must admit no partial output,
and a failed interpreter fuel or live-value charge remains sticky even if its
immediate result is ignored. User-defined compile-time forms are stable and
must remain available through every compiling driver entry point; do not
recreate the retired feature gate. Custom fragment grammar remains owned by
`src/parser.rs`, and `src/expansion/fragment.rs` only adapts token trees to
those entry points. The
compile-time interpreter has no ambient capabilities or mutable compiler-table
access. Never expose compiler-private AST nodes through `std.ast` or project a
generated origin onto a physical `Span`; keep generated input separate until
the expanded-syntax representation owns origin-aware locations.
`tests/expansion_robustness.rs` owns property coverage across arbitrary
token-tree/fragment input and deep generated provenance; scheduler properties
remain beside `src/expansion/scheduler.rs`. Preserve those adversarial layers
when changing the expansion boundary.

Recent commits use short, lowercase descriptive subjects, such as `repo
cleanup`. Keep commits focused and imperative. Pull requests should summarize
the behavior change, link relevant issues, identify grammar/spec impact, and
report validation performed. Include diagnostic output when changing errors.
