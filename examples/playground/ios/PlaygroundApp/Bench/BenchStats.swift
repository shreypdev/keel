import Foundation

/// The clock and the statistics of the device benchmark. The Kotlin runner (`BenchRunner.kt`) and the web one
/// (`web/src/bench/`) compute the same numbers the same way; `scripts/bench-device-report.mjs` checks
/// every result file against the schema they share.
enum BenchClock {
    /// A monotonic clock in nanoseconds (`CLOCK_UPTIME_RAW`: it does not count the time the device sleeps).
    @inline(__always)
    static func now() -> UInt64 {
        return clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
    }

    /// The smallest step of `now()` and what one read costs, both in nanoseconds.
    static func facts() -> (resolutionNs: Double, overheadNs: Double) {
        var smallest = UInt64.max
        var last = now()
        var seen = 0
        var reads = 0
        while seen < 20 && reads < 5_000_000 {
            reads += 1
            let t = now()
            if t > last {
                smallest = min(smallest, t - last)
                last = t
                seen += 1
            }
        }
        let count = 100_000
        let t0 = now()
        var sink: UInt64 = 0
        for _ in 0 ..< count {
            sink &+= now()
        }
        let t1 = now()
        if sink == 0 { print("") }
        return (smallest == UInt64.max ? 0 : Double(smallest), Double(t1 - t0) / Double(count))
    }
}

/// The summary of a list of samples, in the unit of the samples (nearest-rank percentiles).
struct BenchSummary {
    var n = 0
    var min = Double.nan
    var p50 = Double.nan
    var p90 = Double.nan
    var p99 = Double.nan
    var max = Double.nan
    var mean = Double.nan

    init(samples: [Double]) {
        let sorted = samples.sorted()
        n = sorted.count
        guard n > 0 else { return }
        min = sorted[0]
        max = sorted[n - 1]
        mean = sorted.reduce(0, +) / Double(n)
        p50 = BenchSummary.percentile(sorted, 0.5)
        p90 = BenchSummary.percentile(sorted, 0.9)
        p99 = BenchSummary.percentile(sorted, 0.99)
    }

    /// The value at rank `ceil(p * n)`; the epsilon keeps 0.07 * 100 from rounding up to rank 8.
    static func percentile(_ sorted: [Double], _ p: Double) -> Double {
        guard !sorted.isEmpty else { return .nan }
        let rank = Swift.min(sorted.count, Swift.max(1, Int((p * Double(sorted.count) - 1e-9).rounded(.up))))
        return sorted[rank - 1]
    }

    /// The summary as the result file records it. A summary of nothing has no numbers to write.
    var json: [String: Any] {
        return ["n": n, "min": min, "p50": p50, "p90": p90, "p99": p99, "max": max, "mean": mean]
    }
}

/// One measured operation, as a row of the result file (nanoseconds per operation).
func benchOp(id: String, batch: Int, perOpNs: [Double], note: String) -> [String: Any] {
    let s = BenchSummary(samples: perOpNs)
    return [
        "id": id, "mode": batch == 1 ? "each" : "batched", "batch": batch, "samples": s.n,
        "min": s.min, "p50": s.p50, "p90": s.p90, "p99": s.p99, "max": s.max, "mean": s.mean, "note": note,
    ]
}

/// A check of the benchmark itself failed: the numbers of this run are not to be trusted, so none is reported.
struct BenchCheckError: Error, CustomStringConvertible {
    let message: String
    var description: String { "device bench check failed: \(message)" }
}
