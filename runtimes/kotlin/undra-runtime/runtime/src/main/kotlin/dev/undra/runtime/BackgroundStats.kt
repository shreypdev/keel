package dev.undra.runtime

/**
 * What the core's background tasks have and have done ([UndraStats.background], the `background` object of the core's statistics,
 * ADR-046).
 *
 * The platform reads [pending] right after it reports `Lifecycle.Background` (and after a mutation was queued offline) to decide
 * whether to ask the OS for a background window: `android-work`'s `UndraWork.scheduleIfPending` does.
 *
 * @property tasks background tasks registered (the query layer's replay, refetch and flush are three).
 * @property pending how much work a background window would drain now: queued offline mutations, stale persisted queries and
 *   unflushed persistence. Above zero, a window is worth asking for.
 * @property runs background runs started so far (`UndraCore.runInBackground`).
 * @property finished runs in which every task finished before the deadline.
 * @property replayed offline mutations replayed by background runs.
 * @property refetched stale queries refetched by background runs.
 */
public class BackgroundStats(
    public val tasks: Int,
    public val pending: Int,
    public val runs: Long,
    public val finished: Long,
    public val replayed: Long,
    public val refetched: Long,
) {
    /** Whether a background window has work to drain ([pending] is above zero). */
    public val hasPendingWork: Boolean get() = pending > 0

    override fun equals(other: Any?): Boolean =
        this === other || (
            other is BackgroundStats && tasks == other.tasks && pending == other.pending && runs == other.runs &&
                finished == other.finished && replayed == other.replayed && refetched == other.refetched
            )

    override fun hashCode(): Int {
        var result = tasks
        result = 31 * result + pending
        result = 31 * result + runs.hashCode()
        result = 31 * result + finished.hashCode()
        result = 31 * result + replayed.hashCode()
        return 31 * result + refetched.hashCode()
    }

    override fun toString(): String =
        "BackgroundStats(tasks=$tasks, pending=$pending, runs=$runs, finished=$finished, replayed=$replayed, refetched=$refetched)"

    internal companion object {
        /** Reads the `background` object of the statistics document; a number that is missing is [UndraStats.UNKNOWN]. */
        fun fromJson(doc: Map<*, *>): BackgroundStats {
            fun long(key: String): Long = (doc[key] as? Long) ?: UndraStats.UNKNOWN.toLong()
            fun int(key: String): Int = (doc[key] as? Long)?.coerceIn(0L, Int.MAX_VALUE.toLong())?.toInt() ?: UndraStats.UNKNOWN
            return BackgroundStats(
                tasks = int("tasks"),
                pending = int("pending"),
                runs = long("runs"),
                finished = long("finished"),
                replayed = long("replayed"),
                refetched = long("refetched"),
            )
        }
    }
}

/** What [UndraStats] reports when the core says nothing about its background tasks: every number unknown. */
internal val NO_BACKGROUND_STATS: BackgroundStats = BackgroundStats(
    tasks = UndraStats.UNKNOWN,
    pending = UndraStats.UNKNOWN,
    runs = UndraStats.UNKNOWN.toLong(),
    finished = UndraStats.UNKNOWN.toLong(),
    replayed = UndraStats.UNKNOWN.toLong(),
    refetched = UndraStats.UNKNOWN.toLong(),
)
