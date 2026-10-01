package dev.undra.playground.bench

import org.json.JSONArray
import org.json.JSONObject
import kotlin.math.ceil

/**
 * The statistics of the device benchmark. The Swift runner (`BenchRunner.swift`) and the web one (`web/src/bench/`)
 * compute the same numbers the same way; `scripts/bench-device-report.mjs` checks every result file against the schema
 * they share.
 */
class BenchSummary(samples: List<Double>) {
    val n: Int
    val min: Double
    val p50: Double
    val p90: Double
    val p99: Double
    val max: Double
    val mean: Double

    init {
        val sorted = samples.sorted()
        n = sorted.size
        min = sorted.firstOrNull() ?: Double.NaN
        max = sorted.lastOrNull() ?: Double.NaN
        mean = if (n == 0) Double.NaN else sorted.sum() / n
        p50 = percentile(sorted, 0.5)
        p90 = percentile(sorted, 0.9)
        p99 = percentile(sorted, 0.99)
    }

    /** The summary as the result file records it. */
    fun toJson(): JSONObject = JSONObject()
        .put("n", n).put("min", num(min)).put("p50", num(p50)).put("p90", num(p90)).put("p99", num(p99)).put("max", num(max)).put("mean", num(mean))

    companion object {
        /** The value at rank `ceil(p * n)`; the epsilon keeps 0.07 * 100 from rounding up to rank 8. */
        fun percentile(sorted: List<Double>, p: Double): Double {
            if (sorted.isEmpty()) return Double.NaN
            val rank = minOf(sorted.size, maxOf(1, ceil(p * sorted.size - 1e-9).toInt()))
            return sorted[rank - 1]
        }

        /** A number for the result file: JSON has no NaN, so the summary of nothing is `null`. */
        fun num(value: Double): Any = if (value.isFinite()) value else JSONObject.NULL
    }
}

/** One measured operation, as a row of the result file (nanoseconds per operation). */
fun benchOp(id: String, batch: Int, perOpNs: List<Double>, note: String): JSONObject {
    val s = BenchSummary(perOpNs)
    return JSONObject()
        .put("id", id).put("mode", if (batch == 1) "each" else "batched").put("batch", batch).put("samples", s.n)
        .put("min", BenchSummary.num(s.min)).put("p50", BenchSummary.num(s.p50)).put("p90", BenchSummary.num(s.p90))
        .put("p99", BenchSummary.num(s.p99)).put("max", BenchSummary.num(s.max)).put("mean", BenchSummary.num(s.mean)).put("note", note)
}

/** A check of the benchmark itself failed: the numbers of this run are not to be trusted, so none is reported. */
class BenchCheckException(message: String) : Exception("device bench check failed: $message")

/** A JSON array of the objects. */
fun jsonArray(items: List<JSONObject>): JSONArray = JSONArray().also { array -> items.forEach { array.put(it) } }
