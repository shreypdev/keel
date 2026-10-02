import { useLoadMore, useSignal, useUndra } from "@undra/runtime/react";
import { FeedQueryHandle, touchFeed } from "@playground/core";
import { useState } from "react";
import { feedFooter } from "../paging";

/**
 * The `feed` query (ADR-043): an infinite query over the 10,000 rows of the big list, fifty to a page. `data` is a keyed list that
 * grows by the page: `fetchNextPage()` appends the next fifty as one keyed patch of inserts. `useLoadMore` turns a sentinel under the
 * last row into that call: when it scrolls into view, and there is a next page and none is loading, the next page is fetched, and so
 * on until the sentinel is pushed out of view or the pages run out. Refresh is `refetch()`, which fetches the loaded pages again in
 * order and patches only what changed; "Touch even rows" makes the core's even rows show a new version first, so there is a change to find.
 *
 * This view owns its handle: `useUndra(() => FeedQueryHandle.create(evenOnly), [evenOnly])` creates it when the tab opens, and
 * again (closing the previous) when "Even rows only" changes, since the parameter is part of the cache key.
 */
export function FeedView() {
  const [evenOnly, setEvenOnly] = useState(false);
  const [revision, setRevision] = useState(0);
  const feed = useUndra(() => FeedQueryHandle.create(evenOnly), [evenOnly]);
  const rows = useSignal(feed?.data) ?? [];
  const status = useSignal(feed?.status) ?? "idle";
  const fetching = useSignal(feed?.fetching) ?? false;
  const hasNextPage = useSignal(feed?.hasNextPage) ?? false;
  const fetchingNextPage = useSignal(feed?.fetchingNextPage) ?? false;
  const error = useSignal(feed?.error) ?? null;
  const sentinel = useLoadMore(feed, { rootMargin: "120px" });
  const footer = feedFooter({ loaded: rows.length, hasNextPage, fetchingNextPage });

  if (feed === undefined) {
    return <h2>Feed</h2>;
  }

  const touch = (): void => {
    const next = revision + 1;
    setRevision(next);
    void touchFeed(next).then(() => feed.refetch());
  };

  return (
    <>
      <h2>
        Feed{" "}
        <span className="badge" data-testid="feed-count">
          {rows.length}
        </span>
      </h2>
      <div className="row wrap status-line">
        <span className={`status status-${status}`} data-testid="feed-status">
          {status}
        </span>
        {fetching && (
          <span className="fetching" data-testid="feed-fetching" role="status">
            fetching…
          </span>
        )}
        <span className="spacer" />
        <label className="switch">
          <input type="checkbox" role="switch" checked={evenOnly} data-testid="feed-even-only" onChange={(event) => setEvenOnly(event.target.checked)} />
          Even rows only
        </label>
      </div>
      <div className="row wrap">
        <button data-testid="feed-refresh" disabled={fetching} onClick={() => void feed.refetch()}>
          Refresh
        </button>
        <button data-testid="feed-touch" disabled={fetching} onClick={touch}>
          Touch even rows
        </button>
        <span className="muted" data-testid="feed-revision">
          revision {revision}
        </span>
      </div>
      {error !== null && (
        <p className="error" role="alert" data-testid="feed-error">
          {error.message}
        </p>
      )}
      <div className="viewport feed-scroll" data-testid="feed-scroll">
        <ul className="feed-list">
          {rows.map((row) => (
            <li key={row.id} className="feed-row" data-testid="feed-row">
              <span className="row-id">#{row.id}</span>
              <span className="row-label">{row.label}</span>
              <span key={row.version} className={row.version > 0 ? "row-version flash" : "row-version"}>
                v{row.version}
              </span>
            </li>
          ))}
        </ul>
        <div ref={sentinel} className="feed-sentinel" data-testid="feed-sentinel" aria-hidden="true" />
        <p className={footer.busy ? "feed-footer fetching" : "feed-footer muted"} data-testid="feed-footer" role="status">
          {footer.text}
        </p>
      </div>
      <p className="note">
        Scroll the list: the sentinel under the last row asks for the next page when it comes into view (<code>useLoadMore</code>).
      </p>
    </>
  );
}
