import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./index.css";
import { startKeel } from "./keel";

const root = createRoot(document.getElementById("root") as HTMLElement);

startKeel().then(
  (playground) => {
    root.render(
      <StrictMode>
        <App playground={playground} />
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
