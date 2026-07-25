# Elamite

Elamite is a statically typed, garbage-collected language that compiles to C.
The language and compiler are currently under design and initial
implementation.

- [`SPEC.md`](SPEC.md) defines the language.
- [`examples/spec_demo.elx`](examples/spec_demo.elx) is the authoritative
  surface-language example.
- [`IMPL.md`](IMPL.md) describes the compiler implementation milestones.
- [`LEDGER.md`](LEDGER.md) maps every normative `SPEC.md` rule to an
  implementation milestone, runtime dependency, and test layer.
- [`ISSUES.md`](ISSUES.md) records unresolved language-design questions.

## Development

The compiler is an edition-2024 Rust package:

```sh
cargo fmt --check
cargo check --all-targets
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

Milestones 1 through 8 are complete (Milestones 6 through 8 for their stated
non-generic, non-method, non-trait scope; see `IMPL.md`). The compiler library
includes the package resolver, source manager, diagnostics, span-preserving
lexer, hand-written surface parser, stable-identity name resolver, canonical
type/inference core, core expression/function/control-flow checker, typed
high-level IR, explicit control-flow IR, deterministic C99 emitter, and native
build driver.

Check, build, or run a package with:

```sh
cargo run -- check path/to/package
cargo run -- build path/to/package
cargo run -- run path/to/package
```

All three commands accept `--target=x86` or `--target=x86_64`. `build` and
`run` also accept `--release`, `--out-dir=PATH`, and `--cc=PATH`; add
`--keep-c` to retain the generated translation unit for inspection. Executable
packages produce a native executable; library packages produce a relocatable
object. The Milestone 8 executable subset includes primitive calculations,
locals, direct function calls, `if`, `while`, short-circuit operators, tuples,
fixed arrays, initial non-recursive structs, and output. Constructs outside
that subset receive lowering diagnostics rather than partial code generation.

Managed allocation begins in Milestone 10. Its backend boundary is already
defined by `ManagedMemoryStrategy`: Boehm is the default, while future
non-moving, cycle-reclaiming strategies can implement the same contract
without changing language lowering.
