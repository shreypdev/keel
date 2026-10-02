#!/usr/bin/env node
// A backend for Fieldbook's native apps, in memory, with no dependencies: the same routes the web app's in-page
// DemoServer (web/src/demo-server.ts) answers.
//
//   node server/server.mjs [port]      # default 8787
//
// iOS Simulator: http://127.0.0.1:8787. Android emulator: http://10.0.2.2:8787 (the host as the emulator sees it).
// Sign in with any name and the team code `fieldbook`. Access tokens last 60 seconds so a 401 and the refresh are
// easy to see; set FIELDBOOK_TOKEN_SECONDS=0 for tokens that never expire.
import { createServer } from "node:http";

const port = Number(process.argv[2] ?? process.env.PORT ?? 8787);
const TTL = Number(process.env.FIELDBOOK_TOKEN_SECONDS ?? 60) * 1000;
const notes = new Map();
const photos = new Map();
const seen = new Map(); // Idempotency-Key -> what the first request answered
let access = { token: "access-1", expires: Date.now() + TTL };
let generation = 1;

const read = (req) => new Promise((resolve) => {
  const chunks = [];
  req.on("data", (c) => chunks.push(c));
  req.on("end", () => resolve(Buffer.concat(chunks)));
});

function route(method, path, body, headers) {
  const json = () => { try { return JSON.parse(body.toString("utf8")); } catch { return undefined; } };
  if (method === "POST" && path === "/auth/login") {
    const b = json();
    if (typeof b?.name !== "string" || b.code !== "fieldbook") return [401, { error: "wrong team code" }];
    return [200, { access: access.token, refresh: "refresh", user: b.name }];
  }
  if (method === "POST" && path === "/auth/refresh") {
    if (json()?.refresh !== "refresh") return [401, { error: "refresh refused" }];
    generation++;
    access = { token: `access-${generation}`, expires: TTL > 0 ? Date.now() + TTL : Infinity };
    return [200, { access: access.token, refresh: "refresh" }];
  }
  const live = headers.authorization === `Bearer ${access.token}` && (TTL === 0 || Date.now() < access.expires);
  if (!live) return [401, { error: "sign in" }];
  if (method === "GET" && path === "/me") return [200, { name: "member" }];
  let m = /^\/notes\/(\d+)$/.exec(path);
  if (m && method === "PUT") { const n = json(); if (typeof n?.title !== "string") return [400, { error: "expected a note" }]; notes.set(m[1], n); return [204]; }
  if (m && method === "DELETE") return notes.delete(m[1]) ? [204] : [404];
  m = /^\/notes\/(\d+)\/photos\/(\d+)$/.exec(path);
  if (m && method === "PUT") { photos.set(`${m[1]}/${m[2]}`, body); return [204]; }
  if (method === "GET" && path === "/notes") return [200, [...notes.values()]];
  return [404, { error: "no such route" }];
}

createServer(async (req, res) => {
  const body = await read(req);
  const key = req.headers["idempotency-key"];
  let answer = key ? seen.get(key) : undefined;
  if (!answer) {
    answer = route(req.method, new URL(req.url, "http://x").pathname, body, req.headers);
    if (key && answer[0] < 300) seen.set(key, answer);
  }
  console.log(`${req.method} ${req.url} -> ${answer[0]}`);
  res.writeHead(answer[0], { "Content-Type": "application/json" });
  res.end(answer[1] === undefined ? undefined : JSON.stringify(answer[1]));
}).listen(port, () => console.log(`Fieldbook server on http://127.0.0.1:${port}`));
