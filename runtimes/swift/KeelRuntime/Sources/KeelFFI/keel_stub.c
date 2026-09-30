/*
 * keel_stub.c - link-time stand-ins for the Keel C ABI (docs/SPEC.md section 6).
 *
 * Compiled only when KEEL_STUB_FFI is defined (Package.swift adds the define through
 * `cSettings` unless KEEL_LINK_CORE=1). It exists so that the Swift package builds and its
 * tests run on a machine that does not have the Rust core.
 *
 * Behaviour: the stub reports ABI version 0 (the real core reports KEEL_ABI_VERSION), returns
 * empty buffers and fails every call with the documented "bad request" / non-zero codes, so
 * a runtime that reaches it fails loudly at attach time. It never calls the registered
 * callbacks and never allocates, so `keel_buf_free` is a no-op.
 *
 * Never define KEEL_STUB_FFI while linking the real core: the symbols would clash.
 */
#include "include/keel.h"

#ifdef KEEL_STUB_FFI

#include <stddef.h>

static KeelBuf keel_stub_empty_buf(void) {
    KeelBuf buf;
    buf.ptr = NULL;
    buf.len = 0;
    buf.cap = 0;
    return buf;
}

uint32_t keel_abi_version(void) {
    return 0;
}

uint64_t keel_schema_hash(void) {
    return 0;
}

KeelBuf keel_schema_json(void) {
    return keel_stub_empty_buf();
}

uint32_t keel_init(const uint8_t *cfg, uint32_t len, keel_reply_cb reply, keel_changeset_cb changes, keel_stream_cb stream, void *user) {
    (void)cfg;
    (void)len;
    (void)reply;
    (void)changes;
    (void)stream;
    (void)user;
    return 1; /* non-zero: the core is not linked */
}

void keel_shutdown(void) {
}

uint32_t keel_call(const uint8_t *ptr, uint32_t len) {
    (void)ptr;
    (void)len;
    return 5; /* bad request */
}

KeelBuf keel_call_sync(const uint8_t *ptr, uint32_t len) {
    (void)ptr;
    (void)len;
    return keel_stub_empty_buf();
}

void keel_cancel(uint32_t call_id) {
    (void)call_id;
}

void keel_stream_credit(uint32_t call_id, uint32_t credit) {
    (void)call_id;
    (void)credit;
}

void keel_observe(uint64_t handle, uint32_t signal_id, uint8_t on) {
    (void)handle;
    (void)signal_id;
    (void)on;
}

void keel_release(uint64_t handle) {
    (void)handle;
}

void keel_port_register(uint32_t port_id, keel_port_cb cb, void *user) {
    (void)port_id;
    (void)cb;
    (void)user;
}

void keel_port_reply(const uint8_t *ptr, uint32_t len) {
    (void)ptr;
    (void)len;
}

void keel_event(uint32_t port_id, uint32_t method_id, const uint8_t *ptr, uint32_t len) {
    (void)port_id;
    (void)method_id;
    (void)ptr;
    (void)len;
}

void keel_timer_fired(uint32_t timer_id) {
    (void)timer_id;
}

KeelBuf keel_snapshot(void) {
    return keel_stub_empty_buf();
}

uint32_t keel_restore(const uint8_t *ptr, uint32_t len) {
    (void)ptr;
    (void)len;
    return 5; /* bad request */
}

KeelBuf keel_stats_json(void) {
    return keel_stub_empty_buf();
}

void keel_buf_free(KeelBuf buf) {
    (void)buf;
}

#endif /* KEEL_STUB_FFI */
