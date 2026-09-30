import type { LogAdapter } from "@keel/runtime";

/** One record the core (or the runtime) logged. */
export interface LogRecord {
  /** 0 trace, 1 debug, 2 info, 3 warn, 4 error, 5 fatal. */
  readonly level: number;
  /** The module the record came from, such as `keel::panic`. */
  readonly target: string;
  /** The text of the record. */
  readonly message: string;
}

/** The `Log` port of the contract tests: keeps every record so a scenario can look at what was logged. */
export class CapturingLog implements LogAdapter {
  /** Every record received so far, oldest first. */
  readonly records: LogRecord[] = [];

  log(level: number, target: string, message: string): void {
    this.records.push({ level, target, message });
  }

  /** The records at `minLevel` or above, optionally narrowed to a target and to a text the message contains. */
  find(filter: { readonly minLevel?: number; readonly target?: string; readonly contains?: string } = {}): LogRecord[] {
    return this.records.filter(
      (record) =>
        record.level >= (filter.minLevel ?? 0) &&
        (filter.target === undefined || record.target === filter.target) &&
        (filter.contains === undefined || record.message.includes(filter.contains)),
    );
  }
}
