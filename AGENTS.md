# Repository Guidelines

## Project Structure & Module Organization

- `src/lib.rs` is the compiler library root. Add compiler phases as focused
  modules beneath `src/`, following the representation boundaries in
  `ROADMAP.md`.
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
- `src/promotion.rs` decides which locals need managed storage. It answers only
  "is this local's address taken", deliberately conservatively; precise escape
  analysis belongs to the **Post-conformance optimization** milestone, not
  here.
- `examples/` holds Elamite source examples. `SPEC.md` is the language design;
  `ROADMAP.md` is the compiler roadmap; `LEDGER.md` maps every normative `SPEC.md`
  rule to an implementation milestone; and `ISSUES.md` records unresolved
  design work.
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
Expand this interface as later driver milestones in `ROADMAP.md` are implemented.

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
milestone exit criteria in `ROADMAP.md`.

## Design, Commits, and Pull Requests

Treat `SPEC.md` and the authoritative `examples/spec_demo.elx` as design inputs.
Append new entries to `ISSUES.md`; use `I-X.Y` for a sub-issue related to
`I-X`. Do not implement or change surface grammar while its design review is
active.

The currently implemented language uses a **minimal function model**: safe
function values are references (`&fn(P) -> R` or `&unsafe fn(P) -> R`), and
their general raw counterparts are `*fn(P) -> R` and
`*unsafe fn(P) -> R`. Both are directly callable; every raw-function call
requires `unsafe:` and a runtime null check. Exact function references may
explicitly convert to matching raw function pointers, but function and data
pointer domains never cast between each other.

`ROADMAP.md` **Explicit-capture closures** records the accepted design for
separate, explicit-capture safe closures. Do not implement its anonymous `fn`
literals, captures, or `Callable` integration until the **Normative closure
contract** package has moved that contract into `SPEC.md`, `LEDGER.md`, and the
authoritative example. The accepted design has no implicit captures, `move`,
generic closure literals, unsafe closures, or closure-to-function-pointer
conversion. Until that milestone lands, ordinary stateful Elamite callbacks
continue to use `&Trait`, formed automatically from a concrete safe reference
when that exact trait-object type is expected (or explicitly with `as &Trait`);
C callbacks carry registered state through a separate raw context pointer, and
recursion uses named functions.

`ROADMAP.md` **Standard-library concurrency** records the accepted concurrency
direction. It adds no concurrency syntax: native threads are created through
`std.thread`, and channels, copy-based mutexes, and sequentially consistent
atomic cells live in `std.sync`. Do not implement these declarations, the
structural `Transfer` capability, runtime thread entry, or broader callback
behavior until the **Normative concurrency contract** package has made the
complete contract normative. Until then, retain `SPEC.md` §10.3's
single-runtime-thread callback restriction.

`ROADMAP.md` **Tuple destructuring and positional fields** records the accepted
tuple-binding and positional-field design. Do not extend `let`/`var` to tuple
patterns or parse numeric postfix fields such as `.0` until the **Normative
tuple-access contract** package has specified their exact shape, copying,
scope, place, tokenization, and receiver rules in `SPEC.md` and `LEDGER.md`.
Existing tuple patterns remain match-only until that gate lands.

Deterministic cleanup uses a lexical, block-scoped `defer` statement in two
forms: `defer call` defers one safe unit-returning call, and `defer:` defers an
indented block of statements as a single registration. The language has no
`with` and no `errdefer`. Deferred code is evaluated at scope exit and is not a
closure or first-class value.

There is no compiler-known cleanup trait or protocol. Resource types expose
ordinary safe unit-returning methods such as `close` or `release`, and each
type's API defines its own idempotence, sharing, and error behavior. `defer`
does not privilege any method name or trait.

Because a deferred block runs while its scope is already exiting, it cannot
redirect control: `return`, `break`, `continue`, postfix `?`, and a nested
`defer` are all invalid inside it. A `defer` statement is invalid inside an
`unsafe` block, and an `unsafe` block is invalid inside a `defer:` block. When
this direction changes, update this rule.

The C backend targets **C99**. Generated C and foreign declarations must not
rely on C11-only features (`_Static_assert`, `_Generic`, anonymous
struct/union members); lower tagged unions (enums) with an explicit
discriminant field instead of an anonymous union.

The initial supported target matrix is **Linux, x86-64 and x86 (32-bit)**.
Both architectures are in scope from the compiler driver onward — do not
hardcode a 64-bit pointer width; `isize`/`usize` and any layout-sensitive code
must work at both widths. See `LEDGER.md` §13 for the full target-assumption
list.

The lexer and parser are **hand-written**, not generated by a parser-generator
crate (`pest`, `lalrpop`, `chumsky`) — matching rustc's and rust-analyzer's own
choice, and needed for the custom error recovery and precise spans `ROADMAP.md`
§2.3 requires. Crates are for cross-cutting infrastructure around that
hand-written core, not the core grammar itself: `codespan-reporting` for
diagnostic rendering (already wired into `SourceManager`/`Diagnostic`), `insta`
for token/syntax-tree snapshot tests, `proptest` for property tests, `lasso`
for symbol interning once resolution needs it, `clap` for the eventual CLI.
`salsa` (incremental compilation) and `rowan` (lossless CST) are deliberately
deferred past initial conformance, not rejected — see `LEDGER.md` §18 for the
full adopted/deferred/rejected list and reasoning, including why the `cc`
crate is *not* the right tool for invoking the C backend's compiler.

Recent commits use short, lowercase descriptive subjects, such as `repo
cleanup`. Keep commits focused and imperative. Pull requests should summarize
the behavior change, link relevant issues, identify grammar/spec impact, and
report validation performed. Include diagnostic output when changing errors.
