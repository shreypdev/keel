// The testing kit on Fieldbook (docs/TESTING.md): the app's own core, in this process, with the kit's deterministic
// fakes as its ports and a manual clock. No timer is real and no server exists: the test says what the network
// answers and when time passes, so the offline story is exact down to the request.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { Auth, Notebook, UndraIds, configureServer } from "@app/fieldbook-core";
import { HttpError } from "@undra/runtime";
import { PreviewCore, matches, response } from "@undra/testkit";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { SERVER_URL } from "./demo-server";

const wasm = readFileSync(fileURLToPath(new URL("../../build/web/fieldbook_core.wasm", import.meta.url)));
const header = (request: { readonly headers: readonly { readonly name: string; readonly value: string }[] }, name: string): string | undefined =>
  request.headers.find((h) => h.name.toLowerCase() === name)?.value;

describe("Fieldbook on the testing kit", () => {
  let preview: PreviewCore;
  let notebook: Notebook;

  beforeAll(async () => {
    preview = await PreviewCore.load({
      wasm,
      expectedSchemaHash: UndraIds.schemaHash,
      shared: false,
      // A member who is already signed in on this device (the seed is JSON, the same document in every kit), and a
      // server that takes every note.
      seed: JSON.stringify({
        version: 1,
        secure_store: { "fieldbook.access": "access-1", "fieldbook.refresh": "refresh-1" },
        http: [{ url_prefix: `${SERVER_URL}/notes`, status: 204 }],
      }),
    });
    configureServer({ baseUrl: SERVER_URL }, preview.core);
    notebook = await Notebook.create(preview.core);
  });

  afterAll(() => {
    preview.close();
  });

  it("sends a note when online, with the member's token", async () => {
    const note = await notebook.add("Heron at the weir", "", "birds");
    await preview.advance(500);
    const put = preview.fakes.http.calls.find((call) => call.url === `${SERVER_URL}/notes/${String(note.id)}`);
    expect(put?.method).toBe("put");
    expect(put === undefined ? undefined : header(put, "authorization")).toBe("Bearer access-1");
    expect(notebook.pending.peek()).toBe(0);
  });

  it("queues a note written offline, then replays it with the same idempotency key when the network returns", async () => {
    preview.fakes.http.reset();
    preview.fakes.connectivity.goOffline();
    preview.fakes.http.respond(matches.any(), new HttpError.Network("offline"));
    const note = await notebook.add("No signal at the gate", "", "");
    await preview.advance(500);
    expect(notebook.pending.peek()).toBe(1);
    expect(notebook.visible.peek().map((n) => n.id)).toContain(note.id);
    const first = preview.fakes.http.calls.at(-1);
    const key = first === undefined ? undefined : header(first, "idempotency-key");
    expect(key).toBeDefined();

    preview.fakes.http.reset();
    preview.fakes.http.respond(matches.urlPrefix(`${SERVER_URL}/notes`), response(204));
    preview.fakes.connectivity.goOnline();
    await preview.advance(2_000);
    expect(notebook.pending.peek()).toBe(0);
    const replay = preview.fakes.http.calls.find((call) => call.method === "put");
    expect(replay === undefined ? undefined : header(replay, "idempotency-key")).toBe(key);
  });

  it("refuses to sign out while a write waits, and signs out once it was sent", async () => {
    preview.fakes.http.reset();
    preview.fakes.http.respond(matches.url(`${SERVER_URL}/me`), response(200, '{"name":"Ada"}'));
    const auth = await Auth.create(preview.core);
    await auth.resume();
    expect(auth.session.peek()).toEqual({ kind: "signedIn", user: "Ada" });

    preview.fakes.connectivity.goOffline();
    preview.fakes.http.respond(matches.any(), new HttpError.Network("offline"));
    await notebook.add("one more", "", "");
    await preview.advance(500);
    await expect(auth.signOut()).rejects.toMatchObject({ kind: "pendingWrites", count: 1 });

    preview.fakes.http.reset();
    preview.fakes.http.respond(matches.any(), response(204));
    preview.fakes.connectivity.goOnline();
    await preview.advance(2_000);
    await auth.signOut();
    expect(auth.session.peek()).toEqual({ kind: "signedOut" });
  });
});
