import { useSignal, useUndra } from "@undra/runtime/react";
import { Notes } from "@playground/core";
import { useEffect, useState } from "react";
import { describe } from "./LiveView";

type Opening = { readonly kind: "opening" } | { readonly kind: "open"; readonly version: number } | { readonly kind: "failed"; readonly message: string };

/**
 * Notes kept in SQLite through the opt-in `Db` port (ADR-048; `Notes`, examples/playground/core/src/notes.rs).
 *
 * In the browser the adapter is wa-sqlite in a worker over OPFS, and that dependency awaits
 * approval: until it lands, `waSqliteDb()` answers every open with a typed `DbError.Unavailable`,
 * and this view shows that error, which is what the core sees. The list appears once the adapter
 * exists (the Node, iOS and Android apps have theirs today).
 */
export function NotesView() {
  const notes = useUndra(Notes);
  const list = useSignal(notes?.notes);
  const [opening, setOpening] = useState<Opening>({ kind: "opening" });

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

  return (
    <>
      <h2>
        Notes <span className="badge">SQLite</span>
      </h2>
      {opening.kind === "opening" && <p className="muted">opening the database…</p>}
      {opening.kind === "failed" && (
        <>
          <p className="error" role="alert" data-testid="notes-error">
            {opening.message}
          </p>
          <p className="note">
            The browser's Db adapter (wa-sqlite in a worker over OPFS) is pending: its dependency awaits approval. Until then the core's{" "}
            <code>Db</code> port answers with this typed error instead of failing silently. The same core keeps its notes in SQLite on Node, iOS and
            Android today.
          </p>
        </>
      )}
      {opening.kind === "open" && (
        <>
          <p className="muted" data-testid="notes-version">
            schema version {opening.version}
          </p>
          <ul>
            {(list ?? []).map((note) => (
              <li key={String(note.id)} data-testid="notes-item">
                {note.done ? "✓ " : ""}
                {note.title}
              </li>
            ))}
          </ul>
        </>
      )}
    </>
  );
}
