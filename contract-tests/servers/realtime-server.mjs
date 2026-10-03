#!/usr/bin/env node
// The local server of the real-time scenarios (S23 WebSocket, S24 SSE; ADR-047) and of every runtime's
// failure-injection suite for its WebSocket and Sse adapters: one dependency-free Node script, so the four
// runtimes meet the same server behaviour byte for byte.
//
//   node contract-tests/servers/realtime-server.mjs [--port N] [--host 127.0.0.1]
//
// prints `READY <port>` on stdout once it listens (port 0, the default, picks a free one) and serves until it
// is killed (or, with --exit-on-stdin-close, until its stdin closes: a harness that dies takes it along). In-process: `import { startRealtimeServer } from ".../realtime-server.mjs"`.
//
// WebSocket (RFC 6455, server side: masked client frames, fragments, ping/pong, close handshake):
//   /ws/echo                 echoes every text and binary message; the first offered subprotocol is chosen
//   /ws/headers              sends one text message, the JSON of the upgrade request's headers, then echoes
//   /ws/flood?n=N&size=S     sends N text messages ("0", "1", ..., padded with "." to S bytes), honouring the
//                            socket's backpressure, then closes (1000, "end")
//   /ws/close?code=C&reason=R  sends "hello", then a close frame (C, R)
//   /ws/drop                 sends "hello", then destroys the socket without a close frame
//   /ws/deny?status=S        answers the upgrade with HTTP S
//   /ws/auth                 answers the upgrade 401 with a Basic challenge (realm "undra") unless it carries the credentials
//                            undra:secret, then accepts it and sends nothing (an app's authenticator or session delegate answers)
//   /ws/bad-utf8             sends a text frame that is not UTF-8
//   /ws/stall                accepts and sends nothing
// Server-sent events:
//   /sse/feed                the scripted feed below (comments, retry, ids, multi-line data, CRLF, an event
//                            with no data), resumed after the event whose id is the Last-Event-ID header; ends
//   /sse/flood?n=N&size=S    N events of S bytes of data, honouring backpressure, then ends
//   /sse/status?code=C       answers C with an empty body
//   /sse/html                answers 200 text/html
//   /sse/hang                answers 200 text/event-stream, one comment, then nothing until the client leaves
// Bookkeeping:
//   /stats                   JSON: { connections: [{ id, path, headers, protocols, closeCode, closeReason,
//                            written, clientClosed }] }, newest last
//   /reset                   forgets the connections
//   /undra-rn-check          the React Native Http check: its request's headers as JSON
import { createServer } from "node:http";
import { createHash } from "node:crypto";
import { pathToFileURL } from "node:url";

const GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/** The feed of /sse/feed: [id or null, raw block]. Ids decide where a resume starts. */
const FEED = [
  ["1", ": a comment\nretry: 1500\nid: 1\ndata: one\n\n"],
  ["2", "event: tick\nid: 2\ndata: two\ndata: lines\n\n"],
  [null, "data: three\n\n"],
  ["4", "id: 4\r\ndata: four\r\n\r\n"],
  [null, "event: ignored\n\n"],
];

function frame(opcode, payload) {
  const len = payload.length;
  let head;
  if (len < 126) {
    head = Buffer.from([0x80 | opcode, len]);
  } else if (len < 65536) {
    head = Buffer.alloc(4);
    head[0] = 0x80 | opcode;
    head[1] = 126;
    head.writeUInt16BE(len, 2);
  } else {
    head = Buffer.alloc(10);
    head[0] = 0x80 | opcode;
    head[1] = 127;
    head.writeBigUInt64BE(BigInt(len), 2);
  }
  return Buffer.concat([head, payload]);
}

function closePayload(code, reason) {
  const text = Buffer.from(reason ?? "", "utf8");
  const out = Buffer.alloc(2 + text.length);
  out.writeUInt16BE(code, 0);
  text.copy(out, 2);
  return out;
}

/** Parses client frames from a growing buffer; calls onFrame(fin, opcode, payload) for each whole frame. */
function frameReader(onFrame, onError) {
  let buf = Buffer.alloc(0);
  return (chunk) => {
    buf = Buffer.concat([buf, chunk]);
    for (;;) {
      if (buf.length < 2) return;
      const fin = (buf[0] & 0x80) !== 0;
      const opcode = buf[0] & 0x0f;
      const masked = (buf[1] & 0x80) !== 0;
      let len = buf[1] & 0x7f;
      let at = 2;
      if (len === 126) {
        if (buf.length < 4) return;
        len = buf.readUInt16BE(2);
        at = 4;
      } else if (len === 127) {
        if (buf.length < 10) return;
        len = Number(buf.readBigUInt64BE(2));
        at = 10;
      }
      if (!masked) {
        onError(1002, "client frames must be masked");
        return;
      }
      if (buf.length < at + 4 + len) return;
      const mask = buf.subarray(at, at + 4);
      const payload = Buffer.from(buf.subarray(at + 4, at + 4 + len));
      for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i & 3];
      buf = buf.subarray(at + 4 + len);
      onFrame(fin, opcode, payload);
    }
  };
}

export function startRealtimeServer({ port = 0, host = "127.0.0.1" } = {}) {
  let nextId = 1;
  let connections = [];

  function record(path, headers) {
    const c = {
      id: nextId++,
      path,
      headers,
      protocols: [],
      closeCode: null,
      closeReason: null,
      written: 0,
      clientClosed: false,
    };
    connections.push(c);
    return c;
  }

  const server = createServer((req, res) => {
    const url = new URL(req.url, "http://x");
    const path = url.pathname;
    if (path === "/stats") {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ connections }));
      return;
    }
    if (path === "/reset") {
      connections = [];
      res.writeHead(204);
      res.end();
      return;
    }
    if (path.startsWith("/undra-rn-check")) {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ ok: true, path: req.url, headers: req.headers }));
      return;
    }
    if (!path.startsWith("/sse/")) {
      res.writeHead(404);
      res.end();
      return;
    }
    const c = record(path, req.headers);
    req.on("close", () => {
      if (!res.writableFinished) c.clientClosed = true;
    });
    res.on("close", () => {
      if (!res.writableFinished) c.clientClosed = true;
    });
    if (path === "/sse/status") {
      res.writeHead(Number(url.searchParams.get("code") ?? 500));
      res.end();
      return;
    }
    if (path === "/sse/html") {
      res.writeHead(200, { "content-type": "text/html" });
      res.end("<p>not an event stream</p>");
      return;
    }
    const stream = { "content-type": "text/event-stream", "cache-control": "no-cache" };
    if (path === "/sse/feed") {
      const last = req.headers["last-event-id"];
      let start = 0;
      if (last !== undefined) {
        const at = FEED.findIndex(([id]) => id === last);
        start = at < 0 ? 0 : at + 1;
      }
      res.writeHead(200, stream);
      for (const [, block] of FEED.slice(start)) {
        res.write(block);
        c.written++;
      }
      res.end();
      return;
    }
    if (path === "/sse/hang") {
      res.writeHead(200, stream);
      res.write(": open\n\n");
      return;
    }
    if (path === "/sse/flood") {
      const n = Number(url.searchParams.get("n") ?? 1000);
      const size = Number(url.searchParams.get("size") ?? 0);
      res.writeHead(200, stream);
      let i = 0;
      const pump = () => {
        while (i < n) {
          const data = String(i).padEnd(size, ".");
          const ok = res.write(`id: ${i}\ndata: ${data}\n\n`);
          i++;
          c.written++;
          if (!ok) {
            res.once("drain", pump);
            return;
          }
        }
        res.end();
      };
      pump();
      return;
    }
    res.writeHead(404);
    res.end();
  });

  server.on("upgrade", (req, socket) => {
    const url = new URL(req.url, "http://x");
    const path = url.pathname;
    const c = record(path, req.headers);
    socket.on("error", () => {});
    if (path === "/ws/deny") {
      const status = Number(url.searchParams.get("status") ?? 403);
      socket.end(`HTTP/1.1 ${status} Refused\r\nContent-Length: 0\r\nConnection: close\r\n\r\n`);
      return;
    }
    if (path === "/ws/auth" && req.headers.authorization !== `Basic ${Buffer.from("undra:secret").toString("base64")}`) {
      socket.end('HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm="undra"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n');
      return;
    }
    const key = req.headers["sec-websocket-key"];
    if (!key || !path.startsWith("/ws/")) {
      socket.end("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
      return;
    }
    const offered = (req.headers["sec-websocket-protocol"] ?? "")
      .split(",")
      .map((p) => p.trim())
      .filter(Boolean);
    c.protocols = offered;
    const accept = createHash("sha1").update(key + GUID).digest("base64");
    const lines = [
      "HTTP/1.1 101 Switching Protocols",
      "Upgrade: websocket",
      "Connection: Upgrade",
      `Sec-WebSocket-Accept: ${accept}`,
    ];
    if (offered.length > 0) lines.push(`Sec-WebSocket-Protocol: ${offered[0]}`);
    socket.write(lines.join("\r\n") + "\r\n\r\n");

    let closeSent = false;
    const send = (opcode, payload) => {
      if (socket.destroyed) return true;
      return socket.write(frame(opcode, payload));
    };
    const sendClose = (code, reason) => {
      if (closeSent) return;
      closeSent = true;
      send(0x8, closePayload(code, reason));
    };
    let fragments = [];
    let fragmentOpcode = 0;
    const onMessage = (opcode, payload) => {
      if (path === "/ws/echo" || path === "/ws/headers") send(opcode, payload);
    };
    socket.on(
      "data",
      frameReader(
        (fin, opcode, payload) => {
          if (opcode === 0x8) {
            c.closeCode = payload.length >= 2 ? payload.readUInt16BE(0) : 1005;
            c.closeReason = payload.length > 2 ? payload.subarray(2).toString("utf8") : "";
            sendClose(c.closeCode === 1005 ? 1000 : c.closeCode, "");
            socket.end();
            return;
          }
          if (opcode === 0x9) {
            send(0xa, payload);
            return;
          }
          if (opcode === 0xa) return;
          if (opcode === 0x1 || opcode === 0x2) {
            fragmentOpcode = opcode;
            fragments = [payload];
          } else if (opcode === 0x0) {
            fragments.push(payload);
          }
          if (fin) onMessage(fragmentOpcode, Buffer.concat(fragments));
        },
        (code, reason) => {
          sendClose(code, reason);
          socket.end();
        },
      ),
    );
    // A client that goes away without a close frame ends its half of the connection and says nothing more (a platform that
    // tears the connection down before the frame is written: URLSession before macOS 26 / iOS 26): answer it by ending
    // ours, as a WebSocket server does, so that `clientClosed` says the client left.
    socket.on("end", () => socket.end());
    socket.on("close", () => {
      c.clientClosed = true;
    });

    switch (path) {
      case "/ws/headers":
        send(0x1, Buffer.from(JSON.stringify(req.headers), "utf8"));
        break;
      case "/ws/flood": {
        const n = Number(url.searchParams.get("n") ?? 1000);
        const size = Number(url.searchParams.get("size") ?? 0);
        let i = 0;
        const pump = () => {
          while (i < n) {
            const ok = send(0x1, Buffer.from(String(i).padEnd(size, "."), "utf8"));
            i++;
            c.written++;
            if (!ok) {
              socket.once("drain", pump);
              return;
            }
          }
          sendClose(1000, "end");
        };
        pump();
        break;
      }
      case "/ws/close":
        send(0x1, Buffer.from("hello"));
        sendClose(Number(url.searchParams.get("code") ?? 1000), url.searchParams.get("reason") ?? "");
        break;
      case "/ws/drop":
        send(0x1, Buffer.from("hello"));
        setTimeout(() => socket.destroy(), 50);
        break;
      case "/ws/bad-utf8":
        send(0x1, Buffer.from([0xff, 0xfe, 0xfd]));
        break;
      default:
        break;
    }
  });

  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, host, () => {
      const address = server.address();
      resolve({
        port: address.port,
        url: `http://${host}:${address.port}`,
        wsUrl: `ws://${host}:${address.port}`,
        stats: () => ({ connections: connections.map((c) => ({ ...c })) }),
        reset: () => {
          connections = [];
        },
        close: () =>
          new Promise((done) => {
            server.closeAllConnections?.();
            server.close(() => done());
          }),
        server,
      });
    });
  });
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const args = process.argv.slice(2);
  const option = (name, fallback) => {
    const at = args.indexOf(name);
    return at >= 0 ? args[at + 1] : fallback;
  };
  const running = await startRealtimeServer({
    port: Number(option("--port", "0")),
    host: option("--host", "127.0.0.1"),
  });
  process.stdout.write(`READY ${running.port}\n`);
  if (args.includes("--exit-on-stdin-close")) {
    process.stdin.on("end", () => process.exit(0));
    process.stdin.resume();
  }
}
