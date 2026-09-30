# SDE: keel-ffi review fixes (branch `wt/ffi-fixes`)

Closes `.10x/reviews/2026-09-30-keel-ffi-review.md` (H1, M1 to M4, L1 to L5, I1). Decision record:
ADR-026.

| Finding | Change | Regression tests |
|---|---|---|
| H1 | `registry.rs`: `Arc<PortReg>` + in-flight count, removal drains (never under the map lock), thread-local stack guards self-removal | `tests/host_contract.rs` (`unregistering_a_port_waits_...`, `replacing_...`, `shutdown_waits_...`, `no_callback_outlives_...`, `unregistering_from_inside_...`), `registry.rs` unit tests, `tests/c/lifetime.c`; old code = ASan heap-use-after-free in both the Rust and the C version |
| L5 | `session::stop` retires the registrations inside the INSTALLED critical section | `init_waits_for_a_shutdown_that_is_draining_port_callbacks`, `init_racing_shutdown_leaves_a_consistent_process` |
| M1 | `take_host_reply` always `free()`s; `cap` reserved | `native::tests::*` (Miri: old branch = UB, new = clean), `abi.rs::a_host_that_sets_cap_on_its_malloc_block_is_still_freed_with_free` |
| M2 | contract in `keel.h`, SPEC 6, `native.rs` docs, README; allowed-list verified against the code | `abi.rs::callbacks_may_call_exactly_the_documented_entry_points`, `host_contract.rs::port_callbacks_run_concurrently_...` |
| M3 | `smoke.c` 8-byte layout; `tests/c/run.sh` builds smoke + lifetime (ASan); `tests/wasm/run.sh` builds the TS runtime fresh, no skipping; CI runs the C harness (Linux + macOS), `npm ci` in `wasm-ffi`, wasm32 clippy, a Miri run of the out_reply path | the harnesses themselves |
| M4 | `api::port_reply` drops a well-formed reply with id 0 | `host_contract.rs::a_late_answer_to_a_log_record_is_dropped_not_logged`, `api` unit test, `lifetime.c::a_late_log_answer_does_not_loop` |
| L1 | empty snapshot carries `keel_runtime::object_table::process_generation_floor()` | `abi.rs::a_snapshot_with_no_runtime_keeps_the_process_generation_floor`, unit tests |
| L2 | `replies.rs` (`ReplyWatch`) keyed by port call id | wasm `raw.test.mjs` ("a host that answers a different call ...") + unit tests |
| L3 | wasm `keel_alloc` panics (trap) for unsatisfiable sizes; TS `allocate()` refuses 0 | `raw.test.mjs` ("keel_alloc traps ..."), `wasm-main.test.ts` (keel_alloc returns 0) |
| L4 | documented per-thread `portSyncReply` (SPEC 6.1, keel.h, `KeelNative.Callbacks`); the shipped `InprocTransport` checked (ThreadLocal) | none (docs) |
| I1 | `Subscription::detach` drops its `Weak` instead of forgetting it | `ports::tests::subscribing_and_detaching_...`; C stress: heap flat over 1000 init/shutdown cycles |

Outside the brief's file list, flagged for the integrator: `keel-runtime/src/object_table.rs` gained the
read-only `process_generation_floor()` (L1 cannot be done without reading the private counter);
`docs/SPEC.md` 6.1 and 7 (per the task) and ADR-026 (R11).

Known local caveats: `abi.rs` does not link under ASan on macOS aarch64 (ld64 rejects the `inventory`
constructors of `keel-ports`/`keel-query`: "initializer pointer has no target"), so `host_contract.rs`
deliberately links no core and is the ASan test there; CI's Linux ASan job runs every test binary.
Miri needs `MIRIFLAGS=-Zmiri-disable-isolation` for the abi tests (the runtime reads the clock).
