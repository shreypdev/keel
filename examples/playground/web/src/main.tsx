import { KeelCore } from "@keel/runtime";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { applyPageMode, listenForTheme } from "./embed";
import { startStatsPoster } from "./embed-stats";
import "./index.css";
import { startKeel } from "./keel";
import { parseParams } from "./url-params";

const params = parseParams(location.search);
// Before the first render: the forced theme and the embed mode style the page from its first paint.
applyPageMode(document.documentElement, params);
// Embedded (`?embed=1` in an iframe), the landing page is the parent: it can restyle this page and reads its stats.
const parent = params.embed && window.parent !== window ? window.parent : null;
if (parent !== null) listenForTheme(window, document.documentElement, parent);

const root = createRoot(document.getElementById("root") as HTMLElement);

startKeel().then(
  (playground) => {
    // After the stores exist, so loading them is not among the measured change-sets.
    if (parent !== null) startStatsPoster(KeelCore.shared.mirror, parent);
    root.render(
      <StrictMode>
        <App playground={playground} params={params} />
      </StrictMode>,
    );
  },
  (error: unknown) => {
    // Loading fails when the core was built from another schema than these bindings
    // (`keel bindgen`, `keel build`), or when the dev server cannot be reached (`keel dev`).
    root.render(
      <main>
        <pre className="startup-error" role="alert">
          Keel did not start: {String(error)}
        </pre>
      </main>,
    );
  },
);
