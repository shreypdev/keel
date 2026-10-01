import { describe, expect, test } from "vitest";
import { parseParams, parseThemeMessage, resolveTab, tabFromHash } from "./url-params";

describe("parseParams", () => {
  test("an empty query asks for nothing", () => {
    expect(parseParams("")).toEqual({ screen: undefined, stream: false, embed: false, theme: undefined });
    expect(parseParams("?")).toEqual({ screen: undefined, stream: false, embed: false, theme: undefined });
  });

  test("the landing page's embed URL", () => {
    expect(parseParams("?screen=list&stream=1&embed=1")).toEqual({ screen: "biglist", stream: true, embed: true, theme: undefined });
  });

  test("screen names the four views; list is the 10k list (tab id biglist)", () => {
    expect(parseParams("?screen=todos").screen).toBe("todos");
    expect(parseParams("?screen=counter").screen).toBe("counter");
    expect(parseParams("?screen=list").screen).toBe("biglist");
    expect(parseParams("?screen=remote").screen).toBe("remote");
  });

  test("the tab id biglist is not a screen name; unknown screens are ignored", () => {
    expect(parseParams("?screen=biglist").screen).toBeUndefined();
    expect(parseParams("?screen=stress").screen).toBeUndefined();
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
      expect(parseParams(`?stream=${on}&embed=${on}`)).toMatchObject({ stream: true, embed: true });
    }
    for (const off of ["0", "false", "", "yes", "2", "on"]) {
      expect(parseParams(`?stream=${off}&embed=${off}`)).toMatchObject({ stream: false, embed: false });
    }
    expect(parseParams("?stream&embed")).toMatchObject({ stream: false, embed: false });
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
    expect(parseParams(undefined as unknown as string)).toEqual({ screen: undefined, stream: false, embed: false, theme: undefined });
    expect(parseParams(null as unknown as string)).toEqual({ screen: undefined, stream: false, embed: false, theme: undefined });
  });
});

describe("tabFromHash", () => {
  test("names a tab, with or without the #", () => {
    expect(tabFromHash("#counter")).toBe("counter");
    expect(tabFromHash("#biglist")).toBe("biglist");
    expect(tabFromHash("remote")).toBe("remote");
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

  test("an unknown screen falls back like an absent one; `stress` does not exist yet", () => {
    expect(resolveTab(parseParams("?screen=stress"), "")).toBe("todos");
    expect(resolveTab(parseParams("?screen=stress"), "#counter")).toBe("counter");
    expect(resolveTab(parseParams("?screen=stress&embed=1"), "")).toBe("biglist");
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
