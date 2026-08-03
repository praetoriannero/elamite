/* Test suite for the generational-reference runtime. */

#include "genref.h"

#include <stdio.h>
#include <string.h>

static int g_checks;
static int g_failures;

#define CHECK(condition)                                                    \
    do {                                                                    \
        g_checks++;                                                         \
        if (!(condition)) {                                                 \
            g_failures++;                                                   \
            fprintf(stderr, "  FAIL %s:%d: %s\n", __FILE__, __LINE__,       \
                    #condition);                                            \
        }                                                                   \
    } while (0)

#define RUN(test)                                                           \
    do {                                                                    \
        fprintf(stderr, "- %s\n", #test);                                   \
        test();                                                             \
        gr_shutdown();                                                      \
    } while (0)

/* ------------------------------------------------------------------- types */

static int g_leaf_destroyed;
static int g_node_destroyed;

typedef struct {
    int value;
} Leaf;

static void leaf_destroy(void *object) {
    (void)object;
    g_leaf_destroyed++;
}

static const gr_type LEAF_TYPE = {
    .name = "Leaf",
    .size = sizeof(Leaf),
    .destroy = leaf_destroy,
    .trace = NULL,
    .acyclic = true,
};

/* A graph node with two owning edges and one non-owning back-edge. */
typedef struct {
    int id;
    gr_owned left;
    gr_owned right;
    gr_ref back;
} Node;

static void node_destroy(void *object) {
    (void)object;
    g_node_destroyed++;
}

static void node_trace(void *object, gr_trace_ctx *ctx) {
    Node *node = (Node *)object;
    gr_trace_edge(ctx, node->left);
    gr_trace_edge(ctx, node->right);
    /* `back` is deliberately not traced: it is non-owning. */
}

static const gr_type NODE_TYPE = {
    .name = "Node",
    .size = sizeof(Node),
    .destroy = node_destroy,
    .trace = node_trace,
    .acyclic = false,
};

/* A list cell that owns its successor but can never form a cycle. */
typedef struct {
    int value;
    gr_owned next;
} Cell;

static void cell_trace(void *object, gr_trace_ctx *ctx) {
    gr_trace_edge(ctx, ((Cell *)object)->next);
}

static const gr_type CELL_TYPE = {
    .name = "Cell",
    .size = sizeof(Cell),
    .destroy = NULL,
    .trace = cell_trace,
    .acyclic = true,
};

/* ------------------------------------------------------------------- tests */

static void test_allocate_and_dereference(void) {
    gr_owned owner = gr_new(&LEAF_TYPE);
    CHECK(gr_owned_alive(owner));

    Leaf *leaf = (Leaf *)gr_get(owner);
    CHECK(leaf->value == 0); /* gr_new zeroes the payload */
    leaf->value = 42;

    gr_ref ref = gr_weaken(owner);
    CHECK(((Leaf *)GR_DEREF(ref))->value == 42);
    CHECK(gr_owner_count(ref) == 1);

    gr_drop(&owner);
    CHECK(owner.inner.target == NULL);
}

static void test_null_reference_is_never_alive(void) {
    gr_ref null_ref = GR_NULL_REF;
    CHECK(!gr_alive(null_ref));
    CHECK(gr_try(null_ref) == NULL);
    CHECK(gr_owner_count(null_ref) == 0);

    gr_owned null_owned = GR_NULL_OWNED;
    CHECK(!gr_owned_alive(null_owned));
    gr_drop(&null_owned); /* must not crash */
}

static void test_destructor_runs_once_on_last_release(void) {
    g_leaf_destroyed = 0;
    gr_owned first = gr_new(&LEAF_TYPE);
    gr_owned second = gr_clone(first);

    gr_drop(&first);
    CHECK(g_leaf_destroyed == 0);

    gr_drop(&second);
    CHECK(g_leaf_destroyed == 1);
}

static void test_stale_reference_is_detected(void) {
    gr_owned owner = gr_new(&LEAF_TYPE);
    gr_ref observer = gr_weaken(owner);
    CHECK(gr_alive(observer));

    gr_drop(&owner);

    CHECK(!gr_alive(observer));
    CHECK(gr_try(observer) == NULL);
    CHECK(gr_get_stats().failed_checks > 0);
}

static void test_stale_reference_survives_block_reuse(void) {
    gr_owned first = gr_new(&LEAF_TYPE);
    gr_ref observer = gr_weaken(first);
    void *address = first.inner.target;
    uint64_t generation = observer.generation;
    gr_drop(&first);

    /* The pool hands the same block back for the next same-class request. The
     * remembered generation must no longer match. */
    gr_owned second = gr_new(&LEAF_TYPE);
    CHECK(second.inner.target == address);
    CHECK(second.inner.generation != generation);
    CHECK(!gr_alive(observer));
    CHECK(gr_alive(gr_weaken(second)));
    CHECK(gr_get_stats().blocks_reused == 1);

    gr_drop(&second);
}

static void test_multiple_owners_keep_the_object_alive(void) {
    g_leaf_destroyed = 0;
    gr_owned a = gr_new(&LEAF_TYPE);
    gr_owned b = gr_clone(a);
    gr_owned c = gr_clone(a);
    gr_ref observer = gr_weaken(a);

    CHECK(gr_owner_count(observer) == 3);

    gr_drop(&a);
    CHECK(gr_alive(observer));
    CHECK(gr_owner_count(observer) == 2);

    gr_drop(&b);
    CHECK(gr_alive(observer));
    CHECK(gr_owner_count(observer) == 1);
    CHECK(g_leaf_destroyed == 0);

    gr_drop(&c);
    CHECK(!gr_alive(observer));
    CHECK(g_leaf_destroyed == 1);
}

static void test_non_owning_references_do_not_retain(void) {
    g_leaf_destroyed = 0;
    gr_owned owner = gr_new(&LEAF_TYPE);
    gr_ref first = gr_weaken(owner);
    gr_ref second = gr_weaken(owner);
    gr_ref third = first;

    CHECK(gr_owner_count(first) == 1);
    gr_drop(&owner);

    CHECK(g_leaf_destroyed == 1);
    CHECK(!gr_alive(first));
    CHECK(!gr_alive(second));
    CHECK(!gr_alive(third));
}

static void test_release_cascades_through_owned_edges(void) {
    gr_owned head = gr_new(&CELL_TYPE);
    Cell *first = (Cell *)gr_get(head);
    first->value = 1;
    first->next = gr_new(&CELL_TYPE);

    Cell *middle = (Cell *)gr_get(first->next);
    middle->value = 2;
    middle->next = gr_new(&CELL_TYPE);

    gr_ref tail_observer = gr_weaken(middle->next);
    gr_ref middle_observer = gr_weaken(first->next);
    CHECK(gr_get_stats().live_objects == 3);

    gr_drop(&head);

    CHECK(!gr_alive(middle_observer));
    CHECK(!gr_alive(tail_observer));
    CHECK(gr_get_stats().live_objects == 0);
}

static void test_long_chain_release_does_not_recurse(void) {
    const int length = 50000;
    gr_owned head = gr_new(&CELL_TYPE);
    gr_owned cursor = gr_clone(head);
    for (int index = 1; index < length; index++) {
        Cell *cell = (Cell *)gr_get(cursor);
        cell->next = gr_new(&CELL_TYPE);
        gr_owned next = gr_clone(cell->next);
        gr_drop(&cursor);
        cursor = next;
    }
    gr_drop(&cursor);
    CHECK(gr_get_stats().live_objects == (size_t)length);

    gr_drop(&head);
    CHECK(gr_get_stats().live_objects == 0);
}

static void test_acyclic_types_are_never_candidates(void) {
    gr_owned leaf = gr_new(&LEAF_TYPE);
    gr_owned second_owner = gr_clone(leaf);
    gr_drop(&second_owner);
    CHECK(gr_pending_candidates() == 0);

    gr_owned node = gr_new(&NODE_TYPE);
    gr_owned node_second = gr_clone(node);
    gr_drop(&node_second);
    CHECK(gr_pending_candidates() == 1);

    gr_drop(&leaf);
    gr_drop(&node);
}

static void test_two_node_cycle_is_reclaimed(void) {
    g_node_destroyed = 0;
    gr_owned a = gr_new(&NODE_TYPE);
    gr_owned b = gr_new(&NODE_TYPE);
    ((Node *)gr_get(a))->id = 1;
    ((Node *)gr_get(b))->id = 2;

    /* a owns b, b owns a. */
    ((Node *)gr_get(a))->left = gr_clone(b);
    ((Node *)gr_get(b))->left = gr_clone(a);

    gr_ref observer_a = gr_weaken(a);
    gr_ref observer_b = gr_weaken(b);

    gr_drop(&a);
    gr_drop(&b);

    /* Counting alone cannot reclaim these: each is still held by the other. */
    CHECK(gr_alive(observer_a));
    CHECK(gr_alive(observer_b));
    CHECK(gr_get_stats().live_objects == 2);

    size_t reclaimed = gr_collect_cycles();
    CHECK(reclaimed == 2);
    CHECK(!gr_alive(observer_a));
    CHECK(!gr_alive(observer_b));
    CHECK(g_node_destroyed == 2);
    CHECK(gr_get_stats().live_objects == 0);
}

static void test_longer_cycle_is_reclaimed(void) {
    gr_owned a = gr_new(&NODE_TYPE);
    gr_owned b = gr_new(&NODE_TYPE);
    gr_owned c = gr_new(&NODE_TYPE);

    ((Node *)gr_get(a))->left = gr_clone(b);
    ((Node *)gr_get(b))->left = gr_clone(c);
    ((Node *)gr_get(c))->left = gr_clone(a);
    /* A chord across the cycle, so nodes have differing owner counts. */
    ((Node *)gr_get(a))->right = gr_clone(c);

    gr_ref observer = gr_weaken(b);
    gr_drop(&a);
    gr_drop(&b);
    gr_drop(&c);

    CHECK(gr_alive(observer));
    CHECK(gr_collect_cycles() == 3);
    CHECK(!gr_alive(observer));
    CHECK(gr_get_stats().live_objects == 0);
}

static void test_externally_held_cycle_is_kept(void) {
    gr_owned a = gr_new(&NODE_TYPE);
    gr_owned b = gr_new(&NODE_TYPE);
    ((Node *)gr_get(a))->left = gr_clone(b);
    ((Node *)gr_get(b))->left = gr_clone(a);

    /* An owner outside the cycle. */
    gr_owned external = gr_clone(a);
    gr_ref observer_a = gr_weaken(a);
    gr_ref observer_b = gr_weaken(b);

    gr_drop(&a);
    gr_drop(&b);

    CHECK(gr_collect_cycles() == 0);
    CHECK(gr_alive(observer_a));
    CHECK(gr_alive(observer_b));
    CHECK(gr_owner_count(observer_a) == 2); /* b's edge plus `external` */

    /* Once the outside owner goes, the cycle becomes collectable. */
    gr_drop(&external);
    CHECK(gr_collect_cycles() == 2);
    CHECK(!gr_alive(observer_a));
    CHECK(!gr_alive(observer_b));
}

static void test_cycle_releases_edges_leaving_the_group(void) {
    /* a <-> b forms a cycle; both own `shared`, which also has an outside
     * owner. Collecting the cycle must decrement `shared` without freeing it. */
    gr_owned a = gr_new(&NODE_TYPE);
    gr_owned b = gr_new(&NODE_TYPE);
    gr_owned shared = gr_new(&NODE_TYPE);

    ((Node *)gr_get(a))->left = gr_clone(b);
    ((Node *)gr_get(b))->left = gr_clone(a);
    ((Node *)gr_get(a))->right = gr_clone(shared);
    ((Node *)gr_get(b))->right = gr_clone(shared);

    gr_ref shared_observer = gr_weaken(shared);
    CHECK(gr_owner_count(shared_observer) == 3);

    gr_drop(&a);
    gr_drop(&b);

    CHECK(gr_collect_cycles() == 2);
    CHECK(gr_alive(shared_observer));
    CHECK(gr_owner_count(shared_observer) == 1);
    CHECK(gr_get_stats().live_objects == 1);

    gr_drop(&shared);
    CHECK(!gr_alive(shared_observer));
}

static void test_back_edges_are_not_owning(void) {
    /* The idiomatic way to avoid needing the collector: parent owns child,
     * child refers back non-owningly. */
    gr_owned parent = gr_new(&NODE_TYPE);
    gr_owned child = gr_new(&NODE_TYPE);

    ((Node *)gr_get(parent))->left = gr_clone(child);
    ((Node *)gr_get(child))->back = gr_weaken(parent);

    gr_ref parent_observer = gr_weaken(parent);
    gr_ref child_observer = gr_weaken(child);

    gr_drop(&child); /* parent still owns it */
    CHECK(gr_alive(child_observer));

    gr_drop(&parent);
    /* No cycle collection needed: counting alone reclaimed both. */
    CHECK(!gr_alive(parent_observer));
    CHECK(!gr_alive(child_observer));
    CHECK(gr_get_stats().live_objects == 0);

    /* The back edge is now stale rather than dangling. */
    CHECK(gr_collect_cycles() == 0);
}

static void test_collect_with_no_candidates_is_a_no_op(void) {
    CHECK(gr_collect_cycles() == 0);
    gr_owned owner = gr_new(&NODE_TYPE);
    CHECK(gr_collect_cycles() == 0);
    CHECK(gr_owned_alive(owner));
    gr_drop(&owner);
}

int main(void) {
    fprintf(stderr, "genref tests\n");

    RUN(test_allocate_and_dereference);
    RUN(test_null_reference_is_never_alive);
    RUN(test_destructor_runs_once_on_last_release);
    RUN(test_stale_reference_is_detected);
    RUN(test_stale_reference_survives_block_reuse);
    RUN(test_multiple_owners_keep_the_object_alive);
    RUN(test_non_owning_references_do_not_retain);
    RUN(test_release_cascades_through_owned_edges);
    RUN(test_long_chain_release_does_not_recurse);
    RUN(test_acyclic_types_are_never_candidates);
    RUN(test_two_node_cycle_is_reclaimed);
    RUN(test_longer_cycle_is_reclaimed);
    RUN(test_externally_held_cycle_is_kept);
    RUN(test_cycle_releases_edges_leaving_the_group);
    RUN(test_back_edges_are_not_owning);
    RUN(test_collect_with_no_candidates_is_a_no_op);

    fprintf(stderr, "\n%d check(s), %d failure(s)\n", g_checks, g_failures);
    return g_failures == 0 ? 0 : 1;
}
