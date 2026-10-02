// Fs: `read`, `write`, `delete` and `list` over a directory tree (docs/SPEC.md section 8).

import Foundation

/// `Fs` over `FileManager`, confined to one root directory.
///
/// Paths from the core are relative to the root (`"notes/a.txt"`); a leading `/` is ignored, and
/// any `..` component or symbolic link on a path is refused with `FsError::Denied`, so the core
/// cannot reach outside the root. (Links are never followed: every component is opened from its
/// parent's descriptor with `O_NOFOLLOW`; `delete` of a link removes the link.) `write` creates
/// missing parent directories and is atomic. `list` returns the entry names of one directory,
/// sorted; a name that is a directory has no trailing slash.
///
/// The default adapter's root is `<Application Support>/<bundle id>/Undra/<namespace>/fs`, the
/// namespace being the core's it is registered with (ADR-044 amendment A): two cores of one app
/// never share files. ``init(root:)`` confines the core to a directory of the app's choice instead.
///
/// Failures are `FsError`s (port status 1): a missing path is `.notFound`, a permission error, a
/// link or a path outside the root is `.denied`, a full disk or quota (`ENOSPC`, `EDQUOT`,
/// `NSFileWriteOutOfSpaceError`) is `.full` (ADR-049), anything else is `.io` with the platform's
/// message.
public struct FsAdapter: UndraAdapter {
    /// Writes a file's bytes atomically below a directory: `PosixFiles.writeAtomically`. Tests
    /// replace it to make a write fail with the `errno` the platform would leave.
    typealias AtomicWriter = @Sendable (
        _ bytes: [UInt8],
        _ name: String,
        _ temp: String,
        _ directory: OwnedDescriptor,
        _ mode: mode_t
    ) throws -> Void

    /// The root for the core with the namespace given.
    private let rootFor: @Sendable (String) -> URL
    private let writeAtomically: AtomicWriter

    /// Creates the adapter over `<Application Support>/<bundle id>/Undra/<namespace>/fs`, the
    /// namespace being that of the core it is registered with.
    public init() {
        self.rootFor = { namespace in KvAdapter.defaultDirectory(namespace: namespace, named: "fs") }
        self.writeAtomically = FsAdapter.posixWriter
    }

    /// Creates the adapter over `root` (created on first write), for every core it serves.
    public init(root: URL) {
        self.init(root: root, writeAtomically: FsAdapter.posixWriter)
    }

    /// Creates the adapter over `root`, sealing written files with `writeAtomically`.
    init(root: URL, writeAtomically: @escaping AtomicWriter) {
        self.rootFor = { _ in root }
        self.writeAtomically = writeAtomically
    }

    /// The default ``AtomicWriter``.
    static let posixWriter: AtomicWriter = { (bytes: [UInt8], name: String, temp: String, directory: OwnedDescriptor, mode: mode_t) throws -> Void in
        try PosixFiles.writeAtomically(bytes, named: name, via: temp, in: directory, mode: mode)
    }

    /// `fnv1a32("port.Fs")`.
    public var portId: UInt32 {
        return StandardPorts.Fs.portId
    }

    /// The asynchronous `Fs` method table over the root directory of `core`'s namespace.
    public func makePortImpl(core: UndraCore) -> PortImpl? {
        let root = rootFor(core.namespace)
        let writeAtomically = self.writeAtomically
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
                try FsAdapter.write(path, data: data, root: root, writeAtomically: writeAtomically)
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
    //
    // Every operation walks from a descriptor of the root, one component at a time, each opened
    // with `O_NOFOLLOW` (`openat`), so a symbolic link anywhere below the root is refused and one
    // swapped in between two steps is too: no check is separated from its use. The root itself is
    // the app's own configuration and is opened as given.

    /// The files `write` is still writing start with this; `list` hides them (the React Native
    /// module's store, which shares the root, names its temporary files the same way).
    static let temporaryPrefix = ".undra-tmp-"

    /// The components of a core path, or the error that refuses it: any `..` is `Denied`, a NUL
    /// byte is an `Io` error (a C string would end there). `.` and empty components, and so a
    /// leading `/`, are dropped. Split on bytes: a `/` followed by a combining mark is one
    /// `Character` in Swift, and a splitter that went by `Character` would miss that separator.
    static func components(of path: String) throws -> [String] {
        if path.utf8.contains(0) {
            throw fail(.io("the path contains a NUL byte"))
        }
        var parts: [String] = []
        for bytes in path.utf8.split(separator: UInt8(ascii: "/"), omittingEmptySubsequences: true) {
            if bytes.elementsEqual([0x2E, 0x2E]) {
                throw fail(.denied)
            }
            if bytes.elementsEqual([0x2E]) {
                continue
            }
            parts.append(String(decoding: bytes, as: UTF8.self))
        }
        return parts
    }

    static func read(_ path: String, root: URL) throws -> [UInt8] {
        let parts = try components(of: path)
        guard let name = parts.last else {
            throw fail(.io("the path names the root, a directory"))
        }
        let parent = try openDirectory(root: root, parts: parts.dropLast(), create: false)
        let raw = openat(parent.raw, name, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
        if raw < 0 {
            let code = errno
            if code == ELOOP || code == EMLINK, try kind(of: name, in: parent) == S_IFLNK {
                throw fail(.denied)
            }
            throw failure(code)
        }
        let file = OwnedDescriptor(raw)
        var info = stat()
        if fstat(file.raw, &info) != 0 {
            throw failure(errno)
        }
        if info.st_mode & S_IFMT == S_IFDIR {
            throw fail(.io("\(path) is a directory"))
        }
        if info.st_mode & S_IFMT != S_IFREG {
            throw fail(.io("\(path) is not a regular file"))
        }
        do {
            return try PosixFiles.readAll(from: file.raw, sizeHint: Int(info.st_size))
        } catch let error as PosixError {
            throw failure(error.code)
        }
    }

    static func write(_ path: String, data: [UInt8], root: URL, writeAtomically: AtomicWriter = FsAdapter.posixWriter) throws {
        let parts = try components(of: path)
        guard let name = parts.last else {
            throw fail(.io("cannot write to the root"))
        }
        let parent = try openDirectory(root: root, parts: parts.dropLast(), create: true)
        let existing = try kind(of: name, in: parent)
        if existing == S_IFLNK {
            throw fail(.denied)
        }
        if existing == S_IFDIR {
            throw fail(.io("\(path) is a directory"))
        }
        // The rename that seals the file replaces what is at `name` and never follows it, so a
        // link put there after the check above is replaced, not written through.
        do {
            try writeAtomically(data, name, temporaryPrefix + PosixFiles.randomHex(), parent, 0o644)
        } catch let error as PosixError {
            throw failure(error.code)
        } catch {
            throw fail(map(error))
        }
    }

    static func delete(_ path: String, root: URL) throws {
        let parts = try components(of: path)
        guard let name = parts.last else {
            throw fail(.denied)
        }
        let parent = try openDirectory(root: root, parts: parts.dropLast(), create: false)
        switch try kind(of: name, in: parent) {
        case 0:
            throw fail(.notFound)
        case S_IFDIR:
            try removeTree(name, in: parent)
        default:
            // A file, or a link: removed, never followed.
            if unlinkat(parent.raw, name, 0) != 0 {
                throw failure(errno)
            }
        }
    }

    static func list(_ path: String, root: URL) throws -> [String] {
        let parts = try components(of: path)
        let directory: OwnedDescriptor
        if let name = parts.last {
            let parent = try openDirectory(root: root, parts: parts.dropLast(), create: false)
            let existing = try kind(of: name, in: parent)
            if existing != 0 && existing != S_IFDIR && existing != S_IFLNK {
                throw fail(.io("\(path) is not a directory"))
            }
            directory = try openChild(of: parent, named: name, create: false)
        } else {
            directory = try openDirectory(root: root, parts: [], create: false)
        }
        let names = try entries(of: directory).filter { !$0.hasPrefix(temporaryPrefix) }
        return names.sorted()
    }

    // MARK: Walking

    /// Opens the root (creating it first with `create`), then `parts` below it, none through a link.
    private static func openDirectory(root: URL, parts: ArraySlice<String>, create: Bool) throws -> OwnedDescriptor {
        if create {
            do {
                try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
            } catch {
                throw fail(map(error))
            }
        }
        let raw = open(root.path, O_RDONLY | O_DIRECTORY | O_CLOEXEC)
        if raw < 0 {
            throw failure(errno)
        }
        var directory = OwnedDescriptor(raw)
        for part in parts {
            directory = try openChild(of: directory, named: part, create: create)
        }
        return directory
    }

    private static func openDirectory(root: URL, parts: [String], create: Bool) throws -> OwnedDescriptor {
        return try openDirectory(root: root, parts: parts[...], create: create)
    }

    /// Opens the directory `name` of `parent` without following a link; with `create`, a missing one
    /// is made. A link is `Denied`; a file where a directory should be is `NotFound` (an `Io` error
    /// with `create`).
    private static func openChild(of parent: OwnedDescriptor, named name: String, create: Bool) throws -> OwnedDescriptor {
        for attempt in 0 ..< 2 {
            let raw = openat(parent.raw, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
            if raw >= 0 {
                return OwnedDescriptor(raw)
            }
            let code = errno
            if code == ENOENT && create && attempt == 0 {
                if mkdirat(parent.raw, name, 0o700) != 0 && errno != EEXIST {
                    throw failure(errno)
                }
                continue
            }
            if code == ELOOP || code == ENOTDIR || code == EMLINK {
                let existing = try kind(of: name, in: parent)
                if existing == S_IFLNK {
                    throw fail(.denied)
                }
                if existing != 0 && create {
                    throw fail(.io("\(name) is not a directory"))
                }
                throw fail(.notFound)
            }
            throw failure(code)
        }
        throw failure(ENOENT)
    }

    /// What is at `name` in `parent`, without following a link: the `S_IFMT` bits, or 0 for nothing.
    private static func kind(of name: String, in parent: OwnedDescriptor) throws -> mode_t {
        var info = stat()
        if fstatat(parent.raw, name, &info, AT_SYMLINK_NOFOLLOW) != 0 {
            if errno == ENOENT {
                return 0
            }
            throw failure(errno)
        }
        return info.st_mode & S_IFMT
    }

    /// The names in `directory`, `.` and `..` excluded, unsorted.
    private static func entries(of directory: OwnedDescriptor) throws -> [String] {
        let duplicate = dup(directory.raw)
        if duplicate < 0 {
            throw failure(errno)
        }
        guard let stream = fdopendir(duplicate) else {
            let code = errno
            _ = close(duplicate)
            throw failure(code)
        }
        defer { _ = closedir(stream) }
        rewinddir(stream) // the duplicate shares its offset with `directory`
        var names: [String] = []
        errno = 0
        while let entry = readdir(stream) {
            let name = withUnsafePointer(to: &entry.pointee.d_name) { field -> String in
                return field.withMemoryRebound(to: CChar.self, capacity: Int(entry.pointee.d_namlen) + 1) {
                    return String(cString: $0)
                }
            }
            if name != "." && name != ".." {
                names.append(name)
            }
        }
        if errno != 0 {
            throw failure(errno)
        }
        return names
    }

    /// Removes the directory `name` of `parent` and everything in it, never following a link. It
    /// keeps a stack of names and re-opens the chain from `parent` each round, so a tree of any
    /// depth needs a bounded number of descriptors.
    private static func removeTree(_ name: String, in parent: OwnedDescriptor) throws {
        var stack = [name]
        while let innermost = stack.last {
            let directory = try descend(stack, from: parent)
            var child: String?
            var files: [String] = []
            for entry in try entries(of: directory) {
                if try kind(of: entry, in: directory) == S_IFDIR {
                    child = entry
                    break
                }
                files.append(entry)
            }
            for file in files {
                if unlinkat(directory.raw, file, 0) != 0 && errno != ENOENT {
                    throw failure(errno)
                }
            }
            if let child = child {
                stack.append(child)
                continue
            }
            // Empty now: remove it from its parent.
            let above = try descend(Array(stack.dropLast()), from: parent)
            if unlinkat(above.raw, innermost, AT_REMOVEDIR) != 0 {
                let code = errno
                if code == ENOTEMPTY || code == EEXIST {
                    continue // something was created meanwhile: go round again
                }
                throw failure(code)
            }
            stack.removeLast()
        }
    }

    /// Opens the directory `chain` below `start`, none through a link.
    private static func descend(_ chain: [String], from start: OwnedDescriptor) throws -> OwnedDescriptor {
        let duplicate = dup(start.raw)
        if duplicate < 0 {
            throw failure(errno)
        }
        var directory = OwnedDescriptor(duplicate)
        for name in chain {
            directory = try openChild(of: directory, named: name, create: false)
        }
        return directory
    }

    // MARK: Errors

    private static func fail(_ error: FsError) -> UndraPortError {
        return UndraPortError(body: error.undraEncoded())
    }

    /// The `FsError` of an `errno`: a missing path (or a file where a directory should be) is
    /// `NotFound`; no permission, and a link met under `O_NOFOLLOW`, is `Denied`; a full disk or
    /// quota is `Full` (ADR-049).
    static func failure(_ code: Int32) -> UndraPortError {
        return fail(fsError(errno: code))
    }

    /// The `FsError` of an `errno` (``failure(_:)``'s mapping).
    static func fsError(errno code: Int32) -> FsError {
        switch code {
        case ENOENT, ENOTDIR:
            return .notFound
        case EACCES, EPERM, ELOOP:
            return .denied
        case ENOSPC, EDQUOT:
            return .full
        default:
            return .io(String(cString: strerror(code)))
        }
    }

    /// Maps a Foundation error to `FsError`.
    static func map(_ error: any Error) -> FsError {
        if StorageFailure.isOutOfSpace(error) {
            return .full
        }
        if let posix = error as? PosixError {
            return fsError(errno: posix.code)
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
