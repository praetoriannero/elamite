# Generational references with multi-owner reclamation

A standalone C99 implementation of [generational
references](https://verdagon.dev/blog/generational-references), extended with
owner counting so that objects may have several owners, as graph structures
need.

This is an experiment. It is not wired into the compiler, is not referenced by
`spec.md`, and implements no Elamite semantics.

## The design question this answers

A generational reference is a `(pointer, remembered generation)` pair. Every
allocation holds a generation counter in its header; freeing an object
increments it, so every reference minted before that free now disagrees with
the header and is detectably stale. Dereferencing checks that the two agree.
Use-after-free becomes a deterministic abort instead of undefined behavior,
even when the allocator has already reused the storage.

What the scheme does *not* provide is any notion of lifetime. The header can
tell a reference whether its target died; nothing tells an object whether
anyone still points at it. Vale supplies that missing half with single
ownership: one owner, freed at scope exit, with generational references keeping
the remaining non-owning aliases sound.

Single ownership does not describe a graph. This implementation therefore
splits references in two:

| Kind | Keeps target alive | Checked on use | Purpose |
| --- | --- | --- | --- |
| `gr_owned` | yes, counted | yes | shared ownership; decides when to free |
| `gr_ref` | no | yes | observers, back-edges, caches, parent pointers |

Owner counting supplies liveness — nothing is freed while an owner exists.
Generational references supply safety — every reference that outlives the
object fails a check rather than dangling. Cycles among owners are reclaimed by
an opt-in trial-deletion collector.

## Building

```sh
make test      # build and run the test suite
make sanitize  # same suite under ASan + UBSan
make example   # build and run the graph demo
```

Warnings are errors-adjacent by default: `-Wall -Wextra -Wpedantic -Wshadow
-Wstrict-prototypes -Wmissing-prototypes -Wconversion`.

## Using it

Describe a type once. `trace` is required for any type that owns other objects;
`acyclic` opts a type out of cycle collection entirely.

```c
typedef struct {
    int id;
    gr_owned left;
    gr_owned right;
    gr_ref  parent;   /* non-owning back-edge */
} Node;

static void node_trace(void *object, gr_trace_ctx *ctx) {
    Node *node = (Node *)object;
    gr_trace_edge(ctx, node->left);
    gr_trace_edge(ctx, node->right);
    /* `parent` is not traced: it owns nothing */
}

static const gr_type NODE_TYPE = {
    .name = "Node", .size = sizeof(Node),
    .destroy = NULL, .trace = node_trace, .acyclic = false,
};
```

Then:

```c
gr_owned a = gr_new(&NODE_TYPE);     /* one owner */
gr_owned b = gr_clone(a);            /* two owners */
gr_ref   observer = gr_weaken(a);    /* zero effect on lifetime */

((Node *)gr_get(a))->id = 7;

gr_drop(&a);                         /* still alive: b owns it */
gr_drop(&b);                         /* freed, destructor runs */

gr_alive(observer);                  /* false */
gr_try(observer);                    /* NULL */
GR_DEREF(observer);                  /* aborts with file:line */
```

## Semantics

**Generations.** Start at 1 so a zeroed `gr_ref` never matches a live object.
Incremented on every free, never on allocation. A block reused for a later
allocation continues counting, so a reference held across the reuse still
fails.

**The pool is required, not an optimization.** Blocks come from size-classed
free lists backed by chunks that are never returned to the host allocator while
the program runs. The generation check reads the target's header, so that
header must stay mapped and must keep counting up across reuse. Handing memory
back to `malloc` would break soundness, not just performance. `gr_shutdown`
releases the chunks and is only safe at process exit.

**Release cascades.** When the last owner goes, the destructor runs, then every
owned edge is released, iteratively — a 50,000-link chain is in the test suite
and does not touch the C stack.

**Destructors** run while the object and everything it owns are still valid
memory. They must not free owned children themselves and must not resurrect the
object.

**Cycle collection** is Bacon–Rajan trial deletion. Objects whose owner count
drops without reaching zero are registered as candidates, which is the
signature of something possibly held only from inside a cycle. `gr_collect_cycles`
walks the candidate subgraph subtracting internal edges; anything left with a
positive count is externally held and gets restored, and the remainder is
freed. Owned edges pointing *out* of the garbage group are released normally,
so an object shared between a dead cycle and live code survives with a
correctly decremented count.

Types marked `acyclic` never become candidates, so they cost the collector
nothing. Preferring a non-owning `gr_ref` for back-edges avoids creating cycles
in the first place, which is the cheaper habit.

The collector's own candidate buffer is filtered by generation — the same
mechanism the language-level references use, applied to its bookkeeping.

## Limitations

- **Single-threaded.** Counters, the candidate buffer, and the free lists are
  all non-atomic.
- **Manual ownership.** C has no destructors or move semantics, so `gr_clone`
  and `gr_drop` are called by hand. In a language with lifetime hooks the
  compiler would emit them, and most would be optimized away by last-use
  analysis.
- **Collection is explicit.** `gr_collect_cycles` runs only when called. There
  is no allocation-threshold trigger.
- **Size classes round to powers of two**, so a payload just over a boundary
  wastes up to half its block.
- **No compaction.** Freed blocks return to their own size class only.
- **Not a check-elision study.** Vale's performance case rests on removing
  checks via single ownership, linear style, and regions. Every dereference
  here is checked, so this measures the mechanism's cost with none of the
  optimization that makes it competitive.

## Relationship to Elamite

None yet, deliberately. Two things would have to be settled before any of this
could inform the compiler.

`spec.md` §9 currently specifies no destruction protocol, no finalizers, and
that collection never runs cleanup. Owner counting is a destruction protocol,
so adopting it is a specification change rather than a strategy swap behind
`ManagedMemoryStrategy` — that trait emits C only at allocation sites, and
counting needs code at every ownership transition.

The shallow-copy contract is the sharper issue. Copies of a `Vec` descriptor
share element backing and `Map`/`Set` copies share the whole table, which works
today because the collector keeps backing alive regardless of which descriptor
is reachable. Under owner counting, that backing needs an owner, and "every
descriptor aliases it, none owns it" has no translation.
