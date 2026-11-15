import { describe, expect, test } from "bun:test";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { StrictMode } from "react";

import { IpcClient } from "../src/ipc/client";
import type { Transport } from "../src/ipc/transport";
import type { Attachment, Draft } from "../src/ipc/types";
import { StoreProvider, useStore, type StoreValue } from "../src/state/store";
import {
  fixtureBootstrap,
  fixtureConversation,
  fixtureDetail,
  fixtureMessage,
  fixturePreferences,
} from "./ipc-contract.test";

/**
 * Composer lifecycle regressions.
 *
 * The backend keeps the draft in the `conversations` row
 * (`draft` / `draft_attachments_json`) and counts every **real** content
 * change in `draft_revision`. The send transaction consumes the draft with
 * a compare-and-swap on that revision: the row is only cleared when the
 * revision still equals the one the frontend froze with the snapshot.
 *
 * These tests therefore run against a **stateful** backend that behaves
 * like `src/storage/mod.rs`:
 *
 *   - `save_draft` bumps the revision when the content differs or when the
 *     caller says the change is real (`changed = true`);
 *   - `send_message` clears the draft only when the revision still matches
 *     `expected_draft_revision` — and the clear does **not** bump the
 *     revision;
 *   - the reply of a command can be delayed (deferred promises) so the test
 *     controls the order in which the frontend sees the answers.
 *
 * A backend with a fixed revision could not show the race at all: the
 * whole point is that a *second* write of the same content moves the
 * revision and breaks the compare-and-swap of the send.
 */

interface Deferred<T> {
  promise: Promise<T>;
  resolve(value: T): void;
  reject(error: unknown): void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** The authoritative draft row of one conversation (the backend's truth). */
interface Row {
  text: string;
  attachments: Attachment[];
  editText: string;
  editAttachments: Attachment[];
  editTarget: string | null;
  revision: number;
}

function sameAttachments(
  a: Attachment[] | undefined,
  b: Attachment[] | undefined,
): boolean {
  const left = a ?? [];
  const right = b ?? [];
  if (left.length !== right.length) return false;
  return left.every(
    (item, index) =>
      item.name === right[index].name && item.content === right[index].content,
  );
}

const ATTACHMENT: Attachment = { name: "notes.txt", content: "attached body" };

class DraftBackend implements Transport {
  readonly listeners = new Map<string, Set<(payload: unknown) => void>>();
  readonly calls: { command: string; args: Record<string, any> }[] = [];
  readonly rows = new Map<string, Row>();
  /** Every `save_draft` payload (call order). */
  readonly draftWrites: {
    conversation_id: string;
    text: string;
    attachments: Attachment[];
    changed: boolean;
  }[] = [];
  /** Every `send_message` payload (call order). */
  readonly sends: Record<string, any>[] = [];
  /** Deferred `get_draft` replies (call order). */
  readonly draftReads: Deferred<unknown>[] = [];
  /** Deferred `load_conversation` replies (call order). */
  readonly loads: Deferred<unknown>[] = [];
  /** The deferred reply of the running `send_message`. */
  sendDeferred: Deferred<unknown> | null = null;
  /** The deferred reply of the first `save_draft` call. */
  firstSave: Deferred<unknown> | null = null;
  /** When true, `get_draft` waits for an explicit resolve. */
  deferDraftReads = false;
  /** When true, `load_conversation` waits for an explicit resolve. */
  deferLoads = false;
  /** When true, the next `save_draft` call never answers. */
  hangSave = false;
  /** The deferred replies of the hanging `save_draft` calls (they land later). */
  readonly hungSaves: Deferred<unknown>[] = [];
  /** The deferred reply of the next `save_edit_buffer` call. */
  editSave: Deferred<unknown> | null = null;
  /** When true, `send_message` is rejected. */
  failSend = false;
  /** When true, `load_conversation` is rejected. */
  failLoad = false;
  /** When true, the compare-and-swap of the edit send loses the race. */
  breakEditCas = false;
  /** The verdict of the network region check. */
  egressVerdict: "allowed" | "blocked" | "unknown" = "allowed";
  /** The preferences the bootstrap answers with. */
  preferences = fixturePreferences();

  constructor(
    private readonly conversations = [
      fixtureConversation("c1", "First"),
      fixtureConversation("c2", "Second"),
    ],
  ) {}

  row(conversationId: string): Row {
    let row = this.rows.get(conversationId);
    if (!row) {
      row = {
        text: "",
        attachments: [],
        editText: "",
        editAttachments: [],
        editTarget: null,
        revision: 0,
      };
      this.rows.set(conversationId, row);
    }
    return row;
  }

  /** The number of writes that claimed a real content change. */
  changedWrites(conversationId: string): number {
    return this.draftWrites.filter(
      (write) =>
        write.conversation_id === conversationId && write.changed === true,
    ).length;
  }

  draft(conversationId: string): Draft {
    const row = this.row(conversationId);
    return {
      text: row.text,
      attachments: row.attachments,
      edit_text: row.editText,
      edit_attachments: row.editAttachments,
      edit_target_id: row.editTarget,
      revision: row.revision,
    };
  }

  detail(conversationId: string) {
    const base = fixtureDetail(conversationId, [
      {
        ...fixtureMessage("q1", conversationId, "Question?"),
        role: "user",
      },
      fixtureMessage("a1", conversationId, "Answer"),
    ]);
    return {
      ...base,
      edit_latest: {
        target_id: "q1",
        content: "Question?",
        attachments: [] as Attachment[],
      },
    };
  }

  async invoke<T>(command: string, args: Record<string, any> = {}): Promise<T> {
    this.calls.push({ command, args });
    switch (command) {
      case "bootstrap":
        return {
          ...fixtureBootstrap(this.conversations),
          preferences: this.preferences,
        } as T;

      case "load_conversation": {
        const id = String(args.conversation_id);
        if (this.deferLoads) {
          const pending = deferred<unknown>();
          this.loads.push(pending);
          return pending.promise as Promise<T>;
        }
        if (this.failLoad) {
          throw { code: "storage", message: "could not read the chat" };
        }
        return this.detail(id) as T;
      }

      case "get_draft": {
        const id = String(args.conversation_id);
        if (this.deferDraftReads) {
          const pending = deferred<unknown>();
          this.draftReads.push(pending);
          return pending.promise as Promise<T>;
        }
        return this.draft(id) as T;
      }

      case "save_draft": {
        const id = String(args.conversation_id);
        const text = String(args.text ?? "");
        const attachments = (args.attachments as Attachment[]) ?? [];
        const changed = Boolean(args.changed);
        this.draftWrites.push({
          conversation_id: id,
          text,
          attachments,
          changed,
        });
        // The write lands in the row (the backend counts a real change).
        const apply = () => {
          const row = this.row(id);
          const differs =
            row.text !== text || !sameAttachments(row.attachments, attachments);
          if (!differs && !changed) return { revision: row.revision } as T;
          row.text = text;
          row.attachments = attachments;
          if (differs || changed) row.revision += 1;
          return { revision: row.revision } as T;
        };
        if (this.hangSave) {
          // The command is processed but its answer never arrives — the
          // write itself can still land in the database later on.
          const pending = deferred<unknown>();
          this.hungSaves.push(pending);
          return pending.promise.then(() => apply()) as Promise<T>;
        }
        if (this.firstSave) {
          // The command is processed, but its answer is delayed.
          const pending = this.firstSave;
          this.firstSave = null;
          return pending.promise.then(() => apply()) as Promise<T>;
        }
        return apply();
      }

      case "save_edit_buffer": {
        const id = String(args.conversation_id);
        const text = String(args.text ?? "");
        const attachments = (args.attachments as Attachment[]) ?? [];
        const changed = Boolean(args.changed);
        const apply = () => {
          const row = this.row(id);
          const differs =
            row.editText !== text ||
            !sameAttachments(row.editAttachments, attachments) ||
            row.editTarget !== (args.target_id ?? null);
          if (!differs && !changed) return { revision: row.revision } as T;
          row.editText = text;
          row.editAttachments = attachments;
          row.editTarget = (args.target_id ?? null) as string | null;
          if (differs || changed) row.revision += 1;
          return { revision: row.revision } as T;
        };
        if (this.editSave) {
          const pending = this.editSave;
          this.editSave = null;
          return pending.promise.then(() => apply()) as Promise<T>;
        }
        return apply();
      }

      case "clear_edit_buffer": {
        const id = String(args.conversation_id);
        const row = this.row(id);
        if (!row.editText && row.editAttachments.length === 0 && !row.editTarget) {
          return { revision: row.revision } as T;
        }
        row.editText = "";
        row.editAttachments = [];
        row.editTarget = null;
        row.revision += 1;
        return { revision: row.revision } as T;
      }

      case "send_message": {
        const request = args.request as Record<string, any>;
        this.sends.push(request);
        // The command is processed after the IPC round trip: the compare-and-swap
        // of the draft happens here, not when the request was made.
        if (this.sendDeferred) {
          await this.sendDeferred.promise;
        }
        if (this.failSend) {
          throw { code: "storage", message: "send rejected" };
        }
        const row = this.row(String(request.conversation_id));
        const expected = Number(request.expected_draft_revision ?? 0);
        if (row.revision === expected && !(request.mode === "edit" && this.breakEditCas)) {
          if (request.mode === "edit") {
            row.editText = "";
            row.editAttachments = [];
            row.editTarget = null;
          } else {
            row.text = "";
            row.attachments = [];
          }
        }
        return {
          conversation_id: String(request.conversation_id),
          batch_id: "b1",
          primary_request_id: "r1",
          assistant_of: {},
          adopted_main_leaf: null,
        } as T;
      }

      case "list_preferences":
        return this.preferences as T;

      case "check_egress":
        return {
          verdict: this.egressVerdict,
          country_code: "US",
          detail: this.egressVerdict === "allowed" ? "" : "blocked region",
        } as T;

      default:
        return undefined as T;
    }
  }

  async listen<T>(
    event: string,
    handler: (payload: T) => void,
  ): Promise<() => void> {
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

async function mount(backend: DraftBackend) {
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
      if (!store) throw new Error("store is not mounted");
      return store;
    },
  };
}

/** Let the 600 ms autosave timer fire. */
async function tick(ms = 800) {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, ms));
  });
}

/** Release a deferred `send_message` and let the receipt land. */
async function releaseSend(backend: DraftBackend) {
  const pending = backend.sendDeferred;
  if (!pending) throw new Error("no deferred send");
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
    await Promise.resolve();
  });
}

/** Fire the backend event that triggers a refetch of the open chat. */
async function emitConversationChanged(backend: DraftBackend, conversationId: string) {
  const handlers = backend.listeners.get("conversation-changed");
  expect(handlers).toBeDefined();
  for (const handler of handlers ?? []) {
    await act(async () => {
      (handler as (payload: unknown) => void)({ conversation_id: conversationId });
      await Promise.resolve();
    });
  }
}

describe("composer lifecycle: a successful send consumes the draft", () => {
  test("the submitted text and attachments disappear and stay gone", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "old draft";
    const mounted = await mount(backend);
    expect(mounted.store.state.composerText).toBe("old draft");

    await act(async () => {
      mounted.store.actions.setComposerText("hello world");
      mounted.store.actions.setComposerAttachments([ATTACHMENT]);
    });

    await act(async () => {
      await mounted.store.actions.send();
    });

    // The payload carried exactly what the composer held.
    expect(backend.sends).toHaveLength(1);
    expect(backend.sends[0].prompt).toBe("hello world");
    expect(backend.sends[0].attachments).toEqual([ATTACHMENT]);
    expect(backend.sends[0].mode).toBe("normal");

    // The composer is empty and the backend row was consumed.
    expect(mounted.store.state.composerText).toBe("");
    expect(mounted.store.state.composerAttachments).toEqual([]);
    expect(backend.row("c1").text).toBe("");
    expect(backend.row("c1").attachments).toEqual([]);

    // Switching away and back does not revive the consumed snapshot.
    await act(async () => {
      await mounted.store.actions.selectConversation("c2");
    });
    await act(async () => {
      await mounted.store.actions.selectConversation("c1");
    });
    expect(mounted.store.state.composerText).toBe("");
    expect(backend.row("c1").text).toBe("");

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("a fast send across the autosave window writes the snapshot only once", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "old draft";
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.setComposerText("quick question");
    });

    // The send is in flight while the 600 ms autosave timer fires.
    const pending = deferred<unknown>();
    backend.sendDeferred = pending;
    await act(async () => {
      void mounted.store.actions.send();
      await Promise.resolve();
    });

    await tick();

    // The same snapshot must not be written a second time with `changed`.
    expect(backend.changedWrites("c1")).toBe(1);

    await releaseSend(backend);
    backend.sendDeferred = null;

    // The compare-and-swap of the send consumed the draft.
    expect(backend.row("c1").text).toBe("");
    expect(mounted.store.state.composerText).toBe("");

    // A later refresh does not refill the composer.
    await emitConversationChanged(backend, "c1");
    await tick();
    expect(mounted.store.state.composerText).toBe("");
    expect(backend.row("c1").text).toBe("");

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("an autosave that is already running does not write the snapshot twice", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "old draft";
    const mounted = await mount(backend);

    // The autosave starts, but its answer is delayed.
    const slowSave = deferred<unknown>();
    backend.firstSave = slowSave;

    await act(async () => {
      mounted.store.actions.setComposerText("racing snapshot");
    });
    await tick();

    // The send starts while the save is still in flight.
    await act(async () => {
      void mounted.store.actions.send();
      await Promise.resolve();
    });

    // The delayed save finally lands.
    await act(async () => {
      slowSave.resolve({ revision: backend.row("c1").revision + 1 });
      await slowSave.promise;
      await Promise.resolve();
      await Promise.resolve();
    });
    await tick();

    // One real write for the snapshot, and the send consumed it.
    expect(backend.changedWrites("c1")).toBe(1);
    expect(backend.row("c1").text).toBe("");
    expect(mounted.store.state.composerText).toBe("");

    await act(async () => {
      mounted.root.unmount();
    });
  });
});

describe("composer lifecycle: failures keep the draft", () => {
  test("a rejected send keeps the text and the attachments", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "old draft";
    backend.failSend = true;
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.setComposerText("never sent");
      mounted.store.actions.setComposerAttachments([ATTACHMENT]);
    });

    await act(async () => {
      await mounted.store.actions.send();
    });

    expect(backend.sends).toHaveLength(1);
    expect(mounted.store.state.composerText).toBe("never sent");
    expect(mounted.store.state.composerAttachments).toEqual([ATTACHMENT]);
    expect(mounted.store.state.toast?.kind).toBe("error");
    expect(mounted.store.state.toast?.text).toContain("Send failed");

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("the network region gate keeps the draft and sends nothing", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "old draft";
    backend.preferences = [
      {
        provider_id: "p1",
        model_id: "anthropic/claude-sonnet",
        enabled: true,
        sort_order: 0,
        is_default: true,
        overrides: null,
      },
    ];
    backend.egressVerdict = "blocked";
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.setComposerText("gated question");
    });
    await act(async () => {
      await mounted.store.actions.send();
    });

    // The gate blocks the request: nothing is sent and nothing is cleared.
    expect(backend.sends).toHaveLength(0);
    expect(mounted.store.state.composerText).toBe("gated question");
    expect(mounted.store.state.egressModal).not.toBeNull();

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("a successful send with a failing refresh is not reported as a failure", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "old draft";
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.setComposerText("sent fine");
    });

    // The refresh behind the send fails, the send itself succeeds.
    backend.failLoad = true;
    await act(async () => {
      await mounted.store.actions.send();
    });

    expect(backend.sends).toHaveLength(1);
    expect(mounted.store.state.composerText).toBe("");
    expect(mounted.store.state.toast?.text ?? "").not.toContain("Send failed");

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("a timed out flush does not block the send and a late write does not revive it", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "old draft";
    const mounted = await mount(backend);

    // The flush never answers.
    backend.hangSave = true;

    await act(async () => {
      mounted.store.actions.setComposerText("slow backend");
    });

    const started = Date.now();
    await act(async () => {
      await mounted.store.actions.send();
    });
    // The send did not wait forever for the flush.
    expect(Date.now() - started).toBeLessThan(9000);
    expect(backend.sends).toHaveLength(1);
    expect(mounted.store.state.composerText).toBe("");

    // The late write lands in the database: it must not bring the consumed
    // text back (neither in the row nor in the composer).
    backend.hangSave = false;
    const late = backend.hungSaves[0];
    expect(late).toBeDefined();
    await act(async () => {
      late.resolve({ revision: 1 });
      await late.promise;
      await Promise.resolve();
      await Promise.resolve();
    });
    await tick();
    // The other hanging writes of the queue land as well.
    await act(async () => {
      for (const pending of backend.hungSaves) pending.resolve({ revision: 1 });
      await Promise.resolve();
      await Promise.resolve();
    });
    await tick();
    await emitConversationChanged(backend, "c1");
    await tick();

    expect(mounted.store.state.composerText).toBe("");
    expect(backend.row("c1").text).toBe("");

    await act(async () => {
      mounted.root.unmount();
    });
  }, 30000);
  test("a save that lands after the send does not refill the composer", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "old draft";
    const mounted = await mount(backend);

    // The first save of the draft hangs: the backend answers late.
    backend.hangSave = true;

    await act(async () => {
      mounted.store.actions.setComposerText("first version");
    });
    await tick();

    // The user replaces the text and sends it.
    await act(async () => {
      mounted.store.actions.setComposerText("second version");
    });
    await act(async () => {
      await mounted.store.actions.send();
    });

    // The send was accepted and the composer is empty.
    expect(backend.sends).toHaveLength(1);
    expect(backend.sends[0].prompt).toBe("second version");
    expect(mounted.store.state.composerText).toBe("");

    // The late save lands in the database now: it must neither come back
    // into the composer nor stay in the row.
    backend.hangSave = false;
    await act(async () => {
      for (const pending of backend.hungSaves) pending.resolve({ revision: 1 });
      await Promise.resolve();
      await Promise.resolve();
    });
    await tick();
    await tick();

    expect(mounted.store.state.composerText).toBe("");
    expect(backend.row("c1").text).toBe("");

    // A refresh does not bring the old version back either.
    await emitConversationChanged(backend, "c1");
    await tick();
    expect(mounted.store.state.composerText).toBe("");
    expect(backend.row("c1").text).toBe("");

    await act(async () => {
      mounted.root.unmount();
    });
  }, 30000);

  test("the reply of a save of another chat does not overwrite the revision of the open chat", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "old draft";
    backend.row("c2").text = "second chat draft";
    const mounted = await mount(backend);

    // A save of the first chat hangs.
    backend.hangSave = true;
    await act(async () => {
      mounted.store.actions.setComposerText("first version");
    });
    await tick();
    await act(async () => {
      mounted.store.actions.setComposerText("second version");
    });

    // The user opens the other chat (the hanging write is given up after
    // its deadline).
    await act(async () => {
      await mounted.store.actions.selectConversation("c2");
    });
    expect(mounted.store.state.currentConversationId).toBe("c2");
    const openChatRevision = mounted.store.state.draftRevision;

    // The late write of the first chat lands now: its answer must not
    // become the revision of the open chat.
    backend.hangSave = false;
    await act(async () => {
      for (const pending of backend.hungSaves) pending.resolve({ revision: 42 });
      await Promise.resolve();
      await Promise.resolve();
    });
    await tick();
    await tick();

    expect(mounted.store.state.draftRevision).toBe(openChatRevision);
    expect(mounted.store.state.draftRevision).not.toBe(42);

    await act(async () => {
      mounted.root.unmount();
    });
  }, 30000);
});

describe("composer lifecycle: late reads", () => {
  test("a draft read that returns after the clear does not refill the composer", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "old draft";
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.setComposerText("consumed text");
    });

    await act(async () => {
      await mounted.store.actions.send();
    });
    expect(mounted.store.state.composerText).toBe("");

    // A read that was started before the clear answers with the old draft.
    backend.deferDraftReads = true;
    await emitConversationChanged(backend, "c1");
    const stale = backend.draftReads[backend.draftReads.length - 1];
    expect(stale).toBeDefined();
    await act(async () => {
      stale.resolve({
        text: "consumed text",
        attachments: [],
        edit_text: "",
        edit_attachments: [],
        edit_target_id: null,
        revision: 1,
      });
      await stale.promise;
      await Promise.resolve();
      await Promise.resolve();
    });

    // The consumed snapshot must not come back.
    expect(mounted.store.state.composerText).toBe("");

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("two refetches of the same chat that answer out of order keep the newer snapshot", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "old draft";
    const mounted = await mount(backend);

    backend.deferLoads = true;
    await emitConversationChanged(backend, "c1");
    await emitConversationChanged(backend, "c1");
    expect(backend.loads.length).toBeGreaterThanOrEqual(2);

    const older = backend.loads[backend.loads.length - 2];
    const newer = backend.loads[backend.loads.length - 1];

    // The newer request answers first ...
    await act(async () => {
      newer.resolve({
        ...backend.detail("c1"),
        messages: [
          {
            ...fixtureMessage("a1", "c1", "new answer"),
            role: "assistant",
          },
        ],
      });
      await newer.promise;
      await Promise.resolve();
    });
    expect(mounted.store.state.messages.c1?.[0].content).toBe("new answer");

    // ... the older one must not roll the thread back.
    await act(async () => {
      older.resolve({
        ...backend.detail("c1"),
        messages: [
          {
            ...fixtureMessage("a1", "c1", "stale answer"),
            role: "assistant",
          },
        ],
      });
      await older.promise;
      await Promise.resolve();
    });
    expect(mounted.store.state.messages.c1?.[0].content).toBe("new answer");

    await act(async () => {
      mounted.root.unmount();
    });
  });
});

describe("composer lifecycle: input during a send", () => {
  test("text, attachments and the branch selection survive the receipt", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "old draft";
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.setComposerText("first");
    });

    const pending = deferred<unknown>();
    backend.sendDeferred = pending;
    await act(async () => {
      void mounted.store.actions.send();
      await Promise.resolve();
    });

    await act(async () => {
      mounted.store.actions.setComposerText("typed during the send");
      mounted.store.actions.setComposerAttachments([ATTACHMENT]);
      mounted.store.actions.toggleBranch("leaf-a");
    });

    await releaseSend(backend);
    backend.sendDeferred = null;

    expect(mounted.store.state.composerText).toBe("typed during the send");
    expect(mounted.store.state.composerAttachments).toEqual([ATTACHMENT]);
    expect(mounted.store.state.branchSelection).toEqual(["leaf-a"]);

    // The new input is persisted.
    await tick();
    expect(backend.row("c1").text).toBe("typed during the send");

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("switching to another chat during the send keeps that chat intact", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "old draft";
    backend.row("c2").text = "second chat draft";
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.setComposerText("chat one text");
    });

    const pending = deferred<unknown>();
    backend.sendDeferred = pending;
    await act(async () => {
      void mounted.store.actions.send();
      await Promise.resolve();
    });

    await act(async () => {
      await mounted.store.actions.selectConversation("c2");
    });

    await releaseSend(backend);
    backend.sendDeferred = null;

    // The other chat keeps its own draft.
    expect(backend.row("c2").text).toBe("second chat draft");

    // The sent chat does not come back.
    await act(async () => {
      await mounted.store.actions.selectConversation("c1");
    });
    expect(mounted.store.state.composerText).toBe("");
    expect(backend.row("c1").text).toBe("");

    await act(async () => {
      mounted.root.unmount();
    });
  });
});

describe("composer lifecycle: the edit buffer", () => {
  test("an edit send only consumes the edit buffer", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "normal draft";
    backend.row("c1").attachments = [ATTACHMENT];
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.startEdit("q1");
    });
    expect(mounted.store.state.editingLatest).toBe("q1");

    await act(async () => {
      await mounted.store.actions.send();
    });

    expect(backend.sends).toHaveLength(1);
    expect(backend.sends[0].mode).toBe("edit");

    // The edit buffer is consumed ...
    expect(mounted.store.state.editingLatest).toBeNull();
    expect(mounted.store.state.editText).toBe("");
    // ... the normal draft is untouched.
    expect(mounted.store.state.composerText).toBe("normal draft");
    expect(mounted.store.state.composerAttachments).toEqual([ATTACHMENT]);
    expect(backend.row("c1").text).toBe("normal draft");
    expect(backend.row("c1").editText).toBe("");

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("an edit that is changed during the send survives the receipt and the refresh", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "normal draft";
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.startEdit("q1");
    });

    const pending = deferred<unknown>();
    backend.sendDeferred = pending;
    await act(async () => {
      void mounted.store.actions.send();
      await Promise.resolve();
    });

    // The user keeps working on the edit while the request is in flight.
    await act(async () => {
      mounted.store.actions.setEditText("edited again");
    });

    await releaseSend(backend);
    backend.sendDeferred = null;

    expect(mounted.store.state.editingLatest).toBe("q1");
    expect(mounted.store.state.editText).toBe("edited again");
    expect(mounted.store.state.composerText).toBe("normal draft");

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("cancelling the edit leaves the normal draft alone", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "normal draft";
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.startEdit("q1");
    });
    expect(mounted.store.state.editingLatest).toBe("q1");

    await act(async () => {
      mounted.store.actions.cancelEdit();
    });

    expect(mounted.store.state.editingLatest).toBeNull();
    expect(mounted.store.state.editText).toBe("");
    // The normal draft is untouched.
    expect(mounted.store.state.composerText).toBe("normal draft");
    expect(backend.row("c1").text).toBe("normal draft");

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("the edit buffer of a chat comes back when the chat is opened again", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "normal draft";
    backend.row("c2").text = "second chat draft";
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.startEdit("q1");
    });
    await act(async () => {
      mounted.store.actions.setEditText("half finished edit");
    });
    await tick();

    // The edit buffer is durable: it belongs to the chat, not to the view.
    await act(async () => {
      await mounted.store.actions.selectConversation("c2");
    });
    expect(mounted.store.state.composerText).toBe("second chat draft");

    await act(async () => {
      await mounted.store.actions.selectConversation("c1");
    });
    expect(mounted.store.state.editingLatest).toBe("q1");
    expect(mounted.store.state.editText).toBe("half finished edit");
    expect(mounted.store.state.composerText).toBe("normal draft");

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("a late edit read does not re-open the edit mode of an accepted edit send", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "normal draft";
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.startEdit("q1");
    });
    await act(async () => {
      mounted.store.actions.setEditText("edited question");
    });
    await tick();

    // A read that was started **before** the send answers late with the
    // old edit buffer.
    backend.deferDraftReads = true;
    await emitConversationChanged(backend, "c1");
    const staleRead = backend.draftReads[backend.draftReads.length - 1];
    expect(staleRead).toBeDefined();

    // The send runs; its own refresh waits for its read.
    const sending = mounted.store.actions.send();
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    // The stale read answers with the edit buffer of before the send.
    await act(async () => {
      staleRead.resolve({
        text: "normal draft",
        attachments: [],
        edit_text: "edited question",
        edit_attachments: [],
        edit_target_id: "q1",
        revision: 2,
      });
      await staleRead.promise;
      await Promise.resolve();
      await Promise.resolve();
    });

    // The consumed edit must not come back: neither the edit mode nor the
    // old text.
    expect(mounted.store.state.editingLatest).toBeNull();
    expect(mounted.store.state.editText).toBe("");

    // The refresh behind the send answers now.
    await act(async () => {
      const fresh = backend.draftReads[backend.draftReads.length - 1];
      fresh.resolve({
        text: "normal draft",
        attachments: [],
        edit_text: "",
        edit_attachments: [],
        edit_target_id: null,
        revision: 3,
      });
      await fresh.promise;
      await Promise.resolve();
    });
    await act(async () => {
      await sending;
    });

    expect(backend.sends).toHaveLength(1);
    expect(backend.sends[0].mode).toBe("edit");
    expect(mounted.store.state.editingLatest).toBeNull();
    expect(mounted.store.state.editText).toBe("");
    // The normal draft is untouched by the edit send.
    expect(mounted.store.state.composerText).toBe("normal draft");

    await act(async () => {
      mounted.root.unmount();
    });
  });

  test("an edit buffer that the backend still holds is corrected, not refilled", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "normal draft";
    const mounted = await mount(backend);

    await act(async () => {
      mounted.store.actions.startEdit("q1");
    });
    await act(async () => {
      mounted.store.actions.setEditText("edited question");
    });
    await tick();

    // The compare-and-swap of the send loses the race: the row keeps the
    // edit buffer. (The backend only clears it when the revision matches.)
    backend.breakEditCas = true;
    await act(async () => {
      await mounted.store.actions.send();
    });

    // The edit buffer is gone from the composer ...
    expect(mounted.store.state.editingLatest).toBeNull();
    expect(mounted.store.state.editText).toBe("");

    // ... a read that still carries it must not refill it, and the row is
    // corrected.
    await emitConversationChanged(backend, "c1");
    await tick();

    expect(mounted.store.state.editingLatest).toBeNull();
    expect(mounted.store.state.editText).toBe("");
    expect(backend.row("c1").editText).toBe("");

    await act(async () => {
      mounted.root.unmount();
    });
  });
});

describe("composer lifecycle: an empty draft is a draft", () => {
  test("deleting the text and the attachments is persisted as an empty draft", async () => {
    const backend = new DraftBackend();
    backend.row("c1").text = "old draft";
    backend.row("c1").attachments = [ATTACHMENT];
    const mounted = await mount(backend);
    expect(mounted.store.state.composerText).toBe("old draft");

    await act(async () => {
      mounted.store.actions.setComposerText("");
      mounted.store.actions.setComposerAttachments([]);
    });
    await tick();

    expect(backend.row("c1").text).toBe("");
    expect(backend.row("c1").attachments).toEqual([]);

    // Switching away and back does not restore the old draft.
    await act(async () => {
      await mounted.store.actions.selectConversation("c2");
    });
    await act(async () => {
      await mounted.store.actions.selectConversation("c1");
    });
    expect(mounted.store.state.composerText).toBe("");

    await act(async () => {
      mounted.root.unmount();
    });
  });
});
