import {
  type AppState,
  type ClockAdapter,
  type ConnectivityAdapter,
  type FsAdapter,
  FsError,
  type HttpAdapter,
  HttpError,
  type HttpMethod,
  type HttpRequest,
  type HttpResponse,
  type KvAdapter,
  type LifecycleAdapter,
  type LogAdapter,
  type NetKind,
  type RngAdapter,
  type TimerAdapter,
  type UndraPanicReport,
} from "@undra/runtime";
import { compareUtf8 } from "./hex.js";

// The deterministic fakes of the standard ports (docs/TESTING.md): the same behaviour as `undra::ports::fakes`,
// held to it by testkit/conformance/fakes.json, which the test suite replays against these classes.

const NS_PER_MS = 1_000_000n;
const U64 = (1n << 64n) - 1n;

/** A timer armed on the fake clock. */
interface Armed {
  readonly deadlineNs: bigint;
  readonly seq: number;
  readonly id: number;
  readonly fire: (timerId: number) => void;
}

/**
 * Thrown by {@link FakeClock.advance} and `PreviewCore.advance` when one call fires more timers than its cap and another is still due:
 * a timer that re-arms itself at the same instant (or every millisecond across a long window) never lets time move on. The clock
 * stays at the last deadline that fired and the timers still armed stay armed.
 */
export class TimerStormError extends Error {
  constructor(
    /** How many timers fired before the cap stopped the call. */
    readonly fired: number,
    /** The id of the timer that was due next. */
    readonly timerId: number,
    /** The monotonic reading of the clock when it stopped, in whole milliseconds. */
    readonly atMs: number,
  ) {
    super(
      `advance fired ${fired} timers and timer ${timerId} is due again at ${atMs} ms: a timer that re-arms itself without time passing never ends (raise maxTimers if the window really holds that many)`,
    );
    this.name = "TimerStormError";
  }
}

/**
 * A deterministic `Clock` and `Timer`: time only moves when the test says so.
 *
 * `nowMs()` is a wall clock you can {@link FakeClock.setNowMs}; `monotonicNs()` starts at 0 and `advance` moves both by the
 * same amount. `set` arms a timer on the monotonic counter; `advance` fires every timer that comes due in deadline order
 * (ties in arming order) with the clock reading exactly the deadline while each one fires. Nothing here reads the system clock.
 */
export class FakeClock implements ClockAdapter, TimerAdapter {
  /** The wall-clock reading of a new clock: 2023-11-14T22:13:20Z. */
  static readonly DEFAULT_NOW_MS = 1_700_000_000_000;
  /** The most timers one {@link FakeClock.advance} fires by default. */
  static readonly MAX_TIMERS_PER_ADVANCE = 100_000;

  #wallNs: bigint;
  #monoNs = 0n;
  #seq = 0;
  #timers: Armed[] = [];

  constructor(nowMs: number = FakeClock.DEFAULT_NOW_MS) {
    this.#wallNs = BigInt(nowMs) * NS_PER_MS;
  }

  nowMs(): number {
    const ms = this.#wallNs / NS_PER_MS;
    return Number(this.#wallNs < 0n && this.#wallNs % NS_PER_MS !== 0n ? ms - 1n : ms);
  }

  monotonicNs(): bigint {
    return this.#monoNs;
  }

  /** Sets the wall clock. The monotonic counter and the armed timers are not affected: a wall-clock jump is not the passage of time. */
  setNowMs(nowMs: number): void {
    this.#wallNs = BigInt(nowMs) * NS_PER_MS;
  }

  /** `Timer.set`: arms timer `timerId` to fire `fire(timerId)` after `delayMs` of fake time. */
  set(timerId: number, delayMs: number, fire: (timerId: number) => void): void {
    this.#seq += 1;
    this.#timers.push({ deadlineNs: this.#monoNs + BigInt(Math.max(0, Math.ceil(delayMs))) * NS_PER_MS, seq: this.#seq, id: timerId, fire });
    this.#timers.sort((a, b) => (a.deadlineNs === b.deadlineNs ? a.seq - b.seq : a.deadlineNs < b.deadlineNs ? -1 : 1));
  }

  /** How many timers are armed and have not fired. */
  get pendingTimers(): number {
    return this.#timers.length;
  }

  /** The ids of the armed timers, in the order they will fire. */
  pendingTimerIds(): number[] {
    return this.#timers.map((t) => t.id);
  }

  /** Milliseconds until the next armed timer is due, or `undefined` when none is armed. */
  nextDueInMs(): number | undefined {
    const next = this.#timers[0];
    if (next === undefined) return undefined;
    const left = next.deadlineNs - this.#monoNs;
    return Number((left + NS_PER_MS - 1n) / NS_PER_MS);
  }

  /**
   * Fires the next armed timer if it is due within `limitMs` from now, moving the clock to its deadline first. Returns its id, or
   * `undefined` when none is due in the window (the clock does not move). {@link advance} is a loop over this; a harness that has
   * to wait for the core between timers (`PreviewCore.advance`) drives it itself.
   */
  fireNext(limitMs: number): number | undefined {
    const next = this.#timers[0];
    if (next === undefined) return undefined;
    const target = this.#monoNs + BigInt(Math.max(0, Math.ceil(limitMs))) * NS_PER_MS;
    if (next.deadlineNs > target) return undefined;
    this.#timers.shift();
    const moved = next.deadlineNs > this.#monoNs ? next.deadlineNs - this.#monoNs : 0n;
    this.#monoNs += moved;
    this.#wallNs += moved;
    next.fire(next.id);
    return next.id;
  }

  /** Moves the clock by `ms` without firing anything: what is left of a window after its last timer. */
  moveBy(ms: number): void {
    const by = BigInt(Math.max(0, Math.ceil(ms))) * NS_PER_MS;
    this.#monoNs += by;
    this.#wallNs += by;
  }

  /**
   * Moves time forward by `ms` and fires the timers that come due, in order; returns their ids. The callbacks run synchronously, and may arm
   * timers that fall inside the window (they fire in the same call).
   *
   * @param maxTimers the most timers one call may fire (default {@link FakeClock.MAX_TIMERS_PER_ADVANCE}).
   * @throws TimerStormError when `maxTimers` fired and another is still due.
   */
  advance(ms: number, maxTimers: number = FakeClock.MAX_TIMERS_PER_ADVANCE): number[] {
    const fired: number[] = [];
    let left = BigInt(Math.max(0, Math.ceil(ms))) * NS_PER_MS;
    for (;;) {
      const next = this.#timers[0];
      if (next === undefined || next.deadlineNs > this.#monoNs + left) break;
      if (fired.length >= maxTimers) throw new TimerStormError(fired.length, next.id, Number(this.#monoNs / NS_PER_MS));
      const step = next.deadlineNs > this.#monoNs ? next.deadlineNs - this.#monoNs : 0n;
      left -= step;
      const id = this.fireNext(Number((step + NS_PER_MS - 1n) / NS_PER_MS));
      if (id === undefined) break;
      fired.push(id);
    }
    this.#monoNs += left;
    this.#wallNs += left;
    return fired;
  }
}

const ZERO_SEED_REPLACEMENT = 0x9e37_79b9_7f4a_7c15n;
const MULTIPLIER = 0x2545_f491_4f6c_dd1dn;

/**
 * A deterministic `Rng`: xorshift64*, the same seed always yields the same bytes, and the same bytes as `SeededRng` in Rust, Swift
 * and Kotlin. Not cryptographically secure, on purpose. A seed of 0 is replaced by a fixed constant; `fill(n)` consumes
 * `ceil(n / 8)` outputs, little-endian, and drops the unused tail; a fill never returns more than 16 MiB.
 */
export class SeededRng implements RngAdapter {
  /** The most bytes one `fill` returns: 16 MiB. */
  static readonly MAX_FILL = 1 << 24;
  /** The seed of a generator made without one. */
  static readonly DEFAULT_SEED = 0x4b45_454c_5f52_4e47n;

  #state: bigint;

  constructor(seed: bigint | number = SeededRng.DEFAULT_SEED) {
    this.#state = SeededRng.#initial(BigInt(seed));
  }

  static #initial(seed: bigint): bigint {
    const s = seed & U64;
    return s === 0n ? ZERO_SEED_REPLACEMENT : s;
  }

  /** Restarts the sequence from `seed`. */
  reseed(seed: bigint | number): void {
    this.#state = SeededRng.#initial(BigInt(seed));
  }

  /** The next 64-bit output. */
  nextU64(): bigint {
    let x = this.#state;
    x ^= x >> 12n;
    x ^= (x << 25n) & U64;
    x ^= x >> 27n;
    this.#state = x;
    return (x * MULTIPLIER) & U64;
  }

  fill(out: Uint8Array): void {
    const len = Math.min(out.length, SeededRng.MAX_FILL);
    let at = 0;
    while (at < len) {
      let word = this.nextU64();
      for (let i = 0; i < 8 && at < len; i++) {
        out[at++] = Number(word & 0xffn);
        word >>= 8n;
      }
    }
  }

  /** `len` bytes from the sequence (what `Rng.fill(len)` answers). */
  bytes(len: number): Uint8Array {
    const out = new Uint8Array(Math.min(len, SeededRng.MAX_FILL));
    this.fill(out);
    return out;
  }
}

/** Decides whether a scripted reply applies to a request. */
export type Matcher = (request: HttpRequest) => boolean;

/** Constructors of {@link Matcher}s. */
export const matches = {
  /** Every request. */
  any: (): Matcher => () => true,
  /** Requests to exactly `url`. */
  url:
    (url: string): Matcher =>
    (r) =>
      r.url === url,
  /** Requests whose URL starts with `prefix`. */
  urlPrefix:
    (prefix: string): Matcher =>
    (r) =>
      r.url.startsWith(prefix),
  /** Requests with `method`. */
  method:
    (method: HttpMethod): Matcher =>
    (r) =>
      r.method === method,
  /** Requests both matchers match. */
  and:
    (a: Matcher, b: Matcher): Matcher =>
    (r) =>
      a(r) && b(r),
};

type Reply = HttpResponse | HttpError;
type Script =
  | { readonly kind: "fixed"; readonly reply: Reply }
  | { readonly kind: "sequence"; readonly replies: Reply[] }
  | { readonly kind: "handler"; readonly handler: (request: HttpRequest) => Reply | Promise<Reply> };

/** Builds a response. */
export function response(status: number, body: Uint8Array | string = new Uint8Array(0), headers: ReadonlyArray<readonly [string, string]> = []): HttpResponse {
  return {
    status,
    headers: headers.map(([name, value]) => ({ name, value })),
    body: typeof body === "string" ? new TextEncoder().encode(body) : body,
  };
}

/**
 * An `Http` adapter that answers from a script and remembers every request. Rules are tried in the order they were added and the
 * first that matches wins. A request nothing matches fails with `HttpError.Network` naming the request, and is still recorded.
 */
export class FakeHttp implements HttpAdapter {
  readonly #rules: Array<{ readonly matcher: Matcher; readonly script: Script }> = [];
  readonly #calls: HttpRequest[] = [];

  /** Answers every request `matcher` matches with `reply` (a response, or an `HttpError` to fail with). */
  respond(matcher: Matcher | string, reply: Reply): this {
    this.#rules.push({ matcher: typeof matcher === "string" ? matches.url(matcher) : matcher, script: { kind: "fixed", reply } });
    return this;
  }

  /** Answers the requests `matcher` matches with `replies`, one each, in order; once used up the rule no longer matches. */
  respondSequence(matcher: Matcher | string, replies: readonly Reply[]): this {
    this.#rules.push({ matcher: typeof matcher === "string" ? matches.url(matcher) : matcher, script: { kind: "sequence", replies: [...replies] } });
    return this;
  }

  /** Answers every request `matcher` matches by calling `handler`. */
  respondWith(matcher: Matcher | string, handler: (request: HttpRequest) => Reply | Promise<Reply>): this {
    this.#rules.push({ matcher: typeof matcher === "string" ? matches.url(matcher) : matcher, script: { kind: "handler", handler } });
    return this;
  }

  /** Every request received so far, oldest first (unmatched ones included). */
  get calls(): readonly HttpRequest[] {
    return this.#calls;
  }

  /** Forgets every rule and every recorded request. */
  reset(): void {
    this.#rules.length = 0;
    this.#calls.length = 0;
  }

  async request(req: HttpRequest): Promise<HttpResponse> {
    this.#calls.push(req);
    let handler: ((request: HttpRequest) => Reply | Promise<Reply>) | undefined;
    for (const rule of this.#rules) {
      if (!rule.matcher(req)) continue;
      const script = rule.script;
      if (script.kind === "fixed") return settle(script.reply);
      if (script.kind === "sequence") {
        const next = script.replies.shift();
        if (next !== undefined) return settle(next);
        continue;
      }
      handler = script.handler;
      break;
    }
    if (handler !== undefined) return settle(await handler(req));
    throw new HttpError.Network(`FakeHttp: no scripted response for ${req.method.toUpperCase()} ${req.url}`);
  }
}

function settle(reply: Reply): HttpResponse {
  if (reply instanceof HttpError) throw reply;
  return reply;
}

/** One operation a {@link MemKv} served through its port, for asserting on persistence behaviour. */
export type StoreOp =
  | { readonly op: "get"; readonly key: string }
  | { readonly op: "set"; readonly key: string }
  | { readonly op: "delete"; readonly key: string }
  | { readonly op: "list"; readonly prefix: string };

/** An in-memory `Kv` (and, as {@link MemSecureStore}, `SecureStore`): keys ordered by UTF-8 bytes, operations recorded. */
export class MemKv implements KvAdapter {
  readonly #map = new Map<string, Uint8Array>();
  readonly #ops: StoreOp[] = [];

  /** Puts `value` under `key` without recording an operation: seeds a test. */
  insert(key: string, value: Uint8Array): void {
    this.#map.set(key, value.slice());
  }

  /** The value under `key`, read directly (not an operation). */
  value(key: string): Uint8Array | undefined {
    return this.#map.get(key);
  }

  /** The keys, ascending. */
  keys(): string[] {
    return [...this.#map.keys()].sort(compareUtf8);
  }

  /** How many entries. */
  get size(): number {
    return this.#map.size;
  }

  /** The operations served through the port, oldest first. */
  get ops(): readonly StoreOp[] {
    return this.#ops;
  }

  async get(key: string): Promise<Uint8Array | null> {
    this.#ops.push({ op: "get", key });
    return this.#map.get(key)?.slice() ?? null;
  }

  async set(key: string, value: Uint8Array): Promise<void> {
    this.#ops.push({ op: "set", key });
    this.#map.set(key, value.slice());
  }

  async delete(key: string): Promise<void> {
    this.#ops.push({ op: "delete", key });
    this.#map.delete(key);
  }

  async list(prefix: string): Promise<string[]> {
    this.#ops.push({ op: "list", prefix });
    return this.keys().filter((k) => k.startsWith(prefix));
  }
}

/** The `SecureStore` fake: the same store under its own port id. */
export class MemSecureStore extends MemKv {}

/** Splits `path` into segments, dropping empty and `.` ones; `..` is `Denied`. */
function segments(path: string): string[] {
  const parts: string[] = [];
  for (const part of path.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") throw new FsError.Denied();
    parts.push(part);
  }
  return parts;
}

const emptyPath = (): FsError => new FsError.Io("the path is empty");

/**
 * An in-memory `Fs` with the semantics the platform adapters share: paths are `/`-separated, empty and `.` segments are ignored and
 * `..` is `Denied`; `write` creates missing directories and replaces a file; `read` of a missing path is `NotFound`, of a directory
 * `Io`; `delete` removes a file or a directory with everything under it; `list` answers the names directly inside a directory, ascending.
 */
export class MemFs implements FsAdapter {
  readonly #files = new Map<string, Uint8Array>();
  readonly #dirs = new Set<string>();

  /** Creates the file at `path` without going through the port: seeds a test. Fails like a `write` would. */
  seed(path: string, contents: Uint8Array): void {
    this.#write(path, contents);
  }

  /** The contents of the file at `path`, read directly. */
  contents(path: string): Uint8Array | undefined {
    try {
      return this.#files.get(segments(path).join("/"));
    } catch {
      return undefined;
    }
  }

  /** The path of every file, ascending. */
  filePaths(): string[] {
    return [...this.#files.keys()].sort(compareUtf8);
  }

  async read(path: string): Promise<Uint8Array> {
    const parts = segments(path);
    if (parts.length === 0) throw emptyPath();
    const key = parts.join("/");
    const file = this.#files.get(key);
    if (file !== undefined) return file.slice();
    if (this.#dirs.has(key)) throw new FsError.Io("is a directory");
    throw new FsError.NotFound();
  }

  async write(path: string, data: Uint8Array): Promise<void> {
    this.#write(path, data);
  }

  #write(path: string, data: Uint8Array): void {
    const parts = segments(path);
    if (parts.length === 0) throw emptyPath();
    const key = parts.join("/");
    if (this.#dirs.has(key)) throw new FsError.Io("is a directory");
    const parents = parts.slice(0, -1);
    let prefix = "";
    for (const parent of parents) {
      prefix = prefix === "" ? parent : `${prefix}/${parent}`;
      if (this.#files.has(prefix)) throw new FsError.Io("not a directory");
    }
    prefix = "";
    for (const parent of parents) {
      prefix = prefix === "" ? parent : `${prefix}/${parent}`;
      this.#dirs.add(prefix);
    }
    this.#files.set(key, data.slice());
  }

  async delete(path: string): Promise<void> {
    const parts = segments(path);
    if (parts.length === 0) throw emptyPath();
    const key = parts.join("/");
    if (this.#files.delete(key)) return;
    if (!this.#dirs.delete(key)) throw new FsError.NotFound();
    const below = `${key}/`;
    for (const file of [...this.#files.keys()]) if (file.startsWith(below)) this.#files.delete(file);
    for (const dir of [...this.#dirs]) if (dir.startsWith(below)) this.#dirs.delete(dir);
  }

  async list(dir: string): Promise<string[]> {
    const parts = segments(dir);
    const key = parts.join("/");
    if (parts.length > 0) {
      if (this.#files.has(key)) throw new FsError.Io("not a directory");
      if (!this.#dirs.has(key)) throw new FsError.NotFound();
    }
    const prefix = parts.length === 0 ? "" : `${key}/`;
    const names = new Set<string>();
    for (const path of [...this.#files.keys(), ...this.#dirs]) {
      if (!path.startsWith(prefix)) continue;
      const name = path.slice(prefix.length).split("/")[0];
      if (name !== undefined && name !== "") names.add(name);
    }
    return [...names].sort(compareUtf8);
  }
}

/** One record the core logged. */
export interface LogEntry {
  readonly level: number;
  readonly target: string;
  readonly message: string;
}

/** A `Log` that keeps every record. */
export class CaptureLog implements LogAdapter {
  readonly #entries: LogEntry[] = [];

  log(level: number, target: string, message: string): void {
    this.#entries.push({ level, target, message });
  }

  /** The records so far. */
  get entries(): readonly LogEntry[] {
    return this.#entries;
  }

  /** The messages so far. */
  messages(): string[] {
    return this.#entries.map((e) => e.message);
  }

  /** Whether any message contains `needle`. */
  contains(needle: string): boolean {
    return this.#entries.some((e) => e.message.includes(needle));
  }

  /** Forgets the records. */
  clear(): void {
    this.#entries.length = 0;
  }
}

/**
 * A `Diagnostics` that keeps every panic report it is told (ADR-046): the fake of the port a native core calls once per
 * panic it contained, and what a wasm core's trap report goes to. Pass `diagnostics.onPanic` as `LoadOptions.onPanic`
 * (`PreviewCore` does), or register {@link CaptureDiagnostics.panicked} behind the `Diagnostics` port of a core of your own.
 * The same as `undra::ports::fakes::CaptureDiagnostics` of the Rust kit.
 *
 * ```ts
 * const fakes = createFakes();
 * // ... a panicking call ...
 * expect(fakes.diagnostics.last?.operation).toBe("Todos.add");
 * ```
 */
export class CaptureDiagnostics {
  readonly #reports: UndraPanicReport[] = [];

  /** `Diagnostics.panicked`: keeps the report. */
  panicked(report: UndraPanicReport): void {
    this.#reports.push(report);
  }

  /** `LoadOptions.onPanic`, bound to this fake: `UndraCore.load({ onPanic: fakes.diagnostics.onPanic })`. */
  readonly onPanic = (report: UndraPanicReport): void => {
    this.panicked(report);
  };

  /** Every report so far, oldest first. */
  get reports(): readonly UndraPanicReport[] {
    return this.#reports;
  }

  /** How many reports there are. */
  get length(): number {
    return this.#reports.length;
  }

  /** The most recent report, or `undefined`. */
  get last(): UndraPanicReport | undefined {
    return this.#reports[this.#reports.length - 1];
  }

  /** Removes and returns every report so far. */
  take(): UndraPanicReport[] {
    return this.#reports.splice(0);
  }

  /** Forgets every report. */
  clear(): void {
    this.#reports.length = 0;
  }
}

/** A source of connectivity changes a test or a preview drives: `set` reports the new state to every subscriber. */
export class ScriptedConnectivity implements ConnectivityAdapter {
  #online = true;
  #kind: NetKind = "wifi";
  readonly #subscribers = new Set<(online: boolean, kind: NetKind) => void>();

  /** The state the app is in. */
  get current(): { readonly online: boolean; readonly kind: NetKind } {
    return { online: this.#online, kind: this.#kind };
  }

  /** Changes the state and tells the subscribers (the core, once loaded). */
  set(online: boolean, kind: NetKind): void {
    this.#online = online;
    this.#kind = kind;
    for (const emit of [...this.#subscribers]) emit(online, kind);
  }

  /** Goes offline (`kind` none). */
  goOffline(): void {
    this.set(false, "none");
  }

  /** Comes back online on `kind`. */
  goOnline(kind: NetKind = "wifi"): void {
    this.set(true, kind);
  }

  subscribe(emit: (online: boolean, kind: NetKind) => void): () => void {
    this.#subscribers.add(emit);
    queueMicrotask(() => {
      if (this.#subscribers.has(emit)) emit(this.#online, this.#kind);
    });
    return () => {
      this.#subscribers.delete(emit);
    };
  }
}

/** A source of lifecycle changes a test or a preview drives. */
export class ScriptedLifecycle implements LifecycleAdapter {
  #state: AppState = "active";
  readonly #subscribers = new Set<(state: AppState) => void>();

  /** The state the app is in. */
  get current(): AppState {
    return this.#state;
  }

  /** Changes the state and tells the subscribers. */
  set(state: AppState): void {
    this.#state = state;
    for (const emit of [...this.#subscribers]) emit(state);
  }

  subscribe(emit: (state: AppState) => void): () => void {
    this.#subscribers.add(emit);
    queueMicrotask(() => {
      if (this.#subscribers.has(emit)) emit(this.#state);
    });
    return () => {
      this.#subscribers.delete(emit);
    };
  }
}

/** One of each fake. */
export interface Fakes {
  readonly clock: FakeClock;
  readonly rng: SeededRng;
  readonly log: CaptureLog;
  readonly diagnostics: CaptureDiagnostics;
  readonly http: FakeHttp;
  readonly kv: MemKv;
  readonly secureStore: MemSecureStore;
  readonly fs: MemFs;
  readonly connectivity: ScriptedConnectivity;
  readonly lifecycle: ScriptedLifecycle;
}

/** Fresh fakes in the documented default state: the default time and seed, empty stores, no scripted replies, online on Wi-Fi, active. */
export function createFakes(): Fakes {
  return {
    clock: new FakeClock(),
    rng: new SeededRng(),
    log: new CaptureLog(),
    diagnostics: new CaptureDiagnostics(),
    http: new FakeHttp(),
    kv: new MemKv(),
    secureStore: new MemSecureStore(),
    fs: new MemFs(),
    connectivity: new ScriptedConnectivity(),
    lifecycle: new ScriptedLifecycle(),
  };
}
