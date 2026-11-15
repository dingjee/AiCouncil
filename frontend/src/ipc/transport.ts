/**
 * Transport abstraction over the Tauri IPC bridge.
 *
 * The production transport talks to the Tauri 2 webview bridge; tests and the
 * browser mock harness inject their own implementation. Nothing else in the
 * app is allowed to reach for `window.__TAURI__` directly.
 */

export type InvokeArgs = Record<string, unknown>;

export interface Transport {
  invoke<T>(command: string, args?: InvokeArgs): Promise<T>;
  listen<T>(event: string, handler: (payload: T) => void): Promise<() => void>;
}

interface TauriInternals {
  invoke?: (cmd: string, args?: unknown) => Promise<unknown>;
  transformCallback?: (cb: (payload: unknown) => void, once?: boolean) => number;
}

interface TauriGlobal {
  core?: {
    invoke?: (cmd: string, args?: unknown) => Promise<unknown>;
  };
  event?: {
    listen?: (
      event: string,
      handler: (e: { payload: unknown }) => void,
    ) => Promise<() => void>;
  };
}

function tauriInternals(): TauriInternals | undefined {
  return (globalThis as { __TAURI_INTERNALS__?: TauriInternals })
    .__TAURI_INTERNALS__;
}

function tauriGlobal(): TauriGlobal | undefined {
  return (globalThis as { __TAURI__?: TauriGlobal }).__TAURI__;
}

/**
 * Production transport.
 *
 * Tauri 2 exposes `__TAURI_INTERNALS__.invoke` for every command and
 * `plugin:event|listen` for events. The global `__TAURI__` namespace is used
 * as a fallback when the app is configured with `withGlobalTauri`.
 */
export function createTauriTransport(): Transport {
  return {
    async invoke<T>(command: string, args?: InvokeArgs): Promise<T> {
      const internals = tauriInternals();
      const globalApi = tauriGlobal();
      const fn = internals?.invoke ?? globalApi?.core?.invoke;
      if (!fn) {
        throw new Error(
          "Tauri IPC bridge unavailable: the app is not running inside the native shell.",
        );
      }
      return (await fn(command, args)) as T;
    },
    async listen<T>(
      event: string,
      handler: (payload: T) => void,
    ): Promise<() => void> {
      const globalApi = tauriGlobal();
      const globalListen = globalApi?.event?.listen;
      if (globalListen) {
        return globalListen(event, (e) => handler(e.payload as T));
      }
      const internals = tauriInternals();
      if (!internals?.invoke || !internals.transformCallback) {
        throw new Error("Tauri event bridge unavailable.");
      }
      /**
       * Tauri 2 hands the callback the **event envelope**
       * `{ event, id, payload }` (see `runCallback` in tauri's event module),
       * not the payload itself. Unwrap it here so every handler receives the
       * bare payload the IPC contract promises — without this, the native
       * shell silently drops every event (streaming deltas, conversation
       * changes, …) while the browser mock keeps working.
       */
      const handlerId = internals.transformCallback((incoming) => {
        const envelope = incoming as { payload?: unknown } | null;
        handler(
          (envelope && typeof envelope === "object" && "payload" in envelope
            ? envelope.payload
            : incoming) as T,
        );
      });
      await internals.invoke("plugin:event|listen", {
        event,
        target: { kind: "Any" },
        handler: handlerId,
      });
      return () => {
        void internals.invoke?.("plugin:event|unlisten", {
          event,
          eventId: handlerId,
        });
      };
    },
  };
}

/** Thrown when the backend answers with an error payload. */
export class IpcCommandError extends Error {
  readonly code: string | undefined;
  readonly detail: string | undefined;

  constructor(message: string, code?: string, detail?: string) {
    super(message);
    this.name = "IpcCommandError";
    this.code = code;
    this.detail = detail;
  }
}

/**
 * Normalise the several error shapes a Tauri command may produce into a
 * single `Error` carrying a readable message and the backend `code`.
 */
export function toError(value: unknown): Error {
  if (value instanceof Error) return value;
  if (typeof value === "string") return new Error(value);
  if (value && typeof value === "object") {
    const record = value as Record<string, unknown>;
    const message =
      typeof record.message === "string"
        ? record.message
        : typeof record.error === "string"
          ? record.error
          : JSON.stringify(value);
    const code = typeof record.code === "string" ? record.code : undefined;
    const detail = typeof record.detail === "string" ? record.detail : undefined;
    return new IpcCommandError(message, code, detail);
  }
  return new Error(String(value));
}
