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
import type {
  Attachment,
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
import { initialState, type AppState, type Panel } from "./types";
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
  addAttachments(files: File[]): Promise<void>;
  addPastedText(text: string): void;
  removeAttachment(index: number): void;
  continueFrom(messageId: Id): Promise<void>;
  toggleBranch(messageId: Id): void;
  exportMarkdown(scope: ExportScope): Promise<void>;
  setPanel(panel: Panel): void;
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
  /** Guards the draft autosave against out-of-order replies. */
  const draftToken = useRef(0);
  /** Monotonic id for the egress intent (matches the native behaviour). */
  const egressIntent = useRef(0);
  /**
   * Per conversation: the composer content the backend is known to hold.
   *
   * A refetch may only overwrite the composer when it still matches this
   * baseline — otherwise input the user typed meanwhile would be lost.
   */
  const draftBaselines = useRef(
    new Map<Id, { text: string; attachments: Attachment[] }>(),
  );
  /** Per conversation: the last content we actually wrote to the backend. */
  const lastSavedDrafts = useRef(
    new Map<Id, { text: string; attachments: Attachment[] }>(),
  );
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

  const showToast = useCallback(
    (text: string, options?: ToastOptions) => {
      dispatch({ type: "toast/show", text, kind: options?.kind ?? "info" });
    },
    [dispatch],
  );

  const dismissToast = useCallback(() => dispatch({ type: "toast/hide" }), [dispatch]);

  // ----- conversation helpers -----

  const applyConversationDetail = useCallback(
    (conversationId: Id, detail: Awaited<ReturnType<IpcClient["loadConversation"]>>, draft: Draft | null) => {
      dispatch({
        type: "conversation/loaded",
        conversationId,
        conversation: detail.conversation,
        messages: detail.messages,
        generations: detail.generations,
        usage: detail.usage,
        // Keep the known revision when the draft could not be read: the
        // backend uses it as the send CAS token.
        draftRevision: draft ? draft.revision : stateRef.current.draftRevision,
        // An **active** edit lives in the draft; `edit_latest` only says
        // that the latest question *may* be edited.
        editTargetId: draft?.edit_target_id ?? null,
        editText: draft?.edit_text ?? "",
        editAttachments: draft?.edit_attachments ?? [],
        editLatestEligible: detail.edit_latest?.target_id ?? null,
      });
      if (draft) {
        const baseline = draftBaselines.current.get(conversationId);
        const current = stateRef.current;
        const hasNewerInput = baseline
          ? current.composerText !== baseline.text ||
            !sameAttachments(current.composerAttachments, baseline.attachments)
          : false;
        if (!hasNewerInput) {
          dispatch({ type: "composer/text", text: draft.text });
          dispatch({ type: "composer/attachments", attachments: draft.attachments });
          draftBaselines.current.set(conversationId, {
            text: draft.text,
            attachments: draft.attachments,
          });
          lastSavedDrafts.current.set(conversationId, {
            text: draft.text,
            attachments: draft.attachments,
          });
        }
      }
    },
    [dispatch],
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
   */
  const loadConversation = useCallback(
    async (conversationId: Id): Promise<void> => {
      const token = selectionToken.current;
      const { detail, draft } = await fetchConversation(conversationId);
      if (token !== selectionToken.current) return;
      if (stateRef.current.currentConversationId !== conversationId) return;
      applyConversationDetail(conversationId, detail, draft);
    },
    [applyConversationDetail, fetchConversation],
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
          const { detail, draft } = await fetchConversation(first);
          if (token !== selectionToken.current) return;
          applyConversationDetail(first, detail, draft);
          dispatch({ type: "conversation/selected", conversationId: first });
        } catch (error) {
          showToast(`Failed to open chat: ${describe(error)}`, { kind: "error" });
        }
      }
    } catch (error) {
      dispatch({ type: "boot/failed", error: describe(error) });
    }
  }, [applyConversationDetail, client, fetchConversation, showToast]);

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
      if (
        previousId &&
        previousId !== conversationId &&
        (current.composerText.trim().length > 0 ||
          current.composerAttachments.length > 0)
      ) {
        await persistDraft(
          previousId,
          current.composerText,
          current.composerAttachments,
          true,
        );
      }
      selectionToken.current += 1;
      const token = selectionToken.current;
      dispatch({ type: "conversation/selected", conversationId });
      if (!conversationId) return;
      try {
        const { detail, draft } = await fetchConversation(conversationId);
        // A newer selection happened meanwhile: drop the stale snapshot
        // *before* it can touch the composer or the message list.
        if (token !== selectionToken.current) return;
        applyConversationDetail(conversationId, detail, draft);
      } catch (error) {
        if (token !== selectionToken.current) return;
        showToast(`Failed to open chat: ${describe(error)}`, { kind: "error" });
      }
    },
    [loadConversation, showToast],
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

  const setModelEnabled = useCallback(
    async (modelId: string, enabled: boolean) => {
      const current = stateRef.current;
      const preference = current.preferences.find(
        (entry) => entry.model_id === modelId,
      );
      if (!preference) return;
      const next = {
        enabled,
        sort_order: preference.sort_order,
        is_default: preference.is_default,
        overrides: preference.overrides,
      };
      dispatch({ type: "model/enabled", modelId, enabled });
      try {
        await client.setModelPreference({
          provider_id: preference.provider_id,
          model_id: modelId,
          preference: next,
        });
      } catch (error) {
        dispatch({ type: "model/enabled", modelId, enabled: !enabled });
        showToast(`Could not update model: ${describe(error)}`, { kind: "error" });
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

  const setParallel = useCallback(
    (value: boolean) => {
      dispatch({ type: "model/parallel", value });
      if (!value) return;
      const current = stateRef.current;
      const enabled = enabledModels(current);
      const checked = current.modelSelection.checked.filter((id) => enabled.includes(id));
      if (checked.length === 0) {
        const primary =
          current.modelSelection.primary && enabled.includes(current.modelSelection.primary)
            ? current.modelSelection.primary
            : (enabled[0] ?? null);
        dispatch({ type: "model/checkedSet", modelIds: primary ? [primary] : [] });
      }
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

  const setComposerText = useCallback(
    (text: string) => dispatch({ type: "composer/text", text }),
    [dispatch],
  );

  const setComposerAttachments = useCallback(
    (attachments: Attachment[]) =>
      dispatch({ type: "composer/attachments", attachments }),
    [dispatch],
  );

  const persistDraft = useCallback(
    async (conversationId: Id, text: string, attachments: Attachment[], changed: boolean) => {
      draftToken.current += 1;
      const token = draftToken.current;
      try {
        const result = await client.saveDraft({
          conversation_id: conversationId,
          text,
          attachments,
          changed,
        });
        if (token !== draftToken.current) return;
        dispatch({ type: "draft/revision", revision: result.revision });
        // The backend now holds exactly this content.
        draftBaselines.current.set(conversationId, { text, attachments });
        lastSavedDrafts.current.set(conversationId, { text, attachments });
      } catch (error) {
        if (token !== draftToken.current) return;
        showToast(`Draft could not be saved: ${describe(error)}`, { kind: "error" });
      }
    },
    [client, showToast],
  );

  /**
   * Flush the composer draft before a send and return the revision the
   * backend handed back.
   *
   * The backend consumes the draft only if the stored revision still equals
   * the `expected_draft_revision` we send, so the flush has to happen
   * **before** the snapshot. The flush is time-boxed: a slow backend must
   * never block the composer.
   */
  const flushDraftBeforeSend = useCallback(
    async (
      conversationId: Id,
      mode: "normal" | "edit",
      text: string,
      attachments: Attachment[],
      targetId: Id | null,
    ): Promise<number | null> => {
      const write =
        mode === "edit"
          ? client.saveEditBuffer({
              conversation_id: conversationId,
              text,
              attachments,
              target_id: targetId,
              changed: true,
            })
          : client.saveDraft({
              conversation_id: conversationId,
              text,
              attachments,
              changed: true,
            });
      const guarded = write.catch((error) => {
        showToast(`Draft could not be saved: ${describe(error)}`, {
          kind: "error",
        });
        return null;
      });
      const timeout = new Promise<null>((resolve) => {
        setTimeout(() => resolve(null), DRAFT_FLUSH_TIMEOUT_MS);
      });
      const result = await Promise.race([guarded, timeout]);
      if (result && typeof result.revision === "number") {
        dispatch({ type: "draft/revision", revision: result.revision });
        draftBaselines.current.set(conversationId, { text, attachments });
        lastSavedDrafts.current.set(conversationId, { text, attachments });
        return result.revision;
      }
      return null;
    },
    [client, showToast],
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
      showToast(`Long paste attached (${text.length} chars)`);
    },
    [showToast],
  );

  const removeAttachment = useCallback(
    (index: number) => {
      const current = stateRef.current;
      const next = current.composerAttachments.filter((_, i) => i !== index);
      dispatch({ type: "composer/attachments", attachments: next });
    },
    [dispatch],
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
   * Submit a send and reconcile.
   *
   * The submission is asynchronous: while it is in flight the user may type
   * new input, attach files, pick other branches or switch to another chat.
   * The composer and the branch selection are therefore only cleared when
   * they still hold **exactly what was submitted** — a newer edit or another
   * chat is never touched.
   */
  const dispatchSend = useCallback(
    async (
      request: SendMessageRequest,
      submitted: {
        conversationId: Id;
        revision: number;
        text: string;
        attachments: Attachment[];
        branchSelection: Id[];
      },
    ) => {
      const conversationId = submitted.conversationId;
      try {
        const receipt = await client.sendMessage(request);
        // The receipt is advisory; the DB is authoritative. Refetch.
        const current = stateRef.current;
        if (current.currentConversationId === conversationId) {
          const composerUnchanged =
            current.composerText === submitted.text &&
            sameAttachments(current.composerAttachments, submitted.attachments);
          if (composerUnchanged) {
            dispatch({ type: "composer/clear" });
            draftBaselines.current.set(conversationId, {
              text: "",
              attachments: [],
            });
            lastSavedDrafts.current.set(conversationId, {
              text: "",
              attachments: [],
            });
          }
          if (sameIds(current.branchSelection, submitted.branchSelection)) {
            dispatch({ type: "branch/selection", messageIds: [] });
          }
          await loadConversation(conversationId);
        }
        void receipt;
      } catch (error) {
        showToast(`Send failed (draft kept): ${describe(error)}`, { kind: "error" });
      }
    },
    [client, loadConversation, showToast],
  );

  const send = useCallback(async () => {
    const current = stateRef.current;
    const conversationId = current.currentConversationId;
    if (!conversationId) return;

    let enabled = enabledModels(current);
    let selection = resolveSelection(enabled, [], current.modelSelection);
    if (selection.invalidReason === "composer.send_disabled_no_model") {
      // The cached preference list may be stale (the preferences can change
      // outside the webview). Re-read it once before refusing the send, and
      // use the **fresh** list: the store ref is only updated on the next
      // render, which has not happened yet at this point.
      const fresh = await refreshPreferences();
      if (fresh) {
        enabled = enabledModels({ ...stateRef.current, preferences: fresh });
        selection = resolveSelection(enabled, [], stateRef.current.modelSelection);
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

    // Flush the composer draft **before** the snapshot: the backend only
    // consumes the draft when the stored revision still equals the
    // `expected_draft_revision` we send, so the revision has to come from
    // the flush itself.
    let revision = current.draftRevision;
    const saved = lastSavedDrafts.current.get(conversationId);
    const alreadySaved =
      saved !== undefined &&
      saved.text === prompt &&
      sameAttachments(saved.attachments, attachments);
    if (!alreadySaved) {
      const flushed = await flushDraftBeforeSend(
        conversationId,
        editing ? "edit" : "normal",
        prompt,
        attachments,
        editing,
      );
      if (flushed !== null) revision = flushed;
    }

    const request: SendMessageRequest = {
      conversation_id: conversationId,
      mode: editing ? "edit" : "normal",
      prompt,
      attachments,
      expected_draft_revision: revision,
      selection: buildSelection(current, selection.targets, selection.primary, current.branchSelection),
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
    await dispatchSend(request, {
      conversationId,
      revision: current.draftRevision,
      text: prompt,
      attachments,
      branchSelection: [...current.branchSelection],
    });
  }, [
    buildSelection,
    client,
    dispatchSend,
    flushDraftBeforeSend,
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
    pendingSend.current = null;
    dispatch({ type: "egress/modal", modal: null });
    if (!request) return;
    const current = stateRef.current;
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
      {
        conversationId: request.conversation_id,
        revision: current.draftRevision,
        text: request.prompt,
        attachments: request.attachments,
        branchSelection: [...current.branchSelection],
      },
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

  const retry = useCallback(
    async (requestId: Id) => {
      const conversationId = stateRef.current.currentConversationId;
      try {
        await client.retryRequest(requestId);
        if (conversationId) await loadConversation(conversationId);
      } catch (error) {
        showToast(`Retry not available: ${describe(error)}`, { kind: "error" });
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
      // The edit buffer is durable: it survives a restart like the draft.
      void client
        .saveEditBuffer({
          conversation_id: conversationId,
          text: message.content,
          attachments: message.attachments ?? [],
          target_id: messageId,
          changed: true,
        })
        .catch((error) => {
          showToast(`Draft could not be saved: ${describe(error)}`, { kind: "error" });
        });
    },
    [client, dispatch, showToast],
  );

  const cancelEdit = useCallback(() => {
    dispatch({ type: "edit/cancel" });
    const current = stateRef.current;
    if (current.currentConversationId) {
      void client
        .clearEditBuffer(current.currentConversationId)
        .catch(() => undefined);
    }
  }, [client, dispatch]);

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

  const setPanel = useCallback(
    (panel: Panel) => dispatch({ type: "panel/set", panel }),
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
      switch (event.type) {
        case "started": {
          dispatch({
            type: "generation/upserted",
            conversationId: payload.conversation_id,
            generation: {
              request_id: payload.request_id,
              batch_id: payload.batch_id,
              user_message_id: "",
              assistant_message_id: payload.assistant_message_id,
              provider_id: "",
              model_id: "",
              status: "streaming",
              system_prompt: null,
              price_prompt: null,
              price_completion: null,
              remote_generation_id: null,
              error: null,
              retry_of: null,
              retry_root_id: null,
              retry_not_before_ms: null,
              dispatch_phase: "attempted",
              failure_kind: null,
              failure_message: null,
              started_at_ms: Date.now(),
              finished_at_ms: null,
            },
          });
          break;
        }
        case "text_delta": {
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
        case "failed": {
          showToast(event.error.message, { kind: "error" });
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
        handles = await Promise.all([
          client.onChatEvent(handleChatEvent),
          client.onConversationChanged((payload) => {
            if (stateRef.current.currentConversationId === payload.conversation_id) {
              void loadConversation(payload.conversation_id);
            }
          }),
          client.onConversationsChanged(() => {
            void refreshConversations();
          }),
          client.onTitleEvent(() => {
            void refreshConversations();
          }),
          client.onCatalogChanged((catalog) => {
            dispatch({
              type: "catalog/loaded",
              entries: catalog.entries,
              updatedAt: catalog.updated_at_ms,
            });
            // The enabled/primary list refers to catalogue entries: keep it
            // in sync with the catalogue it describes.
            void refreshPreferences();
          }),
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

  useEffect(() => {
    const conversationId = state.currentConversationId;
    if (!conversationId) return;
    if (!state.composerText && state.composerAttachments.length === 0) return;
    // Nothing to do when the backend already holds this exact content:
    // saving again would emit another change event, which would trigger
    // another refetch, which would schedule another save … an endless cycle.
    const saved = lastSavedDrafts.current.get(conversationId);
    if (
      saved &&
      saved.text === state.composerText &&
      sameAttachments(saved.attachments, state.composerAttachments)
    ) {
      return;
    }
    const timer = setTimeout(() => {
      void persistDraft(
        conversationId,
        state.composerText,
        state.composerAttachments,
        true,
      );
    }, 600);
    return () => clearTimeout(timer);
  }, [
    state.currentConversationId,
    state.composerText,
    state.composerAttachments,
    persistDraft,
  ]);

  // ----- edit buffer autosave -----

  useEffect(() => {
    const conversationId = state.currentConversationId;
    const targetId = state.editingLatest;
    if (!conversationId || !targetId) return;
    const timer = setTimeout(() => {
      void client
        .saveEditBuffer({
          conversation_id: conversationId,
          text: state.editText,
          attachments: state.editAttachments,
          target_id: targetId,
          changed: true,
        })
        .catch((error) => {
          showToast(`Draft could not be saved: ${describe(error)}`, { kind: "error" });
        });
    }, 600);
    return () => clearTimeout(timer);
  }, [
    client,
    showToast,
    state.currentConversationId,
    state.editingLatest,
    state.editText,
    state.editAttachments,
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
      addAttachments,
      addPastedText,
      removeAttachment,
      continueFrom,
      toggleBranch,
      exportMarkdown,
      setPanel,
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
      addAttachments,
      addPastedText,
      removeAttachment,
      continueFrom,
      toggleBranch,
      exportMarkdown,
      setPanel,
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
