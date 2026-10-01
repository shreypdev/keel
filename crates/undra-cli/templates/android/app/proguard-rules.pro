# The core's JNI_OnLoad registers the natives of the bindings' `UndraCoreNative` by name, and the core
# calls the runtime's `NativeCallbacks` by name and descriptor: R8 must not rename or remove them. The
# generated bindings and the runtime ship these rules themselves (META-INF/proguard); they are repeated
# here so an app that repackages its dependencies keeps them.
-keep class @@KOTLIN_PACKAGE@@.UndraCoreNative {
    native <methods>;
}
-keep interface dev.undra.runtime.NativeCallbacks { *; }
-keepclassmembers class * implements dev.undra.runtime.NativeCallbacks { *; }
