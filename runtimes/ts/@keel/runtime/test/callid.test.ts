import { describe, expect, it } from "vitest";
import { nextCallId } from "../src/callid.js";

describe("nextCallId", () => {
  const free = () => false;

  it("counts up from 1", () => {
    expect(nextCallId(0, free)).toBe(1);
    expect(nextCallId(1, free)).toBe(2);
    expect(nextCallId(41, free)).toBe(42);
  });

  it("wraps at 2^32 and never yields the reserved 0", () => {
    expect(nextCallId(0xffff_fffe, free)).toBe(0xffff_ffff);
    expect(nextCallId(0xffff_ffff, free)).toBe(1);
  });

  it("skips ids that are still in flight, including across the wrap", () => {
    expect(nextCallId(5, (id) => id === 6 || id === 7)).toBe(8);
    expect(nextCallId(0xffff_ffff, (id) => id === 1 || id === 2)).toBe(3);
  });
});
