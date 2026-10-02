/*
 * `@undra/runtime/realtime`: the opt-in `WebSocket` and `Sse` ports of ADR-047, kept out of the
 * main entry so a hello-world bundle does not carry them (ADR-052). The records, the errors and
 * their codecs (`WsMessage`, `WsError`, `SseEvent`, ...) are in the main entry; this entry has the
 * bindings, the default adapters and `OptInPortIds`, the ids they are registered under.
 *
 * Register them before the first use, one line each:
 *
 * ```ts
 * import { UndraCore } from "@undra/runtime";
 * import { OptInPortIds, browserWebSocket, fetchSse, ssePort, webSocketPort } from "@undra/runtime/realtime";
 *
 * await UndraCore.load({
 *   mode: "wasm-main",
 *   wasm,
 *   expectedSchemaHash: UndraIds.schemaHash,
 *   ports: {
 *     [OptInPortIds.WebSocket.portId]: webSocketPort(browserWebSocket()),
 *     [OptInPortIds.Sse.portId]: ssePort(fetchSse()),
 *   },
 * });
 * ```
 *
 * In `wasm-worker` mode both ports run here, on the main thread, like every asynchronous port
 * (ADR-049 §2): the worker sends their calls across.
 */
export { OptInPortIds } from "./adapters/opt-in-ids.js";
export { webSocketPort, type WebSocketAdapter, type WebSocketConnection } from "./realtime/websocket.js";
export {
  browserWebSocket,
  DID_NOT_KEEP_UP,
  HEADERS_REFUSED,
  type BrowserWebSocketOptions,
  type PlatformWebSocket,
  type WebSocketConstructorLike,
  type WebSocketHeaderInit,
} from "./realtime/browser-websocket.js";
export { nodeWebSocket, type NodeWebSocketOptions } from "./realtime/node-websocket.js";
export { ssePort, type SseAdapter, type SseStream } from "./realtime/sse.js";
export { fetchSse, type FetchLike, type FetchSseOptions } from "./realtime/fetch-sse.js";
export { SseParser } from "./realtime/sse-parser.js";
