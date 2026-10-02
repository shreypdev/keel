import { Signal } from "@undra/runtime";
import type { Reporter } from "@playground/core";

/*
 * The Workshop tab's app side: the `Reporter` the page implements (ADR-041). The core calls it back while a job
 * runs; the runtime delivers each call through the mirror's drain, after the store changes the core made before it,
 * and never inside the core's own callback, so this code may call into the core. Kept apart from the view so that it
 * is tested in Node.
 */

/** How many notes the log keeps. */
const KEPT = 8;

/** The question a job asked and waits on. */
export interface Question {
  /** What the core asked. */
  readonly text: string;
  /** Answers it: `true` goes on, `false` declines. */
  readonly answer: (yes: boolean) => void;
}

/** What a job reported, as signals the Workshop view renders. */
export class JobLog implements Reporter {
  /** The newest progress report (`progress` is `#[undra(coalesce)]`: a burst arrives as its newest report). */
  readonly step = new Signal<{ readonly done: number; readonly total: number } | null>(null);
  /** The newest notes, oldest first. */
  readonly lines = new Signal<readonly string[]>([]);
  /** The question the core is waiting on, if any. */
  readonly asking = new Signal<Question | null>(null);

  progress(done: number, total: number): void {
    this.step._set({ done, total });
  }

  note(line: string): void {
    this.lines._set([...this.lines.peek(), line].slice(-KEPT));
  }

  /** Asks the person; the job waits for the answer, or is cancelled (the signal aborts and the question goes away). */
  confirm(question: string, signal: AbortSignal): Promise<boolean> {
    return new Promise<boolean>((resolve, reject) => {
      const done = (): void => {
        if (this.asking.peek() === asked) this.asking._set(null);
      };
      const asked: Question = {
        text: question,
        answer: (yes) => {
          done();
          resolve(yes);
        },
      };
      signal.addEventListener("abort", () => {
        done();
        reject(signal.reason);
      });
      this.asking._set(asked);
    });
  }

  /** Forgets what an earlier job reported. */
  clear(): void {
    this.step._set(null);
    this.lines._set([]);
  }
}

/** One line about a progress report, for the view. */
export function describeStep(step: { readonly done: number; readonly total: number } | null): string {
  return step === null ? "no job yet" : `step ${step.done} of ${step.total}`;
}
