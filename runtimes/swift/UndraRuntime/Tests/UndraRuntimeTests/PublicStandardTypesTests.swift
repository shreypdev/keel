import Foundation
import XCTest
// A plain `import`, not `@testable`: this file sees only what the runtime exports, so it stops
// compiling if any standard type, initializer, case or conformance an app needs becomes internal.
import UndraRuntime

/// The records of the standard ports as an app and generated bindings see them: public,
/// unprefixed, `Sendable`, with the conformances generated code relies on (ADR-024, amended).
final class PublicStandardTypesTests: XCTestCase {
    // MARK: Building and reading them from outside the module

    func testARequestIsBuiltFromItsPublicInitializer() throws {
        let full = HttpRequest(
            method: .post,
            url: "https://example.com/todos",
            headers: [Header(name: "content-type", value: "application/json")],
            body: Array("{\"title\":\"milk\"}".utf8),
            timeoutMs: 5_000
        )
        XCTAssertEqual(full.method.name, "POST")
        XCTAssertEqual(full.headers.first?.value, "application/json")
        XCTAssertEqual(full.timeoutMs, 5_000)

        // Headers, body and timeout default to nothing, like `HttpRequest::new` in Rust and the
        // defaults of the Kotlin data class.
        let bare = HttpRequest(method: .get, url: "https://example.com/")
        XCTAssertEqual(bare.headers, [])
        XCTAssertNil(bare.body)
        XCTAssertNil(bare.timeoutMs)

        var mutable = bare
        mutable.method = .delete
        mutable.body = [1]
        XCTAssertEqual(mutable.method, .delete)
        XCTAssertNotEqual(mutable, bare)
        XCTAssertEqual(try HttpRequest.undraDecoded(from: full.undraEncoded()), full)
    }

    func testAResponseAndItsHeadersRoundTrip() throws {
        let response = HttpResponse(status: 204, headers: [Header(name: "etag", value: "x")], body: [])
        XCTAssertEqual(try HttpResponse.undraDecoded(from: response.undraEncoded()), response)
        let empty = HttpResponse(status: 200)
        XCTAssertEqual(empty.headers, [])
        XCTAssertEqual(empty.body, [])
        XCTAssertEqual(try Header.undraDecoded(from: Header(name: "a", value: "b").undraEncoded()), Header(name: "a", value: "b"))
    }

    func testEveryCaseOfTheEnumsIsNameable() throws {
        XCTAssertEqual(
            HttpMethod.allCases,
            [.get, .post, .put, .delete, .patch, .head, .options]
        )
        XCTAssertEqual(NetKind.allCases, [.wifi, .cellular, .wired, .unknown, .disconnected])
        XCTAssertEqual(UndraAppState.allCases, [.active, .inactive, .background])
        // `NetKind?` has no ambiguous `.none`: the case that means "no network" is `.disconnected`.
        let missing: NetKind? = nil
        XCTAssertNil(missing)
        XCTAssertNotNil(NetKind?.some(.disconnected))
        for kind in NetKind.allCases {
            XCTAssertEqual(try NetKind.undraDecoded(from: kind.undraEncoded()), kind)
        }
    }

    // MARK: Errors

    func testTheErrorsCarryTheMessagesOfTheRustErrorAttributes() {
        XCTAssertEqual(HttpError.network("dns").description, "network error: dns")
        XCTAssertEqual(HttpError.timeout.description, "the request timed out")
        XCTAssertEqual(HttpError.cancelled.description, "the request was cancelled")
        XCTAssertEqual(HttpError.invalidUrl("x y").description, "invalid URL: x y")
        XCTAssertEqual(FsError.notFound.description, "not found")
        XCTAssertEqual(FsError.denied.description, "access denied")
        XCTAssertEqual(FsError.io("disk full").description, "I/O error: disk full")
        // A `LocalizedError`, as every generated error is.
        XCTAssertEqual(HttpError.timeout.localizedDescription, "the request timed out")
        XCTAssertEqual(FsError.io("disk full").localizedDescription, "I/O error: disk full")
        XCTAssertEqual((FsError.denied as any Error).localizedDescription, "access denied")
        // ADR-049's additions.
        XCTAssertEqual(FsError.full.description, "the disk is full")
        XCTAssertEqual(FsError.unavailable("no root").description, "the file system is unavailable: no root")
        XCTAssertEqual(StorageError.unavailable("needs IndexedDB").description, "storage is unavailable: needs IndexedDB")
        XCTAssertEqual(StorageError.full.description, "the storage is full")
        XCTAssertEqual(StorageError.locked.description, "the storage is locked")
        XCTAssertEqual(StorageError.corrupt("bad header").description, "stored data is corrupt: bad header")
        XCTAssertEqual(StorageError.io("EIO").description, "storage I/O error: EIO")
        XCTAssertEqual((StorageError.locked as any Error).localizedDescription, "the storage is locked")
    }

    func testAnAppAnswersAStorageFailureWithAPortError() throws {
        // How an app's own `Kv` or `SecureStore` implementation reports a failure: the encoded
        // `StorageError` in an `UndraPortError`, which the runtime answers with port status 1.
        let failure = UndraPortError(body: StorageError.full.undraEncoded())
        XCTAssertEqual(try StorageError.undraDecoded(from: failure.body), .full)
        for error in [StorageError.unavailable("u"), .full, .locked, .corrupt("c"), .io("i")] {
            XCTAssertEqual(try StorageError.undraDecoded(from: error.undraEncoded()), error)
        }
    }

    func testTheErrorsAreTheDomainOfAGeneratedCall() {
        // What a generated method does with a failed call (`UndraCallError.mapped(_:domain:)`):
        // the standard errors are `UndraError`s, so they work as a call's typed error.
        let body = HttpError.invalidUrl("nope").undraEncoded()
        let reply = UndraReplyError(status: .error, body: body)
        let mapped = UndraCallError.mapped(reply, domain: HttpError.self)
        XCTAssertEqual(mapped as? HttpError, .invalidUrl("nope"))
        let fs = UndraCallError.mapped(UndraReplyError(status: .error, body: FsError.notFound.undraEncoded()), domain: FsError.self)
        XCTAssertEqual(fs as? FsError, .notFound)
        let storage = UndraCallError.mapped(
            UndraReplyError(status: .error, body: StorageError.corrupt("x").undraEncoded()),
            domain: StorageError.self
        )
        XCTAssertEqual(storage as? StorageError, .corrupt("x"))
    }

    // MARK: Conformances generated code relies on

    func testTheTypesAreCodableForTheStructsThatHoldThem() throws {
        // A generated record that holds a standard type derives `Codable`, so the standard
        // types must be.
        let request = HttpRequest(method: .put, url: "u", headers: [Header(name: "a", value: "b")], body: [1, 2], timeoutMs: 9)
        let data = try JSONEncoder().encode(request)
        XCTAssertEqual(try JSONDecoder().decode(HttpRequest.self, from: data), request)
        let response = HttpResponse(status: 500, headers: [], body: [7])
        XCTAssertEqual(try JSONDecoder().decode(HttpResponse.self, from: JSONEncoder().encode(response)), response)
        for kind in NetKind.allCases {
            XCTAssertEqual(try JSONDecoder().decode([NetKind].self, from: JSONEncoder().encode([kind])), [kind])
        }
        for state in UndraAppState.allCases {
            XCTAssertEqual(try JSONDecoder().decode([UndraAppState].self, from: JSONEncoder().encode([state])), [state])
        }
    }

    func testTheTypesAreSendableAndHashable() async {
        let request = HttpRequest(method: .get, url: "u")
        let error = HttpError.timeout
        // Crossing a task boundary compiles only for `Sendable` values (Swift 6 language mode).
        let (echoed, failure) = await Task.detached { (request, error) }.value
        XCTAssertEqual(echoed, request)
        XCTAssertEqual(failure, .timeout)
        XCTAssertEqual(Set([HttpMethod.get, .get, .post]).count, 2)
        XCTAssertEqual(Set([Header(name: "a", value: "1"), Header(name: "a", value: "1")]).count, 1)
        XCTAssertEqual(Set([FsError.notFound, .notFound, .denied, .full, .unavailable("a")]).count, 4)
        XCTAssertEqual(Set([StorageError.full, .full, .locked, .io("a"), .io("a"), .io("b")]).count, 4)
        let locked = await Task.detached { StorageError.locked }.value
        XCTAssertEqual(locked, .locked)
    }

    func testAnAppCanDeclareItsOwnTypeWithAStandardName() {
        // A declaration closer than the import wins: here a local `Header`, in generated code a
        // module's own `Header` or `HttpRequest` (an app record that only shares a standard name),
        // which Swift prefers over an imported type of the same name inside that module. This test
        // shows the nearer-scope rule and that the runtime's type can always be named in full; it
        // does not compile a second module.
        struct Header { var text: String }
        XCTAssertEqual(Header(text: "mine").text, "mine")
        XCTAssertEqual(UndraRuntime.Header(name: "a", value: "b").name, "a")
    }
}
