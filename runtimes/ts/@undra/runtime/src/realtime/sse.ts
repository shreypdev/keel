import { HeaderCodec, SseErrorCodec, SseEventCodec } from "../adapters/codecs.js";
import { PortIds } from "../adapters/ids.js";
import { type Header, SseError, type SseEvent } from "../adapters/types.js";
import { UndraPortError } from "../errors.js";
import { errorMessage } from "../platform.js";
import type { PortImpl } from "../port.js";
import { codecs, encodeValue } from "../wire/index.js";
import { Lines, readArgs } from "./lines.js";

/**
 * Opens server-sent event streams for the core's `Sse` port (ADR-047). Implement it to replace
 * the default ({@link fetchSse}); {@link ssePort} turns it into the port.
 */
export interface SseAdapter {
  /**
   * Requests `url` with `Accept: text/event-stream`, `Cache-Control: no-cache`, `headers` and, when
   * given, `Last-Event-ID: <lastEventId>`. Resolves once a 2xx `text/event-stream` answer arrived;
   * rejects with an {@link SseError} (`Refused` with the status for any other status, `Protocol`
   * for another content type).
   */
  open(url: string, headers: readonly Header[], lastEventId: string | null): Promise<SseStream>;
}

/** One open stream of an {@link SseAdapter}. */
export interface SseStream {
  /**
   * The events, pulled (the binding asks for the next one only while the core's credit has room;
   * read the body only when asked). The iteration ends by throwing an {@link SseError} (`Ended`
   * when the body ended, `Network` when reading failed, `Protocol` for text that is not UTF-8), or
   * by finishing after `close`. Called once.
   */
  events(): AsyncIterable<SseEvent>;
  /** Stops the stream and releases the connection. Never rejects. */
  close(): Promise<void>;
}

const headers = /* @__PURE__ */ codecs.vec(HeaderCodec);
const optionString = /* @__PURE__ */ codecs.option(codecs.string);
const events = /* @__PURE__ */ codecs.vec(SseEventCodec);
const EMPTY = new Uint8Array(0);

/** What the binding keeps per stream. */
interface Held {
  readonly stream: SseStream;
}

/** `error` as an {@link SseError}: a typed one as it is, anything else as `Network(<its text>)`. */
function asSseError(error: unknown): SseError {
  return error instanceof SseError ? error : new SseError.Network(errorMessage(error));
}

async function typed(work: () => Promise<Uint8Array>): Promise<Uint8Array> {
  try {
    return await work();
  } catch (error) {
    throw new UndraPortError(encodeValue(SseErrorCodec, asSseError(error)));
  }
}

async function closeQuietly(stream: SseStream): Promise<void> {
  try {
    await stream.close();
  } catch {
    // `close` never fails by contract.
  }
}

/**
 * The core's `Sse` port over `adapter` (ADR-047), for `LoadOptions.ports` or
 * `core.registerPort(PortIds.Sse.portId, ...)`. One instance per core. The same discipline as
 * {@link webSocketPort}: `open` refuses a URL that is not `http://` or `https://` (`Refused`, status
 * `null`); events are read ahead only up to the core's latest `max` (16 before the first `next`);
 * `next` answers up to `max` of them (a burst as one reply: 2 ms of quiet or 8 ms after its first
 * event), `[]` after the core's `close`, the stream's end (sticky; an `events()` that finishes by
 * itself is `Ended`), else waits; one `next` at a time; `close` answers
 * a waiting `next` with `[]`; the core's shutdown (and a wasm core's restart after a trap) closes every stream.
 */
export function ssePort(adapter: SseAdapter): PortImpl {
  const lines = new Lines<SseEvent, Held, SseError>({
    unknown: (id) => new SseError.Network(`no event stream ${id}`),
    pending: (id) => new SseError.Protocol(`a next is already pending on stream ${id}`),
    coerce: asSseError,
    finished: () => new SseError.Ended(),
  });
  /** Bumped by `dispose`: an open that was under way then is closed when it answers. */
  let epoch = 0;
  const ids = PortIds.Sse;
  return {
    name: "Sse",
    sync: false,
    methods: {
      [ids.open]: (args) => {
        const [url, extra, lastEventId] = readArgs(args, (r) => [r.readStr(), headers.decode(r), optionString.decode(r)] as const);
        return typed(async () => {
          if (!url.startsWith("http://") && !url.startsWith("https://")) throw new SseError.Refused(null, `invalid URL: ${url}`);
          const asked = epoch;
          const stream = await adapter.open(url, extra, lastEventId);
          if (epoch !== asked) {
            await closeQuietly(stream);
            throw new SseError.Network("the core went away while the stream opened");
          }
          return encodeValue(codecs.u32, lines.open({ stream }, () => stream.events()));
        });
      },
      [ids.next]: (args) => {
        const [stream, max] = readArgs(args, (r) => [r.readU32(), r.readU32()] as const);
        return typed(async () => encodeValue(events, await lines.pull(stream, max)));
      },
      [ids.close]: (args) => {
        const stream = readArgs(args, (r) => r.readU32());
        return typed(async () => {
          const line = lines.get(stream);
          if (lines.close(stream, line)) await closeQuietly(line.handle.stream);
          return EMPTY;
        });
      },
    },
    dispose() {
      epoch++;
      for (const [id, line] of lines.live()) {
        lines.close(id, line);
        void closeQuietly(line.handle.stream);
      }
    },
  };
}
