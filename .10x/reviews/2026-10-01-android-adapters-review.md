# Android platform adapters - adversarial security review

**Date:** 2026-10-01 · **Reviewer:** security-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/android-adapters` at `5c93d45` (10 commits on `main` `6ab7b8c`) · **Read:** `CLAUDE.md` (R6, R12, the Kotlin rule),
`.10x/decisions/sde/android-adapters.md`, `android-adapters/README.md`, `docs/SPEC.md` sections 8 and 11, ADR-025, the
Swift adapters (`KeyValueAdapters`, `FsAdapter`, `HttpAdapter`, `ConnectivityAdapter`, `PlatformDefaults`), the web
adapters (`secure.ts`, `kv.ts`, `fs.ts`, `http.ts`), the JVM `FileAdapters.kt`, `HttpAdapter.kt`, `PortRegistry.kt`,
`SimplePorts.kt`, every file under `android-adapters/src/{main,sharedTest,test,androidTest}`, the playground changes
(`UndraApp.kt`, `remote/DemoServer.kt`, the manifest and `network_security_config.xml`, `smoke.sh`), the CLI template and
`adopt` text, and the `wt/dev-loop` debug manifests for the merge interaction · **Fixes:** `8b504ee`.

## Verdict

**These adapters may hold production secrets.** The SecureStore is AES-256-GCM under a non-extractable `AndroidKeyStore`
key (256 bits, GCM, no padding, encrypt+decrypt, the provider's randomised 12-byte IV per encryption, a 128-bit tag), the
key name is authenticated data, the sealed layout is byte-for-byte the web adapter's (now pinned by a vector sealed under
Node's WebCrypto, `SecureSealTest`), the files are 0600 under `noBackupFilesDir` (excluded from Auto Backup and
device-to-device transfer without any manifest rule), written atomically (proven under SIGKILL from another process),
and every failure (no Keystore, a lost key, a tampered or moved file) is a typed `SecureStoreException` that the runtime
answers as `PortError::Unavailable`; the on-device scan finds the plaintext in no file of the app and, now, in no line of
its logcat, including after a failing read. The Http adapter never touches TLS defaults, sets no cleartext flag in the
library manifest, refuses an `https` to `http` redirect, drops `Authorization`, `Cookie` and `Proxy-Authorization` on
any change of origin, enforces the 64 MiB cap while streaming, bounds the whole exchange with `withTimeoutOrNull` and
closes the socket from the cancelling thread (the attach/abort pair is a correct volatile handshake). No High finding.
One Medium was real and is fixed: the Fs root guard could be bypassed with a leading backslash (`delete("\\")` removed
the root and everything under it, because `FsAdapter.resolve` ignores leading `\` and the guard did not), and two Low
Fs findings next to it (a symbolic link to a directory inside the root was emptied on delete; a NUL byte in a path
escaped as an untyped exception). The runtime follow-ups the implementer named stand (non-recursive JVM `FsAdapter.delete`,
`FileKv.list` reading whole files: measured at 22-34 ms for 1,000 x 10 KB on the emulator, not a finding for this piece).

## Findings

| # | Sev | Where (at `5c93d45`) | Finding | Status |
|---|---|---|---|---|
| M1 | Medium | `AndroidFsAdapter.kt:62,129` (`delete`, `segments`) | **Root guard bypass.** `delete` refuses the root when `segments(path)` is empty, splitting on `/` only. The runtime's `FsAdapter.resolve` (`FileAdapters.kt:233`) trims leading `/` **and** `\`, so `"\\"`, `"\\."`, `"/\\"` resolve to the root but passed the guard: `deleteTree` listed the root, deleted every entry, then `Files.delete(root)` removed the root itself (a new test deleted it on the JVM and on the device). The core can delete every entry one by one anyway, so the damage is "the sandbox directory is gone until the next write", but the guarantee Swift and web give ("the root cannot be deleted", `Denied`) was false. | **Fixed**: `segments` trims the same separators as `resolve`; `the_root_cannot_be_deleted` covers the five backslash forms |
| L1 | Low | `AndroidFsAdapter.kt:109` (`deleteTree`) | **A symbolic link was followed on delete.** `fs.list(link)` lists through a link that stays inside the root (allowed), so `deleteTree` removed the *target's* children before unlinking: `delete("link")` emptied `target/`. `rm`, Swift's `removeItem` and OPFS (no links) remove the link alone. Only code with file-system access to the app's data directory can create the link, so no privilege is gained; the semantic was wrong. | **Fixed**: a link is `lstat`-checked and not listed; test `deleting_a_symbolic_link_to_a_directory_removes_the_link_and_not_what_it_points_at` (fails on the old adapter) |
| L2 | Low | `AndroidFsAdapter.kt:132`, `FileAdapters.kt:205-222` (`io`) | **NUL byte in a path escaped untyped.** `Path.resolve` throws `InvalidPathException` (a `RuntimeException`) which `FsAdapter.io` does not map; the port answered `Unavailable` (the runtime catches `Throwable`, so no crash: R6 holds) instead of a typed `FsError`. Web answers `Io` for a name OPFS refuses. | **Fixed** in `confined` (`FsError.Io`); the runtime's `io()` should map `InvalidPathException` too (follow-up F3) |
| L3 | Low | `SecureSealTest.kt` | The layout and AAD parity with `secure.ts` was documented and tested structurally but no vector crossed the two implementations. | **Fixed**: a value sealed by WebCrypto (fixed key `00..1f`, IV `a0..ab`, name `session.token`) opens with `SecureSeal.open` and fails under another name |
| L4 | Low | `SecureStoreOnDeviceTest.kt` | The plaintext scan covered files and `SharedPreferences`, not the log. By inspection no adapter logs a value (`AndroidLogAdapter` writes what the core sends; the adapters' own `Log.w` calls carry exception messages that name keys and paths only; the runtime logs a failing port method with its exception, never its arguments), but nothing pinned it. | **Fixed**: `the_secret_is_in_no_log_line_after_a_write_a_read_and_a_failing_read` reads the process's own logcat (`logcat -d --pid`), after a `set`, a `get` and a `get` of a tampered file whose exception is logged the way the runtime logs it |
| L5 | Low | `AndroidSecureStoreAdapter.kt:132-137` (KDoc) | The `KeyGenParameterSpec` is right (purposes encrypt+decrypt, `BLOCK_MODE_GCM`, `ENCRYPTION_PADDING_NONE`, 256 bits, randomised encryption left on, so the Keystore draws every IV and refuses a supplied one) but the two things it deliberately does not set, `setUserAuthenticationRequired` and `setUnlockedDeviceRequired`, were not justified, and what happens when the key is gone was not stated. Justification: the offline queue replays and queries refetch with nobody there to authenticate, which is the Swift adapter's `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly` (and the files are in credential-encrypted storage, so "after first unlock" holds on Android too). `KeyPermanentlyInvalidatedException` cannot occur without authentication binding; an OS upgrade or lock-screen change keeps the key; only a wiped Keystore loses it, after which a fresh key is generated and every old value fails authentication (typed, never "missing") until the app replaces or deletes it. | **Fixed** (KDoc) |
| L6 | Low | `AndroidHttpAdapter.kt:95,146` (KDoc) | `timeoutMs` is enforced by `withTimeoutOrNull` plus `disconnect()`, which ends a blocked connect or read at once, but cannot interrupt a name lookup: the `coroutineScope` waits for the IO worker, so a hanging resolver stretches `Timeout` to the resolver's own give-up time. Each in-flight request also holds one `Dispatchers.IO` thread, shared with Kv, Fs and SecureStore (64 by default); `Dispatchers.IO.limitedParallelism` would give Http its own slice, but it is experimental in coroutines 1.6.4 and `allWarningsAsErrors` is on. | **Fixed** (KDoc); the slice is follow-up F5 |
| L7 | Low | `AndroidPlatformDefaults.kt:38-41` (KDoc) | After `AndroidPlatform.close()` the Timer port stays registered over a shut-down executor: `set` throws `RejectedExecutionException`, which the registry answers `Unavailable` (logged). Correct for "the app is closing", undocumented. | **Fixed** (KDoc) |
| L8 | Low | `examples/playground/.../DemoServer.kt:158` | The loopback demo server allocated `ByteArray(Content-Length)` unbounded; the loopback interface is reachable by every app on the device. A demo, not a library, but a 2 GB header would kill the playground. | **Fixed**: bodies over 1 MiB are refused |
| I1 | Info | `AndroidPlatformDefaults.kt:85,109,115` | `installed` is a `WeakHashMap<UndraCore, AndroidPlatform>` whose value references the key (`TimerAdapter(core::timerFired)`, `ConnectivityEvents(core)` in the listeners), so an entry never clears: every core ever installed on stays reachable. One core per production process, so nothing leaks in an app; tests that install on many `RecordingCore`s keep them all. A `WeakReference` to the core inside the event sources, or keying the registry on the core's own port table, would free it. | Open (note, F6) |
| I2 | Info | `FileAdapters.kt:67-84` (`FileKv.list`) | The implementer's note, quantified: 1,000 entries x 10 KB (10.3 MB on disk) list in **22-34 ms warm on the `undra` AVD** (API 35, host SSD; 15-29 ms on the Mac's JVM, where a header-only read is no faster because `open` dominates). The cost is linear in the store's bytes; at 10,000 x 10 KB it is ~250 ms and 100 MB of transient allocation at every launch, on a background thread. Not a finding for this piece (the runtime is untouched by design); Swift's `readKey` reads 4 + key bytes through a mapped file. | Follow-up F2 |
| I3 | Info | `FileAdapters.kt:274-293` (`atomicWrite`) | Atomic under SIGKILL (the on-device test kills a writer in a loop and reads a whole value), not under power loss: no `fsync` before the rename, so ext4/f2fs may persist the rename before the data. Same as Swift's `Data.write(.atomic)`; a torn secure file fails authentication (typed), a torn Kv file fails `readEntry` and reads as missing. | Open (note) |
| I4 | Info | `SecureStoreOnDeviceTest.kt:150-153` | A write interrupted by a kill leaves its `.tmp` sibling; `list` and `get` ignore it, nothing sweeps it. One file per kill, bounded by kills. | Open (note, runtime) |
| I5 | Info | `AndroidKvAdapter.kt:18-20`, template manifest `allowBackup="true"` | Kv under `filesDir` is in Auto Backup by design (the persisted cache and the **offline queue**, which may carry request bodies and headers of queued mutations). Android's cloud backup is end-to-end encrypted with the lock screen since 9; `noBackupFilesDir` needs no `dataExtractionRules` entry, it is excluded from both cloud backup and device-to-device transfer by default. Parity: Swift keeps Kv in Application Support, which iCloud backs up. If a core ever queues a bearer header, that is the core's problem on every platform. | Open (note) |
| I6 | Info | `HttpRules.kt:141,152` | Redirect policy is stricter than curl and `java.net.http` in one place: an `http` to `https` hop on the same host is another origin, so credentials are dropped (fetch does the same). `307`/`308` to another origin forward the body (standard). `Location` with userinfo (`https://x@evil/`) is parsed by `URL`, the origin is the `host`, and `HttpURLConnection` never sends userinfo. | OK |
| I7 | Info | `AndroidHttpAdapter.kt:235,290` | Bodies of at most 256 KiB are buffered by the connection, and Android's `HttpURLConnection` (OkHttp) retries a buffered request once on a stale pooled connection. The server never saw the first attempt in that case; the core's idempotency keys cover the rest. Larger bodies use fixed-length streaming (no retry). | OK |
| I8 | Info | `AndroidConnectivityAdapter.kt:154,185-199` | `close()` resets `tracked`/`last` from the caller's thread while the handler thread may still deliver one queued report; the report reaches `core.event`, which throws if the core is closed and is caught and logged. `install` twice stops the earlier sources through the registry (`PlatformDefaultsOnDeviceTest`); timers armed through the earlier adapter keep firing (documented). | OK |
| I9 | Info | `AndroidLifecycleAdapter.kt:181` | The first report (`listener(initial)`) runs on the thread that called `install` (the main thread in `Application.onCreate`), the rest on the main looper; the core's `event` takes the core lock briefly. Swift reports `Lifecycle` from `scenePhase` on the main actor too. | OK |
| I10 | Info | playground `AndroidManifest.xml` + `wt/dev-loop` `src/debug/AndroidManifest.xml` | **Merge interaction.** This branch gives the playground `android:networkSecurityConfig` (cleartext to `127.0.0.1`/`localhost` only); `wt/dev-loop` adds a debug manifest with `android:usesCleartextTraffic="true"`. On API 24+ the attribute is ignored once a network security config exists, so after both merge the debug flag is a no-op for the playground's `HttpURLConnection` traffic. Nothing breaks: the dev-loop WebSocket client is a plain `java.net.Socket` (`WebSocketClient.kt:395-413`), which the platform does not police, so `ws://10.0.2.2` still connects. The template has no config, so its debug flag keeps working. `UndraApp.kt` needs the hand merge the SDE record names. | Note for the integrator |
| I11 | Info | `AndroidSecureStoreAdapter.kt:131` | `KeyStore.getKey` can throw `UnrecoverableKeyException` for a key blob the Keystore holds but cannot use; the adapter then fails every call (typed) with no port-level way to recover, since only `deleteEntry(alias)` would clear it. Deleting on that exception would turn a transient Keystore outage into a wipe of every token, so the review leaves it; an app can delete the alias and sign in again. | Open (note) |

## Follow-ups for the integrator (none blocks the merge)

* **F1** (`runtime`): `FsAdapter.delete` on the JVM is not recursive (`FileAdapters.kt:162`, `DirectoryNotEmptyException` becomes `Io`); Swift, web and now Android are. Align, and say so in SPEC section 8 (the implementer's finding, confirmed).
* **F2** (`runtime`): `FileKv.list` reads whole files to recover keys (I2); a header-only read of `4 + key` bytes makes it linear in the number of entries. Numbers above.
* **F3** (`runtime`): `FsAdapter.io` should map `java.nio.file.InvalidPathException` to `FsError.Io` (L2 is fixed in the Android wrapper only).
* **F4** (`runtime`, optional): sweep `*.tmp` files in `FileKv.list` (I4).
* **F5** (`android-adapters`, when coroutines is bumped past 1.6.4): run `AndroidHttpAdapter` on `Dispatchers.IO.limitedParallelism(n)` so a stalled server cannot hold the threads Kv and SecureStore need (L6).
* **F6** (`android-adapters`): make the `installed` registry not retain cores (I1).
* **F7** (`wt/dev-loop` merge): hand-merge `UndraApp.kt`; the debug cleartext flag is inert for the playground (I10).

## The attacks

**1. SecureStore crypto.** `KeyGenParameterSpec` at `AndroidSecureStoreAdapter.kt:132-136`: `PURPOSE_ENCRYPT or PURPOSE_DECRYPT`,
`BLOCK_MODE_GCM`, `ENCRYPTION_PADDING_NONE`, `setKeySize(256)`; `setRandomizedEncryptionRequired` is left at its default
(`true`), so `Cipher.init(ENCRYPT_MODE, key)` without parameters (`SecureSeal.kt:37`) makes the Keystore draw the IV and a
caller-supplied IV would be refused: the nonce is 12 random bytes per encryption, never reused, stored at offset 1
(`SecureSeal.kt:42-45`), length-checked (`:41`). Tag length is 128 on decrypt (`GCMParameterSpec(128, ...)`, `:62`) and the
Keystore's default on encrypt; the sealed length test (`1 + 12 + n + 16`) pins it. AAD is `undra.secure:<key>` (`:31`),
identical to `secure.ts:90`, and the layout `format=1, iv[12], ciphertext||tag` to `secure.ts:115-118`; the WebCrypto vector
(L3) opens. The software-key test path and the Keystore path share `SecureSeal`, so the on-device round trip exercises the
same bytes. The key is non-extractable (`key.encoded == null` on device, `KeyInfo.keySize == 256`). Not bound to user
authentication or an unlocked device: justified in L5. `KeyPermanentlyInvalidatedException` is unreachable without
authentication binding; a wiped Keystore gives `getKey == null` (`:131`), a new key, and every old file fails
authentication: `SecureStoreException` (`:65-74`), answered `Unavailable`, never `null` (test
`an_adapter_with_another_key_cannot_read_it_and_says_so_instead_of_returning_null`, and on device with two aliases). Files:
`FileKv.atomicWrite(ownerOnly = true)` writes a UUID-named temp, chmods it `rw-------`, then `ATOMIC_MOVE` (Android's app
umask is 077, so the window before the chmod is 0600 anyway); directory `noBackupFilesDir/undra/secure`
(`PlatformLocationsTest`, and `the_sealed_file_is_in_the_no_backup_directory_...`). `allowBackup="true"` with no
`dataExtractionRules` in the template is fine: `getNoBackupFilesDir()` is excluded from Auto Backup and device-to-device
transfer by default. Key creation is serialised in-process (`PROCESS_LOCK`) and across processes (`.keystore.lock`
`FileChannel.lock()`), and the `:writer` process test proves another process reads what one wrote. Logs: L4.

**2. Fs sandbox.** `..` is refused lexically in `confined` before any I/O (`a_path_with_a_parent_component_is_denied_everywhere`,
six forms, read/write/delete/list), then `FsAdapter.resolve` normalises, checks `startsWith(root)`, walks to the nearest
existing ancestor and compares `toRealPath()` against the root's real path (`FileAdapters.kt:232-249`): a link out of the
root is `Denied` whether it is the leaf, a parent, or dangling (`a_symbolic_link_out_of_the_root_is_not_followed`, on the
device too, where `/data/data` is itself a link to `/data/user/0` and both sides are canonicalised). Absolute paths lose
their leading separators (`"/a.txt"` is `a.txt`). Case: ext4/f2fs on `/data` are case-sensitive, nothing to normalise.
Overlong names and a full disk: `IOException` to `Io` with the platform's text, the temp file deleted in `finally`.
Concurrent writers: each write has its own UUID temp and an atomic rename; the last rename wins whole. Root deletion: M1;
links: L1; NUL: L2. TOCTOU between the link check and the operation needs a writer inside the app's private directory,
which already has everything.

**3. Http.** Redirects (`Redirects.follow`): `https` to `http` is not followed (`HttpRules.kt:141`; the caller gets the 3xx),
`http` to `https` is; any change of scheme, host (case-insensitive) or effective port drops `Authorization`, `Cookie` and
`Proxy-Authorization` (`:120,152`; `a_redirect_to_another_host_does_not_carry_credentials` over a real socket, plus the
unit matrix); `301`/`302` turn `POST` into `GET` and drop the body headers, `303` all but `HEAD`, `307`/`308` repeat; 20 hops
then `Network("too many redirects")` (21 requests observed). Schemes other than http(s) in `Location` and a missing host
make the 3xx the answer. Timeout: `withTimeoutOrNull(timeoutMs)` wraps connect, send, every hop and the body read
(`AndroidHttpAdapter.kt:95`); its cancellation reaches `work.await()`, `abort()` sets `aborted` then `disconnect()`s, and
`attach` publishes the connection before re-checking `aborted` (`:165-173`), so an abort that lands between `openConnection`
and `attach` still disconnects: a correct Dekker pair on two volatiles. The socket timeouts are set to `timeoutMs` as well
(`:221-222`), so even the blocking layer agrees. Three tests show the typed `Timeout` at 300-500 ms for a server that never
answers, one that stalls mid-body, and one that trickles forever; `a_timeout_aborts_the_connection_too` sees the server's
socket close. The limit, L6: a name lookup. Cap: declared `Content-Length` over the cap fails before the first read; a chunked
body fails at the first chunk that crosses it (`readBounded`, `:262-276`, 8 KiB chunks, so at most cap + 8 KiB is ever held).
Cancellation from another thread: `disconnect()` from the cancelling thread while the IO thread is in `read()`; four tests
(hung headers, mid-body, before start, repeated cancellations then a normal request). The worker reports failure as a value
(`Outcome.Failed`) so the `IOException` the abort causes never replaces the caller's `CancellationException`. Keep-alive:
`useCaches = false`, `instanceFollowRedirects = false`, a fully read body is closed and returned to the pool; a hop or a
failure is `disconnect()`ed (`:207`). Cleartext: no `usesCleartextTraffic` anywhere in the library (`grep`), no
`HostnameVerifier`, `TrustManager`, `SSLSocketFactory`, `CookieHandler` or `Authenticator`; the test APK's manifest sets the
flag for itself only. Errors: `SocketTimeoutException` before `InterruptedIOException` (it extends it), `MalformedURL` and
`IllegalArgumentException` (a header Android's `Headers.Builder` refuses, e.g. a line break: tested) to `InvalidUrl`, the rest
`Network` with the `INTERNET`-permission hint on `EPERM`. `GET`/`HEAD` with a body are refused before anything is sent
(`HttpURLConnection` would silently turn them into `POST`). PATCH: the desktop JVM's `HttpURLConnection` cannot send it;
the test is `assumeTrue(isAndroidRuntime)` and passes on the device.

**4. Connectivity and Lifecycle.** `start()` calls `close()` first, so re-registration never stacks callbacks or threads; the
`HandlerThread` is quit on close and on a `SecurityException` from registration. `install` twice: the `installed` registry
hands the previous `AndroidPlatform` to `stopEventSources()` (`PlatformDefaultsOnDeviceTest.installing_again_...`), and the
fix commit's claim holds: `AndroidLifecycleAdapter.close()` disposes the state machine, which cancels its pending
`postDelayed` (`ProcessStateMachine.dispose`, `AndroidLifecycleAdapter.kt:200`), so a 700 ms downgrade armed by the old
adapter cannot fire into the core after the new one reported. Events are posted from the `undra-connectivity` handler thread
and the main looper, under no lock of the adapter; `core.event` takes the core's own lock inside the native call, as every
host event does. The initial Connectivity state is read on the handler thread after registration (`:169`), so it is never
older than a callback that already ran; identical states are reported once; a default-network switch keeps `tracked` on the
new network so a late `onLost` for the old one is not "offline". `requireValidatedNetwork` is documented in the README, the
KDoc and the SDE record with the reason the default is `NET_CAPABILITY_INTERNET`. The network-toggle test is gated
(`assumeTrue(undra.networkToggle == "true")`) and did not run on the shared AVD.

**5. Kv.** `a_kv_write_interrupted_by_killing_the_process_leaves_a_whole_value`: a service in `:writer` loops `set` of a
40-byte-pattern value, the test SIGKILLs it after 30 writes, then reads one whole value of one pattern and lists exactly
the one key; the leftover temp is counted, not removed (I4). Durability: I3. `list` cost: I2.

**6. Permissions and manifests.** Library manifest: `INTERNET` and `ACCESS_NETWORK_STATE`, nothing else; the merged playground
manifest shows exactly those two from the app and the library plus AndroidX's own (`DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION`,
not from this piece). No `usesCleartextTraffic` in the library, the playground's main manifest or the template; the playground
limits cleartext to its own loopback server through a network security config. Dev-loop interaction: I10.

**7. Threading.** Async port methods run on the registry's `Dispatchers.Default` scope (`PortRegistry.runAsync`), never on the
caller; inside them `FileKv`/`FsAdapter` use `withContext(Dispatchers.IO)` for every file operation, `AndroidHttpAdapter`
runs the exchange in `async(Dispatchers.IO)`, and the fix commit's claim holds: `keys.secret()` (the Keystore round trip and
the file lock) and `SecureSeal.seal/open` (the Keystore cipher) are inside `withContext(Dispatchers.IO)`
(`AndroidSecureStoreAdapter.kt:68,76`); only the constructors and `install` run on the caller (no I/O: `getSystemService`,
`registerActivityLifecycleCallbacks`, `registerDefaultNetworkCallback`, `getMyMemoryState`, one `core.event`).
`HttpOnDeviceTest` asserts the request runs off the main thread. The sync ports (`Log`, `Clock`, `Rng`, `Timer`) run inline
and do no I/O beyond `Log.println`.

**8. Parity.** Kv: the file *content* (`u32 len, key, value`) matches Swift; file *names* differ (SHA-256 hex vs
`fnv1a64-fnv1a32`), which is harmless since files never cross platforms, and Swift guards a hash collision on delete where
Kotlin trusts SHA-256; `list` is sorted and prefix-filtered on all three; Swift reads only the key (I2). SecureStore: the
Swift Keychain item has no AAD and no layout (the Keychain seals it), web and Android share layout and AAD exactly; a
failing read is an error on all three, never `null`. Fs: `..` denied on all three; Swift does *not* check symbolic links
(it follows them), Android and the JVM do: stricter; `delete` recursive on Swift, web and Android, non-recursive on the JVM
(F1); root deletion `Denied` on Swift and Android, `Io("the path is empty")` on web. Http: `https` to `http` refused on
Android and the JVM, followed by URLSession (ATS then blocks the `http` side) and by fetch; the 64 MiB cap exists on Android
only (Swift and web hold whatever arrives); typed errors are the ADR-025 set on all four. Connectivity: `NET_CAPABILITY_INTERNET`
is `NWPath.satisfied`, both "a route exists". Lifecycle: Swift's `platformDefault` leaves it to the app (`UndraLifecycle`),
Android's `install` includes it. The one-call install mirrors `Adapters.platformDefault` and adds the two event sources.

## Verification after the fixes (`8b504ee`)

| Check | Result |
|---|---|
| `./gradlew :android-adapters:test` | 131 tests per variant (debug and release), 130 pass, 1 skipped (PATCH on the desktop JVM); was 127 + 1 skipped |
| `./gradlew :android-adapters:connectedAndroidTest` on `undra` (`emulator-5554`, API 35) | 113 tests, 112 pass, 0 fail, 1 skipped (the gated network toggle); was 109 + 1 skipped. New: 3 Fs, 1 logcat scan |
| the three new Fs tests against the adapter at `5c93d45` | all three fail (root deleted through `\`, target emptied through the link, `InvalidPathException` escapes) |
| `runtimes/kotlin/undra-runtime/scripts/test-local.sh` | 500 cases in 31 suites, 0 failed, 2 skipped (JNI smoke, no native library) |
| `examples/playground/android`: `./gradlew :app:assembleDebug` | `BUILD SUCCESSFUL`, 13 MB debug APK |
| `bash contract-tests/run-all.sh` | 54/54 (18 scenarios x ts, kotlin, swift) |
| `FileKv.list` timing, 1,000 x 10 KB, temporary instrumented test on the `undra` AVD (not committed) | writes 762 ms; list 34.3, 25.7, 24.3, 24.1, 22.4 ms |

`smoke.sh` was not re-run: it toggles the device's airplane mode and the shared AVD must not be disturbed; the fixes touch
neither the playground's flow nor the adapters it exercises beyond the Fs guard. No private AVD was created.
