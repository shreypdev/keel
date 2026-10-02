import { UndraCallError } from "@undra/runtime";
import { useSignal, useUndra } from "@undra/runtime/react";
import { type Filter, TodoError, TodoSelection, type Todo, type Todos, draft as draftRow, newest } from "@playground/core";
import { useEffect, useState } from "react";
import { SelectionBar } from "./SelectionBar";

const FILTERS: readonly Filter[] = ["all", "active", "done"];

/**
 * The `Todos` store. `visible` and `remaining` are computed in the core and arrive in the same
 * change-set as the write that changed them; this view only reads them.
 *
 * The strip under the list is the generic code of the core (ADR-058): the ticks live in a `TodoSelection` (this view
 * owns it), the "Latest" line is `newest("Todo", rows)` and "New draft" is `draft("Todo", title)`.
 */
export function TodosView({ todos }: { readonly todos: Todos }) {
  const visible = useSignal(todos.visible);
  const filter = useSignal(todos.filter);
  const remaining = useSignal(todos.remaining);
  const everything = useSignal(todos.todos);
  const selection = useUndra(TodoSelection);
  const picked = useSignal(selection?.rows);
  const pickedCount = useSignal(selection?.count);
  const [latest, setLatest] = useState<Todo | null>(null);
  const [draft, setDraft] = useState("");
  const [problem, setProblem] = useState<string | null>(null);

  // The newest to-do is the core's to say: one call of the function `newest`, for the type named first.
  useEffect(() => {
    let current = true;
    newest("Todo", everything).then(
      (row) => {
        if (current) setLatest(row);
      },
      () => undefined,
    );
    return () => {
      current = false;
    };
  }, [everything]);

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
      {selection !== undefined && (
        <SelectionBar
          kind="todo"
          count={pickedCount}
          latest={latest?.title ?? null}
          picked={(picked ?? []).map((todo) => ({ key: todo.id, title: todo.title }))}
          onSelectAll={() => void selection.selectAll(visible)}
          onClear={() => void selection.clear()}
          // A draft is a to-do the core made but did not store: it is ticked, not added to the list.
          onNew={() => void draftRow("Todo", draft.trim() === "" ? "Untitled" : draft.trim()).then((row) => selection.toggle(row))}
        >
          <button
            disabled={(pickedCount ?? 0) === 0}
            data-testid="todo-remove-selected"
            onClick={() => {
              for (const todo of picked ?? []) void todos.remove(todo.id);
              void selection.clear();
            }}
          >
            Remove selected
          </button>
        </SelectionBar>
      )}
      {visible.length === 0 ? (
        <p className="empty">{everything.length === 0 ? "Nothing to do. Add something above." : `No ${filter} items.`}</p>
      ) : (
        <ul className="list">
          {visible.map((todo) => (
            <li key={todo.id} data-testid="todo-item" data-done={todo.done}>
              <input
                type="checkbox"
                aria-label={`Select ${todo.title}`}
                checked={(picked ?? []).some((row) => row.id === todo.id)}
                disabled={selection === undefined}
                data-testid="todo-select"
                onChange={() => void selection?.toggle(todo)}
              />
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
