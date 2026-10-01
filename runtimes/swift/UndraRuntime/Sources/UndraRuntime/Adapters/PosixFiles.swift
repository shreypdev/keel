// POSIX helpers shared by the file-backed adapters (`Fs` and `Kv`): descriptors that close
// themselves, and the atomic write both stores use.
//
// Foundation's file APIs follow symbolic links and cannot be told not to, so `Fs` walks the tree
// below its root with these descriptors instead (`FsAdapter`), one `openat` per component.

import Foundation

/// A failed POSIX call: the `errno` it left.
struct PosixError: Error, Equatable, CustomStringConvertible {
    /// The `errno` value.
    let code: Int32

    var description: String {
        return String(cString: strerror(code))
    }
}

/// A file descriptor, closed when the last reference to it goes away.
final class OwnedDescriptor {
    /// The raw descriptor. It stays open for as long as this object is alive.
    let raw: Int32

    init(_ raw: Int32) {
        self.raw = raw
    }

    deinit {
        _ = close(raw)
    }
}

enum PosixFiles {
    /// 16 random lowercase hex digits: the unique part of a temporary file name.
    static func randomHex() -> String {
        let digits = String(UInt64.random(in: UInt64.min ... UInt64.max), radix: 16)
        return String(repeating: "0", count: 16 - digits.count) + digits
    }

    /// Writes all of `bytes` to `fd`.
    static func writeAll(_ bytes: [UInt8], to fd: Int32) throws {
        var done = 0
        while done < bytes.count {
            let written = bytes.withUnsafeBytes { buffer -> Int in
                return write(fd, buffer.baseAddress! + done, buffer.count - done)
            }
            if written < 0 {
                if errno == EINTR {
                    continue
                }
                throw PosixError(code: errno)
            }
            done += written
        }
    }

    /// Reads `fd` to its end.
    static func readAll(from fd: Int32, sizeHint: Int) throws -> [UInt8] {
        var result: [UInt8] = []
        result.reserveCapacity(max(sizeHint, 0))
        var chunk = [UInt8](repeating: 0, count: 64 * 1024)
        while true {
            let count = chunk.withUnsafeMutableBytes { buffer -> Int in
                return read(fd, buffer.baseAddress, buffer.count)
            }
            if count < 0 {
                if errno == EINTR {
                    continue
                }
                throw PosixError(code: errno)
            }
            if count == 0 {
                return result
            }
            result.append(contentsOf: chunk[0 ..< count])
        }
    }

    /// Writes `bytes` to the new file `temp` in `directory`, flushes it and renames it to `name`,
    /// then flushes the directory, so a reader sees the old file or the whole new one and the
    /// rename survives a crash. `temp` is removed when anything fails. The rename replaces what is
    /// at `name`, whatever it is, and never follows a link there.
    static func writeAtomically(
        _ bytes: [UInt8],
        named name: String,
        via temp: String,
        in directory: OwnedDescriptor,
        mode: mode_t
    ) throws {
        let raw = openat(directory.raw, temp, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, mode)
        if raw < 0 {
            throw PosixError(code: errno)
        }
        let file = OwnedDescriptor(raw)
        do {
            try writeAll(bytes, to: file.raw)
            if fsync(file.raw) != 0 {
                throw PosixError(code: errno)
            }
            if renameat(directory.raw, temp, directory.raw, name) != 0 {
                throw PosixError(code: errno)
            }
        } catch {
            _ = unlinkat(directory.raw, temp, 0)
            throw error
        }
        // Best effort: some file systems refuse fsync on a directory.
        _ = fsync(directory.raw)
    }
}
