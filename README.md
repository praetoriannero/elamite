# Elamite documentation

Elamite is a statically typed, garbage-collected language that compiles to C99.
The normative design is 0.10.0-draft. The compiler is in an ordered migration:
ordinary copies use the 0.10 shallow contract, and collection representations
implement the accepted mutable-sharing contract. Iteration
invalidation, concurrency, and raw-pointer traversal still retain their 0.9
implementation. The release/version identity and authoritative demonstration
therefore remain 0.9 rather than claiming partial 0.10 conformance.

## Documentation map

- [`docs/spec.md`](docs/spec.md) is the normative language design.
- [`docs/roadmap.md`](docs/roadmap.md) contains active and completed implementation work.
- [`docs/ledger.md`](docs/ledger.md) maps every normative rule to its implementation,
  runtime dependency, and test evidence.
- [`docs/cost_model.md`](docs/cost_model.md) documents current non-normative copying,
  allocation, retention, promotion, and synchronization costs.
- [`docs/architecture.md`](docs/architecture.md) records compiler phase ownership and
  expansion boundaries.
- [`docs/toolchain.md`](docs/toolchain.md) documents installation, commands, supported
  targets, and current limitations.
- [`docs/release.md`](docs/release.md) indexes conformance and release evidence.
- [`docs/issues.md`](docs/issues.md) contains active design questions; it is currently
  empty of unresolved reviews.
- [`docs/proposals.md`](docs/proposals.md) and [`docs/critiques.md`](docs/critiques.md) preserve
  non-normative design history and review.

The currently implemented 0.9 surface example is
[`examples/spec_demo.elx`](examples/spec_demo.elx); it becomes authoritative
for 0.10 only after the migration milestone completes. Focused packages cover
[`closures`](examples/closures), [`macros`](examples/macros),
[`concurrency`](examples/concurrency), [C FFI](examples/c_ffi),
[`raylib`](examples/raylib), [`SDL2`](examples/sdl), and the
[billion-iteration Leibniz benchmark](examples/leibniz_pi). The two
[`tests/fixtures/regression`](tests/fixtures/regression) packages are
adversarial conformance fixtures rather than examples.

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
cargo run -- dump expanded path/to/package
cargo run -- dump typed-ir path/to/package
cargo run -- doc path/to/package
```

Compile one source file directly, without an `elamite.toml`, and choose the
executable path with `-o`:

```sh
cargo run -- path/to/main.elx -o app
```

The explicit workflow is also available as
`cargo run -- build path/to/main.elx -o app`; `check`, `run`, and `dump` accept
a source file in the same position as a package directory. A standalone file
is an implicit executable package containing only that file. Use a manifest
package when the program needs file-backed modules, package dependencies, or
native link configuration.

Successful compilation is quiet by default. Diagnostics and toolchain failures
are written to standard error; `run` forwards the compiled program's own
standard output and standard error.

Format one source file or every source owned by a package with:

```sh
cargo run -- fmt path/to/main.elx
cargo run -- fmt path/to/package
cargo run -- fmt --check path/to/package
```

The formatter uses a preferred maximum line length of 100 columns. Packages
can change it in `elamite.toml`:

```toml
[format]
line_length = 88
```

Pass `--line-length=COLUMNS` to override the manifest or default value for one
invocation. Formatting preserves comments and significant tokens, refuses to
write invalid source, and is quiet on success. Package formatting does not
modify dependencies or standard-library sources.

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

Compilation commands accept `--target=x86` or `--target=x86_64`. Direct
single-file compilation, `build`, and `run` also accept `-o PATH`,
`--release`, `--out-dir=PATH`, and `--cc=PATH`; `-o` selects the exact final
artifact path while `--out-dir` selects the intermediate and metadata
directory. Add `--keep-c` to retain the generated translation unit for
inspection. Run `cargo run -- --help` or append `--help` to a command for the
complete Clap-generated interface. Executable packages produce a native
executable; library packages produce a relocatable object and public metadata.

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
The non-normative [memory cost model](docs/cost_model.md) documents current copy,
allocation, retention, promotion, and synchronization costs and links the
reproducible release-mode baseline.

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
