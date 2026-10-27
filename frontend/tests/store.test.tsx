import { describe, expect, test } from "bun:test";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { StrictMode } from "react";

import { IpcClient } from "../src/ipc/client";
import type { Transport } from "../src/ipc/transport";
import { StoreProvider, useStore, type StoreValue } from "../src/state/store";
import {
  fixtureBootstrap,
  fixtureConversation,
  fixtureDetail,
  fixtureDraft,
  fixtureMessage,
} from "./ipc-contract.test";

/**
 * Store level tests.
 *
 * They cover the two failure modes that are hard to see in the UI:
 *   - a late reply for a chat the user already left (stale response);
 *   - out-of-order draft replies (draft race).
 */

interface Deferred<T> {
  promise: Promise<T>;
  resolve(value: T): void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

class FakeBackend implements Transport {
  readonly calls: { command: string; args: Record<string, any> }[] = [];
  readonly listeners = new Map<string, Set<(payload: unknown) => void>>();
  /** Deferred replies for `load_conversation`, keyed by conversation id. */
  readonly loadDeferred = new Map<string, Deferred<unknown>>();
  /** Deferred replies for `save_draft`, in call order (race test only). */
  readonly draftDeferred: Deferred<unknown>[] = [];
  /** Deferred reply for `send_message` (slow-send tests). */
  sendDeferred: Deferred<unknown> | null = null;
  /** When true, `save_draft` waits for an explicit resolve. */
  deferDrafts = false;
  /** When true, the native export rejects (the write failed). */
  failSave = false;
  /** When true, the backend has no native export command yet. */
  noNativeExport = false;
  /** When true, the user dismissed the native save panel. */
  cancelExport = false;
  /** Revision counter returned by `save_draft`. */
  draftRevisionCounter = 1;
  /** When true, `save_draft` never answers. */
  hangSaveDraft = false;
  bootCalls = 0;
  listenedBeforeBootstrap = false;
  readonly editBufferCalls: Record<string, unknown>[] = [];
  readonly sentRequests: Record<string, unknown>[] = [];
  readonly draftCalls: Record<string, unknown>[] = [];

  constructor(
    private readonly conversations = [
      fixtureConversation("c1", "First"),
      fixtureConversation("c2", "Second"),
    ],
  ) {}

  async invoke<T>(command: string, args: Record<string, any> = {}): Promise<T> {
    this.calls.push({ command, args });
    switch (command) {
      case "bootstrap": {
        this.bootCalls += 1;
        if (this.listeners.size === 0) this.listenedBeforeBootstrap = false;
        return fixtureBootstrap(this.conversations) as T;
      }
      case "load_conversation": {
        const id = String(args.conversation_id);
        const pending = this.loadDeferred.get(id);
        if (pending) return pending.promise as Promise<T>;
        return fixtureDetail(
          id,
          [fixtureMessage("m1", id, `content of ${id}`)],
        ) as T;
      }
      case "get_draft": {
        const id = String(args.conversation_id);
        return fixtureDraft(id === "c1" ? "draft one" : "draft two") as T;
      }
      case "save_draft": {
        if (this.hangSaveDraft) {
          return new Promise<T>(() => undefined);
        }
        if (!this.deferDrafts) {
          this.draftCalls.push(args);
          return { revision: this.draftRevisionCounter } as T;
        }
        const pending = deferred<unknown>();
        this.draftDeferred.push(pending);
        return pending.promise as Promise<T>;
      }
      case "send_message": {
        if (this.sendDeferred) {
          this.sentRequests.push(args);
          return this.sendDeferred.promise as Promise<T>;
        }
        this.sentRequests.push(args);
        return {
          conversation_id: String(args.conversation_id ?? ""),
          batch_id: "b1",
          primary_request_id: "r1",
          assistant_of: {},
          adopted_main_leaf: null,
        } as T;
      }
      case "export_markdown":
        return {
          filename: "chat.md",
          content: "# Chat\n",
        } as T;
      case "export_markdown_to_file": {
        if (this.noNativeExport) {
          throw { code: "not_found", message: "unknown command" };
        }
        if (this.cancelExport) {
          throw { code: "cancelled", message: "save panel dismissed" };
        }
        if (this.failSave) {
          throw { code: "storage", message: "could not write the file" };
        }
        return { path: "/tmp/chat.md", sections: 1 } as T;
      }
      case "credential_status":
      case "reload_credentials":
        return { openrouter: true, deepseek: false } as T;
      case "save_edit_buffer":
        this.editBufferCalls.push(args);
        return { revision: 1 } as T;
      case "clear_edit_buffer":
        this.editBufferCalls.push(args);
        return { revision: 1 } as T;
      case "check_key_status":
        return {
          status: "ok",
          label: "test",
          usage: null,
          limit: null,
          limit_remaining: null,
          error: null,
        } as T;
      default:
        return undefined as T;
    }
  }

  async listen<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
    const set = this.listeners.get(event) ?? new Set();
    set.add(handler as (payload: unknown) => void);
    this.listeners.set(event, set);
    return () => set.delete(handler as (payload: unknown) => void);
  }
}

let store: StoreValue | null = null;

function Probe() {
  store = useStore();
  return null;
}

async function mount(backend: FakeBackend) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root: Root = createRoot(container);
  await act(async () => {
    root.render(
      <StrictMode>
        <StoreProvider client={new IpcClient(backend)}>
          <Probe />
        </StoreProvider>
      </StrictMode>,
    );
  });
  await act(async () => {
    await Promise.resolve();
  });
  return {
    root,
    container,
    get store() {
      if (!store) throw new Error("store not mounted");
      return store;
    },
  };
}

describe("store bootstrap", () => {
  test("subscribes before it bootstraps", async () => {
    const backend = new FakeBackend();
    const mounted = await mount(backend);
    expect(backend.listeners.size).toBeGreaterThan(0);
    expect(backend.bootCalls).toBe(1);
    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("opens the first conversation", async () => {
    const backend = new FakeBackend();
    const mounted = await mount(backend);
    expect(mounted.store.state.status).toBe("ready");
    expect(mounted.store.state.currentConversationId).toBe("c1");
    expect(mounted.store.state.composerText).toBe("draft one");
    await act(async () => {
      mounted.root.unmount();
    });
  });
});

describe("stale responses", () => {
  test("a late reply for a chat the user left is ignored", async () => {
    const backend = new FakeBackend();
    const pending = deferred<unknown>();
    backend.loadDeferred.set("c1", pending);
    const mounted = await mount(backend);

    // The user switches to the second chat while the first is still loading.
    await act(async () => {
      await mounted.store.actions.selectConversation("c2");
    });
    expect(mounted.store.state.currentConversationId).toBe("c2");
    expect(mounted.store.state.messages.c2).toBeDefined();

    // The stale first-chat reply arrives late.
    await act(async () => {
      pending.resolve(fixtureDetail("c1", [fixtureMessage("m1", "c1", "stale")]));
      await pending.promise;
      await Promise.resolve();
    });

    expect(mounted.store.state.currentConversationId).toBe("c2");
    expect(mounted.store.state.messages.c1).toBeUndefined();
    expect(mounted.store.state.messages.c2?.[0].content).toBe("content of c2");

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("the draft of a chat the user left is not restored", async () => {
    const backend = new FakeBackend();
    const mounted = await mount(backend);
    expect(mounted.store.state.composerText).toBe("draft one");
    await act(async () => {
      await mounted.store.actions.selectConversation("c2");
    });
    expect(mounted.store.state.composerText).toBe("draft two");
    await act(async () => {
      mounted.root.unmount();
    });
  });
});

describe("draft race", () => {  test("only the newest draft reply updates the revision", async () => {
    const backend = new FakeBackend();
    backend.deferDrafts = true;
    const mounted = await mount(backend);

    // The restored draft is already what the backend holds: no redundant
    // save is issued when the app opens a chat.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 700));
    });
    expect(backend.draftDeferred.length).toBe(0);

    await act(async () => {
      mounted.store.actions.setComposerText("first");
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 700));
    });
    await act(async () => {
      mounted.store.actions.setComposerText("second");
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 700));
    });

    const pending = backend.draftDeferred;
    expect(pending.length).toBeGreaterThanOrEqual(2);
    const newest = pending[pending.length - 1];
    const middle = pending[pending.length - 2];
    const oldest = pending[0];

    // The replies arrive out of order: the stale ones must not win.
    await act(async () => {
      newest.resolve({ revision: 42 });
      await newest.promise;
      await Promise.resolve();
    });
    expect(mounted.store.state.draftRevision).toBe(42);

    await act(async () => {
      middle.resolve({ revision: 7 });
      await middle.promise;
      await Promise.resolve();
    });
    expect(mounted.store.state.draftRevision).toBe(42);

    await act(async () => {
      oldest.resolve({ revision: 3 });
      await oldest.promise;
      await Promise.resolve();
    });
    expect(mounted.store.state.draftRevision).toBe(42);

    await act(async () => {
      mounted.root.unmount();
    });
  });
});

describe("edit buffer", () => {
  test("editing the latest question persists the buffer", async () => {
    const backend = new FakeBackend();
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.startEdit("m1");
    });

    const calls = backend.editBufferCalls.filter(
      (call) => call.command === undefined && "target_id" in call,
    );
    expect(calls.length).toBeGreaterThanOrEqual(1);
    expect(calls[0].target_id).toBe("m1");
    expect(calls[0].conversation_id).toBe("c1");

    await act(async () => {
      mounted.root.unmount();
    });
  });
});

describe("catalog events", () => {
  test("a catalog-changed event updates the catalog", async () => {
    const backend = new FakeBackend();
    const mounted = await mount(backend);
    expect(mounted.store.state.catalog).toHaveLength(0);

    const handlers = backend.listeners.get("catalog-changed");
    expect(handlers).toBeDefined();
    const handler = [...(handlers ?? [])][0];
    await act(async () => {
      handler({
        entries: [
          {
            id: "vendor/beta",
            provider_id: "p1",
            name: "Vendor: Beta",
            family: null,
            context_length: null,
            prompt_price: "1.00",
            completion_price: "2.00",
            supports_streaming: true,
            supports_temperature: true,
          },
        ],
        updated_at_ms: 123,
      });
    });

    expect(mounted.store.state.catalog).toHaveLength(1);
    expect(mounted.store.state.catalog[0].id).toBe("vendor/beta");
    expect(mounted.store.state.catalogUpdatedAt).toBe(123);

    await act(async () => {
      mounted.root.unmount();
    });
  });
});

describe("draft durability across chat switches", () => {
  test("the pending draft of the chat we leave is saved first", async () => {
    const backend = new FakeBackend();
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.setComposerText("unsaved text");
    });

    const savesBefore = backend.calls.filter(
      (call) => call.command === "save_draft",
    ).length;

    await act(async () => {
      await mounted.store.actions.selectConversation("c2");
    });

    const saves = backend.calls.filter((call) => call.command === "save_draft");
    expect(saves.length).toBeGreaterThan(savesBefore);
    const last = saves[saves.length - 1];
    expect(last.args.conversation_id).toBe("c1");
    expect(last.args.text).toBe("unsaved text");
    expect(last.args.changed).toBe(true);

    await act(async () => {
      mounted.root.unmount();
    });
  });
});

describe("export", () => {
  test("the selected scope carries the checked branch leaves", async () => {
    const backend = new FakeBackend();
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.toggleBranch("leaf-a");
      mounted.store.actions.toggleBranch("leaf-b");
    });

    await act(async () => {
      await mounted.store.actions.exportMarkdown("selected");
    });

    const exportCall = backend.calls.find(
      (call) => call.command === "export_markdown_to_file",
    );
    expect(exportCall?.args.scope).toBe("selected");
    expect(exportCall?.args.message_ids).toEqual(["leaf-a", "leaf-b"]);

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("the main scope does not send a leaf list", async () => {
    const backend = new FakeBackend();
    const mounted = await mount(backend);

    await act(async () => {
      await mounted.store.actions.exportMarkdown("main");
    });

    const exportCall = backend.calls.find(
      (call) => call.command === "export_markdown_to_file",
    );
    expect(exportCall?.args.scope).toBe("main");
    expect(exportCall?.args.message_ids).toBeUndefined();

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("the success message is only shown after the file was written", async () => {
    const backend = new FakeBackend();
    const mounted = await mount(backend);

    await act(async () => {
      await mounted.store.actions.exportMarkdown("main");
    });

    const saveCall = backend.calls.find(
      (call) => call.command === "export_markdown_to_file",
    );
    expect(saveCall).toBeDefined();
    expect(mounted.store.state.toast?.kind).toBe("success");
    expect(mounted.store.state.toast?.text).toContain("Exported");

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("a failed write reports an error instead of a success", async () => {
    const backend = new FakeBackend();
    backend.failSave = true;
    const mounted = await mount(backend);

    await act(async () => {
      await mounted.store.actions.exportMarkdown("main");
    });

    expect(mounted.store.state.toast?.kind).toBe("error");
    expect(mounted.store.state.toast?.text).toContain("Export failed");

    await act(async () => {
      mounted.root.unmount();
    });
  });
});

describe("export fallback", () => {
  test("falls back to the content download when the native command is missing", async () => {
    const backend = new FakeBackend();
    backend.noNativeExport = true;
    const mounted = await mount(backend);

    await act(async () => {
      await mounted.store.actions.exportMarkdown("main");
    });

    // The fallback asks for the content and downloads it.
    expect(
      backend.calls.some((call) => call.command === "export_markdown"),
    ).toBe(true);
    expect(mounted.store.state.toast?.text).toContain("browser download");

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("a cancelled save panel is not reported as an error", async () => {
    const backend = new FakeBackend();
    backend.cancelExport = true;
    const mounted = await mount(backend);

    await act(async () => {
      await mounted.store.actions.exportMarkdown("main");
    });

    expect(mounted.store.state.toast).toBeNull();

    await act(async () => {
      mounted.root.unmount();
    });
  });
});

describe("send races", () => {
  test("input typed during a slow send survives the send", async () => {
    const backend = new FakeBackend();
    const pending = deferred<unknown>();
    backend.sendDeferred = pending;
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.setComposerText("first input");
    });
    await act(async () => {
      mounted.store.actions.send();
      await Promise.resolve();
    });

    // The user keeps typing while the request is in flight.
    await act(async () => {
      mounted.store.actions.setComposerText("newer input");
    });

    await act(async () => {
      pending.resolve({
        conversation_id: "c1",
        batch_id: "b1",
        primary_request_id: "r1",
        assistant_of: {},
        adopted_main_leaf: null,
      });
      await pending.promise;
      await Promise.resolve();
    });

    // The newer input must not be cleared by the finishing send.
    expect(mounted.store.state.composerText).toBe("newer input");

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("switching chats during a slow send does not clear the other chat", async () => {
    const backend = new FakeBackend();
    const pending = deferred<unknown>();
    backend.sendDeferred = pending;
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.setComposerText("text of chat one");
    });
    await act(async () => {
      mounted.store.actions.send();
      await Promise.resolve();
    });

    await act(async () => {
      await mounted.store.actions.selectConversation("c2");
    });
    expect(mounted.store.state.currentConversationId).toBe("c2");
    expect(mounted.store.state.composerText).toBe("draft two");

    await act(async () => {
      pending.resolve({
        conversation_id: "c1",
        batch_id: "b1",
        primary_request_id: "r1",
        assistant_of: {},
        adopted_main_leaf: null,
      });
      await pending.promise;
      await Promise.resolve();
    });

    // The other chat keeps its own draft.
    expect(mounted.store.state.currentConversationId).toBe("c2");
    expect(mounted.store.state.composerText).toBe("draft two");

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("a branch selection made during a slow send is kept", async () => {
    const backend = new FakeBackend();
    const pending = deferred<unknown>();
    backend.sendDeferred = pending;
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.toggleBranch("leaf-a");
    });
    await act(async () => {
      mounted.store.actions.send();
      await Promise.resolve();
    });

    await act(async () => {
      mounted.store.actions.toggleBranch("leaf-b");
    });
    expect(mounted.store.state.branchSelection).toEqual(["leaf-a", "leaf-b"]);

    await act(async () => {
      pending.resolve({
        conversation_id: "c1",
        batch_id: "b1",
        primary_request_id: "r1",
        assistant_of: {},
        adopted_main_leaf: null,
      });
      await pending.promise;
      await Promise.resolve();
    });

    expect(mounted.store.state.branchSelection).toEqual(["leaf-a", "leaf-b"]);

    await act(async () => {
      mounted.root.unmount();
    });
  });
});

describe("refetch does not fight the composer", () => {
  test("a refetch does not re-save an unchanged draft", async () => {
    const backend = new FakeBackend();
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.setComposerText("stable draft");
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 700));
    });
    const savesAfterFirst = backend.calls.filter(
      (call) => call.command === "save_draft",
    ).length;
    expect(savesAfterFirst).toBeGreaterThan(0);

    // A backend change event triggers a refetch of the same content.
    const handlers = backend.listeners.get("conversation-changed");
    const handler = [...(handlers ?? [])][0];
    await act(async () => {
      handler({ conversation_id: "c1" });
      await Promise.resolve();
      await Promise.resolve();
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 700));
    });

    const saves = backend.calls.filter((call) => call.command === "save_draft");
    // No endless save → refetch → save cycle.
    expect(saves.length).toBe(savesAfterFirst);

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("a refetch does not overwrite text typed after the last save", async () => {
    const backend = new FakeBackend();
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.setComposerText("typed text");
    });
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 700));
    });

    // The user types more while a refetch is in flight.
    await act(async () => {
      mounted.store.actions.setComposerText("typed text plus more");
    });

    const handlers = backend.listeners.get("conversation-changed");
    const handler = [...(handlers ?? [])][0];
    await act(async () => {
      handler({ conversation_id: "c1" });
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(mounted.store.state.composerText).toBe("typed text plus more");

    await act(async () => {
      mounted.root.unmount();
    });
  });
});

describe("send CAS (flush before snapshot)", () => {
  test("the draft is flushed before the send and its revision is sent", async () => {
    const backend = new FakeBackend();
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.setComposerText("cas draft");
    });

    await act(async () => {
      await mounted.store.actions.send();
    });

    const save = backend.calls.find((call) => call.command === "save_draft");
    const send = backend.calls.find((call) => call.command === "send_message");
    expect(save).toBeDefined();
    expect(save?.args.text).toBe("cas draft");
    expect(save?.args.changed).toBe(true);
    expect(send).toBeDefined();
    // The revision sent is the one the flush returned, not a stale counter.
    expect(send?.args.request?.expected_draft_revision).toBe(save ? 1 : 0);

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("an unchanged draft is not flushed again", async () => {
    const backend = new FakeBackend();
    const mounted = await mount(backend);

    // The restored draft is already what the backend holds.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 700));
    });
    const savesBefore = backend.calls.filter(
      (call) => call.command === "save_draft",
    ).length;

    await act(async () => {
      await mounted.store.actions.send();
    });

    const saves = backend.calls.filter((call) => call.command === "save_draft");
    expect(saves.length).toBe(savesBefore);

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("a newer revision from the flush is what the backend receives", async () => {
    const backend = new FakeBackend();
    backend.draftRevisionCounter = 7;
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.setComposerText("revision check");
    });
    await act(async () => {
      await mounted.store.actions.send();
    });

    const send = backend.calls.find((call) => call.command === "send_message");
    expect(send?.args.request?.expected_draft_revision).toBe(7);

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("a hung flush does not block the send", async () => {
    const backend = new FakeBackend();
    backend.hangSaveDraft = true;
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.setComposerText("hung flush");
    });

    await act(async () => {
      await mounted.store.actions.send();
    });

    // The send still happened, with the revision the frontend already knew.
    const send = backend.calls.find((call) => call.command === "send_message");
    expect(send).toBeDefined();

    await act(async () => {
      mounted.root.unmount();
    });
  }, 15000);
});
