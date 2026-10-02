import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./index.css";
import { startUndra } from "./undra";

const root = createRoot(document.getElementById("root") as HTMLElement);

startUndra().then(
  (app) => {
    root.render(
      <StrictMode>
        <App app={app} />
      </StrictMode>,
    );
  },
  (error: unknown) => {
    // Loading fails when the core was built from another schema than these bindings
    // (`undra bindgen`, `undra build`), or when the dev server cannot be reached (`undra dev`).
    root.render(<pre className="error">Undra did not start: {String(error)}</pre>);
  },
);
