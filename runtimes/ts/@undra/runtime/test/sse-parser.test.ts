import { describe, expect, it } from "vitest";
import type { SseEvent } from "../src/adapters/types.js";
import { SseParser } from "../src/realtime.js";

/*
 * The one event-stream parser (the HTML standard's "Interpreting an event stream", as the brief
 * spells it), fed whole and in every split, with the feed of contract-tests/servers/realtime-server.mjs.
 */

/** The blocks of /sse/feed, as the server writes them. */
const FEED = [
  ": a comment\nretry: 1500\nid: 1\ndata: one\n\n",
  "event: tick\nid: 2\ndata: two\ndata: lines\n\n",
  "data: three\n\n",
  "id: 4\r\ndata: four\r\n\r\n",
  "event: ignored\n\n",
].join("");

const FEED_EVENTS: SseEvent[] = [
  { id: "1", event: "message", data: "one", retryMs: 1500 },
  { id: "2", event: "tick", data: "two\nlines", retryMs: null },
  { id: "2", event: "message", data: "three", retryMs: null },
  { id: "4", event: "message", data: "four", retryMs: null },
];

const parse = (...pieces: string[]): SseEvent[] => {
  const parser = new SseParser();
  return pieces.flatMap((piece) => parser.push(piece));
};

const ev = (data: string, extra: Partial<SseEvent> = {}): SseEvent => ({ id: null, event: "message", data, retryMs: null, ...extra });

describe("SseParser", () => {
  it("parses the realtime server's feed: comments, retry, ids that persist, multi-line data, CRLF, an event without data", () => {
    expect(parse(FEED)).toEqual(FEED_EVENTS);
  });

  it("gives the same events however the text is split", () => {
    for (let size = 1; size <= 7; size++) {
      const pieces: string[] = [];
      for (let i = 0; i < FEED.length; i += size) pieces.push(FEED.slice(i, i + size));
      expect(parse(...pieces), `pieces of ${size}`).toEqual(FEED_EVENTS);
    }
    for (let cut = 0; cut <= FEED.length; cut++) expect(parse(FEED.slice(0, cut), FEED.slice(cut)), `cut at ${cut}`).toEqual(FEED_EVENTS);
  });

  it("ends lines with LF, CRLF or a lone CR, also when the CR ends a piece", () => {
    expect(parse("data: a\r\rdata: b\r\n\r\ndata: c\n\n")).toEqual([ev("a"), ev("b"), ev("c")]);
    expect(parse("data: a\r", "\n", "\r", "\n")).toEqual([ev("a")]);
    expect(parse("data: a\r", "\r")).toEqual([ev("a")]);
  });

  it("reads fields as the standard says", () => {
    expect(parse("data:x\n\n")).toEqual([ev("x")]);
    expect(parse("data:  two spaces\n\n")).toEqual([ev(" two spaces")]);
    expect(parse("data\n\n"), "a line without a colon is a field with an empty value").toEqual([ev("")]);
    expect(parse("data\ndata\n\n")).toEqual([ev("\n")]);
    expect(parse("unknown: x\ndata: y\n\n")).toEqual([ev("y")]);
    expect(parse("event: \ndata: y\n\n"), "an empty type is message").toEqual([ev("y")]);
    expect(parse(":comment only\n\n")).toEqual([]);
  });

  it("keeps the last event id across events, ignores one with NUL, and reads an empty one as none", () => {
    expect(parse("id: a\ndata: 1\n\ndata: 2\n\nid: b\0c\ndata: 3\n\nid\ndata: 4\n\n")).toEqual([
      ev("1", { id: "a" }),
      ev("2", { id: "a" }),
      ev("3", { id: "a" }),
      ev("4"),
    ]);
    const resumed = new SseParser("2");
    expect(resumed.push("data: three\n\n"), "a resumed stream starts from the id it resumed after").toEqual([ev("three", { id: "2" })]);
    expect(resumed.lastEventId).toBe("2");
  });

  it("takes a retry of ASCII digits only, for the event it belongs to", () => {
    expect(parse("retry: 10\ndata: a\n\ndata: b\n\n")).toEqual([ev("a", { retryMs: 10 }), ev("b")]);
    expect(parse("retry: 1x\ndata: a\n\nretry: -1\ndata: b\n\nretry:\ndata: c\n\n")).toEqual([ev("a"), ev("b"), ev("c")]);
    expect(parse("retry: 99999999999\ndata: a\n\n"), "beyond u32 is ignored").toEqual([ev("a")]);
  });

  it("skips one byte order mark at the start, and only there", () => {
    expect(parse("﻿data: a\n\n")).toEqual([ev("a")]);
    expect(parse("﻿", "data: a\n\n")).toEqual([ev("a")]);
    expect(parse("data: a\n\n﻿data: b\n\n"), "a later BOM starts a field name").toEqual([ev("a")]);
  });

  it("discards an incomplete event at the end", () => {
    const parser = new SseParser();
    expect(parser.push("data: a\n\ndata: b\n")).toEqual([ev("a")]);
    parser.end();
    expect(parser.push("\n"), "the half event is gone").toEqual([]);
  });
});
