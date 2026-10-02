import type { WireErrorDetail } from "./wire/errors.js";

/*
 * The runtime's messages: the production flavour (ADR-057, decisions D1 and D8). The package's `dist` (the `default` export
 * condition: what a production build of an app resolves) swaps `messages.ts` for this module; `development` and `react-native`
 * resolve the readable build. It has the same exports as `messages.ts` and the same types, and it carries no sentence: a message
 * is its code, the values it says and a link to the page that explains it. The classes, `kind`s and fields of every error are the
 * same in both flavours (R6); only `message` differs.
 */

/** The errors page every code links to (`site/docs/errors.html`, "Runtime messages"). */
const PAGE = "https://shreypdev.github.io/undra/docs/errors.html";

/** `T0017: callSync, remote — https://shreypdev.github.io/undra/docs/errors.html#T0017`: the code, the values it says, the link. */
export function msg(code: number, ...values: readonly unknown[]): string {
  const id = `T${String(code).padStart(4, "0")}`;
  return `${id}${values.length > 0 ? `: ${values.join(", ")}` : ""} — ${PAGE}#${id}`;
}

/** `wire: code=unexpected_eof at=12 needed=3 — https://…/errors.html#wire-unexpected_eof`: the code of a wire failure and its fields. */
export function wireText(detail: WireErrorDetail): string {
  const { code, ...fields } = detail;
  return `wire: code=${code}${Object.entries(fields).map(([key, value]) => ` ${key}=${String(value)}`).join("")} — ${PAGE}#wire-${code}`;
}

/** Whether the module has the one export that makes it an Undra core: its ABI version says the rest. */
export function missingExports(exported: Readonly<Record<string, unknown>>): readonly string[] {
  return typeof exported.undra_abi_version === "function" ? [] : ["undra_abi_version"];
}
