# The dev loop: `undra dev`

Edit Rust, save, and the app on every platform is on the new core within a second, **on the screen it was on, with
the state it had**. A dropped connection heals itself: no restart, no relaunch. This page is what `undra dev` does
per platform, which URL each platform uses, how state is carried across a rebuild, how reconnecting works, and what
to check when it does not.

The designs are [ADR-051](../.10x/adrs/ADR-051-dev-client-reconnect-and-session-resume.md) (reconnect and session
resume) and [ADR-053](../.10x/adrs/ADR-053-state-preserving-reload.md) (state across a rebuild). The wire is
unchanged (SPEC section 3.2): a client that does none of this still works.

## What it does

```
 your editor ── save ──▶ undra dev ──▶ builds the core ──▶ undra-dev-runner  (the core, a process of its own)
                          watches core/src                      │ ws://127.0.0.1:7443
                                                                ▼
          iOS simulator / device ◀── envelopes ──▶  the platform runtime's `remote` transport
          Android emulator / device                 (stores, mirror, ports: all in the app)
          browser tab
```

The core runs on your machine, in `undra-dev-runner`. The app keeps everything above the pixels that is the
platform's: its UI, its mirror of the core's state, and the adapters of the ports (`Http`, `Kv`, `Fs`,
`Connectivity`, ...), which the core calls over the socket. Clocks, randomness and logging are answered by your
machine, because a remote client cannot answer a synchronous port.

On a change `undra dev` rebuilds, takes the old core's state, and starts the new core on the same address with that
state restored (next section). While a rebuild fails, the old core keeps serving: a typo does not take the app down.

`undra dev` serves **one client at a time**. A second one is told to try again later (close 1013) and retries
by itself, so closing the first lets it in. There is no authentication: keep the default loopback address unless
a device has to reach you, and then only on a network you trust.

## Connecting, per platform

`undra dev` prints these with the real port when it starts.

| Platform | How | URL |
|---|---|---|
| Web | `?undra=<url>` in the page URL, or `VITE_UNDRA_DEV_URL` (under `vite dev`) | `ws://127.0.0.1:7443` |
| iOS simulator | `UNDRA_DEV_URL=<url>` in the scheme's Run environment, or `SIMCTL_CHILD_UNDRA_DEV_URL=<url> xcrun simctl launch ...` | `ws://127.0.0.1:7443` |
| iOS device | `UNDRA_DEV_URL=<url>`, with `undra dev --addr 0.0.0.0:7443` | `ws://<your Mac>:7443` |
| Android emulator | `adb shell am start -n <id>/.MainActivity --es undra_dev_url <url>`, or `./gradlew -PundraDevUrl=<url> :app:installDebug` | `ws://10.0.2.2:7443` |
| Android device, USB | `adb reverse tcp:7443 tcp:7443` (or `undra dev --android`), then as for the emulator | `ws://127.0.0.1:7443` |
| Android device, Wi-Fi | as iOS device | `ws://<your computer>:7443` |
| JVM | `LoadOptions(mode = Mode.REMOTE, remoteUrl = url, ...)` | `ws://127.0.0.1:7443` |

* `10.0.2.2` is the Android emulator's name for the computer it runs on (its loopback interface), so a server on
  the default `127.0.0.1` is reachable at it. A USB device has no such name: `adb reverse` makes the device's own
  `127.0.0.1:<port>` reach yours. `undra dev --android` runs it for every attached device (only the one in
  `ANDROID_SERIAL`, when that is set) at start and after every restart.
* Debug builds only. The Android dev URL and `usesCleartextTraffic` live in the debug build type
  (`app/src/debug/AndroidManifest.xml`, a `BuildConfig` field); a release build has neither. On iOS the URL is read under `#if DEBUG`, and the app needs
  `NSAllowsLocalNetworking` in its Info.plist for a `ws://` address. On the web `?undra=` is read only by a
  development build (`import.meta.env.DEV`): a production page that took its core's address from a link would give
  whoever wrote the link its ports and its screen.
* The schema hash still gates every connection: an app built from other bindings than the core's is told so
  (`UndraSchemaMismatch`) and does not run.

The apps `undra init` makes and the playground do all of this already. To wire your own app, copy the few lines
of `UndraBootstrap.swift`, `UndraApp.kt` + `DevServer.kt`, or `undra.ts` from a generated project (`undra adopt`
writes the steps).

## What happens when you save

```
rebuild ok
  1. the new core starts in standby: its runtime exists, nothing listens yet      (it cannot start? the old core keeps serving)
  2. the old core is suspended: no new calls, the open ones get up to 2 s to finish, its client is
     closed (1001), the session it left is kept; then its state is snapshotted (SPEC 5.9)
  3. the old core exits; the new core restores the snapshot, then listens and holds the old session
  4. the apps reconnect, resume their session, observe again and get the restored values first
```

The snapshot lives in the memory of the `undra dev` process only, for the milliseconds it takes to hand it from one
process to the next, over a pipe nobody else can read. It is never written to a file. It is limited to 16 MiB (the
playground's, with a 10,000-row list, is 205 KiB); over that the state is not carried and says so.

You see this:

```
==> Change detected, rebuilding
    built in 0.5s
Restarted: ws://127.0.0.1:7443  (schema hash 0x...); state kept (3 stores, 205 KiB, restored in 0.2 ms); 1 object not carried over: their handles are stale, the app creates them again; connected apps reconnect by themselves
```

and each app's dev bar says the same for four seconds: **`Reloaded, state kept`**, with `(N objects not carried over)` when
that applies and `(N calls lost in the reload)` when a call was cut off (below). Whatever stops the state from being carried falls back to the loop as it was before: the old core stops, the
new one starts fresh, and the line says why (`state reset: ...`):

| The line says | Why | What the app does |
|---|---|---|
| `state reset: the core refused the snapshot: store `Counter` .. signal `..`: ..` | A store's signals changed in a way that does not migrate by itself (a signal renamed or retyped, a new one without `#[undra(default)]`) and no `#[undra::migrate]` hook converts it (ADR-037). Across a schema change that only adds (methods, types, signals with a default, fields with a default or an `Option`) the state is kept: `state kept (..)`, and the app is told `Reloaded, state kept (the schema changed)`. | An app built from the old bindings reports a schema mismatch and stops (`closed(schemaMismatch)`, the bar says so). Run `undra bindgen`, rebuild the app, relaunch it. |
| `state reset: snapshot over 16 MiB` | A core with more state than that is better reset than stalled. | The app is told its session is lost (4001), loads a new core and starts over; its dev bar says `Reloaded, state reset: snapshot over 16 MiB`. |
| `state reset: the core refused the snapshot: ...` | The new core could not rebuild a store from the snapshot (it is unchanged: restore is all or nothing). | As above. |
| `state reset: the previous core did not ...` | The old core could not produce a snapshot (it died, or did not answer in 15 s). | As above. |
| `state reset: undra dev --no-keep-state` | You asked for it. | As above. |
| `the rebuilt core did not start; still serving the previous build` | The new core's process exited before it was ready. Nothing was touched: the old core keeps serving, with its state. | Nothing happens; fix the error shown and save again. |

### What carries over, and what does not

* **Stores** carry over, with their handles, so everything the app constructed (`Todos`, a counter, a 10,000-row list)
  is where it was: the mirrors converge on the restored values, and commands work at once. Computed signals are
  recomputed by the store's `restore` hook (the same code as its constructor).
* **Objects that are not stores, and query handles**, do not. Their handles are stale after a reload: a call on one fails
  with `UndraCallError.Refused` (the core's status 5), the line above counts them, and the app creates them again. **A
  stale query handle: run the query again** (construct the query handle again, for example by re-mounting the screen that
  owns it); until then the screen keeps the last values it had and `refetch()` is refused.
* **Tasks, timers and streams** do not: a task is a future, not data. A call that is running when the core is replaced gets
  up to two seconds to finish (so an `async` command in the middle of a port call completes); one that does not is cancelled
  and fails as `Unavailable`, and a store it half-wrote keeps that value (a `loading = true` nobody clears: tap again). A store
  that must keep a background task alive across a reload starts it from its `restore = ".."` hook, which receives the `Ctx`.
* **Calls made during the swap** are not run: from the moment the old core is suspended it runs no new call (a tap in those
  milliseconds, or in the up to two seconds an open call is given), and the client fails it as `Unavailable` when the socket
  closes. A command that fails that way is only logged (the connection state already says the core is reconnecting), so
  the `Restarted:` line and the dev bar count such calls together with the cancelled ones: `1 call sent during the reload
  was not run`, `(2 calls lost in the reload)`. The state is the state before them: tap again.
* **The query cache and the offline queue** are not store state: queries refetch when observed again.
* **State reached by old logic** is restored into new logic; that is what a reload is. If it confuses you, relaunch the app
  (a new session replaces the carried one and builds fresh stores on the new code), or run `undra dev --no-keep-state`.

### The runner protocol

`undra dev` and its runner (`undra-dev-runner`, the process that runs the core) talk over the runner's stdin and stdout, in
lines, and nothing else: no port, no file. It is internal, and documented here because the integration tests speak it.

```
undra dev -> runner  (stdin)                     runner -> undra dev  (stdout)
  snapshot                                         UNDRA-DEV snapshot ok <settled> <cancelled> <not run> <stores> <bytes> <token|-> <handles|-> <hex>
                                                   UNDRA-DEV snapshot failed <reason>
  state <old-hash> <lost calls> <token|-> <handles|-> <hex>
                                                   UNDRA-DEV restored <stores> <lost objects> <bytes> <microseconds>
                                                   UNDRA-DEV reset <reason>
  reset <reason>                                   (nothing)
  listen                                           UNDRA-DEV ready <ws-url> <schema-hash>
  (a runner started with --standby)                UNDRA-DEV standby <schema-hash>
  (stdin closes: stop)
  (started with --devtools and UNDRA_DEVTOOLS_TOKEN in its environment: serves the page of ADR-054 at /devtools)
```

`undra dev` reads at most 48 MiB of one stdout line (a snapshot at the 16 MiB limit is 32 MiB of hex) and does not
require UTF-8: the core's own `print!` output shares the pipe and is shown as it comes; a protocol line that follows
`print!` output without a newline is still recognised.

## Reconnecting

All three runtimes do the same, with the idiom of their platform.

| | TypeScript | Kotlin | Swift |
|---|---|---|---|
| The state | `core.connection` (a `Signal`; `useSignal(core.connection)` in React) | `core.connectionState` (a `StateFlow`) | `core.connectionState`, `core.connection.state` (`@Observable`), `core.connectionStates()` (`AsyncStream`) |
| Every change | `onConnectionChange` | `LoadOptions.onConnectionChange` | `LoadOptions.onConnectionChange` |
| The policy | `reconnect: false \| { initialDelayMs, maxDelayMs, jitter, maxAttempts }` | `ReconnectPolicy(...)`, `null` is off | `UndraReconnectPolicy(...)`, `nil` is off |
| What `undra dev` says about a reload (dev only) | `onDevNotice: (message) => void` | `LoadOptions.onDevNotice` | `LoadOptions.onDevNotice`, and the `onDevNotice:` of `.remote(...)` |

The states are `connecting` (during `load`), `connected`, `reconnecting(attempt)` and `closed(reason)`, where the
reason is `requested` (you closed the core), `schemaMismatch`, `sessionLost` or `failed`.

`onDevNotice` receives the one-line messages the dev server says about itself (`Reloaded, state kept`, `Reloaded, state
reset: ..`): a `Log` record with the target `undra::dev`, sent once to every client that attaches within 30
seconds of a rebuild. It is for a status bar; the playground's and the generated apps' bars show it for four seconds. It is
**development only and inert otherwise**: an in-process or production core never produces such a record, and each runtime
dispatches it from its `remote` transport only. The target is the dev server's: a record your core logs under
`undra::dev` is printed in the terminal but never sent to a client, so the bar only ever shows what `undra dev` said.

* **Backoff.** Attempt `n` waits `min(5 s, 250 ms * 2^(n-1))`, less up to half of it at random, so many clients
  do not retry in step: 250, 500, 1000, 2000, 4000, 5000, 5000 ms, jittered. Each attempt gets at most 5 s.
  The first connection of `load` is not retried: it fails fast, as before, with the URL it tried.
* **In flight.** When the connection drops, every call, stream and pending `observe` fails **at once** with the
  platform's "unavailable" outcome (TypeScript `UndraTransportError("closed")`, Kotlin `UndraTransportException`
  with reason `CONNECTION_LOST`, Swift `UndraTransportError.connectionLost`), which a generated call throws as
  `UndraCallError.Unavailable` (`docs/ERRORS.md`). Calls made while reconnecting fail the same way, at once.
  Nothing waits for the network and nothing is replayed behind your back. A command (a method that returns nothing
  and has no error type) that fails this way is logged at warning level and is **not** handed to `onError`: the
  connection state already reports the drop.
* **Resync.** After the handshake the runtime observes every store signal you observed and releases what you
  released meanwhile. The core answers each observation with its current values, so every mirror converges by
  itself. (`connected` is announced after that.)
* **Final.** `close()`, a schema-hash change, a lost session and (for TypeScript and Kotlin) a protocol error end the
  core: no retry, no loop, one `closed(reason)`. TypeScript also calls `onClose` once. A schema change is the
  existing `UndraSchemaMismatch` error.

## What the server keeps for you

A client puts a random token in the URL of every connection of one core (`?undra_session=...`). When it drops,
the dev server **keeps the objects its constructors made** for ten minutes instead of releasing them, and a
reconnecting client that holds objects adds `&undra_resume=1` and finds them again, with their state. They are
released when the time passes, when a different client attaches (a relaunched app), or when the server stops, so
a dev core holds at most one launch's objects. A client that asks to resume something the server does not hold
(a core that could not carry its state) is answered with close code 4001. A client back on a new socket under its own token replaces
its stale one at once: a phone that changed network does not wait for the keepalive.

The token is not a password. The dev server has no authentication: whoever can reach its port can use the core,
token or not; the token only lets a client that comes back be recognised (ADR-051 has the threat model). The
server logs its first eight characters, never the whole token.

The server prints every step:

```
client connected: platform=android mode=dev undra=0.1.0
client disconnected (0 calls cancelled, 1 object(s) kept for 600 s so it can reconnect)
client reconnected: platform=android mode=dev undra=0.1.0 (session 356b33fa, away 6.0 s, 1 object(s) kept)
suspended for a reload: 0 call(s) were still open, 0 sent meanwhile were not run, session 356b33fa (1 object(s)) handed over
holding session 356b33fa (1 object(s)) for its client
a client (ios) asked to resume session 319c2156, which this core does not hold (it was restarted, ...)
```

## Recording a session: `undra dev --record FILE`

`undra dev --record session.json` writes what happened in the session as an `undra.recording` (the format and what to do with it are in
`docs/TESTING.md`): every call, reply, change-set, stream item, port call and its answer, event, timer report and observe the dev server relays, with
the time since the session started, and the readings of the dev core's own `Clock` and `Rng` (native bindings that never cross the socket). It works for
iOS, Android and web alike, because the server sees the same envelopes from all of them. Play the file back under the generated stores
(`RecordedCore`) to preview a state that is costly to reach, or feed its port calls to a `Replayer` in a test.

* The file is rewritten twice a second while it grows, and once more when `undra dev` stops.
* A recording belongs to one core and so to one schema hash: a rebuild starts a new core, and its session goes to `NAME-2.EXT` (`session-2.json`, `session-3.json`, ...);
  the first file keeps what the first core did.
* A core that polls the clock a lot writes a lot: record a session you want to keep, not a benchmark.
* **Secrets.** The calls of the `SecureStore` port are recorded with empty arguments and replies (the file's `source` says "SecureStore payloads left out");
  `--record-secrets` keeps them. HTTP headers and bodies, `Kv` values and files are recorded as they are: do not share a recording of a real account's session.
* If the file cannot be written (a full disk, a removed directory) the runner logs one warning, stops recording and keeps serving; what was written before stays.

## The devtools page

`undra dev` serves a page next to the core, and prints its address with the banner:

```
  devtools      http://127.0.0.1:7443/devtools?token=4a378c25cfbc04128333e3667da4755d
```

Open it in a browser while the app runs (any platform: the page talks to the core, not to the app). It shows:

* **Stores**: every store the core holds, each signal with its type and live value; a keyed list is a table (the first 40
  rows, then "show more"); a value that just changed flashes.
* **A scrubber** over the history: one *step* per burst of commits (a click is a step). Drag it, press the arrows, or
  press **Restore** on a timeline entry: the core is restored to that step (`Runtime::restore`, SPEC 5.9) and the app
  converges on it, with its dev bar saying `time travel: step 3`. **Live** goes back to the newest step you did not
  restore. A restore is itself a step: the history only grows.
* **Timeline**: every change-set, newest first, with the call that caused it (`Counter.increment`; `task, timer or stream`
  for what an async method, a timer or a stream committed) and a per-signal diff (`3 → 4`, a keyed patch as `+1 −2 ~3`).
* **Ports**: each call the core makes to a platform port (Http, Kv, ...) with its decoded arguments and reply, its status
  and its latency. Calls made while no app is attached are counted in Counters, not listed.
* **Queries**: the query cache (status, who watches it, how old, the cached value) and what happened to each entry.
* **Counters**: the core's `undra_stats_json` once a second, and the dev server's own: commits and entries forwarded, steps
  taken, commits merged per step, the history held, the app connection's outbound backlog. They are the *server's* numbers:
  the app's own drains, merges and backlog (SPEC 11.1) are counted by its mirror and never reach the dev server.

The page keeps the dark and light themes of the site (it follows the system; the button overrides it).

**What time travel is.** A restore replaces the core's stores with the snapshot of the step and re-issues their handles, so
the app's references keep working and its mirrors converge through the change-sets the restore emits, like a reload
(the rules of "What carries over" above apply: objects that are not stores and query handles go stale). Stores built
*after* the step are dropped (the answer says how many, and so does the app's dev bar; the app's references to them are stale). Calls running on a replaced
store are cancelled. The history is kept in the dev server (200 steps, 32 MiB, 4 MiB a step; a bigger state is listed but
cannot be restored), records only while a page is open, and starts again after a reload (the page draws a divider). While a
page is open the server observes every store, so a computed nobody shows is evaluated. Snapshots are taken after a burst of
commits, at most one every 10 ms (further apart when a snapshot is slow, so a big state cannot keep the core busy), and a page
that stops reading is dropped rather than waited for.

**Who can open it.** The page can read the core's state and restore it, so every request needs the per-run token in the
address (`undra dev` makes one per run, 128 random bits, and passes it to the runner in its environment); a wrong or missing
token is a `404`, like a path that does not exist. `--devtools auto` (the default) serves the page on a loopback `--addr`
only; with a LAN address use `--devtools on` and treat the printed address as a secret, or `--devtools off`. The page is
compiled into the runner `undra dev` generates; a production core has no dev server and so none of this.

## Android notes

* The Kotlin runtime's WebSocket client is its own, over `java.net.Socket`, because `java.net.http` (what the
  remote transport used) is not on Android. It does connect, read and write on threads of its own, so
  `UndraCore.load` may be called from the main thread (it waits for the connect, like any dev-only synchronous
  call; use a short `remoteTimeout`) and `observe`, `release` and `call` never touch the network on it
  (`NetworkOnMainThreadException`).
* It pings a server that has been quiet and gives up on one that stays quiet, so a laptop that went to sleep is
  noticed in seconds, not minutes.
* It checks what it is sent: a server that breaks RFC 6455, or a message over 64 MiB, ends the core
  (`closed(failed)`, after a close frame 1002 or 1009) instead of looping on reconnects. `wss://` needs a certificate
  that is trusted and names the host.
* The playground and the template show the state in a thin bar (green, amber while reconnecting, red when it is
  over) and a screen with the reason and a Retry button when the dev server cannot be reached at launch.
* No native library is needed in remote mode. The generated Gradle app builds the core itself (its `undraBuild` task runs
  `undra build --platform android` before every build, and is skipped while the core is unchanged); a build that will
  only run against `undra dev` can skip it with `./gradlew -PundraSkipBuild=true -PundraDevUrl=... :app:installDebug`
  (or `UNDRA_SKIP_BUILD=1`).

## Troubleshooting

| You see | Look at |
|---|---|
| The app says it cannot reach the dev server | Is `undra dev` running, on that port? Emulator: `ws://10.0.2.2:<port>`. USB device: `adb reverse tcp:<port> tcp:<port>` (or `undra dev --android`) and `ws://127.0.0.1:<port>`. A phone on Wi-Fi needs `--addr 0.0.0.0:<port>` and your computer's address. |
| Android: `Cleartext HTTP traffic ... not permitted`, or no connection at all | Only a debug build has the cleartext entry (the `INTERNET` permission is in every build); install `:app:installDebug`, not a release build. |
| iOS: the connection fails at once | `NSAllowsLocalNetworking` in the Info.plist; on a device, the local-network permission prompt. |
| `schema mismatch` | The core and the bindings differ: `undra bindgen`, rebuild the app. |
| `refused ... one is already attached` / close 1013 | Another client holds the server: a second simulator, a browser tab, a forgotten app. It retries by itself once the first is gone. |
| The status stays amber | The server is down or unreachable: check the terminal running `undra dev` (a failed rebuild keeps the old core serving, a crashed core stops the server: run it again). |
| `adb reverse` fails or `--android` finds nothing | `adb devices`: a device must say `device`, not `unauthorized` or `offline`. With several devices set `ANDROID_SERIAL`. |
| A web page does not reconnect | The page must be allowed to reach the address (`undra dev` accepts pages on this machine and private networks); try `127.0.0.1`, not `localhost`, and a plain `ws://` URL from an `http://` page. |
| The app's screen resets after every save | Read the `Restarted:` line: it says whether the state was carried and, if not, why (a schema change the state cannot follow, a state over 16 MiB, `--no-keep-state`). The app's dev bar says it too. |
| `1 object not carried over`, and `refetch` is refused with a stale handle | A query handle (or another object that is not a store) does not survive a reload. Run the query again: construct the query handle again, for example by re-mounting its screen. |
| A spinner or a `loading` flag stays on after a save | A call that was still running when the core was replaced was cancelled and left its store at the value it had written; trigger the action again. |
| A tap right as you saved did nothing; the bar said `(1 call lost in the reload)` | Calls made while the old core was being swapped out are not run (their writes are not in the carried state); tap again. |

`undra doctor` checks the toolchains the loop needs (the Android SDK, NDK, JDK, Xcode, `adb` and whether a device is
attached, `undra` on `PATH`), with the fix command for each gap; `undra dev --help` has the command reference.

## Production builds are not a separate step

The dev loop above is `undra dev`. The other half, the build of what you ship, needs no command either: a project made by
`undra init` builds its core from the app's own build system, each time only when the core changed.

| App | What runs `undra build` | When | Skipped by |
|---|---|---|---|
| Android | the `undraBuild` Gradle task, `preBuild` depends on it | every Gradle build; `assembleRelease` and `bundleRelease` build a release core, anything else a debug one (`-PundraRelease=true\|false` overrides; a build that asks for both variants at once, `./gradlew build`, gets a release core in both) | Gradle's up-to-date check over `core/src/**`, the Cargo manifests, `Cargo.lock` and `build/android/jniLibs` (a path dependency outside `core/` is an input only once added with `undraBuild { sources.from(...) }`); `-PundraSkipBuild=true`, `UNDRA_SKIP_BUILD=1` |
| iOS | the **Build the Undra core** Run Script phase, before Compile Sources, with `undra build --platform ios --configuration $CONFIGURATION` | every Xcode build | Xcode's input/output analysis over `ios/Config/undra-core-inputs.xcfilelist` (every file of the core and of its path dependencies, the Cargo manifests and `Cargo.lock`, kept in step by `undra build`) and `undra-core-outputs.xcfilelist` (the XCFramework and a stamp per configuration) |
| Web | the `undra()` plugin of `web/vite.config.ts` (`@undra/runtime/vite`) | `vite build` and `vite dev`; under `vite dev` also on every change of the core's `src/**`, its manifests or `Cargo.lock` (one build at a time, the first included), followed by a full reload; never under Vitest (mode `test`) unless `inTests: true` | `UNDRA_SKIP_BUILD=1`, the `skip` option |

The Vite plugin also builds the page for `es2022` unless the app's `vite.config.ts` sets `build.target` itself: Vite 6 and older
default to `es2020`, which turns the runtime's class fields and private members into helper calls and makes the call
path several times slower (`bench/RESULTS.md`, ADR-056). What `es2022` excludes: the runtime's own code needs class fields
(Chrome 72, Firefox 69, Safari 14.1, so iOS 15 Safari runs it), and your own code may use any ES2022 syntax, up to class
static blocks (Chrome 94, Firefox 93, Safari 16.4). A browser older than that needs a `build.target` of your own, which the
plugin keeps; a target below `es2022` (the pinned Vite also lowers class fields for `safari15`) makes the bundler lower the
runtime's class fields again, and the call path costs what it did before ADR-056.

All three find `undra` on `PATH` and in `~/.undra/bin`, `~/.cargo/bin` and Homebrew's directories, because an app launched
from the Dock or an IDE has a short `PATH`; the Gradle task and the Vite plugin take `UNDRA_BIN` first (the Xcode phase
does not: Xcode's environment is the project's build settings, not your shell's). When it is
missing they say how to install it, in the shape of the CLI's own errors (`error[undra::C0003]`). A build that fails keeps
its own output (`C0004`); under `vite dev` it shows in the page's error overlay, the page keeps the core it had, and the
next save retries.

What none of the three sees: `.cargo/config.toml`, `rust-toolchain.toml`, a new Rust compiler and a new `undra`. After
changing one of those, build once with `undra build` (or `./gradlew :app:undraBuild --rerun`, or Xcode's Clean Build
Folder).
