// An `XMLHttpRequest` over Node's `http`, shaped like React Native's (Libraries/Network/XMLHttpRequest.js) as far as
// `reactNativeSse` uses it: `responseText` grows piece by piece, each piece fires `readystatechange` (LOADING) and
// `progress`; the end fires `readystatechange` (DONE) then `load`, `error` or `abort`, then `loadend`; a failed request
// replaces `responseText` with the error's text, as React Native does.
import { type ClientRequest, request } from "node:http";
import type { XMLHttpRequestLike } from "../../src/realtime.js";

export class NodeXhr implements XMLHttpRequestLike {
  /** Every request made, newest last (what the tests read the headers of). */
  static readonly made: NodeXhr[] = [];

  readyState = 0;
  status = 0;
  statusText = "";
  responseText = "";
  readonly headers: Record<string, string> = {};
  #method = "GET";
  #url = "";
  #responseHeaders: Record<string, string | string[] | undefined> = {};
  #listeners = new Map<string, Array<() => void>>();
  #request: ClientRequest | null = null;
  #done = false;

  open(method: string, url: string): void {
    this.#method = method;
    this.#url = url;
    this.#state(1);
  }

  setRequestHeader(name: string, value: string): void {
    this.headers[name] = value;
  }

  getResponseHeader(name: string): string | null {
    const value = this.#responseHeaders[name.toLowerCase()];
    if (value === undefined) return null;
    return Array.isArray(value) ? value.join(", ") : value;
  }

  addEventListener(type: string, listener: () => void): void {
    const list = this.#listeners.get(type) ?? [];
    list.push(listener);
    this.#listeners.set(type, list);
  }

  send(): void {
    NodeXhr.made.push(this);
    const req = request(this.#url, { method: this.#method, headers: this.headers }, (res) => {
      this.status = res.statusCode ?? 0;
      this.statusText = res.statusMessage ?? "";
      this.#responseHeaders = res.headers;
      this.#state(2);
      res.setEncoding("utf8");
      res.on("data", (piece: string) => {
        if (this.#done) return;
        this.responseText += piece;
        this.#state(3);
        this.#emit("progress");
      });
      res.on("end", () => this.#finish("load"));
      res.on("error", (error) => this.#fail(error));
    });
    req.on("error", (error) => this.#fail(error));
    req.end();
    this.#request = req;
  }

  abort(): void {
    if (this.#done) return;
    this.#request?.destroy();
    this.#finish("abort");
  }

  #fail(error: Error): void {
    if (this.#done) return;
    this.responseText = error.message;
    this.#finish("error");
  }

  #finish(how: "load" | "error" | "abort"): void {
    if (this.#done) return;
    this.#done = true;
    this.#state(4);
    this.#emit(how);
    this.#emit("loadend");
  }

  #state(next: number): void {
    this.readyState = next;
    this.#emit("readystatechange");
  }

  #emit(type: string): void {
    for (const listener of this.#listeners.get(type) ?? []) listener();
  }
}
