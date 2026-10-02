import { UndraCallError } from "@undra/runtime";
import { useLazyList, useSignal, useUndra } from "@undra/runtime/react";
import { Library, ListError } from "@playground/core";
import { useState } from "react";
import { ROW_HEIGHT, VIEWPORT_ROWS, rowWindow } from "../paging";

/** How many rows of the lazy view the panel lists at most. */
const EVENS_SHOWN = 12;

/**
 * The `Library` store (ADR-043): `books` is a `Lazy<Item>` of 10,000 rows that never crosses the boundary whole. The page only
 * learns the length and a version; `useLazyList` gives it the length and a `getItem(index)` that returns the row, or `undefined`
 * while its page loads (reading it is what asks the core for the page, and for the pages around it). The scroll area draws a window
 * of rows by index and a placeholder for each one that has not arrived. A change (add, rename, remove) is one 12-byte notice, and
 * the page asks again only for the pages it has read since the last one.
 *
 * `evens` is a read-only lazy view of the even rows of `source`, a small list the page does receive; the core pages it through a
 * derived index, so the two buttons below it change `source` and the view follows.
 *
 * This view owns its store: `useUndra(Library)` creates it when the tab opens and closes it (releasing its handle) when it closes.
 */
export function LibraryView() {
  const library = useUndra(Library);
  const books = useLazyList(library?.books);
  const evens = useLazyList(library?.evens);
  const source = useSignal(library?.source);
  const [scrollTop, setScrollTop] = useState(0);
  const [problem, setProblem] = useState<string | null>(null);

  if (library === undefined) {
    return <h2>Library</h2>;
  }

  const { firstVisible, from, to } = rowWindow(scrollTop, books.length);

  /** Runs one operation; a refusal (an index the list no longer has) is shown, never thrown into a click handler. */
  const operate = (run: () => Promise<unknown>): void => {
    run().then(
      () => setProblem(null),
      (failure: unknown) => setProblem(failure instanceof ListError || failure instanceof UndraCallError ? failure.message : String(failure)),
    );
  };

  const rows = [];
  for (let index = from; index < to; index++) {
    const book = books.getItem(index);
    const top = index * ROW_HEIGHT;
    rows.push(
      book === undefined ? (
        <div key={index} className="big-row placeholder" style={{ top, height: ROW_HEIGHT }} data-testid="library-placeholder" data-index={index} aria-busy="true">
          <span className="row-id">#…</span>
          <span className="row-label">loading…</span>
        </div>
      ) : (
        <div key={index} className="big-row" style={{ top, height: ROW_HEIGHT }} data-testid="library-row" data-index={index}>
          <span className="row-id">#{book.id}</span>
          <span className="row-label">{book.label}</span>
          <span key={book.version} className={book.version > 0 ? "row-version flash" : "row-version"}>
            v{book.version}
          </span>
        </div>
      ),
    );
  }

  return (
    <>
      <h2>
        Library{" "}
        <span className="badge" data-testid="library-count">
          {books.length}
        </span>
        <span className="badge" data-testid="library-version" title="the version of the list the page has seen">
          v{library.books.version.toString()}
        </span>
      </h2>
      <div className="row wrap">
        <button data-testid="library-add" onClick={() => operate(() => library.addRows(100))}>
          Add 100 rows
        </button>
        <button data-testid="library-rename" onClick={() => operate(() => library.rename(firstVisible, `Renamed at ${new Date().toLocaleTimeString()}`))}>
          Rename the first visible
        </button>
        <button data-testid="library-remove" onClick={() => operate(() => library.removeAt(firstVisible))}>
          Remove the first visible
        </button>
        <button data-testid="library-reset" onClick={() => operate(() => library.reset(10_000))}>
          Reset to 10,000
        </button>
      </div>
      <p className="note">
        The core owns the rows. This page holds the pages it has read: reading a row that is not here asks for its page (and the pages around it),
        and a row that has not arrived is a placeholder.
      </p>
      {problem !== null && (
        <p className="error" role="alert" data-testid="library-error">
          {problem}
        </p>
      )}
      <div
        className="viewport"
        style={{ height: VIEWPORT_ROWS * ROW_HEIGHT }}
        data-testid="library-viewport"
        onScroll={(event) => setScrollTop(event.currentTarget.scrollTop)}
      >
        <div className="spacer-rows" style={{ height: books.length * ROW_HEIGHT }}>
          {rows}
        </div>
      </div>
      <p className="note" data-testid="library-window">
        Drawing rows {books.length === 0 ? 0 : from + 1} to {to} of {books.length.toLocaleString()}.
      </p>

      <h3>
        Even rows of the source{" "}
        <span className="badge" data-testid="evens-count">
          {evens.length}
        </span>
      </h3>
      <div className="row wrap">
        <button data-testid="evens-add" onClick={() => operate(() => library.addRows(2))}>
          Add 2 rows
        </button>
        <button data-testid="evens-drop" onClick={() => operate(() => library.dropSource(5))}>
          Drop 5 from the source
        </button>
        <span className="muted" data-testid="evens-source">
          source: {source?.length ?? 0} rows
        </span>
      </div>
      <ol className="evens" data-testid="evens-list">
        {Array.from({ length: Math.min(evens.length, EVENS_SHOWN) }, (_, index) => {
          const row = evens.getItem(index);
          return (
            <li key={index} data-testid="evens-row">
              {row === undefined ? "loading…" : `#${row.id} ${row.label}`}
            </li>
          );
        })}
      </ol>
      {evens.length > EVENS_SHOWN && <p className="note">… and {evens.length - EVENS_SHOWN} more.</p>}
    </>
  );
}
