// `FsAdapter` stays inside its root (docs/SPEC.md section 8): a path that would leave it through
// `..` or a symbolic link is `Denied`, whatever the operation, wherever in the path the link is.
// The repro is the one in `.10x/decisions/sde/rn-adapters.md`, finding 1.

import Foundation
import XCTest

@testable import UndraRuntime

final class FsConfinementTests: XCTestCase {
    /// `<tmp>/root` (the adapter's root), `<tmp>/outside/secret.txt` (`outside`) and the link
    /// `<tmp>/root/out -> <tmp>/outside`.
    private struct Fixture {
        let base: URL
        let root: URL
        let outside: URL
        let secret: URL

        func exists(_ url: URL) -> Bool {
            return FileManager.default.fileExists(atPath: url.path)
        }

        func link(_ name: String, to target: URL) throws {
            try FileManager.default.createSymbolicLink(
                atPath: root.appendingPathComponent(name).path,
                withDestinationPath: target.path
            )
        }

        func outsideEntries() throws -> [String] {
            return try FileManager.default.contentsOfDirectory(atPath: outside.path).sorted()
        }
    }

    private func makeFixture() throws -> Fixture {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("undra-fs-confinement-" + UUID().uuidString, isDirectory: true)
        addTeardownBlock {
            try? FileManager.default.removeItem(at: base)
        }
        let root = base.appendingPathComponent("root", isDirectory: true)
        let outside = base.appendingPathComponent("outside", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: outside, withIntermediateDirectories: true)
        let secret = outside.appendingPathComponent("secret.txt")
        try Data("outside".utf8).write(to: secret)
        let fixture = Fixture(base: base, root: root, outside: outside, secret: secret)
        try fixture.link("out", to: outside)
        return fixture
    }

    // MARK: Outcomes

    private enum Outcome<Value: Equatable>: Equatable {
        case value(Value)
        case error(FsError)
    }

    /// The result of `body`, an `FsError` when the adapter refused. Any other failure is a test
    /// failure: the adapter answers with typed errors, never anything else.
    private func outcome<Value: Equatable>(
        _ body: () throws -> Value,
        file: StaticString = #filePath,
        line: UInt = #line
    ) -> Outcome<Value> {
        do {
            return .value(try body())
        } catch let error as UndraPortError {
            guard let decoded = try? FsError.undraDecoded(from: error.body) else {
                XCTFail("an undecodable Fs error body", file: file, line: line)
                return .error(.io("undecodable"))
            }
            return .error(decoded)
        } catch {
            XCTFail("expected an UndraPortError, got \(error)", file: file, line: line)
            return .error(.io("\(error)"))
        }
    }

    private func read(_ path: String, _ root: URL) -> Outcome<[UInt8]> {
        return outcome { try FsAdapter.read(path, root: root) }
    }

    private func write(_ path: String, _ root: URL, _ bytes: [UInt8] = [1]) -> Outcome<Bool> {
        return outcome {
            try FsAdapter.write(path, data: bytes, root: root)
            return true
        }
    }

    private func delete(_ path: String, _ root: URL) -> Outcome<Bool> {
        return outcome {
            try FsAdapter.delete(path, root: root)
            return true
        }
    }

    private func list(_ path: String, _ root: URL) -> Outcome<[String]> {
        return outcome { try FsAdapter.list(path, root: root) }
    }

    private func isIo<Value>(_ outcome: Outcome<Value>) -> Bool {
        if case .error(.io) = outcome {
            return true
        }
        return false
    }

    // MARK: The repro

    func testReadThroughALinkedDirectoryIsDenied() throws {
        let fixture = try makeFixture()
        XCTAssertEqual(read("out/secret.txt", fixture.root), .error(.denied))
        XCTAssertEqual(read("/out/secret.txt", fixture.root), .error(.denied))
        XCTAssertEqual(read("./out/./secret.txt", fixture.root), .error(.denied))
        XCTAssertEqual(read("out//secret.txt", fixture.root), .error(.denied))
        // The same file, reached without the link, is what the link must not be a way around.
        XCTAssertEqual(try Data(contentsOf: fixture.secret), Data("outside".utf8))
    }

    func testWriteThroughALinkedDirectoryIsDeniedAndCreatesNothingOutside() throws {
        let fixture = try makeFixture()
        XCTAssertEqual(write("out/new.txt", fixture.root), .error(.denied))
        XCTAssertEqual(write("out/deeper/new.txt", fixture.root), .error(.denied))
        XCTAssertEqual(try fixture.outsideEntries(), ["secret.txt"])
        XCTAssertEqual(try Data(contentsOf: fixture.secret), Data("outside".utf8))
    }

    func testDeleteThroughALinkedDirectoryIsDeniedAndKeepsTheTarget() throws {
        let fixture = try makeFixture()
        XCTAssertEqual(delete("out/secret.txt", fixture.root), .error(.denied))
        XCTAssertTrue(fixture.exists(fixture.secret))
        try FileManager.default.createDirectory(
            at: fixture.outside.appendingPathComponent("sub"),
            withIntermediateDirectories: true
        )
        XCTAssertEqual(delete("out/sub", fixture.root), .error(.denied))
        XCTAssertTrue(fixture.exists(fixture.outside.appendingPathComponent("sub")))
    }

    func testListingThroughALinkIsDenied() throws {
        let fixture = try makeFixture()
        XCTAssertEqual(list("out", fixture.root), .error(.denied), "the link itself is the last component")
        XCTAssertEqual(list("out/", fixture.root), .error(.denied))
        try FileManager.default.createDirectory(
            at: fixture.outside.appendingPathComponent("sub"),
            withIntermediateDirectories: true
        )
        XCTAssertEqual(list("out/sub", fixture.root), .error(.denied), "the link is in the middle")
        // Listing the root shows the link as a name, and nothing about where it leads.
        XCTAssertEqual(list("", fixture.root), .value(["out"]))
    }

    // MARK: Where the link is

    func testALinkAsTheLastComponentIsDeniedAndNeverFollowed() throws {
        let fixture = try makeFixture()
        try fixture.link("secret-link", to: fixture.secret)
        XCTAssertEqual(read("secret-link", fixture.root), .error(.denied))
        XCTAssertEqual(write("secret-link", fixture.root, [9, 9]), .error(.denied))
        XCTAssertEqual(try Data(contentsOf: fixture.secret), Data("outside".utf8), "not written through")
        // A link that leads nowhere is a link too, not a missing file.
        try fixture.link("dangling", to: fixture.base.appendingPathComponent("nowhere"))
        XCTAssertEqual(read("dangling", fixture.root), .error(.denied))
        XCTAssertEqual(write("dangling", fixture.root), .error(.denied))
        XCTAssertFalse(fixture.exists(fixture.base.appendingPathComponent("nowhere")))
    }

    func testALinkInTheMiddleOfAPathIsDenied() throws {
        let fixture = try makeFixture()
        XCTAssertEqual(write("a/b/c.txt", fixture.root, [1]), .value(true))
        try FileManager.default.createSymbolicLink(
            atPath: fixture.root.appendingPathComponent("a/link").path,
            withDestinationPath: fixture.outside.path
        )
        XCTAssertEqual(read("a/link/secret.txt", fixture.root), .error(.denied))
        XCTAssertEqual(write("a/link/new.txt", fixture.root), .error(.denied))
        XCTAssertEqual(delete("a/link/secret.txt", fixture.root), .error(.denied))
        XCTAssertEqual(list("a/link", fixture.root), .error(.denied))
        XCTAssertEqual(try fixture.outsideEntries(), ["secret.txt"])
        // The real directories beside the link work.
        XCTAssertEqual(read("a/b/c.txt", fixture.root), .value([1]))
        XCTAssertEqual(list("a", fixture.root), .value(["b", "link"]))
    }

    func testALinkThatLeadsInsideTheRootIsDeniedToo() throws {
        let fixture = try makeFixture()
        XCTAssertEqual(write("real/file.txt", fixture.root, [4]), .value(true))
        try fixture.link("alias", to: fixture.root.appendingPathComponent("real"))
        XCTAssertEqual(read("alias/file.txt", fixture.root), .error(.denied), "any link, not only one that leaves")
        XCTAssertEqual(write("alias/other.txt", fixture.root), .error(.denied))
        XCTAssertEqual(read("real/file.txt", fixture.root), .value([4]))
    }

    func testDeletingALinkRemovesTheLinkAndNotItsTarget() throws {
        let fixture = try makeFixture()
        XCTAssertEqual(delete("out", fixture.root), .value(true))
        XCTAssertFalse(fixture.exists(fixture.root.appendingPathComponent("out")))
        XCTAssertTrue(fixture.exists(fixture.secret), "the target of the link is untouched")
        XCTAssertEqual(list("", fixture.root), .value([]))

        // A link inside a tree that is deleted is removed, never followed.
        XCTAssertEqual(write("tree/leaf.txt", fixture.root), .value(true))
        try FileManager.default.createSymbolicLink(
            atPath: fixture.root.appendingPathComponent("tree/inner").path,
            withDestinationPath: fixture.outside.path
        )
        XCTAssertEqual(delete("tree", fixture.root), .value(true))
        XCTAssertFalse(fixture.exists(fixture.root.appendingPathComponent("tree")))
        XCTAssertEqual(try fixture.outsideEntries(), ["secret.txt"])
    }

    func testARootThatIsItselfALinkIsFollowed() throws {
        // The root is the app's own configuration (`/var` is a link on a Mac), only what is below
        // it is walked without following.
        let fixture = try makeFixture()
        let alias = fixture.base.appendingPathComponent("alias-root")
        try FileManager.default.createSymbolicLink(atPath: alias.path, withDestinationPath: fixture.root.path)
        XCTAssertEqual(write("notes/a.txt", alias, [7]), .value(true))
        XCTAssertEqual(read("notes/a.txt", alias), .value([7]))
        XCTAssertEqual(read("notes/a.txt", fixture.root), .value([7]))
        XCTAssertEqual(list("", alias), .value(["notes", "out"]))
    }

    // MARK: Paths that are not plain

    func testDotDotAnywhereIsDeniedForEveryOperation() throws {
        let fixture = try makeFixture()
        let paths = ["..", "../secret.txt", "a/../../outside/secret.txt", "a/..", "./..", "x/y/../../../outside"]
        for path in paths {
            XCTAssertEqual(read(path, fixture.root), .error(.denied), path)
            XCTAssertEqual(write(path, fixture.root), .error(.denied), path)
            XCTAssertEqual(delete(path, fixture.root), .error(.denied), path)
            XCTAssertEqual(list(path, fixture.root), .error(.denied), path)
        }
        XCTAssertEqual(try fixture.outsideEntries(), ["secret.txt"])
        XCTAssertFalse(fixture.exists(fixture.base.appendingPathComponent("outside/new.txt")))
    }

    func testAnAbsolutePathIsRelativeToTheRoot() throws {
        let fixture = try makeFixture()
        // SPEC section 8: a leading `/` is ignored. The file `<tmp>/outside/secret.txt` is therefore
        // not what `<tmp>/outside/secret.txt` names: that is `<root>/<tmp>/outside/secret.txt`.
        XCTAssertEqual(read(fixture.secret.path, fixture.root), .error(.notFound))
        XCTAssertEqual(read("/etc/hosts", fixture.root), .error(.notFound))
        let target = fixture.outside.appendingPathComponent("abs.txt").path
        XCTAssertEqual(write(target, fixture.root, [5]), .value(true))
        XCTAssertEqual(try fixture.outsideEntries(), ["secret.txt"], "nothing outside was written")
        XCTAssertEqual(read(target, fixture.root), .value([5]))
        XCTAssertTrue(fixture.exists(URL(fileURLWithPath: fixture.root.path + target)))
        XCTAssertEqual(delete(target, fixture.root), .value(true))
    }

    func testANulByteIsATypedErrorNotATruncatedPath() throws {
        let fixture = try makeFixture()
        // A C string ends at the NUL: "notes\0/x" must not become "notes".
        XCTAssertEqual(write("notes", fixture.root), .value(true))
        for path in ["notes\0/x", "notes\0", "\0", "out\0/secret.txt", "a/b\0../../c"] {
            XCTAssertTrue(isIo(read(path, fixture.root)), path.debugDescription)
            XCTAssertTrue(isIo(write(path, fixture.root)), path.debugDescription)
            XCTAssertTrue(isIo(delete(path, fixture.root)), path.debugDescription)
            XCTAssertTrue(isIo(list(path, fixture.root)), path.debugDescription)
        }
        XCTAssertEqual(read("notes", fixture.root), .value([1]), "nothing was deleted or replaced")
    }

    func testComponentsAreSplitOnSlashBytesNotOnCharacters() throws {
        XCTAssertEqual(try FsAdapter.components(of: "/a//b/./c/"), ["a", "b", "c"])
        XCTAssertEqual(try FsAdapter.components(of: ""), [])
        XCTAssertEqual(try FsAdapter.components(of: "."), [])
        // A `/` followed by a combining mark is one Character in Swift and still a separator.
        XCTAssertEqual(try FsAdapter.components(of: "a/\u{301}b"), ["a", "\u{301}b"])
        XCTAssertEqual(try FsAdapter.components(of: "caf\u{E9}/\u{1F30A}"), ["caf\u{E9}", "\u{1F30A}"])
        // A name that merely contains dots is a name.
        XCTAssertEqual(try FsAdapter.components(of: "..a/a../...  /.hidden"), ["..a", "a..", "...  ", ".hidden"])
        XCTAssertThrowsError(try FsAdapter.components(of: "a/\u{301}/.."))
    }

    func testAMultibyteComponentRoundTripsThroughTheWalk() throws {
        let fixture = try makeFixture()
        XCTAssertEqual(write("a/\u{301}b/caf\u{E9} \u{1F30A}.txt", fixture.root, [3]), .value(true))
        XCTAssertEqual(read("a/\u{301}b/caf\u{E9} \u{1F30A}.txt", fixture.root), .value([3]))
        XCTAssertEqual(list("a", fixture.root), .value(["\u{301}b"]))
    }

    // MARK: Kinds that do not fit the operation

    func testADirectoryWhereAFileIsExpectedAndTheOtherWayRound() throws {
        let fixture = try makeFixture()
        XCTAssertEqual(write("dir/file", fixture.root, [1]), .value(true))
        XCTAssertTrue(isIo(read("dir", fixture.root)), "reading a directory")
        XCTAssertTrue(isIo(write("dir", fixture.root)), "writing over a directory")
        XCTAssertEqual(read("dir/file", fixture.root), .value([1]), "the directory is intact")
        XCTAssertTrue(isIo(list("dir/file", fixture.root)), "listing a file")
        XCTAssertTrue(isIo(read("", fixture.root)), "the root is a directory")
        XCTAssertTrue(isIo(write("", fixture.root)), "writing the root")
        XCTAssertEqual(delete("", fixture.root), .error(.denied), "deleting the root")
        XCTAssertEqual(delete(".", fixture.root), .error(.denied))
        // A file in the middle of a path: nothing is there to read or delete; writing is refused.
        XCTAssertEqual(read("dir/file/more", fixture.root), .error(.notFound))
        XCTAssertEqual(delete("dir/file/more", fixture.root), .error(.notFound))
        XCTAssertEqual(list("dir/file/more", fixture.root), .error(.notFound))
        XCTAssertTrue(isIo(write("dir/file/more", fixture.root)))
        XCTAssertEqual(read("dir/file", fixture.root), .value([1]))
    }

    func testAFifoIsNotReadAndDoesNotBlock() throws {
        let fixture = try makeFixture()
        XCTAssertEqual(mkfifo(fixture.root.appendingPathComponent("pipe").path, 0o600), 0)
        XCTAssertTrue(isIo(read("pipe", fixture.root)))
    }

    func testAMissingRootIsNotFoundForReadsAndCreatedByWrites() throws {
        let fixture = try makeFixture()
        let root = fixture.base.appendingPathComponent("later/root", isDirectory: true)
        XCTAssertEqual(read("a", root), .error(.notFound))
        XCTAssertEqual(list("", root), .error(.notFound))
        XCTAssertEqual(delete("a", root), .error(.notFound))
        XCTAssertFalse(fixture.exists(root), "reads do not create the root")
        XCTAssertEqual(write("a/b", root, [2]), .value(true))
        XCTAssertEqual(read("a/b", root), .value([2]))
    }

    // MARK: Writing

    func testAWriteIsAtomicAndLeavesNoTemporaryFile() throws {
        let fixture = try makeFixture()
        XCTAssertEqual(write("f", fixture.root, [1, 2, 3]), .value(true))
        XCTAssertEqual(write("f", fixture.root, [4]), .value(true))
        XCTAssertEqual(read("f", fixture.root), .value([4]))
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: fixture.root.path).sorted(), ["f", "out"])
        var info = stat()
        XCTAssertEqual(stat(fixture.root.appendingPathComponent("f").path, &info), 0)
        XCTAssertEqual(info.st_mode & 0o777, 0o644 & ~umaskNow(), "created with 0644 under the umask")
    }

    func testListHidesTheTemporaryFilesOfAWrite() throws {
        let fixture = try makeFixture()
        XCTAssertEqual(write("d/kept", fixture.root), .value(true))
        // What a process killed mid-write leaves, from this adapter or from the React Native module.
        try Data([1]).write(to: fixture.root.appendingPathComponent("d/.undra-tmp-0123456789abcdef"))
        XCTAssertEqual(list("d", fixture.root), .value(["kept"]))
        XCTAssertEqual(FsAdapter.temporaryPrefix, ".undra-tmp-", "the name the React Native store uses (UndraStores.cpp)")
        // Another dot file is an ordinary name.
        try Data([1]).write(to: fixture.root.appendingPathComponent("d/.hidden"))
        XCTAssertEqual(list("d", fixture.root), .value([".hidden", "kept"]))
    }

    func testAnOversizedNameIsATypedError() throws {
        let fixture = try makeFixture()
        let name = String(repeating: "n", count: 1000)
        XCTAssertTrue(isIo(write(name, fixture.root)))
        XCTAssertTrue(isIo(read(name, fixture.root)))
        XCTAssertTrue(isIo(delete(name, fixture.root)))
        XCTAssertTrue(isIo(list(name, fixture.root)))
    }

    // MARK: A link swapped in between steps

    func testALinkSwappedInWhileWritingNeverLetsAFileOutOfTheRoot() throws {
        let fixture = try makeFixture()
        try FileManager.default.removeItem(at: fixture.root.appendingPathComponent("out"))
        let swapped = fixture.root.appendingPathComponent("swap")
        let flag = StopFlag()
        let swapper = Thread {
            let manager = FileManager.default
            while !flag.stopped {
                try? manager.removeItem(at: swapped)
                try? manager.createSymbolicLink(atPath: swapped.path, withDestinationPath: fixture.outside.path)
                try? manager.removeItem(at: swapped)
                try? manager.createDirectory(at: swapped, withIntermediateDirectories: true)
            }
        }
        swapper.start()
        defer { flag.stop() }
        for index in 0 ..< 400 {
            // Each lands in the real directory or is refused; none may land through the link.
            _ = write("swap/file-\(index).txt", fixture.root, [UInt8(index & 0xFF)])
            if case .value(let bytes) = read("swap/secret.txt", fixture.root) {
                XCTFail("read \(bytes.count) bytes through the link")
            }
            _ = list("swap", fixture.root)
        }
        flag.stop()
        XCTAssertEqual(try fixture.outsideEntries(), ["secret.txt"], "no write went through the link")
    }

    // MARK: Through the port

    @MainActor
    func testTheRefusalsReachTheCoreAsTypedFsErrors() async throws {
        let fixture = try makeFixture()
        let core = try makeCore(FakeTransport())
        let impl = FsAdapter(root: fixture.root).makePortImpl(core: core)
        func denied(_ method: UInt32, _ args: [UInt8]) async {
            do {
                _ = try await PortCaller.callAsync(impl, method, args)
                XCTFail("expected Denied")
            } catch let error as UndraPortError {
                XCTAssertEqual(try? FsError.undraDecoded(from: error.body), .denied)
            } catch {
                XCTFail("\(error)")
            }
        }
        await denied(StandardPorts.Fs.read, keyArgs("out/secret.txt"))
        await denied(StandardPorts.Fs.write, encodeArgs { (writer: inout UndraWriter) -> Void in
            writer.writeString("out/new.txt")
            writer.writeBytes([1])
        })
        await denied(StandardPorts.Fs.delete, keyArgs("out/secret.txt"))
        await denied(StandardPorts.Fs.list, keyArgs("out"))
        XCTAssertEqual(try fixture.outsideEntries(), ["secret.txt"])
    }

    // MARK: Helpers

    private func umaskNow() -> mode_t {
        let current = umask(0o022)
        umask(current)
        return current
    }
}

/// A flag a background thread polls.
private final class StopFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var value = false

    var stopped: Bool {
        lock.lock()
        defer { lock.unlock() }
        return value
    }

    func stop() {
        lock.lock()
        value = true
        lock.unlock()
    }
}
