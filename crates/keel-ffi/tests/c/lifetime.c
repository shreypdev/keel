/*
 * The host contract of keel.h, checked from a real C host (review H1, M2, M4): built by run.sh
 * with -Wall -Wextra -Werror and, where the compiler has it, -fsanitize=address. The hosts
 * really free() their `user` the moment keel_port_register / keel_shutdown returns, so a port
 * callback that still ran (or started) afterwards is a heap-use-after-free that ASan reports.
 *
 * Nothing here needs a core: the Log port is reached through the runtime's own warnings (a
 * malformed keel_port_reply), which run on the calling thread without the core lock.
 */
#define _POSIX_C_SOURCE 200809L
#include "keel.h"

#include <assert.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#define PORT_LOG 0x575ff24au /* fnv1a32("port.Log") */

/* RuntimeConfig: platform String, mode String, core_threads u8, blocking_threads u8, log_level u8 */
static const uint8_t CFG[] = {1, 0, 0, 0, 'c', 6, 0, 0, 0, 'i', 'n', 'p', 'r', 'o', 'c', 1, 0, 2};

static void sleep_ms(long ms) {
    struct timespec t = {ms / 1000, (ms % 1000) * 1000000L};
    nanosleep(&t, NULL);
}

static void on_reply(void *u, uint32_t id, const uint8_t *p, uint32_t n) { (void)u; (void)id; (void)p; (void)n; }
static void on_changes(void *u, const uint8_t *p, uint32_t n) { (void)u; (void)p; (void)n; }
static void on_stream(void *u, uint32_t id, const uint8_t *p, uint32_t n) { (void)u; (void)id; (void)p; (void)n; }

/* What a host keeps behind `user`. It is free()d while a callback may still be sleeping. */
typedef struct {
    atomic_int entered, finished, running, max_running;
    int hold_ms;
    int answer;
} Host;

static Host *new_host(int hold_ms, int answer) {
    Host *h = calloc(1, sizeof *h);
    assert(h);
    h->hold_ms = hold_ms;
    h->answer = answer;
    return h;
}

static uint8_t on_port(void *user, uint32_t port, uint32_t method, uint32_t call, const uint8_t *args, uint32_t n,
                       KeelBuf *out) {
    (void)method; (void)call; (void)args; (void)n; (void)out;
    Host *h = user;
    if (port != PORT_LOG) return 2;
    atomic_fetch_add(&h->entered, 1);
    int now = atomic_fetch_add(&h->running, 1) + 1;
    int seen = atomic_load(&h->max_running);
    while (now > seen && !atomic_compare_exchange_weak(&h->max_running, &seen, now)) {}
    if (h->hold_ms) sleep_ms(h->hold_ms); /* the window a late unregister races against */
    atomic_fetch_sub(&h->running, 1);
    atomic_fetch_add(&h->finished, 1); /* touches `user` after the sleep: a UAF if it was freed */
    return (uint8_t)h->answer;
}

/* A malformed PortReply: warned about (level 3) through the Log port, on this thread. */
static void *provoke_a_log_record(void *arg) {
    (void)arg;
    const uint8_t junk[2] = {1, 2};
    keel_port_reply(junk, 2);
    return NULL;
}

static void wait_entered(Host *h, int n) {
    for (int i = 0; i < 5000 && atomic_load(&h->entered) < n; i++) sleep_ms(1);
    assert(atomic_load(&h->entered) >= n);
}

enum { UNREGISTER, REPLACE, SHUTDOWN };

/* H1: the removal must not return while the callback runs; the host frees `user` right after. */
static void host_may_free_user(int how) {
    Host *h = new_host(200, 2);
    keel_port_register(PORT_LOG, on_port, h);
    assert(keel_init(CFG, sizeof CFG, on_reply, on_changes, on_stream, NULL) == 0);

    pthread_t t;
    pthread_create(&t, NULL, provoke_a_log_record, NULL);
    wait_entered(h, 1); /* the callback is now asleep inside on_port on thread t */

    Host *other = NULL;
    if (how == UNREGISTER) {
        keel_port_register(PORT_LOG, NULL, NULL);
    } else if (how == REPLACE) {
        other = new_host(0, 2);
        keel_port_register(PORT_LOG, on_port, other);
    } else {
        keel_shutdown();
    }
    int finished = atomic_load(&h->finished), running = atomic_load(&h->running);
    free(h); /* what keel.h allows once the call has returned */
    pthread_join(t, NULL);
    assert(finished == 1 && running == 0); /* ...and the callback had indeed finished */

    keel_shutdown();
    free(other);
}

/* M2: callbacks run concurrently (the header says so); hold four of them at once. */
static void callbacks_run_concurrently(void) {
    Host *h = new_host(150, 2);
    keel_port_register(PORT_LOG, on_port, h);
    assert(keel_init(CFG, sizeof CFG, on_reply, on_changes, on_stream, NULL) == 0);
    pthread_t t[4];
    for (int i = 0; i < 4; i++) pthread_create(&t[i], NULL, provoke_a_log_record, NULL);
    for (int i = 0; i < 4; i++) pthread_join(t[i], NULL);
    assert(atomic_load(&h->finished) == 4);
    assert(atomic_load(&h->max_running) >= 2);
    keel_shutdown();
    free(h);
}

/* M4: a host that answers the Log port asynchronously and replies later (port call id 0) must not
 * make the core log "no port call 0 is pending" -- one more Log call, answered the same way. */
static atomic_int pingpong_calls;

static void *reply_later(void *arg) {
    uint32_t id = (uint32_t)(uintptr_t)arg;
    uint8_t reply[5] = {0};
    memcpy(reply, &id, 4);
    keel_port_reply(reply, sizeof reply);
    return NULL;
}

static uint8_t on_async_log(void *user, uint32_t port, uint32_t method, uint32_t call, const uint8_t *args,
                            uint32_t n, KeelBuf *out) {
    (void)user; (void)method; (void)args; (void)n; (void)out;
    if (port != PORT_LOG) return 2;
    if (atomic_fetch_add(&pingpong_calls, 1) >= 1000) return 2; /* a runaway, bounded for the test */
    pthread_t t;
    pthread_create(&t, NULL, reply_later, (void *)(uintptr_t)call);
    pthread_detach(t);
    return 1;
}

static void a_late_log_answer_does_not_loop(void) {
    keel_port_register(PORT_LOG, on_async_log, NULL);
    assert(keel_init(CFG, sizeof CFG, on_reply, on_changes, on_stream, NULL) == 0);
    provoke_a_log_record(NULL); /* one warning; its call id is 0 and is answered "later" */
    sleep_ms(300);
    int calls = atomic_load(&pingpong_calls);
    assert(calls >= 1 && calls < 10); /* the old core made ~100,000 calls per second here */
    keel_shutdown();
}

int main(void) {
    host_may_free_user(UNREGISTER);
    host_may_free_user(REPLACE);
    host_may_free_user(SHUTDOWN);
    callbacks_run_concurrently();
    a_late_log_answer_does_not_loop();
    puts("c lifetime: ok");
    return 0;
}
