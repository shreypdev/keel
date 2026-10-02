import { describe, expect, test } from "vitest";
import { DEFAULT_LIVE_URL, STRESS_MAX_RATE, parseParams, parseThemeMessage, resolveTab, tabFromHash } from "./url-params";

/** What a query that asks for nothing parses to. */
const NOTHING = { screen: undefined, stream: false, embed: false, theme: undefined, rate: undefined, mode: undefined, autostart: false, ws: undefined };

describe("parseParams", () => {
  test("an empty query asks for nothing", () => {
    expect(parseParams("")).toEqual(NOTHING);
    expect(parseParams("?")).toEqual(NOTHING);
  });

  test("the landing page's embed URLs", () => {
    expect(parseParams("?screen=list&stream=1&embed=1")).toEqual({ ...NOTHING, screen: "biglist", stream: true, embed: true });
    expect(parseParams("?screen=stress&embed=1&rate=10000&mode=firehose&autostart=1")).toEqual({
      ...NOTHING,
      screen: "stress",
      embed: true,
      rate: 10_000,
      mode: "firehose",
      autostart: true,
    });
  });

  test("screen names the seven views; list is the 10k list (tab id biglist)", () => {
    expect(parseParams("?screen=live").screen).toBe("live");
    expect(parseParams("?screen=notes").screen).toBe("notes");
    expect(parseParams("?screen=todos").screen).toBe("todos");
    expect(parseParams("?screen=counter").screen).toBe("counter");
    expect(parseParams("?screen=list").screen).toBe("biglist");
    expect(parseParams("?screen=remote").screen).toBe("remote");
    expect(parseParams("?screen=stress").screen).toBe("stress");
  });

  test("the tab id biglist is not a screen name; unknown screens are ignored", () => {
    expect(parseParams("?screen=biglist").screen).toBeUndefined();
    expect(parseParams("?screen=stres").screen).toBeUndefined();
    expect(parseParams("?screen=").screen).toBeUndefined();
    expect(parseParams("?screen").screen).toBeUndefined();
    expect(parseParams("?screen=%20").screen).toBeUndefined();
  });

  test("values match without regard to case or spaces; the first of a repeated key wins", () => {
    expect(parseParams("?screen=%20LIST%20").screen).toBe("biglist");
    expect(parseParams("?screen=counter&screen=list").screen).toBe("counter");
    expect(parseParams("?theme=DARK").theme).toBe("dark");
  });

  test("flags are on for 1 or true, off for anything else", () => {
    for (const on of ["1", "true", "TRUE"]) {
      expect(parseParams(`?stream=${on}&embed=${on}&autostart=${on}`)).toMatchObject({ stream: true, embed: true, autostart: true });
    }
    for (const off of ["0", "false", "", "yes", "2", "on"]) {
      expect(parseParams(`?stream=${off}&embed=${off}&autostart=${off}`)).toMatchObject({ stream: false, embed: false, autostart: false });
    }
    expect(parseParams("?stream&embed&autostart")).toMatchObject({ stream: false, embed: false, autostart: false });
  });

  test("rate is whole updates a second, or a count of thousands, inside the core's range", () => {
    expect(parseParams("?rate=1").rate).toBe(1);
    expect(parseParams("?rate=1000").rate).toBe(1000);
    expect(parseParams("?rate=50000").rate).toBe(50_000);
    expect(parseParams("?rate=100k").rate).toBe(100_000);
    expect(parseParams("?rate=%2010K%20").rate).toBe(10_000);
    expect(parseParams(`?rate=${STRESS_MAX_RATE}`).rate).toBe(STRESS_MAX_RATE);
    expect(parseParams("?rate=1000k").rate).toBe(STRESS_MAX_RATE);
    for (const bad of ["0", "-5", "1.5", "1e3", "1000001", "1001k", "k", "", "fast", "0x10", "99999999999", "NaN", "Infinity"]) {
      expect(parseParams(`?rate=${bad}`).rate, `rate=${bad}`).toBeUndefined();
    }
    expect(parseParams("?rate").rate).toBeUndefined();
  });

  test("mode is firehose or progress; anything else is the screen's default", () => {
    expect(parseParams("?mode=firehose").mode).toBe("firehose");
    expect(parseParams("?mode=%20PROGRESS%20").mode).toBe("progress");
    for (const bad of ["churn", "board", "", "toString", "__proto__", "0"]) expect(parseParams(`?mode=${bad}`).mode, `mode=${bad}`).toBeUndefined();
  });

  test("ws is a ws:// or wss:// URL for the Live view; anything else is the default", () => {
    expect(parseParams("?ws=ws://127.0.0.1:9000/ws/echo").ws).toBe("ws://127.0.0.1:9000/ws/echo");
    expect(parseParams(`?ws=${encodeURIComponent("wss://echo.example/socket?room=1")}`).ws).toBe("wss://echo.example/socket?room=1");
    expect(parseParams("?ws=%20ws://h/%20").ws).toBe("ws://h/");
    for (const bad of ["", "http://h/", "javascript:alert(1)", "ws:", "ws://", "h/ws"]) expect(parseParams(`?ws=${encodeURIComponent(bad)}`).ws, `ws=${bad}`).toBeUndefined();
    expect(DEFAULT_LIVE_URL).toBe("ws://127.0.0.1:4180/ws/echo");
  });

  test("theme is light or dark; anything else follows the system", () => {
    expect(parseParams("?theme=light").theme).toBe("light");
    expect(parseParams("?theme=dark").theme).toBe("dark");
    expect(parseParams("?theme=auto").theme).toBeUndefined();
    expect(parseParams("?theme=").theme).toBeUndefined();
  });

  test("malformed queries never throw", () => {
    const hostile = ["?%", "?%zz=1&screen=%E0%A4%A", "?screen=list&&&=&=", "?__proto__=1&screen=__proto__", "?screen=constructor&theme=toString", "&&&", "?a=b=c"];
    for (const query of hostile) expect(() => parseParams(query)).not.toThrow();
    expect(parseParams("?screen=__proto__").screen).toBeUndefined();
    expect(parseParams("?screen=constructor").screen).toBeUndefined();
    expect(parseParams("?theme=toString").theme).toBeUndefined();
    // A valid value next to garbage is still read.
    expect(parseParams("?%zz&screen=counter&%").screen).toBe("counter");
  });

  test("a value that is not a string counts as an empty query", () => {
    expect(parseParams(undefined as unknown as string)).toEqual(NOTHING);
    expect(parseParams(null as unknown as string)).toEqual(NOTHING);
  });
});

describe("tabFromHash", () => {
  test("names a tab, with or without the #", () => {
    expect(tabFromHash("#counter")).toBe("counter");
    expect(tabFromHash("#biglist")).toBe("biglist");
    expect(tabFromHash("remote")).toBe("remote");
    expect(tabFromHash("#stress")).toBe("stress");
  });

  test("anything else is no tab", () => {
    for (const hash of ["", "#", "#list", "#nope", "#Counter", "##counter"]) expect(tabFromHash(hash)).toBeUndefined();
    expect(tabFromHash(undefined as unknown as string)).toBeUndefined();
  });
});

describe("resolveTab", () => {
  const none = parseParams("");

  test("without a screen, the hash still picks the tab and todos is the default", () => {
    expect(resolveTab(none, "")).toBe("todos");
    expect(resolveTab(none, "#counter")).toBe("counter");
    expect(resolveTab(none, "#biglist")).toBe("biglist");
    expect(resolveTab(none, "#nope")).toBe("todos");
  });

  test("screen= wins over the hash", () => {
    expect(resolveTab(parseParams("?screen=remote"), "#counter")).toBe("remote");
    expect(resolveTab(parseParams("?screen=list"), "")).toBe("biglist");
  });

  test("the stress screen is reachable by screen= and by the hash", () => {
    expect(resolveTab(parseParams("?screen=stress"), "")).toBe("stress");
    expect(resolveTab(parseParams("?screen=stress"), "#counter")).toBe("stress");
    expect(resolveTab(parseParams("?screen=stress&embed=1"), "")).toBe("stress");
    expect(resolveTab(none, "#stress")).toBe("stress");
  });

  test("an unknown screen falls back like an absent one", () => {
    expect(resolveTab(parseParams("?screen=nope"), "")).toBe("todos");
    expect(resolveTab(parseParams("?screen=nope"), "#counter")).toBe("counter");
    expect(resolveTab(parseParams("?screen=nope&embed=1"), "")).toBe("biglist");
  });

  test("embedded, the default is the list", () => {
    expect(resolveTab(parseParams("?embed=1"), "")).toBe("biglist");
    expect(resolveTab(parseParams("?embed=1&screen=counter"), "")).toBe("counter");
  });
});

describe("parseThemeMessage", () => {
  test("accepts undra-theme with light or dark", () => {
    expect(parseThemeMessage({ type: "undra-theme", theme: "light" })).toBe("light");
    expect(parseThemeMessage({ type: "undra-theme", theme: "dark" })).toBe("dark");
    expect(parseThemeMessage({ type: "undra-theme", theme: "dark", extra: 1 })).toBe("dark");
  });

  test("ignores everything else without throwing", () => {
    const other: unknown[] = [
      undefined,
      null,
      0,
      "undra-theme",
      "dark",
      [],
      ["undra-theme", "dark"],
      {},
      { type: "undra-theme" },
      { type: "undra-theme", theme: "DARK" },
      { type: "undra-theme", theme: "sepia" },
      { type: "undra-theme", theme: 1 },
      { type: "undra-stats", theme: "dark" },
      { theme: "dark" },
      { type: ["undra-theme"], theme: "dark" },
    ];
    for (const data of other) expect(parseThemeMessage(data)).toBeUndefined();
  });

  test("a message with no prototype is ignored", () => {
    expect(parseThemeMessage(Object.create(null))).toBeUndefined();
  });
});
