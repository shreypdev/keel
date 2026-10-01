import XCTest
@testable import UndraTestKit
import UndraRuntime

// testkit/conformance/fakes.json is what the Rust fakes answer (generated from them, checked in CI). Every kit replays it against its own
// fakes, so the four implementations cannot drift apart.

private func render(_ j: JSON) -> String {
    switch j {
    case .string(let s): return jsonString(s)
    case .number(let n): return n
    case .bool(let b): return b ? "true" : "false"
    case .null: return "null"
    case .array(let items): return "[" + items.map(render).joined(separator: ",") + "]"
    case .object(let fields): return "{" + fields.keys.sorted().map { "\(jsonString($0)):\(render(fields[$0]!))" }.joined(separator: ",") + "}"
    }
}

private func parse(_ text: String) -> JSON {
    return (try? parseJSON(text)) ?? .null
}

private func errorJSON(_ error: Error) -> String {
    switch error {
    case FsError.notFound: return "{\"error\":\"not_found\"}"
    case FsError.denied: return "{\"error\":\"denied\"}"
    case FsError.io(let message): return "{\"error\":\"io\",\"message\":\(jsonString(message))}"
    case HttpError.network(let message): return "{\"error\":\"network\",\"message\":\(jsonString(message))}"
    case HttpError.timeout: return "{\"error\":\"timeout\"}"
    case HttpError.cancelled: return "{\"error\":\"cancelled\"}"
    case HttpError.invalidUrl(let url): return "{\"error\":\"invalid_url\",\"message\":\(jsonString(url))}"
    default: return "{\"error\":\"unexpected \(error)\"}"
    }
}

private func list(_ items: [String]) -> String {
    return "[" + items.map(jsonString).joined(separator: ",") + "]"
}

final class ConformanceTests: XCTestCase {
    private func section(_ name: String) throws -> [[String: JSON]] {
        let doc = try parseJSON(try fixture("conformance/fakes.json")).object ?? [:]
        return (doc[name]?.array ?? []).compactMap { $0.object }
    }

    private func same(_ expected: JSON?, _ actual: String, _ what: String, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(render(expected ?? .null), render(parse(actual)), what, file: file, line: line)
    }

    func testSeededRngGivesTheSameBytesForTheSameSeed() throws {
        for c in try section("rng") {
            let seed: UInt64
            switch c["seed"] {
            case .string(let hex)?: seed = parseHex64(hex)!
            case .number(let raw)?: seed = UInt64(raw)!
            default: return XCTFail("no seed")
            }
            let rng = SeededRng(seed: seed)
            let got = (c["fills"]?.array ?? []).map { rng.fill(UInt32($0.int!)).hex }
            same(c["results"], list(got), "seed \(seed)")
        }
    }

    func testFakeClockFiresTimersInDeadlineOrderWithTheClockAtEachDeadline() throws {
        for c in try section("clock") {
            let clock = FakeClock(nowMs: c["now_ms"]!.int!)
            func state() -> String { "{\"now_ms\":\(clock.nowMs),\"monotonic_ns\":\(clock.monotonicNs)}" }
            for step in (c["steps"]?.array ?? []).compactMap({ $0.object }) {
                switch step["op"]?.string {
                case "timer":
                    clock.set(timerId: UInt32(step["id"]!.int!), delayMs: UInt64(step["delay_ms"]!.int!))
                case "advance":
                    let fired = clock.advance(ms: step["ms"]!.int!)
                    same(step["fired"], "[" + fired.map(String.init).joined(separator: ",") + "]", "fired")
                    same(step["state"], state(), "state after advance")
                case "set_now":
                    clock.setNowMs(step["ms"]!.int!)
                    same(step["state"], state(), "state after set_now")
                default:
                    XCTFail("unknown op")
                }
            }
        }
    }

    func testMemKvStoresBytesOrderedByUTF8NotUTF16() async throws {
        for c in try section("store") {
            let kv = MemKv()
            for step in (c["steps"]?.array ?? []).compactMap({ $0.object }) {
                switch step["op"]?.string {
                case "set": await kv.set(step["key"]!.string!, step["value"]!.string!.fromHex()!)
                case "delete": await kv.delete(step["key"]!.string!)
                case "list": same(step["result"], list(await kv.list(step["prefix"]!.string!)), "list \(step["prefix"]!.string!)")
                case "get":
                    let value = await kv.get(step["key"]!.string!)
                    same(step["result"], value.map { jsonString($0.hex) } ?? "null", "get \(step["key"]!.string!)")
                default: XCTFail("unknown op")
                }
            }
        }
    }

    func testMemFsHasTheSemanticsThePlatformAdaptersShare() throws {
        for c in try section("fs") {
            let fs = MemFs()
            for step in (c["steps"]?.array ?? []).compactMap({ $0.object }) {
                let path = (step["path"] ?? step["dir"])!.string!
                let got: String
                do {
                    switch step["op"]?.string {
                    case "write": try fs.write(path, step["data"]!.string!.fromHex()!); got = "null"
                    case "read": got = jsonString(try fs.read(path).hex)
                    case "delete": try fs.delete(path); got = "null"
                    default: got = list(try fs.list(path))
                    }
                } catch {
                    got = errorJSON(error)
                }
                same(step["result"], got, "\(step["op"]?.string ?? "?") \(path)")
            }
        }
    }

    func testFakeHttpAnswersTheFirstMatchingRuleAndFailsTheRest() async throws {
        for c in try section("http") {
            let fakes = Fakes()
            try Seed(json: "{\"http\":\(render(c["rules"]!))}").apply(to: fakes)
            for r in (c["requests"]?.array ?? []).compactMap({ $0.object }) {
                let method = HttpMethod.allCases.first { $0.name.lowercased() == r["method"]!.string! }!
                let got: String
                do {
                    let resp = try await fakes.http.request(HttpRequest(method: method, url: r["url"]!.string!))
                    let headers = resp.headers.map { "[\(jsonString($0.name)),\(jsonString($0.value))]" }.joined(separator: ",")
                    got = "{\"status\":\(resp.status),\"headers\":[\(headers)],\"body\":\(jsonString(resp.body.hex))}"
                } catch {
                    got = errorJSON(error)
                }
                same(r["result"], got, "\(r["method"]!.string!) \(r["url"]!.string!)")
            }
        }
    }

    func testTheExampleSeedReadsAndSeedsTheFakes() async throws {
        let seed = try Seed(json: try fixture("fixtures/seed.json"))
        XCTAssertEqual(seed.nowMs, 1_700_000_000_000)
        XCTAssertEqual(seed.rngSeed, 42)
        let fakes = Fakes()
        try seed.apply(to: fakes)
        XCTAssertEqual(String(decoding: fakes.kv.value("greeting") ?? [], as: UTF8.self), "hello")
        XCTAssertEqual(fakes.kv.value("blob")?.hex, "00ff")
        XCTAssertEqual(String(decoding: fakes.secureStore.value("token") ?? [], as: UTF8.self), "t-123")
        XCTAssertEqual(String(decoding: fakes.fs.contents("notes/a.txt") ?? [], as: UTF8.self), "hello")
        XCTAssertEqual(fakes.rng.fill(8).hex, "a0a39b71b74ace56")
        XCTAssertEqual(fakes.clock.nowMs, 1_700_000_000_000)
        let resp = try await fakes.http.request(HttpRequest(method: .get, url: "https://api.test/lists/inbox/todos"))
        XCTAssertEqual(resp.status, 200)
        XCTAssertEqual(resp.headers.first?.value, "application/json")
    }

    func testASeedNamesThePathOfTheFirstBadValue() {
        func path(_ text: String) -> String {
            do {
                _ = try Seed(json: text)
                return "no error"
            } catch let error as SeedError {
                return error.path
            } catch {
                return "\(error)"
            }
        }
        XCTAssertEqual(path("{\"kv\": {\"a\": 1}}"), "kv.a")
        XCTAssertEqual(path("{\"http\": [{\"status\": \"x\"}]}"), "http[0].status")
        XCTAssertEqual(path("{\"http\": [{\"method\": \"fetch\"}]}"), "http[0].method")
        XCTAssertEqual(path("{\"connectivity\": {\"online\": true, \"kind\": \"5g\"}}"), "connectivity.kind")
        XCTAssertEqual(path("{\"lifecycle\": 3}"), "lifecycle")
        XCTAssertEqual(path("[]"), "$")
        XCTAssertEqual(path("{\"version\": 2}"), "version")
        XCTAssertEqual(path("{"), "$")
        XCTAssertThrowsError(try Seed(json: "{\"fs\": {\"../x\": \"y\"}}").apply(to: Fakes())) { error in
            XCTAssertEqual((error as? SeedError)?.path, "fs.../x")
        }
    }
}
