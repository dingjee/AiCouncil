import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import "katex/dist/katex.min.css";
import "../src/styles/app.css";

import { App } from "../src/App";
import { createMockTransport } from "./mock-transport";
import { IpcClient } from "../src/ipc/client";
import { StoreProvider } from "../src/state/store";

document.documentElement.dataset.shell = "browser";

const client = new IpcClient(createMockTransport());

const container = document.getElementById("root");
if (!container) throw new Error("root container missing");

createRoot(container).render(
  <StrictMode>
    <StoreProvider client={client}>
      <App />
    </StoreProvider>
  </StrictMode>,
);
