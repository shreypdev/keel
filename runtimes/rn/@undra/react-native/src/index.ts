// `@undra/react-native`: the native Undra core under a React Native app (ADR-038).
//
// The polyfills come first: `@undra/runtime` creates its UTF-8 decoders when its modules load, and
// Hermes may not have `TextDecoder`. Import this package before anything that imports
// `@undra/runtime` (the generated bindings do).
import "./polyfills.js";

export {
  appStateLifecycle,
  lifecycleState,
  nativeDefaultPorts,
  reactNativeAdapters,
  realtimePorts,
  type ReactNativeAdapters,
} from "./adapters.js";
export {
  reactNativeSse,
  reactNativeWebSocket,
  type ReactNativeSseOptions,
  type ReactNativeWebSocketConstructor,
  type ReactNativeWebSocketOptions,
  type XMLHttpRequestConstructorLike,
  type XMLHttpRequestLike,
} from "./realtime.js";
export { isHttpUrl, reactNativeHttp, type ReactNativeHttpOptions } from "./http.js";
export { nativeFrameScheduler, type FrameSchedulerOptions } from "./frame.js";
export { installNative, loadNative, nativePlatformDefaults, type NativeLoadOptions } from "./load.js";
export {
  NativeStartCode,
  RecordKind,
  portPlan,
  startFailure,
  type NativeHostCounters,
  type NativePlatformDefaults,
  type PortPlan,
  type UndraNativeModule,
} from "./native.js";
export { Utf8TextDecoder, Utf8TextEncoder, installPolyfills } from "./text-codec.js";
export { NativeTransport, type NativeTransportOptions } from "./transport.js";
