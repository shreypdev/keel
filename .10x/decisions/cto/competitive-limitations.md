# CTO: what the competitive catalogue changes in the v1.x plan (2026-10-01)

**Source.** `.10x/specs/2026-10-01-competitive-limitations.md`. It maps 68 sourced limitations of KMP,
UniFFI, Crux, React Native, Flutter, Capacitor and "write it three times" onto Undra: 43 solved, 18
planned, 6 missing, 1 out of scope by decision. It also records 30 places where an alternative still beats
Undra. Facts there are dated and sourced; the rankings are judgement.

**The five findings that should change the plan**

1. **Android ships without platform adapters.** On Android the Kotlin runtime installs only Clock, Rng, Log
   and Timer. The playground fakes Kv and HTTP (`android-adapters/README.md:21-47`, `UndraApp.kt`), and CI
   cannot see the gap, because the contract suite brings its own Kv and Http and runs Kotlin on the JVM. Without adapters, "offline and persistence on three
   platforms" is not true for an Android team on day one, while KMP teams get Ktor and DataStore.
   *Change:* promote this out of the C4 audit into a named Phase-1 piece. It covers
   `UndraAndroid.load(context)`, the Http, Kv, SecureStore, Fs, Connectivity and Lifecycle adapters, the R8
   rules, and an Android smoke test that runs on the default adapters.
2. **The data layer is short of what TanStack-class users expect.** There is no polling (SPEC §9 lists
   `interval_ms`; the code does not implement it) and no paged or infinite queries (`Lazy<T>` is rejected in
   v1, yet E3 is scoped as "ergonomics"). Queued offline mutations also lose their optimistic state and
   their invalidations when the app restarts. *Change:* re-scope E3 as an ADR-first piece (lazy lists,
   infinite queries, polling) and widen A5 to cover a restart without a schema change.
3. **The boundary surface is narrower than UniFFI's and flutter_rust_bridge's.** Undra has no objects as
   parameters or returns (E0064), no host callbacks or listeners as arguments (E0004), no generic
   instantiations (E0002) and no newtypes (E0007). UniFFI migrants, our first market, hit E0064 first.
   *Change:* add ADRs for object handles as values and for foreign callback objects (runtime-model and wire
   changes), plus generic type aliases. Extend C3 with newtypes, `HashSet` and borrowed bytes. G4 must not
   ship narrower than flutter_rust_bridge.
4. **Running in production is unplanned.** Undra emits no crash-symbol files, documents no debugger path
   into Rust, and has no OS background-execution hook (BGTaskScheduler / WorkManager) to drain the offline
   queue. *Change:* add symbols and debugging to D3 (both S), and add a background piece (M) to Track G.
5. **Two shape decisions get expensive once anything is published.** Today only one Undra library can
   exist per app: native `undra_*` symbols are global and there is one core per process. That blocks SDK
   vendors. In our judgement these overlap with the blueprint's first customers (companies already running
   Rust cores: fintech, security, messaging). KMP documents the same pain, and UniFFI
   appears to avoid it through crate-namespaced symbols (not verified). Separately, the iOS 17 floor (Observation) is above the iOS 15 that KMP, React Native and
   Flutter reach. *Change:* decide per-core ABI namespacing (together with G1's ADR-038, which touches the
   same surface) and an `ObservableObject` binding mode for iOS 15–16 by ADR in Phase 1. A4 already uses the
   same "nothing is published yet" argument.

**Where competitors clearly beat us (keep in the post, do not spin):** ecosystem, backing and production
record (JetBrains + Google, Meta + Expo, Google, Mozilla); developer tooling (Flutter and RN DevTools, the
KMP plugin's cross-language debugging, reload that keeps state); and breadth (shared UI when wanted,
Windows, Linux and older-iOS reach, a wider boundary surface). B4, B3, F1, C5 and the five changes above
address the second and third. Only time addresses the first.

**Not fixed, on purpose:** shared UI, Python/C#/Go/Ruby hosts, over-the-air updates of native logic, and
library-by-library ecosystem parity.
