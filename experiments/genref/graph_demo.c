/* A directed graph with shared ownership, cycles, and non-owning observers.
 *
 * Demonstrates the three things counting alone cannot do on its own:
 * shared ownership deciding when to free, non-owning references going
 * detectably stale instead of dangling, and cyclic garbage being reclaimed. */

#include "genref.h"

#include <stdio.h>
#include <string.h>

#define MAX_EDGES 4

typedef struct {
    char label[16];
    size_t edge_count;
    gr_owned edges[MAX_EDGES];
} Vertex;

static void vertex_destroy(void *object) {
    printf("    destroying %s\n", ((Vertex *)object)->label);
}

static void vertex_trace(void *object, gr_trace_ctx *ctx) {
    Vertex *vertex = (Vertex *)object;
    for (size_t index = 0; index < vertex->edge_count; index++) {
        gr_trace_edge(ctx, vertex->edges[index]);
    }
}

static const gr_type VERTEX_TYPE = {
    .name = "Vertex",
    .size = sizeof(Vertex),
    .destroy = vertex_destroy,
    .trace = vertex_trace,
    .acyclic = false,
};

static gr_owned make_vertex(const char *label) {
    gr_owned owner = gr_new(&VERTEX_TYPE);
    Vertex *vertex = (Vertex *)gr_get(owner);
    snprintf(vertex->label, sizeof(vertex->label), "%s", label);
    return owner;
}

/* Adds an owning edge from `source` to `target`. Both ends stay alive while
 * the edge exists, which is what gives the graph multiple owners per node. */
static void add_edge(gr_owned source, gr_owned target) {
    Vertex *vertex = (Vertex *)gr_get(source);
    if (vertex->edge_count == MAX_EDGES) {
        fputs("demo: vertex is full\n", stderr);
        return;
    }
    vertex->edges[vertex->edge_count++] = gr_clone(target);
}

static void print_state(const char *stage, gr_ref *observers,
                        const char **names, size_t count) {
    printf("  %s\n", stage);
    for (size_t index = 0; index < count; index++) {
        if (gr_alive(observers[index])) {
            printf("    %-4s alive, %u owner(s)\n", names[index],
                   gr_owner_count(observers[index]));
        } else {
            printf("    %-4s reclaimed (reference is stale, not dangling)\n",
                   names[index]);
        }
    }
    gr_stats stats = gr_get_stats();
    printf("    live objects: %zu, pending cycle candidates: %zu\n",
           stats.live_objects, gr_pending_candidates());
}

int main(void) {
    /*        root
     *         |
     *         v
     *      +--a-->b--+        a, b, c form a cycle
     *      |  ^     |         hub is owned by both a and c
     *      |  +--c<-+
     *      v
     *     hub <-------- also owned by `keep`
     */
    gr_owned root = make_vertex("root");
    gr_owned a = make_vertex("a");
    gr_owned b = make_vertex("b");
    gr_owned c = make_vertex("c");
    gr_owned hub = make_vertex("hub");
    gr_owned keep = gr_clone(hub);

    add_edge(root, a);
    add_edge(a, b);
    add_edge(b, c);
    add_edge(c, a); /* closes the cycle */
    add_edge(a, hub);
    add_edge(c, hub);

    const char *names[] = {"root", "a", "b", "c", "hub"};
    gr_ref observers[] = {gr_weaken(root), gr_weaken(a), gr_weaken(b),
                          gr_weaken(c), gr_weaken(hub)};
    const size_t count = sizeof(names) / sizeof(names[0]);

    printf("graph demo\n\n");
    print_state("after construction", observers, names, count);

    /* Release the local handles. Everything is still reachable from `root`. */
    gr_drop(&a);
    gr_drop(&b);
    gr_drop(&c);
    gr_drop(&hub);
    printf("\n");
    print_state("after dropping local handles", observers, names, count);

    /* Dropping the last handle on root frees root, and its edge into `a`
     * cascades. But a, b, c hold each other, so counting stalls there. */
    printf("\n  dropping root\n");
    gr_drop(&root);
    print_state("after dropping root", observers, names, count);

    printf("\n  running the cycle collector\n");
    size_t reclaimed = gr_collect_cycles();
    printf("    reclaimed %zu object(s)\n", reclaimed);
    print_state("after cycle collection", observers, names, count);

    /* `hub` survived because `keep` still owns it, even though both of its
     * owners inside the cycle went away. */
    printf("\n  dropping the last hub owner\n");
    gr_drop(&keep);
    print_state("final", observers, names, count);

    gr_stats stats = gr_get_stats();
    printf("\n  allocations: %zu, frees: %zu, stale checks caught: %zu\n",
           stats.total_allocations, stats.total_frees, stats.failed_checks);
    printf("  cycle collections: %zu, objects freed by them: %zu\n",
           stats.cycle_collections, stats.cycle_objects_freed);

    gr_report_live();
    gr_shutdown();
    return 0;
}
