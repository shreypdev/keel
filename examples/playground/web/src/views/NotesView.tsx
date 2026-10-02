import { useSignal, useUndra } from "@undra/runtime/react";
import { type Note, NoteSelection, Notes, draft as draftRow, newest } from "@playground/core";
import { useEffect, useState } from "react";
import { describe } from "./LiveView";
import { SelectionBar } from "./SelectionBar";

type Opening = { readonly kind: "opening" } | { readonly kind: "open"; readonly version: number } | { readonly kind: "failed"; readonly message: string };

/**
 * Notes kept in SQLite through the opt-in `Db` port (ADR-048; `Notes`, examples/playground/core/src/notes.rs).
 *
 * The core owns the list and the SQL; the page's adapter is `waSqliteDb()`: SQLite (wa-sqlite) in a
 * dedicated worker over the origin private file system, so the notes survive a reload. The list
 * changes only after the database did: `add` and `toggle` are statements in the core, and the
 * mirror shows their result.
 *
 * The strip under the list is the generic code of the core (ADR-058): the ticks live in a `NoteSelection` (the same
 * `Selection<T>` the to-do screen instantiates for to-dos), "Latest" is `newest("Note", rows)` and "New draft" is
 * `draft("Note", title)`.
 */
export function NotesView() {
  const notes = useUndra(Notes);
  const list = useSignal(notes?.notes);
  const selection = useUndra(NoteSelection);
  const picked = useSignal(selection?.rows);
  const pickedCount = useSignal(selection?.count);
  const [latest, setLatest] = useState<Note | null>(null);
  const [opening, setOpening] = useState<Opening>({ kind: "opening" });
  const [draft, setDraft] = useState("");
  const [problem, setProblem] = useState<string | null>(null);

  useEffect(() => {
    if (notes === undefined) return;
    let current = true;
    notes.open("playground").then(
      (version) => {
        if (current) setOpening({ kind: "open", version });
      },
      (error: unknown) => {
        if (current) setOpening({ kind: "failed", message: describe(error) });
      },
    );
    return () => {
      current = false;
    };
  }, [notes]);

  useEffect(() => {
    let current = true;
    newest("Note", list ?? []).then(
      (row) => {
        if (current) setLatest(row);
      },
      () => undefined,
    );
    return () => {
      current = false;
    };
  }, [list]);

  const run = (work: () => Promise<unknown>): void => {
    work().then(
      () => setProblem(null),
      (error: unknown) => setProblem(describe(error)),
    );
  };

  const add = (): void => {
    const title = draft.trim();
    if (notes === undefined || title === "") return;
    setDraft("");
    run(() => notes.add(title));
  };

  return (
    <>
      <h2>
        Notes <span className="badge">SQLite</span>
      </h2>
      {opening.kind === "opening" && <p className="muted">opening the database…</p>}
      {opening.kind === "failed" && (
        <p className="error" role="alert" data-testid="notes-error">
          {opening.message}
        </p>
      )}
      {opening.kind === "open" && notes !== undefined && (
        <>
          <p className="muted" data-testid="notes-version">
            schema version {opening.version}
          </p>
          <form
            className="row"
            onSubmit={(event) => {
              event.preventDefault();
              add();
            }}
          >
            <input aria-label="New note" placeholder="A note" data-testid="notes-input" value={draft} onChange={(event) => setDraft(event.target.value)} />
            <button type="submit" className="primary" data-testid="notes-add" disabled={draft.trim() === ""}>
              Add
            </button>
          </form>
          {problem !== null && (
            <p className="error" role="alert" data-testid="notes-error">
              {problem}
            </p>
          )}
          {selection !== undefined && (
            <SelectionBar
              kind="note"
              count={pickedCount}
              latest={latest?.title ?? null}
              picked={(picked ?? []).map((note) => ({ key: String(note.id), title: note.title }))}
              onSelectAll={() => void selection.selectAll(list ?? [])}
              onClear={() => void selection.clear()}
              // A draft is a note the core made but did not store: it is ticked, not saved to the database.
              onNew={() => void draftRow("Note", draft.trim() === "" ? "Untitled" : draft.trim()).then((row) => selection.toggle(row))}
            />
          )}
          <ul className="list">
            {(list ?? []).map((note) => (
              <li key={String(note.id)} data-testid="notes-item">
                <input
                  type="checkbox"
                  aria-label={`Select ${note.title}`}
                  checked={(picked ?? []).some((row) => row.id === note.id)}
                  disabled={selection === undefined}
                  data-testid="notes-select"
                  onChange={() => void selection?.toggle(note)}
                />
                <label>
                  <input type="checkbox" data-testid="notes-toggle" checked={note.done} onChange={() => run(() => notes.toggle(note.id))} />
                  <span className={note.done ? "done" : undefined}>{note.title}</span>
                </label>
              </li>
            ))}
          </ul>
        </>
      )}
      <p className="note">
        The core keeps these notes in SQLite through its <code>Db</code> port: in this page, wa-sqlite in a dedicated worker over the origin private file
        system, so they survive a reload. The list changes only after the database did.
      </p>
    </>
  );
}
