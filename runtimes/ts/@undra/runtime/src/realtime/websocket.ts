import { HeaderCodec, WsErrorCodec, WsMessageCodec, WsOpenedCodec } from "../adapters/codecs.js";
import { OptInPortIds } from "../adapters/opt-in-ids.js";
import { type Header, WsError, type WsMessage } from "../adapters/types.js";
import { UndraPortError } from "../errors.js";
import { errorMessage } from "../platform.js";
import type { PortImpl } from "../port.js";
import { codecs, encodeValue } from "../wire/index.js";
import { Lines, readArgs } from "./lines.js";
import { msg } from "../messages.js";

/**
 * Opens WebSocket connections for the core's `WebSocket` port (ADR-047). Implement it to replace
 * the default ({@link browserWebSocket}, {@link nodeWebSocket}); {@link webSocketPort} turns it into
 * the port and owns ids, the pull and the read-ahead.
 *
 * ```ts
 * import { UndraCore } from "@undra/runtime";
 * import { OptInPortIds, browserWebSocket, webSocketPort } from "@undra/runtime/realtime";
 *
 * await UndraCore.load({ ..., ports: { [OptInPortIds.WebSocket.portId]: webSocketPort(browserWebSocket()) } });
 * ```
 */
export interface WebSocketAdapter {
  /**
   * Opens a connection to `url` (`ws://` or `wss://`), offering `protocols` and sending `headers`
   * with the upgrade. Resolves once the handshake succeeded; rejects with a {@link WsError}
   * (`Refused` for a refused upgrade, with its HTTP status where the platform reports it).
   */
  connect(url: string, protocols: readonly string[], headers: readonly Header[]): Promise<WebSocketConnection>;
}

/** One open connection of a {@link WebSocketAdapter}. */
export interface WebSocketConnection {
  /** The subprotocol the server selected, or `""`. */
  readonly protocol: string;
  /**
   * The inbound messages, pulled: the binding asks for the next one only while the core's credit
   * has room, so an implementation that reads the network lazily pushes back on the server. The
   * iteration ends by throwing a {@link WsError} (`Closed` for the peer's close frame, `Network` for
   * a drop, `Protocol` for a broken frame), or by finishing after `close`. Called once.
   */
  messages(): AsyncIterable<WsMessage>;
  /** Sends `message`; resolves when the platform accepted it and its outbound buffer is under 1 MiB. Rejects with a {@link WsError}. */
  send(message: WsMessage): Promise<void>;
  /** Starts the closing handshake with `code` and `reason`; `messages()` then finishes. Never rejects. */
  close(code: number, reason: string): Promise<void>;
}

/** The close code of a connection the core is gone from (1001, going away): core shutdown, a replaced port. */
const GOING_AWAY = 1001;

const headers = /* @__PURE__ */ codecs.vec(HeaderCodec);
const strings = /* @__PURE__ */ codecs.vec(codecs.string);
const messages = /* @__PURE__ */ codecs.vec(WsMessageCodec);
const EMPTY = new Uint8Array(0);

/** What the binding keeps per connection. */
interface Held {
  readonly connection: WebSocketConnection;
  /** The core's close, once it closed the connection. */
  closedWith: { readonly code: number; readonly reason: string } | null;
}

/** `error` as a {@link WsError}: a typed one as it is, anything else as `Network(<its text>)`. */
function asWsError(error: unknown): WsError {
  return error instanceof WsError ? error : new WsError.Network(errorMessage(error));
}

/** Runs `work`, failing the port call with the typed error (status 1) of whatever it threw. */
async function typed(work: () => Promise<Uint8Array>): Promise<Uint8Array> {
  try {
    return await work();
  } catch (error) {
    throw new UndraPortError(encodeValue(WsErrorCodec, asWsError(error)));
  }
}

/** Closes `connection`, ignoring what an adapter that broke its contract threw. */
async function closeQuietly(connection: WebSocketConnection, code: number, reason: string): Promise<void> {
  try {
    await connection.close(code, reason);
  } catch {
    // `close` never fails by contract; a failure here has nobody to tell.
  }
}

/**
 * The core's `WebSocket` port over `adapter` (ADR-047), for `LoadOptions.ports` or
 * `core.registerPort(OptInPortIds.WebSocket.portId, ...)`. One instance per core.
 *
 * * `connect` refuses a URL that is not `ws://` or `wss://` (`Refused`, status `null`) before the
 *   adapter is asked; anything but a {@link WsError} the adapter throws is `Network(<its text>)`.
 *   Connection ids count from 1 and are never reused.
 * * Inbound messages are read from `messages()` into a buffer only while it holds fewer than the
 *   core's latest `max` (16 before the first `receive`). `receive` answers up to `max` buffered
 *   messages at once, `[]` after the core's close, the connection's end (sticky), else waits; a
 *   burst is one reply (a `receive` with fewer than `max` waits for more until `max` are there,
 *   2 ms pass with nothing new, or 8 ms pass since its first message); a second `receive` while one
 *   waits is `Protocol`.
 * * `send` after the core's close is `Closed` with that close's code and reason; after the
 *   connection ended, its end.
 * * `close` answers a waiting `receive` with `[]`, drops the buffer and closes the adapter's
 *   connection; closing again is not an error.
 * * When the core closes, the port is replaced, or a wasm core restarts after a trap (`dispose`),
 *   every open connection is closed with 1001; after a restart the port serves the new instance.
 */
export function webSocketPort(adapter: WebSocketAdapter): PortImpl {
  const lines = new Lines<WsMessage, Held, WsError>({
    unknown: (id) => new WsError.Network(msg(152, id)),
    pending: (id) => new WsError.Protocol(msg(153, id)),
    coerce: asWsError,
    finished: () => new WsError.Network(msg(154)),
  });
  /** Bumped by `dispose`: a connect that was under way then is closed when it opens. */
  let epoch = 0;
  const ids = OptInPortIds.WebSocket;
  return {
    name: "WebSocket",
    sync: false,
    methods: {
      [ids.connect]: (args) => {
        const [url, protocols, extra] = readArgs(args, (r) => [r.readStr(), strings.decode(r), headers.decode(r)] as const);
        return typed(async () => {
          if (!url.startsWith("ws://") && !url.startsWith("wss://")) throw new WsError.Refused(null, msg(144, url));
          const asked = epoch;
          const connection = await adapter.connect(url, protocols, extra);
          if (epoch !== asked) {
            await closeQuietly(connection, GOING_AWAY, "");
            throw new WsError.Network(msg(155));
          }
          const conn = lines.open({ connection, closedWith: null }, () => connection.messages());
          return encodeValue(WsOpenedCodec, { conn, protocol: connection.protocol });
        });
      },
      [ids.send]: (args) => {
        const [conn, message] = readArgs(args, (r) => [r.readU32(), WsMessageCodec.decode(r)] as const);
        return typed(async () => {
          const line = lines.get(conn);
          const closedWith = line.handle.closedWith;
          if (closedWith !== null) throw new WsError.Closed(closedWith.code, closedWith.reason);
          if (line.terminal !== null) throw line.terminal;
          await line.handle.connection.send(message);
          return EMPTY;
        });
      },
      [ids.receive]: (args) => {
        const [conn, max] = readArgs(args, (r) => [r.readU32(), r.readU32()] as const);
        return typed(async () => encodeValue(messages, await lines.pull(conn, max)));
      },
      [ids.close]: (args) => {
        const [conn, code, reason] = readArgs(args, (r) => [r.readU32(), r.readU16(), r.readStr()] as const);
        return typed(async () => {
          const line = lines.get(conn);
          if (line.handle.closedWith !== null) return EMPTY;
          line.handle.closedWith = { code, reason };
          lines.close(conn, line);
          await closeQuietly(line.handle.connection, code, reason);
          return EMPTY;
        });
      },
    },
    dispose() {
      epoch++;
      for (const [conn, line] of lines.live()) {
        line.handle.closedWith = { code: GOING_AWAY, reason: "" };
        lines.close(conn, line);
        void closeQuietly(line.handle.connection, GOING_AWAY, "");
      }
    },
  };
}
