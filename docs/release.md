# Conformance release audit

This file is the evidence index for `roadmap.md` Milestone 19. The initial release
is a source/compiler conformance checkpoint; Cargo publishing remains disabled.

| Gate | Evidence |
| --- | --- |
| Authoritative behavior | `examples/spec_demo/elamite.toml`, `examples/spec_demo.elx`, `examples/spec_demo/expected.stdout`, and `authoritative_demo_matches_in_debug_and_release` |
| Section conformance | `tests/fixtures/conformance/README.md` and its numbered package fixtures |
| Normative ledger | `ledger.md` assigns every rule to an implementation milestone, dependency, and concrete test layer; the M19 audit found no unowned rule |
| Target/optimization matrix | `.github/workflows/conformance.yml` runs x86 and x86-64 in debug and release; the driver always applies C99, strong warnings, and the selected width flag |
| C hardening | `generated_c_is_clean_under_address_and_undefined_behavior_sanitizers` plus the driver-owned `-Wall -Wextra -Werror` flags |
| Parser/semantic robustness | `tests/robustness.rs`, the retained parser corpus, and the existing seeded parser property test |
| Runtime stress | `12_runtime_stress`, repeated in debug and release, plus the callback, trap, and C harness process tests in `tests/backend.rs` |
| Diagnostics | `malformed_semantic_inputs_stop_at_diagnostics_without_internal_leaks` and the focused compile-fail suites |
| Performance | `benchmarks/m19-baseline.sh` and `benchmarks/m19-baseline.tsv`; `cost_model.md` and the allocation/copied-byte workloads in `benchmarks/memory-cost-baseline.sh` |
| Toolchain and limitations | `docs/toolchain.md`, including Linux/multilib/Boehm prerequisites, native concurrency, and remaining foreign-thread restrictions |
| Version identity | `elamc --version` reports both the compiler version and `spec.md` revision; `command_line_version_reports_the_specification_revision` locks the format |
| Rights/notices | MIT `LICENSE`, Cargo `license = "MIT"`, the `README.md` third-party notice, `Cargo.lock`, and `publish = false` |

Run the local release gates with:

```sh
cargo fmt --check
cargo check --all-targets
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo run -- test examples/spec_demo
cargo run -- test examples/spec_demo --release
cargo run -- conformance examples/spec_demo --all-modes
cargo run -- conformance tests/fixtures/conformance --all-modes
```

The x86 cells require a working 32-bit libc, C compiler, and Boehm development
library. CI owns that installed environment; a local machine without multilib
cannot claim those cells from a frontend-only run.

Cost-changing releases must include comparable before/after
`memory-cost-baseline.sh` results, update `cost_model.md`, and describe which
measurements changed. Those observations never replace semantic or target
conformance gates.

## Implementation cost changes

Read-only call borrowing specializes proven internal direct calls with a
hidden read-only pointer convention while retaining eager value copies for
uncertain and ABI-visible calls. On the 2026-08-01 x86-64 baseline host, the
unchanged `function_loop` workload's 5,000 calls changed as follows:

| Counter | Before | After |
| --- | ---: | ---: |
| Requested allocations | 15,009 | 9 |
| Requested bytes | 530,318 | 318 |
| Scanned allocations | 10,006 | 6 |
| Scanned bytes | 210,126 | 126 |
| Explicit `memcpy` calls | 5,003 | 3 |
| Explicit `memcpy` bytes | 85,051 | 51 |

Both runs reported `0.00` seconds at the baseline timer's resolution. The
other fixed workloads retained identical allocation and explicit-copy
counters; their compile time and peak RSS varied as expected for
non-deterministic host measurements. These observations are implementation
measurements, not semantic thresholds.
