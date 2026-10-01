package dev.undra.reactnative;

import com.facebook.react.BaseReactPackage;
import com.facebook.react.bridge.NativeModule;
import com.facebook.react.bridge.ReactApplicationContext;
import com.facebook.react.module.model.ReactModuleInfoProvider;
import java.util.Collections;

/**
 * Exists because React Native's autolinking links a library with a Gradle project only when it has a
 * {@code ReactPackage}. It provides no Java module: {@code UndraNative} is the package's C++ TurboModule, which
 * autolinking registers from {@code react-native.config.cjs} (ADR-038, decision 1; amendment B, B8).
 */
public final class UndraReactNativePackage extends BaseReactPackage {
    @Override
    public NativeModule getModule(String name, ReactApplicationContext reactContext) {
        return null;
    }

    @Override
    public ReactModuleInfoProvider getReactModuleInfoProvider() {
        return Collections::emptyMap;
    }
}
