import type { SseEvent } from "../adapters/types.js";

/** The largest `retry` the wire carries (`u32`). */
const MAX_RETRY = 0xffff_ffff;

/**
 * The event-stream parser of the HTML standard ("Interpreting an event stream"), the one every
 * TypeScript and React Native adapter uses (`fetchSse`, the React Native XHR path). Feed it text
 * as it arrives, in pieces of any size; it returns the events each piece completed.
 *
 * * Lines end with CRLF, LF or CR (a CR at the end of a piece waits for the next piece to see
 *   whether an LF follows). A leading U+FEFF (byte order mark) is skipped.
 * * A line starting with `:` is a comment. `field: value` drops one space after the colon; a line
 *   without a colon is a field with an empty value; unknown fields are ignored.
 * * `event` sets the event type, `data` appends its value and a LF, `id` sets the last event id
 *   (unless the value contains NUL; it persists across events and starts as the `lastEventId` the
 *   stream was opened with), `retry` made only of ASCII digits sets this event's `retryMs`.
 * * A blank line dispatches: with no data, the data and the type are reset and nothing is
 *   dispatched; else one trailing LF is removed from the data and `{ id, event, data, retryMs }`
 *   is emitted (`id` `null` when the last event id is empty, `event` `"message"` when no type was
 *   set), and the data, the type and the retry are reset.
 *
 * ```ts
 * const parser = new SseParser();
 * parser.push("data: one\n");      // []
 * parser.push("\nid: 2\ndata: tw"); // [{ id: null, event: "message", data: "one", retryMs: null }]
 * ```
 */
export class SseParser {
  #pending = "";
  #started = false;
  #skipLf = false;
  #data = "";
  #hasData = false;
  #type = "";
  #retry: number | null = null;
  #lastEventId: string;

  /** @param lastEventId The last event id the stream resumes after (the `Last-Event-ID` it was opened with), if any. */
  constructor(lastEventId: string | null = null) {
    this.#lastEventId = lastEventId ?? "";
  }

  /** Parses the next piece of the stream; returns the events it completed, in order. */
  push(text: string): SseEvent[] {
    if (text.length === 0) return [];
    if (!this.#started) {
      this.#started = true;
      if (text.charCodeAt(0) === 0xfeff) text = text.slice(1);
    }
    if (this.#skipLf) {
      this.#skipLf = false;
      if (text.charCodeAt(0) === 0x0a) text = text.slice(1);
    }
    const events: SseEvent[] = [];
    let buffer = this.#pending + text;
    let start = 0;
    for (let i = 0; i < buffer.length; i++) {
      const c = buffer.charCodeAt(i);
      if (c !== 0x0a && c !== 0x0d) continue;
      this.#line(buffer.slice(start, i), events);
      if (c === 0x0d) {
        if (i + 1 < buffer.length) {
          if (buffer.charCodeAt(i + 1) === 0x0a) i++;
        } else {
          this.#skipLf = true;
        }
      }
      start = i + 1;
    }
    buffer = buffer.slice(start);
    this.#pending = buffer;
    return events;
  }

  /** The stream ended: an incomplete line and an event without its blank line are discarded, as the standard says. */
  end(): void {
    this.#pending = "";
    this.#skipLf = false;
    this.#data = "";
    this.#hasData = false;
    this.#type = "";
    this.#retry = null;
  }

  /** The last event id so far (`""` when none), for a reconnect's `Last-Event-ID`. */
  get lastEventId(): string {
    return this.#lastEventId;
  }

  #line(line: string, events: SseEvent[]): void {
    if (line.length === 0) {
      this.#dispatch(events);
      return;
    }
    if (line.charCodeAt(0) === 0x3a) return;
    const colon = line.indexOf(":");
    let field: string;
    let value: string;
    if (colon < 0) {
      field = line;
      value = "";
    } else {
      field = line.slice(0, colon);
      value = line.slice(colon + 1);
      if (value.charCodeAt(0) === 0x20) value = value.slice(1);
    }
    switch (field) {
      case "event":
        this.#type = value;
        break;
      case "data":
        this.#data += `${value}\n`;
        this.#hasData = true;
        break;
      case "id":
        if (!value.includes("\0")) this.#lastEventId = value;
        break;
      case "retry":
        if (/^[0-9]+$/.test(value)) {
          const ms = Number(value);
          if (ms <= MAX_RETRY) this.#retry = ms;
        }
        break;
      default:
        break;
    }
  }

  #dispatch(events: SseEvent[]): void {
    if (!this.#hasData) {
      this.#data = "";
      this.#type = "";
      return;
    }
    const data = this.#data.endsWith("\n") ? this.#data.slice(0, -1) : this.#data;
    events.push({
      id: this.#lastEventId === "" ? null : this.#lastEventId,
      event: this.#type === "" ? "message" : this.#type,
      data,
      retryMs: this.#retry,
    });
    this.#data = "";
    this.#hasData = false;
    this.#type = "";
    this.#retry = null;
  }
}
