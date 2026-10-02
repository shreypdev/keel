/** The page's entry: connect to the dev server, decode what it sends, paint. */

import { Connection } from "./net.js";
import { decodeServerMsg, encodeRestore, encodeResync, PROTOCOL } from "./proto.js";
import { DevtoolsState } from "./state.js";
import { mount } from "./ui/shell.js";

function start(): void {
  const root = document.getElementById("app");
  if (root === null) return;
  const token = new URLSearchParams(location.search).get("token") ?? "";
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const state = new DevtoolsState();
  let conn: Connection | undefined;
  mount(root, state, {
    restore: (step) => {
      const id = state.nextTravel();
      conn?.send(encodeRestore(id, step));
    },
  });
  conn = new Connection({
    url: `${proto}//${location.host}/devtools/ws?token=${encodeURIComponent(token)}`,
    onState: (s) => state.setConn(s),
    onGiveUp: (reason) => state.notify(reason),
    onMessage: (bytes) => {
      try {
        const msg = decodeServerMsg(bytes);
        if (msg.t === "welcome" && msg.welcome.protocol !== PROTOCOL) {
          state.notify(`this page speaks devtools protocol ${PROTOCOL}, the dev server ${msg.welcome.protocol}: run \`undra dev\` from the same version`);
          conn?.stop();
          state.setConn("closed");
          return;
        }
        state.onMessage(msg);
      } catch (e) {
        state.notify(`a message from the dev server could not be read: ${e instanceof Error ? e.message : String(e)}`);
        conn?.send(encodeResync());
      }
    },
  });
  conn.start();
}

start();
