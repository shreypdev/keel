/*
 * keel.h - the Keel native C ABI (docs/SPEC.md section 6).
 *
 * This header is the single source of truth for the C ABI on the Swift side. It must stay
 * byte-for-byte equivalent (in signatures and types) to the exports of crates/keel-ffi.
 * A change here is a boundary change and needs an ADR first (CLAUDE.md, R11).
 *
 * Conventions
 *  - Every `const uint8_t *ptr, uint32_t len` pair is borrowed for the duration of the call
 *    unless stated otherwise.
 *  - All functions are thread-safe.
 *  - `KeelBuf` memory is owned by the core; release it with `keel_buf_free`.
 *  - `reply_cb`, `changeset_cb`, `stream_cb` and `port_cb` may be invoked on the core thread,
 *    a blocking thread, or the caller's thread (sync path), possibly while the core lock is held.
 *    A callback must not call back into the core synchronously, except `keel_buf_free`; it
 *    enqueues onto its own thread instead (SPEC section 5.1 re-entrancy rule).
 */
#ifndef KEEL_H
#define KEEL_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ABI version implemented by this header. `keel_abi_version()` must return this value. */
#define KEEL_ABI_VERSION 1u

/* A byte buffer owned by the core. Free with `keel_buf_free`. */
typedef struct { uint8_t *ptr; uint32_t len; uint32_t cap; } KeelBuf;

/* Reply payload (SPEC 3.4) for an asynchronous call. */
typedef void (*keel_reply_cb)(void *user, uint32_t call_id, const uint8_t *ptr, uint32_t len);

/* ChangeSet payload (SPEC 3.5). */
typedef void (*keel_changeset_cb)(void *user, const uint8_t *ptr, uint32_t len);

/*
 * Port call from the core (SPEC 6.3). Returns:
 *   0 = replied synchronously into `out_reply` (a PortReply payload),
 *   1 = will reply asynchronously through `keel_port_reply`,
 *   2 = unavailable.
 *
 * HOST REPLY MEMORY RULE (the one place where the host, not the core, allocates a KeelBuf).
 * When the host returns 0 it fills `*out_reply` with a complete PortReply payload
 * (`port_call_id u32, status u8, body`, never empty) in a block obtained from the C allocator:
 *
 *     out_reply->ptr = (uint8_t *)malloc(n);   // host allocates with malloc
 *     out_reply->len = n;
 *     out_reply->cap = 0;                       // reserved: set it to 0
 *
 * Ownership passes to the core when the callback returns. The core copies the bytes
 * immediately and ALWAYS releases the block with `free`: never with `keel_buf_free` (that
 * function is for buffers the core allocated) and never as a Rust allocation, whatever `cap`
 * says. `cap` is reserved and ignored; do not use it for the block's capacity (an earlier
 * draft let a non-zero `cap` select a Rust deallocator, and a host that filled it in
 * naturally made the core free a C block with the wrong allocator). The host must not touch
 * the block after returning. When the host returns 1 or 2 it leaves `*out_reply` untouched and
 * the core ignores it. `ptr`/`len` of the arguments are borrowed for the duration of the
 * callback only: copy them before returning if they are needed later (an asynchronous port
 * does exactly that).
 */
typedef uint8_t (*keel_port_cb)(void *user, uint32_t port_id, uint32_t method_id, uint32_t port_call_id, const uint8_t *ptr, uint32_t len, KeelBuf *out_reply);

/* StreamItem payload (SPEC 3.7). */
typedef void (*keel_stream_cb)(void *user, uint32_t call_id, const uint8_t *ptr, uint32_t len);

uint32_t keel_abi_version(void);                       /* 1 */
uint64_t keel_schema_hash(void);
KeelBuf  keel_schema_json(void);                       /* owned copy */
uint32_t keel_init(const uint8_t *cfg, uint32_t len, keel_reply_cb reply, keel_changeset_cb changes, keel_stream_cb stream, void *user); /* idempotent per process; cfg = encoded RuntimeConfig record; returns 0 ok */
void     keel_shutdown(void);
uint32_t keel_call(const uint8_t *ptr, uint32_t len);  /* Call payload (SPEC 3.3); returns 0 accepted, 5 bad request. Reply via reply_cb. Works for sync and async methods. */
KeelBuf  keel_call_sync(const uint8_t *ptr, uint32_t len); /* Reply payload (SPEC 3.4) returned directly; only for sync methods (async -> status 5) */
void     keel_cancel(uint32_t call_id);
void     keel_stream_credit(uint32_t call_id, uint32_t credit);
void     keel_observe(uint64_t handle, uint32_t signal_id, uint8_t on);
void     keel_release(uint64_t handle);
void     keel_port_register(uint32_t port_id, keel_port_cb cb, void *user);
void     keel_port_reply(const uint8_t *ptr, uint32_t len);   /* PortReply payload */
void     keel_event(uint32_t port_id, uint32_t method_id, const uint8_t *ptr, uint32_t len);
void     keel_timer_fired(uint32_t timer_id);
KeelBuf  keel_snapshot(void);
uint32_t keel_restore(const uint8_t *ptr, uint32_t len);
KeelBuf  keel_stats_json(void);                        /* live handles, tasks, txn count, crossings */
void     keel_buf_free(KeelBuf buf);

#ifdef __cplusplus
}
#endif

#endif /* KEEL_H */
