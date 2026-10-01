#include "UndraStores.h"

#include <dirent.h>
#include <fcntl.h>
#include <stdlib.h>
#include <sys/stat.h>
#include <unistd.h>

#include <algorithm>
#include <array>
#include <cerrno>
#include <cstring>
#include <utility>

namespace undra::rn {

namespace {

// ----- small POSIX helpers ----------------------------------------------------------------------

/// Closes a descriptor when it goes out of scope.
class Fd {
 public:
  explicit Fd(int fd = -1) noexcept : fd_(fd) {}
  ~Fd() {
    reset();
  }
  Fd(Fd &&other) noexcept : fd_(std::exchange(other.fd_, -1)) {}
  Fd &operator=(Fd &&other) noexcept {
    if (this != &other) {
      reset();
      fd_ = std::exchange(other.fd_, -1);
    }
    return *this;
  }
  Fd(const Fd &) = delete;
  Fd &operator=(const Fd &) = delete;
  int get() const noexcept {
    return fd_;
  }
  bool valid() const noexcept {
    return fd_ >= 0;
  }
  /// Gives the descriptor up without closing it (a `DIR` stream took it).
  int release() noexcept {
    return std::exchange(fd_, -1);
  }
  void reset() noexcept {
    if (fd_ >= 0) {
      ::close(fd_);
      fd_ = -1;
    }
  }

 private:
  int fd_;
};

/// Closes a directory stream when it goes out of scope.
struct DirCloser {
  DIR *dir;
  ~DirCloser() {
    if (dir != nullptr) {
      ::closedir(dir);
    }
  }
};

std::string errnoText(int code) {
  return std::strerror(code);
}

/// Reads the whole of `fd` into `out`.
bool readAll(int fd, std::vector<uint8_t> &out, int &code) {
  out.clear();
  struct stat st{};
  if (::fstat(fd, &st) == 0 && st.st_size > 0) {
    out.reserve(static_cast<std::size_t>(st.st_size));
  }
  uint8_t chunk[16384];
  while (true) {
    const ssize_t n = ::read(fd, chunk, sizeof(chunk));
    if (n < 0) {
      if (errno == EINTR) continue;
      code = errno;
      return false;
    }
    if (n == 0) return true;
    out.insert(out.end(), chunk, chunk + n);
  }
}

/// Reads exactly `len` bytes into `out` unless the file ends first (`got` says how many).
bool readExactly(int fd, uint8_t *out, std::size_t len, std::size_t &got, int &code) {
  got = 0;
  while (got < len) {
    const ssize_t n = ::read(fd, out + got, len - got);
    if (n < 0) {
      if (errno == EINTR) continue;
      code = errno;
      return false;
    }
    if (n == 0) return true;
    got += static_cast<std::size_t>(n);
  }
  return true;
}

bool writeAll(int fd, const uint8_t *data, std::size_t len, int &code) {
  std::size_t done = 0;
  while (done < len) {
    const ssize_t n = ::write(fd, data + done, len - done);
    if (n < 0) {
      if (errno == EINTR) continue;
      code = errno;
      return false;
    }
    done += static_cast<std::size_t>(n);
  }
  return true;
}

/// A fresh suffix for a temporary file name: 16 random hex digits.
std::string randomHex() {
  uint8_t raw[8];
  arc4random_buf(raw, sizeof(raw));
  static const char *digits = "0123456789abcdef";
  std::string out;
  out.reserve(16);
  for (uint8_t b : raw) {
    out.push_back(digits[b >> 4]);
    out.push_back(digits[b & 0xf]);
  }
  return out;
}

bool endsWith(std::string_view text, std::string_view suffix) {
  return text.size() >= suffix.size() && text.compare(text.size() - suffix.size(), suffix.size(), suffix) == 0;
}

bool startsWith(std::string_view text, std::string_view prefix) {
  return text.size() >= prefix.size() && text.compare(0, prefix.size(), prefix) == 0;
}

/// Writes `data` to a temporary file in the directory `dirFd`, flushes it and renames it to `name`;
/// then flushes the directory, so the rename survives a crash. The temporary file is removed on
/// failure. `code` is the failing `errno`.
bool atomicWriteAt(int dirFd, const std::string &name, const std::string &temp, const uint8_t *data, std::size_t len, mode_t mode, int &code) {
  Fd out(::openat(dirFd, temp.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, mode));
  if (!out.valid()) {
    code = errno;
    return false;
  }
  bool written = writeAll(out.get(), data, len, code);
  if (written && ::fsync(out.get()) != 0) {
    code = errno;
    written = false;
  }
  out.reset();
  if (written && ::renameat(dirFd, temp.c_str(), dirFd, name.c_str()) != 0) {
    code = errno;
    written = false;
  }
  if (!written) {
    ::unlinkat(dirFd, temp.c_str(), 0);
    return false;
  }
  ::fsync(dirFd); // best effort: some file systems refuse fsync on a directory
  return true;
}

// ----- SHA-256 (FIPS 180-4) ---------------------------------------------------------------------

constexpr std::array<uint32_t, 64> kSha256K{
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be,
    0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa,
    0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85,
    0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f,
    0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2};

constexpr uint32_t rotr(uint32_t x, int n) {
  return (x >> n) | (x << (32 - n));
}

void sha256Block(std::array<uint32_t, 8> &h, const uint8_t *block) {
  uint32_t w[64];
  for (int i = 0; i < 16; ++i) {
    w[i] = (static_cast<uint32_t>(block[4 * i]) << 24) | (static_cast<uint32_t>(block[4 * i + 1]) << 16) |
        (static_cast<uint32_t>(block[4 * i + 2]) << 8) | static_cast<uint32_t>(block[4 * i + 3]);
  }
  for (int i = 16; i < 64; ++i) {
    const uint32_t s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >> 3);
    const uint32_t s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >> 10);
    w[i] = w[i - 16] + s0 + w[i - 7] + s1;
  }
  uint32_t a = h[0], b = h[1], c = h[2], d = h[3], e = h[4], f = h[5], g = h[6], hh = h[7];
  for (int i = 0; i < 64; ++i) {
    const uint32_t s1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
    const uint32_t ch = (e & f) ^ (~e & g);
    const uint32_t t1 = hh + s1 + ch + kSha256K[static_cast<std::size_t>(i)] + w[i];
    const uint32_t s0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
    const uint32_t maj = (a & b) ^ (a & c) ^ (b & c);
    const uint32_t t2 = s0 + maj;
    hh = g;
    g = f;
    f = e;
    e = d + t1;
    d = c;
    c = b;
    b = a;
    a = t1 + t2;
  }
  h[0] += a;
  h[1] += b;
  h[2] += c;
  h[3] += d;
  h[4] += e;
  h[5] += f;
  h[6] += g;
  h[7] += hh;
}

/// `value` as `width` lowercase hex digits, zero-padded.
std::string hexOf(uint64_t value, int width) {
  static const char *digits = "0123456789abcdef";
  std::string out(static_cast<std::size_t>(width), '0');
  for (int i = width - 1; i >= 0 && value != 0; --i) {
    out[static_cast<std::size_t>(i)] = digits[value & 0xf];
    value >>= 4;
  }
  return out;
}

// ----- Fs path handling -------------------------------------------------------------------------

/// Temporary files of `FsRoot::write` start with this; `list` hides them.
constexpr std::string_view kFsTempPrefix = ".undra-tmp-";
/// Temporary files of `KvStore::set` end with this; `list` skips them (`android-adapters` does too).
constexpr std::string_view kKvTempSuffix = ".tmp";

FsFailure denied() {
  return FsFailure{FsErrorKind::Denied, {}};
}

FsFailure notFound() {
  return FsFailure{FsErrorKind::NotFound, {}};
}

FsFailure io(std::string message) {
  return FsFailure{FsErrorKind::Io, std::move(message)};
}

/// The `FsError` of an `errno`.
FsFailure fromErrno(int code) {
  switch (code) {
    case ENOENT:
    case ENOTDIR:
      return notFound();
    case EACCES:
    case EPERM:
    case ELOOP:
      return denied();
    default:
      return io(errnoText(code));
  }
}

/// The components of a core path (see `FsRoot`), or the failure that refuses it.
std::optional<FsFailure> splitPath(const std::string &path, std::vector<std::string> &parts) {
  parts.clear();
  if (path.find('\0') != std::string::npos) {
    return io("the path contains a NUL byte");
  }
  std::size_t at = 0;
  while (at <= path.size()) {
    const std::size_t slash = path.find('/', at);
    const std::size_t end = slash == std::string::npos ? path.size() : slash;
    std::string part = path.substr(at, end - at);
    if (part == "..") {
      return denied();
    }
    if (!part.empty() && part != ".") {
      parts.push_back(std::move(part));
    }
    if (slash == std::string::npos) break;
    at = slash + 1;
  }
  return std::nullopt;
}

/// What is at `name` in `dirFd`, without following a link: `S_IFMT` bits, or 0 when nothing.
std::optional<FsFailure> kindAt(int dirFd, const std::string &name, mode_t &kind) {
  struct stat st{};
  if (::fstatat(dirFd, name.c_str(), &st, AT_SYMLINK_NOFOLLOW) != 0) {
    if (errno == ENOENT) {
      kind = 0;
      return std::nullopt;
    }
    return fromErrno(errno);
  }
  kind = st.st_mode & S_IFMT;
  return std::nullopt;
}

/// Opens the directory `name` of `dirFd` without following a link. With `create`, a missing one is
/// made. A link is `Denied`; a file where a directory should be is `NotFound` (or `Io` with `create`).
std::optional<FsFailure> openDirAt(int dirFd, const std::string &name, bool create, Fd &out) {
  for (int attempt = 0; attempt < 2; ++attempt) {
    out = Fd(::openat(dirFd, name.c_str(), O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
    if (out.valid()) {
      return std::nullopt;
    }
    const int code = errno;
    if (code == ENOENT && create && attempt == 0) {
      if (::mkdirat(dirFd, name.c_str(), 0700) != 0 && errno != EEXIST) {
        return fromErrno(errno);
      }
      continue;
    }
    if (code == ELOOP || code == ENOTDIR || code == EMLINK) {
      mode_t kind = 0;
      if (auto failure = kindAt(dirFd, name, kind)) return failure;
      if (kind == S_IFLNK) return denied();
      if (kind != 0 && create) return io(name + " is not a directory");
      return notFound();
    }
    return fromErrno(code);
  }
  return fromErrno(ENOENT);
}

/// Opens the root (creating it with `create`) and walks `parts[0..count)` below it.
std::optional<FsFailure> openChain(const std::string &root, const std::vector<std::string> &parts, std::size_t count, bool create, Fd &out) {
  if (create) {
    std::string error;
    if (!makeDirectories(root, error)) {
      return io(error);
    }
  }
  Fd dir(::open(root.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC));
  if (!dir.valid()) {
    return fromErrno(errno);
  }
  for (std::size_t i = 0; i < count; ++i) {
    Fd next;
    if (auto failure = openDirAt(dir.get(), parts[i], create, next)) return failure;
    dir = std::move(next);
  }
  out = std::move(dir);
  return std::nullopt;
}

/// Removes the directory `name` of `parentFd` and everything in it, never following a link. It
/// keeps a stack of names rather than recursing with open descriptors, so a tree of any depth
/// needs a bounded number of descriptors.
std::optional<FsFailure> removeTree(int parentFd, const std::string &name) {
  std::vector<std::string> stack{name};
  while (!stack.empty()) {
    // Opens the directory at the top of the stack, from `parentFd`.
    Fd dir;
    {
      Fd at(::dup(parentFd));
      if (!at.valid()) return fromErrno(errno);
      for (const std::string &part : stack) {
        Fd next;
        if (auto failure = openDirAt(at.get(), part, false, next)) return failure;
        at = std::move(next);
      }
      dir = std::move(at);
    }
    Fd forStream(::dup(dir.get()));
    if (!forStream.valid()) return fromErrno(errno);
    DIR *stream = ::fdopendir(forStream.get());
    if (stream == nullptr) return fromErrno(errno);
    forStream.release(); // the stream owns it now
    DirCloser closer{stream};
    std::optional<std::string> child;
    std::vector<std::string> files;
    errno = 0;
    while (struct dirent *entry = ::readdir(stream)) {
      const std::string entryName = entry->d_name;
      if (entryName == "." || entryName == "..") continue;
      mode_t kind = 0;
      if (auto failure = kindAt(dir.get(), entryName, kind)) return failure;
      if (kind == S_IFDIR) {
        child = entryName;
        break;
      }
      files.push_back(entryName);
    }
    for (const std::string &file : files) {
      if (::unlinkat(dir.get(), file.c_str(), 0) != 0 && errno != ENOENT) return fromErrno(errno);
    }
    if (child) {
      stack.push_back(*child);
      continue;
    }
    // Empty now: remove it from its parent.
    Fd up(::dup(parentFd));
    if (!up.valid()) return fromErrno(errno);
    for (std::size_t i = 0; i + 1 < stack.size(); ++i) {
      Fd next;
      if (auto failure = openDirAt(up.get(), stack[i], false, next)) return failure;
      up = std::move(next);
    }
    if (::unlinkat(up.get(), stack.back().c_str(), AT_REMOVEDIR) != 0) {
      const int code = errno;
      if (code == ENOTEMPTY || code == EEXIST) {
        continue; // something was created meanwhile: go round again
      }
      return fromErrno(code);
    }
    stack.pop_back();
  }
  return std::nullopt;
}

} // namespace

// ----- wire -------------------------------------------------------------------------------------

bool WireReader::need(std::size_t n) noexcept {
  if (!ok_ || len_ - at_ < n) {
    ok_ = false;
    return false;
  }
  return true;
}

uint8_t WireReader::u8() noexcept {
  if (!need(1)) return 0;
  return data_[at_++];
}

uint32_t WireReader::u32() noexcept {
  if (!need(4)) return 0;
  const uint32_t v = static_cast<uint32_t>(data_[at_]) | (static_cast<uint32_t>(data_[at_ + 1]) << 8) |
      (static_cast<uint32_t>(data_[at_ + 2]) << 16) | (static_cast<uint32_t>(data_[at_ + 3]) << 24);
  at_ += 4;
  return v;
}

std::string WireReader::str() {
  const uint32_t n = u32();
  if (!need(n)) return {};
  std::string out(reinterpret_cast<const char *>(data_ + at_), n);
  at_ += n;
  if (!validUtf8(out)) {
    ok_ = false;
    return {};
  }
  return out;
}

std::vector<uint8_t> WireReader::bytes() {
  const uint32_t n = u32();
  if (!need(n)) return {};
  std::vector<uint8_t> out(data_ + at_, data_ + at_ + n);
  at_ += n;
  return out;
}

WireWriter &WireWriter::u8(uint8_t v) {
  out.push_back(v);
  return *this;
}

WireWriter &WireWriter::u16(uint16_t v) {
  out.push_back(static_cast<uint8_t>(v));
  out.push_back(static_cast<uint8_t>(v >> 8));
  return *this;
}

WireWriter &WireWriter::u32(uint32_t v) {
  for (int i = 0; i < 4; ++i) out.push_back(static_cast<uint8_t>(v >> (8 * i)));
  return *this;
}

WireWriter &WireWriter::str(std::string_view s) {
  u32(static_cast<uint32_t>(s.size()));
  out.insert(out.end(), s.begin(), s.end());
  return *this;
}

WireWriter &WireWriter::bytes(const uint8_t *data, std::size_t len) {
  u32(static_cast<uint32_t>(len));
  if (len > 0) out.insert(out.end(), data, data + len);
  return *this;
}

WireWriter &WireWriter::strings(const std::vector<std::string> &items) {
  u32(static_cast<uint32_t>(items.size()));
  for (const std::string &item : items) str(item);
  return *this;
}

bool validUtf8(std::string_view text) noexcept {
  std::size_t i = 0;
  const auto *s = reinterpret_cast<const unsigned char *>(text.data());
  const std::size_t n = text.size();
  while (i < n) {
    const unsigned char c = s[i];
    if (c < 0x80) {
      ++i;
      continue;
    }
    std::size_t len = 0;
    uint32_t cp = 0;
    if ((c & 0xe0) == 0xc0) {
      len = 2;
      cp = c & 0x1f;
    } else if ((c & 0xf0) == 0xe0) {
      len = 3;
      cp = c & 0x0f;
    } else if ((c & 0xf8) == 0xf0) {
      len = 4;
      cp = c & 0x07;
    } else {
      return false;
    }
    if (n - i < len) return false;
    for (std::size_t k = 1; k < len; ++k) {
      if ((s[i + k] & 0xc0) != 0x80) return false;
      cp = (cp << 6) | (s[i + k] & 0x3f);
    }
    if ((len == 2 && cp < 0x80) || (len == 3 && cp < 0x800) || (len == 4 && cp < 0x10000)) return false;
    if (cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) return false;
    i += len;
  }
  return true;
}

// ----- hashes -----------------------------------------------------------------------------------

uint32_t fnv1a32Of(std::string_view text) noexcept {
  uint32_t hash = 0x811c9dc5u;
  for (char c : text) {
    hash ^= static_cast<uint8_t>(c);
    hash *= 0x01000193u;
  }
  return hash;
}

uint64_t fnv1a64Of(std::string_view text) noexcept {
  uint64_t hash = 0xcbf29ce484222325ull;
  for (char c : text) {
    hash ^= static_cast<uint8_t>(c);
    hash *= 0x00000100000001b3ull;
  }
  return hash;
}

std::string sha256Hex(std::string_view text) {
  std::array<uint32_t, 8> h{0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19};
  const auto *data = reinterpret_cast<const uint8_t *>(text.data());
  const std::size_t len = text.size();
  std::size_t at = 0;
  for (; len - at >= 64; at += 64) sha256Block(h, data + at);
  uint8_t tail[128] = {};
  const std::size_t rest = len - at;
  if (rest > 0) std::memcpy(tail, data + at, rest);
  tail[rest] = 0x80;
  const std::size_t blocks = rest + 1 + 8 <= 64 ? 1 : 2;
  const uint64_t bits = static_cast<uint64_t>(len) * 8;
  for (int i = 0; i < 8; ++i) tail[blocks * 64 - 1 - static_cast<std::size_t>(i)] = static_cast<uint8_t>(bits >> (8 * i));
  for (std::size_t b = 0; b < blocks; ++b) sha256Block(h, tail + 64 * b);
  std::string out;
  out.reserve(64);
  for (uint32_t word : h) out += hexOf(word, 8);
  return out;
}

// ----- directories ------------------------------------------------------------------------------

bool makeDirectories(const std::string &path, std::string &error) {
  if (path.empty()) {
    error = "no directory";
    return false;
  }
  struct stat st{};
  if (::stat(path.c_str(), &st) == 0) {
    if (S_ISDIR(st.st_mode)) return true;
    error = path + " is not a directory";
    return false;
  }
  std::size_t at = 1;
  while (true) {
    const std::size_t slash = path.find('/', at);
    const std::string prefix = slash == std::string::npos ? path : path.substr(0, slash);
    if (!prefix.empty() && ::mkdir(prefix.c_str(), 0700) != 0 && errno != EEXIST) {
      error = "cannot create " + prefix + ": " + errnoText(errno);
      return false;
    }
    if (slash == std::string::npos) break;
    at = slash + 1;
  }
  if (::stat(path.c_str(), &st) != 0 || !S_ISDIR(st.st_mode)) {
    error = "cannot create " + path;
    return false;
  }
  return true;
}

// ----- KvStore ----------------------------------------------------------------------------------

std::string kvFileName(KvNaming naming, std::string_view key) {
  switch (naming) {
    case KvNaming::Fnv:
      return hexOf(fnv1a64Of(key), 16) + "-" + hexOf(fnv1a32Of(key), 8);
    case KvNaming::Sha256:
      return sha256Hex(key);
  }
  return sha256Hex(key);
}

KvStore::KvStore(std::string directory, KvNaming naming) : directory_(std::move(directory)), naming_(naming) {}

std::string KvStore::pathOf(const std::string &key) const {
  return directory_ + "/" + kvFileName(naming_, key);
}

bool KvStore::readKey(const std::string &path, std::optional<std::string> &key, std::string &error) const {
  key.reset();
  Fd in(::open(path.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK));
  if (!in.valid()) {
    if (errno == ENOENT) return true;
    error = "cannot open " + path + ": " + errnoText(errno);
    return false;
  }
  struct stat st{};
  if (::fstat(in.get(), &st) != 0 || !S_ISREG(st.st_mode)) return true; // not an entry
  uint8_t head[4];
  std::size_t got = 0;
  int code = 0;
  if (!readExactly(in.get(), head, sizeof(head), got, code)) {
    error = "cannot read " + path + ": " + errnoText(code);
    return false;
  }
  if (got < sizeof(head)) return true;
  const uint32_t len = static_cast<uint32_t>(head[0]) | (static_cast<uint32_t>(head[1]) << 8) |
      (static_cast<uint32_t>(head[2]) << 16) | (static_cast<uint32_t>(head[3]) << 24);
  // A length that does not fit in the file is not an entry; the check also bounds the allocation.
  if (static_cast<uint64_t>(len) > static_cast<uint64_t>(st.st_size) - sizeof(head)) return true;
  std::string text(len, '\0');
  if (!readExactly(in.get(), reinterpret_cast<uint8_t *>(text.data()), len, got, code)) {
    error = "cannot read " + path + ": " + errnoText(code);
    return false;
  }
  if (got < len || !validUtf8(text)) return true;
  key = std::move(text);
  return true;
}

bool KvStore::get(const std::string &key, std::optional<std::vector<uint8_t>> &value, std::string &error) const {
  value.reset();
  const std::string path = pathOf(key);
  Fd in(::open(path.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK));
  if (!in.valid()) {
    if (errno == ENOENT || errno == ENOTDIR) return true;
    error = "cannot open the entry of '" + key + "': " + errnoText(errno);
    return false;
  }
  std::vector<uint8_t> bytes;
  int code = 0;
  if (!readAll(in.get(), bytes, code)) {
    error = "cannot read the entry of '" + key + "': " + errnoText(code);
    return false;
  }
  WireReader reader(bytes.data(), bytes.size());
  const std::string stored = reader.str();
  if (!reader.ok()) {
    error = "the entry file of '" + key + "' is damaged (" + path + ")";
    return false;
  }
  if (stored != key) return true; // a name collision: this key has no value
  const std::size_t header = 4 + stored.size();
  value.emplace(bytes.begin() + static_cast<std::ptrdiff_t>(header), bytes.end());
  return true;
}

bool KvStore::set(const std::string &key, const uint8_t *value, std::size_t len, std::string &error) const {
  if (!makeDirectories(directory_, error)) return false;
  Fd dir(::open(directory_.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC));
  if (!dir.valid()) {
    error = "cannot open " + directory_ + ": " + errnoText(errno);
    return false;
  }
  WireWriter entry;
  entry.out.reserve(4 + key.size() + len);
  entry.str(key);
  if (len > 0) entry.out.insert(entry.out.end(), value, value + len);
  const std::string name = kvFileName(naming_, key);
  const std::string temp = name + "." + randomHex() + std::string(kKvTempSuffix);
  int code = 0;
  if (!atomicWriteAt(dir.get(), name, temp, entry.out.data(), entry.out.size(), 0600, code)) {
    error = "cannot write the entry of '" + key + "': " + errnoText(code);
    return false;
  }
  return true;
}

bool KvStore::remove(const std::string &key, std::string &error) const {
  const std::string path = pathOf(key);
  std::optional<std::string> stored;
  if (!readKey(path, stored, error)) return false;
  // Removing a colliding key's file would lose someone else's value.
  if (stored && *stored != key) return true;
  if (::unlink(path.c_str()) != 0 && errno != ENOENT && errno != ENOTDIR) {
    error = "cannot remove the entry of '" + key + "': " + errnoText(errno);
    return false;
  }
  return true;
}

bool KvStore::list(const std::string &prefix, std::vector<std::string> &keys, std::string &error) const {
  keys.clear();
  DIR *stream = ::opendir(directory_.c_str());
  if (stream == nullptr) {
    if (errno == ENOENT || errno == ENOTDIR) return true;
    error = "cannot list " + directory_ + ": " + errnoText(errno);
    return false;
  }
  DirCloser closer{stream};
  std::vector<std::string> names;
  while (struct dirent *entry = ::readdir(stream)) {
    const std::string name = entry->d_name;
    if (name.empty() || name[0] == '.' || endsWith(name, kKvTempSuffix)) continue;
    names.push_back(name);
  }
  for (const std::string &name : names) {
    std::optional<std::string> key;
    std::string ignored;
    // A file that vanished or cannot be read meanwhile is skipped, as the native runtimes do.
    if (!readKey(directory_ + "/" + name, key, ignored)) continue;
    if (key && startsWith(*key, prefix)) keys.push_back(std::move(*key));
  }
  std::sort(keys.begin(), keys.end());
  return true;
}

// ----- FsRoot -----------------------------------------------------------------------------------

FsRoot::FsRoot(std::string root) : root_(std::move(root)) {}

std::optional<FsFailure> FsRoot::read(const std::string &path, std::vector<uint8_t> &out) const {
  std::vector<std::string> parts;
  if (auto failure = splitPath(path, parts)) return failure;
  if (parts.empty()) return io("the path names the root, a directory");
  Fd parent;
  if (auto failure = openChain(root_, parts, parts.size() - 1, false, parent)) return failure;
  Fd in(::openat(parent.get(), parts.back().c_str(), O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC));
  if (!in.valid()) {
    const int code = errno;
    if (code == ELOOP || code == EMLINK) {
      mode_t kind = 0;
      if (auto failure = kindAt(parent.get(), parts.back(), kind)) return failure;
      if (kind == S_IFLNK) return denied();
    }
    return fromErrno(code);
  }
  struct stat st{};
  if (::fstat(in.get(), &st) != 0) return fromErrno(errno);
  if (S_ISDIR(st.st_mode)) return io(path + " is a directory");
  if (!S_ISREG(st.st_mode)) return io(path + " is not a regular file");
  int code = 0;
  if (!readAll(in.get(), out, code)) return fromErrno(code);
  return std::nullopt;
}

std::optional<FsFailure> FsRoot::write(const std::string &path, const uint8_t *data, std::size_t len) const {
  std::vector<std::string> parts;
  if (auto failure = splitPath(path, parts)) return failure;
  if (parts.empty()) return io("the path names the root, a directory");
  Fd parent;
  if (auto failure = openChain(root_, parts, parts.size() - 1, true, parent)) return failure;
  mode_t kind = 0;
  if (auto failure = kindAt(parent.get(), parts.back(), kind)) return failure;
  if (kind == S_IFLNK) return denied();
  if (kind == S_IFDIR) return io(path + " is a directory");
  const std::string temp = std::string(kFsTempPrefix) + randomHex();
  int code = 0;
  if (!atomicWriteAt(parent.get(), parts.back(), temp, data, len, 0644, code)) return fromErrno(code);
  return std::nullopt;
}

std::optional<FsFailure> FsRoot::remove(const std::string &path) const {
  std::vector<std::string> parts;
  if (auto failure = splitPath(path, parts)) return failure;
  if (parts.empty()) return denied(); // never the root
  Fd parent;
  if (auto failure = openChain(root_, parts, parts.size() - 1, false, parent)) return failure;
  mode_t kind = 0;
  if (auto failure = kindAt(parent.get(), parts.back(), kind)) return failure;
  if (kind == 0) return notFound();
  if (kind == S_IFDIR) return removeTree(parent.get(), parts.back());
  // A file, or a link (removed, never followed).
  if (::unlinkat(parent.get(), parts.back().c_str(), 0) != 0) return fromErrno(errno);
  return std::nullopt;
}

std::optional<FsFailure> FsRoot::list(const std::string &dir, std::vector<std::string> &names) const {
  names.clear();
  std::vector<std::string> parts;
  if (auto failure = splitPath(dir, parts)) return failure;
  Fd at;
  if (parts.empty()) {
    if (auto failure = openChain(root_, parts, 0, false, at)) return failure;
  } else {
    Fd parent;
    if (auto failure = openChain(root_, parts, parts.size() - 1, false, parent)) return failure;
    if (auto failure = openDirAt(parent.get(), parts.back(), false, at)) {
      // A file is not a directory to list (as Swift and the JVM answer), not a missing one.
      mode_t kind = 0;
      if (failure->kind == FsErrorKind::NotFound && !kindAt(parent.get(), parts.back(), kind) && kind == S_IFREG) {
        return io(dir + " is not a directory");
      }
      return failure;
    }
  }
  Fd forStream(::dup(at.get()));
  if (!forStream.valid()) return fromErrno(errno);
  DIR *stream = ::fdopendir(forStream.get());
  if (stream == nullptr) return fromErrno(errno);
  forStream.release(); // the stream owns it now
  DirCloser closer{stream};
  while (struct dirent *entry = ::readdir(stream)) {
    const std::string name = entry->d_name;
    if (name == "." || name == ".." || startsWith(name, kFsTempPrefix)) continue;
    names.push_back(name);
  }
  std::sort(names.begin(), names.end());
  return std::nullopt;
}

} // namespace undra::rn
