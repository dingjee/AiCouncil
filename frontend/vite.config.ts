import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { resolve } from "node:path";

/**
 * Tauri 2 frontend build.
 *
 * - Production entry is `index.html` only; the browser mock harness lives in
 *   `dev/` and is never part of the shipped bundle (no dev server, no mock
 *   backend, no test mode in the release artifact).
 * - `base: "./"` keeps asset URLs relative so the bundle also loads from the
 *   custom protocol / file path used by the macOS bundle.
 */
export default defineConfig(({ mode }) => {
  const isMock = mode === "mock";
  return {
    base: "./",
    plugins: [react()],
    clearScreen: false,
    server: {
      port: 5173,
      strictPort: false,
    },
    build: {
      outDir: "dist",
      emptyOutDir: true,
      target: "es2022",
      sourcemap: false,
      // The bundle is loaded from the local disk by the native shell; a
      // single chunk avoids a request waterfall.
      chunkSizeWarningLimit: 900,
      // KaTeX ships its own fonts; keep them as real assets (no CDN at runtime).
      assetsInlineLimit: 4096,
      rollupOptions: {
        input: isMock
          ? resolve(import.meta.dirname, "dev/index.html")
          : resolve(import.meta.dirname, "index.html"),
      },
    },
  };
});
