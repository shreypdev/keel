# The core's JNI_OnLoad registers the natives of the bindings' `UndraCoreNative` by name, and the core
# calls the runtime's `NativeCallbacks` by name and descriptor: R8 must not rename or remove them. The
# generated bindings and the runtime ship these rules themselves (META-INF/proguard); they are repeated
# here so the playground does not depend on how its dependencies are packaged.
-keep class dev.undra.playground.core.UndraCoreNative {
    native <methods>;
}
-keep interface dev.undra.runtime.NativeCallbacks { *; }
-keepclassmembers class * implements dev.undra.runtime.NativeCallbacks { *; }
