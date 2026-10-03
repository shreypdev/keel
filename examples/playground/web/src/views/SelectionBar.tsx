import type { ReactNode } from "react";

/** One ticked row, as the bar lists it. */
export interface Picked {
  readonly key: string;
  readonly title: string;
}

/**
 * The strip under the to-do and the note list: what is ticked, the newest row, and the commands of the selection store
 * (`TodoSelection` and `NoteSelection` are one generic `Selection<T>` of the core, instantiated twice, ADR-058).
 * The two screens share it because the two stores have the same members; only their row types differ.
 */
export function SelectionBar(props: {
  readonly kind: "todo" | "note";
  readonly count: number | undefined;
  readonly latest: string | null;
  readonly picked: readonly Picked[];
  readonly onSelectAll: () => void;
  readonly onClear: () => void;
  readonly onNew: () => void;
  readonly children?: ReactNode;
}) {
  const { kind, count, latest, picked } = props;
  return (
    <section className="selection" aria-label={`Selected ${kind}s`} data-testid={`${kind}-selection`}>
      <div className="row wrap">
        <span className="badge" data-testid={`${kind}-selected-count`}>
          {count ?? 0} selected
        </span>
        <button onClick={props.onSelectAll} data-testid={`${kind}-select-all`}>
          Select all
        </button>
        <button disabled={(count ?? 0) === 0} onClick={props.onClear} data-testid={`${kind}-select-clear`}>
          Clear
        </button>
        {props.children}
        <button onClick={props.onNew} data-testid={`${kind}-new-draft`}>
          New draft
        </button>
        <span className="spacer" />
        <span className="muted" data-testid={`${kind}-latest`}>
          {latest === null ? "Nothing yet" : `Latest: ${latest}`}
        </span>
      </div>
      {picked.length > 0 && (
        <div className="chips" data-testid={`${kind}-picked`}>
          {picked.map((row) => (
            <span key={row.key} className="badge">
              {row.title}
            </span>
          ))}
        </div>
      )}
    </section>
  );
}
