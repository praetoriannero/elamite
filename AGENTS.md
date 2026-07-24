# Repository Guidelines

## Project Structure & Module Organization

- `src/lib.rs` is the compiler library root. Add compiler phases as focused
  modules beneath `src/`, following the representation boundaries in
  `IMPL.md`.
- `src/main.rs` is the command-line entry point. Keep language behavior in the
  library so it can be exercised directly by tests and other tools.
- `examples/` holds Elamite source examples. `SPEC.md` is the language design;
  `IMPL.md` is the compiler roadmap; and `ISSUES.md` records unresolved design
  work.
- Add integration tests under `tests/` and Elamite fixtures beneath a focused
  fixture directory such as `tests/fixtures/parser/`.

## Build, Test, and Development Commands

Use a stable Rust toolchain that supports edition 2024:

```sh
cargo fmt --check
cargo check --all-targets
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo run
```

`cargo run` currently prints the compiler version; expand its interface as the
driver milestones in `IMPL.md` are implemented.

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
milestone exit criteria in `IMPL.md`.

## Design, Commits, and Pull Requests

Treat `SPEC.md` and the authoritative `examples/spec_demo.elx` as design inputs.
Append new entries to `ISSUES.md`; use `I-X.Y` for a sub-issue related to
`I-X`. Do not implement or change surface grammar while its design review is
active.

The language uses a **minimal function model**: function values are safe or
unsafe function references (`&fn(P) -> R` or `&unsafe fn(P) -> R`), with no
closures, no anonymous `fn` literals, no capture, and no `move`. Callbacks that
carry data use `&dyn Trait`, and recursion uses named functions. Keep `SPEC.md`,
`examples/spec_demo.elx`, and `ISSUES.md` consistent with this direction, and
do not reintroduce closures, capture, anonymous function literals, or `move`
unless the design decision is explicitly reopened. When this direction
changes, update this rule.

Deterministic cleanup uses a lexical, block-scoped `defer` statement containing
one safe unit-returning call. The language has no `with`, `errdefer`, or
multiline `defer:` form. Deferred calls are evaluated at scope exit and are not
closures or first-class values.

Recent commits use short, lowercase descriptive subjects, such as `repo
cleanup`. Keep commits focused and imperative. Pull requests should summarize
the behavior change, link relevant issues, identify grammar/spec impact, and
report validation performed. Include diagnostic output when changing errors.
