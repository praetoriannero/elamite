# Standard library

> Current implementation baseline: ordinary driver APIs remain the shipped
> 0.10 shallow compatibility library. The focused 0.11 path now implements
> owned C interoperability and stops at **Owned-model tooling and final
> conformance**.

The compiler ships the `std` package from `stdlib/src/`. These files pass
through ordinary parsing, resolution, checking, and monomorphization. The
intrinsic inventory in `src/standard.rs` is limited to representations and
native hooks that safe Elamite source cannot express.

Public APIs favor owned results at operating-system boundaries. An operation
does not return a reference into runtime backing, C library allocation, or
directory stream. Ordinary copying remains shallow throughout these modules.

The root source now declares the accepted 0.11 `Clone` and `Drop` traits, and
the compiler inventory exposes the structural `Copy`, `Send`, and `Sync`
capabilities to owned-model analysis. The executable 0.10 path does not invoke
`Drop` or infer owned collection behavior from those declarations; their
runtime hooks remain gated by the ordered migration milestones.

## Owned C interoperability

The 0.11 path keeps `@importc` and `@exportc` restricted to the platform C ABI
and the recursively audited ABI-safe type set. Safe references, slices,
ordinary aggregates, owning descriptors, and closure objects do not cross by
layout coincidence. Imported calls and ownership reconstruction remain
explicitly `unsafe`; C variadics and non-C calling conventions are unsupported.

`std.ffi.MaybeUninit[T]` provides move-only, ABI-aligned output storage for an
ABI-safe `T`. `pointer()` requires a mutable place and yields `*var T` for the
foreign call; `assume_init()` is unsafe and consumes the storage only after the
wrapper has checked the C success condition. `Box[T].pointer()` and
`pointer_var()` expose nonowning stable addresses. `into_raw()` visibly hands
off destruction responsibility, while unsafe `Box[T].from_raw()` restores it
only when the exact Elamite allocator contract still applies. Library-owned C
resources instead use a move-only ordinary wrapper whose `Drop` calls that
library's matching deleter. The compatibility-only `ForeignRoot` types are not
callable on 0.11.

Named functions, exact function references, and capture-free closures can be
converted to matching raw callback pointers. Capturing callback state stays in
an address-stable `Box` or `Shared` owner held until unregistration. Foreign
threads may enter through an exported function or registered callback without
collector attachment; the unsafe registration contract remains responsible
for `Send`, `Sync`, synchronization, and lifetime obligations.

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

## Owned concurrency

The 0.11 path treats `Send` and `Sync` as structural capabilities. Raw pointers
receive neither automatically; a nominal wrapper may assume either only with
an explicit `unsafe impl`. `std.thread.spawn` consumes a borrow-free `Send`
closure and produces one move-only `Thread[R]`; joining consumes the handle and
moves `R`. Dropping the handle detaches it while normal process shutdown still
waits for the child and destroys an unclaimed result.

`std.thread.scope` passes `&var Scope` to an ordinary closure. `Scope.spawn`
admits `Send` closures containing inferred scoped borrows, and scope exit joins
every remaining child before those borrows can expire. Channels move `Send`
messages and explicitly cloned endpoints increment synchronized endpoint
counts. `Mutex[T].lock` yields a move-only `MutexGuard[T]`; only `get` and
`get_var` expose the protected value, and guard destruction unlocks it.
`AtomicBool`, `AtomicI32`, and `AtomicUsize` are non-`Copy` cells whose
operations remain sequentially consistent through C99-compatible pthread
mutex hooks.
