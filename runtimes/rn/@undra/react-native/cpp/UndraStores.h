// The `Kv` and `Fs` ports of @undra/react-native, in portable C++ (ADR-038, amendment B, B4 and B5).
//
// One implementation for iOS and Android, over POSIX calls only, so `cpp/test/run.sh` runs it on the
// Mac under AddressSanitizer and UndefinedBehaviorSanitizer exactly as the phones run it. What differs
// per platform is only where the files live and how a key names its file, chosen to match the native
// runtime of that platform byte for byte (the Swift `KvAdapter` on iOS, `android-adapters` on Android),
// so a value one shell wrote is read by the other.
//
// Nothing here throws to its caller on an I/O failure: every operation returns its outcome. Memory
// exhaustion can throw `std::bad_alloc`; the worker that runs the job catches it.
#pragma once

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace undra::rn {

// ----- wire (docs/SPEC.md section 3) ------------------------------------------------------------

/// Reads the little-endian wire format of SPEC 3 from a borrowed buffer. A read past the end sets
/// `ok` to false and returns a zero value; check `finish()` at the end.
class WireReader {
 public:
  WireReader(const uint8_t *data, std::size_t len) noexcept : data_(data), len_(len) {}
  uint8_t u8() noexcept;
  uint16_t u16() noexcept;
  uint32_t u32() noexcept;
  uint64_t u64() noexcept;
  /// A `String`: `u32` byte length, then UTF-8 (validated).
  std::string str();
  /// A `Bytes`: `u32` length, then the bytes.
  std::vector<uint8_t> bytes();
  /// Whether every read succeeded and nothing is left over.
  bool finish() const noexcept { return ok_ && at_ == len_; }
  bool ok() const noexcept { return ok_; }
  /// Marks the input malformed (an unknown enum variant, say).
  void fail() noexcept { ok_ = false; }

 private:
  bool need(std::size_t n) noexcept;
  const uint8_t *data_;
  std::size_t len_;
  std::size_t at_ = 0;
  bool ok_ = true;
};

/// Writes the wire format of SPEC 3.
class WireWriter {
 public:
  WireWriter &u8(uint8_t v);
  WireWriter &u16(uint16_t v);
  WireWriter &u32(uint32_t v);
  WireWriter &u64(uint64_t v);
  WireWriter &str(std::string_view s);
  WireWriter &bytes(const uint8_t *data, std::size_t len);
  WireWriter &bytes(const std::vector<uint8_t> &data) { return bytes(data.data(), data.size()); }
  /// A `Vec<String>`.
  WireWriter &strings(const std::vector<std::string> &items);
  std::vector<uint8_t> out;
};

/// Whether `text` is well-formed UTF-8 (no overlongs, no surrogates, nothing above U+10FFFF).
bool validUtf8(std::string_view text) noexcept;

// ----- hashes -----------------------------------------------------------------------------------

/// FNV-1a 32 over the bytes of `text` (docs/SPEC.md section 1.1).
uint32_t fnv1a32Of(std::string_view text) noexcept;
/// FNV-1a 64 over the bytes of `text`.
uint64_t fnv1a64Of(std::string_view text) noexcept;
/// SHA-256 of the bytes of `text`, as 64 lowercase hex digits (FIPS 180-4).
std::string sha256Hex(std::string_view text);

// ----- Kv (and the files of SecureStore on Android) ---------------------------------------------

/// How a key names its file: the layout of the platform's native runtime.
enum class KvNaming : uint8_t {
  /// `<fnv1a64 as 16 hex>-<fnv1a32 as 8 hex>`: the Swift `KvAdapter` (`FileKeyValueBackend`).
  Fnv,
  /// The SHA-256 of the key in hex: `android-adapters` (`FileKv`).
  Sha256,
};

/// The file name of `key` under `naming`.
std::string kvFileName(KvNaming naming, std::string_view key);

/// The variants of the `Kv` and `SecureStore` ports' `StorageError` (docs/SPEC.md section 8,
/// ADR-049), in wire order: `Unavailable(String)`, `Full`, `Locked`, `Corrupt(String)`, `Io(String)`.
enum class StorageErrorKind : uint16_t { Unavailable = 0, Full = 1, Locked = 2, Corrupt = 3, Io = 4 };

/// The `StorageError` of an `errno`: a full disk or quota is `Full`, `EPERM` (protected data before
/// the device's first unlock) is `Locked`, anything else `Io` (the Swift adapter's mapping).
StorageErrorKind storageErrorOf(int code) noexcept;

/// A key-value store in a directory: one file per key, holding `u32 key length, key, value`.
///
/// Writes go to a temporary file in the same directory that is `fsync`ed and renamed over the entry
/// (mode 0600), so a killed process leaves the old value or the new one. `get` checks the key stored
/// in the file, so a name collision is never another key's value; `list` reads only each file's key
/// and skips temporary files, dot files and anything that is not an entry. Not thread-safe: the
/// host runs one store on one worker thread.
class KvStore {
 public:
  KvStore(std::string directory, KvNaming naming);

  /// The value under `key` into `value` (`nullopt` when there is none). `false` with `error` set
  /// when the store cannot answer (an unreadable or damaged entry). Every method that fails also
  /// sets `*kind`, when given, to the failure's `StorageError` variant (a damaged entry is
  /// `Corrupt`, a full disk `Full`).
  bool get(const std::string &key, std::optional<std::vector<uint8_t>> &value, std::string &error,
      StorageErrorKind *kind = nullptr) const;
  /// Stores `value` under `key`, replacing what was there.
  bool set(const std::string &key, const uint8_t *value, std::size_t len, std::string &error,
      StorageErrorKind *kind = nullptr) const;
  /// Removes `key`; a missing key is not an error.
  bool remove(const std::string &key, std::string &error, StorageErrorKind *kind = nullptr) const;
  /// The keys that start with `prefix`, sorted by byte order.
  bool list(const std::string &prefix, std::vector<std::string> &keys, std::string &error,
      StorageErrorKind *kind = nullptr) const;

  const std::string &directory() const noexcept { return directory_; }

 private:
  std::string pathOf(const std::string &key) const;
  /// The key stored at `path`: `nullopt` when the file is missing or not an entry.
  bool readKey(const std::string &path, std::optional<std::string> &key, std::string &error,
      StorageErrorKind *kind) const;

  std::string directory_;
  KvNaming naming_;
};

// ----- Fs ---------------------------------------------------------------------------------------

/// The variants of the `Fs` port's `FsError` (docs/SPEC.md section 8), in wire order.
enum class FsErrorKind : uint16_t { NotFound = 0, Denied = 1, Io = 2, Full = 3, Unavailable = 4 };

/// Why an `Fs` operation failed.
struct FsFailure {
  FsErrorKind kind;
  /// The `Io` and `Unavailable` variants' text; empty for the others.
  std::string message;
};

/// The `Fs` port over one root directory, with the rules of docs/SPEC.md section 8, stricter where it
/// allows (ADR-038 amendment B, B4):
///
///  * paths are `/`-separated and relative to the root; a leading `/`, empty and `.` components are
///    ignored; **any** `..` component is `Denied`, and a NUL byte is `Io`;
///  * every component is opened from the root's descriptor with `O_NOFOLLOW`, so **any** symbolic
///    link on the path is `Denied`; `delete` removes a link without following it;
///  * `write` creates missing parents and is atomic (temporary file, `fsync`, `renameat`);
///  * `delete` removes a file, or a directory with everything in it (links never followed), and
///    never the root (`Denied`); `read` and `write` of the root are `Io` (it is a directory);
///  * `list` returns the names of one directory sorted by byte order, without temporary files.
///
/// Not thread-safe: the host runs it on one worker thread.
class FsRoot {
 public:
  explicit FsRoot(std::string root);

  std::optional<FsFailure> read(const std::string &path, std::vector<uint8_t> &out) const;
  std::optional<FsFailure> write(const std::string &path, const uint8_t *data, std::size_t len) const;
  std::optional<FsFailure> remove(const std::string &path) const;
  std::optional<FsFailure> list(const std::string &dir, std::vector<std::string> &names) const;

  const std::string &root() const noexcept { return root_; }

 private:
  std::string root_;
};

/// Creates `path` and its missing parents (mode 0700). `false` with `error` set on failure.
bool makeDirectories(const std::string &path, std::string &error);

} // namespace undra::rn
