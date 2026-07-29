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

Milestones 1 through 13 are complete (see `IMPL.md`). The compiler library
includes the package resolver, source manager, diagnostics, span-preserving
lexer, hand-written surface parser, stable-identity name resolver, canonical
type/inference core, core expression/function/control-flow checker, typed
high-level IR, explicit control-flow IR, logical value-copy lowering,
source-ordered match lowering, storage-promotion analysis, safe-reference
lowering over Boehm-managed storage, deterministic C99 emitter, and native
build driver.

Check, build, or run a package with:

```sh
cargo run -- check path/to/package
cargo run -- build path/to/package
cargo run -- run path/to/package
```

Create a new executable package containing a hello-world program with:

```sh
cargo run -- init hello
cargo run -- run hello
```

Executable packages are the default. Add `--lib` to create a library with
`src/lib.elx` instead:

```sh
cargo run -- init hello_lib --lib
cargo run -- build hello_lib
```

All three commands accept `--target=x86` or `--target=x86_64`. `build` and
`run` also accept `--release`, `--out-dir=PATH`, and `--cc=PATH`; add
`--keep-c` to retain the generated translation unit for inspection. Run
`cargo run -- --help` or append `--help` to a command for the complete
Clap-generated interface. Executable packages produce a native executable;
library packages produce a relocatable object. The Milestone 8 executable
subset includes primitive calculations,
locals, direct function calls, `if`, `while`, short-circuit operators, tuples,
fixed arrays, initial non-recursive structs, and output. Milestone 9 adds
recursive logical copies, non-generic enums, and `match` over the currently
representable value types. Milestone 10 adds safe references: `&T` and
`&var T` lower to `T *`, every local whose address is taken is promoted to
managed storage, and referenced composite literals allocate their own cell.
Milestone 11 adds executable inherent methods, all five receiver forms,
associated and unbound selection, typed function references and indirect
calls, identity comparison, and homogeneous variadic slice packing.
Milestone 12 adds all-or-nothing generic inference from arguments and expected
results, generic aggregate construction and patterns, cached concrete function
and nominal instances, fixed-point reachability discovery, deterministic
rejection of unbounded instantiation growth, and type-argument-bearing C
symbols and helpers. Milestone 13 adds exact trait conformance and coherence,
static and qualified selection, generic-bound enforcement, compiler-supported
conditional derivations, structural equality and ordering, structural
`StableHash` inference, object-safety validation, fat trait references, and
thunked C99 vtables.
Constructs outside that subset receive lowering diagnostics rather than partial
code generation.

Managed allocation runs behind `ManagedMemoryStrategy`: Boehm is the default,
while future non-moving, cycle-reclaiming strategies can implement the same
contract without changing language lowering. A program that needs no managed
storage links no collector at all. Building one that does requires a Boehm
development package (`libgc-dev` on Debian- and Ubuntu-family systems).
