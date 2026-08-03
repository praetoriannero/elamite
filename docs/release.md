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

### Temporary and return storage reuse

Fresh temporary copies and conservatively dead local returns now retain their
semantic copy records but forward existing storage instead of allocating an
independent physical duplicate. Compared with the preceding read-only-call
baseline on the same host, toolchain, target, and unchanged workload hashes:

| Workload | Allocations, before → after | Allocated bytes, before → after | `memcpy` bytes, before → after |
| --- | ---: | ---: | ---: |
| `aggregate_closure` | 16,042 → 16,015 | 376,880 → 376,328 | 50,150 → 50,050 |
| `cross_thread` | 41,075 → 41,046 | 785,568 → 785,128 | 128,224 → 128,128 |
| `function_loop` | 9 → 3 | 318 → 106 | 51 → 17 |
| `map_set_copy` | 5,019 → 5,017 | 825,600 → 825,544 | 720 → 720 |
| `string_copy` | 10,003 → 10,002 | 1,290,386 → 1,290,257 | 1,280,384 → 1,280,256 |
| `vector_copy` | 4,008 → 4,007 | 1,073,056 → 1,073,032 | 496 → 496 |

All runs remained below the timer's `0.01`-second resolution. Compile time and
peak RSS varied with the host and are recorded in the baseline without being
treated as regressions. The formatter runtime also rejects impossible-size
growth through the process-fatal OOM path before `size_t` arithmetic can
overflow.

### Copy-on-write `String`

Logical `String` copies now share immutable, pointer-free backing through an
atomic sticky shared flag; the first mutable access after sharing detaches, and
subsequent access to that unique replacement reuses it. The two-word value
descriptor and foreign ABI exclusion are unchanged. Compared with the
temporary/return-reuse baseline on the same host, toolchain, target, and
unchanged workload hashes:

| Workload | Allocations, before → after | Allocated bytes, before → after | `memcpy` calls / bytes, before → after |
| --- | ---: | ---: | ---: |
| `aggregate_closure` | 16,015 → 10,012 | 376,328 → 320,324 | 6,006 / 50,050 → 3 / 25 |
| `cross_thread` | 41,046 → 9,022 | 785,128 → 625,072 | 32,032 / 128,128 → 8 / 32 |
| `function_loop` | 3 → 3 | 106 → 114 | 1 / 17 → 1 / 17 |
| `map_set_copy` | 5,017 → 5,017 | 825,544 → 825,544 | 12 / 720 → 12 / 720 |
| `string_copy` | 10,002 → 2 | 1,290,257 → 265 | 10,003 / 1,280,256 → 3 / 256 |
| `vector_copy` | 4,007 → 4,007 | 1,073,032 → 1,073,032 | 5 / 496 → 5 / 496 |

All runtimes remained below the timer's `0.01`-second resolution. The eight
additional `function_loop` bytes are the one-word shared-backing header; its
allocation count is unchanged. String and other pointer-free byte buffers now
use the atomic allocator, so the new `string_copy` run reports zero scanned
allocations. Map/set and vector counters are unchanged, and compile time and
peak RSS remain non-deterministic host observations rather than thresholds.

### Shallow ordinary-copy lowering

Assignment, arguments, returns, tuple destructuring, pattern binding, closure
capture/copy, indexing results, postfix `?`, iteration yields, and aggregate
construction now retain explicit copy records while lowering to immediate C
representation assignments. Recursive helpers remain only for the transitional
thread/channel/join/mutex transfer boundary. A focused debug/release alias
matrix covers every migrated value flow.

Compared with the preceding COW `String` baseline on the same host, toolchains,
target, and unchanged workload hashes:

| Workload | Allocations, before → after | Allocated bytes, before → after | Scanned allocations/bytes, before → after |
| --- | ---: | ---: | ---: |
| `aggregate_closure` | 10,012 → 8 | 320,324 → 212 | 8,007 / 256,208 → 4 / 128 |
| `cross_thread` | 9,022 → 5,020 | 625,072 → 320,920 | 9,014 / 624,968 → 5,012 / 320,816 |
| `function_loop` | 3 → 3 | 114 → 114 | 1 / 24 → 1 / 24 |
| `map_set_copy` | 5,017 → 17 | 825,544 → 1,544 | 2,002 / 56,056 → 2 / 56 |
| `string_copy` | 2 → 2 | 265 → 265 | 0 / 0 → 0 / 0 |
| `vector_copy` | 4,007 → 7 | 1,073,032 → 1,032 | 2,001 / 48,024 → 1 / 24 |

Explicit `memcpy` counts and bytes were unchanged in all six workloads because
the removed recursive copies used generated assignments and managed allocation,
not the instrumentation's byte-buffer `memcpy` path. All runtimes remained
below `0.01` seconds. Compile time and peak RSS varied and remain
non-deterministic observations rather than semantic or performance thresholds.

### Shallow standard collection representations

`String` no longer carries a sticky COW flag or detaches before mutable-byte
access. Ordinary descriptors alias writable backing directly. `Vec[T]` is now
an inline pointer/length/capacity descriptor: element writes alias shared
backing, while length and capacity updates remain local and growth may give one
descriptor replacement backing. `Map` and `Set` retain their existing shared
table handles, whose structural mutations were already identity-preserving.

Compared with the preceding shallow ordinary-copy baseline on the same host,
toolchains, target, and unchanged workload hashes:

| Workload | Allocations, before → after | Allocated bytes, before → after | Explicit `memcpy` calls / bytes, before → after |
| --- | ---: | ---: | ---: |
| `aggregate_closure` | 8 → 6 | 212 → 172 | 3 / 25 → 3 / 25 |
| `cross_thread` | 5,020 → 19,033 | 320,920 → 368,912 | 8 / 32 → 16,024 / 64,096 |
| `function_loop` | 3 → 2 | 114 → 82 | 1 / 17 → 1 / 17 |
| `map_set_copy` | 17 → 17 | 1,544 → 1,544 | 12 / 720 → 12 / 720 |
| `string_copy` | 2 → 2 | 265 → 257 | 3 / 256 → 3 / 256 |
| `vector_copy` | 7 → 6 | 1,032 → 1,008 | 5 / 496 → 5 / 496 |

The temporary `cross_thread` increase is deliberate: the still-implemented
0.9 transfer boundary promises independent ordinary storage, so mutable
`String` bytes must now be copied eagerly instead of sharing immutable COW
backing. The ordered C-like thread/channel publication package removes that
legacy transfer helper and this temporary proportional copy. The other reduced
counts come from removing String flag words and heap-allocated vector headers.
All runtimes remained below the timer's `0.01`-second resolution; compile time
and peak RSS remain non-deterministic observations rather than thresholds.

## Accepted 0.10 language revision

The normative draft now specifies shallow fieldwise ordinary copies, Go-like
`Vec` descriptors, identity-preserving `Map`/`Set` tables, shallow mutable
`String` backing, and shallow closure/thread/channel/join/mutex values. The
structural `Transfer` capability and data-race-free-safe-code claim are removed;
conflicting unordered cross-thread access is C99 undefined behavior and
synchronization is the programmer's responsibility.

Raw data pointers gain unsafe element-scaled arithmetic, compound pointer
updates, same-extent subtraction, unchecked indexing, and relational ordering.
Null is ordered below every non-null pointer; two non-null pointers may be
ordered only within one live extent. Integer-pointer conversion, function-
pointer arithmetic, and ordering through `PartialOrd`/`Ord` remain absent.

This section records the complete accepted design, not a claim of complete
implementation. Shallow ordinary-copy lowering and standard collection
representations have landed, while the compiler version output and
demonstration remain 0.9 and the remaining iteration, concurrency, and pointer
packages continue in `roadmap.md`.
