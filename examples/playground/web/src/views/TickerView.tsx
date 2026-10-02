import { useSignal, useUndra } from "@undra/runtime/react";
import { TickError, TickerQueryHandle, setTickerFailing, tickerFetches } from "@playground/core";
import { useEffect, useState } from "react";
import { SLOW_POLL_MS, tickerLine } from "../paging";

/**
 * The `ticker` query (ADR-043): a counter the core bumps on every fetch, with `interval = "1s"`: while somebody watches it, the app is
 * active and the network is up, the core fetches it again a second after the last fetch ended. "Poll every 5 s" is this observer's
 * override (`setPollInterval`); the entry polls at the smallest interval among its observers, and clearing the override goes back to the
 * query's own second. "Fail on purpose" makes the next fetches fail: `error` is set, `data` keeps its last value, polling goes on, and
 * the first success clears the error. Closing the tab (the last observer) stops the polling.
 *
 * This view owns its handle (`useUndra`), and puts the failure switch back when it closes, since that flag lives in the core.
 */
export function TickerView() {
  const ticker = useUndra(() => TickerQueryHandle.create(), []);
  const data = useSignal(ticker?.data) ?? null;
  const status = useSignal(ticker?.status) ?? "idle";
  const fetching = useSignal(ticker?.fetching) ?? false;
  const error = useSignal(ticker?.error) ?? null;
  const updatedAt = useSignal(ticker?.updatedAt) ?? null;
  const [slow, setSlow] = useState(false);
  const [failing, setFailing] = useState(false);
  const [fetches, setFetches] = useState<number | null>(null);

  // The override belongs to this observer: set when the switch changes, cleared with the handle.
  useEffect(() => {
    if (ticker !== undefined) void ticker.setPollInterval(slow ? SLOW_POLL_MS : null);
  }, [ticker, slow]);

  // What the core counts (every fetch, successful or not), read again whenever the value or the error changes.
  useEffect(() => {
    if (ticker === undefined) return;
    let current = true;
    void tickerFetches().then((count) => {
      if (current) setFetches(count);
    });
    return () => {
      current = false;
    };
  }, [ticker, data, error]);

  useEffect(
    () => () => {
      void setTickerFailing(false);
    },
    [],
  );

  if (ticker === undefined) {
    return <h2>Ticker</h2>;
  }

  const fail = (next: boolean): void => {
    setFailing(next);
    void setTickerFailing(next);
  };

  return (
    <>
      <h2>Ticker</h2>
      <p className="counter-value" data-testid="ticker-value" aria-live="polite">
        {data ?? "–"}
      </p>
      <p className="counter-meta">
        <span className={`status status-${status}`} data-testid="ticker-status">
          {status}
        </span>{" "}
        {fetching && (
          <span className="fetching" data-testid="ticker-fetching" role="status">
            fetching…
          </span>
        )}
      </p>
      <p className="counter-meta" data-testid="ticker-line">
        {tickerLine({ value: data, slow })}
      </p>
      {error instanceof TickError && (
        <p className="error" role="alert" data-testid="ticker-error">
          {error.message}
        </p>
      )}
      <div className="row center">
        <button aria-pressed={slow} data-testid="ticker-slow" onClick={() => setSlow(!slow)}>
          Poll every {SLOW_POLL_MS / 1000} s
        </button>
        <button aria-pressed={failing} data-testid="ticker-fail" onClick={() => fail(!failing)}>
          Fail on purpose
        </button>
      </div>
      <p className="note" data-testid="ticker-meta">
        <span data-testid="ticker-fetches">{fetches === null ? "…" : `${fetches} ${fetches === 1 ? "fetch" : "fetches"} in the core`}</span>
        {updatedAt !== null && <> · updated {new Date(updatedAt).toLocaleTimeString()}</>}
      </p>
    </>
  );
}
