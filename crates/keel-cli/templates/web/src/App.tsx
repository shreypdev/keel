import { type Filter, type Todos } from "@@TS_PACKAGE@@";
import { useState } from "react";
import { useSignal } from "./useSignal";

const FILTERS: readonly Filter[] = ["all", "active", "done"];

/** Reading a signal with `useSignal` is a local read: the core pushes changes, React renders. */
export function App({ todos }: { readonly todos: Todos }) {
  const visible = useSignal(todos.visible);
  const filter = useSignal(todos.filter);
  const remaining = useSignal(todos.remaining);
  const [draft, setDraft] = useState("");
  const [problem, setProblem] = useState<string | null>(null);

  const add = async (): Promise<void> => {
    try {
      await todos.add(draft);
      setDraft("");
      setProblem(null);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <main>
      <h1>{remaining} left</h1>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void add();
        }}
      >
        <input value={draft} onChange={(e) => setDraft(e.target.value)} placeholder="What needs doing?" />
        <button type="submit" disabled={draft.trim() === ""}>
          Add
        </button>
      </form>
      {problem !== null && <p className="error">{problem}</p>}
      <nav>
        {FILTERS.map((f) => (
          <button key={f} aria-pressed={filter === f} onClick={() => void todos.setFilter(f)}>
            {f}
          </button>
        ))}
        <button onClick={() => void todos.clearDone()}>Clear done</button>
      </nav>
      <ul>
        {visible.map((todo) => (
          <li key={todo.id}>
            <label>
              <input type="checkbox" checked={todo.done} onChange={() => void todos.toggle(todo.id)} />
              <span className={todo.done ? "done" : undefined}>{todo.title}</span>
            </label>
          </li>
        ))}
      </ul>
    </main>
  );
}
