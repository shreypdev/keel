import { describe, expect, it } from "vitest";
import { opResult, percentile, summarize } from "./stats";

describe("percentile", () => {
  it("is the nearest rank", () => {
    expect(percentile([10, 20, 30, 40], 0.5)).toBe(20);
    expect(percentile([10, 20, 30, 40], 0.99)).toBe(40);
    expect(percentile([10, 20, 30, 40], 0)).toBe(10);
    expect(percentile([7], 0.99)).toBe(7);
  });
  it("is NaN for no samples", () => {
    expect(percentile([], 0.5)).toBeNaN();
  });
  it("is not pushed up a rank by floating-point error", () => {
    const hundred = Array.from({ length: 100 }, (_, i) => i + 1);
    expect(percentile(hundred, 0.07)).toBe(7);
    expect(percentile(hundred, 0.99)).toBe(99);
  });
});

describe("summarize", () => {
  it("summarises in any order", () => {
    const s = summarize([5, 1, 9, 3, 7]);
    expect(s).toEqual({ n: 5, min: 1, p50: 5, p90: 9, p99: 9, max: 9, mean: 5 });
  });
  it("reports the second largest of a hundred as p99", () => {
    const s = summarize(Array.from({ length: 100 }, (_, i) => i + 1));
    expect(s.p99).toBe(99);
    expect(s.max).toBe(100);
  });
  it("is NaN everywhere for no samples", () => {
    const s = summarize([]);
    expect(s.n).toBe(0);
    expect(s.p50).toBeNaN();
    expect(s.mean).toBeNaN();
  });
});

describe("opResult", () => {
  it("carries the batch size and the sample count", () => {
    const r = opResult("sync_call", "batched", 1000, [100, 110, 120], "a note");
    expect(r).toMatchObject({ id: "sync_call", mode: "batched", batch: 1000, samples: 3, p50: 110, max: 120, note: "a note" });
  });
});
