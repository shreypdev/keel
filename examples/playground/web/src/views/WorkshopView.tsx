import { ReportError, type Shelf, Workshop } from "@playground/core";
import { UndraCallError } from "@undra/runtime";
import { useSignal, useUndra } from "@undra/runtime/react";
import { useEffect, useMemo, useState } from "react";
import { JobLog, describeStep } from "../workshop";

/**
 * The `Workshop` store and its children (ADR-040, ADR-041). `workshop.shelf(name)` returns a `Shelf`, itself a store:
 * the same wrapper every time for the same name, observed like any store. "Merge" passes two shelves back to the core
 * in one call, which moves the items in one transaction. "Run a job" lends the page's `Reporter` (a `JobLog`) to the
 * core, which calls it back with progress, notes and a question this view lets you answer.
 */
export function WorkshopView() {
  const workshop = useUndra(Workshop);
  const [shelves, setShelves] = useState<readonly [Shelf, Shelf] | null>(null);
  const log = useMemo(() => new JobLog(), []);
  const [running, setRunning] = useState<AbortController | null>(null);
  const [outcome, setOutcome] = useState<string | null>(null);

  useEffect(() => {
    if (workshop === undefined) return;
    let current = true;
    void Promise.all([workshop.shelf("left"), workshop.shelf("right")]).then(([left, right]) => {
      if (current) setShelves([left, right]);
    });
    return () => {
      current = false;
    };
  }, [workshop]);

  const left = useSignal(shelves?.[0].items);
  const right = useSignal(shelves?.[1].items);
  const jobs = useSignal(workshop?.jobs);
  const step = useSignal(log.step);
  const lines = useSignal(log.lines) ?? [];
  const asking = useSignal(log.asking);

  if (workshop === undefined || shelves === null) {
    return <h2>Workshop</h2>;
  }
  const [leftShelf, rightShelf] = shelves;

  const run = (): void => {
    const controller = new AbortController();
    log.clear();
    setOutcome(null);
    setRunning(controller);
    workshop.run(3, log, controller.signal).then(
      (steps) => setOutcome(`done: ${steps} steps`),
      (error: unknown) => {
        if (error instanceof ReportError.Declined) setOutcome("declined");
        else if (error instanceof UndraCallError) setOutcome(`failed: ${error.message}`);
        else setOutcome("cancelled");
      },
    ).finally(() => setRunning(null));
  };

  return (
    <>
      <h2>Workshop</h2>
      <div className="row wrap">
        <span data-testid="shelf-left">left: {left}</span>
        <span data-testid="shelf-right">right: {right}</span>
        <button data-testid="shelf-stock" onClick={() => void leftShelf.stock(1)}>
          Stock left +1
        </button>
        <button data-testid="shelf-merge" onClick={() => void workshop.merge(leftShelf, rightShelf)}>
          Merge left onto right
        </button>
      </div>
      <div className="row wrap">
        <button className="primary" data-testid="job-run" disabled={running !== null} onClick={run}>
          Run a job
        </button>
        <button data-testid="job-cancel" disabled={running === null} onClick={() => running?.abort()}>
          Cancel
        </button>
        <span data-testid="job-step">{describeStep(step ?? null)}</span>
        <span data-testid="job-count">
          {jobs} {jobs === 1 ? "job" : "jobs"} run
        </span>
      </div>
      {asking != null && (
        <div className="row wrap" data-testid="job-question">
          <span>{asking.text}</span>
          <button data-testid="job-yes" onClick={() => asking.answer(true)}>
            Yes
          </button>
          <button data-testid="job-no" onClick={() => asking.answer(false)}>
            No
          </button>
        </div>
      )}
      {outcome !== null && <p data-testid="job-outcome">{outcome}</p>}
      <ul data-testid="job-notes">
        {lines.map((line, i) => (
          <li key={i}>{line}</li>
        ))}
      </ul>
      <p className="note">
        The shelves are stores the workshop hands out; <code>merge</code> takes two of them as arguments. The job calls
        this page back through a <code>Reporter</code> it implements: every call arrives after the store changes made
        before it, never inside the core.
      </p>
    </>
  );
}
