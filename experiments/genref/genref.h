/* Generational references with multi-owner reclamation.
 *
 * A generational reference is a (pointer, remembered generation) pair. Every
 * allocation carries a generation counter in its header; freeing an object
 * increments it, so every reference minted before the free now disagrees with
 * the header and is detectably stale. Dereferencing checks the two agree.
 *
 * Generational references decide *validity*, never *lifetime*: the header can
 * tell a reference whether its target died, but nothing in the scheme tells an
 * object whether anyone still points at it. This library therefore separates
 * two reference kinds:
 *
 *   gr_owned   Owning. Counted. The object lives while at least one exists.
 *   gr_ref     Non-owning. A pure generational reference. Never keeps the
 *              target alive; goes stale when the last owner releases.
 *
 * Multiple owners are the expected case. Ownership cycles are reclaimed by an
 * opt-in trial-deletion collector (see gr_collect_cycles).
 *
 * Not thread-safe. Counters and collector state are non-atomic.
 */

#ifndef GENREF_H
#define GENREF_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/* ---------------------------------------------------------------- references */

/* Non-owning generational reference. Copyable by value. A zeroed gr_ref is the
 * null reference and always fails a liveness check. */
typedef struct {
    void *target;
    uint64_t generation;
} gr_ref;

/* Owning generational reference. Holds one unit of the target's owner count.
 * Copy only with gr_clone; discard only with gr_drop. */
typedef struct {
    gr_ref inner;
} gr_owned;

#define GR_NULL_REF ((gr_ref){NULL, 0})
#define GR_NULL_OWNED ((gr_owned){{NULL, 0}})

/* -------------------------------------------------------------------- types */

typedef struct gr_trace_ctx gr_trace_ctx;

/* Releases resources held by the object. Called once, immediately before the
 * block is freed, while the object and everything it owns are still valid
 * memory. A destructor must not resurrect the object or free owned children
 * itself; owned children are released by the runtime after it returns. */
typedef void (*gr_destroy_fn)(void *object);

/* Enumerates every owning reference the object holds by calling gr_trace_edge
 * once per gr_owned field. Required for any type that owns other objects:
 * release cascades and cycle collection both drive off it. */
typedef void (*gr_trace_fn)(void *object, gr_trace_ctx *ctx);

typedef struct {
    const char *name; /* for diagnostics and leak reports */
    size_t size;      /* payload bytes */
    gr_destroy_fn destroy;
    gr_trace_fn trace;
    /* Set when values of this type can never participate in an ownership
     * cycle. Such objects are never registered as cycle candidates, which
     * removes all collector overhead for them. Leaves release cascades
     * unaffected: a type that owns children still needs `trace`. */
    bool acyclic;
} gr_type;

/* Called from a gr_trace_fn, once per owning field. Null and stale edges are
 * ignored. */
void gr_trace_edge(gr_trace_ctx *ctx, gr_owned child);

/* --------------------------------------------------------------- allocation */

/* Allocates one zeroed object with an owner count of 1. Never returns a null
 * reference: allocation failure aborts. */
gr_owned gr_new(const gr_type *type);

/* Adds an owner. The target must be alive. */
gr_owned gr_clone(gr_owned owner);

/* Removes an owner and nulls the handle. Frees the object when the count
 * reaches zero, running its destructor and cascading to what it owned. Safe on
 * an already-null handle. */
void gr_drop(gr_owned *owner);

/* --------------------------------------------------------------- references */

/* Non-owning view of an owned reference. */
gr_ref gr_weaken(gr_owned owner);

/* True when the target is still alive and the remembered generation matches. */
bool gr_alive(gr_ref ref);
bool gr_owned_alive(gr_owned owner);

/* Checked dereference. Returns NULL when the reference is null or stale. */
void *gr_try(gr_ref ref);

/* Checked dereference that aborts on a stale reference rather than returning.
 * Use GR_DEREF so the diagnostic carries a source location. */
void *gr_deref_at(gr_ref ref, const char *file, int line);
#define GR_DEREF(ref) gr_deref_at((ref), __FILE__, __LINE__)

/* Dereference through an owning reference. Still checked: holding an owner
 * makes this always succeed, so a failure means the handle was corrupted or
 * released. */
void *gr_get(gr_owned owner);

/* Current owner count, or 0 for a null or stale reference. */
uint32_t gr_owner_count(gr_ref ref);

/* Generation currently stored in the target's header. Diagnostics only. */
uint64_t gr_current_generation(gr_ref ref);

/* ---------------------------------------------------------- cycle collection */

/* Runs trial deletion over the registered cycle candidates and frees every
 * group of objects kept alive only by references among themselves. Returns the
 * number of objects reclaimed.
 *
 * Candidates accumulate when an owner is released and the count stays above
 * zero, which is the signature of a possible cycle. Types marked acyclic never
 * become candidates.
 *
 * Destructors of cyclic garbage run before any of the group is freed, so they
 * observe an intact object graph. */
size_t gr_collect_cycles(void);

/* Length of the candidate buffer. An upper bound rather than an exact count:
 * entries naming objects that counting has since reclaimed stay in the buffer
 * until the next collection discards them by generation. */
size_t gr_pending_candidates(void);

/* ------------------------------------------------------------- introspection */

typedef struct {
    size_t live_objects;
    size_t total_allocations;
    size_t total_frees;
    size_t failed_checks;    /* stale dereferences caught */
    size_t cycle_collections;
    size_t cycle_objects_freed;
    size_t bytes_reserved;   /* pool memory obtained from the host allocator */
    size_t blocks_reused;    /* allocations served from a free list */
} gr_stats;

gr_stats gr_get_stats(void);

/* Writes one line per live object to stderr. Intended for shutdown leak
 * reporting: anything still live is either genuinely reachable or an
 * unreclaimed ownership cycle. */
void gr_report_live(void);

/* Releases every pool chunk back to the host allocator. All references become
 * dangling, so call this only at process shutdown after the last use. */
void gr_shutdown(void);

#endif /* GENREF_H */
