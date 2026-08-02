# Leibniz pi benchmark

This package computes one billion terms of the direct Leibniz series for pi.
It is intentionally a simple, CPU-bound floating-point loop rather than an
efficient way to calculate pi.

Build the native executable in release mode before measuring it so compiler
startup and C compilation are excluded from the result:

```sh
cargo run --release -- build examples/leibniz_pi --release \
    --out-dir=/tmp/elamite-leibniz-pi
/usr/bin/time -f 'elapsed=%e seconds, peak_rss=%M KiB' \
    /tmp/elamite-leibniz-pi/leibniz_pi
```

Elapsed time depends on the CPU, native C compiler, target, and system load.

## Reference observation

On 2026-08-01, an x86-64 release build produced
`3.1415926525880504` in 0.74 seconds on each of three runs. The test host was
an Intel Core i7-14700K under WSL2 using GCC-compatible `cc` 15.2.0. This is
about 1.35 billion loop iterations per second and is an observation, not a
performance guarantee.
