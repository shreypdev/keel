/*
 * Two cores in one process (ADR-044), as a C host sees them: two copies of the fixture library,
 * each `dlopen`ed (RTLD_LOCAL), so each is its own image with its own `undra_fixture_undra_api` table,
 * runtime, port registry, embedder slot, generation counter and threads. Built and run by run.sh
 * (with AddressSanitizer when the compiler has it), which passes the two copies' paths.
 *
 * What it proves, with core A shut down while core B is busy:
 *   - a panicking call through either table is a status 2 reply on both paths (sync and callback),
 *     never an abort (R6);
 *   - A's shutdown answers A's calls in flight (status 3) before it returns, and leaves B's open
 *     stream, B's port call in flight and B's runtime threads alone: B's stream still delivers on
 *     credit, B's port call is answered, B's synchronous calls work;
 *   - after A's shutdown returned, no callback A's host registered (reply, change-set, stream, port)
 *     runs again, A's `port_reply` for its abandoned port call is ignored, and A's `stats_json`
 *     reports no runtime and no runtime thread of its image although B's threads are running;
 *   - A can be started again beside B.
 */
#define _POSIX_C_SOURCE 200809L
#include "undra.h"

#include <assert.h>
#include <dlfcn.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

/* --- ids (undra-meta's fnv1a32 compositions) ------------------------------------------------- */

static uint32_t fnv(uint32_t hash, const char *s) {
    for (; *s; s++) {
        hash ^= (uint8_t)*s;
        hash *= 16777619u;
    }
    return hash;
}
static uint32_t id2(const char *a, const char *b) { return fnv(fnv(2166136261u, a), b); }
static uint32_t type_id(const char *t) { return fnv(2166136261u, t); }
static uint32_t method_id(const char *t, const char *m) { return fnv(id2(t, "."), m); }
static uint32_t port_id(const char *t) { return id2("port.", t); }

/* --- little-endian helpers --------------------------------------------------------------------- */

static uint32_t rd32(const uint8_t *p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}
static uint64_t rd64(const uint8_t *p) { return (uint64_t)rd32(p) | ((uint64_t)rd32(p + 4) << 32); }
static void wr32(uint8_t *p, uint32_t v) { memcpy(p, &v, 4); }
static void wr64(uint8_t *p, uint64_t v) { memcpy(p, &v, 8); }

/* --- one host per core ------------------------------------------------------------------------- */

#define MAX_CALLS 64

typedef struct {
    const char *name;
    const UndraApi *api;
    pthread_mutex_t lock;
    pthread_cond_t changed;
    int gone; /* set once the core's shutdown returned: no callback of this host may run then */
    uint8_t status[MAX_CALLS]; /* reply status per call id (255: none yet) */
    uint32_t stream_items[MAX_CALLS];
    int stream_ended[MAX_CALLS];
    uint32_t echo_port_call; /* the last deferred Echo.ping port call */
    uint32_t log_calls;
} Host;

static Host A = {.name = "A"}, B = {.name = "B"};

static void host_reset(Host *h) {
    pthread_mutex_lock(&h->lock);
    memset(h->status, 255, sizeof h->status);
    memset(h->stream_items, 0, sizeof h->stream_items);
    memset(h->stream_ended, 0, sizeof h->stream_ended);
    h->echo_port_call = 0;
    h->gone = 0;
    pthread_mutex_unlock(&h->lock);
}

static Host *host_of(void *user) {
    Host *h = user;
    assert(h == &A || h == &B);
    if (h->gone) {
        fprintf(stderr, "two_cores: a callback of core %s ran after its shutdown returned\n", h->name);
        abort();
    }
    return h;
}

/* Waits until `pred(h)` holds, at most 10 s. */
static int wait_for(Host *h, int (*pred)(Host *, uint32_t), uint32_t arg) {
    struct timespec until;
    clock_gettime(CLOCK_REALTIME, &until);
    until.tv_sec += 10;
    pthread_mutex_lock(&h->lock);
    int ok;
    while (!(ok = pred(h, arg))) {
        if (pthread_cond_timedwait(&h->changed, &h->lock, &until) != 0) {
            ok = pred(h, arg);
            break;
        }
    }
    pthread_mutex_unlock(&h->lock);
    return ok;
}
static int replied(Host *h, uint32_t call) { return h->status[call] != 255; }
static int port_called(Host *h, uint32_t unused) { (void)unused; return h->echo_port_call != 0; }
static uint32_t stream_items_wanted;
static int items_arrived(Host *h, uint32_t call) { return h->stream_items[call] >= stream_items_wanted; }

static void on_reply(void *user, uint32_t call_id, const uint8_t *p, uint32_t n) {
    Host *h = host_of(user);
    assert(n >= 5 && rd32(p) == call_id && call_id < MAX_CALLS);
    pthread_mutex_lock(&h->lock);
    h->status[call_id] = p[4];
    pthread_cond_broadcast(&h->changed);
    pthread_mutex_unlock(&h->lock);
}

static void on_changes(void *user, const uint8_t *p, uint32_t n) {
    (void)host_of(user);
    (void)p;
    (void)n;
}

static void on_stream(void *user, uint32_t call_id, const uint8_t *p, uint32_t n) {
    Host *h = host_of(user);
    assert(n >= 5 && rd32(p) == call_id && call_id < MAX_CALLS);
    pthread_mutex_lock(&h->lock);
    if (p[4] == 0) {
        h->stream_items[call_id]++;
    } else {
        h->stream_ended[call_id] = 1;
    }
    pthread_cond_broadcast(&h->changed);
    pthread_mutex_unlock(&h->lock);
}

/* Log: answered synchronously (a malloc'd PortReply); Echo.ping: deferred (1). */
static uint8_t on_port(void *user, uint32_t port, uint32_t method, uint32_t call, const uint8_t *args,
                       uint32_t n, UndraBuf *out) {
    Host *h = host_of(user);
    (void)method;
    (void)args;
    (void)n;
    if (port == port_id("Log")) {
        pthread_mutex_lock(&h->lock);
        h->log_calls++;
        pthread_mutex_unlock(&h->lock);
        uint8_t *reply = malloc(5);
        assert(reply != NULL);
        memset(reply, 0, 5);
        wr32(reply, call);
        out->ptr = reply;
        out->len = 5;
        out->cap = 0;
        return 0;
    }
    if (port == port_id("Echo")) {
        pthread_mutex_lock(&h->lock);
        h->echo_port_call = call;
        pthread_cond_broadcast(&h->changed);
        pthread_mutex_unlock(&h->lock);
        return 1;
    }
    return 2;
}

/* RuntimeConfig: platform "c", mode "inproc", core_threads 1, blocking_threads 2, log_level 2. */
static const uint8_t CFG[] = {1, 0, 0, 0, 'c', 6, 0, 0, 0, 'i', 'n', 'p', 'r', 'o', 'c', 1, 2, 2};

static void start(Host *h) {
    host_reset(h);
    assert(h->api->init(CFG, sizeof CFG, on_reply, on_changes, on_stream, h) == 0);
    h->api->port_register(port_id("Log"), on_port, h);
    h->api->port_register(port_id("Echo"), on_port, h);
}

/* --- calls ----------------------------------------------------------------------------------- */

/* A Call payload on an object method (target 1), `args` appended. */
static uint32_t method_call(uint8_t *out, uint64_t handle, const char *method, uint32_t call_id,
                            const uint8_t *args, uint32_t n) {
    out[0] = 1;
    wr64(out + 1, handle);
    wr32(out + 9, method_id("Calculator", method));
    wr32(out + 13, call_id);
    if (n > 0) memcpy(out + 17, args, n);
    return 17 + n;
}

/* `call_sync`: the reply's status, with the body's first 8 bytes in `*word` when there are some. */
static uint8_t call_sync(Host *h, const uint8_t *payload, uint32_t n, uint32_t call_id, uint64_t *word) {
    UndraBuf r = h->api->call_sync(payload, n);
    assert(r.len >= 5 && rd32(r.ptr) == call_id);
    uint8_t status = r.ptr[4];
    if (word != NULL && r.len >= 13) *word = rd64(r.ptr + 5);
    h->api->buf_free(r);
    return status;
}

static uint64_t calculator(Host *h) {
    uint8_t ctor[25] = {2};
    wr32(ctor + 1, type_id("Calculator"));
    wr32(ctor + 5, method_id("Calculator", "new"));
    wr32(ctor + 9, 1);
    wr64(ctor + 13, 0); /* base: i64 0 */
    uint64_t handle = 0;
    assert(call_sync(h, ctor, 21, 1, &handle) == 0 && handle != 0);
    return handle;
}

/* Submits `method` on `handle` through `call` (the callback path). */
static void submit(Host *h, uint64_t handle, const char *method, uint32_t call_id, uint32_t arg, int has_arg) {
    uint8_t payload[32], args[4];
    wr32(args, arg);
    uint32_t n = method_call(payload, handle, method, call_id, args, has_arg ? 4 : 0);
    assert(h->api->call(payload, n) == 0);
}

static int stats_say(Host *h, const char *needle) {
    UndraBuf s = h->api->stats_json();
    size_t want = strlen(needle);
    int found = 0;
    for (size_t at = 0; !found && s.len >= want && at <= s.len - want; at++) {
        found = memcmp(s.ptr + at, needle, want) == 0;
    }
    h->api->buf_free(s);
    return found;
}

static const UndraApi *open_core(const char *path) {
    void *lib = dlopen(path, RTLD_NOW | RTLD_LOCAL);
    if (lib == NULL) {
        fprintf(stderr, "two_cores: dlopen %s: %s\n", path, dlerror());
        exit(1);
    }
    const void *(*entry)(void) = (const void *(*)(void))dlsym(lib, "undra_fixture_undra_api");
    assert(entry != NULL);
    const UndraApi *api = entry();
    assert(api != NULL && api->abi_version == UNDRA_ABI_VERSION && api->size >= sizeof(UndraApi));
    assert(strcmp(api->name_space, "undra_fixture") == 0);
    return api;
}

int main(int argc, char **argv) {
    if (argc != 3) {
        fprintf(stderr, "usage: two_cores <copy A of libundra_fixture> <copy B>\n");
        return 2;
    }
    pthread_mutex_init(&A.lock, NULL);
    pthread_cond_init(&A.changed, NULL);
    pthread_mutex_init(&B.lock, NULL);
    pthread_cond_init(&B.changed, NULL);
    A.api = open_core(argv[1]);
    B.api = open_core(argv[2]);
    /* Two images: two tables, two sets of entry points, one schema. */
    assert(A.api != B.api && A.api->init != B.api->init && A.api->shutdown != B.api->shutdown);
    assert(A.api->schema_hash == B.api->schema_hash);

    start(&A);
    start(&B);
    uint64_t calc_a = calculator(&A), calc_b = calculator(&B);

    /* R6 through either table: a panic is a status 2 reply on the sync and the callback paths. */
    Host *both[2] = {&A, &B};
    uint64_t calcs[2] = {calc_a, calc_b};
    for (int i = 0; i < 2; i++) {
        uint8_t payload[32];
        uint32_t n = method_call(payload, calcs[i], "boom", 2, NULL, 0);
        assert(call_sync(both[i], payload, n, 2, NULL) == 2);
        submit(both[i], calcs[i], "boom", 3, 0, 0);
        assert(wait_for(both[i], replied, 3) && both[i]->status[3] == 2);
        submit(both[i], calcs[i], "async_boom", 4, 0, 0);
        assert(wait_for(both[i], replied, 4) && both[i]->status[4] == 2);
    }

    /* B: a stream left open without credit, and a port call the host has deferred. */
    submit(&B, calc_b, "ticks", 10, 1000, 1);
    assert(wait_for(&B, replied, 10) && B.status[10] == 4); /* StreamOpened */
    submit(&B, calc_b, "ping_host", 11, 7, 1);
    assert(wait_for(&B, port_called, 0));
    uint32_t b_port_call = B.echo_port_call;

    /* A: a port call in flight and a call that never finishes on its own. */
    submit(&A, calc_a, "ping_host", 11, 5, 1);
    assert(wait_for(&A, port_called, 0));
    uint32_t a_port_call = A.echo_port_call;
    submit(&A, calc_a, "never", 12, 0, 0);

    /* Shut A down while B is busy. Its calls in flight are answered as cancelled before it returns. */
    A.api->shutdown();
    assert(A.status[11] == 3 && A.status[12] == 3);
    pthread_mutex_lock(&A.lock);
    A.gone = 1; /* from here on, any callback with A's `user` aborts the test */
    pthread_mutex_unlock(&A.lock);
    assert(stats_say(&A, "\"initialized\":false") && stats_say(&A, "\"runtime_threads\":0"));

    /* A's late answer to its abandoned port call goes nowhere; A refuses calls without a runtime. */
    uint8_t late[9];
    wr32(late, a_port_call);
    late[4] = 0;
    wr32(late + 5, 1005);
    A.api->port_reply(late, sizeof late);
    uint8_t payload[32];
    uint32_t n = method_call(payload, calc_a, "add", 13, NULL, 0);
    assert(A.api->call(payload, n) == 5);

    /* B is untouched: its port call is answered, its stream delivers on credit, it logs to its own host. */
    assert(!stats_say(&B, "\"initialized\":false"));
    uint8_t answer[9];
    wr32(answer, b_port_call);
    answer[4] = 0;
    wr32(answer + 5, 1007);
    B.api->port_reply(answer, sizeof answer);
    assert(wait_for(&B, replied, 11) && B.status[11] == 0);
    B.api->stream_credit(10, 3);
    stream_items_wanted = 3;
    assert(wait_for(&B, items_arrived, 10));
    assert(!B.stream_ended[10]);
    pthread_mutex_lock(&B.lock);
    uint32_t logs_before = B.log_calls;
    pthread_mutex_unlock(&B.lock);
    B.api->observe(0x7777777700000001ull, 0, 1); /* unknown handle: a warning to B's Log port */
    pthread_mutex_lock(&B.lock);
    assert(B.log_calls > logs_before);
    pthread_mutex_unlock(&B.lock);
    uint8_t add_args[16] = {0};
    wr64(add_args, 2);
    wr64(add_args + 8, 3);
    n = method_call(payload, calc_b, "add", 14, add_args, 16);
    uint64_t sum = 0;
    assert(call_sync(&B, payload, n, 14, &sum) == 0 && sum == 5);

    /* A starts again beside B, and works. */
    start(&A);
    calc_a = calculator(&A);
    n = method_call(payload, calc_a, "add", 14, add_args, 16);
    sum = 0;
    assert(call_sync(&A, payload, n, 14, &sum) == 0 && sum == 5);

    /* B's shutdown ends B's open stream and leaves A running. */
    B.api->shutdown();
    pthread_mutex_lock(&B.lock);
    B.gone = 1;
    pthread_mutex_unlock(&B.lock);
    assert(stats_say(&B, "\"runtime_threads\":0"));
    n = method_call(payload, calc_a, "add", 15, add_args, 16);
    assert(call_sync(&A, payload, n, 15, &sum) == 0 && sum == 5);
    A.api->shutdown();
    A.gone = 1;

    puts("c two cores: ok");
    return 0;
}
