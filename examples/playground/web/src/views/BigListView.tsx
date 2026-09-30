import { type BigList, ListError } from "@playground/core";
import { useEffect, useRef, useState } from "react";
import { useSignal } from "../useSignal";

/** Every row is this tall, so the position of a row is arithmetic and only the visible ones need to exist. */
const ROW_HEIGHT = 36;
/** How many rows the viewport shows. */
const VIEWPORT_ROWS = 10;
/** Rows drawn above and below the viewport, so scrolling never shows a gap. */
const OVERSCAN = 5;
/** How often the stream updates a row: ten times a second. */
const STREAM_INTERVAL_MS = 100;

/**
 * The `BigList` store: 10,000 keyed rows, drawn as a window of a dozen. Every button is a
 * one-row operation, which reaches this view as a keyed patch of a single operation (the core
 * never sends the list again), and the rows are keyed by `id`, so React moves and updates the
 * few nodes that changed.
 */
export function BigListView({ bigList }: { readonly bigList: BigList }) {
  const items = useSignal(bigList.items);
  const count = useSignal(bigList.count);
  const [scrollTop, setScrollTop] = useState(0);
  const [streaming, setStreaming] = useState(false);
  const [readout, setReadout] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);

  const firstVisible = Math.min(Math.floor(scrollTop / ROW_HEIGHT), Math.max(0, items.length - 1));
  const from = Math.max(0, firstVisible - OVERSCAN);
  const to = Math.min(items.length, firstVisible + VIEWPORT_ROWS + OVERSCAN);

  // What the stream needs to pick a visible row without being a dependency of its timer.
  const visibleWindow = useRef({ firstVisible, total: items.length });
  useEffect(() => {
    visibleWindow.current = { firstVisible, total: items.length };
  });

  useEffect(() => {
    if (!streaming) return;
    let tick = 0;
    const timer = setInterval(() => {
      const { firstVisible: first, total } = visibleWindow.current;
      if (total === 0) return;
      const index = first + Math.floor(Math.random() * Math.min(VIEWPORT_ROWS, total - first));
      bigList.updateAt(index, `Streamed update ${++tick}`).catch(() => {
        // The list shrank under the timer (a remove or a reset): the next tick picks another row.
      });
    }, STREAM_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [streaming, bigList]);

  /** Runs one operation, and shows how long the round trip took: the call, the core, the change-set and its application. */
  const operate = async (name: string, run: () => Promise<unknown>): Promise<void> => {
    const started = performance.now();
    try {
      await run();
      setReadout(`${name}: ${(performance.now() - started).toFixed(2)} ms`);
      setProblem(null);
    } catch (error) {
      setProblem(error instanceof ListError ? error.message : String(error));
    }
  };

  const middle = Math.floor(items.length / 2);

  return (
    <>
      <h2>
        10k list{" "}
        <span className="badge" data-testid="biglist-count">
          {count}
        </span>
      </h2>
      <div className="row wrap">
        <button data-testid="biglist-insert-top" onClick={() => void operate("insert at top", () => bigList.insertAt(0, "Inserted at the top"))}>
          Insert top
        </button>
        <button
          data-testid="biglist-insert-middle"
          onClick={() => void operate("insert in the middle", () => bigList.insertAt(middle, "Inserted in the middle"))}
        >
          Insert middle
        </button>
        <button
          data-testid="biglist-update"
          onClick={() => void operate("update", () => bigList.updateAt(firstVisible, `Updated at ${new Date().toLocaleTimeString()}`))}
        >
          Update
        </button>
        <button data-testid="biglist-move" onClick={() => void operate("move to the middle", () => bigList.moveItem(firstVisible, middle))}>
          Move
        </button>
        <button data-testid="biglist-remove" onClick={() => void operate("remove", () => bigList.removeAt(firstVisible))}>
          Remove
        </button>
        <button data-testid="biglist-reset" onClick={() => void operate("reset", () => bigList.reset())}>
          Reset
        </button>
        <label className="switch">
          <input type="checkbox" role="switch" checked={streaming} data-testid="biglist-stream" onChange={(event) => setStreaming(event.target.checked)} />
          Stream updates
        </label>
      </div>
      <p className="note">
        Update, Move and Remove act on the first visible row. The stream updates a random visible row ten times a second.
      </p>
      {problem !== null && (
        <p className="error" role="alert">
          {problem}
        </p>
      )}
      <div
        className="viewport"
        style={{ height: VIEWPORT_ROWS * ROW_HEIGHT }}
        data-testid="biglist-viewport"
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
      >
        <div className="spacer-rows" style={{ height: items.length * ROW_HEIGHT }}>
          {items.slice(from, to).map((item, offset) => (
            <div key={item.id} className="big-row" style={{ top: (from + offset) * ROW_HEIGHT, height: ROW_HEIGHT }} data-testid="biglist-row" data-index={from + offset}>
              <span className="row-id">#{item.id}</span>
              <span className="row-label">{item.label}</span>
              <span key={item.version} className={item.version > 0 ? "row-version flash" : "row-version"}>
                v{item.version}
              </span>
            </div>
          ))}
        </div>
      </div>
      <p className="note" data-testid="biglist-window">
        Drawing rows {items.length === 0 ? 0 : from + 1} to {to} of {items.length.toLocaleString()}.{" "}
        <span data-testid="biglist-timing">{readout ?? "Try a button."}</span>
      </p>
    </>
  );
}
