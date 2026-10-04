# React Native default ports (ADR-038 amendment B, G1b) - adversarial review

**Date:** 2026-10-02 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/rn-adapters` at `b1b2eb0` (`main` `3e8a304` merged) · **Read:** `CLAUDE.md` (R1, R2, R6, R10, R12), ADR-038 with
amendment B, ADR-026, ADR-031, ADR-034, `docs/SPEC.md` 6, 8, 11, 11.2, 17.4, `docs/REACT_NATIVE.md`,
`.10x/decisions/sde/rn-adapters.md`, `git diff main...HEAD` in full: `cpp/UndraStores.*`, `cpp/UndraDefaults.*`,
`cpp/UndraPlatformAndroid.cpp`, `cpp/UndraHost.*` and `cpp/UndraJsi.*` changes, `ios/UndraPlatformApple.mm`, the Java
library (`android/`), `src/{adapters,http,load,native,transport}.ts`, the tests (`cpp/test/*`, `android/test/*`,
`test/defaults.test.ts`), `examples/playground/core/src/platform.rs`, the playground app's checks,
`scripts/rn-device-checks.sh`, `ci.yml` and `rn-devices.yml`; for comparison the Swift `KeyValueAdapters.swift` and
`FsAdapter.swift`, `android-adapters`' `AndroidSecureStoreAdapter.kt`, `SecureSeal.kt`, `AndroidConnectivityAdapter.kt`
and the JVM `FileKv`, and `crates/undra-ffi` (`session.rs`, `api.rs`) and `undra-runtime` (`port_reply`, `event`) for
what the ports rely on · **Fixes:** `3c88f38`, `1c9b5d2`, `a1639db`, `f1f4a66`; `main` merged at `8b6fa73`.

## Verdict

**Sound; merge.** The part that could hurt most holds and is now proved: nothing a stopped core asked for reaches the
next one. `Host::shutdown` runs `undra_shutdown`, then stops the event source and joins the port workers, and only
then releases the process's slot (B3); a new test starts a reloaded runtime's core while an old `SecureStore` worker is
inside a job and shows that the start waits for the join and that the late reply reaches no core. Reversing B3's order
makes that test fail twice over (the new core starts early, then logs the old core's stray `port_reply`), so it guards
the M2 class of the React Native review for these ports. No use-after-free, double free or leak under ASan + UBSan in
any path tried, including a 32 MiB `Kv` write in flight at shutdown. `Fs` has no check-then-use gap: a directory
swapped atomically for a link to the outside, thousands of times while reads and writes walk through it, never leaks or
writes outside (a test that a check-then-use walk fails); a `Kv` writer killed at 40 random moments never leaves a torn
value; a full disk is a typed failure with the old value intact. The Android seal is the Kotlin adapter's byte for byte
in both directions, and the Keychain items' stored attributes are the Swift adapter's. Every device check passes on
both devices with the fixes (iOS 19/19, Android 20/20), and again after `main` was merged.

Nothing High. One Medium, fixed: `docs/SPEC.md`, the binding specification, still described React Native's async ports
as all JavaScript's and listed none of the package's new API (M1). Eight Lows, all fixed, one of them reproduced on the
simulator before its fix (a Keychain key holding U+0000 listed truncated, L1) and one with a test that fails on the
original (a second, JavaScript `Connectivity` source where an app polyfills `navigator.onLine`, L2); the rest are
abort or leak paths on failures a test cannot force (out of memory, a skewed Java half: L3, L4, L6, L7), each fixed
with its exact code path stated, worker names lost on Android (L5, seen on the emulator) and a misleading comment (L8).
The `Device` store's `no_coalesce` is kept, for the playground's check only, and the recommendation attached to it is
corrected (I8).

**May a React Native team rely on the defaults?** Yes, within ADR-038's limits (one instance per process, React Native
0.87 proven). Two cautions beyond those: the CI jobs that would catch a regression on Linux and on a GitHub emulator
have not run on a runner (I10, I11), and the module's Android platform (`UndraPlatformAndroid.cpp`) is compiled only
by an app build (I9).

## Findings

| # | Sev | Where (at `b1b2eb0`) | Finding | Status |
|---|---|---|---|---|
| M1 | Medium | `docs/SPEC.md:780` (11.2 "Ports"), `:1316-1322` (17.4), `:740-747` (the port table of 11) | The binding spec said React Native's async ports are "answered by the registered `PortImpl`" (status 2 when none), had no React Native column in the port table, and listed `reactNativeAdapters()` as `{ lifecycle: AppState }`, `NativeTransport`'s options without `nativePorts` and the module without `platformDefaults`; `reactNativeHttp`, `isHttpUrl`, `nativePlatformDefaults` and `nativeDefaultPorts` (public exports) were missing. CLAUDE.md: code that disagrees with the SPEC is wrong; here the code was right and the SPEC stale. | **Fixed** (`f1f4a66`): 11.2 gains the default ports (callbacks queued per port, `Connectivity` after `undra_init`, the shared layouts, overrides, shutdown order); the table a React Native column; 17.4 the API. |
| L1 | Low | `ios/UndraPlatformApple.mm:138` | `Keychain::list` built each key with `UTF8String`, which ends at the first U+0000: a `SecureStore` key `"a\0b"` was listed as `"a"`, a key that does not exist, while the Swift adapter (and the Android store) list it whole. | **Fixed** (`1c9b5d2`): the account's UTF-8 bytes. RN12 round-trips such a key; on the iPhone 17 Pro simulator before the fix `RN12 FAIL ... got ["rn.checks.nul"]`, after it 19/19. |
| L2 | Low | `src/load.ts:111`, `:144` | `UndraCore.attach` merges `browserAdapters()` under `loadNative`'s adapters. An app that polyfills `navigator.onLine` and a global `addEventListener` (both absent from React Native 0.87, both common polyfill targets) got `browserConnectivity()` too: a second `Connectivity` source beside the native one, reporting `online` from the polyfill whatever the device said. | **Fixed** (`a1639db`): `null` for the JavaScript adapter of every port answered natively. Test "a native default is the only one" (fails on the original: one JavaScript `Connectivity` event). |
| L3 | Low (THEORY) | `cpp/UndraDefaults.cpp:196-201`, `:350-353` | `NativeDefaults::answer` is `noexcept` but built its fallback reply (`portReply`, a vector) and a log string inside its `catch (std::bad_alloc)` and `catch (...)` handlers: when the first allocation failed for lack of memory, the second throw is `std::terminate`, an abort on a port worker (R6). `startConnectivity`'s handlers logged the same way. Not forced by a test (it needs real exhaustion); the path is exact. | **Fixed** (`3c88f38`): the fallback reply is five bytes on the stack; logs go through `logQuietly`, which drops the record if it cannot be built. A test store that throws `std::bad_alloc` is answered "unavailable", logged, and the worker answers the next call. |
| L4 | Low (THEORY) | `cpp/UndraPlatformAndroid.cpp:195-198` | Four `GetStaticMethodID` calls in a row with no exception check between them: when the Java and C++ halves differ (the case the error message is written for), the first `NoSuchMethodError` is pending across the next three JNI calls, which the JNI spec forbids and CheckJNI aborts on, instead of reporting the friendly error. | **Fixed** (`3c88f38`): each lookup clears what it threw and keeps the first reason. The file compiles for Android with the NDK r27 and fbjni 0.7.0's headers under `-Werror` (I9). |
| L5 | Low | `cpp/UndraPlatformAndroid.cpp:387`; `cpp/UndraDefaults.cpp:136` | The workers are named "so a stack dump or a profiler shows which port it serves", but on Android `AttachCurrentThread` renamed all three to `undra-port-work` (seen in `/proc/<pid>/task/*/comm` on the emulator), and the `SecureStore` worker's 17-character name is refused by Linux and bionic (`ERANGE`, 15 characters at most). | **Fixed** (`3c88f38`): the worker passes its own name to the attach; `undra-secure`. The emulator now shows `undra-kv`, `undra-fs`, `undra-secure`. |
| L6 | Low (THEORY) | `android/.../NetworkMonitor.java:69` | `registerDefaultNetworkCallback` can refuse with other `RuntimeException`s than `SecurityException` (the per-app callback limit, `TooManyRequestsException`); the handler thread started just before was then never quit (one thread leaked per failed start, e.g. per reload). | **Fixed** (`3c88f38`): `RuntimeException`, logged, thread quit. |
| L7 | Low (THEORY) | `ios/UndraPlatformApple.mm:171-173` | `PathMonitor::start` returned `false` with `monitor_` set when only `dispatch_queue_create` failed; `stop()` would then `dispatch_sync` on a nil queue. | **Fixed** (`1c9b5d2`): both cleared on failure. |
| L8 | Low | `cpp/UndraHost.cpp:209` | The failed-`undra_init` path stopped the defaults with the comment "nothing was posted: no core asked"; a start-up hook (query hydration reads `Kv`) can post before `undra_init` fails. The code was right (stop joins); the comment invited removing it. | **Fixed** (`3c88f38`): the comment says why the join is needed. |
| I1 | Info | `cpp/UndraStores.cpp:694`, `:781`; Swift `KeyValueAdapters.swift:174-187` | A killed process leaves its temporary file (`<name>.<hex>.tmp` for `Kv`, `.undra-tmp-<hex>` for `Fs`); nothing removes it later. The C++ and Kotlin lists skip them, but the Swift `FileKeyValueBackend.list` reads every file of the directory, so a SwiftUI shell of the same app would list a leftover's key (twice, or after its deletion; from the code). | Open (Swift runtime; a sweep of stale temporaries at first use would also do) |
| I2 | Info | `cpp/UndraStores.cpp:155`, `:168` | `fsync`, not `F_FULLFSYNC`, on Apple platforms: the "old or new value" guarantee is for a killed process (proved), not for power loss on iOS. B5 claims only the former. | Note |
| I3 | Info | `FsRoot::read` | A hard link inside the root to a file outside is read (the core cannot create one; the Swift and JVM adapters behave the same). | Note |
| I4 | Info | `Fs` on the iOS simulator | The simulator's container is on the Mac's file system, case-insensitive by default: `A.txt` and `a.txt` are one file there and two on a device. `Kv` names are lowercase hex and unaffected. | Note |
| I5 | Info | `KvNaming::Fnv` | FNV-1a 64 + 32 (the Swift layout, kept for compatibility) is not collision-resistant against chosen keys: `get` and `delete` check the stored key, but `set` of a colliding key replaces the other key's entry, as Swift's does. Only matters for keys that embed attacker-chosen text. | Note |
| I6 | Info | `KvStore::list` | Byte (code-point) order; Kotlin sorts UTF-16 units and Swift by `String`'s ordering: they differ beyond the Basic Multilingual Plane and for keys not in NFC (B5 says so). | Note |
| I7 | Info | the core (`undra-ports`) | A `SecureStore` that answers "unavailable" (a tampered sealed value on the emulator, `AEADBadTagException`) surfaces as `UndraCallError.Panicked` with E0062 "the `SecureStore` port has no adapter registered (method `get`)": an adapter is registered and failed. The module's own log record says why; the core's wording does not. | Open (core) |
| I8 | Info | `examples/playground/core/src/platform.rs` | `no_coalesce` on the `Device` store: decided under attack 4 below. | Kept; the record's recommendation corrected in `REACT_NATIVE.md` |
| I9 | Info | `cpp/test/run.sh` | `UndraPlatformAndroid.cpp` is compiled by app builds only; fbjni's headers live in Gradle's cache, not in the repository or `node_modules`, so `run.sh` cannot check it as it checks `UndraJsi.cpp`. Compiled here by hand (NDK r27, `--target=aarch64-linux-android24`, `-Werror`), before and after the fixes. | Open (a CI step with the runner's NDK and fbjni from Maven would close it) |
| I10 | Info | `ci.yml` "React Native (host + model)" | Not run on Linux here (no container on this Mac): the new tests' `__linux__` branch (`renameat2` with `RENAME_EXCHANGE`) compiles against bionic, not against glibc 2.39; the job's other steps ran in a clean clone (below). | Open |
| I11 | Info | `rn-devices.yml` | Not run on a runner. Its Android image is pinned by API level, target and ABI (34, `google_apis`, x86_64), not by revision; `google_apis` images have `su`, and a missing `su` fails RN17 loudly (`android_phases`: "no root (su) on ..."), never a silent pass. | Open (integrator) |
| I12 | Info | iOS `SecureStore` | No live round trip through the Swift runtime and the C++ module in one app (the playground's React Native app has no Swift runtime). Compared at the byte level instead: the item the C++ wrote on the simulator is, in its `keychain-2-debug.db`, class `genp`, service SHA-1 `6bb9e111...` = `dev.undra.securestore`, account SHA-1 `a92459cc...` = `rn.checks.secret`, `pdmn` `cku` (`AfterFirstUnlockThisDeviceOnly`), `sync` 0, the app's default access group: the four attributes the Swift `KeychainBackend` sets (class, service, account, accessibility), nothing else set by either, so each finds the other's items. | Note |
| I13 | Info | `src/http.ts:14` | `isHttpUrl` is stricter than WHATWG in two corners: a space in the path and an empty port (`http://h:/`) are `InvalidUrl`. | Note |
| I14 | Info | `UndraPlatform.secret`, `AndroidSecureStoreAdapter.KeystoreKeySource` | The shared Keystore alias and lock file are a feature (either shell opens the other's secrets, by design). In the unsupported one-process case (a Kotlin host beside the React Native host) the two in-process locks differ and `FileChannel.lock` on one file twice in a JVM throws `OverlappingFileLockException`: the losing first call would be "unavailable" and the next succeed (THEORY, from the code). | Note (B9) |
| I15 | Info | `NativeDefaults::post`, `::kv` | A large value is copied three times on its way to disk (the job's copy of the arguments, `WireReader::bytes`, the entry). | Note |

## The attacks, one by one

**1. Memory and lifetime in the C++ ports: holds, now proved across a reload.** The workers answer with
`undra_port_reply` and the event source with `undra_event`; both copy what they are given and neither transfers
ownership (no `UndraBuf` is ever made by the defaults; the trampoline returns 1 and leaves `out_reply` untouched, 2
when the job cannot be queued). After `undra_shutdown`, `api::runtime()` is `None` and both are no-ops. The danger is
the next core: B3 orders shutdown, then `stop()` (event source stopped under `reportMutex_`, then each worker: queue
dropped, running job finished, thread joined), then `releaseSlot`. The existing test shut down mid-job on one host;
the new one ("a reload's start waits for the old host's port workers; their late reply reaches no core") starts the
new core on another thread while an old worker is blocked inside a job, checks the start waits, releases the job, and
checks every old worker ended before the new core existed and that the new core never logs a stray `port_reply`. With
`releaseSlot` moved before `defaults_->stop()` it fails at "the new core does not start while an old port worker is
still running a job", and with that check removed, at "no reply of the old core reached the new one" (the core's "no
port call N is pending"). Four 32 MiB `Kv` writes posted to the worker when the core is shut down: ASan clean, the one
running finishes, the rest are dropped, the worker is joined. **JNI:** each worker attaches once when it starts and
detaches when it ends (`workerStarted`/`workerEnded`, thread-local flag); the JS thread is never detached;
`JavaNetworkSource::stop` attaches only when it has to and detaches after. A Java exception is cleared at once
(`takeException`) and becomes "unavailable" plus a log record, never a pending exception: on the emulator a sealed
value with its last byte flipped (via `su`) gave `SecureStore.get failed: the Android Keystore failed for
'rn.checks.secret': javax.crypto.AEADBadTagException`, the core's call failed (I7), and RN12's seal and open on the
same worker thread passed right after. `resolveJava`'s lookups were the one place an exception could stay pending
(L4). **Connectivity after shutdown:** `stopped_` is set under `reportMutex_` before the source stops; Java's `stop()`
unregisters, quits and joins the handler thread, iOS's cancels and barriers its queue; only then is the `Report`
freed. No deadlock: `report` holds `reportMutex_` while `undra_event` runs subscribers under the core lock, and
nothing that holds the core lock takes `reportMutex_`. **Panics and aborts (R6):** L3.

**2. `Fs` and `Kv` security: holds.** Traversal: any `..` is `Denied` before any I/O (`splitPath`); a leading `/`, `.`
and empty components are ignored; NUL is `Io`; a 300-byte component is `Io` (`ENAMETOOLONG`); an 18 KB path works (the
descriptor walk has no `PATH_MAX`). **TOCTOU:** every component is `openat(..., O_NOFOLLOW)` from its parent's
descriptor; a read opens the last one `O_NOFOLLOW` too, and a write creates a temporary beside it with `O_EXCL |
O_NOFOLLOW` and `renameat`s it over the name (a link created after the `kindAt` check is replaced, never followed), so
check and use are one step. Proved by the swap race (`renamex_np(RENAME_SWAP)` on macOS, `renameat2(RENAME_EXCHANGE)`
on Linux): 12,080 swaps against 19,589 reads in the scratch run, 0 leaks, 0 writes outside; a walk that `lstat`s and
then opens without `O_NOFOLLOW` (which passes every static link test) leaked 11 reads. **Atomicity:** a child writing
4 MiB values in a loop, `SIGKILL`ed at 40 moments (3 to 9 per run with a temporary file pending): always the old or
the new value, `list` sees one key; an in-place writer fails it in 3 runs of 4. **Disk full** (a 4 MB HFS+ image):
`Kv.set` "No space left on device" (unavailable, logged), `Fs.write` `Io("No space left on device")`, the old values
intact, no temporary left. A directory where a file is expected and the reverse: `Io` or `NotFound` as documented.
Hard links: I3. Case: I4. **Names:** SHA-256 and the FNV pair against FIPS and the Swift runtime's own vectors; a
collision is never another key's value (`get`, `delete`; `set`: I5); a key with `/`, spaces, non-ASCII or 100 KB is
one file. **Cross-port:** each port has its own worker and directory, `Fs` cannot leave `undra/fs`, and on Android
`SecureStore`'s files are in `no_backup`, so no two workers touch one file.

**3. `SecureStore`: holds.** Android: `Cipher.init(ENCRYPT_MODE, key)` with no parameters, so the IV is the provider's
(the Keystore refuses a caller's IV, `setRandomizedEncryptionRequired` left on); the AAD is `undra.secure:<key>`, so a
file moved to another key fails; layout `01 | iv(12) | ct+tag`. The test vector, recomputed here with Node's
`aes-256-gcm`, is `PureTest`'s byte for byte; `android-adapters`' own `SecureSeal.open` (the Kotlin file, compiled with
`kotlinc` and a one-line `UndraException` stub) opens it, and the React Native Java opens a value the Kotlin
`SecureSeal.seal` made. The key is not bound to authentication, so removing the lock screen does not invalidate it
(`KeyPermanentlyInvalidatedException` cannot occur); a wiped Keystore makes a new key and old values fail
authentication ("unavailable") until replaced: the documented behaviour of `android-adapters`, with no rotation in
either. The alias shared with `android-adapters` is the point (B1; I14 for the unsupported one-process case). iOS: the
items are the Swift adapter's (I12); `kSecAttrSynchronizable` is never set, so items are non-synchronizable and queries
match only those, on both sides; L1.

**4. `Connectivity` and `Lifecycle`: holds; `no_coalesce` kept for the playground only.** "Initial state first": iOS's
monitor delivers the current path when it starts; Android posts `current()` after registering, and whatever the
registration's own callbacks report comes in order on the same handler thread, every later change with a callback, so
the last report is the current state. Before the first report the core assumes online (the query layer's default and
the `Device` store's), as on Swift. Dedupe compares with the last report *sent*, on one serial thread per platform:
offline, online, offline in one tick are three reports (the host test's source reports online at start, then offline
twice and cellular once: three reports reach the core, the repeat dropped). RN20 shows offline and online on the
emulator. `AppState`: `inactive` is surfaced (SPEC 8's `AppState` has it; on the simulator a cold start reports
`inactive` then `active`), and only a repeat of the same state is dropped. **`no_coalesce` on the `Device` store.** It
does not mask anything: the core received both reports at the time (its counter says so), and the mirror applies what
it queued when React Native lets its drain run again. What `no_coalesce` adds is that the mirror applies each queued
change-set instead of the last, so the playground's listener logs `background` after the app is back, which is what
RN19 checks. The cost is one change-set per report even when unobserved; these reports are a few a minute and
deduplicated at the source, so it is negligible. It is a public attribute used as documented (R10). It is not the
pattern to recommend for "the UI must see every transition": UI frameworks render the last value of a frame anyway
(SPEC 11.1), and a transition kept in the core (a counter, a bounded history) survives a paused drain whatever the
coalescing. The record's sentence suggesting the attribute for such UIs is therefore answered by a limit in
`REACT_NATIVE.md` that says what a paused drain does and where every transition belongs.

**5. The playground change (R10, R1, R12): holds.** `platform.rs` uses `#[undra::api]`, `#[undra::store]`,
`ctx.kv()`/`secure_store()`/`fs()`/`http()`, the generated `on_connectivity_changed`/`on_lifecycle_changed`,
`Runtime::extension` and an `InitHook` through `undra::runtime::inventory`: all public and in SPEC (16.2 lists
`InitHook` and `Runtime::extension`). The hook only subscribes; nothing reads a clock or randomness (R12). The schema
hash `0xefd907be3070520a` is the one the bindings, the reference pages and `schema_docs` carry (`bindgen --check
--docs` up to date, `schema_docs` passes, `build-all` changes nothing); the contract column is 54/54.

**6. CI honesty: holds where it can be checked.** RN17 without `su` is a FAIL line, never a pass (I11). The Linux job
runs `cpp/test/run.sh` (now 15 + 22 + 22 checks), `android/test/run.sh` with `setup-java` 17 (the JDK is needed now; the
runner image has one anyway), `npm test`, typecheck and the contract column. A clean clone of this branch ran the job's
steps on this Mac (below; `apt`, `clang++-18` and glibc not, I10).

**7. Everything else.** Docs on every new public item (TypeScript exports, C++ headers, the public Java classes);
`REACT_NATIVE.md`'s port table matches the code (directories, names, service, alias, accessibility, `inactive` on iOS
only). The Swift `FsAdapter` finding is recorded with its exact failing case in the record (root `<tmp>/root`, link
`out -> <tmp>/outside`, `read("out/secret.txt")` returns `outside`, `write("out/new.txt")` creates
`<tmp>/outside/new.txt`); `FsAdapter.resolve` (`runtimes/swift/.../Adapters/FsAdapter.swift:66-78`) refuses only `..`,
and `read`/`write` go through `Data(contentsOf:)`/`Data.write`, which follow links. Reproduced here with those 13
lines compiled verbatim into a scratch `swiftc` program (Xcode 26.6): `resolve("out/secret.txt")` is not refused,
`Data(contentsOf:)` reads `outside`, and an atomic write to `out/new.txt` lands in `<tmp>/outside`. Not fixed here (the
Swift runtime is not this piece's).

## Gaps (recorded, not fixed)

* I1 (the Swift runtime's list and stale temporaries), I7 (the core's wording), I9 to I11 (CI that has not run).
* A live Swift-to-C++ Keychain round trip (I12) needs an app with both runtimes; the attribute comparison stands in.
* The JNI exception path was shown on the emulator with a temporary startup read of the secret, built into an APK for
  the experiment and not committed; a permanent device check (tamper, relaunch, expect "unavailable" and a working next
  call) would keep it shown.

## Runs after the fixes

At `f1f4a66` (before merging `main`): `cpp/test/run.sh` 15 (stores) + 22 + 22 (host, both shims), the JSI and Apple
compile checks; `android/test/run.sh` 4; `npm test` 60, typecheck clean, `test:contract` 17 pass + S17 skipped;
playground app `tsc` clean; the Android C++ under the NDK with `-Werror`. Devices (release core, Release app):

```
iPhone 17 Pro simulator (iOS 26.5), scripts/rn-device-checks.sh ios
UNDRA-RN CHECK RN12 PASS SecureStore default: ... stored and read back (marker nonce muq1pe738mhqn); Keychain service dev.undra.securestore
UNDRA-RN CHECK RN17 PASS SecureStore is not plain text in the app's files: UNDRA-SECRET-muq1pe738mhqn in no file; the Kv marker next to it in 1 (the scan sees the files)
UNDRA-RN CHECK RN18 PASS Kv survives the process: the first launch wrote muq1pe738mhqn; after the kill the second read muq1pe738mhqn
UNDRA-RN CHECK RN19 PASS Lifecycle reaches the core: Settings in front: 'UNDRA-RN LIFECYCLE state=background reports=4'; back: 'UNDRA-RN LIFECYCLE state=active reports=5'
== UNDRA-RN CHECKS 19/19 passed

undra-rn emulator (arm64, API 35, emulator-5556), scripts/rn-device-checks.sh android
UNDRA-RN CHECK RN12 PASS SecureStore default: ... Android Keystore key dev.undra.securestore, sealed files in /data/user/0/dev.undra.playground.rn/no_backup/undra/secure
UNDRA-RN CHECK RN17 PASS SecureStore is not plain text in the app's files: UNDRA-SECRET-muq1qcb245ci6 in no file; the Kv marker next to it in 1 (the scan sees the files)
UNDRA-RN CHECK RN18 PASS Kv survives the process: the first launch wrote muq1qcb245ci6; after force-stop the second read muq1qcb245ci6
UNDRA-RN CHECK RN19 PASS Lifecycle reaches the core: Home: 'UNDRA-RN LIFECYCLE state=background reports=3'; back: 'UNDRA-RN LIFECYCLE state=active reports=4'
UNDRA-RN CHECK RN20 PASS Connectivity follows airplane mode: on: 'UNDRA-RN CONNECTIVITY online=false kind=none reports=2'; off: 'UNDRA-RN CONNECTIVITY online=true kind=cellular reports=3'
== UNDRA-RN CHECKS 20/20 passed
```

After merging `main` (`8b6fa73`: dev-reload, which brings `@undra/runtime`'s `onDevNotice`, the transport's suspend
and the CLI's reload): `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo
test --workspace --no-fail-fast` 2,607 passed, 2 failed, 11 ignored (2,609 = `main`'s 2,603 + the playground's 6
`platform` tests). The two failures are `main`'s `crates/undra-cli/tests/dev_reload.rs`
(`no_keep_state_starts_every_rebuilt_core_fresh` and
`a_change_during_a_reload_is_built_next_and_the_state_survives_both_swaps`, both at `dev_reload.rs:145`, "a Close
frame": the connection dropped without one) while two device builds ran beside it, load average about 20; an earlier
full run failed a third test of that file at `:172` the same way. Run alone the file passed 8/8 twice (a third run
alone, at load average about 20, failed three of its tests the same way); none of its code is this piece's: an
integrator's note on that file's sensitivity to a loaded machine, not a finding here. `contract-tests/run-all.sh` 18 x
3 = 54/54. `cpp/test/run.sh` 15 + 22 + 22 and both compile checks; `android/test/run.sh` 4; `npm test` 60, typecheck
clean (against the runtime's rebuilt declarations), `test:contract` 17 + S17 skipped; the app's `tsc` clean; `undra
bindgen -C examples/playground --check --docs` up to date (`0xefd907be3070520a`); `cargo test -p undra-cli --test
schema_docs -- --ignored` 1 passed; `node site/scripts/build-all.mjs` changes nothing; `check-links
--words` clean (landing prose 342 words).

A clean clone of `8b6fa73` ran `ci.yml`'s React Native job's steps on this Mac (Node 20 not pinned here: Node 24;
Apple clang, not `clang++-18`; no `apt`): the CLI, the host and web cores, `npm ci` in the runtime (then `npm run
build`), the package and the playground app (`--ignore-scripts`), `run.sh` with `UNDRA_RN_REQUIRE_JSI=1` (15 + 22 +
22, both compile checks), `android/test/run.sh` (4), `npm test` (60), typecheck, `test:contract` (17 + S17 skipped):
all green.

Devices again after the merge, release core, Release app, full builds:

```
iPhone 17 Pro simulator: scripts/rn-device-checks.sh ios
UNDRA-RN CHECK RN17 PASS SecureStore is not plain text in the app's files: UNDRA-SECRET-muq223cp3rdks in no file; the Kv marker next to it in 1 (the scan sees the files)
UNDRA-RN CHECK RN18 PASS Kv survives the process: the first launch wrote muq223cp3rdks; after the kill the second read muq223cp3rdks
UNDRA-RN CHECK RN19 PASS Lifecycle reaches the core: Settings in front: 'UNDRA-RN LIFECYCLE state=background reports=4'; back: 'UNDRA-RN LIFECYCLE state=active reports=5'
== UNDRA-RN CHECKS 19/19 passed

undra-rn emulator (emulator-5556): scripts/rn-device-checks.sh android
UNDRA-RN CHECK RN17 PASS SecureStore is not plain text in the app's files: UNDRA-SECRET-muq22ssk28dnl in no file; the Kv marker next to it in 1 (the scan sees the files)
UNDRA-RN CHECK RN18 PASS Kv survives the process: the first launch wrote muq22ssk28dnl; after force-stop the second read muq22ssk28dnl
UNDRA-RN CHECK RN19 PASS Lifecycle reaches the core: Home: 'UNDRA-RN LIFECYCLE state=background reports=2'; back: 'UNDRA-RN LIFECYCLE state=active reports=3'
UNDRA-RN CHECK RN20 PASS Connectivity follows airplane mode: on: 'UNDRA-RN CONNECTIVITY online=false kind=none reports=2'; off: 'UNDRA-RN CONNECTIVITY online=true kind=cellular reports=3'
== UNDRA-RN CHECKS 20/20 passed
```

The `undra-rn` emulator was shut down afterwards (airplane mode off); `emulator-5554` was not touched.
