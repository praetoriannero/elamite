# Compiler architecture

Milestone 20 established explicit, owned boundaries without changing Elamite
language behavior or the public compiler commands.

## Pipeline and ownership

| Boundary | Owner |
| --- | --- |
| Tokens and token-preserving syntax | `src/syntax.rs` |
| Hand-written lexing and parsing | `src/lexer.rs`, `src/parser.rs` |
| Loaded and parsed package units | `src/parsed.rs` |
| Expansion between parsing and resolution | `src/expansion.rs` |
| Stable names, IDs, and resolution tables | `src/resolution/model.rs` |
| Collection, imports, bodies, visibility | matching modules in `src/resolution/` |
| Canonical types and interning | `src/types/context.rs` |
| Typed-program facts | `src/types/model.rs` |
| Source-type lowering | `src/types/lower.rs` |
| Checked output and pure checker analyses | `src/check/model.rs`, `coverage.rs`, `containment.rs` |
| Body-checking orchestration | `src/check/mod.rs` |
| Typed IR and lowering | `src/ir/typed/model.rs`, `lower.rs` |
| Control-flow IR and lowering | `src/ir/control_flow/model.rs`, `lower.rs` |
| Shared selected operations and traps | `src/operations.rs`, `src/ir/traps.rs` |
| Target and optimization policy | `src/config.rs` |
| C naming, types, runtime, functions, entry | matching modules in `src/backend/` |

The lexer, parser, resolution, types, checker, IR, and backend façades retain
their established public paths. Compatibility re-exports for `Target` from
`backend` and `Optimization` from `driver` are retained because integration
users already import them there; new compiler code should use `config`
directly.

The expansion pass is intentionally a typed pass-through until the macro
milestones. Future native-language test discovery and execution belongs to the
package-test runner and remains separate from the existing conformance fixture
runner.

## Behavior-neutral baseline

Before structural edits, `examples/spec_demo.elx` produced these SHA-256
digests:

| Artifact | SHA-256 |
| --- | --- |
| tokens | `137620c0ed56528b5feaa0d727a41010f078aacbd1cad8e682371460d3552bd8` |
| syntax | `9b2e37d5ee01fb3542483bbfd27e8b28a3fc283c895d154eb148b8c87b8b5b0f` |
| resolution | `41d851cf30e179fc314a8ae493bca17f8e9def8013996e08cd9d9db6fac0e681` |
| types | `fb690e8edc5c058667beb0fef0e3049ddb3c885866980c31035979fc32b2c5e4` |
| typed IR | `d82585bd4efbdeab5426c6360535530be78b2f5048ef500eab5828a01ddcbd9c` |
| control flow | `34705be3f70bda830cfc289f840b6532c871eeb83901d46ccb83c4ed2562540a` |
| monomorphized program | `e8aa64cce32a48a748ad79ab7e380d8a682b0d7b08f6af71fc52e4f5e0ba40a2` |
| generated C | `a8d738b059d929f5f7f0a1d96470a6748a5d921639c03a64ab9ad8976cd7c660` |

Diagnostic categories, spans, and ordering remain guarded by the compile-fail
tests. Public library entry points are exercised by integration tests. Native
runtime behavior, target widths, and debug/release modes are guarded by the
complete conformance matrix. Debug C compilation remains `-O0`; release
compilation remains `-O3`.
