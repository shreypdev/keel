import { Notebook, UndraIds, configureServer } from "@app/fieldbook-core";
import { PreviewCore } from "@undra/testkit";
import { type ReactElement, useEffect, useState } from "react";
import wasmUrl from "../../../build/web/fieldbook_core.wasm?url";
import { NotebookView } from "../App";
import { SERVER_URL } from "../demo-server";
import type { Meta, Story } from "./story";

export default { title: "Notebook" } satisfies Meta;

/** What a device holds after a week in the field: JSON in the `Kv` port, as the core stores it. */
const note = (id: number, title: string, body: string, tag: string, pinned = false): [string, string] => [
  `fieldbook.note.${String(id).padStart(8, "0")}`,
  JSON.stringify({ id, title, body, tag, pinned, created_ms: 1_700_000_000_000 + id * 3_600_000, photos: [] }),
];

/**
 * The app's own wasm core running its real logic on the testing kit's deterministic fakes (docs/TESTING.md): the notes
 * are seeded into the `Kv` fake, the member is signed in on the `SecureStore` fake and the server is a fake that takes
 * every note. Nothing is real, so the story looks the same on every machine and every note is one keystroke away.
 */
function RealCoreOnFakes({ offline }: { readonly offline: boolean }): ReactElement {
  const [state, setState] = useState<{ readonly preview: PreviewCore; readonly notebook: Notebook }>();
  useEffect(() => {
    let live = true;
    let loaded: PreviewCore | undefined;
    const seed = {
      version: 1,
      kv: Object.fromEntries([
        note(1, "Heron at the weir", "Grey, still, one leg up.", "birds", true),
        note(2, "Gate 3 hinge", "Rusted through; photo in the shed.", "repairs"),
        note(3, "Owl pellet", "Under the oak by the north fence.", "birds"),
      ]),
      secure_store: { "fieldbook.access": "access-1", "fieldbook.refresh": "refresh-1" },
      http: [{ url_prefix: `${SERVER_URL}/notes`, ...(offline ? { error: "timeout" } : { status: 204 }) }],
      connectivity: { online: !offline, kind: offline ? "none" : "wifi" },
    };
    void (async () => {
      const preview = await PreviewCore.load({ wasm: wasmUrl, expectedSchemaHash: UndraIds.schemaHash, seed: JSON.stringify(seed), shared: false });
      loaded = preview;
      configureServer({ baseUrl: SERVER_URL }, preview.core);
      const notebook = await Notebook.create(preview.core);
      await notebook.load();
      if (live) setState({ preview, notebook });
      else preview.close();
    })();
    return () => {
      live = false;
      loaded?.close();
    };
  }, [offline]);
  return state === undefined ? <p>Loading…</p> : (
    <main>
      <NotebookView notebook={state.notebook} onProblem={(e) => console.error(e)} />
    </main>
  );
}

/** Three notes, one pinned: type a title and add one, pin another, search, pick a tag. The logic is the Rust core's. */
export const ThreeNotes: Story = { name: "Three notes, the real core on scripted ports", render: () => <RealCoreOnFakes offline={false} /> };

/** The same notes with the network down: writes wait in the queue (the fake answers every request with a timeout). */
export const WithNoSignal: Story = { name: "The same, with no signal", render: () => <RealCoreOnFakes offline /> };
