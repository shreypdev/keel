import { UndraReader } from "../wire/index.js";

/*
 * The pull discipline both real-time bindings share (ADR-047 §3; the brief's "pump"): one *line*
 * per WebSocket connection or event stream, with a buffer the adapter's async iterable is read
 * into only while it holds fewer items than the core's latest `max` (16 before the first pull),
 * so an adapter whose iterable is lazy (Node's socket, a fetch body) stops reading the network
 * when the core stops pulling. At most one pull waits per line; the first error, or the end of the
 * iterable, is the line's terminal, answered to every later pull.
 *
 * A burst is answered as one reply, as every runtime does: a pull that holds fewer than its `max`
 * items while the line is still open waits until `max` are there, {@link QUIET_MS} pass with nothing
 * new, or {@link LINGER_MS} pass since its first item, so sixteen frames arriving one by one cross the
 * boundary once, not sixteen times. Internal to `@undra/runtime/realtime`.
 */

/** How many items a line reads ahead before the core's first pull (SPEC 3.7's initial grant). */
export const INITIAL_WINDOW = 16;

/** A pull with some items answers once nothing new arrived for this long (ms). */
export const QUIET_MS = 2;

/** A pull with some items answers at the latest this long after its first item (ms). */
export const LINGER_MS = 8;

/** The typed errors a {@link Lines} answers with, chosen by the binding that owns it. */
export interface LineErrors<E> {
  /** No line has this id (never opened, or forgotten long after it closed). */
  unknown(id: number): E;
  /** A pull arrived while another one was waiting. */
  pending(id: number): E;
  /** What the adapter's iterable threw, as the binding's error (a typed error passes through). */
  coerce(error: unknown): E;
  /** The iterable finished although the core did not close the line. */
  finished(): E;
}

interface Pull<T> {
  readonly max: number;
  resolve(items: T[]): void;
  reject(error: unknown): void;
  /** When the pull answers whatever it has (`LINGER_MS` after its first item). */
  deadline: number | null;
  /** The quiet timer of a pull that has items. */
  timer: ReturnType<typeof setTimeout> | undefined;
}

/** One connection or stream, as its binding sees it. */
export class Line<T, H, E> {
  /** What the binding keeps about the line (the adapter's connection). */
  readonly handle: H;
  /** Items read from the adapter that the core has not pulled yet. */
  readonly buffer: T[] = [];
  /** The `max` of the latest pull: how far the pump reads ahead. */
  window = INITIAL_WINDOW;
  /** How the line ended, once it did (and the core did not close it first). */
  terminal: E | null = null;
  /** Whether the core closed it. */
  closed = false;
  /** The pull that waits for an item. */
  pull: Pull<T> | null = null;
  /** Wakes the pump when the buffer has room again. */
  room: (() => void) | null = null;
  /** The adapter's iterator, once the pump started. */
  iterator: AsyncIterator<T> | null = null;

  constructor(handle: H) {
    this.handle = handle;
  }

  /** Wakes the pump if it waits for room. */
  wake(): void {
    const room = this.room;
    this.room = null;
    room?.();
  }
}

/** How many closed lines a binding remembers (to answer a late pull `[]` and a late send with its close). */
const REMEMBERED = 4096;

/**
 * The lines of one binding instance: ids from 1, never reused; a pump per line; the pull, close
 * and dispose rules of the brief.
 */
export class Lines<T, H, E> {
  readonly #errors: LineErrors<E>;
  readonly #lines = new Map<number, Line<T, H, E>>();
  /** Ids of closed lines, oldest first (bounded by {@link REMEMBERED}). */
  readonly #closed: number[] = [];
  #next = 1;

  constructor(errors: LineErrors<E>) {
    this.#errors = errors;
  }

  /** Registers a line for `handle`, starts reading `source` into it, and returns its id. */
  open(handle: H, source: () => AsyncIterable<T>): number {
    const id = this.#next++;
    const line = new Line<T, H, E>(handle);
    this.#lines.set(id, line);
    void this.#pump(line, source);
    return id;
  }

  /** The line `id`; throws the binding's "unknown" error when there is none. */
  get(id: number): Line<T, H, E> {
    const line = this.#lines.get(id);
    if (line === undefined) throw this.#errors.unknown(id);
    return line;
  }

  /** Every line the core has not closed. */
  live(): Array<[number, Line<T, H, E>]> {
    return [...this.#lines].filter(([, line]) => !line.closed);
  }

  /**
   * The core's pull: up to `max` buffered items at once; `[]` once the core closed the line; the
   * terminal error (sticky); else waits for the first item.
   */
  pull(id: number, max: number): Promise<T[]> | T[] {
    const line = this.get(id);
    if (line.closed) return [];
    if (line.pull !== null) throw this.#errors.pending(id);
    line.window = Math.max(1, max);
    const ended = line.terminal !== null || line.iterator === null;
    if (line.buffer.length >= line.window || (line.buffer.length > 0 && ended)) {
      const items = line.buffer.splice(0, line.window);
      line.wake();
      return items;
    }
    if (line.terminal !== null) throw line.terminal;
    line.wake();
    return new Promise<T[]>((resolve, reject) => {
      line.pull = { max: line.window, resolve, reject, deadline: null, timer: undefined };
      if (line.buffer.length > 0) this.#linger(line);
    });
  }

  /**
   * Marks `line` closed by the core: a waiting pull answers `[]`, the buffer is dropped, the pump
   * stops (and asks the adapter's iterator to `return`). Returns `false` when it was closed already.
   */
  close(id: number, line: Line<T, H, E>): boolean {
    if (line.closed) return false;
    line.closed = true;
    line.buffer.length = 0;
    const pull = line.pull;
    line.pull = null;
    if (pull !== null) {
      clearTimeout(pull.timer);
      pull.resolve([]);
    }
    line.wake();
    const iterator = line.iterator;
    line.iterator = null;
    if (iterator?.return !== undefined) {
      try {
        void Promise.resolve(iterator.return()).catch(() => {});
      } catch {
        // An iterator that cannot return has nothing to release.
      }
    }
    this.#closed.push(id);
    if (this.#closed.length > REMEMBERED) this.#lines.delete(this.#closed.shift() as number);
    return true;
  }

  /** Something arrived (an item, or the end): answers the waiting pull now, or lets it linger for more. */
  #deliver(line: Line<T, H, E>): void {
    const pull = line.pull;
    if (pull === null) return;
    if (line.buffer.length >= pull.max || line.terminal !== null || line.iterator === null) this.#answer(line);
    else if (line.buffer.length > 0) this.#linger(line);
  }

  /** (Re)starts the quiet timer of the waiting pull, never past its deadline. */
  #linger(line: Line<T, H, E>): void {
    const pull = line.pull;
    if (pull === null) return;
    const now = Date.now();
    pull.deadline ??= now + LINGER_MS;
    clearTimeout(pull.timer);
    pull.timer = setTimeout(() => this.#answer(line), Math.max(0, Math.min(QUIET_MS, pull.deadline - now)));
  }

  /** Answers the waiting pull with what is buffered (up to its `max`), else with the terminal. */
  #answer(line: Line<T, H, E>): void {
    const pull = line.pull;
    if (pull === null) return;
    if (line.buffer.length > 0) {
      clearTimeout(pull.timer);
      line.pull = null;
      const items = line.buffer.splice(0, pull.max);
      line.wake();
      pull.resolve(items);
    } else if (line.terminal !== null) {
      clearTimeout(pull.timer);
      line.pull = null;
      pull.reject(line.terminal);
    }
  }

  /** Reads the adapter's iterable into the line while the buffer is under the window. */
  async #pump(line: Line<T, H, E>, source: () => AsyncIterable<T>): Promise<void> {
    try {
      const iterator = source()[Symbol.asyncIterator]();
      if (line.closed) {
        void Promise.resolve(iterator.return?.()).catch(() => {});
        return;
      }
      line.iterator = iterator;
      for (;;) {
        while (!line.closed && line.buffer.length >= line.window) {
          await new Promise<void>((resolve) => {
            line.room = resolve;
          });
        }
        if (line.closed) return;
        const step = await iterator.next();
        if (line.closed) return;
        if (step.done === true) {
          line.terminal = this.#errors.finished();
          break;
        }
        line.buffer.push(step.value);
        this.#deliver(line);
      }
    } catch (error) {
      if (line.closed) return;
      line.terminal = this.#errors.coerce(error);
    }
    line.iterator = null;
    this.#deliver(line);
  }
}

/** Decodes a port call's arguments with `read`, requiring that all of them are consumed. */
export function readArgs<T>(args: Uint8Array, read: (r: UndraReader) => T): T {
  const r = new UndraReader(args);
  const value = read(r);
  r.finish();
  return value;
}
