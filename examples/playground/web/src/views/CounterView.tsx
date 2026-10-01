import { useUndra, useSignal } from "@undra/runtime/react";
import { Counter } from "@playground/core";

/**
 * The `Counter` store. Each command is one transaction in the core, so `count`, `changes` and the
 * computed `parity` reach this view in one change-set, and never disagree on screen.
 *
 * This view owns its counter: `useUndra(Counter)` creates the store when the tab opens and closes it
 * (releasing its handle in the core) when the tab closes, so the count starts from zero each time.
 * The other views share one store that lives as long as the page.
 */
export function CounterView() {
  const counter = useUndra(Counter);
  // Called before the store exists: `undefined` for a missing signal, the value once it does.
  const count = useSignal(counter?.count);
  const changes = useSignal(counter?.changes);
  const parity = useSignal(counter?.parity);

  if (counter === undefined) {
    return <h2>Counter</h2>;
  }

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
