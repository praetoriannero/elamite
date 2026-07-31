# Elamite

Elamite is a statically typed, garbage-collected language that compiles to C.
The language and compiler are currently under design and initial
implementation.

- [`SPEC.md`](SPEC.md) defines the language.
- [`examples/spec_demo.elx`](examples/spec_demo.elx) is the authoritative
  surface-language example.
- [`ROADMAP.md`](ROADMAP.md) describes the compiler implementation milestones.
- [`LEDGER.md`](LEDGER.md) maps every normative `SPEC.md` rule to an
  implementation milestone, runtime dependency, and test layer.
- [`ISSUES.md`](ISSUES.md) records unresolved language-design questions.
- [`docs/toolchain.md`](docs/toolchain.md) documents installation, commands,
  developer interfaces, and deliberate initial limitations.
- [`docs/architecture.md`](docs/architecture.md) records compiler phase
  ownership and the Milestone 20 behavior-neutral refactor baseline.
- [`docs/release.md`](docs/release.md) indexes the Milestone 19 conformance and
  release evidence.

## Development

The compiler is an edition-2024 Rust package:

```sh
cargo fmt --check
cargo check --all-targets
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

`cargo build` produces the compiler executable `elamc`. The language, its `.elx`
sources, and `elamite.toml` manifests keep the Elamite name.

Check, build, run, or inspect a package with:

```sh
cargo run -- check path/to/package
cargo run -- build path/to/package
cargo run -- run path/to/package
cargo run -- dump typed-ir path/to/package
cargo run -- doc path/to/package
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

Compilation commands accept `--target=x86` or `--target=x86_64`. `build` and
`run` also accept `--release`, `--out-dir=PATH`, and `--cc=PATH`; add
`--keep-c` to retain the generated translation unit for inspection. Run
`cargo run -- --help` or append `--help` to a command for the complete
Clap-generated interface. Executable packages produce a native executable;
library packages produce a relocatable object and public metadata.

Run language-native package tests with:

```sh
cargo run -- test path/to/package
```

Run compiler conformance fixtures separately with:

```sh
cargo run -- conformance path/to/suite
```

The runner supports fixture filtering, target and optimization matrices,
isolated build directories, stable expected output/status files, and retained
artifacts after failure.

Managed allocation runs behind `ManagedMemoryStrategy`: Boehm is the default,
while future non-moving, cycle-reclaiming strategies can implement the same
contract without changing language lowering. A program that needs no managed
storage links no collector at all. Building one that does requires a Boehm
development package (`libgc-dev` on Debian- and Ubuntu-family systems).

## License

Elamite is available under the [MIT License](LICENSE). Copyright (c) 2026
Elamite contributors.

Third-party components retain their respective copyright and license terms;
Elamite's MIT license does not replace them. Rust dependencies are recorded in
`Cargo.lock`. Generated native programs may also link the
Boehm-Demers-Weiser garbage collector when managed storage is required.
Distributions that bundle these components must retain the license files and
notices supplied by their authors.

The repository currently has `publish = false` and does not produce a bundled
third-party binary distribution. Repeat the dependency-license audit if that
packaging policy changes.
