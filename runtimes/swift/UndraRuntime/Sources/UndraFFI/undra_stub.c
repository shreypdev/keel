/*
 * undra_stub.c - link-time stand-ins for the Undra C ABI (docs/SPEC.md section 6).
 *
 * Compiled only when UNDRA_STUB_FFI is defined (Package.swift adds the define through
 * `cSettings` unless UNDRA_LINK_CORE=1). It exists so that the Swift package builds and its
 * tests run on a machine that does not have the Rust core.
 *
 * Behaviour: the stub reports ABI version 0 (the real core reports UNDRA_ABI_VERSION), returns
 * empty buffers and fails every call with the documented "bad request" / non-zero codes, so
 * a runtime that reaches it fails loudly at attach time. It never calls the registered
 * callbacks and never allocates, so `undra_buf_free` is a no-op.
 *
 * Never define UNDRA_STUB_FFI while linking the real core: the symbols would clash.
 */
#include "include/undra.h"

#ifdef UNDRA_STUB_FFI

#include <stddef.h>

static UndraBuf undra_stub_empty_buf(void) {
    UndraBuf buf;
    buf.ptr = NULL;
    buf.len = 0;
    buf.cap = 0;
    return buf;
}

uint32_t undra_abi_version(void) {
    return 0;
}

uint64_t undra_schema_hash(void) {
    return 0;
}

UndraBuf undra_schema_json(void) {
    return undra_stub_empty_buf();
}

uint32_t undra_init(const uint8_t *cfg, uint32_t len, undra_reply_cb reply, undra_changeset_cb changes, undra_stream_cb stream, void *user) {
    (void)cfg;
    (void)len;
    (void)reply;
    (void)changes;
    (void)stream;
    (void)user;
    return 1; /* non-zero: the core is not linked */
}

void undra_shutdown(void) {
}

uint32_t undra_call(const uint8_t *ptr, uint32_t len) {
    (void)ptr;
    (void)len;
    return 5; /* bad request */
}

UndraBuf undra_call_sync(const uint8_t *ptr, uint32_t len) {
    (void)ptr;
    (void)len;
    return undra_stub_empty_buf();
}

void undra_cancel(uint32_t call_id) {
    (void)call_id;
}

void undra_stream_credit(uint32_t call_id, uint32_t credit) {
    (void)call_id;
    (void)credit;
}

void undra_observe(uint64_t handle, uint32_t signal_id, uint8_t on) {
    (void)handle;
    (void)signal_id;
    (void)on;
}

void undra_release(uint64_t handle) {
    (void)handle;
}

void undra_port_register(uint32_t port_id, undra_port_cb cb, void *user) {
    (void)port_id;
    (void)cb;
    (void)user;
}

void undra_port_reply(const uint8_t *ptr, uint32_t len) {
    (void)ptr;
    (void)len;
}

void undra_event(uint32_t port_id, uint32_t method_id, const uint8_t *ptr, uint32_t len) {
    (void)port_id;
    (void)method_id;
    (void)ptr;
    (void)len;
}

void undra_timer_fired(uint32_t timer_id) {
    (void)timer_id;
}

UndraBuf undra_snapshot(void) {
    return undra_stub_empty_buf();
}

uint32_t undra_restore(const uint8_t *ptr, uint32_t len) {
    (void)ptr;
    (void)len;
    return 5; /* bad request */
}

UndraBuf undra_stats_json(void) {
    return undra_stub_empty_buf();
}

void undra_buf_free(UndraBuf buf) {
    (void)buf;
}

#endif /* UNDRA_STUB_FFI */
