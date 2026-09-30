import type { Counter } from "@playground/core";
import { useSignal } from "../useSignal";

/**
 * The `Counter` store. Each command is one transaction in the core, so `count`, `changes` and the
 * computed `parity` reach this view in one change-set, and never disagree on screen.
 */
export function CounterView({ counter }: { readonly counter: Counter }) {
  const count = useSignal(counter.count);
  const changes = useSignal(counter.changes);
  const parity = useSignal(counter.parity);

  return (
    <>
      <h2>Counter</h2>
      <p className="counter-value" data-testid="counter-value" aria-live="polite">
        {count}
      </p>
      <p className="counter-meta">
        <span className="badge" data-testid="counter-parity">
          {parity}
        </span>{" "}
        <span data-testid="counter-changes">
          {changes} {changes === 1 ? "change" : "changes"}
        </span>
      </p>
      <div className="row center">
        <button className="big" aria-label="Decrement" data-testid="counter-dec" onClick={() => void counter.decrement()}>
          −
        </button>
        <button className="big primary" aria-label="Increment" data-testid="counter-inc" onClick={() => void counter.increment()}>
          +
        </button>
        <button data-testid="counter-reset" onClick={() => void counter.reset()}>
          Reset
        </button>
      </div>
      <p className="note">
        <code>count</code> and <code>changes</code> are written in one transaction and <code>parity</code> is computed from <code>count</code> in
        the core: one change-set crosses the boundary for all three.
      </p>
    </>
  );
}
