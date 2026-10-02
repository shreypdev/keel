# @undra/react-native: C++ (cpp/UndraPlatformAndroid.cpp) looks these up by name over JNI (UndraDatabase: the Db
# port's SQLite, ADR-048) and binds
# UndraPlatform.nativeConnectivityChanged with RegisterNatives, so R8 must keep them as they are.
-keep class dev.undra.reactnative.UndraPlatform { *; }
-keep class dev.undra.reactnative.NetworkMonitor { *; }
-keep class dev.undra.reactnative.UndraDatabase { *; }
