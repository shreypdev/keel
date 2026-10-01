// The portable `Kv` and `Fs` of @undra/react-native (cpp/UndraStores.*), on this machine's file
// system, without a core: the rules of docs/SPEC.md section 8 and ADR-038 amendment B (B4, B5), the
// file layouts the native runtimes use (the Swift `KvAdapter`'s names on iOS, `android-adapters`'
// on Android, checked against their own test vectors), and the hashes against published vectors.
// `run.sh` builds it with AddressSanitizer and UndefinedBehaviorSanitizer.
//
// Prints `ok - <name>` per check and exits non-zero on the first failure.

#include <dirent.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <iterator>
#include <optional>
#include <string>
#include <vector>

#include "../UndraStores.h"

using namespace undra::rn;

namespace {

int g_checks = 0;

[[noreturn]] void fail(const std::string &what) {
  std::fprintf(stderr, "not ok - %s\n", what.c_str());
  std::exit(1);
}

void check(bool condition, const std::string &what) {
  if (!condition) fail(what);
}

void ok(const char *name) {
  ++g_checks;
  std::printf("ok - %s\n", name);
}

std::string g_base;

/// A fresh directory under the test's temporary directory.
std::string freshDir(const std::string &name) {
  const std::string path = g_base + "/" + name;
  std::string error;
  check(makeDirectories(path, error), "create " + path + ": " + error);
  return path;
}

std::vector<uint8_t> bytesOf(const std::string &text) {
  return std::vector<uint8_t>(text.begin(), text.end());
}

std::string readFile(const std::string &path) {
  std::ifstream in(path, std::ios::binary);
  return std::string(std::istreambuf_iterator<char>(in), std::istreambuf_iterator<char>());
}

void writeFile(const std::string &path, const std::string &contents) {
  std::ofstream out(path, std::ios::binary);
  out << contents;
}

bool exists(const std::string &path) {
  struct stat st{};
  return ::lstat(path.c_str(), &st) == 0;
}

std::vector<std::string> entries(const std::string &dir) {
  std::vector<std::string> out;
  if (DIR *d = ::opendir(dir.c_str())) {
    while (struct dirent *e = ::readdir(d)) {
      const std::string name = e->d_name;
      if (name != "." && name != "..") out.push_back(name);
    }
    ::closedir(d);
  }
  return out;
}

std::string entryBytes(const std::string &key, const std::string &value) {
  std::string out;
  const auto n = static_cast<uint32_t>(key.size());
  for (int i = 0; i < 4; ++i) out.push_back(static_cast<char>(n >> (8 * i)));
  return out + key + value;
}

void testHashes() {
  check(sha256Hex("") == "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", "SHA-256 of nothing");
  check(sha256Hex("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq") ==
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        "SHA-256 of the FIPS 180-2 two-block vector");
  const std::pair<std::size_t, const char *> padding[] = {
      {3, "cd2eb0837c9b4c962c22d2ff8b5441b7b45805887f051d39bf133b583baf6860"},
      {55, "d5e285683cd4efc02d021a5c62014694958901005d6f71e89e0989fac77e4072"},
      {56, "04c26261370ee7541549d16dee320c723e3fd14671e66a099afe0a377c16888e"},
      {63, "75220b47218278e656f2013bb8f0c455a25eaf01e86c64924e9d48d89776d6f2"},
      {64, "7ce100971f64e7001e8fe5a51973ecdfe1ced42befe7ee8d5fd6219506b5393c"},
      {65, "9537c5fdf120482f7d58d25e9ed583f52c02b4e304ea814db1633ad565aed7e9"},
      {119, "000b48d4edf0fa7bee3c6236ecd2785baa5db4eeb8bb54341b029e0d9fa5fb0c"},
      {120, "13f05a0b594787f5ecd315edc96141bd3243203d1b7d4f0836f37308b276ba98"},
      {1000, "44f8354494a5ba03ba1792a8d3e9c534c47a9181980fde7a3f44b06ef2ae7c7f"},
  };
  for (const auto &[n, hex] : padding) {
    check(sha256Hex(std::string(n, 'x')) == hex, "SHA-256 of " + std::to_string(n) + " bytes (padding edges)");
  }
  ok("SHA-256 matches FIPS 180 vectors at every padding edge");

  // The Swift runtime's own vectors (AdapterTests.swift, FileKeyValueBackend.fileName(for:)).
  check(kvFileName(KvNaming::Fnv, "a") == "af63dc4c8601ec8c-e40c292c", "Swift name of 'a'");
  check(kvFileName(KvNaming::Fnv, "") == "cbf29ce484222325-811c9dc5", "Swift name of ''");
  check(kvFileName(KvNaming::Fnv, "undra.query.queue") == "def1907793b60cec-f2daf06c", "Swift name of the queue key");
  check(kvFileName(KvNaming::Fnv, "h\xc3\xa9llo") == "a35ff71f960240e0-4aa48540", "Swift name hashes UTF-8 bytes");
  // android-adapters' FileKv: the SHA-256 of the key's UTF-8, in hex.
  check(kvFileName(KvNaming::Sha256, "undra.query.queue") == "9334218c1145c27b4af000b3198a409d4f15f5726bc23f2c6f4f7704b8a37efa",
        "Android name of the queue key");
  check(kvFileName(KvNaming::Sha256, "h\xc3\xa9llo") == "3c48591d8d098a4538f5e013dfcf406e948eac4d3277b10bf614e295d6068179",
        "Android name hashes UTF-8 bytes");
  ok("Kv file names are the Swift runtime's (FNV) and android-adapters' (SHA-256)");

  check(validUtf8("plain") && validUtf8("h\xc3\xa9llo") && validUtf8("\xf0\x9f\x98\x80"), "valid UTF-8 passes");
  check(!validUtf8("\xc0\xaf") && !validUtf8("\xed\xa0\x80") && !validUtf8("\xf4\x90\x80\x80") && !validUtf8("\xe2\x82"),
        "overlongs, surrogates, > U+10FFFF and truncation are refused");
  ok("UTF-8 validation refuses overlongs, surrogates and truncation");
}

void testKv(KvNaming naming, const char *label) {
  const std::string dir = g_base + "/kv-" + label;
  KvStore kv(dir, naming);
  std::string error;
  std::optional<std::vector<uint8_t>> value;
  std::vector<std::string> keys;

  check(kv.get("missing", value, error) && !value, "a missing key reads as none before the directory exists");
  check(kv.list("", keys, error) && keys.empty(), "an empty store lists nothing before the directory exists");
  check(kv.remove("missing", error), "removing a missing key is not an error");

  const std::vector<uint8_t> v1 = bytesOf("one");
  check(kv.set("b.key", v1.data(), v1.size(), error), "set: " + error);
  const std::string file = dir + "/" + kvFileName(naming, "b.key");
  check(readFile(file) == entryBytes("b.key", "one"), "the entry file is `u32 key length, key, value`");
  struct stat st{};
  check(::stat(file.c_str(), &st) == 0 && (st.st_mode & 0777) == 0600, "the entry file is owner-only");
  check(kv.get("b.key", value, error) && value && *value == v1, "get returns what set stored");

  const std::vector<uint8_t> v2 = bytesOf("two, longer");
  check(kv.set("b.key", v2.data(), v2.size(), error), "overwrite");
  check(kv.get("b.key", value, error) && value && *value == v2, "get returns the new value");
  check(kv.set("a.key", nullptr, 0, error), "an empty value");
  check(kv.get("a.key", value, error) && value && value->empty(), "an empty value reads back empty, not none");
  const std::string unicode = "cl\xc3\xa9/\xf0\x9f\x94\x91 with spaces";
  check(kv.set(unicode, v1.data(), v1.size(), error), "a key with slashes, spaces and non-ASCII");
  std::vector<uint8_t> big(1 << 20);
  for (std::size_t i = 0; i < big.size(); ++i) big[i] = static_cast<uint8_t>(i * 7);
  check(kv.set("big", big.data(), big.size(), error), "a 1 MiB value");
  check(kv.get("big", value, error) && value && *value == big, "the 1 MiB value reads back");

  for (const std::string &name : entries(dir)) {
    check(name.size() < 4 || name.substr(name.size() - 4) != ".tmp", "no temporary file is left behind: " + name);
  }

  // Things in the directory that are not entries are skipped by list.
  writeFile(dir + "/.keystore.lock", "");
  writeFile(dir + "/junk", "\x01\x02");
  writeFile(dir + "/leftover.1234.tmp", entryBytes("b.leftover", "x"));
  writeFile(dir + "/badutf8", entryBytes("b.\xff", "x"));
  writeFile(dir + "/toolong", std::string("\xff\xff\xff\x7f", 4) + "abc");
  check(kv.list("", keys, error), "list: " + error);
  check((keys == std::vector<std::string>{"a.key", "b.key", "big", unicode}), "list returns every key, sorted by byte order");
  check(kv.list("b.", keys, error) && (keys == std::vector<std::string>{"b.key"}), "list filters by prefix");
  check(kv.list("zzz", keys, error) && keys.empty(), "no key with that prefix");
  ok((std::string(label) + ": Kv set, get, overwrite and list, entries in the native layout, junk skipped").c_str());

  // A file at a key's name that holds another key (a name collision): not this key's value, and
  // removing this key leaves it alone.
  writeFile(dir + "/" + kvFileName(naming, "victim"), entryBytes("other", "theirs"));
  check(kv.get("victim", value, error) && !value, "a colliding file is not this key's value");
  check(kv.remove("victim", error) && exists(dir + "/" + kvFileName(naming, "victim")), "removing this key keeps the other key's file");
  // A damaged entry at the key's own name is a failure, never "missing".
  writeFile(dir + "/" + kvFileName(naming, "damaged"), "\x09\x00");
  check(!kv.get("damaged", value, error) && error.find("damaged") != std::string::npos, "a damaged entry is an error: " + error);
  check(kv.remove("b.key", error) && kv.get("b.key", value, error) && !value, "remove");
  ok((std::string(label) + ": Kv collisions never read or remove another key; a damaged entry is an error").c_str());
}

void testFs() {
  const std::string base = freshDir("fs-case");
  const std::string root = base + "/root";
  const std::string outside = freshDir("fs-case/outside");
  writeFile(outside + "/secret.txt", "outside");
  FsRoot fs(root);
  std::vector<uint8_t> data;
  std::vector<std::string> names;

  auto is = [](const std::optional<FsFailure> &failure, FsErrorKind kind) { return failure && failure->kind == kind; };

  // Before the root exists.
  check(is(fs.read("a.txt", data), FsErrorKind::NotFound), "read before the root exists: NotFound");
  check(is(fs.list("", names), FsErrorKind::NotFound), "list of a missing root: NotFound");

  const std::vector<uint8_t> hello = bytesOf("hello");
  check(!fs.write("notes/2026/a.txt", hello.data(), hello.size()), "write creates the root and the parents");
  check(readFile(root + "/notes/2026/a.txt") == "hello", "the file is under the root");
  check(!fs.read("notes/2026/a.txt", data) && data == hello, "read it back");
  check(!fs.read("/notes/./2026//a.txt", data) && data == hello, "a leading '/', '.' and empty components are ignored");
  const std::vector<uint8_t> bye = bytesOf("bye");
  check(!fs.write("notes/2026/a.txt", bye.data(), bye.size()), "overwrite");
  check(!fs.read("notes/2026/a.txt", data) && data == bye, "the new contents");
  check(!fs.write("notes/b.txt", nullptr, 0), "an empty file");
  check(!fs.read("notes/b.txt", data) && data.empty(), "reads back empty");
  writeFile(root + "/notes/.undra-tmp-0123456789abcdef", "half a write");
  check(!fs.list("notes", names) && (names == std::vector<std::string>{"2026", "b.txt"}), "list: one directory, sorted, temporary files hidden");
  check(!fs.list("", names) && (names == std::vector<std::string>{"notes"}), "list of the root");
  check(!fs.list("/", names) && (names == std::vector<std::string>{"notes"}), "list of '/' is the root");
  ok("Fs write (atomic, parents created), read, overwrite, list");

  check(is(fs.read("nope.txt", data), FsErrorKind::NotFound), "a missing file: NotFound");
  check(is(fs.read("nope/a.txt", data), FsErrorKind::NotFound), "a missing directory: NotFound");
  check(is(fs.read("notes", data), FsErrorKind::Io), "reading a directory: Io");
  check(is(fs.read("", data), FsErrorKind::Io) && is(fs.write("", hello.data(), hello.size()), FsErrorKind::Io), "the root is a directory: Io");
  check(is(fs.write("notes", hello.data(), hello.size()), FsErrorKind::Io), "writing over a directory: Io");
  check(is(fs.read("notes/b.txt/x", data), FsErrorKind::NotFound), "a file where a directory should be: NotFound on read");
  check(is(fs.write("notes/b.txt/x", hello.data(), hello.size()), FsErrorKind::Io), "and Io on write");
  check(is(fs.list("notes/b.txt", names), FsErrorKind::Io), "listing a file: Io");
  check(is(fs.list("missing", names), FsErrorKind::NotFound), "listing a missing directory: NotFound");
  check(is(fs.read(std::string("a\0b", 3), data), FsErrorKind::Io), "a NUL byte: Io");
  check(is(fs.remove("missing.txt"), FsErrorKind::NotFound), "deleting a missing file: NotFound");
  ok("Fs typed errors: NotFound, Io for directories and NUL");

  for (const char *path : {"../secret.txt", "../outside/secret.txt", "notes/../../outside/secret.txt", "notes/..", "..", "a/b/../c"}) {
    check(is(fs.read(path, data), FsErrorKind::Denied), std::string("read ") + path + ": Denied");
    check(is(fs.write(path, hello.data(), hello.size()), FsErrorKind::Denied), std::string("write ") + path + ": Denied");
    check(is(fs.remove(path), FsErrorKind::Denied), std::string("delete ") + path + ": Denied");
    check(is(fs.list(path, names), FsErrorKind::Denied), std::string("list ") + path + ": Denied");
  }
  check(readFile(outside + "/secret.txt") == "outside", "nothing outside was touched");
  check(is(fs.remove(""), FsErrorKind::Denied) && is(fs.remove("/"), FsErrorKind::Denied) && is(fs.remove("./"), FsErrorKind::Denied),
        "the root cannot be deleted");
  check(exists(root), "the root is still there");
  ok("Fs: any '..' is Denied, and the root is never deleted");

  // Symbolic links: out of the root, into the root, dangling; in the middle and at the end.
  check(::symlink(outside.c_str(), (root + "/out").c_str()) == 0, "a link to a directory outside");
  check(::symlink((outside + "/secret.txt").c_str(), (root + "/out.txt").c_str()) == 0, "a link to a file outside");
  check(::symlink((root + "/notes").c_str(), (root + "/in").c_str()) == 0, "a link to a directory inside");
  check(::symlink((root + "/missing").c_str(), (root + "/dangling").c_str()) == 0, "a dangling link");
  for (const char *path : {"out/secret.txt", "out.txt", "in/b.txt", "dangling", "dangling/x"}) {
    check(is(fs.read(path, data), FsErrorKind::Denied), std::string("read through the link ") + path + ": Denied");
  }
  for (const char *path : {"out/new.txt", "out.txt", "in/new.txt", "dangling", "dangling/x"}) {
    check(is(fs.write(path, hello.data(), hello.size()), FsErrorKind::Denied), std::string("write through the link ") + path + ": Denied");
  }
  check(is(fs.list("out", names), FsErrorKind::Denied) && is(fs.list("in", names), FsErrorKind::Denied), "list through a link: Denied");
  check(is(fs.remove("out/secret.txt"), FsErrorKind::Denied), "delete through a link: Denied");
  check(readFile(outside + "/secret.txt") == "outside" && !exists(outside + "/new.txt"), "nothing outside was read, written or deleted");
  check(!exists(root + "/notes/new.txt"), "nothing was written through the inside link either");
  check(!fs.remove("out.txt") && !exists(root + "/out.txt") && readFile(outside + "/secret.txt") == "outside",
        "deleting a link removes the link, never its target");
  check(!fs.remove("out") && !exists(root + "/out") && exists(outside + "/secret.txt"), "deleting a link to a directory keeps the directory");
  ok("Fs: any symbolic link on the path is Denied; delete removes a link without following it");

  // Recursive delete, with a link to the outside inside the tree.
  check(!fs.write("tree/a/b/c.txt", hello.data(), hello.size()) && !fs.write("tree/a/d.txt", hello.data(), hello.size()), "a tree");
  check(::symlink(outside.c_str(), (root + "/tree/a/escape").c_str()) == 0, "a link out inside the tree");
  check(!fs.remove("tree"), "delete a directory with everything in it");
  check(!exists(root + "/tree") && readFile(outside + "/secret.txt") == "outside", "gone, and the link's target untouched");
  std::string deep = "deep";
  for (int i = 0; i < 300; ++i) deep += "/d";
  check(!fs.write(deep + "/leaf.txt", hello.data(), hello.size()), "a tree 300 directories deep");
  check(!fs.remove("deep") && !exists(root + "/deep"), "deleted with a bounded number of descriptors");
  check(!fs.remove("notes/b.txt") && is(fs.read("notes/b.txt", data), FsErrorKind::NotFound), "delete one file");
  ok("Fs delete removes a tree of any depth and never follows the links in it");

  if (::geteuid() != 0) {
    const std::string locked = root + "/locked";
    check(::mkdir(locked.c_str(), 0700) == 0, "a directory");
    writeFile(locked + "/f.txt", "x");
    check(::chmod(locked.c_str(), 0) == 0, "made unreadable");
    check(is(fs.read("locked/f.txt", data), FsErrorKind::Denied), "a refused permission: Denied");
    check(is(fs.list("locked", names), FsErrorKind::Denied), "listing it: Denied");
    ::chmod(locked.c_str(), 0700);
    ok("Fs: a refused permission is Denied");
  } else {
    std::printf("# skipped the permission check: running as root\n");
  }
}

} // namespace

int main() {
  char pattern[] = "/tmp/undra-rn-stores.XXXXXX";
  const char *dir = ::mkdtemp(pattern);
  if (dir == nullptr) fail("mkdtemp");
  // Resolved, so the links of the Fs checks point at real paths (macOS's /tmp is a link).
  char real[4096];
  g_base = ::realpath(dir, real) != nullptr ? real : dir;
  testHashes();
  testKv(KvNaming::Fnv, "fnv");
  testKv(KvNaming::Sha256, "sha256");
  testFs();
  std::string cmd = "rm -rf '" + g_base + "'";
  if (std::system(cmd.c_str()) != 0) std::printf("# could not remove %s\n", g_base.c_str());
  std::printf("# %d checks passed\n", g_checks);
  return 0;
}
