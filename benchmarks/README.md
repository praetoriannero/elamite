# Milestone 19 performance baseline

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

## Initial observation

Recorded 2026-07-28 on Linux x86-64 under WSL2, 28 logical CPUs, rustc 1.89.0,
and GCC-compatible `cc` 15.2.0. See `m19-baseline.tsv` for the raw output.
