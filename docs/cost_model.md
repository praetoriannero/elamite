# Elamite implementation cost model

> Version: 6
>
> Applies to: the transitional 0.10 migration compiler on Linux x86 and x86-64
>
> Status: non-normative implementation documentation

> Implementation revision: **Shallow collection representations complete**. The
> compiler directly copies immediate C representations for ordinary language
> copies, including aggregate, collection, and closure values. `String`, `Vec`,
> `Map`, and `Set` now use the accepted shallow representations. The compiler
> still retains structural `Transfer` and copy-based mutex behavior until their
> ordered migration packages land.

This document explains where the current compiler copies values, allocates
storage, retains memory, and synchronizes. `spec.md` defines the accepted 0.10
observable behavior, while this document describes the partially migrated
compiler and its measurements. Nothing here creates an allocation,
timing, collector, address, or complexity guarantee.

Ordinary assignment, argument, return, capture, pattern, indexing, propagation,
and aggregate copies now copy only their immediate representation. Inline
aggregate storage is distinct, while nested descriptors and handles retain
identity. Legacy transfer helpers still recursively materialize values for
threads, channels, joins, and mutex operations. This transitional split is an
implementation boundary, not the final 0.10 concurrency contract.

## Reading the tables

- **Ordinary copy** means the implemented shallow 0.10 value operation.
- **Transfer copy** means the temporary pre-migration recursive boundary used
  by the current concurrency runtime.
- **Physical work** describes this compiler revision, not a guarantee.
- **Allocation** counts requested Elamite runtime allocations, before
  collector metadata and rounding.
- **Retention** explains why storage can remain live after the source-level
  operation finishes.
- **Implementation freedom** is work a future compiler may safely avoid.

For transfer operations, `C(T)` below means the recursive current cost of
materializing `T`, including owned backing storage. `n` is a collection length,
`b` a UTF-8 byte length, and `w` the target pointer width.

## Costs by type family

| Type family | Required semantics | Current physical representation and copy | Likely allocation | Retention and implementation freedom |
| --- | --- | --- | --- | --- |
| Unit, booleans, characters, integers, floats | Independent scalar value | Inline C scalar; constant-size assignment | None | May live only in registers or be eliminated |
| `str` | Immutable UTF-8 view | Two-word byte pointer/length descriptor; copying preserves immutable backing identity | None for the copy | Literal or existing backing determines lifetime; descriptor copies may be eliminated |
| `String` | Shallow mutable backing identity | Two-word byte-pointer/length descriptor; ordinary copy aliases writable backing directly | Construction allocates `b + 1` pointer-free bytes; ordinary copy allocates nothing | Legacy transfer copies allocate and copy `b + 1` bytes until thread publication migrates; ordinary mutation never detaches |
| Tuples, fixed arrays, structs | Immediate inline slots copy; nested backing identities remain shared | One C aggregate assignment, proportional only to inline representation size | None merely for copying | C may lower a large inline assignment to moves or `memcpy`; reachable managed contents are not traversed |
| Enums and `Option`/`Result` | Discriminant and active inline payload copy shallowly | One explicit-tag C99 aggregate assignment | None merely for copying | Inactive payload storage affects layout but no reachable backing is traversed |
| `Vec[T]` | Ordinary copies share backing while retaining descriptor-local length and capacity | Inline pointer/length/capacity descriptor; ordinary copy assigns three target words | None for a copy | Element writes alias within both ranges; growth updates one descriptor and may diverge from its copies |
| `Map[K, V]` | Ordinary copies preserve complete mutable table identity | One managed-header pointer; lookup remains linear | None for a copy | Structural mutation is visible through every ordinary copy; hashing remains an implementation choice |
| `Set[T]` | Ordinary copies preserve complete mutable table identity | One managed-header pointer; membership remains linear | None for a copy | Structural mutation is visible through every ordinary copy; hashing remains an implementation choice |
| Safe references and raw pointers | Explicit alias identity | One pointer; copy preserves the same address | None | Safe references may cause pointee promotion separately; raw pointers never root storage |
| Function references/pointers | Callable identity | One C function pointer | None | May be propagated in registers |
| Closures | Construction shallow-copies captures once; callable copies preserve environment identity | One managed environment pointer; construction allocates one environment, ordinary copying copies only the pointer | One allocation at construction; none for a copy | Legacy transfer still materializes an environment recursively; captureless environments may be optimized away |
| `&Trait` | Explicit fat reference alias | Data pointer plus vtable pointer; copying preserves identity | None for coercion/copy once the referent exists | Coercing an address-taken local can trigger promotion |
| `Identity[T]`, `ForeignRoot`, thread/channel/mutex/atomic handles | Shared identity | One managed/raw handle pointer; copying is constant-size and preserves synchronized or registered state | Constructors allocate state; handle copies do not | Ordinary shallow copying treats these like every other identity-bearing descriptor |
| Slices, including variadic parameter packs | Immutable view | Pointer plus length; a variadic call currently materializes managed backing for its trailing arguments | One backing allocation for a nonempty variadic pack | A proven nonescaping pack may eventually use caller storage |

`Map` and `Set` operations are currently `O(n)` lookup operations. Their names
do not promise a particular hashing representation. Vector indexed access and
length are constant-time in the current implementation; inserting or removing
away from the tail shifts the remaining inline element representations.

## Costs by source operation

| Operation | Semantic behavior | Current physical work and allocation | Retention / future freedom |
| --- | --- | --- | --- |
| Binding and assignment | Destination receives an ordinary shallow value | One immediate scalar, pointer, descriptor, or C aggregate assignment | No allocation merely for the copy; broader last-use analysis may still remove inline movement |
| Value argument | Callee receives a shallow value and may observe shared backing through descriptors | Owned ABI passes the immediate representation; eligible internal direct calls may still use a hidden read-only pointer to avoid large inline C movement | Uncertain and ABI-visible calls retain the owned ABI but no longer recursively materialize backing |
| Return value | Caller receives an ordinary shallow value | One immediate return representation; the existing reuse pass still records proven source handoffs | C may add ABI-level aggregate movement; no managed backing is traversed |
| Pattern binding | Bound payload is a shallow value; `_` binds nothing | Active payload and named inline representations assign directly | Tests and discriminants allocate nothing; nested descriptors preserve identity |
| Plain closure capture | Capture evaluates once left-to-right and shallow-copies into a new environment | One environment allocation plus immediate capture assignments | Copying the resulting closure pointer allocates nothing and preserves environment identity |
| Collection iteration | Iterable evaluates once into shallow hidden state and each yielded value is shallow | Immediate iterable and yielded representations only | A hidden `Vec` descriptor fixes its own length and keeps the backing pointer captured at loop entry |
| Thread transfer | 0.9 gives the destination an independent transfer-safe value | Uses recursive helpers; mutable `String` bytes and collection backing are duplicated eagerly | 0.10 removes this boundary and shallow-copies all thread-visible representations |
| Channel send | Argument is evaluated once and transfer-copied into a queue/rendezvous message | Message-node allocation plus `C(T)` while synchronized; receive returns the stored independent value | Queue nodes and backing stay reachable through the channel until consumed/closed/collected |
| Thread join | Every join returns an independent copy of one cached result | Native join occurs once; each call applies `C(R)` | Thread state remains reachable through handles and the runtime registry until joined/unregistered |
| `Mutex.new/read/replace/update` | Mutex identity is shared; values crossing its boundary are independent | State allocation at `new`; recursive copies on stored/read/replacement values while locked, including proportional `String` byte copies | Slow transfer-copy callbacks extend lock hold time until the mutex migration removes this boundary |
| Atomic operation | Atomic handle identity is shared | A native mutex protects the scalar cell in the C99 backend; operations allocate nothing after construction | May use target-provided atomic hooks later while retaining sequential consistency |
| `String`/`str` concatenation | Produces new text | Allocates result length and copies both byte ranges | Temporary/dead-input reuse or ropes are permitted if text behavior is unchanged |
| `Vec ++ Vec` | Produces a distinct concatenated vector whose element values are shallow | Inline result descriptor plus one exact backing allocation and immediate element assignments from both inputs | Fresh-input reuse is permitted when alias behavior is preserved |
| Vector growth | Existing value remains the same logical vector with added capacity | Geometric capacity growth, new backing allocation, and shallow relocation of existing element representations; argument copying occurred earlier | Abandoned backing is GC-reclaimable, not immediately freed |
| Map/set growth | Existing collection retains entries | Geometric parallel-array growth and shallow relocation, after linear lookup | Old arrays remain until collection; representation may be replaced wholesale |
| `clear` | Collection becomes empty | Sets length to zero; does not shrink or release backing | Capacity and references in abandoned slots may remain conservatively retained until later overwrite/collection |
| Formatting and f-strings | Produces formatted text | Geometrically grown formatter buffer plus byte appends; displaying nested values walks them; impossible-size growth traps as OOM before arithmetic overflow | Buffer reuse and size precomputation are permitted |
| Safe reference formation | Reference preserves place identity and lifetime | Address-taken local is conservatively promoted to one managed cell for the function invocation | Current promotion answers only “address taken”; precise escape analysis may keep nonescaping cells on stack |
| `defer` | Executes registered code at lexical exits | Registrations are static control-flow edges, not closure allocations; deferred calls have their ordinary argument/copy costs when executed | Compiler may simplify edges while preserving reverse registration order |

Collection mutators receive already evaluated shallow arguments. Copying a key
or value therefore costs only its immediate representation; current linear
map/set search and any backing growth dominate large operations instead.

### Read-only call borrowing

The compiler still specializes concrete internal direct-call instances when a
parameter has a large aggregate or legacy runtime-managed strategy and the
typed body proves that the parameter's source storage is never mutated or
address-exposed.
The generated C function receives a hidden `const T *` for that parameter, and
the call site passes the address of its already evaluated temporary. Calls
remain synchronous, so that storage remains live for the entire invocation.
`String` and other pointer-sized identity carriers retain their ordinary owned
parameter ABI because their shallow copy is already constant-size.

Returning, storing, capturing, or forwarding a parameter still records the
ordinary logical-copy operation at that use. The optimization removes only
redundant entry movement; it does not
change the source parameter type, create an Elamite reference, or make mutable
storage observable through an alias. Recursive and separately monomorphized
generic direct calls can use the specialized convention.

The compiler conservatively retains owned value parameters for indirect
function calls, closure and trait-object dispatch, vtable entries, foreign
imports and exports, and any function whose address is used as a value. It also
falls back for trivial or identity-preserving types, promoted/address-taken
parameters, mutating receiver operations, and any analysis uncertainty. This
keeps every source-visible and foreign ABI stable.

### Temporary and return storage reuse

Every semantic copy remains represented in typed and control-flow IR. When an
ordinary copy's source lifetime is a fresh temporary, the compiler may still
change its physical mode from `Materialize` to `ReuseSource`; both modes now
forward the same shallow representation, but retaining the distinction keeps
the established optimizer and measurement seam available for later inline
aggregate work.

A direct return of a local may also reuse its representation because the local
is dead on that exit edge, but only when it is not a parameter, is not
promoted/address-taken, and the function contains no `defer` registration.
These conservative exclusions remain useful for future storage selection even
though ordinary descriptor backing is intentionally shared.

Transfer copies always materialize. Ordinary copies from borrowed parameters,
pattern payloads, collection interiors, repeated local aggregate inputs, and
other lexical or uncertain storage remain explicit IR records but lower to
shallow C assignments rather than recursive helpers. `ReuseSource` is not a
source-level move: Elamite code can continue using every source binding.

### Shallow mutable `String` backing

Every nonempty `String` descriptor points directly into a pointer-free managed
allocation containing writable UTF-8 bytes and a trailing NUL used only by
runtime helpers. An ordinary copy assigns the two-word descriptor, so mutable
byte access through either descriptor observes the same backing without a
flag, reference count, allocation, or detach check. The default empty value
uses a null descriptor; its first mutable-byte access installs a terminator-only
backing allocation in that descriptor place.

The current source library exposes whole-value replacement but no in-place
content-mutating `String` operation. Future mutators must use the runtime's
mutable-byte hook, which now returns existing non-null backing directly.

The temporary 0.9 transfer boundary cannot publish writable ordinary backing
under its independence contract, so its helper eagerly allocates and copies
`b + 1` bytes. This is the only `String` copy that remains proportional to
content and disappears with the later C-like thread/channel publication
package. Boehm GC remains solely responsible for reclamation.
`String` remains outside the C ABI-safe type set, so foreign code receives text
only through explicit raw-pointer/length wrappers and their documented rooting
requirements.

## Compiler-side logical-copy inventory

The checker records why each source expression needs a logical copy and the
coarse lifetime boundary it crosses. Typed IR combines that context with the
concrete type's allocation class: no allocation, preserved identity, shallow
inline copying, recursive transfer copying, shared backing, or runtime-managed
copying. It also distinguishes
ordinary value copies from transfer copies without yet selecting different
runtime helpers for them.

Before control-flow lowering, read-only call analysis changes eligible argument
copies into an explicit borrowed passing mode, while reuse analysis marks
eligible remaining semantic copies as `ReuseSource`. Control-flow lowering
preserves those facts on every explicit `Copy` rvalue and assigns a stable,
per-function copy ID. Debug compiler builds verify that the emitted IDs are
unique and form a complete sequence, and the public IR exposes the same
inventory for optimizer tests and measurements. This inventory counts semantic
copy operations that remain after proven call borrowing whether materialized
or reused, not recursive field copies, allocator requests, ABI traffic, or
physical bytes. It adds no generated-C counters, output, or runtime behavior in
release programs; the separate opt-in `elamite-cost-v1` instrumentation below
remains the source of physical allocation and byte-copy measurements.

Copies performed inside today’s shared synchronization helpers remain physical
implementation work rather than additional IR copy records. Their call-site
arguments still carry the legacy transfer purpose and invoke the existing
recursive helper family; ordinary `Copy` rvalues bypass that family. The
concurrency migration removes these remaining transfer helpers.

## Allocation, garbage collection, and retained memory

Elamite uses a non-moving Boehm collector whenever lowered code requires
managed storage. Programs that need no managed storage do not link it. The
collector traces stacks conservatively, permits interior safe references, and
reclaims unreachable cycles, but collection timing is unspecified.

Important consequences of the current implementation are:

- allocation can occur implicitly during closure construction, variadic calls,
  collection construction/growth, formatting, transfer, synchronization, and
  safe-reference promotion; an ordinary shallow copy alone does not allocate;
- allocation failure performs one full collection and retries before the
  process-fatal OOM path;
- `String`, raw `str` concatenation, and formatter byte buffers use the
  pointer-free allocator because their backing contains no managed pointers;
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
on x86 and 16 bytes on x86-64 before C ABI alignment, while vector descriptors
and set headers contain three target words and map headers contain four. Closure environments,
aggregate padding, native mutexes, thread state, and collector metadata also
follow the selected C ABI. A nonempty `String` backing contains only its bytes
and trailing runtime NUL.

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
concurrent workloads cannot lose increments. Generated source remains C99 and
does not require C11 `_Atomic`.

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
