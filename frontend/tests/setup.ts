/**
 * Test bootstrap: registers a DOM so component tests can run under `bun test`.
 */

import { GlobalRegistrator } from "@happy-dom/global-registrator";

if (!(globalThis as { document?: unknown }).document) {
  GlobalRegistrator.register({ url: "http://localhost/" });
}

// React 18 needs this flag to accept `act()` from react-dom/test-utils.
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// KaTeX refuses to render in quirks mode; make the test document report
// standards mode so the renderer does not warn.
if (typeof document !== "undefined") {
  try {
    Object.defineProperty(document, "compatMode", {
      value: "CSS1Compat",
      configurable: true,
    });
  } catch {
    // A read-only compatMode is fine; the warning is harmless.
  }
}

export {};
