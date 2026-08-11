# Elamite implementation cost model

> Version: 17
>
> Applies to: the 0.10.0-draft compatibility path and implemented
> 0.11.0-draft phases through explicit shared and graph ownership, on Linux x86 and
> x86-64
>
> Status: non-normative implementation documentation

> Implementation revision: **The compatibility path remains shallow; the
> owned path moves unique core values, stores closure environments inline, and
> clones only when requested.** The semantic-revision selection is explicit in
> control-flow IR. The C backend never infers a value model from source syntax.

This document explains where the current 0.10 compiler copies values, allocates
storage, retains memory, and synchronizes. `spec.md` now defines the accepted
0.11 target; this cost model remains the measured migration baseline until each
representation milestone updates it. Nothing here creates an allocation,
timing, collector, address, or complexity guarantee.

Ordinary assignment, argument, return, capture, pattern, indexing, propagation,
and aggregate copies now copy only their immediate representation. Inline
aggregate storage is distinct, while nested descriptors and handles retain
identity. Threads, channels, and joins now publish those same immediate values.
Mutex operations use the same rule while holding the mutex lock; the lock
serializes access to its immediate stored representation but does not isolate
or automatically protect backing reached through external aliases.

## Implemented 0.11 owned-core costs

The ordinary driver still stops the 0.11 path before promotion and tracing-GC
removal, but the checked-to-C conformance path now implements the complete
owned-core, inline-closure, and explicit shared/graph layers. On that path:

- `String`, `Vec[T]`, `Map[K, V]`, and `Set[T]` have one owner. Moving them is
  a constant-size descriptor transfer; it allocates and copies no backing.
- `clone()` is explicit. A string clone allocates and copies `b` bytes; vector
  and set clones allocate `O(n)` backing and clone each element; map clones
  allocate parallel `O(n)` backing and clone each key and value.
- destruction visits still-owned elements in reverse order and immediately
  releases collection and string backing. `clear` destroys elements but keeps
  reusable capacity. Growth geometrically allocates replacement backing,
  relocates immediate representations, and frees the old backing.
- `[T]` and `[var T]` are two-word non-owning pointer/length descriptors.
  Forming them from an array, vector, or slice allocates nothing. Shared views
  yield `&T`; exclusive views yield `&var T` and statically block overlapping
  access or relocation.
- `Box[T]` performs one explicit `sizeof(T)` allocation, keeps the pointee at a
  stable address, moves as one pointer, recursively clones only on explicit
  `clone()`, and destroys the pointee before freeing its allocation.
- owned collection and array iteration moves visited elements. Its inline
  index/descriptor state allocates nothing; early exit destroys unvisited
  elements exactly once. Slice iteration borrows and yields references without
  allocation.

## Implemented 0.11 inline-closure costs

- Every closure expression has a distinct inline aggregate containing its
  capture representations. Construction evaluates captures left to right,
  moves each non-`Copy` plain capture, copies each `Copy` capture, and stores
  `&`/`&var` as non-owning pointers. The closure itself performs no allocation.
- Moving a closure transfers its inline environment. The current C99 lowering
  may assign bytes proportional to the immediate environment size; it never
  traverses or clones owned backing. An explicit `clone()` visits every capture
  and incurs exactly the clone costs of those fields. Destruction visits owned
  capture fields in reverse order.
- Direct, generic-bound, and erased calls use one shared receiver contract.
  The current control-flow lowering may materialize one non-owning immediate
  environment assignment at a call site before passing its address; the call
  allocates nothing and cannot move a capture out.
- A closure capture using `&` or `&var` keeps its source in the enclosing stack
  frame and adds no promotion allocation. Borrow provenance prevents the
  closure from escaping that storage. Taking an ordinary standalone reference
  to a closure for erased dispatch can still use the broader conservative
  promotion path scheduled for removal later.
- A capture-free closure explicitly converted to its exact safe function
  reference is one function pointer and carries no environment. Capturing
  closures never convert to code pointers. Borrowed erasure adds only the
  ordinary two-word trait-object view; owning erasure allocates solely through
  its explicit `Box`.
- Owned-path lowering no longer requests or links the tracing collector.
  Compatibility programs retain their existing collector costs until the
  migration seam is removed.

## Implemented 0.11 shared and graph costs

- `Shared[T]` and `Weak[T]` each move as one pointer. Construction performs one
  control-block allocation containing `T`, a native mutex, and pointer-width
  strong/weak counters. Clone, downgrade, upgrade, and destruction take that
  mutex for constant-time counter work; counter overflow is process-fatal.
- The control block retains one implicit weak count until the last strong owner
  has destroyed `T`. Explicit weak owners can retain only the empty control
  block. Strong cycles intentionally retain their complete strongly connected
  allocations until code replaces back edges with `Weak` or uses a store.
- `Store[T]` is one heap descriptor plus geometrically grown slot and live-slot
  arrays. `Handle[T]` is three `uintptr_t` fields (store identity, slot,
  generation), copies without allocation, and retains no store or element.
  Lookup is constant time and returns a borrow; wrong-store and stale checks
  are constant-time comparisons before access.
- Insertion scans for a reusable free slot and can therefore take `O(capacity)`
  before geometric growth. Removal shifts the dense live-slot index in
  `O(n)` and increments the slot generation; exhausted generations are
  retired. Consuming iteration moves
  occupied values through the dense live-slot index without allocation.
  `compact()` explicitly allocates and copies both arrays while preserving
  logical slots and generations. Exclusive access statically blocks relocating
  operations while an element borrow is live.
- Store destruction visits all occupied slots, destroys each remaining `T`,
  and releases all backing plus the descriptor. Handles neither participate in
  cleanup nor keep elements alive. All sizes and counters use target-width
  types on both supported architectures; none of these compiler-private
  layouts is a C interoperability guarantee.

These costs coexist temporarily with ordinary address-taken promotion,
compatibility concurrency, and variadic-pack machinery owned by later
migration milestones. They do not claim that the 0.11 path is yet available
through the compiling driver.

## Reading the tables

- **Ordinary copy** means the implemented shallow 0.10 value operation.
- **Physical work** describes this compiler revision, not a guarantee.
- **Allocation** counts requested Elamite runtime allocations, before
  collector metadata and rounding.
- **Retention** explains why storage can remain live after the source-level
  operation finishes.
- **Implementation freedom** is work a future compiler may safely avoid.

`n` is a collection length, `b` a UTF-8 byte length, and `w` the target pointer
width.

## Costs by type family

The tables in this section remain the measured 0.10 compatibility baseline.
The implemented 0.11 replacements are specified in the owned-core and
inline-closure sections above.

| Type family | Required semantics | Current physical representation and copy | Likely allocation | Retention and implementation freedom |
| --- | --- | --- | --- | --- |
| Unit, booleans, characters, integers, floats | Independent scalar value | Inline C scalar; constant-size assignment | None | May live only in registers or be eliminated |
| `str` | Immutable UTF-8 view | Two-word byte pointer/length descriptor; copying preserves immutable backing identity | None for the copy | Literal or existing backing determines lifetime; descriptor copies may be eliminated |
| `String` | Shallow mutable backing identity | Two-word byte-pointer/length descriptor; every value copy aliases writable backing directly | Construction allocates `b + 1` pointer-free bytes; copying allocates nothing | Ordinary, published, and mutex copies all preserve the same backing identity |
| Tuples, fixed arrays, structs | Immediate inline slots copy; nested backing identities remain shared | One C aggregate assignment, proportional only to inline representation size | None merely for copying | C may lower a large inline assignment to moves or `memcpy`; reachable managed contents are not traversed |
| Enums and `Option`/`Result` | Discriminant and active inline payload copy shallowly | One explicit-tag C99 aggregate assignment | None merely for copying | Inactive payload storage affects layout but no reachable backing is traversed |
| `Vec[T]` | Ordinary copies share backing while retaining descriptor-local length and capacity | Inline pointer/length/capacity descriptor; ordinary copy assigns three target words | None for a copy | Element writes alias within both ranges; growth updates one descriptor and may diverge from its copies |
| `Map[K, V]` | Ordinary copies preserve complete mutable table identity | One managed-header pointer; lookup remains linear | None for a copy | Structural mutation is visible through every ordinary copy; hashing remains an implementation choice |
| `Set[T]` | Ordinary copies preserve complete mutable table identity | One managed-header pointer; membership remains linear | None for a copy | Structural mutation is visible through every ordinary copy; hashing remains an implementation choice |
| Safe references and raw pointers | Explicit alias identity | One pointer; copy preserves the same address; raw arithmetic uses element-scaled C pointer operations and ordering uses constant-size null guards plus a non-null byte-pointer comparison | None | Safe references may cause pointee promotion separately; raw pointers never root storage, and arithmetic or ordering does not extend provenance or lifetime |
| Function references/pointers | Callable identity | One C function pointer | None | May be propagated in registers |
| Closures | Construction shallow-copies captures once; callable copies preserve environment identity | One managed environment pointer; construction allocates one environment, ordinary copying copies only the pointer | One allocation at construction; none for a copy | Spawn copies the environment pointer directly; captureless environments may be optimized away |
| `&Trait` | Explicit fat reference alias | Data pointer plus vtable pointer; copying preserves identity | None for coercion/copy once the referent exists | Coercing an address-taken local can trigger promotion |
| `Identity[T]`, `ForeignRoot`, thread/channel/mutex/atomic handles | Shared identity | One managed/raw handle pointer; copying is constant-size and preserves synchronized or registered state | Constructors allocate state; handle copies do not | Ordinary shallow copying treats these like every other identity-bearing descriptor |
| Slices, including variadic parameter packs | Immutable view | Pointer plus length; a variadic call currently materializes managed backing for its trailing arguments | One backing allocation for a nonempty variadic pack | A proven nonescaping pack may eventually use caller storage |
| `Duration`, `Instant`, `SystemTime`, `Generator` | Independent numeric state; clock domains remain nominally distinct | One inline `u64`; copying and clock reads are constant-size | None | Clock reads do not create synchronization edges; generator state advances only through its mutable receiver |
| Filesystem paths and metadata | Ordinary shallow structs | A path contains one shared-backing `String` descriptor; metadata and status records are inline aggregates | Path construction allocates owned text; source-hosted lexical transformations may allocate a split vector and one or more concatenation results; metadata copying allocates nothing | Path copies share their string backing; operations never expose a reference into managed storage |
| File and directory handles | Shared native-resource identity with idempotent cleanup | One pointer to managed handle state containing the native handle and closed flag | One state allocation on successful open | Copies preserve one close state; unreachable open handles can retain native resources, so deterministic code uses `defer` |

`Map` and `Set` operations are currently `O(n)` lookup operations. Their names
do not promise a particular hashing representation. Vector indexed access and
length are constant-time in the current implementation; inserting or removing
away from the tail shifts the remaining inline element representations.

## Costs by source operation

This table likewise records the compiling 0.10 compatibility path; owned-path
operations use the replacement costs above.

| Operation | Semantic behavior | Current physical work and allocation | Retention / future freedom |
| --- | --- | --- | --- |
| Binding and assignment | Destination receives an ordinary shallow value | One immediate scalar, pointer, descriptor, or C aggregate assignment | No allocation merely for the copy; broader last-use analysis may still remove inline movement |
| Value argument | Callee receives a shallow value and may observe shared backing through descriptors | Owned ABI passes the immediate representation; eligible internal direct calls may still use a hidden read-only pointer to avoid large inline C movement | Uncertain and ABI-visible calls retain the owned ABI but no longer recursively materialize backing |
| Return value | Caller receives an ordinary shallow value | One immediate return representation; the existing reuse pass still records proven source handoffs | C may add ABI-level aggregate movement; no managed backing is traversed |
| Pattern binding | Bound payload is a shallow value; `_` binds nothing | Active payload and named inline representations assign directly | Tests and discriminants allocate nothing; nested descriptors preserve identity |
| Plain closure capture | Capture evaluates once left-to-right and shallow-copies into a new environment | One environment allocation plus immediate capture assignments | Copying the resulting closure pointer allocates nothing and preserves environment identity |
| Direct collection iteration | Iterable evaluates once into shallow hidden state and each yielded value is shallow | One immediate iterable copy and one length snapshot before the loop, then one shallow yielded assignment per visited item; no managed allocation merely for the loop | A hidden `Vec` descriptor fixes its own length and backing pointer; map/set structural mutation and vector length mutation during the active loop are UB rather than checked operations |
| User-defined `Iterator` iteration | Iterator evaluates once and shallow-copies into mutable hidden state; each `Some` payload shallow-copies into the binding | One managed cell for the hidden state, one direct `next` call per attempted step, one `Option` result representation, and one shallow payload assignment per visited item | Managed state permits a yielded safe reference to outlive the loop; a fully exhausted `n`-item iterator makes `n + 1` calls, while `break` makes no final call; proven nonescaping state may eventually remain on the stack |
| Thread spawn | Callable evaluates once and its environment shallow-copies into startup state | One immediate callable assignment plus thread/startup-state allocation; no capture backing is traversed | Startup state and captured roots remain live through registered thread state until completion |
| Channel send | Argument evaluates once and shallow-copies into a queue/rendezvous message | One immediate assignment plus one message-node allocation while synchronized | Queue nodes and shared backing stay reachable through the channel until consumed, closed, or collected |
| Thread join | Every join shallow-copies one cached result | Native join occurs once; each call returns the immediate cached `R` representation | Repeated results may share mutable backing; thread state remains rooted through handles/registry until unregistered |
| `Mutex.new/read/replace/update` | Stored and returned values are shallow; the handle shares synchronized identity | One state allocation at `new`; immediate representation assignments while locked; no managed allocation merely for a stored or returned copy | Locking orders callers using the handle, but external aliases to nested backing remain aliases and require the programmer's synchronization protocol |
| Atomic operation | Atomic handle identity is shared | A native mutex protects the scalar cell in the C99 backend; operations allocate nothing after construction | May use target-provided atomic hooks later while retaining sequential consistency |
| `String`/`str` concatenation | Produces new text | Allocates result length and copies both byte ranges | Temporary/dead-input reuse or ropes are permitted if text behavior is unchanged |
| `Vec ++ Vec` | Produces a distinct concatenated vector whose element values are shallow | Inline result descriptor plus one exact backing allocation and immediate element assignments from both inputs | Fresh-input reuse is permitted when alias behavior is preserved |
| Vector growth | Existing value remains the same logical vector with added capacity | Geometric capacity growth, new backing allocation, and shallow relocation of existing element representations; argument copying occurred earlier | Abandoned backing is GC-reclaimable, not immediately freed |
| Map/set growth | Existing collection retains entries | Geometric parallel-array growth and shallow relocation, after linear lookup | Old arrays remain until collection; representation may be replaced wholesale |
| `clear` | Collection becomes empty | Sets length to zero; does not shrink or release backing | Capacity and references in abandoned slots may remain conservatively retained until later overwrite/collection |
| Formatting and f-strings | Produces formatted text | Geometrically grown formatter buffer plus byte appends; displaying nested values walks them; impossible-size growth traps as OOM before arithmetic overflow | Buffer reuse and size precomputation are permitted |
| Safe reference formation | Reference preserves place identity and lifetime | Address-taken local is conservatively promoted to one managed cell for the function invocation | Current promotion answers only “address taken”; precise escape analysis may keep nonescaping cells on stack |
| Unsafe raw-pointer arithmetic, subtraction, indexing, and relational ordering | Offsets are element-scaled; indexing performs no bounds check; null orders below every non-null pointer; and non-null operations remain subject to provenance/liveness/extent obligations | Constant-size pointer/integer operations; each executed index reuses the raw null/alignment check before access; ordering uses equality guards before its C relational comparison; no managed allocation | The compiler may fold redundant arithmetic, checks, or comparison branches when validity is proven, but raw pointers remain non-rooting and undefined behavior is not made observable |
| `defer` | Executes registered code at lexical exits | Registrations are static control-flow edges, not closure allocations; deferred calls have their ordinary argument/copy costs when executed | Compiler may simplify edges while preserving reverse registration order |
| Stable ordering sort | Mutates vector elements in ascending order while retaining equal-input order | Stable insertion sort over shared vector backing; `O(n²)` comparisons/moves, one shallow element temporary, and no allocation | The deliberately small baseline may be replaced by another stable in-place algorithm |
| Binary search | Returns the first equal index in sorted input | `O(log n)` comparisons over a slice or copied vector descriptor, with no allocation | No backing is retained beyond the ordinary argument lifetime |
| Seeded randomness | Advances an explicit SplitMix64 state with a versioned output sequence | Constant-size integer arithmetic with no allocation; rejection sampling may draw repeatedly | Fixed seeds are reproducible across supported targets; no operation reads ambient entropy |
| Clock read and duration arithmetic | Reads one clock domain or performs checked nanosecond arithmetic | One native clock read or constant-size checked integer operation, with no allocation | Clock reads are observations, not synchronization edges |
| Borrowed text search, split, and trim | Scalar-indexed exact matching; borrowed results retain original backing | Source-hosted search/trim scan UTF-8 through constant-size scalar steps without allocation; split geometrically grows an `O(k)` result vector for `k` borrowed descriptors but copies no substring bytes | Descriptor copies and repeated candidate comparisons may be optimized; Unicode scalar and White_Space behavior is preserved |
| Owned text split, trim, and case mapping | Materializes owned results without mutating input backing | Owned split adds one exact string allocation per result to the borrowed split vector; trim allocates one exact string; case mapping geometrically grows a temporary `Vec[char]` and then performs one exact UTF-8 string allocation/encoding pass, including scalar expansion | A private builder may remove the temporary scalar vector later while preserving shallow-backing behavior |
| Filesystem/environment/process operation | Returns owned snapshots or explicit native handles and portable failures | Native call plus result construction; storage is proportional to returned path/text/byte data, successful handles allocate state, file reads grow geometrically until EOF, and process capture uses two native temporary streams before materializing its final vectors | No returned value borrows C library storage; process execution does not invoke a shell, and invalid host argument bytes expand to one U+FFFD scalar each |

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

Copies from borrowed parameters, pattern payloads, collection interiors,
repeated local aggregate inputs, and other lexical or uncertain storage remain
explicit IR records but lower to shallow C assignments. `ReuseSource` is not a
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

Thread and channel publication copies this descriptor directly, so no byte
allocation or content copy occurs at spawn, send, receive, or join. Mutex
operations copy it directly as well, so `new`, `read`, `replace`, and
`update` add no proportional byte-copy path. Boehm GC remains solely
responsible for reclamation.
`String` remains outside the C ABI-safe type set, so foreign code receives text
only through explicit raw-pointer/length wrappers and their documented rooting
requirements.

## Compiler-side logical-copy inventory

The checker records why each source expression needs a logical copy and the
coarse lifetime boundary it crosses. Typed IR combines that context with the
concrete type's allocation class: no allocation, preserved identity, shallow
inline copying, shared backing, or runtime-managed copying. The lifetime class
still distinguishes lexical, caller, returned, thread, and synchronized-storage
boundaries without assigning them different value semantics.

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

Mutex call arguments remain explicit logical-copy records at synchronized-
storage lifetime boundaries. Runtime `new`, `read`, `replace`, and `update`
lower the concrete value representation directly while holding the lock; no
recursive copy-helper family is emitted.

## Allocation, garbage collection, and retained memory

Elamite uses a non-moving Boehm collector whenever lowered code requires
managed storage. Programs that need no managed storage do not link it. The
collector traces stacks conservatively, permits interior safe references, and
reclaims unreachable cycles, but collection timing is unspecified.

Important consequences of the current implementation are:

- allocation can occur implicitly during closure construction, variadic calls,
  user-defined iteration state promotion, collection construction/growth,
  formatting, publication, synchronization, and safe-reference promotion; an
  ordinary shallow copy alone does not allocate;
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
held, but this is only an immediate shallow representation assignment and does
not recursively traverse backing. The three sequentially consistent atomic
cell types currently use a native mutex per cell rather than C11 `_Atomic`,
preserving the C99 target.

These synchronization operations establish the normative ordering described
by `spec.md`: `pthread_create` and `pthread_join` own thread start/completion,
channel and mutex edges use their queue or value mutex, and every atomic cell
operation is a synchronous mutex-protected linearization point. Correctly
synchronized publication of ordinary shared backing is TSan-clean; unordered
conflicting access remains undefined behavior and is never benchmarked or run
as a conformance fixture. Wall time and fairness are intentionally unspecified.
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

The final 0.10 observation was recorded on 2026-08-03 with identical workload
hashes to the preceding shallow baseline. All deterministic counters were
unchanged: the six workloads respectively request 6, 1,013, 2, 17, 2, and 6
allocations and explicitly copy 25, 32, 17, 720, 256, and 496 bytes. Compile
time, runtime time, and peak RSS remain observations rather than thresholds.
The local host could build but not execute the x86 instrumented artifact, so
the checked-in row set remains an x86-64 observation; widths must never be
compared as though their requested sizes were interchangeable.

The version 13 standard-library expansion was measured again on 2026-08-04
against that checked observation, using the same x86-64 host/toolchain class
and identical workload hashes. The fixed workloads do not invoke the new APIs,
so all requested-allocation and explicit-copy counters remained identical.
Compile time and peak RSS varied and retain their ordinary non-deterministic
status. New filesystem, process, time, ordering, text, and random costs are
documented in the tables above rather than inferred from unrelated workloads.

Version 14 was measured on 2026-08-05 UTC after source-hosting the standard
text and lexical path algorithms. The same x86-64 compiler/toolchain class and
all six unchanged workload hashes produced the same deterministic counters:
the workloads requested 6, 1,013, 2, 17, 2, and 6 allocations and explicitly
copied 25, 32, 17, 720, 256, and 496 bytes. Those fixed workloads do not call
the migrated APIs, so their unchanged counters establish that demand-driven
standard-library reachability adds no unused runtime cost. The operation tables
above record the material split, case-mapping, and path-allocation changes;
compile time, runtime time, and peak RSS remain non-semantic observations.

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
strategy, concurrency publication, synchronized storage, or instrumentation
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
