import { describe, expect, test } from "vitest";
import { applyPageMode, applyTheme, listenForTheme } from "./embed";
import { parseParams } from "./url-params";

/** The attributes of a fake <html>. */
function fakeRoot(): { attributes: Map<string, string>; setAttribute(name: string, value: string): void; removeAttribute(name: string): void } {
  const attributes = new Map<string, string>();
  return {
    attributes,
    setAttribute: (name, value) => void attributes.set(name, value),
    removeAttribute: (name) => void attributes.delete(name),
  };
}

type Listener = (event: { readonly data: unknown; readonly source: unknown }) => void;

/** A fake window: keeps its message listeners and can deliver an event to them. */
function fakeWindow() {
  const listeners = new Set<Listener>();
  return {
    listeners,
    addEventListener: (_type: "message", listener: Listener) => void listeners.add(listener),
    removeEventListener: (_type: "message", listener: Listener) => void listeners.delete(listener),
    deliver: (data: unknown, source: unknown) => {
      for (const listener of [...listeners]) listener({ data, source });
    },
  };
}

describe("applyTheme", () => {
  test("forces a scheme, and gives it back to the system for undefined", () => {
    const root = fakeRoot();
    applyTheme(root, "dark");
    expect(root.attributes.get("data-theme")).toBe("dark");
    applyTheme(root, "light");
    expect(root.attributes.get("data-theme")).toBe("light");
    applyTheme(root, undefined);
    expect(root.attributes.has("data-theme")).toBe(false);
  });
});

describe("applyPageMode", () => {
  test("a plain page sets nothing, so prefers-color-scheme decides", () => {
    const root = fakeRoot();
    applyPageMode(root, parseParams(""));
    expect([...root.attributes]).toEqual([]);
  });

  test("embed and theme from the URL", () => {
    const root = fakeRoot();
    applyPageMode(root, parseParams("?screen=list&stream=1&embed=1&theme=light"));
    expect(root.attributes.get("data-embed")).toBe("");
    expect(root.attributes.get("data-theme")).toBe("light");
  });

  test("theme works without embed, and embed without theme", () => {
    const themed = fakeRoot();
    applyPageMode(themed, parseParams("?theme=dark"));
    expect([...themed.attributes]).toEqual([["data-theme", "dark"]]);
    const embedded = fakeRoot();
    applyPageMode(embedded, parseParams("?embed=1"));
    expect([...embedded.attributes]).toEqual([["data-embed", ""]]);
  });
});

describe("listenForTheme", () => {
  const parent = { name: "the landing page" };

  test("applies undra-theme messages from the parent, as often as they come", () => {
    const win = fakeWindow();
    const root = fakeRoot();
    listenForTheme(win, root, parent);
    win.deliver({ type: "undra-theme", theme: "dark" }, parent);
    expect(root.attributes.get("data-theme")).toBe("dark");
    win.deliver({ type: "undra-theme", theme: "light" }, parent);
    expect(root.attributes.get("data-theme")).toBe("light");
  });

  test("ignores other shapes, and messages from any other window", () => {
    const win = fakeWindow();
    const root = fakeRoot();
    listenForTheme(win, root, parent);
    win.deliver({ type: "undra-theme", theme: "dark" }, { name: "some other frame" });
    win.deliver({ type: "undra-theme", theme: "dark" }, null);
    win.deliver({ type: "undra-theme", theme: "purple" }, parent);
    win.deliver("undra-theme", parent);
    win.deliver(null, parent);
    win.deliver({ type: "undra-stats" }, parent);
    expect([...root.attributes]).toEqual([]);
  });

  test("the returned function stops listening", () => {
    const win = fakeWindow();
    const root = fakeRoot();
    const stop = listenForTheme(win, root, parent);
    expect(win.listeners.size).toBe(1);
    stop();
    expect(win.listeners.size).toBe(0);
    win.deliver({ type: "undra-theme", theme: "dark" }, parent);
    expect([...root.attributes]).toEqual([]);
  });
});
