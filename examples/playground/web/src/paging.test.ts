import { describe, expect, it } from "vitest";
import { OVERSCAN, ROW_HEIGHT, SLOW_POLL_MS, VIEWPORT_ROWS, feedFooter, rowWindow, tickerLine } from "./paging";

// The Library, Feed and Ticker tabs' own logic (src/paging.ts); the views are driven in a real browser by smoke/smoke.spec.ts.

describe("the Library's window", () => {
  it("shows the first rows from the top, with overscan below and none above", () => {
    expect(rowWindow(0, 10_000)).toEqual({ firstVisible: 0, from: 0, to: VIEWPORT_ROWS + OVERSCAN });
  });

  it("follows the scroll position row by row", () => {
    expect(rowWindow(ROW_HEIGHT * 5_000, 10_000)).toEqual({ firstVisible: 5_000, from: 5_000 - OVERSCAN, to: 5_000 + VIEWPORT_ROWS + OVERSCAN });
    expect(rowWindow(ROW_HEIGHT * 5_000 + ROW_HEIGHT - 1, 10_000).firstVisible, "a row is visible until it is scrolled out").toBe(5_000);
  });

  it("is clamped to the list: a short list, an empty one, a scroll position past the end", () => {
    expect(rowWindow(0, 3)).toEqual({ firstVisible: 0, from: 0, to: 3 });
    expect(rowWindow(0, 0)).toEqual({ firstVisible: 0, from: 0, to: 0 });
    expect(rowWindow(ROW_HEIGHT * 9_999, 10_000).to).toBe(10_000);
    const shrunk = rowWindow(ROW_HEIGHT * 9_999, 40);
    expect(shrunk).toEqual({ firstVisible: 39, from: 34, to: 40 });
  });

  it("treats a scroll position that is not a number as the top", () => {
    for (const bad of [Number.NaN, Number.POSITIVE_INFINITY, -50]) expect(rowWindow(bad, 100).firstVisible).toBe(0);
  });
});

describe("the Feed's footer", () => {
  it("says nothing before the first page, then invites scrolling", () => {
    expect(feedFooter({ loaded: 0, hasNextPage: false, fetchingNextPage: false })).toEqual({ text: "", busy: false });
    expect(feedFooter({ loaded: 50, hasNextPage: true, fetchingNextPage: false })).toEqual({ text: "Scroll for more", busy: false });
  });

  it("is busy while the next page loads, whatever else is true", () => {
    for (const loaded of [0, 50]) expect(feedFooter({ loaded, hasNextPage: true, fetchingNextPage: true })).toEqual({ text: "Loading more…", busy: true });
  });

  it("ends with the number of rows, grouped", () => {
    expect(feedFooter({ loaded: 10_000, hasNextPage: false, fetchingNextPage: false }).text).toBe("That is all 10,000 rows");
  });
});

describe("the Ticker's line", () => {
  it("tells what it holds and how often it asks", () => {
    expect(tickerLine({ value: null, slow: false })).toBe("waiting for the first fetch (then every 1 s)");
    expect(tickerLine({ value: 1, slow: false })).toBe("fetched 1 time, polling every 1 s");
    expect(tickerLine({ value: 7, slow: true })).toBe(`fetched 7 times, polling every ${SLOW_POLL_MS / 1000} s`);
  });
});
