import { useSignal } from "@keel/runtime/react";
import { RemoteError, type RemoteTodo, createRemoteTodo, setRemoteDone } from "@playground/core";
import { useState } from "react";
import { type Playground, setOffline } from "../keel";
import { INBOX } from "../playground-server";

/** An item the server has not answered for yet has an identity counting down from `u32::MAX` (see `RemoteTodo.id`). */
const isPending = (todo: RemoteTodo): boolean => todo.id >= 0x8000_0000;

/**
 * The `inbox` query over the `Http` port. The core caches it, shows a mutation at once (an
 * optimistic placeholder), takes it back if the server refuses, and, while the network is down,
 * keeps an idempotent mutation queued and replays it when the network returns.
 */
export function RemoteView({ playground }: { readonly playground: Playground }) {
  const { inbox, server } = playground;
  const data = useSignal(inbox.data);
  const status = useSignal(inbox.status);
  const fetching = useSignal(inbox.fetching);
  const error = useSignal(inbox.error);
  const updatedAt = useSignal(inbox.updatedAt);
  const [draft, setDraft] = useState("");
  const [offline, setOfflineState] = useState(server.offline);
  const [problem, setProblem] = useState<string | null>(null);
  const [inFlight, setInFlight] = useState(0);

  const report = (failure: unknown): void => {
    setProblem(failure instanceof RemoteError ? failure.message : String(failure));
  };

  /** Runs a mutation without holding the UI: its outcome is the list changing, and a refusal is shown. */
  const save = (run: () => Promise<unknown>): void => {
    setInFlight((n) => n + 1);
    run()
      .then(() => setProblem(null), report)
      .finally(() => setInFlight((n) => n - 1));
  };

  const add = (): void => {
    const title = draft.trim();
    if (title === "") return;
    setDraft("");
    save(() => createRemoteTodo(INBOX, title));
  };

  return (
    <>
      <h2>
        Remote <span className="badge">{INBOX}</span>
      </h2>
      <div className="row wrap status-line">
        <span className={`status status-${status}`} data-testid="remote-status">
          {status}
        </span>
        {fetching && (
          <span className="fetching" data-testid="remote-fetching" role="status">
            fetching…
          </span>
        )}
        {inFlight > 0 && (
          <span className="fetching" data-testid="remote-saving" role="status">
            saving…
          </span>
        )}
        <span className="spacer" />
        <span className="muted" data-testid="remote-updated">
          {updatedAt === null ? "not fetched yet" : `updated ${new Date(updatedAt).toLocaleTimeString()}`}
        </span>
      </div>
      {error !== null && (
        <p className="error" role="alert" data-testid="remote-error">
          {error.message}
        </p>
      )}
      <form
        className="row"
        onSubmit={(event) => {
          event.preventDefault();
          add();
        }}
      >
        <input value={draft} onChange={(event) => setDraft(event.target.value)} placeholder="New item" aria-label="New remote item" data-testid="remote-input" />
        <button type="submit" className="primary" data-testid="remote-add">
          Add
        </button>
      </form>
      <div className="row wrap">
        <button data-testid="remote-refresh" onClick={() => inbox.refetch().then(() => setProblem(null), report)}>
          Refresh
        </button>
        <label className="switch">
          <input
            type="checkbox"
            role="switch"
            checked={offline}
            data-testid="remote-offline"
            onChange={(event) => {
              setOfflineState(event.target.checked);
              setOffline(playground, event.target.checked);
            }}
          />
          Offline
        </label>
      </div>
      {problem !== null && (
        <p className="error" role="alert" data-testid="remote-action-error">
          {problem}
        </p>
      )}
      {data === null ? (
        <p className="empty">Nothing yet.</p>
      ) : data.length === 0 ? (
        <p className="empty">The list is empty.</p>
      ) : (
        <ul className="list">
          {data.map((todo) => (
            <li key={todo.id} data-testid="remote-item" data-pending={isPending(todo)}>
              <label>
                <input
                  type="checkbox"
                  checked={todo.done}
                  disabled={isPending(todo)}
                  data-testid="remote-toggle"
                  onChange={() => save(() => setRemoteDone(INBOX, todo.id, !todo.done))}
                />
                <span className={todo.done ? "done" : undefined}>{todo.title}</span>
              </label>
              {isPending(todo) && (
                <span className="pending" data-testid="remote-pending">
                  {offline ? "waiting for the network" : "sending…"}
                </span>
              )}
            </li>
          ))}
        </ul>
      )}
      <p className="note">
        The server is this page: a few lines of TypeScript behind the core's <code>Http</code> port, answering after 300 ms. Turn Offline on and add
        an item: the core keeps it queued, and replays the request when you turn Offline off.
      </p>
    </>
  );
}
