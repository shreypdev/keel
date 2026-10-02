package dev.undra.contract

/**
 * One scenario of `contract-tests/scenarios.md`: its id and title (the title is printed after the
 * verdict) and the body, which throws [Mismatch] (or anything else) when a step does not hold.
 */
class Scenario(val id: String, val title: String, val body: (Bootstrap) -> Unit)

/** A scenario that cannot run here: `Main` prints `SCENARIO <id> SKIP <reason>` (scenarios.md lists every allowed skip). */
class Skipped(val reason: String) : RuntimeException(reason)

/** A scenario that needs the loaded core; it fails at once if S16 did not manage to load it. */
private fun scenario(id: String, title: String, body: (World) -> Unit): Scenario =
    Scenario(id, title) { boot -> body(boot.world ?: fail("the core is not loaded (S16 failed to load it)")) }

/**
 * The scenarios in the order they run. S16 is first because it is the one that loads the core:
 * its failing load has to come before the load the others use (`UndraCore.load` leaves nothing behind when
 * it fails), and the others need the core it loads. S17 is last because its last steps shut that core down
 * (S17.6) and load and close a fresh one (S17.7), so S18, S19, S20, the opt-in ports' S23 to S25 and S26 run before
 * it. The build-B steps of S14 and S15 run in a second JVM ([migrationBuildB]).
 */
val SCENARIOS: List<Scenario> = listOf(
    Scenario("S16", "schema mismatch rejection", ::s16SchemaMismatch),
    scenario("S01", "primitives round-trip", ::s01Primitives),
    scenario("S02", "records, enums and errors", ::s02RecordsEnumsErrors),
    scenario("S03", "sync call", ::s03SyncCall),
    scenario("S04", "async call", ::s04AsyncCall),
    scenario("S05", "error propagation", ::s05ErrorPropagation),
    scenario("S06", "cancellation", ::s06Cancellation),
    scenario("S07", "stream with backpressure", ::s07Stream),
    scenario("S08", "store observe: initial change-set", ::s08Observe),
    scenario("S09", "transaction: a single change-set", ::s09Transaction),
    scenario("S10", "keyed patch", ::s10KeyedPatch),
    scenario("S11", "computed", ::s11Computed),
    scenario("S12", "query: fetch, stale, refetch", ::s12Query),
    scenario("S13", "optimistic mutation and rollback", ::s13Optimistic),
    scenario("S14", "offline queue replay", ::s14Offline),
    scenario("S15", "snapshot and restore", ::s15Snapshot),
    scenario("S18", "coalesced burst", ::s18CoalescedBurst),
    scenario("S19", "derived keyed list", ::s19DerivedKeyedList),
    scenario("S20", "storage failures are typed", ::s20Storage),
    scenario("S23", "websocket", ::s23WebSocket),
    scenario("S24", "server-sent events", ::s24Sse),
    scenario("S25", "db", ::s25Db),
    // S26 loads two other cores (other namespaces) next to the playground core, and closes them again.
    scenario("S26", "two cores", ::s26TwoCores),
    // S27 step 8 loads S26's two cores again (S26 closed them) to pass one core's object to the other.
    scenario("S27", "objects cross", ::s27ObjectsCross),
    scenario("S28", "host callbacks", ::s28HostCallbacks),
    scenario("S31", "newtypes, generic instantiations and leaf types", ::s31Ledger),
    scenario("S32", "paged queries and lazy lists", ::s32Paging),
    scenario("S33", "polling", ::s33Polling),
    scenario("S17", "panic containment", ::s17Panic),
)
