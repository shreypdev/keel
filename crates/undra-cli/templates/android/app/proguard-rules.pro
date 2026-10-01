# The native library finds these by name and descriptor (RegisterNatives in JNI_OnLoad): R8 must
# not rename or remove them. (The android-adapters module will ship these as consumer rules.)
-keep class dev.undra.runtime.UndraNative { *; }
-keep class dev.undra.runtime.UndraNative$Callbacks { *; }
-keepclassmembers class * implements dev.undra.runtime.UndraNative$Callbacks { *; }
