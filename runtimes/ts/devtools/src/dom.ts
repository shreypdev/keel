/** A few lines of DOM: elements from plain calls, no framework. */

type Child = Node | string | number | false | null | undefined;
type PropValue = string | number | boolean | ((event: Event) => void) | undefined;

/** Creates an element. `class` sets the class, `on<event>` adds a listener, `data-*` and the rest are attributes (`false`/`undefined` leave them out). */
export function h<K extends keyof HTMLElementTagNameMap>(tag: K, props?: Record<string, PropValue> | null, ...children: Child[]): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props ?? {})) {
    if (value === undefined || value === false) continue;
    if (typeof value === "function") el.addEventListener(key.slice(2), value);
    else if (key === "class") el.className = String(value);
    else el.setAttribute(key, value === true ? "" : String(value));
  }
  append(el, children);
  return el;
}

/** Appends children, skipping the empty ones. */
export function append(parent: Element, children: readonly Child[]): void {
  for (const child of children) {
    if (child === false || child === null || child === undefined) continue;
    parent.append(typeof child === "number" ? String(child) : child);
  }
}

/** Replaces the children of `parent`. */
export function replace(parent: Element, ...children: Child[]): void {
  parent.replaceChildren();
  append(parent, children);
}

/** `12:03:44.123` from a Unix time in milliseconds. */
export function clock(unixMs: number): string {
  const d = new Date(unixMs);
  const p = (n: number, w = 2): string => String(n).padStart(w, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`;
}

/** `1.5 KiB`. */
export function bytesText(n: number): string {
  if (n >= 1 << 20) return `${(n / (1 << 20)).toFixed(1)} MiB`;
  if (n >= 1 << 10) return `${(n / (1 << 10)).toFixed(1)} KiB`;
  return `${n} B`;
}
