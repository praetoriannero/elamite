# Elamite toolchain

Elamite's initial toolchain supports Linux on x86-64 and x86 (32-bit). The
compiler itself may run on either architecture; `--target=x86` and
`--target=x86_64` select the generated C and native-artifact architecture.
Without `--target`, the compiler selects the host architecture.

## Native prerequisites

Elamite emits C99 and invokes `cc` unless `--cc=PATH` or the `CC` environment
variable selects another compiler. The compiler must support:

- C99, including `<stdint.h>` and `<stdbool.h>`;
- `-m32` and `-m64` for the selected target;
- `-c` and relocatable ELF objects for library packages; and
- the ordinary system linker interface accepted by GCC or Clang.

A 64-bit Linux installation commonly needs its distribution's multilib C
development packages before `--target=x86` can link executables.

Managed storage uses the Boehm-Demers-Weiser collector. The dependency is
demand-driven: programs that require no managed storage do not include or link
the collector. Programs that do require it need the Boehm headers and a
linkable `gc` library; Debian- and Ubuntu-family distributions provide these
through `libgc-dev`, with the corresponding 32-bit development libraries also
needed for x86 builds.

## Commands

```sh
elamc init hello
elamc init hello_lib --lib
elamc check path/to/package --target=x86_64
elamc build path/to/package --release --keep-c
elamc run path/to/package
elamc dump typed-ir path/to/package
elamc doc path/to/package
elamc test path/to/package --filter=qualified-test-name
elamc conformance path/to/suite --filter=case-name
```

The installed executable is named `elamc`; `elamite` remains the language and
manifest name. `elamc --version` reports both the compiler version and the
targeted `SPEC.md` revision.

`test` discovers language-native declarations in one selected package,
compiles a test-only native artifact, and executes each selected test in a
fresh process. Its report uses aligned Cargo-style rows with green passing and
red failing statuses on terminals, and automatically falls back to plain text
when redirected. `conformance` is the separate compiler-development fixture
matrix command.

`check` runs all semantic passes without lowering or native tools. `build` and
`run` accept `--out-dir`, `--cc`, `--release`, `--keep-c`, and either supported
target. Repeatable `--c-flag=FLAG` options add sanitizer or hardening flags
without removing the driver's C99 and warning flags. Executable packages
produce an executable; library packages produce a relocatable object and
versioned `.elamite-meta` public metadata.

`dump` accepts `tokens`, `syntax`, `resolution`, `types`, `typed-ir`,
`control-flow`, `monomorphized`, or `generated-c`. Dumps are deterministic for
identical inputs and start with enough source identity information to interpret
their spans.

`doc` emits Markdown for externally reachable declarations, including attached
documentation, signatures, and source links. Extraction does not depend on
successful body checking or lowering; public documentation remains available
when an unrelated private body is invalid.

`test` accepts a directory whose immediate children are package fixtures, or
one fixture package directly. Each fixture contains `expected.stdout`;
`expected.stderr` and `expected.status` are optional and default to empty
stderr and status zero. `--filter` selects one or more named fixtures,
`--target` selects one architecture, `--all-targets` selects both,
`--release` selects optimized builds, and `--all-modes` runs debug and release.
An `expected.x86.*` or `expected.x86_64.*` file overrides its portable
`expected.*` counterpart for a target-specific result.
Build output is isolated in a unique temporary directory. Successful output is
removed; failing output, including generated C, is retained and reported.

Run `elamc --help` or `elamc <command> --help` for the authoritative flag
spelling.

## Compiler interfaces

The Elamite source language described by `SPEC.md` is the compatibility
boundary. The following developer interfaces remain unstable before the
initial conformance release:

- generated C names and helper layout;
- textual intermediate-representation dump formats;
- diagnostic prose, while diagnostic categories and important spans are
  intended to remain stable;
- `.elamite-meta` contents beyond its checked format version; and
- the Rust compiler-library API.

Do not treat a library package's relocatable object as a stable C ABI. Exported
C entry points require the explicit FFI forms in `SPEC.md`.

## Deliberate initial limitations

- Package dependencies are local paths. There is no registry, Git resolver,
  version solver, or lockfile workflow.
- Concurrency, tasks, async/await, cross-thread managed values, and callbacks
  from foreign-created threads are unsupported. A C callback may enter Elamite
  only on the OS thread that initialized and is executing the runtime; nested
  and later callbacks on that same thread are supported.
- The language has named functions and callable function references, but no
  closures, anonymous function literals, or captures.
- C variadic functions and foreign ABIs other than `C` are unsupported.
- Wildcard and grouped `use` declarations are unsupported.
- The C backend does not yet lower 128-bit integer constants, arithmetic, or
  display, even though `i128` and `u128` remain reserved primitive types.
- Boehm collection timing is unspecified and is never a deterministic cleanup
  mechanism. Use explicit resource operations and `defer`.
- Foreign exceptions, `longjmp`, and Elamite traps must not unwind through the
  language boundary. Recoverable errors require explicit wrapper translation.

See `ISSUES.md` for unresolved design work and `LEDGER.md` for the exact
implementation and test status of normative rules.
