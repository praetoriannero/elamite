# Standard library

The compiler ships the `std` package from `stdlib/src/`. These files pass
through ordinary parsing, resolution, checking, and monomorphization. The
intrinsic inventory in `src/standard.rs` is limited to representations and
native hooks that safe Elamite source cannot express.

Public APIs favor owned results at operating-system boundaries. An operation
does not return a reference into a managed buffer, C library allocation, or
directory stream. Ordinary copying remains shallow throughout these modules.

## System modules

`std.fs.Path` owns a `String`. Its `from`, `join`, `parent`, and `file_name`
operations are lexical; filesystem access begins only at `open`, `read_dir`,
`metadata`, creation, removal, or rename. Directory entries own both their
complete child path and final name. An embedded NUL is retained by lexical
operations but reported as `InvalidInput` at an operating-system boundary.

`File` and `Directory` are shared-identity native handles. Their copies observe
one closed state. `close()` is safe, unit-returning, and idempotent, so it can be
registered directly with `defer`. `File` provides `read_to_end`, `write_all`,
and metadata; `Directory.next` returns one owned entry, end-of-stream, or an
I/O failure. Operations other than `close` report `InvalidInput` after the
shared handle has closed. `read_to_end` reads until EOF, including from native
files whose metadata does not predict their eventual byte count.

`IoError` deliberately does not expose `errno`. Its exhaustive portable
categories are `NotFound`, `PermissionDenied`, `AlreadyExists`, `InvalidInput`,
`IsDirectory`, `NotDirectory`, `DirectoryNotEmpty`, `ReadOnly`, `BrokenPipe`,
`Interrupted`, `WouldBlock`, `TimedOut`, `StorageFull`, `ResourceExhausted`,
`Unsupported`, and `Other`.

`std.env.args` returns an owned argument snapshot; invalid UTF-8 argument bytes
are replaced individually with U+FFFD so every result remains a valid
`String`. `get` distinguishes a missing variable from an invalid name or
non-text host value, and `current_dir` returns an owned `Path`. Non-text current
directories or directory-entry names are `IoError.InvalidInput`. No setter is
provided, so this surface introduces no new process-global mutation.

`std.process.run` invokes a program directly without a shell or an implicit
environment mutation. Launch failures are `ProcessError`; every completed
child, including a nonzero exit, returns `Output` with byte-vector stdout and
stderr. Capture uses native temporary files before materializing those vectors,
so simultaneous output on both streams cannot fill a parent-side pipe.
`exit` terminates immediately and does not run deferred cleanup.

## Time and randomness

`std.time.Instant` is process-local monotonic time. `SystemTime` is wall time
since the Unix epoch. The types do not interoperate, and neither clock read is a
synchronization edge. `Duration` stores nonnegative nanoseconds; construction
and arithmetic that may overflow return `Option`.

`std.random.Generator` implements SplitMix64. `seeded` is the only constructor:
it never consults a clock, environment variable, or operating-system entropy
source. The algorithm and fixed-seed sequence are compatibility commitments.
`below` and `between` use rejection sampling and do not advance the generator
for an empty range.

## Ordering and text

`std.ordering.sort` is a stable, allocation-free insertion sort over shared
vector backing. `binary_search` and `binary_search_vec` return the first equal
index. These deliberately small baselines use the ordinary `Ord` rules and may
be replaced by algorithms with the same stability and allocation contracts.

`std.text` separates borrowed and allocating results. `find`, `contains`,
`trim`, and borrowed `split` do not copy substring bytes; `split` allocates its
result vector. `split_string`, `trim_string`, and case conversion materialize
owned strings. Case conversion maps ASCII letters, maps `ß` to `SS` when
uppercasing, and otherwise preserves non-ASCII scalars. Text matching is exact
and does not normalize. Numeric parsing
accepts documented ASCII syntax and reports `Empty`, `InvalidSyntax`, or
`OutOfRange` without trapping.

The current representation and operation costs are recorded in
`cost_model.md`; they are implementation documentation rather than semantic
complexity guarantees.
