import { AuthError, type Note, NoteError, type Notebook, type Auth } from "@app/fieldbook-core";
import { UndraError } from "@undra/runtime";
import { useState } from "react";
import { Photo } from "./Photo";
import { type Fieldbook, setOffline } from "./undra";
import { useSignal } from "./useSignal";

/** What to tell the user about a failed call: the core's typed errors read well as they are. */
function say(error: unknown): string {
  if (error instanceof AuthError.BadCredentials) return "Wrong name or team code (the demo's code is “fieldbook”).";
  if (error instanceof UndraError) return error.message;
  return String(error);
}

/** The signed-out screen, or the notebook: which one is a signal of the core, not state of the page. */
export function App({ app }: { readonly app: Fieldbook }) {
  const session = useSignal(app.auth.session);
  return session.kind === "signedOut" ? <SignIn auth={app.auth} /> : <Notes app={app} user={session.user} />;
}

function SignIn({ auth }: { readonly auth: Auth }) {
  const busy = useSignal(auth.busy);
  const [name, setName] = useState("");
  const [code, setCode] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  return (
    <main>
      <h1>Fieldbook</h1>
      <p className="muted">Field notes with photos. Works without a signal.</p>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          auth.signIn(name, code).then(() => setProblem(null), (error: unknown) => setProblem(say(error)));
        }}
      >
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Your name" aria-label="Your name" />
        <input value={code} onChange={(e) => setCode(e.target.value)} placeholder="Team code" aria-label="Team code" type="password" />
        <button type="submit" disabled={busy || name.trim() === "" || code === ""}>Sign in</button>
      </form>
      {problem !== null && <p className="error" role="alert">{problem}</p>}
    </main>
  );
}

function Notes({ app, user }: { readonly app: Fieldbook; readonly user: string }) {
  const { auth, notebook } = app;
  const visible = useSignal(notebook.visible);
  const tags = useSignal(notebook.tags);
  const filter = useSignal(notebook.filter);
  const pending = useSignal(notebook.pending);
  const [offline, setOfflineState] = useState(app.server.offline);
  const [problem, setProblem] = useState<string | null>(null);
  const report = (error: unknown): void => setProblem(say(error));

  return (
    <main>
      <header className="bar">
        <h1>Fieldbook</h1>
        <span className="muted">{user}</span>
        <button onClick={() => void auth.signOut().then(() => setProblem(null), report)}>Sign out</button>
      </header>
      <p className="status" role="status" data-testid="pending">
        {pending === 0 ? "Everything is sent" : `${String(pending)} change${pending === 1 ? "" : "s"} waiting to send`}
        <label className="switch">
          <input
            type="checkbox"
            checked={offline}
            onChange={(e) => {
              setOfflineState(e.target.checked);
              setOffline(app, e.target.checked);
            }}
          />
          offline
        </label>
      </p>
      {problem !== null && <p className="error" role="alert">{problem}</p>}
      <NewNote notebook={notebook} onProblem={report} />
      <input
        className="search"
        value={filter.query}
        onChange={(e) => void notebook.setQuery(e.target.value)}
        placeholder="Search notes"
        aria-label="Search notes"
      />
      <nav aria-label="Tags">
        <button aria-pressed={filter.tag === ""} onClick={() => void notebook.setTag("")}>all</button>
        {tags.map((tag) => (
          <button key={tag} aria-pressed={filter.tag === tag} onClick={() => void notebook.setTag(tag)}>{tag}</button>
        ))}
      </nav>
      <ul className="notes">
        {visible.map((note) => (
          <NoteRow key={note.id} note={note} notebook={notebook} onProblem={report} />
        ))}
      </ul>
    </main>
  );
}

function NewNote({ notebook, onProblem }: { readonly notebook: Notebook; readonly onProblem: (e: unknown) => void }) {
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [tag, setTag] = useState("");
  return (
    <form
      className="new"
      onSubmit={(event) => {
        event.preventDefault();
        notebook.add(title, body, tag).then(
          () => {
            setTitle("");
            setBody("");
            onProblem(undefined);
          },
          (error: unknown) => onProblem(error instanceof NoteError.EmptyTitle ? "Give the note a title." : error),
        );
      }}
    >
      <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="Title" aria-label="Title" />
      <input value={tag} onChange={(e) => setTag(e.target.value)} placeholder="tag" aria-label="Tag" size={8} />
      <textarea value={body} onChange={(e) => setBody(e.target.value)} placeholder="What did you see?" aria-label="Note" rows={2} />
      <button type="submit">Add note</button>
    </form>
  );
}

function NoteRow({ note, notebook, onProblem }: { readonly note: Note; readonly notebook: Notebook; readonly onProblem: (e: unknown) => void }) {
  return (
    <li className={note.pinned ? "pinned" : undefined}>
      <div className="row">
        <strong>{note.title}</strong>
        {note.tag !== "" && <span className="badge">{note.tag}</span>}
        <span className="grow" />
        <button aria-label={note.pinned ? "Unpin" : "Pin"} onClick={() => void notebook.togglePin(note.id).catch(onProblem)}>
          {note.pinned ? "Unpin" : "Pin"}
        </button>
        <button aria-label="Delete" onClick={() => void notebook.remove(note.id).catch(onProblem)}>Delete</button>
      </div>
      {note.body !== "" && <p>{note.body}</p>}
      <p className="muted">{new Date(note.created).toLocaleString()}</p>
      <div className="photos">
        {note.photos.map((path) => (
          <Photo key={path} path={path} />
        ))}
        <label className="attach">
          + photo
          <input
            type="file"
            accept="image/*"
            hidden
            onChange={(e) => {
              const file = e.target.files?.[0];
              if (file === undefined) return;
              void file.arrayBuffer().then((bytes) => notebook.attachPhoto(note.id, new Uint8Array(bytes))).catch(onProblem);
              e.target.value = "";
            }}
          />
        </label>
      </div>
    </li>
  );
}
