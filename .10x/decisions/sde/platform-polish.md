# SDE: platform polish (branch `wt/platform-polish`)

Closes the platform-runtime defects the playground found (findings 3 to 6 of `playground.md`) and ships the
framework adapters SPEC 10.3 and 13 promise (finding 7). Nothing in the Rust crates except `keel-bindgen`'s Swift
generator changed.

| Finding | Fix | Test |
|---|---|---|
| 3. Swift generated streams lose backpressure | `KeelCore.stream(_:method:args:decode:mapError:)`: an `AsyncThrowingStream(unfolding:)` over the credit channel that decodes in `next()`. The generator emits `return self.core.stream(..., decode:, mapError:)` and no longer writes the `keelDecodeStream` helper. | `CoreStreamTests` (credit window through the decoding overload, stop reading, break, cancel, decode failure, `mapError`); contract S07 reads the generated `Probe.ticks` and checks the window (16 + the item waiting for credit); `generators.rs` forbids a copying wrapper |
| 4. `InprocTransport.start` calls `keel_init` before comparing hashes | `keel_schema_hash()` needs no running core: read and compared after the ABI check, before the claim and before `keel_init`. The entry points are an injectable `CoreEntry` (default: the C ABI) so the order is unit-tested. | `InprocTransportTests` (order of calls, mismatch while another core is loaded, claim released after a failed init); contract S16 step 1b (core initialised by another embedder: still `KeelSchemaMismatchError`) |
| 5. TS `Mirror` strands a change-set enqueued by a subscriber | `flush()` runs rounds until the queue is empty (each round one batch, so a signal is announced once per round), at most 1000 (the core's commit cap), then reports and hands the rest to a later flush; whatever an error leaves in the queue is scheduled, not stranded. | `mirror.test.ts` (six tests, five fail on the old code) |
| 6. Swift stores warn "must restate @unchecked Sendable" | Generated classes (stores, query handles, objects) say `, @unchecked Sendable` | `generators.rs`; the generated playground package builds with no warnings |
| 7. No `@keel/runtime/react` | `react`, `vue`, `svelte`, `solid` subpaths (below); `./worker` exported too | `react.test.ts` (jsdom, real React: SSR, hydration, StrictMode, life cycle), `vue.test.ts`, `svelte.test.ts` (against `svelte/store`), `solid.test.ts` (browser build), `lifetime.test.ts`, `package.test.ts` |

## Decisions

* **The fix for 3 is in the runtime, not in a better generated helper.** The credit logic already lives in one
  place (`StreamChannel`); a second hand-rolled bounded buffer in every generated file would be code a Swift engineer
  reads as a workaround (R3). The generated method still returns `AsyncThrowingStream<T, Error>` (SPEC 10.1), so no
  public generated shape changed; SPEC 17.3 lists the new overload. No ADR: no wire, ABI, threading or generated public
  shape is touched.
* **Hash before claim.** A wrong schema is wrong whether or not another core is loaded, so it is reported first;
  `alreadyLoaded` only follows a matching hash. `KeelCore.connect` keeps its own comparison for the other transports.
* **Adapter API.**
  * react: `useSignal(signal)` (also `null`/`undefined`, giving `undefined`), `useKeel(Class, core?)` and
    `useKeel(factory, deps)` for objects whose `create` takes arguments (query handles). `useKeel` returns the
    object or `undefined` (server, first render, until the core answers), throws a failed creation during render (error
    boundary), creates in an effect and closes on cleanup (StrictMode: two creations, one close), and never returns an
    object whose inputs changed (it is being closed).
  * vue: `useSignal(signalOrRefOrGetter)` as `Readonly<ShallowRef>` (a `watch` re-subscribes when a ref or getter
    changes; `toRaw` because a proxy cannot reach `#private` fields), `useKeel(Class, core?)` as a `ShallowRef`.
  * svelte: `signalStore(signal)`, a `Readable` (`subscribe` calls `run(peek())` first). No run-time import of svelte.
  * solid: `useSignal(signal)` as an `Accessor`, `useKeel(Class, core?)`; `Show` is the way to wait for the store.
  * All: the framework is an optional peer dependency and a devDependency; `@keel/runtime` core imports none of them
    (`package.test.ts` checks). `./node` of SPEC 13 is not exported: there is no node-specific module yet.
  * vue, solid and react (class form) share `openKeel`/`KeelClass` in `src/lifetime.ts` (create, hand over, close,
    also when the creation finishes after the owner is gone).
* **The playground web app** takes `useSignal` from the adapter (its own hook is deleted) and `CounterView` owns its
  store with `useKeel(Counter)`: the tab opens it, the tab closes it. The other views share stores created at startup,
  so their state survives a tab switch. Playwright smoke passes (installed Chrome, `KEEL_BROWSER_CHANNEL=chrome`).

## Found on the way (not fixed here)

* `@Observable` does not compile a stored property with a back-ticked name: the `stores` golden has a signal named
  `default`, so `crates/keel-bindgen/tests/golden/stores/swift` (and the keel-cli copy) does not build with SwiftPM,
  and never did (the goldens are compared as text; only `full`, `objects`, `queries`, `stdlib` were compiled here, all
  warning-free). A schema with a signal named after a Swift keyword needs another spelling in the generator.
* `crates/keel-cli/tests/golden/stores/swift` is a derived copy of the stores output: it was refreshed
  (`UPDATE_GOLDEN=1`) for the new Swift shape; nothing else of keel-cli changed.
