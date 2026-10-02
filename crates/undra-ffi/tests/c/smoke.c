/*
 * A C host for the undra-ffi C ABI, written against the Swift package's undra.h (the header the
 * ABI is specified by): init with callbacks, a sync port answered through a malloc'd out_reply,
 * sync and async calls, snapshot, stats, shutdown and a second init. Built and run by run.sh
 * with -Wall -Wextra -Werror, so a prototype in undra.h that disagrees with the library in
 * arity or width is caught here as well as at run time.
 */
#include "undra.h"

/* The fixture core's one export (C ABI v2, ADR-044); a core's own `<namespace>_undra.h` declares it. */
const void *undra_fixture_undra_api(void);
/* Its table, read once in main() before anything else runs. */
static const UndraApi *api;

#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define PORT_LOG 0x575ff24au /* fnv1a32("port.Log") */

static uint32_t replies = 0, last_reply_call = 0;
static uint8_t last_reply_status = 255;
static uint32_t log_calls = 0;
static uint8_t last_log_level = 255;

static uint32_t rd32(const uint8_t *p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

static void on_reply(void *user, uint32_t call_id, const uint8_t *p, uint32_t n) {
    (void)user;
    assert(n >= 5 && rd32(p) == call_id);
    last_reply_call = call_id;
    last_reply_status = p[4];
    replies++;
}
static void on_changes(void *user, const uint8_t *p, uint32_t n) { (void)user; (void)p; (void)n; }
static void on_stream(void *user, uint32_t call_id, const uint8_t *p, uint32_t n) {
    (void)user; (void)call_id; (void)p; (void)n;
}

/* The Log port: args are `level u8, target String, message String`; answer with an empty PortReply. */
static uint8_t on_port(void *user, uint32_t port, uint32_t method, uint32_t call, const uint8_t *args,
                       uint32_t n, UndraBuf *out) {
    (void)user; (void)method;
    if (port != PORT_LOG || n < 1) return 2;
    last_log_level = args[0];
    log_calls++;
    uint8_t *reply = malloc(5);            /* host allocates with malloc ... */
    memset(reply, 0, 5);
    memcpy(reply, &call, 4);               /* port_call_id, status 0 = ok, empty body */
    out->ptr = reply;
    out->len = 5;
    out->cap = 0;                          /* cap is reserved; the core copies the block and free()s it */
    return 0;
}

/* RuntimeConfig: platform String, mode String, core_threads u8, blocking_threads u8, log_level u8 */
static const uint8_t CFG[] = {1, 0, 0, 0, 'c', 6, 0, 0, 0, 'i', 'n', 'p', 'r', 'o', 'c', 1, 0, 2};

static void call_payload(uint8_t *out, uint32_t method_id, uint32_t call_id) {
    memset(out, 0, 17); /* target 0 (free function), handle 0 */
    memcpy(out + 9, &method_id, 4);
    memcpy(out + 13, &call_id, 4);
}

int main(void) {
    api = (const UndraApi *)undra_fixture_undra_api();
    assert(api != NULL && api->abi_version == UNDRA_ABI_VERSION && api->size == sizeof(UndraApi));
    assert(strcmp(api->name_space, "undra_fixture") == 0);
    assert(api == undra_fixture_undra_api()); /* immutable: the same table every time */
    assert(api->abi_version == UNDRA_ABI_VERSION);
    uint64_t hash = api->schema_hash;
    UndraBuf schema = api->schema_json();
    assert(schema.len > 0 && schema.ptr[0] == '{');
    api->buf_free(schema);

    for (int round = 0; round < 2; round++) {
        assert(api->init(CFG, sizeof CFG, on_reply, on_changes, on_stream, NULL) == 0);
        assert(api->init(CFG, sizeof CFG, on_reply, on_changes, on_stream, NULL) == 0); /* idempotent */
        assert(api->schema_hash == hash);
        api->port_register(PORT_LOG, on_port, NULL);

        uint8_t call[17];
        call_payload(call, 0xDEADBEEFu, 7);
        UndraBuf r = api->call_sync(call, sizeof call); /* UndraBuf returned by value */
        assert(r.len >= 5 && rd32(r.ptr) == 7 && r.ptr[4] == 5); /* status 5: no such method */
        api->buf_free(r);

        replies = 0;
        call_payload(call, 0xDEADBEEFu, 8);
        assert(api->call(call, sizeof call) == 0);
        /* An unknown method is answered inline, on this thread, before undra_call returns. */
        assert(replies == 1 && last_reply_call == 8 && last_reply_status == 5);
        assert(api->call(call, 3) == 5); /* truncated payload: refused without a reply */

        log_calls = 0;
        api->observe(0x7777777700000001ull, 0, 1); /* unknown handle: a warning goes to the Log port */
        assert(log_calls == 1 && last_log_level == 3);
        api->release(0x7777777700000001ull);

        UndraBuf snap = api->snapshot();
        /* SPEC 5.9 / ADR-022, ADR-040: `count u32, generation_floor u64` then the stores. No stores: 12 bytes.
         * The library is linked without a core, so nothing in this process ever issued a handle:
         * the generation floor is 0. */
        assert(snap.len == 12 && rd32(snap.ptr) == 0 && rd32(snap.ptr + 4) == 0 && rd32(snap.ptr + 8) == 0);
        assert(api->restore(snap.ptr, snap.len) == 0);
        api->buf_free(snap);
        UndraBuf stats = api->stats_json();
        assert(stats.len > 0 && memchr(stats.ptr, '{', stats.len) != NULL);
        api->buf_free(stats);
        api->buf_free((UndraBuf){0}); /* the empty buffer is harmless */

        api->shutdown();
        api->shutdown(); /* idempotent */

        /* With no runtime the snapshot keeps the same layout and the process-wide floor (L1). */
        snap = api->snapshot();
        assert(snap.len == 12 && rd32(snap.ptr) == 0 && rd32(snap.ptr + 4) == 0 && rd32(snap.ptr + 8) == 0);
        api->buf_free(snap);
    }
    puts("c smoke: ok");
    return 0;
}
