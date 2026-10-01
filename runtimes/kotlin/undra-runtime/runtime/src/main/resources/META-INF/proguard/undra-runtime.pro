# R8 / ProGuard consumer rules of the Undra Kotlin runtime (dev.undra:runtime), applied to every app that
# depends on it.
#
# Every core's JNI shim (undra-ffi) finds the callbacks interface by name and calls its methods by name and
# descriptor (SPEC 6.1, ADR-044): keep the interface and the methods of whatever implements it. The natives of
# each core live on its generated UndraCoreNative, which its bindings keep (META-INF/proguard/undra-<namespace>.pro).
-keep interface dev.undra.runtime.NativeCallbacks { *; }
-keepclassmembers class * implements dev.undra.runtime.NativeCallbacks {
    public <methods>;
}
