import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import "katex/dist/katex.min.css";
import "../src/styles/app.css";

import { App } from "../src/App";
import { createMockTransport } from "./mock-transport";
import type { FixtureName } from "./fixtures";
import { IpcClient } from "../src/ipc/client";
import { StoreProvider } from "../src/state/store";

document.documentElement.dataset.shell = "browser";

/**
 * The fixture is chosen with `?fixture=long`; the geometry harness uses it
 * to get a long synthetic imported-style chat without any real user data.
 */
const requested = new URLSearchParams(window.location.search).get("fixture");
const fixture: FixtureName = requested === "long" ? "long" : "default";

const client = new IpcClient(createMockTransport(fixture));

const container = document.getElementById("root");
if (!container) throw new Error("root container missing");

createRoot(container).render(
  <StrictMode>
    <StoreProvider client={client}>
      <App />
    </StoreProvider>
  </StrictMode>,
);
