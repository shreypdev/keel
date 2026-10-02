// The Apple platform of @undra/react-native's default ports (ios/UndraPlatformApple.mm) on this Mac: where each
// core's stores are (ADR-044 amendment A). Every default store is per core namespace, in the Swift runtime's
// layout: `<Application Support>/<bundle id>/undra/<namespace>/{kv,fs,db}` and the Keychain service
// `<namespace>.dev.undra.securestore`, so two cores of one app never share one, and the SwiftUI shell and the
// React Native shell of an app agree where a core's data is. `run.sh` builds it with the platform's sources; it
// touches no file and no Keychain item (it only asks where they would be).
//
// Prints `ok - <name>` per check and exits non-zero on the first failure.

#import <Foundation/Foundation.h>

#include <cstdio>
#include <cstdlib>
#include <string>

#include "../UndraDefaults.h"

using namespace undra::rn;

namespace {

int g_checks = 0;

void check(bool condition, const std::string &what) {
  if (!condition) {
    std::fprintf(stderr, "not ok - %s\n", what.c_str());
    std::exit(1);
  }
  ++g_checks;
}

void ok(const char *name) {
  std::printf("ok - %s\n", name);
}

bool endsWith(const std::string &text, const std::string &suffix) {
  return text.size() >= suffix.size() && text.compare(text.size() - suffix.size(), suffix.size(), suffix) == 0;
}

} // namespace

int main() {
  std::string error;
  std::unique_ptr<Platform> a = makePlatform("playground_a", error);
  std::unique_ptr<Platform> b = makePlatform("playground_b", error);
  check(a != nullptr && b != nullptr, "the Apple platform is made for a namespace");

  const std::string kvA = a->kvDirectory();
  const std::string kvB = b->kvDirectory();
  check(endsWith(kvA, "/undra/playground_a/kv"), "Kv is <Application Support>/<bundle id>/undra/<namespace>/kv, got " + kvA);
  check(endsWith(kvB, "/undra/playground_b/kv"), "the other core's Kv is its own, got " + kvB);
  check(kvA != kvB, "two namespaces, two Kv directories");
  check(kvA.find("/Application Support/") != std::string::npos || kvA.find("/tmp") != std::string::npos || kvA.find("/T/") != std::string::npos,
      "under the app's Application Support (or the temporary directory where there is none), got " + kvA);
  ok("Kv: <Application Support>/<bundle id>/undra/<namespace>/kv, one per core");

  const std::string fsA = a->fsRoot();
  check(endsWith(fsA, "/undra/playground_a/fs") && endsWith(b->fsRoot(), "/undra/playground_b/fs") && fsA != b->fsRoot(),
      "Fs is .../undra/<namespace>/fs, one per core, got " + fsA);
  // The Kv and Fs of a core are siblings, as the Swift runtime's are.
  check(fsA.substr(0, fsA.size() - 2) == kvA.substr(0, kvA.size() - 2), "Kv and Fs of a core share the namespace directory");
  ok("Fs: .../undra/<namespace>/fs, one per core");

  std::unique_ptr<SecretStore> secretsA = a->makeSecretStore();
  std::unique_ptr<SecretStore> secretsB = b->makeSecretStore();
  check(secretsA != nullptr && secretsB != nullptr, "the platform has a secret store");
  check(secretsA->describe() == "Keychain service playground_a.dev.undra.securestore", "SecureStore: the service is <namespace>.dev.undra.securestore, got " + secretsA->describe());
  check(secretsB->describe() == "Keychain service playground_b.dev.undra.securestore" && secretsA->describe() != secretsB->describe(),
      "the other core's items are in its own service");
  ok("SecureStore: Keychain service <namespace>.dev.undra.securestore, one per core");

  std::unique_ptr<DbBackend> dbA = a->makeDbBackend();
  std::unique_ptr<DbBackend> dbB = b->makeDbBackend();
  check(dbA != nullptr && dbB != nullptr, "the platform has a SQLite");
  check(dbA->describe().find("/undra/playground_a/db") != std::string::npos, "Db is .../undra/<namespace>/db, got " + dbA->describe());
  check(dbB->describe().find("/undra/playground_b/db") != std::string::npos && dbA->describe() != dbB->describe(), "the other core's databases are its own");
  ok("Db: .../undra/<namespace>/db/<name>.sqlite, one per core");

  std::printf("# %d checks\n", g_checks);
  return 0;
}
