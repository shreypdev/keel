import type { WebSocketFactory, WebSocketLike } from "../../src/transport/remote.js";
import {
  type ChangeEntry,
  type Envelope,
  Kind,
  decodeEnvelope,
  encodeChangeSet,
  encodeEnvelope,
  encodeHello,
  encodeReply,
  ReplyStatus,
} from "../../src/wire/index.js";
import { SCHEMA } from "./fake-core.js";

/*
 * A WebSocket that is driven by hand, and the "server" behind it: the tests of the remote
 * transport open, answer, drop and refuse connections exactly when they want to, with the
 * clock held by `vi.useFakeTimers()`. Every message the client sends is decoded as an envelope.
 */

/** What the next connection does by itself. */
export type Behaviour =
  /** Opens at once and answers the client's `Hello` with the server's. */
  | "answer"
  /** Closes with 1006 before it opens: the connection is refused. */
  | "refuse"
  /** Opens but never answers `Hello`. */
  | "silent";

/** One socket the client opened. */
export class FakeSocket implements WebSocketLike {
  binaryType = "blob";
  readyState = 0;
  /** Everything the client sent, decoded. */
  readonly envelopes: Envelope[] = [];
  /** How the client closed it, if it did. */
  closedByClient: { code: number | undefined; reason: string | undefined } | null = null;
  readonly #listeners = new Map<string, Set<(event: Event) => void>>();
  #serverSeq = 0;

  constructor(
    readonly url: string,
    readonly server: FakeServer,
  ) {}

  send(data: Uint8Array): void {
    const envelope = decodeEnvelope(data);
    this.envelopes.push(envelope);
    if (envelope.kind === Kind.Hello) this.server.onHello(this);
  }

  close(code?: number, reason?: string): void {
    if (this.readyState === 3) return;
    this.readyState = 3;
    this.closedByClient = { code, reason };
  }

  addEventListener(type: string, listener: (event: Event) => void): void {
    let set = this.#listeners.get(type);
    if (set === undefined) this.#listeners.set(type, (set = new Set()));
    set.add(listener);
  }

  removeEventListener(type: string, listener: (event: Event) => void): void {
    this.#listeners.get(type)?.delete(listener);
  }

  /** The number of listeners left on the socket (the client detaches them when it lets go). */
  get listenerCount(): number {
    let n = 0;
    for (const set of this.#listeners.values()) n += set.size;
    return n;
  }

  // ----- the server's side ---------------------------------------------------------

  #emit(type: string, event: object): void {
    for (const listener of [...(this.#listeners.get(type) ?? [])]) listener(event as Event);
  }

  open(): void {
    this.readyState = 1;
    this.#emit("open", {});
  }

  /** The server sends one envelope (the sequence number counts up from 0). */
  deliver(kind: Kind, payload: Uint8Array, schema: bigint = this.server.schemaHash): void {
    const bytes = encodeEnvelope(kind, this.#serverSeq++, schema, payload);
    const data = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
    this.#emit("message", { data });
  }

  /** The server sends something that is not an envelope: a text message. */
  deliverText(text: string): void {
    this.#emit("message", { data: text });
  }

  sendHello(schema: bigint = this.server.schemaHash): void {
    this.deliver(Kind.Hello, encodeHello({ undraVersion: "test", schemaHash: schema, platform: "rust", mode: "dev" }), schema);
  }

  /** The server (or the network) ends the connection. */
  serverClose(code = 1006, reason = ""): void {
    this.readyState = 3;
    this.#emit("close", { code, reason });
  }

  /** The envelopes of `kind` the client sent. */
  sent(kind: Kind): Envelope[] {
    return this.envelopes.filter((e) => e.kind === kind);
  }

  /** The query parameters of the URL the client connected to. */
  get query(): URLSearchParams {
    return new URL(this.url).searchParams;
  }
}

/** A dev server for the client to connect to, as far as the transport can tell. */
export class FakeServer {
  schemaHash: bigint = SCHEMA;
  behaviour: Behaviour = "answer";
  /** Every socket the client opened, in order. */
  readonly sockets: FakeSocket[] = [];
  /** The WebSocket constructor to hand the client. */
  readonly Socket: WebSocketFactory;

  constructor() {
    const server = this;
    this.Socket = class extends FakeSocket {
      constructor(url: string) {
        super(url, server);
        server.sockets.push(this);
        queueMicrotask(() => {
          if (server.behaviour === "refuse") this.serverClose(1006);
          else this.open();
        });
      }
    };
  }

  /** The socket the client uses now. */
  get current(): FakeSocket {
    const socket = this.sockets.at(-1);
    if (socket === undefined) throw new Error("the client has not connected yet");
    return socket;
  }

  onHello(socket: FakeSocket): void {
    if (this.behaviour === "answer") queueMicrotask(() => socket.sendHello());
  }

  /** A change-set for the current socket, as the core sends one. */
  changeSet(entries: readonly ChangeEntry[], txnId = 1n): void {
    this.current.deliver(Kind.ChangeSet, encodeChangeSet({ txnId, entries }));
  }

  /** A reply to a call. */
  reply(callId: number, body: Uint8Array): void {
    this.current.deliver(Kind.Reply, encodeReply({ callId, status: ReplyStatus.Ok, body }));
  }
}
