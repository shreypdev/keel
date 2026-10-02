import { DbError, SseError, UndraCallError, WsError, type WsMessage } from "@undra/runtime";
import { useUndra } from "@undra/runtime/react";
import { Live } from "@playground/core";
import { useRef, useState } from "react";
import { DEFAULT_LIVE_URL } from "../url-params";

/** One line of the conversation: what was sent, what came back, and what happened to the connection. */
interface Entry {
  readonly id: number;
  readonly direction: "out" | "in" | "info";
  readonly text: string;
}

type State = "idle" | "connecting" | "open" | "closed";

/** The text of a failure the way the page shows it: a typed port error says what happened. */
export function describe(error: unknown): string {
  if (error instanceof WsError || error instanceof SseError || error instanceof DbError || error instanceof UndraCallError) return error.message;
  return String(error);
}

/** A message as the log shows it. */
function show(message: WsMessage): string {
  return message.kind === "text" ? message.value : `${message.value.length} bytes`;
}

/**
 * A WebSocket the core drives (`Live`, examples/playground/core/src/live.rs) over the opt-in
 * `WebSocket` port (ADR-047): the page asks the core to connect, send and read; the core reaches
 * the network only through the port, whose adapter here is the browser's `WebSocket`. Inbound
 * messages are pulled with the core's credit: a `read` that nobody makes leaves them on the
 * platform, at most one pull ahead.
 *
 * The address comes from `?ws=`, else the shared realtime server's echo route on port 4180
 * (`node contract-tests/servers/realtime-server.mjs --port 4180`), which the smoke test starts.
 */
export function LiveView({ initialUrl }: { readonly initialUrl: string | undefined }) {
  const live = useUndra(Live);
  const [url, setUrl] = useState(initialUrl ?? DEFAULT_LIVE_URL);
  const [state, setState] = useState<State>("idle");
  const [protocol, setProtocol] = useState("");
  const [draft, setDraft] = useState("ping");
  const [log, setLog] = useState<readonly Entry[]>([]);
  const [problem, setProblem] = useState<string | null>(null);
  const nextId = useRef(1);

  const append = (direction: Entry["direction"], text: string): void => {
    const id = nextId.current++;
    setLog((entries) => [...entries.slice(-49), { id, direction, text }]);
  };

  /** Reads one message at a time until the connection ends; the core pulls 16 at a time underneath. */
  const readUntilTheEnd = async (on: Live): Promise<void> => {
    for (;;) {
      let messages: WsMessage[];
      try {
        messages = await on.read(1);
      } catch (error) {
        setProblem(describe(error));
        append("info", "the connection ended");
        setState("closed");
        return;
      }
      if (messages.length === 0) {
        append("info", "closed");
        setState("closed");
        return;
      }
      for (const message of messages) append("in", show(message));
    }
  };

  const connect = async (): Promise<void> => {
    if (live === undefined) return;
    setProblem(null);
    setState("connecting");
    try {
      const chosen = await live.connect(url.trim(), [], []);
      setProtocol(chosen);
      setState("open");
      append("info", `connected to ${url.trim()}`);
      void readUntilTheEnd(live);
    } catch (error) {
      setProblem(describe(error));
      setState("closed");
    }
  };

  const send = async (): Promise<void> => {
    if (live === undefined || draft === "") return;
    const text = draft;
    try {
      await live.send({ kind: "text", value: text });
      append("out", text);
    } catch (error) {
      setProblem(describe(error));
    }
  };

  const disconnect = async (): Promise<void> => {
    if (live === undefined) return;
    try {
      await live.disconnect(1000, "bye");
    } catch (error) {
      setProblem(describe(error));
    }
  };

  const open = state === "open";
  return (
    <>
      <h2>
        Live <span className="badge">WebSocket</span>
      </h2>
      <form
        className="row"
        onSubmit={(event) => {
          event.preventDefault();
          void connect();
        }}
      >
        <input aria-label="WebSocket URL" data-testid="live-url" value={url} disabled={open || state === "connecting"} onChange={(event) => setUrl(event.target.value)} />
        <button type="submit" className="primary" data-testid="live-connect" disabled={live === undefined || open || state === "connecting"}>
          Connect
        </button>
        <button type="button" data-testid="live-disconnect" disabled={!open} onClick={() => void disconnect()}>
          Disconnect
        </button>
      </form>
      <p className="muted">
        <span data-testid="live-state">{state}</span>
        {open && protocol !== "" && <span data-testid="live-protocol"> · subprotocol {protocol}</span>}
      </p>
      {problem !== null && (
        <p className="error" role="alert" data-testid="live-error">
          {problem}
        </p>
      )}
      <form
        className="row"
        onSubmit={(event) => {
          event.preventDefault();
          void send();
        }}
      >
        <input aria-label="Message" data-testid="live-draft" value={draft} disabled={!open} onChange={(event) => setDraft(event.target.value)} />
        <button type="submit" data-testid="live-send" disabled={!open || draft === ""}>
          Send
        </button>
      </form>
      <ul className="live-log" aria-live="polite">
        {log.map((entry) => (
          <li key={entry.id} className={`live-${entry.direction}`} data-testid="live-message" data-direction={entry.direction}>
            {entry.direction === "out" ? "→ " : entry.direction === "in" ? "← " : ""}
            {entry.text}
          </li>
        ))}
      </ul>
      <p className="note">
        The core holds the connection: the page asks it to connect, send and read, and the core reaches the network only through its{" "}
        <code>WebSocket</code> port. A message nobody reads stays on the platform, at most one pull of 16 ahead. The address is <code>?ws=</code>, or
        the realtime server of the contract tests on port 4180.
      </p>
    </>
  );
}
