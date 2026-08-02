# Elamite implementation cost model

> Version: 2
>
> Applies to: the compiler implementation described by `spec.md`
> 0.9.0-draft on Linux x86 and x86-64
>
> Status: non-normative implementation documentation

This document explains where the current compiler copies values, allocates
storage, retains memory, and synchronizes. `spec.md` defines observable
language behavior. Nothing here gives a program a way to observe allocation
placement, collector timing, object addresses beyond the existing pointer and
identity rules, or a guaranteed complexity bound.

The current implementation deliberately favors simple, eager value copying,
with proven read-only direct calls as its first selective exception. The
compiler may remove a copy or allocation whenever the program still behaves as
if independent logical values were produced. The remaining planned direction
is temporary reuse and thread-safe copy-on-write storage; it is not implicit
mutable aliasing or a source-level move operation.

## Reading the tables

- **Semantic copy** is the independence required by the language.
- **Physical work** describes this compiler revision, not a guarantee.
- **Allocation** counts requested Elamite runtime allocations, before
  collector metadata and rounding.
- **Retention** explains why storage can remain live after the source-level
  operation finishes.
- **Implementation freedom** is work a future compiler may safely avoid.

For a value containing other values, `C(T)` below means the recursive current
cost of physically copying `T`, including any owned backing storage. `n` is a
collection length, `b` a UTF-8 byte length, and `w` the target pointer width.

## Costs by type family

| Type family | Required semantics | Current physical representation and copy | Likely allocation | Retention and implementation freedom |
| --- | --- | --- | --- | --- |
| Unit, booleans, characters, integers, floats | Independent scalar value | Inline C scalar; constant-size assignment | None | May live only in registers or be eliminated |
| `str` | Immutable UTF-8 view | Two-word byte pointer/length descriptor; copying preserves immutable backing identity | None for the copy | Literal or existing backing determines lifetime; descriptor copies may be eliminated |
| `String` | Independent mutable string value | Two-word mutable byte pointer/length descriptor; copying eagerly allocates and copies `b` bytes plus a terminator | One `b + 1` byte allocation per physical copy | Old buffers remain until GC; thread-safe COW may make read-only copies constant-size later |
| Tuples, fixed arrays, structs | Recursive value independence | Inline aggregate; fields copy recursively, so cost is the sum of `C(field)` | Only allocations required by fields | Inline scalar work and temporary aggregates may be elided |
| Enums and `Option`/`Result` | Discriminant plus active payload independence | Inline explicit C99 tag and payload; only the active payload copies recursively | Only allocations required by the active payload | Inactive storage size affects layout but performs no recursive copy |
| `Vec[T]` | Independent sequence and elements | Managed header plus contiguous backing; copy allocates an exact-length header/backing and applies `C(T)` to every element | Two allocations for a nonempty copy, one for empty | Capacity of a copy becomes exactly `n`; COW or dead-source reuse may remove backing copies |
| `Map[K, V]` | Independent entries | Managed header plus parallel contiguous key/value arrays; lookup is currently linear; copy allocates exact-length arrays and applies `C(K) + C(V)` per entry | Three allocations for a nonempty copy, one for empty | Hashes accelerate rejection but do not provide a bucket table yet; representation may change |
| `Set[T]` | Independent elements | Managed header plus contiguous array; membership is currently linear; copy allocates exact-length backing and applies `C(T)` per element | Two allocations for a nonempty copy, one for empty | Representation may become hashed or COW without changing unspecified iteration order |
| Safe references and raw pointers | Explicit alias identity | One pointer; copy preserves the same address | None | Safe references may cause pointee promotion separately; raw pointers never root storage |
| Function references/pointers | Callable identity | One C function pointer | None | May be propagated in registers |
| Closures | Nominal callable value with recursively copied captures | One managed environment pointer; construction allocates an environment, and copying allocates another environment and applies `C(capture)` to each capture | One environment allocation plus captured-value allocations | Captureless closures currently still use an environment representation; escape/copy analysis may remove it |
| `&Trait` | Explicit fat reference alias | Data pointer plus vtable pointer; copying preserves identity | None for coercion/copy once the referent exists | Coercing an address-taken local can trigger promotion |
| `Identity[T]`, `ForeignRoot`, thread/channel/mutex/atomic handles | Deliberately shared identity | One managed/raw handle pointer; copying is constant-size and preserves synchronized or registered state | Constructors allocate state; handle copies do not | These are intentional exceptions to ordinary backing independence |
| Slices, including variadic parameter packs | Immutable view | Pointer plus length; a variadic call currently materializes managed backing for its trailing arguments | One backing allocation for a nonempty variadic pack | A proven nonescaping pack may eventually use caller storage |

`Map` and `Set` operations are currently `O(n)` lookup operations. Their names
do not promise a particular hashing representation. Vector indexed access and
length are constant-time in the current implementation; inserting or removing
away from the tail shifts the remaining inline element representations.

## Costs by source operation

| Operation | Semantic behavior | Current physical work and allocation | Retention / future freedom |
| --- | --- | --- | --- |
| Binding and assignment | Destination receives an independent ordinary value | Calls the type's recursive copy helper; owned text and collections allocate eagerly | Dead-source analysis may reuse storage; trivial copies may disappear |
| Value argument | Callee cannot mutate the caller's ordinary value through the parameter | Costly arguments to eligible internal direct calls use hidden read-only storage; all other calls recursively copy before or at the call boundary | Borrowing uses one hidden pointer and removes the physical copy; uncertain or ABI-visible calls retain the eager-copy fallback |
| Return value | Caller receives an independent ordinary value | Recursive result copy where lowering records one; C may add ABI-level aggregate movement | Return-slot reuse may remove intermediate copies |
| Pattern binding | Bound payload is an ordinary value; `_` binds nothing | Active payload and named bindings copy recursively; tests and discriminants do not copy owned backing | Consuming a dead scrutinee or binding may permit reuse |
| Plain closure capture | Capture is a snapshot taken left-to-right | Closure environment allocation plus recursive capture copies | Reference/pointer captures preserve aliases; nonescaping environments may be stack or scalar replaced |
| Collection iteration | Iterable is snapshotted once and each yielded value is an ordinary copy | Recursive copy of the complete iterable, then `C(element)` for each yielded value | Loop analysis may borrow an immutable iterable while preserving snapshot behavior |
| Thread transfer | Source remains usable while destination gets an independent transfer-safe value | Same eager recursive detachment helpers currently used for ordinary values | Only reviewed synchronization handles retain identity; physical COW sharing requires a race-free protocol |
| Channel send | Argument is evaluated once and transfer-copied into a queue/rendezvous message | Message-node allocation plus `C(T)` while synchronized; receive returns the stored independent value | Queue nodes and backing stay reachable through the channel until consumed/closed/collected |
| Thread join | Every join returns an independent copy of one cached result | Native join occurs once; each call applies `C(R)` | Thread state remains reachable through handles and the runtime registry until joined/unregistered |
| `Mutex.new/read/replace/update` | Mutex identity is shared; values crossing its boundary are independent | State allocation at `new`; recursive copies on stored/read/replacement values while locked | Slow copy callbacks extend lock hold time; a future representation may shorten it without exposing references |
| Atomic operation | Atomic handle identity is shared | A native mutex protects the scalar cell in the C99 backend; operations allocate nothing after construction | May use target-provided atomic hooks later while retaining sequential consistency |
| `String`/`str` concatenation | Produces new text | Allocates result length and copies both byte ranges | Temporary/dead-input reuse or ropes are permitted if text behavior is unchanged |
| `Vec ++ Vec` | Produces an independent concatenated vector | Header plus exact backing allocation and recursive element copies from both inputs | COW or fresh-input reuse is permitted |
| Vector growth | Existing value remains the same logical vector with added capacity | Geometric capacity growth, new backing allocation, and shallow relocation of existing element representations; argument copying occurred earlier | Abandoned backing is GC-reclaimable, not immediately freed |
| Map/set growth | Existing collection retains entries | Geometric parallel-array growth and shallow relocation, after linear lookup | Old arrays remain until collection; representation may be replaced wholesale |
| `clear` | Collection becomes empty | Sets length to zero; does not shrink or release backing | Capacity and references in abandoned slots may remain conservatively retained until later overwrite/collection |
| Formatting and f-strings | Produces formatted text | Geometrically grown formatter buffer plus byte appends; displaying nested values walks them | Buffer reuse and size precomputation are permitted |
| Safe reference formation | Reference preserves place identity and lifetime | Address-taken local is conservatively promoted to one managed cell for the function invocation | Current promotion answers only “address taken”; precise escape analysis may keep nonescaping cells on stack |
| `defer` | Executes registered code at lexical exits | Registrations are static control-flow edges, not closure allocations; deferred calls have their ordinary argument/copy costs when executed | Compiler may simplify edges while preserving reverse registration order |

Collection mutators receive already evaluated ordinary arguments. Consequently,
copying a large inserted key or value can dominate the table operation even
before its current linear search or growth work is considered.

### Read-only call borrowing

The compiler specializes concrete internal direct-call instances when a
parameter has a recursive, owned-buffer, or runtime-managed copy strategy and
the typed body proves that the parameter's source storage is never mutated or
address-exposed. The generated C function receives a hidden `const T *` for
that parameter, and the call site passes the address of its already evaluated
temporary. Calls remain synchronous, so that storage remains live for the
entire invocation.

Returning, storing, capturing, or forwarding a parameter across an uncertain
boundary still goes through the ordinary logical-copy operation at that use.
The optimization therefore removes only the redundant entry copy; it does not
change the source parameter type, create an Elamite reference, or make mutable
storage observable through an alias. Recursive and separately monomorphized
generic direct calls can use the specialized convention.

The compiler conservatively retains owned value parameters for indirect
function calls, closure and trait-object dispatch, vtable entries, foreign
imports and exports, and any function whose address is used as a value. It also
falls back for trivial or identity-preserving types, promoted/address-taken
parameters, mutating receiver operations, and any analysis uncertainty. This
keeps every source-visible and foreign ABI stable.

## Compiler-side logical-copy inventory

The checker records why each source expression needs a logical copy and the
coarse lifetime boundary it crosses. Typed IR combines that context with the
concrete type's allocation class: no allocation, preserved identity, recursive
copying, owned-buffer copying, or runtime-managed copying. It also distinguishes
ordinary value copies from transfer copies without yet selecting different
runtime helpers for them.

Before control-flow lowering, read-only call analysis changes eligible argument
copies into an explicit borrowed passing mode. Control-flow lowering preserves
the remaining copy facts on every explicit `Copy` rvalue and assigns a stable,
per-function copy ID. Debug compiler builds verify that the emitted IDs are
unique and form a complete sequence, and the public IR exposes the same
inventory for optimizer tests and measurements. This inventory counts logical
copy operations that remain after proven call borrowing, not recursive field
copies, allocator requests, ABI traffic, or physical bytes. It adds no
generated-C counters, output, or runtime behavior in release programs; the
separate opt-in `elamite-cost-v1` instrumentation below remains the source of
physical allocation and byte-copy measurements.

Copies performed inside today’s shared synchronization helpers remain physical
implementation work rather than additional IR copy records. Their call-site
arguments already carry transfer purpose; the later ordinary/transfer helper
separation package will lift the helper-internal boundary operations into their
own selectable IR form.

## Allocation, garbage collection, and retained memory

Elamite uses a non-moving Boehm collector whenever lowered code requires
managed storage. Programs that need no managed storage do not link it. The
collector traces stacks conservatively, permits interior safe references, and
reclaims unreachable cycles, but collection timing is unspecified.

Important consequences of the current implementation are:

- allocation can occur implicitly during binding, calls, returns, captures,
  iteration, formatting, transfer, synchronization, and safe-reference
  promotion;
- allocation failure performs one full collection and retries before the
  process-fatal OOM path;
- clearing or growing a collection does not immediately return backing memory;
- conservative stack scanning can retain an otherwise unreachable allocation
  when an integer-like stack word resembles its address;
- raw pointers do not register roots, while safe references and open foreign
  roots do;
- collector metadata, size-class rounding, native thread stacks, C library
  buffers, and allocator fragmentation are not included in requested-byte
  counts; and
- peak RSS is therefore an upper-bound observation, not the sum of requested
  Elamite allocations and not a deterministic semantic property.

## Synchronization costs

Native threads are joinable pthreads. Thread creation allocates runtime state
and a copied startup environment, and program shutdown joins every remaining
Elamite-created thread. Channels use one mutex and condition variables around
their queue; bounded sends can block, capacity zero performs a rendezvous, and
unbounded sends allocate a node. Mutex values are copied while their lock is
held. The three sequentially consistent atomic cell types currently use a
native mutex per cell rather than C11 `_Atomic`, preserving the C99 target.

These synchronization operations establish the normative ordering described
by `spec.md`; their wall time and fairness are intentionally unspecified.
Standard output also takes a process-wide lock for each complete output call
when concurrency is reachable.

## x86 versus x86-64

Both targets implement the same value behavior. On x86, pointers, `isize`,
`usize`, collection lengths/capacities, hidden borrowed-argument pointers, and
pointer-bearing descriptors use 32 bits; on x86-64 they use 64 bits. Thus a
two-word string, slice, or trait-object descriptor ordinarily occupies 8 bytes
on x86 and 16 bytes on x86-64 before C ABI alignment, while vector/set headers
contain three target words and map headers contain four. Closure environments,
aggregate padding, native mutexes, thread state, and collector metadata also
follow the selected C ABI.

Never infer an exact layout for FFI from this document. Imported C declarations
and the generated C compiler determine ABI layout. Run the baseline separately
for each target because requested bytes and peak memory are not directly
comparable across widths.

## Reproducing the baseline

Build and measure every fixed workload with:

```sh
./benchmarks/memory-cost-baseline.sh > benchmarks/memory-cost-baseline.tsv
```

The checked-in observation is
[`benchmarks/memory-cost-baseline.tsv`](../benchmarks/memory-cost-baseline.tsv).
It records the source hash, release compiler/toolchain identity, target, wall
time, peak RSS, requested allocations, and explicit byte-copy totals. Use
`ELAMITE_BENCH_TARGET=x86` for a native 32-bit observation and `ELAMC_BIN` to
select a prebuilt compiler.

Instrumentation is enabled only by passing:

```sh
--c-flag=-DELAMITE_COST_INSTRUMENTATION=1
```

The developer-only counter implementation uses GCC/Clang atomic builtins so
concurrent workloads cannot lose increments. Ordinary generated C remains C99
and has no dependency on those builtins when instrumentation is disabled.

An instrumented executable writes one tab-separated `elamite-cost-v1` record
to standard error at normal process exit. Counters are safe under the supported
native-thread runtime. They mean:

- `allocations` / `allocated_bytes`: successful requests through Elamite's
  scanned or pointer-free runtime allocators;
- `scanned_allocations` / `scanned_bytes`: the subset whose memory may contain
  managed pointers; and
- `memcpy_calls` / `memcpy_bytes`: explicit generated `memcpy` operations used
  for owned text, formatting, and backing relocation.

The counters deliberately exclude `memmove`, scalar and recursive C
assignments, parameter/return ABI traffic, collector internals, foreign
allocations, and C library work. They are stable measurement fields, not a
claim that `memcpy_bytes` equals all physical memory traffic. Instrumentation
changes runtime cost and output and must not be enabled for semantic
conformance tests.

## Maintenance contract

A change must update this document in the same change when it alters a cost
described here. This includes representation, logical-copy lowering, backend
copy helpers, collection growth, formatter behavior, promotion, managed-memory
strategy, concurrency transfer, synchronized storage, or instrumentation
schema changes.

For a material cost change:

1. Record a before and after release-mode baseline using identical workload
   source hashes, target, host class, compiler, C toolchain, and environment.
2. Report time, peak RSS, requested allocations/bytes, and explicit copied
   bytes; explain metrics the instrumentation cannot observe.
3. Update the relevant current-behavior table and keep future targets clearly
   labeled as targets rather than achieved guarantees.
4. Summarize user-visible cost changes in `docs/release.md`.
5. Run ordinary semantic conformance independently; benchmarks carry no
   timing, memory, or allocation pass threshold.

Changing a normative complexity or allocation guarantee is a separate language
design decision. It requires a reviewed `spec.md` change, target coverage, and
compatibility analysis; editing this implementation document alone cannot make
such a promise.
