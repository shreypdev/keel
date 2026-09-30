import { type PlaygroundParams, type Theme, parseThemeMessage } from "./url-params";

/** The part of `<html>` that the page mode needs: its attributes (the stylesheet keys off them). */
export interface RootLike {
  setAttribute(name: string, value: string): void;
  removeAttribute(name: string): void;
}

/** The part of `window` that {@link listenForTheme} needs. */
export interface MessageSource {
  addEventListener(type: "message", listener: (event: { readonly data: unknown; readonly source: unknown }) => void): void;
  removeEventListener(type: "message", listener: (event: { readonly data: unknown; readonly source: unknown }) => void): void;
}

/**
 * Forces the colour scheme (`data-theme` on `<html>`, which `index.css` honours) or, for
 * `undefined`, hands it back to `prefers-color-scheme`.
 */
export function applyTheme(root: RootLike, theme: Theme | undefined): void {
  if (theme === undefined) root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", theme);
}

/**
 * Sets the page mode the URL asked for, before the first render: the forced theme, and
 * `data-embed`, which `index.css` turns into a transparent background and compact padding.
 */
export function applyPageMode(root: RootLike, params: PlaygroundParams): void {
  applyTheme(root, params.theme);
  if (params.embed) root.setAttribute("data-embed", "");
  else root.removeAttribute("data-embed");
}

/**
 * Applies `{ type: "keel-theme", theme }` messages from `from` (the embedding page) to `root`,
 * so the landing page's theme toggle restyles the iframe. A message of any other shape, or from
 * any other window, is ignored. Returns a function that stops listening.
 */
export function listenForTheme(source: MessageSource, root: RootLike, from: unknown): () => void {
  const onMessage = (event: { readonly data: unknown; readonly source: unknown }): void => {
    if (event.source !== from) return;
    const theme = parseThemeMessage(event.data);
    if (theme !== undefined) applyTheme(root, theme);
  };
  source.addEventListener("message", onMessage);
  return () => {
    source.removeEventListener("message", onMessage);
  };
}
