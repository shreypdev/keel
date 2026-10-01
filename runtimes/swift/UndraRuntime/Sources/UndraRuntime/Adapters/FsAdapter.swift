// Fs: `read`, `write`, `delete` and `list` over a directory tree (docs/SPEC.md section 8).

import Foundation

/// `Fs` over `FileManager`, confined to one root directory.
///
/// Paths from the core are relative to the root (`"notes/a.txt"`); a leading `/` is ignored and
/// any `..` component is refused with `FsError::Denied`, so the core cannot reach outside the
/// root. `write` creates missing parent directories. `list` returns the entry names of one
/// directory, sorted; a name that is a directory has no trailing slash.
///
/// Failures are `FsError`s (port status 1): a missing path is `.notFound`, a permission error or
/// a path outside the root is `.denied`, a full disk or quota (`NSFileWriteOutOfSpaceError`,
/// `ENOSPC`, `EDQUOT`) is `.full` (ADR-049), anything else is `.io` with the platform's message.
public struct FsAdapter: UndraAdapter {
    /// Writes a file's bytes, atomically. The default is `Data.write(to:options: .atomic)`; tests
    /// replace it to make a write fail as the platform would.
    typealias FileWriter = @Sendable (_ data: [UInt8], _ url: URL) throws -> Void

    private let root: URL
    private let writeFile: FileWriter

    /// Creates the adapter over `<Application Support>/<bundle id>/Undra/fs`.
    public init() {
        self.init(root: KvAdapter.defaultDirectory(named: "fs"))
    }

    /// Creates the adapter over `root` (created on first write).
    public init(root: URL) {
        self.init(root: root, writeFile: FsAdapter.atomicWrite)
    }

    /// Creates the adapter over `root`, writing files with `writeFile`.
    init(root: URL, writeFile: @escaping FileWriter) {
        self.root = root
        self.writeFile = writeFile
    }

    /// `fnv1a32("port.Fs")`.
    public var portId: UInt32 {
        return StandardPorts.Fs.portId
    }

    /// The asynchronous `Fs` method table over the root directory.
    public func makePortImpl(core: UndraCore) -> PortImpl? {
        let root = self.root
        let writeFile = self.writeFile
        return .async([
            StandardPorts.Fs.read: { args in
                var reader = UndraReader(args)
                let path = try reader.readString()
                try reader.finish()
                let bytes = try FsAdapter.read(path, root: root)
                return UndraBytes(bytes).undraEncoded()
            },
            StandardPorts.Fs.write: { args in
                var reader = UndraReader(args)
                let path = try reader.readString()
                let data = try reader.readBytes()
                try reader.finish()
                try FsAdapter.write(path, data: data, root: root, writeFile: writeFile)
                return []
            },
            StandardPorts.Fs.delete: { args in
                var reader = UndraReader(args)
                let path = try reader.readString()
                try reader.finish()
                try FsAdapter.delete(path, root: root)
                return []
            },
            StandardPorts.Fs.list: { args in
                var reader = UndraReader(args)
                let path = try reader.readString()
                try reader.finish()
                let names = try FsAdapter.list(path, root: root)
                return names.undraEncoded()
            },
        ])
    }

    // MARK: Operations

    /// Resolves a core path under `root`, or throws `FsError::Denied`.
    static func resolve(_ path: String, root: URL) throws -> URL {
        var url = root
        for component in path.split(separator: "/", omittingEmptySubsequences: true) {
            if component == ".." {
                throw fail(.denied)
            }
            if component == "." {
                continue
            }
            url.appendPathComponent(String(component))
        }
        return url
    }

    static func read(_ path: String, root: URL) throws -> [UInt8] {
        let url = try resolve(path, root: root)
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory) else {
            throw fail(.notFound)
        }
        if isDirectory.boolValue {
            throw fail(.io("\(path) is a directory"))
        }
        do {
            return [UInt8](try Data(contentsOf: url))
        } catch {
            throw fail(map(error))
        }
    }

    static func write(_ path: String, data: [UInt8], root: URL, writeFile: FileWriter = FsAdapter.atomicWrite) throws {
        let url = try resolve(path, root: root)
        if url.standardizedFileURL == root.standardizedFileURL {
            throw fail(.io("cannot write to the root"))
        }
        do {
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            try writeFile(data, url)
        } catch {
            throw fail(map(error))
        }
    }

    static func delete(_ path: String, root: URL) throws {
        let url = try resolve(path, root: root)
        if url.standardizedFileURL == root.standardizedFileURL {
            throw fail(.denied)
        }
        guard FileManager.default.fileExists(atPath: url.path) else {
            throw fail(.notFound)
        }
        do {
            try FileManager.default.removeItem(at: url)
        } catch {
            throw fail(map(error))
        }
    }

    static func list(_ path: String, root: URL) throws -> [String] {
        let url = try resolve(path, root: root)
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory) else {
            throw fail(.notFound)
        }
        if !isDirectory.boolValue {
            throw fail(.io("\(path) is not a directory"))
        }
        do {
            let names = try FileManager.default.contentsOfDirectory(atPath: url.path)
            return names.sorted()
        } catch {
            throw fail(map(error))
        }
    }

    // MARK: Errors

    private static func fail(_ error: FsError) -> UndraPortError {
        return UndraPortError(body: error.undraEncoded())
    }

    /// The default ``FileWriter``.
    static let atomicWrite: FileWriter = { (data: [UInt8], url: URL) throws -> Void in
        try Data(data).write(to: url, options: .atomic)
    }

    /// Maps a Foundation error to `FsError`.
    static func map(_ error: any Error) -> FsError {
        if StorageFailure.isOutOfSpace(error) {
            return .full
        }
        if let cocoa = error as? CocoaError {
            switch cocoa.code {
            case .fileNoSuchFile, .fileReadNoSuchFile:
                return .notFound
            case .fileReadNoPermission, .fileWriteNoPermission:
                return .denied
            default:
                return .io(cocoa.localizedDescription)
            }
        }
        return .io(String(describing: error))
    }
}
