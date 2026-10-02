/*
 * The app side of the Library, Feed and Ticker tabs (ADR-043), kept apart from the views so that it is tested in Node: the arithmetic of
 * the Library's windowed list and the lines the Feed and the Ticker print. The views read the core's signals; none of this touches the core.
 */

/** Every row of the Library list is this tall, so the position of a row is arithmetic and only the visible ones are drawn. */
export const ROW_HEIGHT = 36;
/** How many rows the Library's scroll area shows. */
export const VIEWPORT_ROWS = 10;
/** Rows drawn above and below the visible ones, so scrolling never shows a gap. */
export const OVERSCAN = 5;
/** How often the Ticker polls when "Poll every 5 s" is on, in milliseconds; off, it is the query's own second. */
export const SLOW_POLL_MS = 5_000;

/** The rows a scroll position shows and the rows drawn around them. */
export interface RowWindow {
  /** The first row the scroll area shows. */
  readonly firstVisible: number;
  /** The first row drawn. */
  readonly from: number;
  /** One past the last row drawn. */
  readonly to: number;
}

/**
 * The window of a list of `length` rows scrolled `scrollTop` pixels: the visible rows with {@link OVERSCAN} around them, clamped to the list.
 * A scroll position past the end (the list shrank under it) shows the last rows.
 *
 * ```ts
 * rowWindow(0, 10_000);        // { firstVisible: 0, from: 0, to: 15 }
 * rowWindow(36 * 5_000, 10_000); // { firstVisible: 5000, from: 4995, to: 5015 }
 * ```
 */
export function rowWindow(scrollTop: number, length: number): RowWindow {
  const last = Math.max(0, length - 1);
  const wanted = Number.isFinite(scrollTop) ? Math.floor(Math.max(0, scrollTop) / ROW_HEIGHT) : 0;
  const firstVisible = Math.min(wanted, last);
  return { firstVisible, from: Math.max(0, firstVisible - OVERSCAN), to: Math.min(length, firstVisible + VIEWPORT_ROWS + OVERSCAN) };
}

/** What the Feed shows under its last row. */
export interface FeedFooter {
  /** The line: what the feed is doing or has done. */
  readonly text: string;
  /** Whether a next page is on its way, so the line is a spinner's. */
  readonly busy: boolean;
}

/** The line under the Feed's rows: loading, scroll for more, or the end. Nothing is said before the first page. */
export function feedFooter(state: { readonly loaded: number; readonly hasNextPage: boolean; readonly fetchingNextPage: boolean }): FeedFooter {
  if (state.fetchingNextPage) return { text: "Loading more…", busy: true };
  if (state.loaded === 0) return { text: "", busy: false };
  if (state.hasNextPage) return { text: "Scroll for more", busy: false };
  return { text: `That is all ${state.loaded.toLocaleString("en-US")} rows`, busy: false };
}

/** The Ticker's one-line state: what it holds and how often it asks. */
export function tickerLine(state: { readonly value: number | null; readonly slow: boolean }): string {
  const every = state.slow ? `${SLOW_POLL_MS / 1000} s` : "1 s";
  return state.value === null ? `waiting for the first fetch (then every ${every})` : `fetched ${state.value} ${state.value === 1 ? "time" : "times"}, polling every ${every}`;
}
