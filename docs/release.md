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
representation assignments. At this stage recursive helpers remained only for
the transitional mutex boundary; the later mutex package below removes that
last family. A focused debug/release alias matrix covers every migrated value
flow.

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

The temporary `cross_thread` increase recorded here came from the then-active
0.9 publication boundary, which copied mutable `String` bytes eagerly instead
of sharing backing. The later thread/channel publication section records its
removal. The other reduced counts come from removing String flag words and
heap-allocated vector headers.
All runtimes remained below the timer's `0.01`-second resolution; compile time
and peak RSS remain non-deterministic observations rather than thresholds.

### Iteration state and mutation invalidation

`for` now emits its hidden shallow iterable and length snapshots once before
the first condition check instead of re-reading collection length on every
iteration. Vector element replacement and existing map-value replacement stay
visible through shared backing. Length-changing vector mutation and structural
map/set mutation during an active loop remain accepted source but are
documented undefined behavior, so regression coverage compiles those cases
without executing them.

This removes one constant-time length read per iteration after the initial
snapshot and changes no managed-allocation or explicit-byte-copy counter. It is
not a material memory-cost change, so the collection-representation baseline
remains the comparable checked-in measurement.

### C-like thread and channel publication

The compiler no longer exposes or recognizes a structural `Transfer`
capability. Spawn environments, cached and repeated join results, and channel
messages now use the same shallow immediate copies as ordinary values. Safe
references, raw pointers, trait objects, strings, and collection backing may
therefore cross these boundaries; synchronization publishes the immediate
representation but does not detach backing or make later conflicting access
race-free.

Focused debug/release runtime coverage exercises shared vector backing across
spawn and repeated joins, shared channel message backing, and one registered
worker consuming safe-reference, raw-pointer, and trait-object aliases. The
existing lifecycle, failure, closure, contention, collector, sanitizer, and
target-width suites remain in place. At this stage mutex values retained
temporary recursive isolation for the next ordered package.

Compared with the preceding collection-representation baseline on the same
host, toolchains, target, and unchanged workload hashes:

| Workload | Allocations, before → after | Allocated bytes, before → after | Explicit `memcpy` calls / bytes, before → after |
| --- | ---: | ---: | ---: |
| `cross_thread` | 19,033 → 1,013 | 368,912 → 32,512 | 16,024 / 64,096 → 8 / 32 |

All other workload allocation and explicit-copy counters were unchanged. The
cross-thread workload also reduced scanned allocation from 3,009 allocations /
288,792 bytes to 1,005 allocations / 32,472 bytes. All runtimes remained below
the timer's `0.01`-second resolution; compile time and peak RSS remain
non-deterministic observations rather than thresholds.

### Programmer-managed mutex values

`Mutex[T].new`, `read`, `replace`, and `update` now assign only the immediate
`T` representation while holding the mutex lock. Descriptor-bearing values
therefore retain shared backing across the mutex boundary: locking serializes
operations on the stored representation but does not isolate backing or
automatically synchronize access through external aliases. The compiler's
temporary isolation purpose and recursive per-type copy-helper family have
been removed.

Focused debug/release coverage checks visible vector aliases through every
mutex operation and verifies direct C assignments. A deliberately racy
external-alias example is compile-only evidence that the compiler does not
claim data-race freedom; it is never executed as a conformance or sanitizer
test. The fixed memory-cost workloads do not perform mutex value operations,
so their requested-allocation and explicit-copy counters remain unchanged in
the comparable post-change baseline; this removal is instead guarded by the
focused generated-C and runtime alias tests.

### Unsafe pointer arithmetic and indexing

Raw data pointers with complete, nonzero-sized pointees now support unsafe
element-scaled `+` and `-`, mutable-place `+=` and `-=`, same-pointee pointer
subtraction to `isize`, and unchecked `pointer[index]` places. Function,
incomplete, zero-sized, and `std.ffi.CVoid` pointees are rejected. Mixed
`*T`/`*var T` subtraction is accepted when the resolved pointee agrees.

Index lowering evaluates the pointer and signed index once in left-to-right
order, forms the adjusted pointer, and routes the access through the existing
mandatory null/alignment trap path. Focused debug/release tests cover negative
indices, assignment, compound offsets, one-past construction, pointer distance,
and exactly-once side effects; generated C is checked for both x86 and x86-64.
These operations add no managed allocation or explicit byte copying, and the
fixed cost workloads do not use them, so no comparable baseline counter changes.

### Unsafe pointer relational ordering

Raw data pointers now support unsafe `<`, `<=`, `>`, and `>=` as primitive
operations, independently of `PartialOrd` and `Ord`. Mixed `*T`/`*var T`
operands are accepted for the same resolved data pointee. Null is below every
non-null pointer, while ordering two non-null pointers is defined only when
both positions belong to the same live extent; unrelated-pointer ordering is
accepted as an unsafe expression but remains undefined behavior if executed.

The C99 backend tests null explicitly and reaches an unsigned-byte-pointer
relational comparison only when both operands are non-null, so it never asks C
to relationally compare a null pointer. Focused tests cover every null case,
same-extent traversal, mixed mutability, diagnostics, x86/x86-64 generated C,
debug/release execution, and address/undefined-behavior sanitizers. Ordering is
constant-size work with no managed allocation or explicit byte copy, and the
fixed cost workloads do not use it, so their baseline counters remain
unchanged.

### Concurrent memory-model conformance

The runtime synchronization audit now maps every Section 10.4 ordering edge to
its C99/POSIX implementation and executable evidence. Thread creation publishes
earlier writes to the worker; a matching channel receive observes writes before
send; successive operations on one mutex can coordinate external ordinary
backing; sequentially consistent atomic flags publish ordinary backing; and a
successful join publishes worker writes to the joining thread. None of these
ordinary shared accesses requires `unsafe` syntax.

The high-contention fixture now exercises all five edges over shared vector
backing and includes a coordinated store-buffering litmus that must never
observe the SC-forbidden outcome. It remains clean under TSan, debug/release
repetition, AddressSanitizer/UndefinedBehaviorSanitizer, and the available
x86/x86-64 conformance matrix. A deliberately racy external-alias fixture is
compile-only evidence and is never executed. The audit required no runtime or
representation change, so it changes neither allocation/copy costs nor the
fixed memory baseline.

### Final 0.10 cost and release identity

The final release-mode x86-64 baseline used the same six workload hashes and
the same WSL2, rustc 1.89.0, and GCC-compatible 15.2.0 toolchain class as the
preceding shallow baseline. Every deterministic counter was unchanged:

| Workload | Allocations | Allocated bytes | Scanned allocations / bytes | Explicit `memcpy` calls / bytes |
| --- | ---: | ---: | ---: | ---: |
| `aggregate_closure` | 6 | 172 | 2 / 112 | 3 / 25 |
| `cross_thread` | 1,013 | 32,512 | 1,005 / 32,472 | 8 / 32 |
| `function_loop` | 2 | 82 | 0 / 0 | 1 / 17 |
| `map_set_copy` | 17 | 1,544 | 2 / 56 | 12 / 720 |
| `string_copy` | 2 | 257 | 0 / 0 | 3 / 256 |
| `vector_copy` | 6 | 1,008 | 0 / 0 | 5 / 496 |

Wall time and peak RSS varied without semantic significance. The local host
built but could not execute the x86 instrumented artifact, so no cross-width
cost numbers are claimed; the installed multilib CI matrix continues to own
x86 semantic execution.

`elamc --version` now reports `SPEC 0.10.0-draft`. The authoritative
demonstration covers shallow vector/map/set identity, safe shared backing across
spawn/join, and unsafe pointer arithmetic, indexing, same-extent ordering, and
null-low ordering in both debug and release. README, specification, ledger,
toolchain, architecture, roadmap, cost model, fixtures, and release evidence
now identify the same implemented revision. The broader `m19-baseline.tsv` was
also refreshed after the demonstration change so its generated-C and native
artifact sizes describe the released inputs.

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

All packages in the accepted design are implemented: shallow
ordinary-copy lowering, standard collection representations, iteration
invalidation, thread/channel/mutex publication, the concurrent memory model,
the complete accepted raw-pointer surface, final cost evidence, release
identity, and the authoritative demonstration.
