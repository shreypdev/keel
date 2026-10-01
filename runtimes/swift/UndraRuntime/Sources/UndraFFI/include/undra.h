/*
 * undra.h - the Undra native C ABI (docs/SPEC.md section 6).
 *
 * This header is the single source of truth for the C ABI on the Swift side. It must stay
 * byte-for-byte equivalent (in signatures and types) to the exports of crates/undra-ffi.
 * A change here is a boundary change and needs an ADR first (CLAUDE.md, R11).
 *
 * Conventions
 *  - Every `const uint8_t *ptr, uint32_t len` pair is borrowed for the duration of the call
 *    unless stated otherwise. A NULL `ptr` is an empty payload.
 *  - `UndraBuf` memory the core returns is owned by the caller; release it with `undra_buf_free`.
 *  - Nothing unwinds out of an undra_* function: a failure is a status, a code or a log record.
 *
 * THE HOST CONTRACT (binding: a host that breaks a rule below has undefined behaviour)
 *
 * The "callbacks" are `reply_cb`, `changeset_cb` and `stream_cb` (given to undra_init) and every
 * `port_cb` (given to undra_port_register).
 *
 *  1. Lifetime.
 *     - `reply_cb`, `changeset_cb`, `stream_cb` and the `user` of undra_init must stay valid until
 *       undra_shutdown RETURNS. None of them is called again once it has returned.
 *     - A `port_cb` and its `user` must stay valid until undra_port_register(id, NULL, ...) or a
 *       replacing undra_port_register(id, other_cb, other_user) has RETURNED for that id, or
 *       undra_shutdown has returned. When one of them returns, no invocation of the old
 *       registration is running, none will start, and its `user` is never read again, so the
 *       host may free `user` at that moment. (The calls wait for that; see 5.)
 *
 *  2. Threads. A callback runs on whichever thread produced the event: the undra-core thread, a
 *     blocking-pool thread, or the host's own thread that is inside an undra_* call (for example
 *     undra_call, which may reply before it returns). Callbacks run CONCURRENTLY: two callbacks,
 *     or two invocations of the same one, can be active at once on different threads (four
 *     simultaneous port_cb invocations have been measured). They must be thread-safe and must
 *     not assume the main thread; hand work to the host's own thread by enqueueing.
 *     A callback may run while the core lock (or a store's delivery lock) is held, so keep it
 *     short and never wait in it for a thread that is itself waiting on the core.
 *
 *  3. No unwinding. A callback must not throw (C++, Objective-C), longjmp or otherwise leave
 *     non-locally: doing so through the core is undefined behaviour. Catch everything inside.
 *
 *  4. Re-entrancy: what a callback may call. The core lock may be held, so a callback must not
 *     call back into the core, except:
 *       undra_buf_free, undra_port_reply, undra_stream_credit, undra_timer_fired, undra_stats_json
 *       and the read-only undra_abi_version, undra_schema_hash, undra_schema_json.
 *     These never take the core lock. undra_port_reply is how a synchronous port is answered
 *     from inside its own port_cb on wasm, and how an asynchronous one is answered from any
 *     thread.
 *     undra_call, undra_call_sync, undra_cancel, undra_observe, undra_release, undra_event and
 *     undra_restore are REFUSED from a callback (status 5, code 6, or logged `E_REENTRANT` and
 *     ignored): never a deadlock, but nothing is done. undra_init, undra_shutdown,
 *     undra_port_register and undra_snapshot must not be called from a callback at all.
 *     To act on the core in response to a callback, enqueue the work onto another thread.
 *
 *  5. Blocking. undra_port_register (when it removes or replaces a registration) and
 *     undra_shutdown WAIT until the port callbacks of the registrations they remove, running on
 *     other threads, have returned. Therefore: never call them from inside a callback (debug
 *     builds of the core assert; release builds skip the wait for the calling thread's own
 *     callbacks, which it could never finish), never hold a lock that a port_cb needs while
 *     calling them, and a port_cb that never returns keeps them from returning.
 *     undra_shutdown may be called from any thread except a callback, also while other
 *     threads are inside other undra_* functions (those complete or fail softly); it delivers the
 *     status-3 replies of calls still in flight through `reply_cb` before it returns, and an
 *     undra_init on another thread waits for it.
 *
 *  6. The Log port. The core's own log records reach the host as calls to the standard Log port
 *     (port id 0x575ff24a, `Log.log(level u8, target String, message String)`), so register a
 *     port_cb for it to see them. They are fire and forget and carry port_call_id 0: the core
 *     never waits for an answer. Return 2, or 0 (following the reply memory rule below), or
 *     1; an undra_port_reply carrying port_call_id 0 is ignored silently. Real port calls are
 *     numbered from 1.
 *
 * JNI: the JNI shim (docs/SPEC.md section 6.1) follows the same contract. Its two-call protocol
 * for a synchronous port, `Callbacks.onPortCall` returning 0 and then `Callbacks.portSyncReply()`,
 * is made on the same thread back to back, so an implementation keeps the pending reply in
 * thread-local state (never in a shared field).
 */
#ifndef UNDRA_H
#define UNDRA_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ABI version implemented by this header. `undra_abi_version()` must return this value. */
#define UNDRA_ABI_VERSION 1u

/* A byte buffer owned by the core. Free with `undra_buf_free`. */
typedef struct { uint8_t *ptr; uint32_t len; uint32_t cap; } UndraBuf;

/* Reply payload (SPEC 3.4) for an asynchronous call. `ptr` is valid only during the call. */
typedef void (*undra_reply_cb)(void *user, uint32_t call_id, const uint8_t *ptr, uint32_t len);

/* ChangeSet payload (SPEC 3.5). `ptr` is valid only during the call. */
typedef void (*undra_changeset_cb)(void *user, const uint8_t *ptr, uint32_t len);

/*
 * Port call from the core (SPEC 6.3). Returns:
 *   0 = replied synchronously into `out_reply` (a PortReply payload),
 *   1 = will reply asynchronously through `undra_port_reply`,
 *   2 = unavailable.
 *
 * HOST REPLY MEMORY RULE (the one place where the host, not the core, allocates an UndraBuf).
 * When the host returns 0 it fills `*out_reply` with a complete PortReply payload
 * (`port_call_id u32, status u8, body`, never empty) in a block obtained from the C allocator:
 *
 *     out_reply->ptr = (uint8_t *)malloc(n);   // host allocates with malloc
 *     out_reply->len = n;
 *     out_reply->cap = 0;                       // reserved: set it to 0
 *
 * Ownership passes to the core when the callback returns. The core copies the bytes
 * immediately and ALWAYS releases the block with `free`: never with `undra_buf_free` (that
 * function is for buffers the core allocated) and never as a Rust allocation, whatever `cap`
 * says. `cap` is reserved and ignored; do not use it for the block's capacity (an earlier
 * draft let a non-zero `cap` select a Rust deallocator, and a host that filled it in
 * naturally made the core free a C block with the wrong allocator). The host must not touch
 * the block after returning. When the host returns 1 or 2 it leaves `*out_reply` untouched and
 * the core ignores it. `ptr`/`len` of the arguments are borrowed for the duration of the
 * callback only: copy them before returning if they are needed later (an asynchronous port
 * does exactly that).
 */
typedef uint8_t (*undra_port_cb)(void *user, uint32_t port_id, uint32_t method_id, uint32_t port_call_id, const uint8_t *ptr, uint32_t len, UndraBuf *out_reply);

/* StreamItem payload (SPEC 3.7). `ptr` is valid only during the call. */
typedef void (*undra_stream_cb)(void *user, uint32_t call_id, const uint8_t *ptr, uint32_t len);

uint32_t undra_abi_version(void);                       /* 1 */
uint64_t undra_schema_hash(void);
UndraBuf  undra_schema_json(void);                       /* owned copy */
uint32_t undra_init(const uint8_t *cfg, uint32_t len, undra_reply_cb reply, undra_changeset_cb changes, undra_stream_cb stream, void *user); /* idempotent per process; cfg = encoded RuntimeConfig record; returns 0 ok */
void     undra_shutdown(void);                          /* not from a callback; waits for running port callbacks (contract 5); init may follow */
uint32_t undra_call(const uint8_t *ptr, uint32_t len);  /* Call payload (SPEC 3.3); returns 0 accepted, 5 bad request. Reply via reply_cb. Works for sync and async methods. */
UndraBuf  undra_call_sync(const uint8_t *ptr, uint32_t len); /* Reply payload (SPEC 3.4) returned directly; only for sync methods (async -> status 5) */
void     undra_cancel(uint32_t call_id);
void     undra_stream_credit(uint32_t call_id, uint32_t credit);
void     undra_observe(uint64_t handle, uint32_t signal_id, uint8_t on);
void     undra_release(uint64_t handle);
void     undra_port_register(uint32_t port_id, undra_port_cb cb, void *user); /* cb NULL removes; removing or replacing waits for running callbacks of the old registration (contracts 1, 5) */
void     undra_port_reply(const uint8_t *ptr, uint32_t len);   /* PortReply payload; allowed from a callback; port_call_id 0 is ignored (contract 6) */
void     undra_event(uint32_t port_id, uint32_t method_id, const uint8_t *ptr, uint32_t len);
void     undra_timer_fired(uint32_t timer_id);
UndraBuf  undra_snapshot(void);                          /* also with no runtime: an empty snapshot carrying the process-wide generation floor (SPEC 5.9) */
uint32_t undra_restore(const uint8_t *ptr, uint32_t len);
UndraBuf  undra_stats_json(void);                        /* live handles, tasks, txn count, crossings */
void     undra_buf_free(UndraBuf buf);

#ifdef __cplusplus
}
#endif

#endif /* UNDRA_H */
