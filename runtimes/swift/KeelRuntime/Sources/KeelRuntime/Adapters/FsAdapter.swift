// Fs: `read`, `write`, `delete` and `list` over a directory tree (docs/SPEC.md section 8).

import Foundation

/// `Fs` over `FileManager`, confined to one root directory.
///
/// Paths from the core are relative to the root (`"notes/a.txt"`); a leading `/` is ignored and
/// any `..` component is refused with `FsError::Denied`, so the core cannot reach outside the
/// root. `write` creates missing parent directories. `list` returns the entry names of one
/// directory, sorted; a name that is a directory has no trailing slash.
public struct FsAdapter: KeelAdapter {
    private let root: URL

    /// Creates the adapter over `<Application Support>/<bundle id>/Keel/fs`.
    public init() {
        self.root = KvAdapter.defaultDirectory(named: "fs")
    }

    /// Creates the adapter over `root` (created on first write).
    public init(root: URL) {
        self.root = root
    }

    public var portId: UInt32 {
        return StandardPorts.Fs.portId
    }

    public func makePortImpl(core: KeelCore) -> PortImpl? {
        let root = self.root
        return .async([
            StandardPorts.Fs.read: { args in
                var reader = KeelReader(args)
                let path = try reader.readString()
                try reader.finish()
                let bytes = try FsAdapter.read(path, root: root)
                return KeelBytes(bytes).keelEncoded()
            },
            StandardPorts.Fs.write: { args in
                var reader = KeelReader(args)
                let path = try reader.readString()
                let data = try reader.readBytes()
                try reader.finish()
                try FsAdapter.write(path, data: data, root: root)
                return []
            },
            StandardPorts.Fs.delete: { args in
                var reader = KeelReader(args)
                let path = try reader.readString()
                try reader.finish()
                try FsAdapter.delete(path, root: root)
                return []
            },
            StandardPorts.Fs.list: { args in
                var reader = KeelReader(args)
                let path = try reader.readString()
                try reader.finish()
                let names = try FsAdapter.list(path, root: root)
                return names.keelEncoded()
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

    static func write(_ path: String, data: [UInt8], root: URL) throws {
        let url = try resolve(path, root: root)
        if url.standardizedFileURL == root.standardizedFileURL {
            throw fail(.io("cannot write to the root"))
        }
        do {
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            try Data(data).write(to: url, options: .atomic)
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

    private static func fail(_ error: PortFsError) -> KeelPortError {
        return KeelPortError(body: error.keelEncoded())
    }

    /// Maps a Foundation error to `FsError`.
    static func map(_ error: any Error) -> PortFsError {
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
