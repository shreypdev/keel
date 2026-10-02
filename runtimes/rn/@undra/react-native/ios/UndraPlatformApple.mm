// The Apple platform of @undra/react-native's default ports (ADR-038 amendment B, B1): Objective-C++
// over the C APIs of Foundation, Security and Network.framework; no Swift.
//
//  * `Kv` and `Fs`: the portable C++ stores over `<Application Support>/<bundle id>/undra/<namespace>/kv`
//    (FNV names) and `.../undra/<namespace>/fs`: the directories and names of the Swift runtime's
//    `KvAdapter` and `FsAdapter` for the core's namespace (ADR-044 amendment A: two cores of one app never
//    share a store), so a value the SwiftUI shell of an app wrote is read here and the reverse;
//  * `SecureStore`: Keychain generic passwords, service `<namespace>.dev.undra.securestore`, account =
//    key, `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`: the Swift `SecureStoreAdapter`'s items;
//  * `Connectivity`: `nw_path_monitor` on a serial queue of its own, classified as the Swift
//    `ConnectivityAdapter` does (online when the path is satisfied; Wi-Fi, cellular, wired, unknown);
//  * `Db` (ADR-048): the sqlite3 C API of the system `libsqlite3` (`cpp/UndraDbSqlite.cpp`) over
//    `<Application Support>/<bundle id>/undra/<namespace>/db/<name>.sqlite`, the Swift `SQLiteDbAdapter`'s
//    files (`SQLiteDbAdapter.defaultDirectory(namespace:)`), so either shell reads the other's database.
#import <Foundation/Foundation.h>
#import <Network/Network.h>
#import <Security/Security.h>

#include <algorithm>
#include <atomic>
#include <memory>
#include <string>
#include <vector>

#include "UndraDefaults.h"

namespace undra::rn {

namespace {

NSString *nsString(const std::string &text) {
  return [[NSString alloc] initWithBytes:text.data() length:text.size() encoding:NSUTF8StringEncoding];
}

/// `<Application Support>/<bundle id>/undra/<namespace>/<name>` (the Swift
/// `KvAdapter.defaultDirectory(namespace:named:)`).
std::string undraDirectory(const std::string &name_space, NSString *name) {
  @autoreleasepool {
    NSURL *base = [[NSFileManager defaultManager] URLsForDirectory:NSApplicationSupportDirectory inDomains:NSUserDomainMask].firstObject;
    if (base == nil) {
      base = [NSURL fileURLWithPath:NSTemporaryDirectory() isDirectory:YES];
    }
    NSString *bundle = NSBundle.mainBundle.bundleIdentifier ?: @"app";
    NSURL *url = [[[[base URLByAppendingPathComponent:bundle isDirectory:YES] URLByAppendingPathComponent:@"undra" isDirectory:YES]
        URLByAppendingPathComponent:nsString(name_space)
                        isDirectory:YES] URLByAppendingPathComponent:name
                                                         isDirectory:YES];
    const char *path = url.fileSystemRepresentation;
    return path != nullptr ? std::string(path) : std::string();
  }
}

std::string osStatus(const char *what, OSStatus status) {
  return std::string(what) + " (OSStatus " + std::to_string(static_cast<int>(status)) + ")";
}

/// The Keychain, one generic-password item per key, in the service `<namespace>.dev.undra.securestore` (the Swift
/// `StorageLocations.keychainService(namespace:)`).
class Keychain final : public SecretStore {
 public:
  explicit Keychain(const std::string &name_space) : service_(nsString(name_space + ".dev.undra.securestore")) {}

  bool get(const std::string &key, std::optional<std::vector<uint8_t>> &value, std::string &error) override {
    @autoreleasepool {
      NSString *account = nsString(key);
      if (account == nil) {
        error = "the key is not UTF-8";
        return false;
      }
      NSMutableDictionary *query = baseQuery(account);
      query[(__bridge id)kSecReturnData] = @YES;
      query[(__bridge id)kSecMatchLimit] = (__bridge id)kSecMatchLimitOne;
      CFTypeRef item = nullptr;
      const OSStatus status = SecItemCopyMatching((__bridge CFDictionaryRef)query, &item);
      if (status == errSecItemNotFound) {
        value.reset();
        return true;
      }
      NSData *data = CFBridgingRelease(item);
      if (status != errSecSuccess || ![data isKindOfClass:[NSData class]]) {
        error = osStatus("Keychain read failed", status);
        return false;
      }
      const auto *bytes = static_cast<const uint8_t *>(data.bytes);
      value.emplace(bytes, bytes + data.length);
      return true;
    }
  }

  bool set(const std::string &key, const std::vector<uint8_t> &value, std::string &error) override {
    @autoreleasepool {
      NSString *account = nsString(key);
      if (account == nil) {
        error = "the key is not UTF-8";
        return false;
      }
      NSData *data = [NSData dataWithBytes:value.data() length:value.size()];
      NSDictionary *update = @{(__bridge id)kSecValueData : data};
      const OSStatus status = SecItemUpdate((__bridge CFDictionaryRef)baseQuery(account), (__bridge CFDictionaryRef)update);
      if (status == errSecSuccess) return true;
      if (status != errSecItemNotFound) {
        error = osStatus("Keychain update failed", status);
        return false;
      }
      NSMutableDictionary *add = baseQuery(account);
      add[(__bridge id)kSecValueData] = data;
      add[(__bridge id)kSecAttrAccessible] = (__bridge id)kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly;
      const OSStatus added = SecItemAdd((__bridge CFDictionaryRef)add, nullptr);
      if (added != errSecSuccess) {
        error = osStatus("Keychain add failed", added);
        return false;
      }
      return true;
    }
  }

  bool remove(const std::string &key, std::string &error) override {
    @autoreleasepool {
      NSString *account = nsString(key);
      if (account == nil) return true;
      const OSStatus status = SecItemDelete((__bridge CFDictionaryRef)baseQuery(account));
      if (status != errSecSuccess && status != errSecItemNotFound) {
        error = osStatus("Keychain delete failed", status);
        return false;
      }
      return true;
    }
  }

  bool list(const std::string &prefix, std::vector<std::string> &keys, std::string &error) override {
    @autoreleasepool {
      keys.clear();
      NSMutableDictionary *query = baseQuery(nil);
      query[(__bridge id)kSecReturnAttributes] = @YES;
      query[(__bridge id)kSecMatchLimit] = (__bridge id)kSecMatchLimitAll;
      CFTypeRef items = nullptr;
      const OSStatus status = SecItemCopyMatching((__bridge CFDictionaryRef)query, &items);
      if (status == errSecItemNotFound) return true;
      NSArray *entries = CFBridgingRelease(items);
      if (status != errSecSuccess || ![entries isKindOfClass:[NSArray class]]) {
        error = osStatus("Keychain list failed", status);
        return false;
      }
      for (NSDictionary *entry in entries) {
        id account = entry[(__bridge id)kSecAttrAccount];
        if (![account isKindOfClass:[NSString class]]) continue;
        // All of its UTF-8 bytes: `UTF8String` would end a key that holds U+0000 at that character.
        NSData *utf8 = [(NSString *)account dataUsingEncoding:NSUTF8StringEncoding];
        if (utf8 == nil) continue;
        std::string name(static_cast<const char *>(utf8.bytes), utf8.length);
        if (name.compare(0, prefix.size(), prefix) == 0) keys.push_back(std::move(name));
      }
      std::sort(keys.begin(), keys.end());
      return true;
    }
  }

  std::string describe() const override {
    return std::string("Keychain service ") + service_.UTF8String;
  }

 private:
  NSMutableDictionary *baseQuery(NSString *account) const {
    NSMutableDictionary *query = [@{
      (__bridge id)kSecClass : (__bridge id)kSecClassGenericPassword,
      (__bridge id)kSecAttrService : service_,
    } mutableCopy];
    if (account != nil) query[(__bridge id)kSecAttrAccount] = account;
    return query;
  }

  NSString *service_;
};

/// `nw_path_monitor` on a queue of its own.
class PathMonitor final : public ConnectivitySource {
 public:
  ~PathMonitor() override {
    stop();
  }

  bool start(Report report) override {
    queue_ = dispatch_queue_create("dev.undra.connectivity", DISPATCH_QUEUE_SERIAL);
    monitor_ = nw_path_monitor_create();
    if (queue_ == nil || monitor_ == nil) {
      monitor_ = nil; // `stop()` then has nothing to cancel, and no queue to wait on
      queue_ = nil;
      return false;
    }
    auto shared = std::make_shared<Report>(std::move(report));
    auto live = live_;
    nw_path_monitor_set_queue(monitor_, queue_);
    nw_path_monitor_set_update_handler(monitor_, ^(nw_path_t path) {
      if (!live->load()) return;
      const bool online = nw_path_get_status(path) == nw_path_status_satisfied;
      NetKind kind = NetKind::None;
      if (online) {
        if (nw_path_uses_interface_type(path, nw_interface_type_wifi)) {
          kind = NetKind::Wifi;
        } else if (nw_path_uses_interface_type(path, nw_interface_type_cellular)) {
          kind = NetKind::Cellular;
        } else if (nw_path_uses_interface_type(path, nw_interface_type_wired)) {
          kind = NetKind::Wired;
        } else {
          kind = NetKind::Unknown;
        }
      }
      (*shared)(online, kind);
    });
    // The monitor delivers the current path as soon as it starts.
    nw_path_monitor_start(monitor_);
    return true;
  }

  void stop() noexcept override {
    if (monitor_ == nil) return;
    live_->store(false);
    nw_path_monitor_cancel(monitor_);
    // A barrier on the monitor's queue: a handler that was running has finished, and every later one
    // sees `live` false, so no report runs after this returns.
    dispatch_sync(queue_, ^{
                  });
    monitor_ = nil;
    queue_ = nil;
  }

 private:
  nw_path_monitor_t monitor_ = nil;
  dispatch_queue_t queue_ = nil;
  std::shared_ptr<std::atomic<bool>> live_ = std::make_shared<std::atomic<bool>>(true);
};

class ApplePlatform final : public Platform {
 public:
  explicit ApplePlatform(std::string name_space) : namespace_(std::move(name_space)) {}

  std::string kvDirectory() override {
    return undraDirectory(namespace_, @"kv");
  }
  KvNaming kvNaming() override {
    return KvNaming::Fnv;
  }
  std::string fsRoot() override {
    return undraDirectory(namespace_, @"fs");
  }
  std::unique_ptr<SecretStore> makeSecretStore() override {
    return std::make_unique<Keychain>(namespace_);
  }
  std::unique_ptr<ConnectivitySource> makeConnectivity() override {
    return std::make_unique<PathMonitor>();
  }
  std::unique_ptr<DbBackend> makeDbBackend() override {
    // Created on the first open.
    const std::string directory = undraDirectory(namespace_, @"db");
    if (directory.empty()) return nullptr;
    return makeSqliteDbBackend(directory);
  }

 private:
  /// The namespace of the core this platform serves: every default store is under it (ADR-044 amendment A).
  std::string namespace_;
};

} // namespace

std::unique_ptr<Platform> makePlatform(const std::string &name_space, std::string & /*error*/) {
  return std::make_unique<ApplePlatform>(name_space);
}

} // namespace undra::rn
