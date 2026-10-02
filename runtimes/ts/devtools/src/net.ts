/** The socket to the dev server, with the reconnect a rebuild needs (the runner is replaced and listens again). */

import type { ConnState } from "./state.js";

/** The part of `WebSocket` the page uses, so a test can stand in for it. */
export interface SocketLike {
  binaryType: string;
  onopen: ((event: unknown) => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onclose: ((event: { code: number }) => void) | null;
  onerror: ((event: unknown) => void) | null;
  send(data: Uint8Array): void;
  close(): void;
}

export interface ConnectionOptions {
  readonly url: string;
  readonly make?: (url: string) => SocketLike;
  readonly onMessage: (bytes: Uint8Array) => void;
  readonly onState: (state: ConnState) => void;
  /** Called when the page stops trying, with why. */
  readonly onGiveUp?: (reason: string) => void;
  readonly setTimer?: (fn: () => void, ms: number) => unknown;
  readonly clearTimer?: (handle: unknown) => void;
}

/** The delay before attempt `n` (from 1): 250 ms doubling up to 5 s. */
export function backoffMs(attempt: number): number {
  return Math.min(5000, 250 * 2 ** Math.max(0, attempt - 1));
}

/** Failed attempts, before the first message ever arrived, after which the page stops: the token was refused or the server is gone. */
const GIVE_UP_AFTER = 4;

export class Connection {
  #socket: SocketLike | undefined;
  #attempt = 0;
  #everOpened = false;
  #stopped = false;
  #timer: unknown;
  readonly #opts: ConnectionOptions;

  constructor(opts: ConnectionOptions) {
    this.#opts = opts;
  }

  start(): void {
    this.#stopped = false;
    this.#connect();
  }

  stop(): void {
    this.#stopped = true;
    (this.#opts.clearTimer ?? clearTimeout)(this.#timer as number);
    this.#socket?.close();
  }

  send(bytes: Uint8Array): boolean {
    if (this.#socket === undefined || this.#state !== "open") return false;
    this.#socket.send(bytes);
    return true;
  }

  #state: ConnState = "connecting";

  #set(state: ConnState): void {
    this.#state = state;
    this.#opts.onState(state);
  }

  #connect(): void {
    this.#set(this.#everOpened ? "reconnecting" : "connecting");
    const socket = (this.#opts.make ?? ((u) => new WebSocket(u) as unknown as SocketLike))(this.#opts.url);
    socket.binaryType = "arraybuffer";
    this.#socket = socket;
    socket.onopen = () => {
      this.#attempt = 0;
      this.#everOpened = true;
      this.#set("open");
    };
    socket.onmessage = (event) => {
      if (event.data instanceof ArrayBuffer) this.#opts.onMessage(new Uint8Array(event.data));
    };
    socket.onclose = (event) => {
      if (this.#socket !== socket) return;
      this.#socket = undefined;
      if (this.#stopped) return;
      this.#attempt += 1;
      // 1013: the server has as many pages as it allows; trying again will not help.
      if (event.code === 1013) return this.#giveUp("the dev server already has as many devtools pages open as it allows");
      if (!this.#everOpened && this.#attempt >= GIVE_UP_AFTER) {
        return this.#giveUp("could not connect: the token in this address is not the one the dev server printed, or `undra dev` is not running");
      }
      this.#set("reconnecting");
      this.#timer = (this.#opts.setTimer ?? setTimeout)(() => this.#connect(), backoffMs(this.#attempt));
    };
    socket.onerror = () => undefined;
  }

  #giveUp(reason: string): void {
    this.#set("closed");
    this.#opts.onGiveUp?.(reason);
  }
}
