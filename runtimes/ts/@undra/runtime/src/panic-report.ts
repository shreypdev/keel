import type { UndraPanicFrame, UndraPanicReport } from "./adapters/types.js";
import { trapStack } from "./panic.js";
import type { WasmSource } from "./transport/wasm-main.js";

/*
 * The panic report of a wasm core that trapped (ADR-046 decision 4.4). A wasm core cannot call the `Diagnostics` port out of
 * a panic (`panic=abort` compiles it to a trap), so the runtime builds the same `UndraPanicReport` from what is left: the
 * core's FATAL `undra::panic` log record, which it writes before it traps, and the JavaScript stack of the trap.
 *
 * This module is reached only by a page that asked for reports (`LoadOptions.onPanic` or `recovery`): `UndraCore` imports it
 * when the core loads, so that the report, and the SHA-256 of the module, are ready by the time a trap needs them. It is
 * not part of the package's public surface (`index.ts` does not export it): that is what lets a bundler keep it out of the
 * chunk every page loads.
 */

/** What the runtime knows about the core besides the trap: the fields of the report that are not in the panic record. */
export interface PanicContext {
  /** Where the core ran: `"main"` (the page's thread) or `"worker"`. */
  readonly thread: string;
  /** `AttachOptions.namespace`, or `""`. */
  readonly namespace: string;
  /** `LoadOptions.coreVersion`, or `""`. */
  readonly coreVersion: string;
  /** The core's schema hash. */
  readonly schemaHash: bigint;
  /** {@link wasmImageId} of the module, or `""` while it is not ready or cannot be had. */
  readonly imageId: string;
}

/** The parts of a core's FATAL `undra::panic` record. */
export interface PanicRecord {
  readonly message: string;
  readonly location: string;
  readonly operation: string;
}

/**
 * Reads the FATAL record of a panic: the message, then, on lines of their own, `    at <file>:<line>:<col>` and (when a
 * call or a task was running) `    in <operation>`, in that order and with four leading spaces. The message may hold
 * newlines itself, so the two trailer lines are read from the end. A panic before `undra_init` writes the older single
 * line, `<message> at <file>:<line>`.
 */
export function parsePanicRecord(record: string): PanicRecord {
  const lines = record.split("\n");
  let end = lines.length;
  let operation = "";
  let location = "";
  const last = (): string => lines[end - 1] as string;
  if (end > 1 && last().startsWith("    in ")) operation = lines[--end]!.slice(7);
  if (end > 1 && last().startsWith("    at ")) location = lines[--end]!.slice(7);
  let message = lines.slice(0, end).join("\n");
  if (location === "") {
    const legacy = /^(.*) at (\S+:\d+(?::\d+)?)$/s.exec(message);
    if (legacy !== null) [, message, location] = legacy as unknown as [string, string, string];
  }
  return { message, location, operation };
}

/** One line of a trap's stack as a frame, or `null` when it is not a frame of the wasm module. */
function frameOf(line: string): UndraPanicFrame | null {
  // V8 `at name (wasm://wasm/hash:wasm-function[12]:0x34a)`, Firefox and Safari `name@url:wasm-function[12]:0x34a` (Safari
  // sometimes without the offset: `name@[wasm code]`); and what a source-map-aware `Error.prepareStackTrace` makes of V8's
  // frame, `at null.<anonymous> (wasm://wasm/hash:1:843)`: the offset plus one, no function index.
  const at = /wasm-function\[(\d+)\]:0x([0-9a-f]+)/i.exec(line);
  const mapped = at === null ? /wasm:\/\/wasm\/\w+:1:(\d+)/.exec(line) : null;
  const bare = at === null && mapped === null ? /wasm-function\[(\d+)\]/i.exec(line) : null;
  const found = at ?? mapped ?? bare;
  if (found === null) return null;
  // The name is what comes before the location; a module without a names section has none (or the index again).
  const name = /^\s*(?:at\s+)?(.+?)(?:\s\(|@)/.exec(line.slice(0, found.index))?.[1];
  const index = at?.[1] ?? bare?.[1];
  return {
    address: at !== null ? BigInt(`0x${at[2]}`) : mapped !== null ? BigInt(mapped[1] as string) - 1n : 0n,
    symbol: name !== undefined && name !== "null.<anonymous>" ? name : index === undefined ? null : `wasm-function[${index}]`,
    file: null,
    line: null,
  };
}

/** The frames of the module in a trap's stack, innermost first. */
export function trapFrames(stack: string): UndraPanicFrame[] {
  const frames: UndraPanicFrame[] = [];
  for (const line of stack.split("\n")) {
    const frame = frameOf(line);
    if (frame !== null) frames.push(frame);
  }
  return frames;
}

/**
 * The report of a trap: `record` is the message of the last FATAL `undra::panic` record the core logged (`null` when it
 * logged none: a stack overflow, out of memory, a module that is broken), `trap` the error the transport raised.
 */
export function trapReport(record: string | null, trap: Error, context: PanicContext): UndraPanicReport {
  const cause = trap.cause;
  const text = cause instanceof Error ? `${cause.name}: ${cause.message}` : trap.message;
  return { ...(record === null ? { message: text, location: "", operation: "" } : parsePanicRecord(record)), frames: trapFrames(trapStack(trap)), ...context };
}

/**
 * The SHA-256 of the module's bytes as lowercase hex (the report's `imageId`: which build the offsets belong to), `""` when it
 * cannot be had: no WebCrypto, a module given already compiled (its bytes are gone), a download that fails. A URL is fetched
 * again, from the browser's cache where the page just loaded it from.
 */
export async function wasmImageId(wasm: WasmSource): Promise<string> {
  const subtle = (globalThis as { crypto?: { subtle?: SubtleCrypto } }).crypto?.subtle;
  if (subtle === undefined) return "";
  let bytes: BufferSource;
  if (wasm instanceof URL) {
    const response = await fetch(wasm, { cache: "force-cache" });
    if (!response.ok) return "";
    bytes = await response.arrayBuffer();
  } else if (wasm instanceof WebAssembly.Module) {
    return "";
  } else {
    bytes = wasm;
  }
  return Array.from(new Uint8Array(await subtle.digest("SHA-256", bytes)), (b) => b.toString(16).padStart(2, "0")).join("");
}

/** What the report of a trap is made of, besides the trap itself: the options the core was loaded with that it reads. */
export interface PanicOptions {
  /** The schema hash of the bindings (the core's, once it has said hello: a different one is refused). */
  readonly expectedSchemaHash: bigint;
  /** `AttachOptions.namespace`: the report's `namespace`, `""` without one. */
  readonly namespace?: string | undefined;
  /** `AttachOptions.onPanic`: who is handed the report. */
  readonly onPanic?: ((report: UndraPanicReport) => void) | undefined;
  /** `LoadOptions.wasm`: hashed for the report's `imageId` when `onPanic` is set. */
  readonly wasm?: WasmSource | undefined;
  /** `LoadOptions.coreVersion`: the report's `coreVersion`, `""` without one. */
  readonly coreVersion?: string | undefined;
}

/** What {@link PanicSupport.start} needs of the core it reports for: `UndraCore` has both. */
export interface PanicHost {
  /** The transport's mode: `"wasm-worker"` or `"wasm-main"`, the report's `thread`. */
  readonly mode: string;
  /** Reports a failure of `onPanic` (it threw) to `onError` and the log, and carries on: `UndraCore.report`. */
  report(error: unknown, operation: string): void;
}

/** What reports the traps of one core. */
export interface PanicReporter {
  /**
   * Builds the report of `trap` (see {@link trapReport}; `record` is the core's last FATAL `undra::panic` message, if any) and hands
   * it to `onPanic` once, if the app set one. A handler that throws goes to {@link PanicHost.report} and changes nothing. Returns the
   * report, which `crashRecovery`'s `UndraCoreRestarted` carries.
   */
  trapped(record: string | null, trap: Error): UndraPanicReport;
}

/** Starts reporting the traps of `host` with `options`: hashes the module for `imageId` in the background when the app set `onPanic`. */
function start(host: PanicHost, options: PanicOptions): PanicReporter {
  let imageId = "";
  // Only for an app that asked for reports: the SHA-256 is the `imageId` of what it receives.
  if (options.onPanic !== undefined && options.wasm !== undefined) {
    wasmImageId(options.wasm).then(
      (id) => {
        imageId = id;
      },
      () => {},
    );
  }
  return {
    trapped(record, trap) {
      const report = trapReport(record, trap, {
        thread: host.mode === "wasm-worker" ? "worker" : "main",
        namespace: options.namespace ?? "",
        coreVersion: options.coreVersion ?? "",
        schemaHash: options.expectedSchemaHash,
        imageId,
      });
      try {
        options.onPanic?.(report);
      } catch (thrown) {
        host.report(thrown, "onPanic");
      }
      return report;
    },
  };
}

/** What `UndraCore` takes from this module, as one value: how `crashRecovery` carries it to the core without a dynamic import. */
export interface PanicSupport {
  /** Starts reporting the traps of a core. */
  readonly start: (host: PanicHost, options: PanicOptions) => PanicReporter;
}

/** {@link PanicSupport} of this module. */
export const panicSupport: PanicSupport = { start };
