import Foundation
import XCTest
@testable import UndraRuntime

/// The standard ports' ids and hand-written codecs (docs/SPEC.md section 8), the `RuntimeConfig`
/// record and the statistics parser.
final class StandardPortTests: XCTestCase {
    // MARK: Ids

    func testTheStandardPortIdsMatchTheGeneratedGoldenIds() {
        // Values taken from crates/undra-bindgen/tests/golden/full/swift/.../Ids.swift.
        XCTAssertEqual(StandardPorts.Clock.portId, 0xcd99_c48e)
        XCTAssertEqual(StandardPorts.Clock.nowMs, 0xccc9_4d90)
        XCTAssertEqual(StandardPorts.Clock.monotonicNs, 0x2cb2_b4bf)
        XCTAssertEqual(StandardPorts.Http.portId, 0x1ebe_b908)
        XCTAssertEqual(StandardPorts.Http.request, 0x6b14_df26)
        XCTAssertEqual(StandardPorts.Kv.portId, 0x5389_110d)
        XCTAssertEqual(StandardPorts.Kv.get, 0xf050_bb1a)
        XCTAssertEqual(StandardPorts.Kv.set, 0x6242_7856)
        XCTAssertEqual(StandardPorts.Connectivity.portId, 0x1fef_f6ff)
        XCTAssertEqual(StandardPorts.Connectivity.changed, 0xb4f2_a010)
    }

    func testTheOtherIdsFollowTheNamingRule() {
        XCTAssertEqual(StandardPorts.Log.portId, fnv1a32("port.Log"))
        XCTAssertEqual(StandardPorts.Log.log, fnv1a32("Log.log"))
        XCTAssertEqual(StandardPorts.Rng.fill, fnv1a32("Rng.fill"))
        XCTAssertEqual(StandardPorts.Timer.set, fnv1a32("Timer.set"))
        XCTAssertEqual(StandardPorts.SecureStore.portId, fnv1a32("port.SecureStore"))
        XCTAssertEqual(StandardPorts.SecureStore.list, fnv1a32("SecureStore.list"))
        XCTAssertEqual(StandardPorts.Fs.write, fnv1a32("Fs.write"))
        XCTAssertEqual(StandardPorts.Lifecycle.changed, fnv1a32("Lifecycle.changed"))
        XCTAssertEqual(StandardPorts.Log.portId, 0x575f_f24a)
        XCTAssertEqual(StandardPorts.Timer.set, 0x923a_766c)
    }

    // MARK: Http

    func testHttpRequestEncodesFieldByFieldInDeclarationOrder() {
        let minimal = HttpRequest(method: .get, url: "u", headers: [Header(name: "a", value: "b")], body: nil, timeoutMs: nil)
        assertCodec(minimal, hex: "0000 01000000 75 01000000 01000000 61 01000000 62 00 00")
        let full = HttpRequest(method: .patch, url: "u", headers: [], body: [1, 2], timeoutMs: 5000)
        assertCodec(full, hex: "0400 01000000 75 00000000 01 02000000 0102 01 88130000")
    }

    func testHttpMethodIndicesAndNames() {
        XCTAssertEqual(HttpMethod.allCases.map { $0.rawValue }, [0, 1, 2, 3, 4, 5, 6])
        XCTAssertEqual(HttpMethod.allCases.map { $0.name }, ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"])
        expectWireError(.invalidTag(tag: 7, at: 0, type: "HttpMethod")) {
            _ = try HttpMethod.undraDecoded(from: [7, 0])
        }
    }

    func testHttpResponseEncoding() {
        let response = HttpResponse(status: 200, headers: [], body: [1, 2, 3])
        assertCodec(response, hex: "c800 00000000 03000000 010203")
        let withHeader = HttpResponse(status: 404, headers: [Header(name: "k", value: "")], body: [])
        assertCodec(withHeader, hex: "9401 01000000 01000000 6b 00000000 00000000")
    }

    func testHttpErrorEncoding() {
        assertCodec(HttpError.network("x"), hex: "0000 01000000 78")
        assertCodec(HttpError.timeout, hex: "0100")
        assertCodec(HttpError.cancelled, hex: "0200")
        assertCodec(HttpError.invalidUrl("u"), hex: "0300 01000000 75")
        expectWireError(.invalidTag(tag: 4, at: 0, type: "HttpError")) {
            _ = try HttpError.undraDecoded(from: [4, 0])
        }
    }

    func testHttpRecordsRoundTripUnicodeAndLargeBodies() throws {
        let big = [UInt8](repeating: 0xAB, count: 100_000)
        let request = HttpRequest(
            method: .post,
            url: "https://example.com/caf\u{E9}/\u{1F30A}",
            headers: [Header(name: "X-\u{E9}", value: "\u{1F30A}"), Header(name: "", value: "")],
            body: big,
            timeoutMs: UInt32.max
        )
        XCTAssertEqual(try HttpRequest.undraDecoded(from: request.undraEncoded()), request)
        let response = HttpResponse(status: 65535, headers: [], body: big)
        XCTAssertEqual(try HttpResponse.undraDecoded(from: response.undraEncoded()), response)
    }

    func testTruncatedHttpRecordsThrowWireErrors() {
        let bytes = HttpRequest(method: .get, url: "u", headers: [], body: [1], timeoutMs: 1).undraEncoded()
        for cut in 0 ..< bytes.count {
            XCTAssertThrowsError(try HttpRequest.undraDecoded(from: Array(bytes[0 ..< cut])), "cut \(cut)") { error in
                XCTAssertTrue(error is WireError, "\(error)")
            }
        }
    }

    // MARK: Fs, events, lifecycle

    func testFsErrorEncoding() {
        assertCodec(FsError.notFound, hex: "0000")
        assertCodec(FsError.denied, hex: "0100")
        assertCodec(FsError.io("e"), hex: "0200 01000000 65")
        expectWireError(.invalidTag(tag: 3, at: 0, type: "FsError")) {
            _ = try FsError.undraDecoded(from: [3, 0])
        }
    }

    func testNetKindAndAppStateIndices() {
        XCTAssertEqual(NetKind.allCases.map { $0.rawValue }, [0, 1, 2, 3, 4])
        assertCodec(NetKind.wired, hex: "0200")
        assertCodec(NetKind.disconnected, hex: "0400")
        XCTAssertEqual(UndraAppState.allCases.map { $0.rawValue }, [0, 1, 2])
        assertCodec(UndraAppState.background, hex: "0200")
        expectWireError(.invalidTag(tag: 5, at: 0, type: "NetKind")) {
            _ = try NetKind.undraDecoded(from: [5, 0])
        }
        expectWireError(.invalidTag(tag: 3, at: 0, type: "AppState")) {
            _ = try UndraAppState.undraDecoded(from: [3, 0])
        }
    }

    func testConnectivityAndLifecycleParameterEncodings() {
        XCTAssertEqual(bytesToHex(ConnectivityAdapter.encodeChanged(online: true, kind: .cellular)), "010100")
        XCTAssertEqual(bytesToHex(ConnectivityAdapter.encodeChanged(online: false, kind: .disconnected)), "000400")
        XCTAssertEqual(bytesToHex(UndraLifecycle.encodeChanged(.inactive)), "0100")
    }

    // MARK: RuntimeConfig

    func testRuntimeConfigMatchesTheCoresOwnTestVector() throws {
        // The bytes asserted by `config_layout_is_platform_mode_then_three_bytes` in undra-runtime.
        let config = RuntimeConfigRecord(platform: "web", mode: "dev", coreThreads: 1, blockingThreads: 2, logLevel: 3)
        XCTAssertEqual(config.undraEncoded(), [3, 0, 0, 0, 0x77, 0x65, 0x62, 3, 0, 0, 0, 0x64, 0x65, 0x76, 1, 2, 3])
        XCTAssertEqual(try RuntimeConfigRecord.undraDecoded(from: config.undraEncoded()), config)
        for cut in 0 ..< config.undraEncoded().count {
            XCTAssertThrowsError(try RuntimeConfigRecord.undraDecoded(from: Array(config.undraEncoded()[0 ..< cut])))
        }
    }

    // MARK: Statistics

    func testStatsParseTheCoresDocument() {
        let json = "{\"platform\":\"ios\",\"mode\":\"inproc\",\"schema_hash\":\"0x691eee0733e4a44f\","
            + "\"live_handles\":3,\"live_stores\":2,\"poisoned_stores\":0,\"tasks\":5,\"active_calls\":1,"
            + "\"open_streams\":1,\"pending_port_calls\":0,\"abandoned_port_calls\":0,\"pending_timers\":2,"
            + "\"blocking_threads\":{\"started\":1,\"max\":4},\"transactions\":42,\"panics\":0,\"turns\":9,\"polls\":10,"
            + "\"crossings\":{\"calls\":7,\"replies\":6,\"change_sets\":5,\"change_set_bytes\":1234,\"port_calls\":0,"
            + "\"port_replies\":0,\"stream_items\":3,\"events\":1,\"bad_requests\":0,\"cancelled\":0}}"
        let stats = UndraStats(json: json)
        XCTAssertEqual(stats.coreLiveHandles, 3)
        XCTAssertEqual(stats.coreLiveStores, 2)
        XCTAssertEqual(stats.coreTasks, 5)
        XCTAssertEqual(stats.coreActiveCalls, 1)
        XCTAssertEqual(stats.coreOpenStreams, 1)
        XCTAssertEqual(stats.coreTransactions, 42)
        XCTAssertEqual(stats.corePanics, 0)
        XCTAssertEqual(stats.values["blocking_threads.max"], 4)
        XCTAssertEqual(stats.values["crossings.change_set_bytes"], 1234)
        XCTAssertNil(stats.values["platform"], "strings are not numbers")
        XCTAssertNil(stats.values["schema_hash"])
        XCTAssertEqual(stats.json, json)
    }

    func testTheStatsWalkerHandlesEscapesArraysNegativesAndFractions() {
        let json = " { \"a\" : -5 , \"b\\\"c\" : 7, \"list\": [1, [2, {\"x\": 3}], \"]\"], \"f\": 1.5, \"e\": 2e3, \"t\": true, \"n\": null, \"z\": {} , \"after\": 9 } "
        let values = UndraStats.flattenIntegers(in: json)
        XCTAssertEqual(values["a"], -5)
        XCTAssertEqual(values["b\"c"], 7)
        XCTAssertNil(values["list"])
        XCTAssertNil(values["x"])
        XCTAssertNil(values["f"])
        XCTAssertNil(values["e"])
        XCTAssertEqual(values["after"], 9)
    }

    func testTheStatsWalkerSurvivesGarbageAndKeepsWhatItRead() {
        XCTAssertEqual(UndraStats.flattenIntegers(in: ""), [:])
        XCTAssertEqual(UndraStats.flattenIntegers(in: "not json"), [:])
        XCTAssertEqual(UndraStats.flattenIntegers(in: "{\"a\":1,\"b\":"), ["a": 1])
        XCTAssertEqual(UndraStats.flattenIntegers(in: "{\"a\":1,,}"), ["a": 1])
        XCTAssertEqual(UndraStats.flattenIntegers(in: "{\"big\":99999999999999999999999}"), [:])
        XCTAssertEqual(UndraStats.flattenIntegers(in: "{\"a\":\"unterminated"), [:])
        XCTAssertEqual(UndraStats.flattenIntegers(in: "[1,2,3]"), [:])
    }

    func testEmptyStatsAreAllZero() {
        let stats = UndraStats()
        XCTAssertEqual(stats.json, "")
        XCTAssertEqual(stats.coreLiveHandles, 0)
        XCTAssertEqual(stats.hostLiveHandles, 0)
        XCTAssertEqual(stats.values, [:])
    }
}
