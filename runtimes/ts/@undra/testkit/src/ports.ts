import { UndraPortError, type PortImpl } from "@undra/runtime";
import type { PortStatusName, RecordedEvent, Recording } from "./recording.js";
import { standardName } from "./names.js";
import { writeRecording } from "./recording.js";
import { portId } from "./names.js";

/** Options of a {@link PortRecorder}. */
export interface PortRecorderOptions {
  /** The schema hash of the core the traffic belongs to. */
  readonly schemaHash: bigint;
  /** Milliseconds, any origin: the recording's times are relative to the first reading. Default `performance.now()`; pass a manual clock for byte-for-byte reproducible recordings. */
  readonly now?: () => number;
  /** Informational: the host platform, e.g. `"web"`. */
  readonly platform?: string;
  /** Informational: where the recording came from. Default `"adapters"`. */
  readonly source?: string;
}

/**
 * Records the port traffic of the `PortImpl`s it wraps: for each call, the port, the method, the encoded arguments, the reply (or the typed
 * error, or "unavailable") and the time. Hand the wrapped ports to `UndraCore.load`/`attach` as `ports`, and write the result with {@link PortRecorder.toJson}.
 *
 * The recording holds `port_call` and `port_reply` events only: `undra dev --record` captures the whole session.
 */
export class PortRecorder {
  readonly #options: PortRecorderOptions;
  readonly #events: RecordedEvent[] = [];
  readonly #start: number;
  #next = 0;

  constructor(options: PortRecorderOptions) {
    this.#options = options;
    this.#start = (options.now ?? (() => performance.now()))();
  }

  #t(): number {
    const read = (this.#options.now ?? (() => performance.now()))();
    const t = Math.max(0, Math.floor(read - this.#start));
    const last = this.#events[this.#events.length - 1];
    return last === undefined ? t : Math.max(last.t, t);
  }

  /** `impl` with every method recording its calls. */
  wrap(port: number, impl: PortImpl): PortImpl {
    const methods: Record<number, (args: Uint8Array, portCallId?: number) => Uint8Array | Promise<Uint8Array>> = {};
    for (const [key, method] of Object.entries(impl.methods)) {
      const methodId = Number(key);
      methods[methodId] = (args, portCallId) => {
        this.#next += 1;
        const call = this.#next;
        this.#events.push({ t: this.#t(), kind: "port_call", port, method: methodId, call, args: args.slice() });
        const reply = (status: PortStatusName, body: Uint8Array): void => {
          this.#events.push({ t: this.#t(), kind: "port_reply", call, status, body: body.slice() });
        };
        const failed = (error: unknown): never => {
          if (error instanceof UndraPortError) reply("error", error.body);
          else reply("unavailable", new Uint8Array(0));
          throw error;
        };
        let result: Uint8Array | Promise<Uint8Array>;
        try {
          result = method(args, portCallId);
        } catch (error) {
          return failed(error);
        }
        if (result instanceof Promise) {
          return result.then(
            (body) => {
              reply("ok", body);
              return body;
            },
            (error: unknown) => failed(error),
          );
        }
        reply("ok", result);
        return result;
      };
    }
    return { sync: impl.sync, methods };
  }

  /** Wraps every port of `ports` (by port id); the result is what to register. */
  wrapAll(ports: ReadonlyMap<number, PortImpl> | Readonly<Record<number, PortImpl>>): Record<number, PortImpl> {
    const entries = ports instanceof Map ? [...ports.entries()] : Object.entries(ports).map(([k, v]) => [Number(k), v] as const);
    return Object.fromEntries(entries.map(([id, impl]) => [id, this.wrap(id, impl)]));
  }

  /** What has been recorded. */
  record(): Recording {
    const base = { schemaHash: this.#options.schemaHash, source: this.#options.source ?? "adapters", events: [...this.#events] };
    return this.#options.platform === undefined ? base : { ...base, platform: this.#options.platform };
  }

  /** The recording as canonical JSON. */
  toJson(): string {
    return writeRecording(this.record());
  }
}

/** Where a replay left the recording. */
export type ReplayError =
  | { readonly kind: "mismatch"; readonly port: number; readonly called: string; readonly expected: string; readonly argsDiffer: boolean; readonly nth: number }
  | { readonly kind: "exhausted"; readonly port: number; readonly called: string; readonly recorded: number }
  | { readonly kind: "unanswered"; readonly port: number; readonly called: string; readonly nth: number }
  | { readonly kind: "unconsumed"; readonly port: number; readonly next: string; readonly remaining: number };

/** Describes a {@link ReplayError} in a sentence. */
export function describeReplayError(e: ReplayError): string {
  switch (e.kind) {
    case "mismatch":
      return e.argsDiffer
        ? `replay: call ${e.nth} of the port was ${e.called} with other arguments than the recording's`
        : `replay: call ${e.nth} of the port was ${e.called}, the recording has ${e.expected} next`;
    case "exhausted":
      return `replay: ${e.called} was called after the recording's ${e.recorded} call(s) of the port were used up`;
    case "unanswered":
      return `replay: call ${e.nth} of the port, ${e.called}, has no reply in the recording; it was answered unavailable`;
    case "unconsumed":
      return `replay: ${e.remaining} recorded call(s) were never made, the next is ${e.next}`;
  }
}

/** Thrown by {@link Replayer.finish} when the replay deviated; `errors` has every deviation. */
export class ReplayFailure extends Error {
  constructor(readonly errors: readonly ReplayError[]) {
    super(errors.map(describeReplayError).join("\n"));
    this.name = "ReplayFailure";
  }
}

/** How strictly a replayed call must match the recorded one. */
export type ArgsPolicy = "exact" | "ignore";

interface Expected {
  readonly method: number;
  readonly args: Uint8Array;
  reply: { readonly status: PortStatusName; readonly body: Uint8Array } | undefined;
}

const SYNC_PORTS = new Set([portId("Clock"), portId("Rng"), portId("Log"), portId("Timer")]);

const label = (port: number, method: number): string => standardName(port, method) ?? `port ${port} method ${method}`;

const sameBytes = (a: Uint8Array, b: Uint8Array): boolean => a.length === b.length && a.every((x, i) => x === b[i]);

/**
 * Answers a core's port calls from a recording, in order: the recording's `port_call` events, per port, are the script. A call that is
 * the next recorded one of its port (same method and, by default, the same arguments) gets the recorded reply; anything else is a typed
 * {@link ReplayError}, kept for {@link Replayer.finish}, and the core is answered "unavailable". A deviation consumes nothing, so one
 * wrong call does not shift every later answer. Time is not replayed: answers are immediate.
 */
export class Replayer {
  readonly #policy: ArgsPolicy;
  readonly #queues = new Map<number, Expected[]>();
  readonly #recorded = new Map<number, number>();
  readonly #answered = new Map<number, number>();
  readonly #errors: ReplayError[] = [];

  constructor(recording: Recording, options: { readonly args?: ArgsPolicy } = {}) {
    this.#policy = options.args ?? "exact";
    const open = new Map<number, { port: number; index: number }>();
    for (const e of recording.events) {
      if (e.kind === "port_call") {
        const queue = this.#queues.get(e.port) ?? [];
        this.#queues.set(e.port, queue);
        open.set(e.call, { port: e.port, index: queue.length });
        queue.push({ method: e.method, args: e.args, reply: undefined });
        this.#recorded.set(e.port, (this.#recorded.get(e.port) ?? 0) + 1);
      } else if (e.kind === "port_reply") {
        const at = open.get(e.call);
        if (at === undefined) continue;
        open.delete(e.call);
        const expected = this.#queues.get(at.port)?.[at.index];
        if (expected !== undefined) expected.reply = { status: e.status, body: e.body };
      }
    }
  }

  #answer(port: number, method: number, args: Uint8Array): Uint8Array {
    const nth = this.#answered.get(port) ?? 0;
    const queue = this.#queues.get(port);
    const next = queue?.[0];
    if (next === undefined) {
      const error: ReplayError = { kind: "exhausted", port, called: label(port, method), recorded: this.#recorded.get(port) ?? 0 };
      this.#errors.push(error);
      throw new Error(describeReplayError(error));
    }
    const sameMethod = next.method === method;
    if (!(sameMethod && (this.#policy === "ignore" || sameBytes(next.args, args)))) {
      const error: ReplayError = { kind: "mismatch", port, called: label(port, method), expected: label(port, next.method), argsDiffer: sameMethod, nth };
      this.#errors.push(error);
      throw new Error(describeReplayError(error));
    }
    queue?.shift();
    this.#answered.set(port, nth + 1);
    const reply = next.reply;
    if (reply === undefined) {
      this.#errors.push({ kind: "unanswered", port, called: label(port, method), nth });
      throw new Error("the recording holds no reply for this port call");
    }
    if (reply.status === "unavailable") throw new Error("the recorded port reply is unavailable");
    if (reply.status === "error") throw new UndraPortError(reply.body);
    return reply.body;
  }

  /** One `PortImpl` per recorded port, ready to register (`ports` of `UndraCore.load`). Every method id of the port answers, so a call the recording never made is reported as a deviation. */
  ports(): Record<number, PortImpl> {
    const out: Record<number, PortImpl> = {};
    for (const port of this.#queues.keys()) {
      const methods = new Proxy<Record<number, (args: Uint8Array) => Uint8Array>>(
        {},
        { get: (_target, key) => (typeof key === "string" && /^\d+$/.test(key) ? (args: Uint8Array) => this.#answer(port, Number(key), args) : undefined) },
      );
      out[port] = { sync: SYNC_PORTS.has(port), methods };
    }
    return out;
  }

  /** The deviations seen so far. */
  errors(): readonly ReplayError[] {
    return [...this.#errors];
  }

  /** How many recorded calls are still waiting to be made. */
  get remaining(): number {
    let n = 0;
    for (const queue of this.#queues.values()) n += queue.length;
    return n;
  }

  /** The deviations so far followed by one `unconsumed` per port with calls left; empty when the replay was clean. */
  problems(): ReplayError[] {
    const problems = [...this.#errors];
    for (const [port, queue] of [...this.#queues.entries()].sort((a, b) => a[0] - b[0])) {
      const next = queue[0];
      if (next !== undefined) problems.push({ kind: "unconsumed", port, next: label(port, next.method), remaining: queue.length });
    }
    return problems;
  }

  /**
   * Ends the replay.
   *
   * @throws ReplayFailure when a call deviated or recorded calls were never made.
   */
  finish(): void {
    const problems = this.problems();
    if (problems.length > 0) throw new ReplayFailure(problems);
  }
}
