import { Signal, type UndraPanicReport } from "@undra/runtime";

/** How many frames of a report the debug panel lists. */
const SHOWN_FRAMES = 4;

/**
 * The last panic report of this page's core (ADR-046 decision 4), for the debug panel: `LoadOptions.onPanic` hands every
 * contained panic here, and the page would hand the same value to its crash reporter (Sentry, `reportError`, an endpoint of its
 * own). A wasm core traps on a panic, so the report is built by the runtime from the core's FATAL record and the trap's stack,
 * before the restart of ADR-049.
 */
export class PanicLog {
  /** The newest report, or `null` before the first panic. */
  readonly last = new Signal<UndraPanicReport | null>(null);

  /** How many panics this page has seen. */
  count = 0;

  /** Records a report; `LoadOptions.onPanic` calls it. */
  record(report: UndraPanicReport): void {
    this.count += 1;
    this.last._set(report);
  }
}

/** The report as the lines the debug panel shows (and a crash reporter would send): what panicked, where, and in which build. */
export function describePanic(report: UndraPanicReport): string[] {
  const where = report.location === "" ? "no location" : report.location;
  const running = report.operation === "" ? "nothing running" : `in ${report.operation}`;
  const core = [report.namespace === "" ? "unnamed core" : report.namespace, report.coreVersion === "" ? "" : report.coreVersion].filter((s) => s !== "").join(" ");
  const image = report.imageId === "" ? "image not hashed" : `image ${report.imageId.slice(0, 12)}…`;
  const frames = report.frames.slice(0, SHOWN_FRAMES).map((f) => `  0x${f.address.toString(16)} ${f.symbol ?? "<unknown>"}${f.file === null ? "" : ` ${f.file}:${f.line ?? "?"}`}`);
  const more = report.frames.length > SHOWN_FRAMES ? [`  … ${report.frames.length - SHOWN_FRAMES} more`] : [];
  return [
    report.message,
    `at ${where}, ${running}, thread ${report.thread}`,
    `${core}, schema 0x${report.schemaHash.toString(16)}, ${image}`,
    report.frames.length === 0 ? "no frames" : `${report.frames.length} frame${report.frames.length === 1 ? "" : "s"}:`,
    ...frames,
    ...more,
  ];
}
