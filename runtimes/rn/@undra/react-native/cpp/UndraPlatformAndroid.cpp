// The Android platform of @undra/react-native's default ports (ADR-038 amendment B, B1 and B8).
//
//  * `Kv` and `Fs`: the portable C++ stores over `filesDir/undra/<namespace>/kv` (SHA-256 names) and
//    `filesDir/undra/<namespace>/fs`, the directories and names of `android-adapters` for the core's namespace
//    (ADR-044 amendment A: every default store is per core namespace);
//  * `SecureStore`: the same C++ store over `noBackupFilesDir/undra/<namespace>/secure`, each value sealed and
//    opened by Java (`UndraPlatform.seal`/`open`, the Android Keystore key `<namespace>.dev.undra.securestore`),
//    `android-adapters`' layout;
//  * `Connectivity`: Java's `NetworkMonitor`, reporting into `nativeConnectivityChanged`;
//  * `Db` (ADR-048): the binding of `UndraDb.cpp` over Android's own SQLite, through Java's `UndraDatabase`
//    (`android.database.sqlite`, the file `getDatabasePath("undra-<namespace>-<name>.sqlite")` of `android-adapters`): one
//    byte array of the wire format crosses per statement each way (`DbWire.java`). The NDK has no public
//    sqlite3, and Android prepares only the first statement of a string and exposes no parameter count, so
//    this side tells Java both from the SQL lexer of `UndraDb.cpp` (checked against sqlite3 on the host).
//
// JNI only, and robust to how the module is linked: the Java class is loaded through the JS thread's
// context class loader in `makePlatform` (which `install()` calls on the JS thread), its methods are
// looked up once, and the native callback is bound with `RegisterNatives`, so nothing depends on an
// exported `Java_` symbol (a static library inside libappmodules.so would drop one). The worker
// threads attach themselves to the VM once (`workerStarted`) and detach when they end.
#if defined(__ANDROID__)

#include <android/log.h>
#include <fbjni/fbjni.h>
#include <jni.h>

#include <atomic>
#include <cstdint>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include "UndraDb.h"
#include "UndraDefaults.h"

namespace undra::rn {

namespace {

constexpr char kTag[] = "Undra";
constexpr char kPlatformClass[] = "dev.undra.reactnative.UndraPlatform";

/// The Java side, resolved once per process.
struct JavaSide {
  JavaVM *vm = nullptr;
  jclass platform = nullptr; // global reference
  jmethodID directories = nullptr;
  jmethodID seal = nullptr;
  jmethodID open = nullptr;
  jmethodID startConnectivity = nullptr;
  jclass monitor = nullptr; // global reference
  jmethodID stop = nullptr;
  jclass database = nullptr; // global reference; null when the Java half has no `UndraDatabase`
  jmethodID dbDirectory = nullptr;
  jmethodID dbOpen = nullptr;
  jmethodID dbFailure = nullptr;
  jmethodID dbExecute = nullptr;
  jmethodID dbQuery = nullptr;
  jmethodID dbClose = nullptr;
};

std::mutex g_javaMutex;
JavaSide g_java;
bool g_javaReady = false;

/// The `JNIEnv` of this thread, or null when it is not attached.
JNIEnv *envOf(JavaVM *vm) {
  JNIEnv *env = nullptr;
  if (vm == nullptr || vm->GetEnv(reinterpret_cast<void **>(&env), JNI_VERSION_1_6) != JNI_OK) {
    return nullptr;
  }
  return env;
}

/// Clears a pending Java exception and returns its `toString()`, or "" when there is none.
std::string takeException(JNIEnv *env) {
  if (!env->ExceptionCheck()) return {};
  jthrowable thrown = env->ExceptionOccurred();
  env->ExceptionClear();
  std::string text = "a Java exception";
  if (thrown != nullptr) {
    jclass cls = env->FindClass("java/lang/Throwable");
    jmethodID toString = cls != nullptr ? env->GetMethodID(cls, "toString", "()Ljava/lang/String;") : nullptr;
    if (toString != nullptr) {
      auto described = static_cast<jstring>(env->CallObjectMethod(thrown, toString));
      if (!env->ExceptionCheck() && described != nullptr) {
        const char *chars = env->GetStringUTFChars(described, nullptr);
        if (chars != nullptr) {
          text = chars;
          env->ReleaseStringUTFChars(described, chars);
        }
        env->DeleteLocalRef(described);
      }
      env->ExceptionClear();
    }
    if (cls != nullptr) env->DeleteLocalRef(cls);
    env->DeleteLocalRef(thrown);
  }
  return text;
}

/// A Java `String` of UTF-8 `text`. `NewStringUTF` takes modified UTF-8, which differs for NUL and
/// characters beyond the BMP, so the string is built from UTF-16 instead.
jstring javaString(JNIEnv *env, const std::string &text) {
  std::u16string units;
  units.reserve(text.size());
  for (std::size_t i = 0; i < text.size();) {
    const auto c = static_cast<unsigned char>(text[i]);
    uint32_t cp = 0;
    std::size_t len = 1;
    if (c < 0x80) {
      cp = c;
    } else if ((c & 0xe0) == 0xc0) {
      cp = c & 0x1f;
      len = 2;
    } else if ((c & 0xf0) == 0xe0) {
      cp = c & 0x0f;
      len = 3;
    } else {
      cp = c & 0x07;
      len = 4;
    }
    for (std::size_t k = 1; k < len && i + k < text.size(); ++k) cp = (cp << 6) | (static_cast<unsigned char>(text[i + k]) & 0x3f);
    i += len;
    if (cp >= 0x10000) {
      cp -= 0x10000;
      units.push_back(static_cast<char16_t>(0xd800 + (cp >> 10)));
      units.push_back(static_cast<char16_t>(0xdc00 + (cp & 0x3ff)));
    } else {
      units.push_back(static_cast<char16_t>(cp));
    }
  }
  return env->NewString(reinterpret_cast<const jchar *>(units.data()), static_cast<jsize>(units.size()));
}

std::string stdString(JNIEnv *env, jstring value) {
  if (value == nullptr) return {};
  const char *chars = env->GetStringUTFChars(value, nullptr);
  std::string out = chars != nullptr ? chars : "";
  if (chars != nullptr) env->ReleaseStringUTFChars(value, chars);
  return out;
}

/// `nativeConnectivityChanged(long handle, boolean online, int kind)`: a report of the network monitor,
/// on its handler thread. `handle` is the `Report` the source owns until `stop()` has returned.
void JNICALL nativeConnectivityChanged(JNIEnv * /*env*/, jclass /*cls*/, jlong handle, jboolean online, jint kind) {
  auto *report = reinterpret_cast<ConnectivitySource::Report *>(static_cast<intptr_t>(handle));
  if (report == nullptr || !*report || kind < 0 || kind > 4) return;
  try {
    (*report)(online == JNI_TRUE, static_cast<NetKind>(kind));
  } catch (...) {
    // Nothing may unwind into Java.
  }
}

/// Loads `dev.undra.reactnative.UndraPlatform` on the JS thread: through the thread's context class
/// loader (the app's), falling back to `FindClass`.
jclass loadPlatformClass(JNIEnv *env, std::string &error) {
  jclass found = nullptr;
  jclass threadClass = env->FindClass("java/lang/Thread");
  jclass loaderClass = env->FindClass("java/lang/ClassLoader");
  if (threadClass != nullptr && loaderClass != nullptr) {
    jmethodID current = env->GetStaticMethodID(threadClass, "currentThread", "()Ljava/lang/Thread;");
    jmethodID contextLoader = env->GetMethodID(threadClass, "getContextClassLoader", "()Ljava/lang/ClassLoader;");
    jmethodID loadClass = env->GetMethodID(loaderClass, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;");
    jobject thread = current != nullptr ? env->CallStaticObjectMethod(threadClass, current) : nullptr;
    jobject loader = thread != nullptr && contextLoader != nullptr ? env->CallObjectMethod(thread, contextLoader) : nullptr;
    if (loader != nullptr && loadClass != nullptr) {
      jstring name = env->NewStringUTF(kPlatformClass);
      found = static_cast<jclass>(env->CallObjectMethod(loader, loadClass, name));
      env->DeleteLocalRef(name);
    }
    takeException(env);
    if (loader != nullptr) env->DeleteLocalRef(loader);
    if (thread != nullptr) env->DeleteLocalRef(thread);
  }
  takeException(env);
  if (threadClass != nullptr) env->DeleteLocalRef(threadClass);
  if (loaderClass != nullptr) env->DeleteLocalRef(loaderClass);
  if (found == nullptr) {
    found = env->FindClass("dev/undra/reactnative/UndraPlatform");
    const std::string why = takeException(env);
    if (found == nullptr) {
      error = "the Android library of @undra/react-native is not in the app (" + why +
          "): run the Gradle sync after adding the package (docs/REACT_NATIVE.md)";
    }
  }
  return found;
}

/// Resolves the Java side once (on the JS thread). `false` with `error` set when it is missing.
bool resolveJava(std::string &error) {
  std::lock_guard<std::mutex> lock(g_javaMutex);
  if (g_javaReady) return true;
  JNIEnv *env = facebook::jni::Environment::current();
  if (env == nullptr) {
    error = "no JNI environment on this thread";
    return false;
  }
  JavaSide side;
  if (env->GetJavaVM(&side.vm) != JNI_OK) {
    error = "no Java VM";
    return false;
  }
  jclass local = loadPlatformClass(env, error);
  if (local == nullptr) return false;
  side.platform = static_cast<jclass>(env->NewGlobalRef(local));
  env->DeleteLocalRef(local);
  // Each lookup clears what the one before it threw (NoSuchMethodError when the Java and C++ halves
  // differ): no JNI call but a handful may run with an exception pending, and CheckJNI aborts.
  std::string why;
  auto lookup = [&](const char *name, const char *signature) -> jmethodID {
    jmethodID id = env->GetStaticMethodID(side.platform, name, signature);
    const std::string thrown = takeException(env);
    if (id == nullptr && why.empty()) why = thrown.empty() ? std::string("no ") + name : thrown;
    return id;
  };
  side.directories = lookup("directories", "()[Ljava/lang/String;");
  side.seal = lookup("seal", "(Ljava/lang/String;Ljava/lang/String;[B)[B");
  side.open = lookup("open", "(Ljava/lang/String;Ljava/lang/String;[B)[B");
  side.startConnectivity = lookup("startConnectivity", "(J)Ldev/undra/reactnative/NetworkMonitor;");
  if (side.directories == nullptr || side.seal == nullptr || side.open == nullptr || side.startConnectivity == nullptr) {
    error = "dev.undra.reactnative.UndraPlatform does not have the methods this module calls (" + why + "): the Java and C++ halves of the package differ";
    env->DeleteGlobalRef(side.platform);
    return false;
  }
  // The monitor class through the platform class's own loader, so it is the app's.
  jmethodID getLoader = nullptr;
  {
    jclass classClass = env->FindClass("java/lang/Class");
    getLoader = classClass != nullptr ? env->GetMethodID(classClass, "getClassLoader", "()Ljava/lang/ClassLoader;") : nullptr;
    jobject loader = getLoader != nullptr ? env->CallObjectMethod(side.platform, getLoader) : nullptr;
    jclass loaderClass = env->FindClass("java/lang/ClassLoader");
    jmethodID loadClass = loaderClass != nullptr ? env->GetMethodID(loaderClass, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;") : nullptr;
    if (loader != nullptr && loadClass != nullptr) {
      jstring name = env->NewStringUTF("dev.undra.reactnative.NetworkMonitor");
      auto monitor = static_cast<jclass>(env->CallObjectMethod(loader, loadClass, name));
      env->DeleteLocalRef(name);
      if (monitor != nullptr && !env->ExceptionCheck()) {
        side.monitor = static_cast<jclass>(env->NewGlobalRef(monitor));
        side.stop = env->GetMethodID(side.monitor, "stop", "()V");
        env->DeleteLocalRef(monitor);
      }
    }
    why = takeException(env);
    if (loader != nullptr) env->DeleteLocalRef(loader);
    if (loaderClass != nullptr) env->DeleteLocalRef(loaderClass);
    if (classClass != nullptr) env->DeleteLocalRef(classClass);
  }
  // The Db port's SQLite, through the same loader; without it `Db` is simply not native (an older Java half).
  {
    jclass classClass = env->FindClass("java/lang/Class");
    jmethodID loaderOf = classClass != nullptr ? env->GetMethodID(classClass, "getClassLoader", "()Ljava/lang/ClassLoader;") : nullptr;
    jobject loader = loaderOf != nullptr ? env->CallObjectMethod(side.platform, loaderOf) : nullptr;
    jclass loaderClass = env->FindClass("java/lang/ClassLoader");
    jmethodID loadClass = loaderClass != nullptr ? env->GetMethodID(loaderClass, "loadClass", "(Ljava/lang/String;)Ljava/lang/Class;") : nullptr;
    jclass database = nullptr;
    if (loader != nullptr && loadClass != nullptr && !env->ExceptionCheck()) {
      jstring name = env->NewStringUTF("dev.undra.reactnative.UndraDatabase");
      database = static_cast<jclass>(env->CallObjectMethod(loader, loadClass, name));
      env->DeleteLocalRef(name);
    }
    const std::string missing = takeException(env);
    if (database != nullptr) {
      auto method = [&](const char *name, const char *signature, bool isStatic) -> jmethodID {
        jmethodID id = isStatic ? env->GetStaticMethodID(database, name, signature) : env->GetMethodID(database, name, signature);
        takeException(env);
        return id;
      };
      side.dbDirectory = method("directory", "()Ljava/lang/String;", true);
      side.dbOpen = method("open", "(Ljava/lang/String;Ljava/lang/String;)Ldev/undra/reactnative/UndraDatabase;", true);
      side.dbFailure = method("failure", "()[B", false);
      side.dbExecute = method("execute", "(Ljava/lang/String;[BIZ)[B", false);
      side.dbQuery = method("query", "(Ljava/lang/String;[BIZ)[B", false);
      side.dbClose = method("close", "()V", false);
      if (side.dbDirectory != nullptr && side.dbOpen != nullptr && side.dbFailure != nullptr && side.dbExecute != nullptr &&
          side.dbQuery != nullptr && side.dbClose != nullptr) {
        side.database = static_cast<jclass>(env->NewGlobalRef(database));
      } else {
        __android_log_print(ANDROID_LOG_WARN, kTag, "dev.undra.reactnative.UndraDatabase lacks a method this module calls: Db is not native");
      }
      env->DeleteLocalRef(database);
    } else {
      __android_log_print(ANDROID_LOG_WARN, kTag, "no dev.undra.reactnative.UndraDatabase (%s): Db is not native", missing.c_str());
    }
    if (loader != nullptr) env->DeleteLocalRef(loader);
    if (loaderClass != nullptr) env->DeleteLocalRef(loaderClass);
    if (classClass != nullptr) env->DeleteLocalRef(classClass);
  }
  const JNINativeMethod natives[] = {
      {const_cast<char *>("nativeConnectivityChanged"), const_cast<char *>("(JZI)V"), reinterpret_cast<void *>(&nativeConnectivityChanged)},
  };
  if (env->RegisterNatives(side.platform, natives, 1) != JNI_OK) {
    takeException(env);
    __android_log_print(ANDROID_LOG_WARN, kTag, "could not bind UndraPlatform.nativeConnectivityChanged: Connectivity is not reported");
    side.stop = nullptr;
  }
  g_java = side;
  g_javaReady = true;
  return true;
}

/// The `SecureStore` of Android: sealed values in the portable C++ store.
class SealedStore final : public SecretStore {
 public:
  /// `name_space` is the core's: the Keystore key is `<namespace>.dev.undra.securestore`, and Java gets it with every call.
  SealedStore(JavaSide java, std::string name_space, std::string directory)
      : java_(java), namespace_(std::move(name_space)), files_(std::move(directory), KvNaming::Sha256) {}

  bool get(const std::string &key, std::optional<std::vector<uint8_t>> &value, std::string &error) override {
    std::optional<std::vector<uint8_t>> sealed;
    if (!files_.get(key, sealed, error)) return false;
    if (!sealed) {
      value.reset();
      return true;
    }
    std::vector<uint8_t> plain;
    if (!call(java_.open, key, *sealed, plain, error)) return false;
    value = std::move(plain);
    return true;
  }

  bool set(const std::string &key, const std::vector<uint8_t> &value, std::string &error) override {
    std::vector<uint8_t> sealed;
    if (!call(java_.seal, key, value, sealed, error)) return false;
    return files_.set(key, sealed.data(), sealed.size(), error);
  }

  bool remove(const std::string &key, std::string &error) override {
    return files_.remove(key, error);
  }

  bool list(const std::string &prefix, std::vector<std::string> &keys, std::string &error) override {
    return files_.list(prefix, keys, error);
  }

  std::string describe() const override {
    return "Android Keystore key " + namespace_ + ".dev.undra.securestore, sealed files in " + files_.directory();
  }

 private:
  /// `UndraPlatform.seal` or `open` on this (attached) worker thread.
  bool call(jmethodID method, const std::string &key, const std::vector<uint8_t> &in, std::vector<uint8_t> &out, std::string &error) {
    JNIEnv *env = envOf(java_.vm);
    if (env == nullptr) {
      error = "the SecureStore thread is not attached to the Java VM";
      return false;
    }
    if (env->PushLocalFrame(8) != JNI_OK) {
      error = "out of JNI local references: " + takeException(env);
      return false;
    }
    jstring jnamespace = javaString(env, namespace_);
    jstring jkey = jnamespace != nullptr ? javaString(env, key) : nullptr;
    jbyteArray jin = jkey != nullptr ? env->NewByteArray(static_cast<jsize>(in.size())) : nullptr;
    if (jin != nullptr && !in.empty()) env->SetByteArrayRegion(jin, 0, static_cast<jsize>(in.size()), reinterpret_cast<const jbyte *>(in.data()));
    auto result = jin != nullptr ? static_cast<jbyteArray>(env->CallStaticObjectMethod(java_.platform, method, jnamespace, jkey, jin)) : nullptr;
    const std::string thrown = takeException(env);
    bool ok = false;
    if (!thrown.empty()) {
      error = "the Android Keystore failed for '" + key + "': " + thrown;
    } else if (result == nullptr) {
      error = "the Android Keystore answered nothing for '" + key + "'";
    } else {
      const jsize n = env->GetArrayLength(result);
      out.resize(static_cast<std::size_t>(n));
      if (n > 0) env->GetByteArrayRegion(result, 0, n, reinterpret_cast<jbyte *>(out.data()));
      ok = true;
    }
    env->PopLocalFrame(nullptr);
    return ok;
  }

  JavaSide java_;
  std::string namespace_;
  KvStore files_;
};

/// The `Connectivity` source of Android: Java's `NetworkMonitor`.
class JavaNetworkSource final : public ConnectivitySource {
 public:
  explicit JavaNetworkSource(JavaSide java) : java_(java) {}
  ~JavaNetworkSource() override {
    stop();
  }

  bool start(Report report) override {
    if (java_.stop == nullptr) return false;
    // Attached for the call: `start` runs on the JS thread, which is a Java thread already.
    JNIEnv *env = envOf(java_.vm);
    if (env == nullptr) return false;
    report_ = std::make_unique<Report>(std::move(report));
    const auto handle = static_cast<jlong>(reinterpret_cast<intptr_t>(report_.get()));
    jobject monitor = env->CallStaticObjectMethod(java_.platform, java_.startConnectivity, handle);
    const std::string thrown = takeException(env);
    if (monitor == nullptr) {
      if (!thrown.empty()) __android_log_print(ANDROID_LOG_WARN, kTag, "the Connectivity source did not start: %s", thrown.c_str());
      report_.reset();
      return false;
    }
    monitor_ = env->NewGlobalRef(monitor);
    env->DeleteLocalRef(monitor);
    return true;
  }

  void stop() noexcept override {
    if (monitor_ == nullptr) return;
    JNIEnv *env = envOf(java_.vm);
    bool attached = false;
    if (env == nullptr && java_.vm != nullptr && java_.vm->AttachCurrentThread(&env, nullptr) == JNI_OK) {
      attached = true;
    }
    if (env != nullptr) {
      // Returns when the handler thread has ended: no report runs after this, so `report_` can go.
      env->CallVoidMethod(monitor_, java_.stop);
      takeException(env);
      env->DeleteGlobalRef(monitor_);
    }
    monitor_ = nullptr;
    if (attached) java_.vm->DetachCurrentThread();
    report_.reset();
  }

 private:
  JavaSide java_;
  jobject monitor_ = nullptr;
  std::unique_ptr<Report> report_;
};

/// Reads what `UndraDatabase` answered (`DbWire.java`): `0, body` (into `body`) or `1, class, message`.
std::optional<DbFailure> readDbResult(JNIEnv *env, jbyteArray result, std::vector<uint8_t> &body) {
  if (result == nullptr) return DbFailure::sql("the Android SQLite answered nothing");
  const jsize n = env->GetArrayLength(result);
  std::vector<uint8_t> bytes(static_cast<std::size_t>(n));
  if (n > 0) env->GetByteArrayRegion(result, 0, n, reinterpret_cast<jbyte *>(bytes.data()));
  if (bytes.empty()) return DbFailure::sql("the Android SQLite answered nothing");
  if (bytes[0] == 0) {
    body.assign(bytes.begin() + 1, bytes.end());
    return std::nullopt;
  }
  WireReader r(bytes.data() + 1, bytes.size() - 1);
  const std::string className = r.str();
  const std::string message = r.str();
  if (!r.finish()) return DbFailure::sql("the Android SQLite answered a malformed failure");
  return androidDbFailure(className, message);
}

/// One connection: a global reference to an `UndraDatabase`, used from its database's (attached) thread.
class JniDbConnection final : public DbConnection {
 public:
  JniDbConnection(JavaSide java, jobject db) : java_(java), db_(db) {}
  ~JniDbConnection() override {
    close();
  }

  std::optional<DbFailure> execute(const std::string &sql, const std::vector<DbValue> &params, DbExecuted &out) override {
    std::vector<uint8_t> body;
    if (auto failure = statement(java_.dbExecute, sql, params, body)) return failure;
    WireReader r(body.data(), body.size());
    out.changes = r.u64();
    out.lastInsertId = static_cast<int64_t>(r.u64());
    if (!r.finish()) return DbFailure::sql("the Android SQLite answered a malformed result");
    return std::nullopt;
  }

  std::optional<DbFailure> query(const std::string &sql, const std::vector<DbValue> &params, DbRows &out) override {
    std::vector<uint8_t> body;
    if (auto failure = statement(java_.dbQuery, sql, params, body)) return failure;
    WireReader r(body.data(), body.size());
    out.columns.clear();
    out.rows.clear();
    const uint32_t columns = r.u32();
    for (uint32_t i = 0; i < columns && r.ok(); ++i) out.columns.push_back(r.str());
    const uint32_t rows = r.u32();
    for (uint32_t i = 0; i < rows && r.ok(); ++i) {
      const uint32_t cells = r.u32();
      std::vector<DbValue> row;
      row.reserve(cells <= columns ? cells : columns);
      for (uint32_t c = 0; c < cells && r.ok(); ++c) row.push_back(readDbValue(r));
      out.rows.push_back(std::move(row));
    }
    if (!r.finish()) return DbFailure::sql("the Android SQLite answered malformed rows");
    return std::nullopt;
  }

  std::optional<DbFailure> executeScript(const std::string &sql) override {
    // Android runs one statement per call: the script is split here (SQLite's own rules, `splitSql`).
    for (const SqlStatement &one : splitSql(sql)) {
      std::vector<uint8_t> body;
      if (auto failure = call(java_.dbExecute, sql.substr(one.begin, one.end - one.begin), {}, -1, false, body)) return failure;
    }
    return std::nullopt;
  }

  void close() noexcept override {
    if (db_ == nullptr) return;
    if (JNIEnv *env = envOf(java_.vm)) {
      env->CallVoidMethod(db_, java_.dbClose);
      takeException(env);
      env->DeleteGlobalRef(db_);
    }
    db_ = nullptr;
  }

 private:
  /// One statement of `execute`/`query`: Java gets the first statement, whether SQL follows it, and its
  /// parameter count, and refuses either after Android compiled it.
  std::optional<DbFailure> statement(jmethodID method, const std::string &sql, const std::vector<DbValue> &params, std::vector<uint8_t> &body) {
    const SqlShape shape = sqlShapeOf(sql);
    if (shape.first.begin == shape.first.end) return DbFailure::sql(db_text::kNoStatement);
    return call(method, sql.substr(shape.first.begin, shape.first.end - shape.first.begin), params, shape.parameters, shape.trailing, body);
  }

  std::optional<DbFailure> call(jmethodID method, const std::string &sql, const std::vector<DbValue> &params, int expected, bool trailing,
      std::vector<uint8_t> &body) {
    JNIEnv *env = envOf(java_.vm);
    if (env == nullptr || db_ == nullptr) return DbFailure::unavailable("the database thread is not attached to the Java VM");
    if (env->PushLocalFrame(8) != JNI_OK) return DbFailure::unavailable("out of JNI local references: " + takeException(env));
    WireWriter args;
    args.u32(static_cast<uint32_t>(params.size()));
    for (const DbValue &value : params) writeDbValue(args, value);
    std::optional<DbFailure> failure;
    jstring jsql = javaString(env, sql);
    jbyteArray jparams = jsql != nullptr ? env->NewByteArray(static_cast<jsize>(args.out.size())) : nullptr;
    if (jparams != nullptr) {
      env->SetByteArrayRegion(jparams, 0, static_cast<jsize>(args.out.size()), reinterpret_cast<const jbyte *>(args.out.data()));
      auto result = static_cast<jbyteArray>(env->CallObjectMethod(db_, method, jsql, jparams, static_cast<jint>(expected), trailing ? JNI_TRUE : JNI_FALSE));
      const std::string thrown = takeException(env);
      failure = thrown.empty() ? readDbResult(env, result, body) : DbFailure::sql("the Android SQLite threw " + thrown);
    } else {
      failure = DbFailure::unavailable("out of Java memory: " + takeException(env));
    }
    env->PopLocalFrame(nullptr);
    return failure;
  }

  JavaSide java_;
  jobject db_;
};

/// The `Db` backend of Android: `UndraDatabase.open` on the database's thread.
class JniDbBackend final : public DbBackend {
 public:
  JniDbBackend(JavaSide java, std::string name_space, std::string directory)
      : java_(java), namespace_(std::move(name_space)), directory_(std::move(directory)) {}

  std::unique_ptr<DbConnection> open(const std::string &name, DbFailure &failure) override {
    JNIEnv *env = envOf(java_.vm);
    if (env == nullptr) {
      failure = DbFailure::unavailable("the database thread is not attached to the Java VM");
      return nullptr;
    }
    if (env->PushLocalFrame(8) != JNI_OK) {
      failure = DbFailure::unavailable("out of JNI local references: " + takeException(env));
      return nullptr;
    }
    std::unique_ptr<DbConnection> connection;
    jstring jnamespace = javaString(env, namespace_);
    jstring jname = jnamespace != nullptr ? javaString(env, name) : nullptr;
    jobject db = jname != nullptr ? env->CallStaticObjectMethod(java_.database, java_.dbOpen, jnamespace, jname) : nullptr;
    std::string thrown = takeException(env);
    if (db == nullptr) {
      failure = DbFailure::unavailable("the Android SQLite could not open " + name + (thrown.empty() ? "" : ": " + thrown));
    } else {
      auto why = static_cast<jbyteArray>(env->CallObjectMethod(db, java_.dbFailure));
      thrown = takeException(env);
      if (!thrown.empty()) {
        failure = DbFailure::unavailable("the Android SQLite could not open " + name + ": " + thrown);
      } else if (why != nullptr) {
        std::vector<uint8_t> ignored;
        failure = readDbResult(env, why, ignored).value_or(DbFailure::unavailable("the Android SQLite could not open " + name));
      } else if (jobject global = env->NewGlobalRef(db)) {
        connection = std::make_unique<JniDbConnection>(java_, global);
      } else {
        failure = DbFailure::unavailable("out of JNI global references");
      }
    }
    env->PopLocalFrame(nullptr);
    return connection;
  }

  std::string describe() const override {
    return directory_ + "/undra-" + namespace_ + "-<name>.sqlite";
  }

 private:
  JavaSide java_;
  std::string namespace_;
  std::string directory_;
};

class AndroidPlatform final : public Platform {
 public:
  AndroidPlatform(JavaSide java, std::string name_space, std::string files, std::string noBackup, std::string databases)
      : java_(java),
        namespace_(std::move(name_space)),
        files_(std::move(files)),
        noBackup_(std::move(noBackup)),
        databases_(std::move(databases)) {}

  std::string kvDirectory() override {
    return files_ + "/undra/" + namespace_ + "/kv";
  }
  KvNaming kvNaming() override {
    return KvNaming::Sha256;
  }
  std::string fsRoot() override {
    return files_ + "/undra/" + namespace_ + "/fs";
  }
  std::unique_ptr<SecretStore> makeSecretStore() override {
    return std::make_unique<SealedStore>(java_, namespace_, noBackup_ + "/undra/" + namespace_ + "/secure");
  }
  std::unique_ptr<ConnectivitySource> makeConnectivity() override {
    if (java_.stop == nullptr) return nullptr;
    return std::make_unique<JavaNetworkSource>(java_);
  }
  std::unique_ptr<DbBackend> makeDbBackend() override {
    if (java_.database == nullptr || databases_.empty()) return nullptr;
    return std::make_unique<JniDbBackend>(java_, namespace_, databases_);
  }
  void workerStarted(const char *name) noexcept override {
    JNIEnv *env = nullptr;
    // The worker's own name (`undra-kv`, ...): attaching renames the thread to the name given here.
    JavaVMAttachArgs args{JNI_VERSION_1_6, name, nullptr};
    attached_ = java_.vm != nullptr && java_.vm->GetEnv(reinterpret_cast<void **>(&env), JNI_VERSION_1_6) == JNI_EDETACHED &&
        java_.vm->AttachCurrentThread(&env, &args) == JNI_OK;
  }
  void workerEnded() noexcept override {
    if (attached_ && java_.vm != nullptr) java_.vm->DetachCurrentThread();
  }

 private:
  JavaSide java_;
  /// The namespace of the core this platform serves: every default store is under it (ADR-044 amendment A).
  std::string namespace_;
  std::string files_;
  std::string noBackup_;
  std::string databases_;
  // Per worker thread: whether this thread attached itself.
  static thread_local bool attached_;
};

thread_local bool AndroidPlatform::attached_ = false;

} // namespace

std::unique_ptr<Platform> makePlatform(const std::string &name_space, std::string &error) {
  if (!validStorageNamespace(name_space)) {
    error = invalidStorageNamespace(name_space);
    return nullptr;
  }
  try {
    if (!resolveJava(error)) return nullptr;
    JNIEnv *env = facebook::jni::Environment::current();
    JavaSide java;
    {
      std::lock_guard<std::mutex> lock(g_javaMutex);
      java = g_java;
    }
    auto dirs = static_cast<jobjectArray>(env->CallStaticObjectMethod(java.platform, java.directories));
    const std::string thrown = takeException(env);
    if (dirs == nullptr || env->GetArrayLength(dirs) < 2) {
      error = thrown.empty() ? "UndraPlatform has no context: keep its provider in the manifest, or call UndraPlatform.install(context) in Application.onCreate"
                             : "UndraPlatform.directories() failed: " + thrown;
      if (dirs != nullptr) env->DeleteLocalRef(dirs);
      return nullptr;
    }
    auto files = static_cast<jstring>(env->GetObjectArrayElement(dirs, 0));
    auto noBackup = static_cast<jstring>(env->GetObjectArrayElement(dirs, 1));
    std::string filesDir = stdString(env, files);
    std::string noBackupDir = stdString(env, noBackup);
    env->DeleteLocalRef(files);
    env->DeleteLocalRef(noBackup);
    env->DeleteLocalRef(dirs);
    if (filesDir.empty() || noBackupDir.empty()) {
      error = "the app's directories are unknown";
      return nullptr;
    }
    std::string databases;
    if (java.database != nullptr) {
      auto directory = static_cast<jstring>(env->CallStaticObjectMethod(java.database, java.dbDirectory));
      if (takeException(env).empty() && directory != nullptr) databases = stdString(env, directory);
      if (directory != nullptr) env->DeleteLocalRef(directory);
    }
    return std::make_unique<AndroidPlatform>(java, name_space, std::move(filesDir), std::move(noBackupDir), std::move(databases));
  } catch (const std::exception &e) {
    error = e.what();
    return nullptr;
  } catch (...) {
    error = "the Android platform could not be reached";
    return nullptr;
  }
}

} // namespace undra::rn

#endif
