/**
 * Application store: reducer + async actions + backend event wiring.
 *
 * Contract notes (docs/TAURI_IPC.md):
 *   - `bootstrap` runs **after** the event subscriptions are attached, so no
 *     event is missed;
 *   - `conversations-changed` → refetch the conversation list;
 *   - `conversation-changed` → refetch the open conversation;
 *   - every async action checks the conversation id and the draft revision
 *     before it touches the state, so a late reply can never write into a
 *     chat the user has already left.
 */

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  type Dispatch,
  type ReactNode,
} from "react";

import { IpcClient } from "../ipc/client";
import { f, t } from "../i18n/strings";
import type {
  Attachment,
  CatalogEntry,
  ChatEvent,
  ModelPreferenceEntry,
  ConversationSummary,
  Draft,
  ExportScope,
  Id,
  Message,
  ModelTarget,
  SendMessageRequest,
  Settings,
} from "../ipc/types";
import { reducer, type Action } from "./reducer";
import { initialState, type AppState, type ModalKind } from "./types";
import { enabledModels, resolveSelection } from "./selectors";
import { isAnthropic } from "../model/presentation";
import {
  canAddAttachment,
  pasteAttachmentName,
  shouldConvertPaste,
} from "../util/attachments";

export { LONG_PASTE_CHARS } from "../util/attachments";

/** Coalescing window for streamed deltas (milliseconds). */
const DELTA_FLUSH_MS = 50;

/**
 * Upper bound for the pre-send draft flush.
 *
 * The flush must never turn a slow backend into a blocked composer, so the
 * send proceeds with the revision the frontend already knows when the flush
 * does not answer in time.
 */
const DRAFT_FLUSH_TIMEOUT_MS = 5000;

/**
 * Upper bound for a single draft write.
 *
 * A write that does not answer in time is abandoned so that the queue of
 * its conversation continues (one hanging write must not block every
 * conversation). The late answer is watched separately: the write may still
 * have reached the database.
 */
const DRAFT_WRITE_TIMEOUT_MS = 5000;

/** The composer content of one buffer (the normal one or the edit one). */
interface DraftContent {
  text: string;
  attachments: Attachment[];
}

/**
 * One composer buffer.
 *
 * `local` is the truth (what the user has right now), `saved` is what the
 * backend is known to hold. `version` counts the local edits: it tells a
 * **new** input apart from a repeated save of the same content — the
 * backend counts a retyped text as a new revision, so the frontend has to
 * count it as well.
 */
interface DraftSlot {
  /** What the user has right now. */
  local: DraftContent;
  /** What the backend is known to hold (`null` = unknown). */
  saved: DraftContent | null;
  /** The number of local edits. */
  version: number;
  /** The version at which the local content was last in sync with the backend. */
  syncedVersion: number;
  /** The local content is a new edit the backend has not counted yet. */
  dirty: boolean;
  /**
   * The row must be empty.
   *
   * A send consumed the content of this buffer: the database has to be
   * empty as well. A read that still carries content is a **stale write**
   * (a save that landed after the send) — it must neither come back into
   * the composer nor stay in the row.
   */
  expectEmpty: boolean;
  /** The pending autosave timer. */
  timer: ReturnType<typeof setTimeout> | null;
}

/**
 * The composer lifecycle of one conversation.
 *
 * The normal draft and the edit buffer are separate buffers, but the
 * backend counts them in **one** revision (`draft_revision`): a write of
 * one buffer moves the compare-and-swap token of the other.
 */
interface ConvDraft {
  normal: DraftSlot;
  edit: DraftSlot;
  /** The message the edit buffer belongs to. */
  editTarget: Id | null;
  /** The normal content a send consumed (it must never come back). */
  consumed: DraftContent | null;
  /** The edit content a send consumed. */
  consumedEdit: DraftContent | null;
  /** The tail of the per conversation write queue. */
  queue: Promise<unknown>;
  /** The number of the newest started read. */
  readSeq: number;
  /** The number of the newest applied read. */
  appliedRead: number;
  /** Bumped by every accepted send: reads that started earlier are stale. */
  clearGen: number;
  /** The revision the backend last reported for this conversation. */
  revision: number;
}

/** The snapshot of a send: what was submitted and when. */
interface SendSnapshot {
  conversationId: Id;
  mode: "normal" | "edit";
  text: string;
  attachments: Attachment[];
  /** The local version of the submitted buffer at the time of the send. */
  version: number;
  branchSelection: Id[];
}

/** A fresh (empty) composer buffer. */
function newSlot(): DraftSlot {
  return {
    local: { text: "", attachments: [] },
    saved: null,
    version: 0,
    syncedVersion: 0,
    dirty: false,
    expectEmpty: false,
    timer: null,
  };
}

/** Compare two composer contents by value. */
function sameContent(a: DraftContent | null, b: DraftContent | null): boolean {
  if (!a || !b) return false;
  return a.text === b.text && sameAttachments(a.attachments, b.attachments);
}

/** True when a composer buffer holds nothing. */
function isEmptyContent(content: DraftContent): boolean {
  return content.text.length === 0 && content.attachments.length === 0;
}

/**
 * Run a promise with a deadline.
 *
 * The promise itself is always handled, so an abandoned write can never
 * raise an unhandled rejection.
 */
async function withDeadline<T>(
  work: Promise<T>,
  ms: number,
): Promise<
  | { status: "done"; value: T }
  | { status: "failed"; error: unknown }
  | { status: "timeout" }
> {
  const guarded = work.then(
    (value) => ({ status: "done", value }) as const,
    (error) => ({ status: "failed", error }) as const,
  );
  let timer: ReturnType<typeof setTimeout> | null = null;
  const deadline = new Promise<{ status: "timeout" }>((resolve) => {
    timer = setTimeout(() => resolve({ status: "timeout" }), ms);
  });
  try {
    return await Promise.race([guarded, deadline]);
  } finally {
    if (timer !== null) clearTimeout(timer);
  }
}

export interface ToastOptions {
  kind?: "info" | "error" | "success";
}

export interface Actions {
  bootstrap(): Promise<void>;
  selectConversation(conversationId: Id | null): Promise<void>;
  refreshConversations(): Promise<void>;
  createConversation(): Promise<void>;
  renameConversation(conversationId: Id, title: string): Promise<void>;
  deleteConversation(conversationId: Id): Promise<void>;
  moveConversation(conversationId: Id, groupId: Id | null): Promise<void>;
  createGroup(name: string): Promise<void>;
  renameGroup(groupId: Id, name: string): Promise<void>;
  deleteGroup(groupId: Id): Promise<void>;
  setModelEnabled(modelId: string, enabled: boolean): Promise<void>;
  setPrimaryModel(modelId: string): Promise<void>;
  setModelOverrides(modelId: string, overrides: Record<string, unknown> | null): Promise<void>;
  reorderModels(modelIds: string[]): Promise<void>;
  setParallel(value: boolean): void;
  toggleChecked(modelId: string, checked: boolean): void;
  setChecked(modelIds: string[]): void;
  refreshCatalog(): Promise<void>;
  refreshPreferences(): Promise<ModelPreferenceEntry[] | null>;
  reloadCredentials(): Promise<void>;
  refreshKeyStatus(): Promise<void>;
  updateSettings(patch: Partial<Settings>): Promise<void>;
  setSidebarCollapsed(collapsed: boolean): void;
  setSidebarWidth(width: number): void;
  send(): Promise<void>;
  cancel(requestId: Id): Promise<void>;
  retry(requestId: Id): Promise<void>;
  deleteMessage(messageId: Id): Promise<void>;
  startEdit(messageId: Id): void;
  cancelEdit(): void;
  setComposerText(text: string): void;
  setComposerAttachments(attachments: Attachment[]): void;
  setEditText(text: string): void;
  setEditAttachments(attachments: Attachment[]): void;
  addAttachments(files: File[]): Promise<void>;
  addPastedText(text: string): void;
  removeAttachment(index: number): void;
  continueFrom(messageId: Id): Promise<void>;
  toggleBranch(messageId: Id): void;
  exportMarkdown(scope: ExportScope): Promise<void>;
  openModal(modal: ModalKind): void;
  closeModal(): void;
  toggleSidebar(collapsed?: boolean): void;
  showToast(text: string, options?: ToastOptions): void;
  dismissToast(): void;
  dismissEgressModal(): void;
  recheckEgress(): Promise<void>;
  sendWithoutAnthropic(): Promise<void>;
}

export interface StoreValue {
  state: AppState;
  dispatch: Dispatch<Action>;
  actions: Actions;
}

const StoreContext = createContext<StoreValue | null>(null);

export interface StoreProviderProps {
  client: IpcClient;
  children: ReactNode;
}

export function StoreProvider({ client, children }: StoreProviderProps) {
  const [state, dispatch] = useReducer(reducer, initialState);

  const stateRef = useRef(state);
  stateRef.current = state;

  /** Guards against late replies of a chat the user already left. */
  const selectionToken = useRef(0);
  /** Monotonic id for the egress intent (matches the native behaviour). */
  const egressIntent = useRef(0);
  /**
   * The composer lifecycle of every conversation the frontend knows.
   *
   * It replaces the two former maps (`draftBaselines` / `lastSavedDrafts`):
   * one record per conversation holds the local content, the content the
   * backend is known to hold, the local edit counter and the write queue.
   */
  const convDrafts = useRef(new Map<Id, ConvDraft>());
  /**
   * The conversation the composer content belongs to.
   *
   * The composer shows the content of the **open** chat only; while another
   * chat is loading the content on screen is stale and must not be written
   * into the slot of the chat that is opening.
   */
  const composerOwner = useRef<Id | null>(null);
  /**
   * Buffered stream deltas.
   *
   * The backend emits one `text_delta` per SSE chunk. Applying every delta
   * to the store would re-render (and re-parse) the whole answer per chunk,
   * which starves the webview's JS thread — timers and pending IPC
   * promises then stop, which is exactly how the native harness stalled.
   * The deltas are therefore coalesced into one update per ~50 ms.
   */
  const deltaBuffer = useRef(
    new Map<Id, { conversationId: Id; delta: string }>(),
  );
  const deltaTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** Last send intent, kept for the "send without Anthropic" path. */
  const pendingSend = useRef<SendMessageRequest | null>(null);
  /** The snapshot of the pending send (the same intent). */
  const pendingSnapshot = useRef<SendSnapshot | null>(null);
  /**
   * Per model: the number of the newest enable/disable request.
   *
   * A late reply of an older request must not roll back a newer choice, so
   * every request carries its number and only the newest one may undo
   * anything.
   */
  const modelToggleSeq = useRef(new Map<string, number>());
  /**
   * Per model: the tail of the write queue.
   *
   * The writes of one model are serialised, so a fast double click cannot
   * persist the older value after the newer one.
   */
  const modelWriteQueue = useRef(new Map<string, Promise<unknown>>());

  const showToast = useCallback(
    (text: string, options?: ToastOptions) => {
      dispatch({ type: "toast/show", text, kind: options?.kind ?? "info" });
    },
    [dispatch],
  );

  const dismissToast = useCallback(() => dispatch({ type: "toast/hide" }), [dispatch]);

  // ----- composer lifecycle -----

  /** The lifecycle record of a conversation (created on demand). */
  const draftOf = useCallback((conversationId: Id): ConvDraft => {
    let entry = convDrafts.current.get(conversationId);
    if (!entry) {
      entry = {
        normal: newSlot(),
        edit: newSlot(),
        editTarget: null,
        consumed: null,
        consumedEdit: null,
        queue: Promise.resolve(),
        readSeq: 0,
        appliedRead: 0,
        clearGen: 0,
        revision: 0,
      };
      convDrafts.current.set(conversationId, entry);
    }
    return entry;
  }, []);

  /**
   * Record a local edit of a buffer.
   *
   * The version is the "input generation": it tells a new input apart from
   * a repeated save of the same content. `dirty` marks the content as a
   * change the backend has not counted yet — it has to be written with
   * `changed = true`, even when the text is identical to the saved one.
   */
  const markLocalEdit = useCallback(
    (slot: DraftSlot, content: DraftContent) => {
      if (sameContent(slot.local, content)) return;
      slot.local = content;
      slot.version += 1;
      slot.dirty = true;
      // The user wrote something again: the row may hold their content.
      slot.expectEmpty = false;
    },
    [],
  );

  /** Stop the pending autosave of a buffer (the write queue takes over). */
  const cancelPendingSave = useCallback((slot: DraftSlot) => {
    if (slot.timer !== null) {
      clearTimeout(slot.timer);
      slot.timer = null;
    }
  }, []);

  /**
   * Run a write in the per conversation queue.
   *
   * The writes of one conversation are serialised: the revision is a
   * compare-and-swap token, so two overlapping writes would let the older
   * one win. The task must always settle (it carries its own deadline),
   * otherwise the queue of the conversation would stall forever.
   */
  const enqueueDraftWrite = useCallback(
    <T,>(conversationId: Id, task: () => Promise<T>): Promise<T> => {
      const entry = draftOf(conversationId);
      const run = entry.queue.then(task, task);
      entry.queue = run.then(
        () => undefined,
        () => undefined,
      );
      return run;
    },
    [draftOf],
  );

  /** The raw IPC call that persists the normal draft. */
  const saveNormal = useCallback(
    (conversationId: Id, content: DraftContent, changed: boolean) =>
      client.saveDraft({
        conversation_id: conversationId,
        text: content.text,
        attachments: content.attachments,
        changed,
      }),
    [client],
  );

  /** The raw IPC call that persists the edit buffer. */
  const saveEdit = useCallback(
    (conversationId: Id, content: DraftContent, changed: boolean) =>
      client.saveEditBuffer({
        conversation_id: conversationId,
        text: content.text,
        attachments: content.attachments,
        target_id: draftOf(conversationId).editTarget,
        changed,
      }),
    [client, draftOf],
  );

  /**
   * Write one buffer of a conversation (runs inside the write queue).
   *
   * The content is read **when the write runs**, not when it was
   * scheduled: a save that was scheduled before a send must never write the
   * consumed snapshot again. A buffer whose content the backend already
   * holds is not written at all — unless the local content is a new edit
   * (the backend counts a retyped text as a new revision).
   */
  const persistSlot = useCallback(
    async (
      conversationId: Id,
      kind: "normal" | "edit",
    ): Promise<{ ok: boolean; revision: number | null; error?: unknown }> => {
      const entry = draftOf(conversationId);
      const slot = kind === "normal" ? entry.normal : entry.edit;
      const content = slot.local;
      if (slot.saved && sameContent(slot.saved, content) && !slot.dirty) {
        return { ok: true, revision: entry.revision };
      }
      const changed = slot.dirty;
      const version = slot.version;
      const promise =
        kind === "normal"
          ? saveNormal(conversationId, content, changed)
          : saveEdit(conversationId, content, changed);
      const outcome = await withDeadline(promise, DRAFT_WRITE_TIMEOUT_MS);
      if (outcome.status === "timeout") {
        // The write may still reach the database — and it carries the
        // content of the moment it was started, which is not the current
        // content any more: the row has to be corrected when it lands.
        void promise.then(
          () => {
            if (sameContent(slot.local, content)) return;
            void enqueueDraftWrite(conversationId, () => {
              // The row has to be corrected: its content is unknown.
              slot.saved = null;
              return persistSlot(conversationId, kind);
            });
          },
          () => undefined,
        );
        return {
          ok: false,
          revision: null,
          error: new Error("draft write timed out"),
        };
      }
      if (outcome.status === "failed") {
        showToast(`Draft could not be saved: ${describe(outcome.error)}`, {
          kind: "error",
        });
        return { ok: false, revision: null, error: outcome.error };
      }
      slot.saved = { text: content.text, attachments: content.attachments };
      if (slot.version === version) {
        slot.dirty = false;
        slot.syncedVersion = version;
      }
      entry.revision = outcome.value.revision;
      // The revision in the state belongs to the **open** chat: a reply of
      // another chat must not overwrite it.
      if (stateRef.current.currentConversationId === conversationId) {
        dispatch({ type: "draft/revision", revision: outcome.value.revision });
      }
      return { ok: true, revision: outcome.value.revision };
    },
    [dispatch, draftOf, enqueueDraftWrite, saveEdit, saveNormal, showToast],
  );

  /** Persist a buffer of a conversation (queued). */
  const writeSlot = useCallback(
    (conversationId: Id, kind: "normal" | "edit"): Promise<unknown> =>
      enqueueDraftWrite(conversationId, () =>
        persistSlot(conversationId, kind),
      ),
    [enqueueDraftWrite, persistSlot],
  );

  /** Persist the normal draft of a conversation (queued). */
  const writeNormal = useCallback(
    (conversationId: Id): Promise<unknown> =>
      writeSlot(conversationId, "normal"),
    [writeSlot],
  );

  /** Persist the edit buffer of a conversation (queued). */
  const writeEdit = useCallback(
    (conversationId: Id): Promise<unknown> => writeSlot(conversationId, "edit"),
    [writeSlot],
  );

  /**
   * Flush the composer before a send and return the revision to freeze.
   *
   * The flush runs **inside** the write queue of the conversation: a save
   * that is still running has to be finished first, otherwise it would move
   * the revision after the freeze and break the compare-and-swap of the
   * send. The normal draft is written before the edit buffer because both
   * share one revision counter.
   *
   * The flush is time-boxed: a slow backend must never block the composer.
   */
  const flushBeforeSend = useCallback(
    async (
      conversationId: Id,
      mode: "normal" | "edit",
    ): Promise<number | null> => {
      const entry = draftOf(conversationId);
      const work = enqueueDraftWrite(conversationId, async () => {
        await persistSlot(conversationId, "normal");
        if (mode === "edit") {
          await persistSlot(conversationId, "edit");
        }
        return entry.revision;
      });
      const outcome = await withDeadline(work, DRAFT_FLUSH_TIMEOUT_MS);
      if (outcome.status === "done") return outcome.value;
      if (outcome.status === "failed") {
        showToast(`Draft could not be saved: ${describe(outcome.error)}`, {
          kind: "error",
        });
      }
      return null;
    },
    [draftOf, enqueueDraftWrite, persistSlot, showToast],
  );

  // ----- conversation helpers -----

  const applyConversationDetail = useCallback(
    (
      conversationId: Id,
      detail: Awaited<ReturnType<IpcClient["loadConversation"]>>,
      draft: Draft | null,
      options: { stale?: boolean } = {},
    ) => {
      const entry = draftOf(conversationId);
      const slot = entry.normal;
      const editSlot = entry.edit;
      // The revision only moves forward.
      if (draft) {
        entry.revision = Math.max(entry.revision, draft.revision);
      }
      // A read that started before an accepted send is stale: its draft may
      // still be the snapshot the send consumed.
      const stale = options.stale === true;
      const editFetched: DraftContent = {
        text: draft?.edit_text ?? "",
        attachments: draft?.edit_attachments ?? [],
      };
      // The consumed edit must never come back, not even when the database
      // still holds it (the compare-and-swap lost the race).
      const editConsumed =
        entry.consumedEdit !== null &&
        sameContent(entry.consumedEdit, editFetched) &&
        isEmptyContent(editSlot.local);
      // The edit row must be empty (an edit send consumed it) but still
      // carries something: a save landed after the send.
      const editStaleRow =
        editSlot.expectEmpty && !isEmptyContent(editFetched);
      // The edit buffer of the user is newer than the backend's when the
      // user changed it since the last synchronisation — and a stale read
      // must never throw it away either.
      const preserveEditBuffer =
        stale ||
        editConsumed ||
        editStaleRow ||
        editSlot.version !== editSlot.syncedVersion;
      dispatch({
        type: "conversation/loaded",
        conversationId,
        conversation: detail.conversation,
        messages: detail.messages,
        generations: detail.generations,
        usage: detail.usage,
        // Keep the known revision when the draft could not be read: the
        // backend uses it as the send CAS token.
        draftRevision: entry.revision,
        // An **active** edit lives in the draft; `edit_latest` only says
        // that the latest question *may* be edited.
        editTargetId: draft?.edit_target_id ?? null,
        editText: draft?.edit_text ?? "",
        editAttachments: draft?.edit_attachments ?? [],
        editLatestEligible: detail.edit_latest?.target_id ?? null,
        preserveEditBuffer,
      });
      if (draft) {
        const fetched: DraftContent = {
          text: draft.text,
          attachments: draft.attachments,
        };
        // The consumed snapshot must never come back, not even when the
        // database still holds it (the compare-and-swap lost the race).
        const consumed =
          entry.consumed !== null &&
          sameContent(entry.consumed, fetched) &&
          isEmptyContent(slot.local);
        const inSync = slot.version === slot.syncedVersion;
        // The row must be empty (a send consumed the content) but still
        // carries something: a save landed after the send. It must not come
        // back into the composer, and the row is corrected.
        const staleRow = slot.expectEmpty && !isEmptyContent(fetched);
        if (consumed || staleRow) {
          slot.saved = null;
          slot.dirty = false;
          void writeSlot(conversationId, "normal");
        } else if (!stale && inSync) {
          dispatch({ type: "composer/text", text: fetched.text });
          dispatch({ type: "composer/attachments", attachments: fetched.attachments });
          composerOwner.current = conversationId;
          slot.local = {
            text: fetched.text,
            attachments: fetched.attachments,
          };
          slot.saved = {
            text: fetched.text,
            attachments: fetched.attachments,
          };
          slot.dirty = false;
          slot.syncedVersion = slot.version;
          if (isEmptyContent(fetched)) slot.expectEmpty = false;
        }
        // The edit buffer follows the same rule as the normal draft.
        if (editConsumed || editStaleRow) {
          // The row still holds the consumed edit: it must not come back
          // into the composer, and the row is corrected.
          editSlot.saved = null;
          editSlot.dirty = false;
          void writeSlot(conversationId, "edit");
        } else if (!preserveEditBuffer) {
          editSlot.local = editFetched;
          editSlot.saved = {
            text: editFetched.text,
            attachments: editFetched.attachments,
          };
          editSlot.dirty = false;
          editSlot.syncedVersion = editSlot.version;
          if (isEmptyContent(editFetched)) editSlot.expectEmpty = false;
          entry.editTarget = draft.edit_target_id;
        }
      }
    },
    [dispatch, draftOf, writeSlot],
  );

  /**
   * Fetch a conversation snapshot without touching the state.
   *
   * The two commands are issued **one after the other** on purpose: the
   * backend serialises its state behind a lock, and two concurrent commands
   * from the webview only add contention (and a lock-order risk) without
   * saving any time.
   */
  const fetchConversation = useCallback(
    async (conversationId: Id) => {
      const detail = await client.loadConversation(conversationId);
      const draft = await client.getDraft(conversationId).catch(() => null);
      return { detail, draft };
    },
    [client],
  );

  /**
   * Load a conversation and apply it — but only if the user has not switched
   * to another chat in the meantime. The token check happens **before** the
   * state is touched, so a late reply can neither repaint the current chat
   * nor overwrite the composer of the chat that is open now.
   *
   * Two reads of the same chat may also answer out of order: the older one
   * is dropped instead of rolling the newer snapshot back. A read that was
   * started before an accepted send is marked stale — its draft may still
   * be the snapshot the send consumed.
   */
  const loadConversation = useCallback(
    async (conversationId: Id): Promise<void> => {
      const token = selectionToken.current;
      const entry = draftOf(conversationId);
      const seq = (entry.readSeq += 1);
      const clearGen = entry.clearGen;
      const { detail, draft } = await fetchConversation(conversationId);
      if (token !== selectionToken.current) return;
      if (stateRef.current.currentConversationId !== conversationId) return;
      // An older snapshot must not overwrite a newer one.
      if (seq < entry.appliedRead) return;
      entry.appliedRead = seq;
      applyConversationDetail(conversationId, detail, draft, {
        stale: clearGen !== entry.clearGen,
      });
    },
    [applyConversationDetail, draftOf, fetchConversation],
  );

  const bootstrap = useCallback(async () => {
    dispatch({ type: "boot/started" });
    try {
      const appState = await client.bootstrap();
      dispatch({
        type: "boot/loaded",
        appVersion: appState.app_version,
        conversations: appState.conversations,
        groups: appState.groups,
        pendingNewConversation: appState.pending_new_conversation,
        catalog: appState.catalog.entries,
        catalogUpdatedAt: appState.catalog.updated_at_ms,
        preferences: appState.preferences,
        credentials: appState.credentials,
        settings: appState.settings,
        egress: appState.egress,
      });
      // The bootstrap payload has no "current chat" pointer: the unsent slot
      // wins (it keeps the pending draft), otherwise the newest chat.
      const first =
        appState.pending_new_conversation ??
        appState.conversations[0]?.id ??
        null;
      if (first) {
        selectionToken.current += 1;
        const token = selectionToken.current;
        try {
          const entry = draftOf(first);
          const seq = (entry.readSeq += 1);
          const clearGen = entry.clearGen;
          const { detail, draft } = await fetchConversation(first);
          if (token !== selectionToken.current) return;
          entry.appliedRead = seq;
          applyConversationDetail(first, detail, draft, {
            stale: clearGen !== entry.clearGen,
          });
          dispatch({ type: "conversation/selected", conversationId: first });
        } catch (error) {
          showToast(`Failed to open chat: ${describe(error)}`, { kind: "error" });
        }
      }
    } catch (error) {
      dispatch({ type: "boot/failed", error: describe(error) });
    }
  }, [applyConversationDetail, client, draftOf, fetchConversation, showToast]);

  const refreshConversations = useCallback(async () => {
    try {
      const conversations = await client.listConversations();
      dispatch({ type: "conversations/loaded", conversations });
    } catch (error) {
      showToast(`Could not refresh chats: ${describe(error)}`, { kind: "error" });
    }
  }, [client, showToast]);

  const selectConversation = useCallback(
    async (conversationId: Id | null) => {
      const current = stateRef.current;
      const previousId = current.currentConversationId;
      // Flush the pending draft of the chat we are leaving before the new
      // chat paints over the composer.
      if (previousId && previousId !== conversationId) {
        const previous = draftOf(previousId);
        // The pending autosave must not write the same content again after
        // the queue wrote it.
        cancelPendingSave(previous.normal);
        await writeNormal(previousId);
      }
      selectionToken.current += 1;
      const token = selectionToken.current;
      dispatch({ type: "conversation/selected", conversationId });
      if (!conversationId) return;
      // The composer shows the content of the open chat: the draft of the
      // chat we are leaving must not stay on screen (and must not be
      // written into the chat that is opening).
      composerOwner.current = null;
      dispatch({ type: "composer/text", text: "" });
      dispatch({ type: "composer/attachments", attachments: [] });
      dispatch({ type: "edit/cancel" });
      try {
        const entry = draftOf(conversationId);
        const seq = (entry.readSeq += 1);
        const clearGen = entry.clearGen;
        const { detail, draft } = await fetchConversation(conversationId);
        // A newer selection happened meanwhile: drop the stale snapshot
        // *before* it can touch the composer or the message list.
        if (token !== selectionToken.current) return;
        if (seq < entry.appliedRead) return;
        entry.appliedRead = seq;
        applyConversationDetail(conversationId, detail, draft, {
          stale: clearGen !== entry.clearGen,
        });
      } catch (error) {
        if (token !== selectionToken.current) return;
        showToast(`Failed to open chat: ${describe(error)}`, { kind: "error" });
      }
    },
    [
      cancelPendingSave,
      draftOf,
      fetchConversation,
      showToast,
      writeNormal,
    ],
  );

  const createConversation = useCallback(async () => {
    try {
      const conversation = await client.createConversation();
      dispatch({ type: "conversation/upserted", conversation });
      await selectConversation(conversation.id);
    } catch (error) {
      showToast(`Could not create chat: ${describe(error)}`, { kind: "error" });
    }
  }, [client, selectConversation, showToast]);

  const renameConversation = useCallback(
    async (conversationId: Id, title: string) => {
      const trimmed = title.trim();
      if (!trimmed) {
        showToast("Title cannot be empty", { kind: "error" });
        return;
      }
      const conversation = stateRef.current.conversations.find(
        (entry) => entry.id === conversationId,
      );
      try {
        const updated = await client.renameConversation({
          conversation_id: conversationId,
          title: trimmed,
          expected_title_revision: conversation?.title_revision ?? 0,
        });
        dispatch({ type: "conversation/upserted", conversation: updated });
      } catch (error) {
        // A revision conflict means the title changed elsewhere: refetch.
        if (describe(error).includes("title_revision_conflict")) {
          await refreshConversations();
        }
        showToast(`Failed to save title: ${describe(error)}`, { kind: "error" });
      }
    },
    [client, refreshConversations, showToast],
  );

  const deleteConversation = useCallback(
    async (conversationId: Id) => {
      try {
        await client.deleteConversation(conversationId);
        dispatch({ type: "conversation/removed", conversationId });
        showToast("Chat deleted");
      } catch (error) {
        showToast(`Could not delete chat: ${describe(error)}`, { kind: "error" });
      }
    },
    [client, showToast],
  );

  const moveConversation = useCallback(
    async (conversationId: Id, groupId: Id | null) => {
      try {
        await client.setConversationGroup(conversationId, groupId);
        await refreshConversations();
        showToast("Chat moved");
      } catch (error) {
        showToast(`Could not move chat: ${describe(error)}`, { kind: "error" });
      }
    },
    [client, refreshConversations, showToast],
  );

  // ----- groups -----

  const createGroup = useCallback(
    async (name: string) => {
      const trimmed = name.trim();
      if (!trimmed) return;
      try {
        const group = await client.createGroup(trimmed);
        dispatch({ type: "groups/loaded", groups: [...stateRef.current.groups, group] });
        showToast("Group created");
      } catch (error) {
        showToast(`Could not create group: ${describe(error)}`, { kind: "error" });
      }
    },
    [client, showToast],
  );

  const renameGroup = useCallback(
    async (groupId: Id, name: string) => {
      const trimmed = name.trim();
      if (!trimmed) return;
      try {
        await client.renameGroup(groupId, trimmed);
        dispatch({
          type: "groups/loaded",
          groups: stateRef.current.groups.map((group) =>
            group.id === groupId ? { ...group, name: trimmed } : group,
          ),
        });
      } catch (error) {
        showToast(`Could not rename group: ${describe(error)}`, { kind: "error" });
      }
    },
    [client, showToast],
  );

  const deleteGroup = useCallback(
    async (groupId: Id) => {
      try {
        await client.deleteGroup(groupId);
        dispatch({
          type: "groups/loaded",
          groups: stateRef.current.groups.filter((group) => group.id !== groupId),
        });
        await refreshConversations();
        showToast("Group deleted");
      } catch (error) {
        showToast(`Could not delete group: ${describe(error)}`, { kind: "error" });
      }
    },
    [client, refreshConversations, showToast],
  );

  // ----- models -----

  /**
   * Enable or disable a model.
   *
   * A model of the catalogue that has **no** preference yet can be added to
   * "My models": a complete preference is built from the catalogue entry
   * (its provider id) and persisted through the existing upsert command.
   * The existing sort order, the default flag and the parameter overrides
   * of a known model are never touched.
   *
   * A failed write restores the previous state and reports the error. The
   * writes of one model are serialised and every request carries a number,
   * so a fast double click can neither roll back the newer choice nor
   * persist the older value.
   */
  const setModelEnabled = useCallback(
    async (modelId: string, enabled: boolean) => {
      const current = stateRef.current;
      const existing = current.preferences.find(
        (entry) => entry.model_id === modelId,
      );
      const catalogEntry = current.catalog.find((entry) => entry.id === modelId);
      const providerId = existing?.provider_id ?? catalogEntry?.provider_id;
      if (!providerId) {
        showToast(t("toast.model_unknown"), { kind: "error" });
        return;
      }

      const next: ModelPreferenceEntry = existing
        ? { ...existing, enabled }
        : {
            provider_id: providerId,
            model_id: modelId,
            enabled,
            // A new model joins at the end of the panel order.
            sort_order: nextSortOrder(current.preferences),
            is_default: false,
            overrides: null,
          };

      const seq = (modelToggleSeq.current.get(modelId) ?? 0) + 1;
      modelToggleSeq.current.set(modelId, seq);

      dispatch({
        type: "model/enabled",
        modelId,
        enabled,
        preference: existing ? undefined : next,
      });

      // The writes of one model run one after the other.
      const previous = modelWriteQueue.current.get(modelId) ?? Promise.resolve();
      const write = previous
        .catch(() => undefined)
        .then(() =>
          client.setModelPreference({
            provider_id: next.provider_id,
            model_id: modelId,
            preference: {
              enabled: next.enabled,
              sort_order: next.sort_order,
              is_default: next.is_default,
              overrides: next.overrides,
            },
          }),
        );
      modelWriteQueue.current.set(
        modelId,
        write.catch(() => undefined),
      );

      try {
        await write;
      } catch (error) {
        // A newer click owns the model now: its result must not be undone.
        if (modelToggleSeq.current.get(modelId) !== seq) return;
        dispatch({
          type: "model/enabled",
          modelId,
          enabled: !enabled,
          // A model that had no preference before must not keep one.
          preference: existing ? undefined : null,
        });
        showToast(f("toast.model_update_failed", { error: describe(error) }), {
          kind: "error",
        });
      }
    },
    [client, showToast],
  );

  const setPrimaryModel = useCallback(
    async (modelId: string) => {
      const current = stateRef.current;
      const preference = current.preferences.find(
        (entry) => entry.model_id === modelId,
      );
      if (!preference) return;
      dispatch({ type: "model/primary", modelId });
      try {
        await client.setPrimaryModel(preference.provider_id, modelId);
      } catch (error) {
        showToast(`Could not set primary model: ${describe(error)}`, { kind: "error" });
      }
    },
    [client, showToast],
  );

  const setModelOverrides = useCallback(
    async (modelId: string, overrides: Record<string, unknown> | null) => {
      const current = stateRef.current;
      const preference = current.preferences.find(
        (entry) => entry.model_id === modelId,
      );
      if (!preference) return;
      dispatch({ type: "model/overrides", modelId, overrides });
      try {
        await client.setModelPreference({
          provider_id: preference.provider_id,
          model_id: modelId,
          preference: {
            enabled: preference.enabled,
            sort_order: preference.sort_order,
            is_default: preference.is_default,
            overrides,
          },
        });
      } catch (error) {
        showToast(`Could not save parameters: ${describe(error)}`, { kind: "error" });
      }
    },
    [client, showToast],
  );

  const reorderModels = useCallback(
    async (modelIds: string[]) => {
      const current = stateRef.current;
      const byId = new Map(current.preferences.map((item) => [item.model_id, item]));
      const next = modelIds
        .map((modelId, index) => {
          const preference = byId.get(modelId);
          if (!preference) return null;
          return { ...preference, sort_order: index };
        })
        .filter((item): item is NonNullable<typeof item> => item !== null);
      dispatch({ type: "preferences/loaded", preferences: next });
      try {
        for (const preference of next) {
          await client.setModelPreference({
            provider_id: preference.provider_id,
            model_id: preference.model_id,
            preference: {
              enabled: preference.enabled,
              sort_order: preference.sort_order,
              is_default: preference.is_default,
              overrides: preference.overrides,
            },
          });
        }
      } catch (error) {
        showToast(`Could not reorder models: ${describe(error)}`, { kind: "error" });
      }
    },
    [client, showToast],
  );

  /**
   * Compatibility shim for the old "parallel" toggle.
   *
   * There is no parallel mode any more: the checked models **are** the
   * selection. `true` checks every enabled model, `false` keeps only the
   * primary one.
   */
  const setParallel = useCallback(
    (value: boolean) => {
      dispatch({ type: "model/parallel", value });
    },
    [dispatch],
  );

  const toggleChecked = useCallback(
    (modelId: string, checked: boolean) => {
      dispatch({ type: "model/checked", modelId, checked });
    },
    [dispatch],
  );

  const setChecked = useCallback(
    (modelIds: string[]) => {
      dispatch({ type: "model/checkedSet", modelIds });
    },
    [dispatch],
  );

  const refreshCatalog = useCallback(async () => {
    dispatch({ type: "catalog/refreshing", value: true });
    try {
      const catalog = await client.refreshCatalog();
      dispatch({
        type: "catalog/loaded",
        entries: catalog.entries,
        updatedAt: catalog.updated_at_ms,
      });
    } catch (error) {
      dispatch({ type: "catalog/refreshing", value: false });
      showToast(`Catalog refresh failed: ${describe(error)}`, { kind: "error" });
    }
  }, [client, showToast]);

  /**
   * Re-read the model preferences (enabled / primary / overrides).
   *
   * The preferences can change outside the webview (another window, a
   * catalogue refresh, the backend's own defaults), so the UI must not
   * refuse an action because its cached list is stale.
   */
  const refreshPreferences = useCallback(async (): Promise<ModelPreferenceEntry[] | null> => {
    try {
      const preferences = await client.listPreferences();
      // A backend that answers with something else must not wipe the list —
      // and must not pass unnoticed: a non-array means a broken bridge.
      if (!Array.isArray(preferences)) {
        console.warn(
          "list_preferences answered with a non-array; keeping the current list",
        );
        return null;
      }
      dispatch({ type: "preferences/loaded", preferences });
      return preferences;
    } catch (error) {
      showToast(`Could not refresh models: ${describe(error)}`, { kind: "error" });
      return null;
    }
  }, [client, showToast]);

  const reloadCredentials = useCallback(async () => {
    dispatch({ type: "credentials/reloading", value: true });
    try {
      const credentials = await client.reloadCredentials();
      dispatch({ type: "credentials/loaded", credentials });
      const missing = Object.entries(credentials)
        .filter(([, configured]) => !configured)
        .map(([provider]) => provider);
      showToast(
        missing.length === 0
          ? "Provider keys reloaded"
          : `Provider keys missing: ${missing.join(", ")}`,
        { kind: missing.length === 0 ? "success" : "error" },
      );
    } catch (error) {
      dispatch({ type: "credentials/reloading", value: false });
      showToast(`Could not reload provider keys: ${describe(error)}`, { kind: "error" });
    }
  }, [client, showToast]);

  const refreshKeyStatus = useCallback(async () => {
    dispatch({ type: "keyStatus/loading", value: true });
    try {
      const status = await client.checkKeyStatus();
      dispatch({ type: "keyStatus/loaded", status });
    } catch (error) {
      dispatch({ type: "keyStatus/loaded", status: null });
      showToast(`Key check failed: ${describe(error)}`, { kind: "error" });
    }
  }, [client, showToast]);

  const updateSettings = useCallback(
    async (patch: Partial<Settings>) => {
      try {
        const updated = await client.updateSettings(patch);
        // A backend that answers with nothing keeps the current settings.
        if (updated) dispatch({ type: "settings/loaded", settings: updated });
      } catch (error) {
        showToast(`Could not save settings: ${describe(error)}`, { kind: "error" });
      }
    },
    [client, showToast],
  );

  const setSidebarWidth = useCallback(
    (width: number) => {
      dispatch({ type: "sidebar/width", width });
    },
    [dispatch],
  );

  const setSidebarCollapsed = useCallback(
    (collapsed: boolean) => {
      dispatch({ type: "sidebar/toggle", collapsed });
      void updateSettings({ sidebar_collapsed: collapsed });
    },
    [updateSettings],
  );

  // ----- composer / drafts -----

  /**
   * Note a local edit of the composer.
   *
   * The slot of the open conversation is updated **immediately** (not only
   * after the next render): a write that is already queued has to see the
   * content the user has right now.
   */
  const noteComposerEdit = useCallback(
    (text: string, attachments: Attachment[]) => {
      const conversationId = stateRef.current.currentConversationId;
      if (!conversationId) return;
      composerOwner.current = conversationId;
      markLocalEdit(draftOf(conversationId).normal, { text, attachments });
    },
    [draftOf, markLocalEdit],
  );

  const setComposerText = useCallback(
    (text: string) => {
      dispatch({ type: "composer/text", text });
      noteComposerEdit(text, stateRef.current.composerAttachments);
    },
    [dispatch, noteComposerEdit],
  );

  const setComposerAttachments = useCallback(
    (attachments: Attachment[]) => {
      dispatch({ type: "composer/attachments", attachments });
      noteComposerEdit(stateRef.current.composerText, attachments);
    },
    [dispatch, noteComposerEdit],
  );

  /**
   * Note a local edit of the edit buffer.
   *
   * The edit buffer is a buffer of its own: an edit send consumes it, the
   * normal draft stays untouched.
   */
  const noteEditEdit = useCallback(
    (text: string, attachments: Attachment[]) => {
      const current = stateRef.current;
      const conversationId = current.currentConversationId;
      if (!conversationId || !current.editingLatest) return;
      markLocalEdit(draftOf(conversationId).edit, { text, attachments });
    },
    [draftOf, markLocalEdit],
  );

  const setEditText = useCallback(
    (text: string) => {
      dispatch({ type: "edit/text", text });
      noteEditEdit(text, stateRef.current.editAttachments);
    },
    [dispatch, noteEditEdit],
  );

  const setEditAttachments = useCallback(
    (attachments: Attachment[]) => {
      dispatch({ type: "edit/attachments", attachments });
      noteEditEdit(stateRef.current.editText, attachments);
    },
    [dispatch, noteEditEdit],
  );

  const addAttachments = useCallback(
    async (files: File[]) => {
      const current = stateRef.current;
      const next = current.composerAttachments.slice();
      let rejection: string | null = null;
      for (const file of files) {
        const text = await readTextFile(file);
        if (text === null) continue;
        const check = canAddAttachment(next, text);
        if (!check.ok) {
          rejection = describeRejection(check.reason, file.name);
          break;
        }
        next.push({ name: file.name || "untitled", content: text });
      }
      if (rejection) showToast(rejection, { kind: "error" });
      if (next.length !== current.composerAttachments.length) {
        dispatch({ type: "composer/attachments", attachments: next });
        noteComposerEdit(current.composerText, next);
      }
    },
    [showToast],
  );

  const addPastedText = useCallback(
    (text: string) => {
      if (!shouldConvertPaste(text)) return;
      const current = stateRef.current;
      const next = current.composerAttachments.slice();
      const check = canAddAttachment(next, text);
      if (!check.ok) {
        showToast(describeRejection(check.reason), { kind: "error" });
        return;
      }
      next.push({ name: pasteAttachmentName(Date.now()), content: text });
      dispatch({ type: "composer/attachments", attachments: next });
      noteComposerEdit(stateRef.current.composerText, next);
      showToast(`Long paste attached (${text.length} chars)`);
    },
    [noteComposerEdit, showToast],
  );

  const removeAttachment = useCallback(
    (index: number) => {
      const current = stateRef.current;
      const next = current.composerAttachments.filter((_, i) => i !== index);
      dispatch({ type: "composer/attachments", attachments: next });
      noteComposerEdit(current.composerText, next);
    },
    [dispatch, noteComposerEdit],
  );

  // ----- messaging -----

  const buildSelection = useCallback(
    (
      current: AppState,
      models: string[],
      primary: string | null,
      branchLeaves: Id[],
    ): SendMessageRequest["selection"] => {
      const providerFor = (modelId: string): Id =>
        current.preferences.find((entry) => entry.model_id === modelId)?.provider_id ??
        current.catalog.find((entry) => entry.id === modelId)?.provider_id ??
        "";

      if (branchLeaves.length > 0) {
        const branches = branchLeaves.map((leaf, index) => {
          const modelId = models[index % models.length] ?? primary ?? models[0];
          const target: ModelTarget = { provider_id: providerFor(modelId), model_id: modelId };
          return { leaf, target };
        });
        return {
          kind: "multi_branch",
          branches,
          primary: primary ? 0 : 0,
        };
      }

      const leaf = current.conversations.find(
        (entry) => entry.id === current.currentConversationId,
      )?.main_leaf_id ?? null;
      const targets = models.map((modelId) => ({
        provider_id: providerFor(modelId),
        model_id: modelId,
      }));
      const primaryIndex = primary ? Math.max(0, models.indexOf(primary)) : 0;
      if (models.length > 1) {
        return {
          kind: "same_question_multi",
          leaf,
          targets,
          primary: primaryIndex,
        };
      }
      return {
        kind: "single",
        leaf,
        targets,
        primary: primaryIndex,
      };
    },
    [],
  );

  /**
   * Consume the snapshot of an accepted send.
   *
   * Only the buffer that was sent is cleared, and only when it still holds
   * exactly what was submitted — the local version tells a new input apart
   * from the same text typed again. Another chat is never touched.
   *
   * The consumed snapshot is remembered: a read that still carries it must
   * not put it back into the composer (the compare-and-swap of the send may
   * have lost the race, so the row can still hold it).
   */
  const consumeSnapshot = useCallback(
    (snapshot: SendSnapshot) => {
      const entry = draftOf(snapshot.conversationId);
      // Reads that started before the clear are stale.
      entry.clearGen += 1;
      const content: DraftContent = {
        text: snapshot.text,
        attachments: snapshot.attachments,
      };
      const slot = snapshot.mode === "edit" ? entry.edit : entry.normal;
      if (snapshot.mode === "edit") {
        entry.consumedEdit = content;
      } else {
        entry.consumed = content;
      }
      const unchanged =
        slot.version === snapshot.version && sameContent(slot.local, content);
      if (!unchanged) return;
      // The buffer is empty now; the backend consumed it with the
      // compare-and-swap of the send.
      slot.local = { text: "", attachments: [] };
      slot.dirty = false;
      // The compare-and-swap may have lost the race: the row can still hold
      // the snapshot — or a save that landed after the send. The next read
      // notices it and corrects the row.
      slot.saved = { text: "", attachments: [] };
      slot.syncedVersion = slot.version;
      slot.expectEmpty = true;
      if (snapshot.mode === "edit") {
        entry.editTarget = null;
        if (stateRef.current.currentConversationId === snapshot.conversationId) {
          dispatch({ type: "edit/clear" });
        }
      } else if (
        stateRef.current.currentConversationId === snapshot.conversationId
      ) {
        dispatch({ type: "composer/clear" });
      }
    },
    [dispatch, draftOf],
  );

  /**
   * Submit a send and reconcile.
   *
   * The submission is asynchronous: while it is in flight the user may type
   * new input, attach files, pick other branches or switch to another chat.
   * The composer and the branch selection are therefore only cleared when
   * they still hold **exactly what was submitted** — a newer edit or another
   * chat is never touched.
   */
  const dispatchSend = useCallback(
    async (request: SendMessageRequest, snapshot: SendSnapshot) => {
      const conversationId = snapshot.conversationId;
      let receipt: unknown;
      try {
        receipt = await client.sendMessage(request);
      } catch (error) {
        // The backend did not accept the message: the draft and the
        // attachments stay in the composer — nothing is cleared.
        showToast(`Send failed (draft kept): ${describe(error)}`, { kind: "error" });
        return;
      }
      void receipt;
      // The message is stored: the submitted snapshot may be consumed.
      consumeSnapshot(snapshot);
      const current = stateRef.current;
      if (current.currentConversationId === conversationId) {
        if (sameIds(current.branchSelection, snapshot.branchSelection)) {
          dispatch({ type: "branch/selection", messageIds: [] });
        }
      }
      // The database is authoritative: refresh the chat. A failing refresh
      // is **not** a failing send.
      try {
        await loadConversation(conversationId);
      } catch (error) {
        showToast(`Could not refresh the chat: ${describe(error)}`, {
          kind: "error",
        });
      }
    },
    [client, consumeSnapshot, dispatch, loadConversation, showToast],
  );

  const send = useCallback(async () => {
    const current = stateRef.current;
    const conversationId = current.currentConversationId;
    if (!conversationId) return;

    let enabled = enabledModels(current);
    let selection = resolveSelection(enabled, [], current.modelSelection);
    // The state the request is built from; it may hold fresher preferences
    // than the snapshot at the beginning of the send.
    let effective = current;
    if (selection.invalidReason === "composer.send_disabled_no_model") {
      // The cached preference list may be stale (the preferences can change
      // outside the webview). Re-read it once before refusing the send, and
      // use the **fresh** list: the store ref is only updated on the next
      // render, which has not happened yet at this point.
      const fresh = await refreshPreferences();
      if (fresh) {
        effective = { ...stateRef.current, preferences: fresh };
        enabled = enabledModels(effective);
        selection = resolveSelection(enabled, [], effective.modelSelection);
      }
    }
    if (selection.invalidReason) {
      showToast(
        selection.invalidReason === "composer.send_disabled_no_model"
          ? "Enable a model in the Models panel first"
          : "Select at least one model",
        { kind: "error" },
      );
      return;
    }

    const editing = current.editingLatest;
    const prompt = editing ? current.editText : current.composerText;
    const attachments = editing ? current.editAttachments : current.composerAttachments;
    if (!prompt.trim() && attachments.length === 0) return;

    const entry = draftOf(conversationId);
    const mode: "normal" | "edit" = editing ? "edit" : "normal";
    const slot = mode === "edit" ? entry.edit : entry.normal;
    // Freeze the snapshot: the content **and** the local version. The
    // version tells a new input apart from the same text typed again.
    const snapshot: SendSnapshot = {
      conversationId,
      mode,
      text: prompt,
      attachments,
      version: slot.version,
      branchSelection: [...current.branchSelection],
    };

    // A pending autosave must not write the snapshot again after the flush:
    // the write queue of the conversation takes over from here.
    cancelPendingSave(slot);

    // Flush the composer **before** the snapshot: the backend only consumes
    // the draft when the stored revision still equals the
    // `expected_draft_revision` we send, so the revision has to come from
    // the flush itself.
    const flushed = await flushBeforeSend(conversationId, mode);
    const revision = flushed ?? entry.revision;

    const request: SendMessageRequest = {
      conversation_id: conversationId,
      mode,
      prompt,
      attachments,
      expected_draft_revision: revision,
      selection: buildSelection(
        effective,
        selection.targets,
        selection.primary,
        current.branchSelection,
      ),
    };

    // Anthropic models are gated on the network region.
    const hasAnthropic = selection.targets.some((modelId) => isAnthropic(modelId));
    if (hasAnthropic) {
      egressIntent.current += 1;
      const intent = egressIntent.current;
      try {
        const egress = await client.checkEgress();
        dispatch({ type: "egress/loaded", egress });
        if (egress.verdict !== "allowed") {
          if (intent !== egressIntent.current) return;
          pendingSend.current = request;
          pendingSnapshot.current = snapshot;
          dispatch({
            type: "egress/modal",
            modal: {
              unknown: egress.verdict === "unknown",
              country: egress.country_code,
              detail: egress.detail,
              mixed: selection.targets.some((modelId) => !isAnthropic(modelId)),
              checking: false,
            },
          });
          return;
        }
      } catch (error) {
        showToast(`Network region check failed: ${describe(error)}`, { kind: "error" });
        return;
      }
    }

    // Diagnostic: the last submitted payload (native smoke harness reads it).
    (globalThis as { __AICOUNCIL_LAST_SEND__?: unknown }).__AICOUNCIL_LAST_SEND__ =
      request;
    await dispatchSend(request, snapshot);
  }, [
    buildSelection,
    cancelPendingSave,
    client,
    dispatchSend,
    draftOf,
    flushBeforeSend,
    refreshPreferences,
    showToast,
  ]);

  const dismissEgressModal = useCallback(() => {
    pendingSend.current = null;
    dispatch({ type: "egress/modal", modal: null });
  }, [dispatch]);

  const recheckEgress = useCallback(async () => {
    dispatch({
      type: "egress/modal",
      modal: stateRef.current.egressModal
        ? { ...stateRef.current.egressModal, checking: true }
        : null,
    });
    try {
      const egress = await client.checkEgress();
      dispatch({ type: "egress/loaded", egress });
      if (egress.verdict === "allowed") {
        dispatch({ type: "egress/modal", modal: null });
        showToast("Network region verified", { kind: "success" });
        return;
      }
      dispatch({
        type: "egress/modal",
        modal: stateRef.current.egressModal
          ? {
              ...stateRef.current.egressModal,
              checking: false,
              unknown: egress.verdict === "unknown",
              country: egress.country_code,
              detail: egress.detail,
            }
          : null,
      });
    } catch (error) {
      dispatch({
        type: "egress/modal",
        modal: stateRef.current.egressModal
          ? { ...stateRef.current.egressModal, checking: false }
          : null,
      });
      showToast(`Network region check failed: ${describe(error)}`, { kind: "error" });
    }
  }, [client, showToast]);

  const sendWithoutAnthropic = useCallback(async () => {
    const request = pendingSend.current;
    const snapshot = pendingSnapshot.current;
    pendingSend.current = null;
    pendingSnapshot.current = null;
    dispatch({ type: "egress/modal", modal: null });
    if (!request || !snapshot) return;
    const filtered = request.selection.targets?.filter(
      (target) => !isAnthropic(target.model_id),
    ) ?? [];
    if (filtered.length === 0) {
      showToast("No non-Anthropic model is selected", { kind: "error" });
      return;
    }
    await dispatchSend(
      {
        ...request,
        selection: { ...request.selection, targets: filtered, primary: 0 },
      },
      snapshot,
    );
  }, [dispatchSend, showToast]);

  const cancel = useCallback(
    async (requestId: Id) => {
      try {
        await client.cancelRequest(requestId);
      } catch (error) {
        showToast(`Could not stop: ${describe(error)}`, { kind: "error" });
      }
    },
    [client, showToast],
  );

  /**
   * Retry a failed generation.
   *
   * The backend's admission control decides whether a retry may run; its
   * verdicts are surfaced verbatim instead of a generic "not available":
   *
   *   - `admission.cooldown`   → the lineage is cooling down (Retry-After or
   *     backoff); the toast says when the retry becomes possible again;
   *   - `admission.needs_action` → something on the user's side must change
   *     first (key, balance, IP, input, model);
   *   - `admission.duplicate`  → an attempt of this lineage is already live.
   *
   * Together with the disabled countdown button this keeps dead retries
   * from ever reaching the backend.
   */
  const retry = useCallback(
    async (requestId: Id) => {
      const conversationId = stateRef.current.currentConversationId;
      try {
        await client.retryRequest(requestId);
        if (conversationId) await loadConversation(conversationId);
      } catch (error) {
        const code =
          error && typeof error === "object"
            ? String((error as { code?: unknown }).code ?? "")
            : "";
        const detail =
          error && typeof error === "object"
            ? String((error as { detail?: unknown }).detail ?? "")
            : "";
        if (code === "admission.cooldown") {
          const untilMs = Number(detail);
          const seconds = Number.isFinite(untilMs)
            ? Math.max(0, Math.ceil((untilMs - Date.now()) / 1000))
            : 0;
          showToast(t("retry.cooldown").replace("{s}", String(seconds)), {
            kind: "error",
          });
        } else if (code === "admission.needs_action") {
          const remedy = t(`retry.needs.${detail || "none"}`);
          showToast(remedy.startsWith("retry.needs.") ? t("retry.needs.none") : remedy, {
            kind: "error",
          });
        } else if (code === "admission.duplicate") {
          showToast(t("toast.retry_in_flight"), { kind: "error" });
        } else {
          showToast(
            t("toast.retry_rejected").replace("{reason}", describe(error)),
            { kind: "error" },
          );
        }
      }
    },
    [client, loadConversation, showToast],
  );

  const deleteMessage = useCallback(
    async (messageId: Id) => {
      const conversationId = stateRef.current.currentConversationId;
      try {
        await client.removeMessage(messageId);
        dispatch({ type: "message/removed", conversationId: conversationId ?? "", messageId });
      } catch (error) {
        showToast(`Could not delete message: ${describe(error)}`, { kind: "error" });
      }
    },
    [client, showToast],
  );

  const startEdit = useCallback(
    (messageId: Id) => {
      const current = stateRef.current;
      const conversationId = current.currentConversationId;
      if (!conversationId) return;
      const messages = current.messages[conversationId] ?? [];
      const message = messages.find((entry) => entry.id === messageId);
      if (!message) return;
      dispatch({
        type: "edit/start",
        messageId,
        text: message.content,
        attachments: message.attachments ?? [],
      });
      const entry = draftOf(conversationId);
      entry.editTarget = messageId;
      // The edit buffer is a local edit: the backend has to count it.
      markLocalEdit(entry.edit, {
        text: message.content,
        attachments: message.attachments ?? [],
      });
      // The edit buffer is durable: it survives a restart like the draft.
      void writeEdit(conversationId).catch((error) => {
        showToast(`Draft could not be saved: ${describe(error)}`, { kind: "error" });
      });
    },
    [dispatch, draftOf, markLocalEdit, showToast, writeEdit],
  );

  const cancelEdit = useCallback(() => {
    dispatch({ type: "edit/cancel" });
    const current = stateRef.current;
    const conversationId = current.currentConversationId;
    if (!conversationId) return;
    const entry = draftOf(conversationId);
    entry.editTarget = null;
    entry.edit.local = { text: "", attachments: [] };
    entry.edit.dirty = false;
    entry.edit.saved = { text: "", attachments: [] };
    entry.edit.syncedVersion = entry.edit.version;
    // The clear moves the revision: it runs in the write queue of the
    // conversation so it cannot overtake a running write.
    void enqueueDraftWrite(conversationId, async () => {
      const outcome = await withDeadline(
        client.clearEditBuffer(conversationId),
        DRAFT_WRITE_TIMEOUT_MS,
      );
      if (outcome.status === "done") {
        entry.revision = outcome.value.revision;
        // The revision in the state belongs to the **open** chat.
        if (stateRef.current.currentConversationId === conversationId) {
          dispatch({ type: "draft/revision", revision: outcome.value.revision });
        }
      }
    });
  }, [client, dispatch, draftOf, enqueueDraftWrite]);

  const continueFrom = useCallback(
    async (messageId: Id) => {
      const conversationId = stateRef.current.currentConversationId;
      if (!conversationId) return;
      try {
        await client.setMainLeaf(conversationId, messageId);
        await loadConversation(conversationId);
      } catch (error) {
        showToast(`Could not continue: ${describe(error)}`, { kind: "error" });
      }
    },
    [client, loadConversation, showToast],
  );

  const toggleBranch = useCallback(
    (messageId: Id) => {
      // The selection is part of the next `send_message` request; there is
      // no separate branch-selection command in the IPC contract.
      dispatch({ type: "branch/toggle", messageId });
    },
    [dispatch],
  );

  const exportMarkdown = useCallback(
    async (scope: ExportScope) => {
      const current = stateRef.current;
      const conversationId = current.currentConversationId;
      if (!conversationId) return;
      // "selected" exports the checked branch leaves.
      const messageIds =
        scope === "selected" ? [...current.branchSelection] : undefined;
      if (scope === "selected" && (!messageIds || messageIds.length === 0)) {
        showToast("Select at least one answer to export", { kind: "error" });
        return;
      }
      // 1) Native path: the backend opens the macOS save panel and writes the
      //    file atomically. The success message is only shown once the write
      //    returned the written path.
      try {
        const saved = await client.exportMarkdownToFile({
          conversation_id: conversationId,
          scope,
          message_ids: messageIds,
        });
        showToast(`Exported → ${saved.path}`, { kind: "success" });
        return;
      } catch (error) {
        // The user closing the save panel is not an error.
        if (isCancelled(error)) return;
        if (!isUnknownCommand(error)) {
          showToast(`Export failed: ${describe(error)}`, { kind: "error" });
          return;
        }
        // The backend has no native save command yet: fall back to the
        // content + browser download (development path).
      }

      try {
        const result = await client.exportMarkdown({
          conversation_id: conversationId,
          scope,
          message_ids: messageIds,
        });
        downloadTextFile(result.filename, result.content);
        showToast(`Exported → ${result.filename} (browser download)`, {
          kind: "success",
        });
      } catch (error) {
        showToast(`Export failed: ${describe(error)}`, { kind: "error" });
      }
    },
    [client, showToast],
  );

  const openModal = useCallback(
    (modal: ModalKind) => dispatch({ type: "modal/set", modal }),
    [dispatch],
  );
  const closeModal = useCallback(
    () => dispatch({ type: "modal/set", modal: null }),
    [dispatch],
  );
  const toggleSidebar = useCallback(
    (collapsed?: boolean) => setSidebarCollapsed(collapsed ?? !stateRef.current.sidebarCollapsed),
    [setSidebarCollapsed],
  );

  // ----- backend events -----

  // ----- backend events -----

  /** Apply the buffered deltas to the store (at most one update per 50 ms). */
  const flushDeltas = useCallback(() => {
    deltaTimer.current = null;
    const pending = deltaBuffer.current;
    if (pending.size === 0) return;
    deltaBuffer.current = new Map();
    for (const [messageId, entry] of pending) {
      dispatch({
        type: "stream/chunk",
        conversationId: entry.conversationId,
        messageId,
        delta: entry.delta,
      });
    }
  }, [dispatch]);

  const handleChatEvent = useCallback(
    (payload: ChatEvent) => {
      const { event } = payload;

      /**
       * Progress for the **existing** generation row: dispatched as a patch
       * the reducer merges against the current row, so synchronous
       * back-to-back events (remote id + first delta in one tick) cannot
       * overwrite each other with a stale full-object snapshot. Terminal
       * generations are never resurrected, and streaming never regresses.
       */
      const markStatus = (
        status: "connecting" | "streaming",
        remoteId?: string,
      ) => {
        dispatch({
          type: "generation/progress",
          conversationId: payload.conversation_id,
          requestId: payload.request_id,
          status,
          remoteId,
        });
      };

      // The UI must reach the authoritative refetch below **no matter what a
      // single malformed event does** (a thrown handler is how 0.2.2 froze
      // generations on "Connecting"). Warn without logging payloads.
      try {
        switch (event.type) {
          case "started": {
            // Connecting only; Streaming is reserved for the first real
            // remote response (text or remote generation id).
            markStatus("connecting");
            break;
          }
          case "text_delta": {
            markStatus("streaming");
            const messageId = payload.assistant_message_id;
            const buffered = deltaBuffer.current.get(messageId);
            deltaBuffer.current.set(messageId, {
              conversationId: payload.conversation_id,
              delta: (buffered?.delta ?? "") + event.delta,
            });
            if (deltaTimer.current === null) {
              deltaTimer.current = setTimeout(flushDeltas, DELTA_FLUSH_MS);
            }
            break;
          }
          case "remote_generation_id": {
            markStatus("streaming", event.id);
            break;
          }
          case "usage_updated": {
            // The ledger changed (a known cost, a late repair): re-read the
            // conversation so the amount in the toolbar follows the ledger.
            if (
              stateRef.current.currentConversationId === payload.conversation_id
            ) {
              void loadConversation(payload.conversation_id);
            }
            break;
          }
          case "failed": {
            // The canonical native format nests the error; a legacy/malformed
            // backend may send it flat — normalize instead of throwing.
            const legacy = event as Partial<{
              error: { message?: string };
              message?: string;
            }>;
            const message =
              event?.error?.message ?? legacy?.message ?? "Generation failed";
            showToast(message, { kind: "error" });
            break;
          }
          case "rejected": {
            showToast(`Request blocked before sending: ${event.detail}`, {
              kind: "error",
            });
            break;
          }
          case "persist_error": {
            showToast(`Storage error (state may not be saved): ${event.error}`, {
              kind: "error",
            });
            break;
          }
          default:
            break;
        }
      } catch (err) {
        console.warn("chat event handling error", err);
      }
      // The persisted state is authoritative, but only the **terminal**
      // events need a refetch: the text deltas are applied locally, and
      // refetching on every delta would flood the backend with commands
      // (and starve the commands of the rest of the app).
      const terminal =
        event.type === "finished" ||
        event.type === "failed" ||
        event.type === "cancelled" ||
        event.type === "rejected" ||
        event.type === "persist_error";
      if (
        terminal &&
        stateRef.current.currentConversationId === payload.conversation_id
      ) {
        // Apply the last buffered deltas before the authoritative refetch.
        flushDeltas();
        void loadConversation(payload.conversation_id);
      }
    },
    [flushDeltas, loadConversation, showToast],
  );

  useEffect(() => {
    const unlisteners: Array<() => void> = [];
    let cancelled = false;

    const attach = async () => {
      let handles: Array<() => void> = [];
      try {
        /*
         * Every attach registers its **own** wrapper.
         *
         * React's strict mode mounts the effect twice; when both attaches
         * register the very same function, the cleanup of the first one
         * would remove the handler of the second one as well and the app
         * would stop hearing the backend.
         */
        const onChat = (payload: ChatEvent) => handleChatEvent(payload);
        const onConversationChanged = (payload: { conversation_id: Id }) => {
          if (stateRef.current.currentConversationId === payload.conversation_id) {
            void loadConversation(payload.conversation_id);
          }
        };
        const onConversationsChanged = () => {
          void refreshConversations();
        };
        const onTitle = () => {
          void refreshConversations();
        };
        const onCatalog = (catalog: {
          entries: CatalogEntry[];
          updated_at_ms: number | null;
        }) => {
          dispatch({
            type: "catalog/loaded",
            entries: catalog.entries,
            updatedAt: catalog.updated_at_ms,
          });
          // The enabled/primary list refers to catalogue entries: keep it
          // in sync with the catalogue it describes.
          void refreshPreferences();
        };
        handles = await Promise.all([
          client.onChatEvent(onChat),
          client.onConversationChanged(onConversationChanged),
          client.onConversationsChanged(onConversationsChanged),
          client.onTitleEvent(onTitle),
          client.onCatalogChanged(onCatalog),
        ]);
      } catch (error) {
        // A missing event bridge must not block the bootstrap: the app shows
        // the error and still loads the persisted state.
        showToast(`Event bridge unavailable: ${describe(error)}`, {
          kind: "error",
        });
      }
      if (cancelled) {
        handles.forEach((off) => off());
        return;
      }
      unlisteners.push(...handles);
      // `bootstrap` must run after the subscriptions are attached.
      void bootstrap();
    };

    void attach();
    return () => {
      cancelled = true;
      unlisteners.forEach((off) => off());
    };
  }, [
    bootstrap,
    client,
    handleChatEvent,
    loadConversation,
    refreshConversations,
    refreshPreferences,
    showToast,
  ]);

  // ----- draft autosave -----

  /**
   * Save the composer draft of the open chat 600 ms after the last edit.
   *
   * The timer is only a debounce: the write itself reads the content
   * **when it runs**, so a save that was scheduled before a send can never
   * write the consumed snapshot again.
   *
   * An empty composer is a draft too: deleting the last character has to be
   * persisted, otherwise the old text comes back when the chat is opened
   * again.
   */
  useEffect(() => {
    const conversationId = state.currentConversationId;
    if (!conversationId) return;
    const entry = draftOf(conversationId);
    const slot = entry.normal;
    // Keep the slot in line with what the composer shows (the content
    // belongs to the open chat only).
    if (composerOwner.current === conversationId) {
      markLocalEdit(slot, {
        text: state.composerText,
        attachments: state.composerAttachments,
      });
    }
    // Nothing to do when the backend already holds this exact content and
    // the content is not a new edit: saving again would emit another change
    // event, which would trigger another refetch, which would schedule
    // another save … an endless cycle.
    if (slot.saved && sameContent(slot.saved, slot.local) && !slot.dirty) {
      return;
    }
    const timer = setTimeout(() => {
      slot.timer = null;
      void writeNormal(conversationId);
    }, 600);
    slot.timer = timer;
    return () => {
      clearTimeout(timer);
      if (slot.timer === timer) slot.timer = null;
    };
  }, [
    draftOf,
    markLocalEdit,
    state.currentConversationId,
    state.composerText,
    state.composerAttachments,
    writeNormal,
  ]);

  // ----- edit buffer autosave -----

  useEffect(() => {
    const conversationId = state.currentConversationId;
    const targetId = state.editingLatest;
    if (!conversationId || !targetId) return;
    const entry = draftOf(conversationId);
    const slot = entry.edit;
    markLocalEdit(slot, {
      text: state.editText,
      attachments: state.editAttachments,
    });
    if (slot.saved && sameContent(slot.saved, slot.local) && !slot.dirty) {
      return;
    }
    const timer = setTimeout(() => {
      slot.timer = null;
      void writeEdit(conversationId);
    }, 600);
    slot.timer = timer;
    return () => {
      clearTimeout(timer);
      if (slot.timer === timer) slot.timer = null;
    };
  }, [
    draftOf,
    markLocalEdit,
    state.currentConversationId,
    state.editingLatest,
    state.editText,
    state.editAttachments,
    writeEdit,
  ]);

  const actions = useMemo<Actions>(
    () => ({
      bootstrap,
      selectConversation,
      refreshConversations,
      createConversation,
      renameConversation,
      deleteConversation,
      moveConversation,
      createGroup,
      renameGroup,
      deleteGroup,
      setModelEnabled,
      setPrimaryModel,
      setModelOverrides,
      reorderModels,
      setParallel,
      toggleChecked,
      setChecked,
      refreshCatalog,
      refreshPreferences,
      reloadCredentials,
      refreshKeyStatus,
      updateSettings,
      setSidebarCollapsed,
      setSidebarWidth,
      send,
      cancel,
      retry,
      deleteMessage,
      startEdit,
      cancelEdit,
      setComposerText,
      setComposerAttachments,
      setEditText,
      setEditAttachments,
      addAttachments,
      addPastedText,
      removeAttachment,
      continueFrom,
      toggleBranch,
      exportMarkdown,
      openModal,
      closeModal,
      toggleSidebar,
      showToast,
      dismissToast,
      dismissEgressModal,
      recheckEgress,
      sendWithoutAnthropic,
    }),
    [
      bootstrap,
      selectConversation,
      refreshConversations,
      createConversation,
      renameConversation,
      deleteConversation,
      moveConversation,
      createGroup,
      renameGroup,
      deleteGroup,
      setModelEnabled,
      setPrimaryModel,
      setModelOverrides,
      reorderModels,
      setParallel,
      toggleChecked,
      setChecked,
      refreshCatalog,
      refreshPreferences,
      reloadCredentials,
      refreshKeyStatus,
      updateSettings,
      setSidebarCollapsed,
      setSidebarWidth,
      send,
      cancel,
      retry,
      deleteMessage,
      startEdit,
      cancelEdit,
      setComposerText,
      setComposerAttachments,
      setEditText,
      setEditAttachments,
      addAttachments,
      addPastedText,
      removeAttachment,
      continueFrom,
      toggleBranch,
      exportMarkdown,
      openModal,
      closeModal,
      toggleSidebar,
      showToast,
      dismissToast,
      dismissEgressModal,
      recheckEgress,
      sendWithoutAnthropic,
    ],
  );

  const value = useMemo<StoreValue>(
    () => ({ state, dispatch, actions }),
    [state, dispatch, actions],
  );

  return <StoreContext.Provider value={value}>{children}</StoreContext.Provider>;
}

export function useStore(): StoreValue {
  const value = useContext(StoreContext);
  if (!value) throw new Error("useStore must be used inside <StoreProvider>");
  return value;
}

/** Compare two attachment lists by value. */
function sameAttachments(
  a: { name: string; content: string }[] | undefined,
  b: { name: string; content: string }[] | undefined,
): boolean {
  const left = a ?? [];
  const right = b ?? [];
  if (left.length !== right.length) return false;
  return left.every(
    (item, index) =>
      item.name === right[index].name && item.content === right[index].content,
  );
}

/** Compare two id lists by value (order matters for the branch selection). */
function sameIds(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((id, index) => id === b[index]);
}

/**
 * The sort order of a model that joins the panel.
 *
 * It goes to the end of the list, so enabling a model never reshuffles the
 * models the user arranged.
 */
function nextSortOrder(preferences: ModelPreferenceEntry[]): number {
  let highest = -1;
  for (const preference of preferences) {
    const order = preference.sort_order ?? 0;
    if (order > highest) highest = order;
  }
  return highest + 1;
}

/** True when the user dismissed the native save panel. */
function isCancelled(error: unknown): boolean {
  if (!error || typeof error !== "object") return false;
  return String((error as { code?: unknown }).code ?? "") === "cancelled";
}

/** True when the backend does not implement the requested command yet. */
function isUnknownCommand(error: unknown): boolean {
  if (!error || typeof error !== "object") return false;
  const code = String((error as { code?: unknown }).code ?? "");
  const message =
    error instanceof Error ? error.message : String((error as { message?: unknown }).message ?? "");
  return code === "not_found" || /unknown command|not implemented/i.test(message);
}

function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

function describeRejection(reason: string | undefined, fileName?: string): string {
  switch (reason) {
    case "too_many":
      return "Attachment limit reached";
    case "file_too_large":
      return `${fileName ?? "Attachment"} is larger than 256KB`;
    case "total_too_large":
      return "Attachments are limited to 1MB in total";
    default:
      return "Attachment could not be added";
  }
}

async function readTextFile(file: File): Promise<string | null> {
  try {
    return await file.text();
  } catch {
    return null;
  }
}

/**
 * Save an exported Markdown file.
 *
 * The backend returns the content; the frontend stores it. In the browser
 * this triggers a download, in the native shell the same anchor download
 * hands the file to the webview's download handler.
 */
export function downloadTextFile(filename: string, content: string): void {
  const blob = new Blob([content], { type: "text/markdown;charset=utf-8" });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.rel = "noopener";
  document.body.appendChild(anchor);
  anchor.click();
  document.body.removeChild(anchor);
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export type { Action, AppState, ConversationSummary, Message };
