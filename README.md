# Elamite

Elamite is a statically typed, garbage-collected language that compiles to C.
The language and compiler are currently under design and initial
implementation.

- [`SPEC.md`](SPEC.md) defines the language.
- [`examples/spec_demo.elx`](examples/spec_demo.elx) is the authoritative
  surface-language example.
- [`IMPL.md`](IMPL.md) describes the compiler implementation milestones.
- [`ISSUES.md`](ISSUES.md) records unresolved language-design questions.

## Development

The compiler is an edition-2024 Rust package:

```sh
cargo fmt --check
cargo check --all-targets
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

The current executable is only a project skeleton. Running it prints the
compiler package version:

```sh
cargo run
```
