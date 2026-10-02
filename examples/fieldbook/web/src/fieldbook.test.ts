// The web app's whole stack in one test: the generated TypeScript bindings over the real Rust core
// (the wasm `undra build --platform web` writes), with the demo server behind the `Http` port. It is the
// scenario the sample is for: sign in, write notes, lose the network, write more, get it back.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { emitConnectivity } from "@undra/runtime";
import { Auth, Notebook, NoteError, UndraFieldbookCore, configureServer } from "@app/fieldbook-core";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { DemoServer, SERVER_URL, TEAM_CODE } from "./demo-server";
import { memoryFs, memoryKv } from "./memory-adapters";

const wasm = readFileSync(fileURLToPath(new URL("../../build/web/fieldbook_core.wasm", import.meta.url)));

describe("Fieldbook over the real core", () => {
  const server = new DemoServer({ latencyMs: 0 });
  let auth: Auth;
  let notebook: Notebook;

  beforeAll(async () => {
    await UndraFieldbookCore.load({
      mode: "wasm-main",
      wasm,
      adapters: { http: server, kv: memoryKv(), secureStore: memoryKv(), fs: memoryFs() },
    });
    configureServer({ baseUrl: SERVER_URL });
    auth = await Auth.create();
    notebook = await Notebook.create();
  });

  afterAll(() => {
    UndraFieldbookCore.core.close();
  });

  it("starts signed out and refuses a wrong code with a typed error", async () => {
    expect(auth.session.peek()).toEqual({ kind: "signedOut" });
    await expect(auth.signIn("Ada", "nope")).rejects.toMatchObject({ kind: "badCredentials" });
    await auth.signIn("Ada", TEAM_CODE);
    expect(auth.session.peek()).toEqual({ kind: "signedIn", user: "Ada" });
  });

  it("saves a note and sends it; the view is pinned first, newest first", async () => {
    await expect(notebook.add("  ", "", "")).rejects.toBeInstanceOf(NoteError.EmptyTitle);
    const first = await notebook.add("Heron at the weir", "grey, still", "Birds");
    await notebook.add("Gate 3 hinge", "rusted through", "repairs");
    expect(notebook.visible.peek().map((n) => n.title)).toEqual(["Gate 3 hinge", "Heron at the weir"]);
    expect(notebook.tags.peek()).toEqual(["birds", "repairs"]);
    await notebook.togglePin(first.id);
    expect(notebook.visible.peek()[0]?.title).toBe("Heron at the weir");
    await vi.waitFor(() => expect(server.notes().map((n) => n.title).sort()).toEqual(["Gate 3 hinge", "Heron at the weir"]));
  });

  it("filters by tag and text", async () => {
    await notebook.setTag("repairs");
    expect(notebook.visible.peek().map((n) => n.title)).toEqual(["Gate 3 hinge"]);
    await notebook.setTag("");
    await notebook.setQuery("GREY");
    expect(notebook.visible.peek().map((n) => n.title)).toEqual(["Heron at the weir"]);
    await notebook.setQuery("");
  });

  it("keeps a photo in the core's Fs port and uploads it", async () => {
    const [note] = notebook.visible.peek();
    const path = await notebook.attachPhoto(note?.id ?? 0, new Uint8Array([1, 2, 3]));
    expect(notebook.notes.peek().find((n) => n.id === note?.id)?.photos).toEqual([path]);
    await vi.waitFor(() => expect(server.photos().size).toBe(1));
  });

  it("writes a note offline, counts it as waiting, and sends it when the network returns", async () => {
    server.offline = true;
    emitConnectivity(UndraFieldbookCore.core, false, "none");
    const note = await notebook.add("No signal at the gate", "", "");
    await vi.waitFor(() => expect(notebook.pending.peek()).toBe(1));
    expect(notebook.visible.peek().some((n) => n.id === note.id)).toBe(true);
    expect(server.notes().some((n) => n.id === note.id)).toBe(false);

    server.offline = false;
    emitConnectivity(UndraFieldbookCore.core, true, "wifi");
    await vi.waitFor(() => expect(server.notes().some((n) => n.id === note.id)).toBe(true), { timeout: 8000 });
    await vi.waitFor(() => expect(notebook.pending.peek()).toBe(0), { timeout: 8000 });
  });

  it("refuses to sign out while writes are waiting, and signs out when none are", async () => {
    server.offline = true;
    emitConnectivity(UndraFieldbookCore.core, false, "none");
    await notebook.add("one more", "", "");
    await vi.waitFor(() => expect(notebook.pending.peek()).toBe(1));
    await expect(auth.signOut()).rejects.toMatchObject({ kind: "pendingWrites", count: 1 });
    server.offline = false;
    emitConnectivity(UndraFieldbookCore.core, true, "wifi");
    await vi.waitFor(() => expect(notebook.pending.peek()).toBe(0), { timeout: 8000 });
    await auth.signOut();
    expect(auth.session.peek()).toEqual({ kind: "signedOut" });
  });
});
