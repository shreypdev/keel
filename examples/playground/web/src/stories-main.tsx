import { type ReactElement, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./index.css";
import type { Meta, Story } from "./stories/story";

/**
 * A plain page that renders the playground's stories (`src/stories/*.stories.tsx`) with Vite alone: a list on the left, the chosen story on the
 * right (`?story=Todos/RealCoreOnFakes`). The modules are Component Story Format, so Storybook can load them unchanged; nothing here depends on it.
 */
interface Entry {
  readonly id: string;
  readonly title: string;
  readonly name: string;
  readonly story: Story;
}

const modules = import.meta.glob<{ readonly default: Meta } & Record<string, Story | Meta>>("./stories/*.stories.tsx", { eager: true });

const entries: Entry[] = Object.values(modules).flatMap((module) =>
  Object.entries(module)
    .filter(([key, value]) => key !== "default" && typeof (value as Story).render === "function")
    .map(([key, value]) => {
      const story = value as Story;
      return { id: `${module.default.title}/${key}`, title: module.default.title, name: story.name ?? key, story };
    }),
);

function Stories(): ReactElement {
  const chosen = new URLSearchParams(location.search).get("story") ?? entries[0]?.id;
  const current = entries.find((e) => e.id === chosen);
  return (
    <div style={{ display: "grid", gridTemplateColumns: "minmax(220px, 280px) 1fr", gap: "1.5rem", padding: "1rem" }}>
      <nav aria-label="Stories">
        <h1 style={{ fontSize: "1.1rem" }}>Stories</h1>
        <ul style={{ listStyle: "none", padding: 0 }}>
          {entries.map((e) => (
            <li key={e.id} style={{ margin: "0.4rem 0" }}>
              <a href={`?story=${encodeURIComponent(e.id)}`} aria-current={e.id === chosen ? "page" : undefined}>
                {e.title}: {e.name}
              </a>
            </li>
          ))}
        </ul>
      </nav>
      <main>{current === undefined ? <p>No such story.</p> : <current.story.render key={current.id} />}</main>
    </div>
  );
}

const root = document.getElementById("root");
if (root !== null) {
  createRoot(root).render(
    <StrictMode>
      <Stories />
    </StrictMode>,
  );
}
