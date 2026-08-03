#include "genref.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ------------------------------------------------------------------ layout */

#define GR_ALIGN 16u
#define GR_MIN_CLASS 6u  /* 64-byte smallest block */
#define GR_MAX_CLASS 63u
#define GR_CHUNK_BYTES (64u * 1024u)

enum gr_color {
    GR_BLACK = 0, /* in use */
    GR_GRAY = 1,  /* under trial deletion */
    GR_WHITE = 2, /* provisional cyclic garbage */
    GR_PURPLE = 3 /* registered cycle candidate */
};

typedef struct gr_header gr_header;

struct gr_header {
    /* Bumped on every free. A reference whose remembered value differs from
     * this one is looking at storage that has since been reclaimed. */
    uint64_t generation;
    const gr_type *type;
    uint32_t owners;
    uint8_t color;
    bool live;
    bool buffered;
    unsigned char class_index;
    int64_t trial;         /* scratch owner count during trial deletion */
    gr_header *next_free;  /* free-list link while dead */
};

#define GR_HEADER_BYTES \
    ((sizeof(gr_header) + GR_ALIGN - 1u) & ~(size_t)(GR_ALIGN - 1u))

static void *payload_of(gr_header *header) {
    return (void *)((char *)header + GR_HEADER_BYTES);
}

static gr_header *header_of(void *payload) {
    return (gr_header *)((char *)payload - GR_HEADER_BYTES);
}

/* ------------------------------------------------------------------ vectors */

typedef struct {
    gr_header **data;
    size_t len;
    size_t cap;
} gr_vec;

static void vec_push(gr_vec *vec, gr_header *value) {
    if (vec->len == vec->cap) {
        size_t next = vec->cap ? vec->cap * 2u : 16u;
        gr_header **grown = realloc(vec->data, next * sizeof(*grown));
        if (grown == NULL) {
            fputs("genref: out of memory growing a work list\n", stderr);
            abort();
        }
        vec->data = grown;
        vec->cap = next;
    }
    vec->data[vec->len++] = value;
}

static gr_header *vec_pop(gr_vec *vec) { return vec->data[--vec->len]; }

static void vec_clear(gr_vec *vec) { vec->len = 0; }

static void vec_free(gr_vec *vec) {
    free(vec->data);
    vec->data = NULL;
    vec->len = 0;
    vec->cap = 0;
}

/* --------------------------------------------------------------- pool state */

typedef struct gr_chunk {
    struct gr_chunk *next;
    size_t block_bytes;
    size_t block_count;
    unsigned char *storage;
} gr_chunk;

typedef struct {
    gr_header *header;
    uint64_t generation;
} gr_candidate;

typedef struct {
    gr_candidate *data;
    size_t len;
    size_t cap;
} gr_candidate_vec;

static gr_header *g_free_lists[GR_MAX_CLASS + 1];
static gr_chunk *g_chunks;
static gr_candidate_vec g_candidates;
static gr_stats g_stats;

static void candidate_push(gr_header *header) {
    if (g_candidates.len == g_candidates.cap) {
        size_t next = g_candidates.cap ? g_candidates.cap * 2u : 16u;
        gr_candidate *grown =
            realloc(g_candidates.data, next * sizeof(*grown));
        if (grown == NULL) {
            fputs("genref: out of memory growing the candidate buffer\n",
                  stderr);
            abort();
        }
        g_candidates.data = grown;
        g_candidates.cap = next;
    }
    g_candidates.data[g_candidates.len].header = header;
    g_candidates.data[g_candidates.len].generation = header->generation;
    g_candidates.len++;
}

static unsigned class_index_for(size_t total_bytes) {
    unsigned index = GR_MIN_CLASS;
    size_t size = (size_t)1 << GR_MIN_CLASS;
    while (size < total_bytes) {
        if (index == GR_MAX_CLASS) {
            fputs("genref: allocation exceeds the largest size class\n",
                  stderr);
            abort();
        }
        size <<= 1;
        index++;
    }
    return index;
}

/* Carves a fresh chunk into blocks of one size class and stocks the free list.
 *
 * Pool memory is never handed back to the host allocator while the program
 * runs. That is what makes the generation check sound: a stale reference must
 * be able to read its target's header and observe the bumped generation, which
 * requires the header to remain mapped and to keep counting up across reuse. */
static void stock_free_list(unsigned class_index) {
    size_t block_bytes = (size_t)1 << class_index;
    size_t chunk_bytes =
        block_bytes > GR_CHUNK_BYTES ? block_bytes : GR_CHUNK_BYTES;
    size_t block_count = chunk_bytes / block_bytes;

    gr_chunk *chunk = calloc(1, sizeof(*chunk));
    unsigned char *storage = calloc(1, chunk_bytes);
    if (chunk == NULL || storage == NULL) {
        fputs("genref: out of memory reserving a pool chunk\n", stderr);
        abort();
    }
    chunk->block_bytes = block_bytes;
    chunk->block_count = block_count;
    chunk->storage = storage;
    chunk->next = g_chunks;
    g_chunks = chunk;
    g_stats.bytes_reserved += chunk_bytes;

    for (size_t index = 0; index < block_count; index++) {
        gr_header *header = (gr_header *)(storage + index * block_bytes);
        /* Generations start at 1 so that a zeroed gr_ref, whose remembered
         * generation is 0, never matches a real allocation. */
        header->generation = 1;
        header->class_index = (unsigned char)class_index;
        header->live = false;
        header->next_free = g_free_lists[class_index];
        g_free_lists[class_index] = header;
    }
}

static void free_block(gr_header *header) {
    /* Bumping here is what invalidates every outstanding non-owning reference
     * to this object, including any that outlive the block's reuse. */
    header->generation++;
    header->live = false;
    header->buffered = false;
    header->color = GR_BLACK;
    header->owners = 0;
    header->type = NULL;
    header->next_free = g_free_lists[header->class_index];
    g_free_lists[header->class_index] = header;
    g_stats.live_objects--;
    g_stats.total_frees++;
}

/* ------------------------------------------------------------------ tracing */

struct gr_trace_ctx {
    gr_vec *out;
};

void gr_trace_edge(gr_trace_ctx *ctx, gr_owned child) {
    if (child.inner.target == NULL) {
        return;
    }
    gr_header *header = header_of(child.inner.target);
    if (!header->live || header->generation != child.inner.generation) {
        return;
    }
    vec_push(ctx->out, header);
}

/* Collects the owning edges of one object. The caller must finish with `out`
 * before the next call: the vector is the caller's, but tracers are free to
 * be re-entered only in this pattern. */
static void children_of(gr_header *header, gr_vec *out) {
    vec_clear(out);
    if (header->type == NULL || header->type->trace == NULL) {
        return;
    }
    gr_trace_ctx ctx = {out};
    header->type->trace(payload_of(header), &ctx);
}

/* --------------------------------------------------------------- allocation */

gr_owned gr_new(const gr_type *type) {
    if (type == NULL || type->size == 0) {
        fputs("genref: gr_new requires a type with a nonzero size\n", stderr);
        abort();
    }
    unsigned class_index = class_index_for(GR_HEADER_BYTES + type->size);
    if (g_free_lists[class_index] == NULL) {
        stock_free_list(class_index);
    } else {
        g_stats.blocks_reused++;
    }

    gr_header *header = g_free_lists[class_index];
    g_free_lists[class_index] = header->next_free;
    header->next_free = NULL;
    header->type = type;
    header->owners = 1;
    header->color = GR_BLACK;
    header->live = true;
    header->buffered = false;
    header->trial = 0;
    memset(payload_of(header), 0, type->size);

    g_stats.live_objects++;
    g_stats.total_allocations++;

    gr_owned owner;
    owner.inner.target = payload_of(header);
    owner.inner.generation = header->generation;
    return owner;
}

/* Registers a possible cycle root. A count that drops without reaching zero is
 * the signature of an object that may be held only from inside a cycle. */
static void note_cycle_candidate(gr_header *header) {
    if (header->type == NULL || header->type->trace == NULL ||
        header->type->acyclic) {
        return;
    }
    header->color = GR_PURPLE;
    if (!header->buffered) {
        header->buffered = true;
        candidate_push(header);
    }
}

/* Drops one owner, cascading into whatever the object owned. Iterative so a
 * long ownership chain cannot overflow the stack. */
static void release_header(gr_header *header) {
    gr_vec work = {0};
    gr_vec kids = {0};
    vec_push(&work, header);

    while (work.len > 0) {
        gr_header *current = vec_pop(&work);
        if (!current->live || current->owners == 0) {
            continue;
        }
        current->owners--;
        if (current->owners > 0) {
            note_cycle_candidate(current);
            continue;
        }

        /* Snapshot the owned edges before running the destructor so that a
         * destructor which clears its own fields cannot strand children. */
        children_of(current, &kids);
        for (size_t index = 0; index < kids.len; index++) {
            vec_push(&work, kids.data[index]);
        }
        if (current->type->destroy != NULL) {
            current->type->destroy(payload_of(current));
        }
        free_block(current);
    }

    vec_free(&work);
    vec_free(&kids);
}

gr_owned gr_clone(gr_owned owner) {
    if (owner.inner.target == NULL) {
        return GR_NULL_OWNED;
    }
    gr_header *header = header_of(owner.inner.target);
    if (!header->live || header->generation != owner.inner.generation) {
        g_stats.failed_checks++;
        fputs("genref: gr_clone on a released owning reference\n", stderr);
        abort();
    }
    header->owners++;
    header->color = GR_BLACK;
    return owner;
}

void gr_drop(gr_owned *owner) {
    if (owner == NULL || owner->inner.target == NULL) {
        return;
    }
    gr_header *header = header_of(owner->inner.target);
    uint64_t remembered = owner->inner.generation;
    owner->inner.target = NULL;
    owner->inner.generation = 0;

    if (!header->live || header->generation != remembered) {
        /* Dropping an owning reference twice, or one whose target was already
         * reclaimed as cyclic garbage. Releasing again would corrupt the count
         * of whatever now occupies the block. */
        g_stats.failed_checks++;
        return;
    }
    release_header(header);
}

/* --------------------------------------------------------------- references */

gr_ref gr_weaken(gr_owned owner) { return owner.inner; }

bool gr_alive(gr_ref ref) {
    if (ref.target == NULL) {
        return false;
    }
    gr_header *header = header_of(ref.target);
    return header->live && header->generation == ref.generation;
}

bool gr_owned_alive(gr_owned owner) { return gr_alive(owner.inner); }

void *gr_try(gr_ref ref) {
    if (!gr_alive(ref)) {
        if (ref.target != NULL) {
            g_stats.failed_checks++;
        }
        return NULL;
    }
    return ref.target;
}

void *gr_deref_at(gr_ref ref, const char *file, int line) {
    void *target = gr_try(ref);
    if (target == NULL) {
        fprintf(stderr,
                "genref: %s:%d: dereference of a %s generational reference\n",
                file, line, ref.target == NULL ? "null" : "stale");
        abort();
    }
    return target;
}

void *gr_get(gr_owned owner) {
    return gr_deref_at(owner.inner, "<owned>", 0);
}

uint32_t gr_owner_count(gr_ref ref) {
    if (!gr_alive(ref)) {
        return 0;
    }
    return header_of(ref.target)->owners;
}

uint64_t gr_current_generation(gr_ref ref) {
    if (ref.target == NULL) {
        return 0;
    }
    return header_of(ref.target)->generation;
}

/* --------------------------------------------------------- cycle collection */

size_t gr_pending_candidates(void) { return g_candidates.len; }

size_t gr_collect_cycles(void) {
    g_stats.cycle_collections++;

    gr_vec roots = {0};
    gr_vec subgraph = {0};
    gr_vec stack = {0};
    gr_vec kids = {0};
    gr_vec externals = {0};

    /* 1. Filter the candidate buffer. An entry whose generation no longer
     *    matches names a block that has since been freed and possibly reused,
     *    so the collector's own bookkeeping relies on the same generation
     *    check the language-level references use. */
    for (size_t index = 0; index < g_candidates.len; index++) {
        gr_header *header = g_candidates.data[index].header;
        if (!header->live ||
            header->generation != g_candidates.data[index].generation) {
            continue;
        }
        header->buffered = false;
        if (header->color == GR_PURPLE && header->owners > 0) {
            vec_push(&roots, header);
        }
    }
    g_candidates.len = 0;

    /* 2. Gather the candidate subgraph, seeding each node's trial count with
     *    its real owner count. */
    for (size_t index = 0; index < roots.len; index++) {
        vec_push(&stack, roots.data[index]);
    }
    while (stack.len > 0) {
        gr_header *current = vec_pop(&stack);
        if (current->color == GR_GRAY) {
            continue;
        }
        current->color = GR_GRAY;
        current->trial = (int64_t)current->owners;
        vec_push(&subgraph, current);
        children_of(current, &kids);
        for (size_t index = 0; index < kids.len; index++) {
            vec_push(&stack, kids.data[index]);
        }
    }

    /* 3. Subtract the edges internal to the subgraph. Whatever still has a
     *    positive trial count is held from outside it. */
    for (size_t index = 0; index < subgraph.len; index++) {
        children_of(subgraph.data[index], &kids);
        for (size_t child = 0; child < kids.len; child++) {
            kids.data[child]->trial--;
        }
    }

    /* 4. Scan. Externally held nodes go back to black, restoring the counts of
     *    everything they reach; the rest are provisionally white. */
    for (size_t index = 0; index < roots.len; index++) {
        vec_clear(&stack);
        vec_push(&stack, roots.data[index]);
        while (stack.len > 0) {
            gr_header *current = vec_pop(&stack);
            if (current->color != GR_GRAY) {
                continue;
            }
            if (current->trial > 0) {
                gr_vec restore = {0};
                current->color = GR_BLACK;
                vec_push(&restore, current);
                while (restore.len > 0) {
                    gr_header *node = vec_pop(&restore);
                    children_of(node, &kids);
                    for (size_t child = 0; child < kids.len; child++) {
                        gr_header *target = kids.data[child];
                        target->trial++;
                        if (target->color != GR_BLACK) {
                            target->color = GR_BLACK;
                            vec_push(&restore, target);
                        }
                    }
                }
                vec_free(&restore);
            } else {
                current->color = GR_WHITE;
                children_of(current, &kids);
                for (size_t child = 0; child < kids.len; child++) {
                    vec_push(&stack, kids.data[child]);
                }
            }
        }
    }

    /* 5. Collect. Owned edges leaving the garbage group are released normally;
     *    edges inside it are dropped wholesale with the group. */
    for (size_t index = 0; index < subgraph.len; index++) {
        gr_header *current = subgraph.data[index];
        if (current->color != GR_WHITE) {
            continue;
        }
        children_of(current, &kids);
        for (size_t child = 0; child < kids.len; child++) {
            if (kids.data[child]->color != GR_WHITE) {
                vec_push(&externals, kids.data[child]);
            }
        }
    }

    size_t reclaimed = 0;
    for (size_t index = 0; index < subgraph.len; index++) {
        gr_header *current = subgraph.data[index];
        if (current->color == GR_WHITE && current->type->destroy != NULL) {
            current->type->destroy(payload_of(current));
        }
    }
    for (size_t index = 0; index < externals.len; index++) {
        release_header(externals.data[index]);
    }
    for (size_t index = 0; index < subgraph.len; index++) {
        gr_header *current = subgraph.data[index];
        if (current->color == GR_WHITE) {
            free_block(current);
            reclaimed++;
        } else if (current->color == GR_GRAY) {
            current->color = GR_BLACK;
        }
    }

    g_stats.cycle_objects_freed += reclaimed;

    vec_free(&roots);
    vec_free(&subgraph);
    vec_free(&stack);
    vec_free(&kids);
    vec_free(&externals);
    return reclaimed;
}

/* ------------------------------------------------------------- introspection */

gr_stats gr_get_stats(void) { return g_stats; }

void gr_report_live(void) {
    size_t reported = 0;
    for (gr_chunk *chunk = g_chunks; chunk != NULL; chunk = chunk->next) {
        for (size_t index = 0; index < chunk->block_count; index++) {
            gr_header *header =
                (gr_header *)(chunk->storage + index * chunk->block_bytes);
            if (!header->live) {
                continue;
            }
            fprintf(stderr, "genref: live %s at %p generation %llu owners %u\n",
                    header->type != NULL ? header->type->name : "<unknown>",
                    payload_of(header),
                    (unsigned long long)header->generation, header->owners);
            reported++;
        }
    }
    fprintf(stderr, "genref: %zu live object(s)\n", reported);
}

void gr_shutdown(void) {
    gr_chunk *chunk = g_chunks;
    while (chunk != NULL) {
        gr_chunk *next = chunk->next;
        free(chunk->storage);
        free(chunk);
        chunk = next;
    }
    g_chunks = NULL;
    free(g_candidates.data);
    g_candidates.data = NULL;
    g_candidates.len = 0;
    g_candidates.cap = 0;
    memset(g_free_lists, 0, sizeof(g_free_lists));
    memset(&g_stats, 0, sizeof(g_stats));
}
