# The dev loop: `undra dev`

Edit Rust, save, and the app on every platform is on the new core within a second. A dropped connection
heals itself: no restart, no relaunch. This page is what `undra dev` does per platform, which URL each
platform uses, how reconnecting works, and what to check when it does not.

The design is [ADR-051](../.10x/adrs/ADR-051-dev-client-reconnect-and-session-resume.md). The wire is
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

On a change `undra dev` rebuilds, stops the old runner (a Close frame, 1001) and starts the new one on the same
address. While a rebuild fails, the old core keeps serving: a typo does not take the app down.

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

1. `undra dev` rebuilds the core (a second or two for a small one) and swaps the runner.
2. The apps' sockets close; each runtime goes to **reconnecting** and retries with backoff.
3. The first retry that finds the new runner is told **session lost** (close code 4001): the new core has none of
   the old core's objects, so the runtime closes that core (`closed(sessionLost)`) instead of letting every call
   fail with a stale handle.
4. The app loads a new core and starts over on it: the web playground reloads its page, the iOS and Android
   playgrounds load the core again and rebuild their screens. Their stores start from the new core's initial
   state.

State is not carried across a rebuild yet: that is the next piece (a snapshot before the rebuild, a restore
after, so you stay on the screen you were on). Changing the **schema** (a public type or signature) changes the
hash: run `undra bindgen`, rebuild the app, relaunch it. `undra dev` says so when it sees the hash change.

## Reconnecting

All three runtimes do the same, with the idiom of their platform.

| | TypeScript | Kotlin | Swift |
|---|---|---|---|
| The state | `core.connection` (a `Signal`; `useSignal(core.connection)` in React) | `core.connectionState` (a `StateFlow`) | `core.connectionState`, `core.connection.state` (`@Observable`), `core.connectionStates()` (`AsyncStream`) |
| Every change | `onConnectionChange` | `LoadOptions.onConnectionChange` | `LoadOptions.onConnectionChange` |
| The policy | `reconnect: false \| { initialDelayMs, maxDelayMs, jitter, maxAttempts }` | `ReconnectPolicy(...)`, `null` is off | `UndraReconnectPolicy(...)`, `nil` is off |

The states are `connecting` (during `load`), `connected`, `reconnecting(attempt)` and `closed(reason)`, where the
reason is `requested` (you closed the core), `schemaMismatch`, `sessionLost` or `failed`.

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
(it was restarted) is answered with close code 4001. A client back on a new socket under its own token replaces
its stale one at once: a phone that changed network does not wait for the keepalive.

The token is not a password. The dev server has no authentication: whoever can reach its port can use the core,
token or not; the token only lets a client that comes back be recognised (ADR-051 has the threat model). The
server logs its first eight characters, never the whole token.

The server prints every step:

```
client connected: platform=android mode=dev undra=0.1.0
client disconnected (0 calls cancelled, 1 object(s) kept for 600 s so it can reconnect)
client reconnected: platform=android mode=dev undra=0.1.0 (session 356b33fa, away 6.0 s, 1 object(s) kept)
a client (ios) asked to resume session 319c2156, which this core does not hold (it was restarted, ...)
```

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
| The app's screen resets after every save | That is the current behaviour: state is not carried across a rebuild yet (see above). A change to state-shaping code shows at once from the first screen. |

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

All three find `undra` on `PATH` and in `~/.undra/bin`, `~/.cargo/bin` and Homebrew's directories, because an app launched
from the Dock or an IDE has a short `PATH`; the Gradle task and the Vite plugin take `UNDRA_BIN` first (the Xcode phase
does not: Xcode's environment is the project's build settings, not your shell's). When it is
missing they say how to install it, in the shape of the CLI's own errors (`error[undra::C0003]`). A build that fails keeps
its own output (`C0004`); under `vite dev` it shows in the page's error overlay, the page keeps the core it had, and the
next save retries.

What none of the three sees: `.cargo/config.toml`, `rust-toolchain.toml`, a new Rust compiler and a new `undra`. After
changing one of those, build once with `undra build` (or `./gradlew :app:undraBuild --rerun`, or Xcode's Clean Build
Folder).
