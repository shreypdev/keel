import { UndraCallError } from "@undra/runtime";
import { useSignal } from "@undra/runtime/react";
import { type Filter, TodoError, type Todos } from "@playground/core";
import { useState } from "react";

const FILTERS: readonly Filter[] = ["all", "active", "done"];

/**
 * The `Todos` store. `visible` and `remaining` are computed in the core and arrive in the same
 * change-set as the write that changed them; this view only reads them.
 */
export function TodosView({ todos }: { readonly todos: Todos }) {
  const visible = useSignal(todos.visible);
  const filter = useSignal(todos.filter);
  const remaining = useSignal(todos.remaining);
  const everything = useSignal(todos.todos);
  const [draft, setDraft] = useState("");
  const [problem, setProblem] = useState<string | null>(null);

  const add = async (): Promise<void> => {
    try {
      await todos.add(draft);
      setDraft("");
      setProblem(null);
    } catch (error) {
      // A refused add is a typed error, not a string to parse: `TodoError.EmptyTitle`. Anything else that goes wrong
      // with the call (the core panicked, was closed, ...) is an `UndraCallError`. Both read well as they are.
      setProblem(error instanceof TodoError || error instanceof UndraCallError ? error.message : String(error));
    }
  };

  return (
    <>
      <h2>
        Todos{" "}
        <span className="badge" data-testid="remaining">
          {remaining} left
        </span>
      </h2>
      <form
        className="row"
        onSubmit={(event) => {
          event.preventDefault();
          void add();
        }}
      >
        <input
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          placeholder="What needs doing?"
          aria-label="New to-do"
          aria-invalid={problem !== null}
          data-testid="todo-input"
        />
        <button type="submit" className="primary" data-testid="todo-add">
          Add
        </button>
      </form>
      {problem !== null && (
        <p className="error" role="alert" data-testid="todo-error">
          {problem}
        </p>
      )}
      <div className="row filters" role="group" aria-label="Filter">
        {FILTERS.map((name) => (
          <button key={name} aria-pressed={filter === name} data-testid={`filter-${name}`} onClick={() => void todos.setFilter(name)}>
            {name}
          </button>
        ))}
        <span className="spacer" />
        <button disabled={!everything.some((todo) => todo.done)} data-testid="clear-done" onClick={() => void todos.clearDone()}>
          Clear done
        </button>
      </div>
      {visible.length === 0 ? (
        <p className="empty">{everything.length === 0 ? "Nothing to do. Add something above." : `No ${filter} items.`}</p>
      ) : (
        <ul className="list">
          {visible.map((todo) => (
            <li key={todo.id} data-testid="todo-item" data-done={todo.done}>
              <label>
                <input type="checkbox" checked={todo.done} data-testid="todo-toggle" onChange={() => void todos.toggle(todo.id)} />
                <span className={todo.done ? "done" : undefined}>{todo.title}</span>
              </label>
              <button className="icon" aria-label={`Remove ${todo.title}`} data-testid="todo-remove" onClick={() => void todos.remove(todo.id)}>
                ×
              </button>
            </li>
          ))}
        </ul>
      )}
    </>
  );
}
