import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { nativeFrameScheduler } from "../src/frame.js";
import { FakeNative } from "./support/fake-native.js";

describe("nativeFrameScheduler", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  test("arms the vsync source once and runs everything waiting at the frame", () => {
    const native = new FakeNative();
    const schedule = nativeFrameScheduler(native, { isActive: () => true });
    const ran: string[] = [];
    schedule(() => ran.push("a"));
    schedule(() => ran.push("b"));
    expect(native.frameRequests).toBe(1);
    expect(ran).toEqual([]);
    native.frame!();
    expect(ran).toEqual(["a", "b"]);
    // The backstop was cleared: nothing runs twice.
    vi.advanceTimersByTime(500);
    expect(ran).toEqual(["a", "b"]);
    schedule(() => ran.push("c"));
    expect(native.frameRequests).toBe(2);
  });

  test("a frame that never comes is replaced by the 100 ms backstop", () => {
    const native = new FakeNative();
    const schedule = nativeFrameScheduler(native, { isActive: () => true });
    const ran: string[] = [];
    schedule(() => ran.push("a"));
    vi.advanceTimersByTime(99);
    expect(ran).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(ran).toEqual(["a"]);
    native.frame!(); // a late frame finds nothing to do
    expect(ran).toEqual(["a"]);
  });

  test("without a vsync source a frame is a 16 ms timer", () => {
    const native = new FakeNative();
    native.frames = false;
    const schedule = nativeFrameScheduler(native, { isActive: () => true });
    const ran: string[] = [];
    schedule(() => ran.push("a"));
    vi.advanceTimersByTime(16);
    expect(ran).toEqual(["a"]);
  });

  test("while the app is not active a zero-delay timer runs it, no frame is asked for", () => {
    const native = new FakeNative();
    const schedule = nativeFrameScheduler(native, { isActive: () => false });
    const ran: string[] = [];
    schedule(() => ran.push("a"));
    expect(native.frameRequests).toBe(0);
    vi.advanceTimersByTime(0);
    expect(ran).toEqual(["a"]);
  });

  test("every scheduler of a module gets the frame: a newer one never takes it from a running one", () => {
    const native = new FakeNative();
    const first = nativeFrameScheduler(native, { isActive: () => true });
    const ran: string[] = [];
    first(() => ran.push("first"));
    // A second scheduler on the same module (a core loaded again, a second loadNative that failed).
    const second = nativeFrameScheduler(native, { isActive: () => true });
    second(() => ran.push("second"));
    native.frame!();
    expect(ran).toEqual(["first", "second"]);
    vi.advanceTimersByTime(500); // both backstops were cleared
    expect(ran).toEqual(["first", "second"]);
    first(() => ran.push("first again"));
    native.frame!();
    expect(ran).toEqual(["first", "second", "first again"]);
  });

  test("a function that throws is reported and the others still run", () => {
    const native = new FakeNative();
    const errors: unknown[] = [];
    const schedule = nativeFrameScheduler(native, { isActive: () => true, onError: (e) => errors.push(e) });
    const ran: string[] = [];
    schedule(() => {
      throw new Error("boom");
    });
    schedule(() => ran.push("b"));
    native.frame!();
    expect(ran).toEqual(["b"]);
    expect(errors).toHaveLength(1);
  });
});
