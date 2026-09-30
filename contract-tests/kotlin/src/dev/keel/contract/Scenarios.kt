package dev.keel.contract

/**
 * One scenario of `contract-tests/scenarios.md`: its id and title (the title is printed after the
 * verdict) and the body, which throws [Mismatch] (or anything else) when a step does not hold.
 */
class Scenario(val id: String, val title: String, val body: (Bootstrap) -> Unit)

/** A scenario that needs the loaded core; it fails at once if S16 did not manage to load it. */
private fun scenario(id: String, title: String, body: (World) -> Unit): Scenario =
    Scenario(id, title) { boot -> body(boot.world ?: fail("the core is not loaded (S16 failed to load it)")) }

/**
 * The seventeen scenarios in the order they run. S16 is first because it is the one that loads the core:
 * its failing load has to come before the load that sticks (`KeelCore.load` leaves nothing behind when
 * it fails, but a successful one cannot be undone), and the others need the core it loads.
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
    scenario("S17", "panic containment", ::s17Panic),
)
