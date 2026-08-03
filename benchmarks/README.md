# Compiler performance and memory-cost baselines

## Memory-cost baseline

`memory-cost-baseline.sh` builds fixed workloads in release mode with the
opt-in `ELAMITE_COST_INSTRUMENTATION` generated-C instrumentation. It records
compile and runtime wall time, peak resident memory, requested runtime
allocations and bytes, scanned allocation totals, and bytes passed through
explicit generated `memcpy` operations.

The workloads beneath `memory-costs/` use these fixed inputs:

| Case | Input |
| --- | --- |
| `string_copy` | 10,000 copies of one 128-byte `String` |
| `vector_copy` | 2,000 copies of one 128-element `Vec[i32]` |
| `map_set_copy` | 1,000 copies each of a 64-entry `Map` and `Set` |
| `aggregate_closure` | 2,000 copies and calls of a closure capturing a nested aggregate |
| `function_loop` | 5,000 calls receiving a `String`/16-element-vector aggregate |
| `cross_thread` | 1,000 channel transfers of an eight-`String` vector |

Source hashes are included in every result so changed inputs cannot be
mistaken for a compiler-only comparison.

Run from the repository root:

```sh
./benchmarks/memory-cost-baseline.sh > benchmarks/memory-cost-baseline.tsv
```

Set `ELAMITE_BENCH_TARGET=x86` to measure the supported 32-bit target. The
script never applies pass/fail thresholds: compare only identical source
hashes on comparable hosts and toolchains.

The checked-in `memory-cost-baseline.tsv` observation was recorded on
2026-08-03 for the completed 0.10 migration, under WSL2 for x86-64 with rustc
1.89.0 and GCC-compatible `cc` 15.2.0. Every fixed source hash and every
allocation, allocated-byte, scanned-allocation, and explicit-copy counter is
unchanged from the immediately preceding shallow baseline; only nondeterministic
time and peak-RSS observations moved. The script supports x86 as well; this
host built but could not execute the 32-bit instrumented program, so a native
32-bit observation remains a host/CI measurement rather than a fabricated
cross-width comparison.

## Milestone 19 performance baseline

`m19-baseline.sh` measures release compilation wall time and peak resident
memory, generated-C and native artifact sizes, and native runtime wall time and
peak resident memory. Its inputs are the authoritative demonstration, the
cross-section runtime stress fixture, and the C ABI/FFI demo.

Run from the repository root after installing the native prerequisites:

```sh
./benchmarks/m19-baseline.sh
```

The numbers are observations, not performance assertions. Compare results only
on a comparable host and toolchain; semantic correctness gates remain
independent.

## Current conformance observation

Recorded 2026-08-03 for the implemented 0.10.0-draft release identity on Linux
x86-64 under WSL2, 28 logical CPUs, rustc 1.89.0, and GCC-compatible `cc`
15.2.0. The authoritative demonstration now includes its 0.10 shallow-memory,
thread-publication, and unsafe-pointer regions. See `m19-baseline.tsv` for the
raw output; timing and peak RSS remain observations rather than thresholds.
