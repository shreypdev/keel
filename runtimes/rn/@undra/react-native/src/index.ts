// `@undra/react-native`: native Undra cores under a React Native app (ADR-038, ADR-044).
//
// The polyfills come first: `@undra/runtime` creates its UTF-8 decoders when its modules load, and
// Hermes may not have `TextDecoder`. Import this package before anything that imports
// `@undra/runtime` (the generated bindings do).
import "./polyfills.js";

export { appStateLifecycle, lifecycleState, reactNativeAdapters } from "./adapters.js";
export { nativeFrameScheduler, type FrameSchedulerOptions } from "./frame.js";
export { installNative, loadNative, type NativeCoreEntry, type NativeLoadOptions } from "./load.js";
export {
  NativeStartCode,
  RecordKind,
  portPlan,
  startFailure,
  type NativeHostCounters,
  type PortPlan,
  type UndraNativeModule,
} from "./native.js";
export { Utf8TextDecoder, Utf8TextEncoder, installPolyfills } from "./text-codec.js";
export { NativeTransport, type NativeTransportOptions } from "./transport.js";
