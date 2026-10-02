import { port } from "./ids.js";

/**
 * Port and method ids of the opt-in ports (ADR-047, ADR-048), what an app registers their
 * bindings under: `{ [OptInPortIds.WebSocket.portId]: webSocketPort(browserWebSocket()) }`.
 * Exported by `@undra/runtime/realtime` and `@undra/runtime/db`, not by the main entry, so an
 * app that does not use the ports ships none of this (ADR-052).
 */
export const OptInPortIds = /* @__PURE__ */ Object.freeze({
  WebSocket: port("WebSocket", ["connect", "send", "receive", "close"] as const),
  Sse: port("Sse", ["open", "next", "close"] as const),
  Db: port("Db", ["open", "execute", "query", "begin", "commit", "rollback", "close"] as const),
});
