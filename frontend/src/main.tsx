import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import "katex/dist/katex.min.css";
import "./styles/app.css";

import { App } from "./App";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { IpcClient } from "./ipc/client";
import { createTauriTransport } from "./ipc/transport";
import { StoreProvider } from "./state/store";

/**
 * The native shell draws the macOS traffic lights over the top-left corner.
 * The stylesheet reserves that space; outside the shell it collapses to the
 * normal gutter.
 */
const inNativeShell = Boolean(
  (globalThis as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__,
);
document.documentElement.dataset.shell = inNativeShell ? "tauri" : "browser";

/**
 * Capture failures that happen before React mounts (module evaluation, a
 * rejected promise). Without this the window is simply blank and the native
 * smoke harness has nothing to report.
 */
function recordFatal(kind: string, error: unknown): void {
  const message =
    error instanceof Error
      ? error.message
      : typeof error === "string"
        ? error
        : JSON.stringify(error);
  const stack = error instanceof Error ? (error.stack ?? null) : null;
  (globalThis as { __AICOUNCIL_ERROR__?: unknown }).__AICOUNCIL_ERROR__ = {
    kind,
    message,
    stack,
    at: new Date().toISOString(),
  };
  // eslint-disable-next-line no-console
  console.error(`[aicouncil] ${kind}`, error);
}

globalThis.addEventListener("error", (event: Event) => {
  const errorEvent = event as ErrorEvent;
  recordFatal("error", errorEvent.error ?? errorEvent.message);
});
globalThis.addEventListener("unhandledrejection", (event: PromiseRejectionEvent) => {
  recordFatal("unhandledrejection", event.reason);
});

const client = new IpcClient(createTauriTransport());

const container = document.getElementById("root");
if (!container) {
  recordFatal("startup", new Error("root container missing"));
} else {
  createRoot(container).render(
    <StrictMode>
      <ErrorBoundary>
        <StoreProvider client={client}>
          <App />
        </StoreProvider>
      </ErrorBoundary>
    </StrictMode>,
  );
}
