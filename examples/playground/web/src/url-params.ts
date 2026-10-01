/*
 * What the page's URL can ask for, parsed in one place and with no DOM, so it is unit-tested in
 * Node. The landing page embeds the playground in an iframe and steers it with these:
 *
 *   playground/?screen=list&stream=1&embed=1&theme=dark
 *
 * Nothing here throws: a malformed or unknown value is ignored and the page behaves as if the
 * parameter were absent.
 */

/** The ids of the five views; `biglist` is the "10k list" tab. */
export const TAB_IDS = ["todos", "counter", "biglist", "remote", "stress"] as const;

/** One of the five views. */
export type TabId = (typeof TAB_IDS)[number];

/** The names `screen=` accepts, and the view each one shows. `list` is the 10k list, whose tab id is `biglist`. */
const SCREENS: Readonly<Record<string, TabId>> = {
  todos: "todos",
  counter: "counter",
  list: "biglist",
  remote: "remote",
  stress: "stress",
};

/** What the stress generator writes: the merged `value` signal or the `no_coalesce` `progress` one. */
export type StressModeParam = "firehose" | "progress";

/** The `mode=` values, as a table for {@link lookup}. */
const STRESS_MODES: Readonly<Record<string, StressModeParam>> = { firehose: "firehose", progress: "progress" };

/** The fastest generator the core accepts, in updates per second (`playground_core::MAX_RATE`). */
export const STRESS_MAX_RATE = 1_000_000;

/** A forced colour scheme. */
export type Theme = "light" | "dark";

/** What the query string asked for. */
export interface PlaygroundParams {
  /** The view `screen=` named, or `undefined` when it is absent or not one of the known screens. */
  readonly screen: TabId | undefined;
  /** `stream=1`: start the list's ten-updates-a-second stream on its own. */
  readonly stream: boolean;
  /** `embed=1`: show only the chosen view, with no tab bar or header, on a transparent background. */
  readonly embed: boolean;
  /** `theme=light|dark`: force the colour scheme; `undefined` follows `prefers-color-scheme`. */
  readonly theme: Theme | undefined;
  /** `rate=10000` (or `rate=10k`): the stress screen's generator rate in updates a second, `1..=1,000,000`; `undefined` when absent or out of range. */
  readonly rate: number | undefined;
  /** `mode=firehose|progress`: what the stress screen's generator writes; `undefined` when absent or unknown. */
  readonly mode: StressModeParam | undefined;
  /** `autostart=1`: the stress screen starts its generator on its own. */
  readonly autostart: boolean;
}

/** A flag is on for `1` or `true`; anything else, including a missing value, is off. */
function isOn(value: string | null): boolean {
  const v = value?.trim().toLowerCase();
  return v === "1" || v === "true";
}

/** A known name from `value`, matched without regard to case or surrounding spaces. */
function lookup<T>(table: Readonly<Record<string, T>>, value: string | null): T | undefined {
  const key = value?.trim().toLowerCase();
  return key !== undefined && Object.hasOwn(table, key) ? table[key] : undefined;
}

/** A rate: whole updates a second, optionally with a `k` suffix (`100k`), within the core's range; anything else is `undefined`. */
function rateOf(value: string | null): number | undefined {
  const match = /^(\d{1,9})(k?)$/i.exec(value?.trim() ?? "");
  if (match === null) return undefined;
  const rate = Number(match[1]) * (match[2] === "" ? 1 : 1000);
  return rate >= 1 && rate <= STRESS_MAX_RATE ? rate : undefined;
}

/** The `theme` values, as a table for {@link lookup}. */
const THEMES: Readonly<Record<string, Theme>> = { light: "light", dark: "dark" };

/**
 * Reads the playground's parameters from a query string (`location.search`, with or without the
 * leading `?`). Never throws: input that is not a string counts as an empty query.
 *
 * ```ts
 * parseParams("?screen=list&stream=1&embed=1");
 * // { screen: "biglist", stream: true, embed: true, theme: undefined, rate: undefined, mode: undefined, autostart: false }
 *
 * parseParams("?screen=stress&embed=1&rate=100k&mode=progress&autostart=1");
 * // { screen: "stress", embed: true, rate: 100000, mode: "progress", autostart: true, ... }
 * ```
 */
export function parseParams(search: string): PlaygroundParams {
  let query: URLSearchParams;
  try {
    query = new URLSearchParams(typeof search === "string" ? search : "");
  } catch {
    query = new URLSearchParams();
  }
  return {
    screen: lookup(SCREENS, query.get("screen")),
    stream: isOn(query.get("stream")),
    embed: isOn(query.get("embed")),
    theme: lookup(THEMES, query.get("theme")),
    rate: rateOf(query.get("rate")),
    mode: lookup(STRESS_MODES, query.get("mode")),
    autostart: isOn(query.get("autostart")),
  };
}

/** The tab a URL fragment names (`#counter`, with or without the `#`), or `undefined`. */
export function tabFromHash(hash: string): TabId | undefined {
  if (typeof hash !== "string") return undefined;
  const wanted = hash.startsWith("#") ? hash.slice(1) : hash;
  return TAB_IDS.find((id) => id === wanted);
}

/**
 * The view to show first. `screen=` wins over the `#hash` (which keeps linking to a tab); with
 * neither, the default is the to-do list, or the 10k list in embed mode, where the landing page
 * wants the list and nothing else.
 */
export function resolveTab(params: PlaygroundParams, hash: string): TabId {
  return params.screen ?? tabFromHash(hash) ?? (params.embed ? "biglist" : "todos");
}

/**
 * The colour scheme a message from the embedding page asks for: `{ type: "undra-theme", theme }`
 * with `theme` `"light"` or `"dark"`. Anything else, from any sender, gives `undefined`.
 * `event.data` is untrusted, so every step is checked.
 */
export function parseThemeMessage(data: unknown): Theme | undefined {
  if (typeof data !== "object" || data === null) return undefined;
  const message = data as { readonly type?: unknown; readonly theme?: unknown };
  if (message.type !== "undra-theme") return undefined;
  return message.theme === "light" || message.theme === "dark" ? message.theme : undefined;
}
