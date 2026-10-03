import Foundation
import XCTest
@testable import UndraRuntime

// How fast the default Sse adapter reads a response body (ADR-047, "Amendment: the Swift adapter delivers
// chunks, 2026-10-02"), against the local server of PortsV2ReviewTests.

/// The server, shared with the other realtime suites.
private func sharedServer() throws -> RealtimeServer {
    if RealtimeAdapterServer.current == nil {
        RealtimeAdapterServer.current = try RealtimeServer.start()
    }
    return RealtimeAdapterServer.current!
}

/// How fast event streams come through the default adapter, against the local server's `/sse/flood`.
///
/// The numbers are printed (`BENCH swift sse/...`, recorded by CI beside the other BENCH lines of this package) and not
/// asserted: how many events a second a machine reads is the machine's. What the test asserts is what does not depend on it:
/// every event arrived, in order.
///
/// The sizes are chosen for what each shows. At 64 bytes an event the limit is URLSession's own receive path, which no
/// adapter can pass: a delegate that does nothing at all read 20,000 such events at about 30,000 a second (100,000 of 49 bytes
/// at 11,000), and a profile of it shows CFNetwork's `conCatData` joining the small reads of a flood into one buffer, at a cost
/// that grows with the buffer. From 512 bytes the adapter's own cost shows; at 4,096 it is what a chunk costs per byte.
final class SseThroughputTests: XCTestCase {
    /// What the server writes for one event of `/sse/flood`: `id: <i>\ndata: <i padded with dots to size>\n\n`.
    private static func wireBytes(index: Int, size: Int) -> Int {
        return "id: \(index)\n".utf8.count + "data: ".utf8.count + max(size, "\(index)".utf8.count) + 2
    }

    /// One pass over `/sse/flood`: `n` events of `size` bytes of data, read straight from the adapter's stream (what the binding's
    /// pump reads, without the binding).
    private func readThroughTheAdapter(_ server: RealtimeServer, n: Int, size: Int) async throws -> (seconds: Double, bytes: Int) {
        let adapter = URLSessionSseAdapter()
        let url = "\(server.http)/sse/flood?n=\(n)&size=\(size)"
        let started = DispatchTime.now().uptimeNanoseconds
        let stream = try await adapter.open(url: url, headers: [], lastEventId: nil)
        var count = 0
        var bytes = 0
        do {
            for try await event in stream.events {
                XCTAssertEqual(event.id, "\(count)", "in order")
                bytes += SseThroughputTests.wireBytes(index: count, size: size)
                count += 1
            }
            XCTFail("the flood ended without .ended")
        } catch let error as SseError {
            XCTAssertEqual(error, .ended)
        }
        let finished = DispatchTime.now().uptimeNanoseconds
        XCTAssertEqual(count, n, "every event arrived")
        return (Double(finished - started) / 1e9, bytes)
    }

    /// The same, through the binding: `next(max: 16)` until the end (the pump, the buffer and the burst window included).
    private func readThroughTheBinding(_ server: RealtimeServer, n: Int, size: Int) async throws -> (seconds: Double, bytes: Int) {
        let binding = SseBinding(adapter: URLSessionSseAdapter())
        defer { binding.detach() }
        let url = "\(server.http)/sse/flood?n=\(n)&size=\(size)"
        let started = DispatchTime.now().uptimeNanoseconds
        let stream = try await binding.open(url: url, headers: [], lastEventId: nil)
        var count = 0
        var bytes = 0
        while true {
            do {
                for event in try await binding.next(stream: stream, max: 16) {
                    XCTAssertEqual(event.id, "\(count)", "in order")
                    bytes += SseThroughputTests.wireBytes(index: count, size: size)
                    count += 1
                }
            } catch {
                XCTAssertEqual(error, .ended)
                break
            }
        }
        let finished = DispatchTime.now().uptimeNanoseconds
        XCTAssertEqual(count, n, "every event arrived")
        return (Double(finished - started) / 1e9, bytes)
    }

    /// The best of `rounds` passes (a pass is as slow as the machine's worst moment, and as fast as its best).
    private func best(_ rounds: Int, _ pass: () async throws -> (seconds: Double, bytes: Int)) async throws -> (seconds: Double, bytes: Int) {
        var fastest: (seconds: Double, bytes: Int)?
        for _ in 0 ..< rounds {
            let result = try await pass()
            if fastest == nil || result.seconds < fastest!.seconds {
                fastest = result
            }
        }
        return fastest!
    }

    func testEventsAndBytesPerSecondThroughTheAdapter() async throws {
        let server = try sharedServer()
        _ = try await readThroughTheAdapter(server, n: 2_000, size: 512) // warms the connection and the code
        let rounds = 2
        let small = try await best(rounds) { try await self.readThroughTheAdapter(server, n: 20_000, size: 64) }
        let medium = try await best(rounds) { try await self.readThroughTheAdapter(server, n: 20_000, size: 512) }
        let large = try await best(rounds) { try await self.readThroughTheAdapter(server, n: 2_000, size: 4_096) }
        let binding = try await best(rounds) { try await self.readThroughTheBinding(server, n: 20_000, size: 512) }
        func line(_ name: String, _ events: Int, _ pass: (seconds: Double, bytes: Int), _ note: String = "") {
            print(String(format: "BENCH swift sse/%@ %.0f events/s (%.1f MB/s, %d events of %d bytes%@)", name,
                         Double(events) / pass.seconds, Double(pass.bytes) / pass.seconds / 1e6, events, pass.bytes / events, note))
        }
        line("adapter_64B", 20_000, small)
        line("adapter_512B", 20_000, medium)
        line("adapter_4096B", 2_000, large)
        line("binding_512B", 20_000, binding, ", next(max: 16)")
    }
}
