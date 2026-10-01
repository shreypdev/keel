# The C++ of @undra/react-native for Android (ADR-038, decision 1).
#
# React Native's autolinking adds this directory to the app's CMake build (react-native.config.js:
# a pure C++ dependency) after `find_package(ReactAndroid)`, and links the static library below into
# the app's `libappmodules.so`. No Undra core is linked here: the module `dlopen`s the APK's
# `lib<namespace>.so` (`undra build --platform rn`, or android) the first time JavaScript installs
# that core (`install(namespace)`, ADR-044), so one build of the module serves every core of the app.
# The default ports' Java half is the Gradle library next to this file (ADR-038 amendment B, B8).
cmake_minimum_required(VERSION 3.13)
project(undra_react_native CXX)

set(UNDRA_RN_CPP_DIR ${CMAKE_CURRENT_LIST_DIR}/../cpp)

add_library(undra_react_native STATIC
  ${UNDRA_RN_CPP_DIR}/UndraApi.cpp
  ${UNDRA_RN_CPP_DIR}/UndraApiAndroid.cpp
  ${UNDRA_RN_CPP_DIR}/UndraDefaults.cpp
  ${UNDRA_RN_CPP_DIR}/UndraFrameSource.cpp
  ${UNDRA_RN_CPP_DIR}/UndraHost.cpp
  ${UNDRA_RN_CPP_DIR}/UndraJsi.cpp
  ${UNDRA_RN_CPP_DIR}/UndraPlatformAndroid.cpp
  ${UNDRA_RN_CPP_DIR}/UndraStores.cpp
  ${UNDRA_RN_CPP_DIR}/UndraTurboModule.cpp
)

target_include_directories(undra_react_native PUBLIC ${UNDRA_RN_CPP_DIR})
target_compile_features(undra_react_native PRIVATE cxx_std_20)
target_compile_options(undra_react_native PRIVATE -fexceptions -frtti -Wall -Wextra)
# fbjni: the JNI environment of the JS thread, where the default ports' Java half is resolved (the
# app's CMake has found the package before it adds this directory).
if(NOT TARGET fbjni::fbjni)
  find_package(fbjni REQUIRED CONFIG)
endif()

target_link_libraries(undra_react_native
  ReactAndroid::jsi
  ReactAndroid::reactnative
  fbjni::fbjni
  android # AChoreographer
  dl      # dlopen of lib<namespace>.so
  log     # __android_log_print
)
