import { afterEach, describe, expect, test } from "bun:test";

import { createTauriTransport } from "../src/ipc/transport";

/**
 * Regression: the native shell delivers events through
 * `__TAURI_INTERNALS__` as the **envelope** `{ event, id, payload }`
 * (see `runCallback` in tauri's event module). The transport must hand
 * handlers the bare payload — before the fix, every chat event was
 * silently dropped and streaming never rendered.
 */

interface FakeInternals {
  invoke: (cmd: string, args?: unknown) => Promise<unknown>;
  transformCallback: (cb: (payload: unknown) => void, once?: boolean) => number;
}

function installInternals(): {
  callbacks: Map<number, (payload: unknown) => void>;
  listened: { event: string; handler: number }[];
  unlistened: number[];
} {
  const callbacks = new Map<number, (payload: unknown) => void>();
  const listened: { event: string; handler: number }[] = [];
  const unlistened: number[] = [];
  let nextId = 1;
  const internals: FakeInternals = {
    invoke: (async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "plugin:event|listen") {
        listened.push({
          event: String(args?.event ?? ""),
          handler: Number(args?.handler ?? 0),
        });
      }
      if (cmd === "plugin:event|unlisten") {
        unlistened.push(Number(args?.eventId ?? 0));
      }
      return undefined;
    }) as FakeInternals["invoke"],
    transformCallback: (cb) => {
      const id = nextId++;
      callbacks.set(id, cb);
      return id;
    },
  };
  (globalThis as { __TAURI_INTERNALS__?: FakeInternals }).__TAURI_INTERNALS__ =
    internals;
  return { callbacks, listened, unlistened };
}

function removeInternals(): void {
  delete (globalThis as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  delete (globalThis as { __TAURI__?: unknown }).__TAURI__;
}

const cleanupFns: (() => void)[] = [];
afterEach(() => {
  cleanupFns.splice(0).forEach((off) => off());
  removeInternals();
});

describe("tauri transport", () => {
  test("unwraps the tauri event envelope for internals listeners", async () => {
    const { callbacks, listened } = installInternals();
    const transport = createTauriTransport();

    const received: unknown[] = [];
    const off = await transport.listen<{ value: number }>("chat-event", (payload) =>
      received.push(payload),
    );
    cleanupFns.push(off);

    expect(listened).toEqual([{ event: "chat-event", handler: 1 }]);

    // Tauri delivers the envelope; the handler must see the payload only.
    const deliver = callbacks.get(listened[0].handler);
    expect(deliver).toBeDefined();
    deliver!({
      event: "chat-event",
      id: 7,
      payload: {
        conversation_id: "c1",
        request_id: "r1",
        assistant_message_id: "m1",
        event: { type: "text_delta", delta: "hello" },
      },
    });

    expect(received).toEqual([
      {
        conversation_id: "c1",
        request_id: "r1",
        assistant_message_id: "m1",
        event: { type: "text_delta", delta: "hello" },
      },
    ]);
  });

  test("still unwraps .payload for the global __TAURI__ listener", async () => {
    installInternals();
    const globalListeners: ((e: { payload: unknown }) => void)[] = [];
    (globalThis as { __TAURI__?: unknown }).__TAURI__ = {
      event: {
        listen: async (
          _event: string,
          handler: (e: { payload: unknown }) => void,
        ) => {
          globalListeners.push(handler);
          return () => undefined;
        },
      },
    };
    const transport = createTauriTransport();

    const received: unknown[] = [];
    await transport.listen("conversation-changed", (payload) =>
      received.push(payload),
    );

    expect(globalListeners).toHaveLength(1);
    globalListeners[0]({ payload: { conversation_id: "c1" } });
    expect(received).toEqual([{ conversation_id: "c1" }]);
  });

  test("unlisten removes the internals registration", async () => {
    const { listened, unlistened } = installInternals();
    const transport = createTauriTransport();

    const off = await transport.listen("chat-event", () => undefined);
    expect(listened).toHaveLength(1);
    expect(unlistened).toHaveLength(0);

    off();
    expect(unlistened).toEqual([listened[0].handler]);
  });
});
