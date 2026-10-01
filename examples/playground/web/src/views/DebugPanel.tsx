import { UndraCallError, UndraCore } from "@undra/runtime";
import { useSignal } from "@undra/runtime/react";
import { explode } from "@playground/core";
import { useState } from "react";
import { describeRestart } from "../recovery-log";
import type { Playground } from "../undra";

/**
 * The debug panel: crash recovery of the web core (ADR-049). A panic in the wasm core traps it; with recovery on,
 * the runtime restarts it from its last snapshot, fails what was in flight with "restarted", and the stores come back
 * with their handles (the query is re-created). "Crash the core" calls `explode`, a function that panics on purpose.
 */
export function DebugPanel({ playground }: { readonly playground: Playground }) {
  const restarts = useSignal(playground.restarts.entries) ?? [];
  const connection = useSignal(UndraCore.shared.connection);
  const [outcome, setOutcome] = useState<string | null>(null);
  const wasm = UndraCore.shared.mode !== "remote";

  const crash = (): void => {
    setOutcome("…");
    explode("crashed on purpose from the debug panel").then(
      () => setOutcome("the core did not crash"),
      (error: unknown) => {
        // A wasm core traps: the call fails "restarted" (Unavailable) and the core comes back. A native core (`undra dev`)
        // contains the panic: the call fails Panicked and nothing restarts.
        if (error instanceof UndraCallError.Unavailable) setOutcome(`the call failed: ${error.transport.reason}`);
        else if (error instanceof UndraCallError) setOutcome(`the call failed: ${error.kind}`);
        else setOutcome(`the call failed: ${String(error)}`);
      },
    );
  };

  return (
    <details className="panel debug" data-testid="debug-panel">
      <summary>Debug</summary>
      <h2>Crash recovery</h2>
      <p className="note">
        {wasm
          ? "On: a panic in the wasm core restarts it from its last snapshot (at most one a second while stores change; at most three restarts a minute)."
          : "Off: this core runs natively (undra dev) and contains its panics; a panicking call fails on its own."}
      </p>
      <div className="row">
        <button data-testid="debug-crash" disabled={connection?.kind === "closed"} onClick={crash}>
          Crash the core
        </button>
        <span className="muted" data-testid="debug-crash-outcome">
          {outcome}
        </span>
      </div>
      <p data-testid="debug-restarts">
        {restarts.length === 0 ? "No restarts." : `${restarts.length === 1 ? "1 restart" : `${restarts.length} restarts`}:`}
      </p>
      {restarts.length > 0 && (
        <ul className="list">
          {restarts.map((entry, index) => (
            <li key={`${entry.at}-${index}`} data-testid="debug-restart">
              <span>
                <strong>{new Date(entry.at).toLocaleTimeString()}</strong> {entry.message}
                <br />
                <span className="muted">{describeRestart(entry)}</span>
              </span>
            </li>
          ))}
        </ul>
      )}
      {connection?.kind === "closed" && (
        <p className="error" role="alert" data-testid="debug-dead">
          The core is down for good ({connection.reason}): reload the page.
        </p>
      )}
    </details>
  );
}
