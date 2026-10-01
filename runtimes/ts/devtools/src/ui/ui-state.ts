/** What the view remembers between paints: the tab, what is expanded, the theme. Never data. */

export type Tab = "timeline" | "ports" | "queries" | "counters";

export interface Actions {
  /** Ask the server to restore the core to `step`. */
  restore(step: number): void;
}

export class UiState {
  tab: Tab = "timeline";
  /** Expanded rows and open values, by key. */
  readonly open = new Set<string>();
  /** How many rows of a table are shown, by signal. */
  readonly rows = new Map<string, number>();
  /** The step the scrubber shows while it is being dragged. */
  scrubbing: number | undefined;

  toggle(key: string): boolean {
    if (this.open.delete(key)) return false;
    this.open.add(key);
    return true;
  }
}

const THEME_KEY = "undra-devtools-theme";

/** The saved theme, or `undefined` to follow the system. */
export function savedTheme(): "dark" | "light" | undefined {
  try {
    const v = localStorage.getItem(THEME_KEY);
    return v === "dark" || v === "light" ? v : undefined;
  } catch {
    return undefined;
  }
}

export function saveTheme(theme: "dark" | "light"): void {
  try {
    localStorage.setItem(THEME_KEY, theme);
  } catch {
    /* storage may be blocked; the choice then lasts until the page closes */
  }
}
