/**
 * Pure reducer for the application state.
 *
 * Every rule that the native app implements in `src/app/mod.rs` and
 * `src/app/answers.rs` is mirrored here so the behaviour can be verified
 * without a browser or a backend.
 */

import type {
  CatalogEntry,
  ConversationSummary,
  EgressState,
  Generation,
  Group,
  Id,
  KeyStatus,
  Message,
  ModelPreferenceEntry,
  Settings,
  UsageRecord,
} from "../ipc/types";
import { initialState, type AppState, type EgressModalState, type ModalKind, type ModelSelectionState, type ToastKind } from "./types";

export type Action =
  | { type: "boot/started" }
  | { type: "boot/failed"; error: string }
  | {
      type: "boot/loaded";
      appVersion: string;
      conversations: ConversationSummary[];
      groups: Group[];
      pendingNewConversation: Id | null;
      catalog: CatalogEntry[];
      catalogUpdatedAt: number | null;
      preferences: ModelPreferenceEntry[];
      credentials: Record<string, boolean>;
      settings: Settings;
      egress: EgressState;
    }
  | { type: "conversations/loaded"; conversations: ConversationSummary[] }
  | { type: "groups/loaded"; groups: Group[] }
  | {
      type: "conversation/loaded";
      conversationId: Id;
      conversation: ConversationSummary;
      messages: Message[];
      generations: Generation[];
      usage: UsageRecord[];
      draftRevision: number;
      editTargetId: Id | null;
      editText: string;
      editAttachments: Message["attachments"];
      editLatestEligible: Id | null;
      /**
       * The local edit buffer is newer than the backend's: the snapshot must
       * not throw it away.
       */
      preserveEditBuffer?: boolean;
    }
  | { type: "conversation/upserted"; conversation: ConversationSummary }
  | { type: "conversation/removed"; conversationId: Id }
  | { type: "conversation/selected"; conversationId: Id | null }
  | {
      type: "message/upserted";
      conversationId: Id;
      message: Message;
    }
  | { type: "message/removed"; conversationId: Id; messageId: Id }
  | { type: "generation/upserted"; conversationId: Id; generation: Generation }
  | {
      type: "generation/progress";
      conversationId: Id;
      requestId: Id;
      status?: "connecting" | "streaming";
      remoteId?: string | null;
    }
  | {
      type: "stream/chunk";
      conversationId: Id;
      messageId: Id;
      delta: string;
    }
  | { type: "catalog/refreshing"; value: boolean }
  | {
      type: "catalog/loaded";
      entries: CatalogEntry[];
      updatedAt: number | null;
    }
  | { type: "preferences/loaded"; preferences: ModelPreferenceEntry[] }
  | {
      type: "model/enabled";
      modelId: string;
      enabled: boolean;
      /**
       * The preference of a model that has none yet.
       *
       * `undefined` leaves the list untouched (the model already has a
       * preference), an entry adds it (upsert) and `null` removes it again
       * (the rollback of a failed write).
       */
      preference?: ModelPreferenceEntry | null;
    }
  | { type: "model/primary"; modelId: string }
  | {
      type: "model/overrides";
      modelId: string;
      overrides: Record<string, unknown> | null;
    }
  | { type: "model/parallel"; value: boolean }
  | { type: "model/checked"; modelId: string; checked: boolean }
  | { type: "model/checkedSet"; modelIds: string[] }
  | { type: "branch/selection"; messageIds: Id[] }
  | { type: "branch/toggle"; messageId: Id }
  | { type: "composer/text"; text: string }
  | { type: "composer/attachments"; attachments: Message["attachments"] }
  | { type: "composer/clear" }
  | { type: "edit/clear" }
  | { type: "draft/revision"; revision: number }
  | { type: "edit/start"; messageId: Id; text: string; attachments: Message["attachments"] }
  | { type: "edit/text"; text: string }
  | { type: "edit/attachments"; attachments: Message["attachments"] }
  | { type: "edit/cancel" }
  | { type: "modal/set"; modal: ModalKind | null }
  | { type: "sidebar/toggle"; collapsed?: boolean }
  | { type: "sidebar/width"; width: number }
  | { type: "conversation/search"; query: string }
  | { type: "model/search"; query: string }
  | { type: "picker/search"; query: string }
  | { type: "picker/toggle"; open?: boolean }
  | { type: "attempts/toggle"; conversationId: Id }
  | { type: "split/toggle"; conversationId: Id }
  | { type: "answer/selected"; questionId: string; messageId: string }
  | { type: "usage/loaded"; conversationId: Id; usage: UsageRecord[] }
  | { type: "credentials/loaded"; credentials: Record<string, boolean> }
  | { type: "credentials/reloading"; value: boolean }
  | { type: "keyStatus/loaded"; status: KeyStatus | null }
  | { type: "keyStatus/loading"; value: boolean }
  | { type: "settings/loaded"; settings: Settings }
  | { type: "egress/loaded"; egress: EgressState }
  | { type: "egress/modal"; modal: EgressModalState | null }
  | { type: "toast/show"; text: string; kind?: ToastKind }
  | { type: "toast/hide" };

function upsertById<T extends { id: string }>(items: T[], item: T): T[] {
  const index = items.findIndex((entry) => entry.id === item.id);
  if (index < 0) return [item, ...items];
  const next = items.slice();
  next[index] = item;
  return next;
}

/** The enabled model ids, in the panel order. */
function enabledIds(preferences: ModelPreferenceEntry[] | undefined): string[] {
  const list = Array.isArray(preferences) ? preferences : [];
  return list
    .slice()
    .sort((a, b) => (a.sort_order ?? 0) - (b.sort_order ?? 0))
    .filter((preference) => preference.enabled)
    .map((preference) => preference.model_id);
}

/** The default model of the enabled preferences (the star). */
function defaultModelId(
  preferences: ModelPreferenceEntry[] | undefined,
): string | null {
  const list = Array.isArray(preferences) ? preferences : [];
  return (
    list.find((preference) => preference.enabled && preference.is_default)
      ?.model_id ?? null
  );
}

/**
 * Normalize a selection.
 *
 * The primary always belongs to the checked set: when it does not, the
 * default model takes over and, if that one is not checked either, the first
 * checked model does. A model the user unchecked is never added back.
 */
function normalizeSelection(
  checked: string[],
  primary: string | null,
  defaultId: string | null,
): ModelSelectionState {
  const unique = checked.filter((id, index) => checked.indexOf(id) === index);
  const nextPrimary =
    (primary && unique.includes(primary) ? primary : null) ??
    (defaultId && unique.includes(defaultId) ? defaultId : null) ??
    unique[0] ??
    null;
  return { parallel: unique.length > 1, checked: unique, primary: nextPrimary };
}

/**
 * Keep the model selection consistent with the preferences.
 *
 * A model that is not enabled can neither be the primary nor stay checked.
 * The selection is only filled when the caller asks for it:
 *
 *   - `seed`: the app starts and picks the enabled default **once**;
 *   - `fallback`: a model left "My models" and took the last place.
 *
 * A selection the user emptied on purpose stays empty — the send is then
 * disabled instead of silently using another model.
 */
function syncSelection(
  preferences: ModelPreferenceEntry[] | undefined,
  selection: ModelSelectionState,
  options: { seed?: boolean; fallback?: boolean } = {},
): ModelSelectionState {
  const enabled = enabledIds(preferences);
  const defaultId = defaultModelId(preferences);
  const checked = selection.checked.filter((id) => enabled.includes(id));
  if (
    checked.length === 0 &&
    (options.seed || options.fallback) &&
    enabled.length > 0
  ) {
    const seed =
      selection.primary && enabled.includes(selection.primary)
        ? selection.primary
        : (defaultId ?? enabled[0]);
    return normalizeSelection([seed], selection.primary, defaultId);
  }
  return normalizeSelection(checked, selection.primary, defaultId);
}

export function reducer(state: AppState, action: Action): AppState {
  switch (action.type) {
    case "boot/started":
      return { ...state, status: "booting", bootError: null };

    case "boot/failed":
      return { ...state, status: "error", bootError: action.error };

    case "boot/loaded":
      return {
        ...state,
        status: "ready",
        bootError: null,
        appVersion: action.appVersion,
        conversations: action.conversations,
        groups: action.groups,
        catalog: action.catalog,
        catalogUpdatedAt: action.catalogUpdatedAt,
        preferences: action.preferences,
        modelSelection: syncSelection(action.preferences, state.modelSelection, {
          // The app starts with the enabled default — once.
          seed: true,
        }),
        credentials: action.credentials,
        settings: action.settings,
        sidebarCollapsed: action.settings.sidebar_collapsed,
        egress: action.egress,
      };

    case "conversations/loaded":
      return { ...state, conversations: action.conversations };

    case "groups/loaded":
      return { ...state, groups: action.groups };

    case "conversation/loaded": {
      /**
       * Keep locally-streamed deltas that the snapshot does not have yet.
       *
       * A refetch can land mid-stream (a `usage_updated` or a title event),
       * and the backend flushes streamed content to the DB only in
       * intervals. Replacing the message list wholesale would then roll the
       * visible text back to the last flush until the next delta arrived.
       * For a message whose generation is still live, the local content is
       * a strict superset of the snapshot's (deltas only ever append), so
       * the local tail wins. A **terminal** snapshot is authoritative and
       * always replaces the local text.
       */
      const liveByMessage = new Map<string, boolean>();
      for (const generation of action.generations) {
        liveByMessage.set(
          generation.assistant_message_id,
          generation.status === "queued" ||
            generation.status === "connecting" ||
            generation.status === "streaming",
        );
      }
      const previousById = new Map(
        (state.messages[action.conversationId] ?? []).map((message) => [
          message.id,
          message,
        ]),
      );
      const messages = action.messages.map((message) => {
        const previous = previousById.get(message.id);
        if (
          previous &&
          previous.content.length > message.content.length &&
          previous.content.startsWith(message.content) &&
          liveByMessage.get(message.id) === true
        ) {
          return previous;
        }
        return message;
      });
      return {
        ...state,
        conversations: upsertById(state.conversations, action.conversation),
        messages: {
          ...state.messages,
          [action.conversationId]: messages,
        },
        generations: {
          ...state.generations,
          [action.conversationId]: action.generations,
        },
        usage: { ...state.usage, [action.conversationId]: action.usage },
        draftRevision: action.draftRevision,
        // The edit buffer of the user may be newer than the backend's: it
        // must survive the refetch.
        editingLatest: action.preserveEditBuffer
          ? state.editingLatest
          : action.editTargetId,
        editText: action.preserveEditBuffer ? state.editText : action.editText,
        editAttachments: action.preserveEditBuffer
          ? state.editAttachments
          : action.editAttachments,
        editLatestEligible: action.editLatestEligible,
      };
    }

    case "conversation/upserted": {
      const conversations = upsertById(state.conversations, action.conversation);
      conversations.sort((a, b) => b.updated_at_ms - a.updated_at_ms);
      return { ...state, conversations };
    }

    case "conversation/removed": {
      const conversations = state.conversations.filter(
        (conversation) => conversation.id !== action.conversationId,
      );
      const messages = { ...state.messages };
      const generations = { ...state.generations };
      const usage = { ...state.usage };
      delete messages[action.conversationId];
      delete generations[action.conversationId];
      delete usage[action.conversationId];
      const currentConversationId =
        state.currentConversationId === action.conversationId
          ? (conversations[0]?.id ?? null)
          : state.currentConversationId;
      return {
        ...state,
        conversations,
        messages,
        generations,
        usage,
        currentConversationId,
      };
    }

    case "conversation/selected":
      return { ...state, currentConversationId: action.conversationId };

    case "message/upserted": {
      const list = state.messages[action.conversationId] ?? [];
      return {
        ...state,
        messages: {
          ...state.messages,
          [action.conversationId]: upsertById(list, action.message),
        },
      };
    }

    case "message/removed": {
      const list = state.messages[action.conversationId] ?? [];
      const removed = list.find((message) => message.id === action.messageId);
      const remaining = list.filter((message) => message.id !== action.messageId);
      const next: AppState = {
        ...state,
        messages: { ...state.messages, [action.conversationId]: remaining },
      };
      if (removed?.parent_id) {
        // Re-parent orphans to the removed node's parent so the thread stays
        // connected instead of losing a whole subtree.
        next.messages = {
          ...next.messages,
          [action.conversationId]: remaining.map((message) =>
            message.parent_id === action.messageId
              ? { ...message, parent_id: removed.parent_id }
              : message,
          ),
        };
      }
      if (state.editingLatest === action.messageId) {
        next.editingLatest = null;
        next.editText = "";
        next.editAttachments = [];
      }
      return next;
    }

    case "generation/upserted": {
      const list = state.generations[action.conversationId] ?? [];
      const index = list.findIndex(
        (generation) => generation.request_id === action.generation.request_id,
      );
      const next = list.slice();
      if (index < 0) next.push(action.generation);
      else next[index] = action.generation;
      return {
        ...state,
        generations: { ...state.generations, [action.conversationId]: next },
      };
    }

    /**
     * Progress patch for an existing generation (backend events).
     *
     * Merges against the **current** reducer row (not a snapshot taken
     * before dispatch), so synchronous back-to-back events cannot overwrite
     * each other with stale fields. A terminal generation is never
     * resurrected, and `streaming` never regresses to `connecting`.
     */
    case "generation/progress": {
      const list = state.generations[action.conversationId] ?? [];
      const index = list.findIndex(
        (generation) => generation.request_id === action.requestId,
      );
      if (index < 0) return state;
      const existing = list[index];
      if (
        existing.status === "completed" ||
        existing.status === "cancelled" ||
        existing.status === "failed" ||
        existing.status === "interrupted"
      ) {
        return state;
      }
      const status =
        action.status === "connecting" && existing.status === "streaming"
          ? "streaming"
          : (action.status ?? existing.status);
      const remote_generation_id =
        action.remoteId !== undefined ? action.remoteId : existing.remote_generation_id;
      if (status === existing.status && remote_generation_id === existing.remote_generation_id) {
        return state;
      }
      const next = list.slice();
      next[index] = { ...existing, status, remote_generation_id };
      return {
        ...state,
        generations: { ...state.generations, [action.conversationId]: next },
      };
    }

    case "stream/chunk": {
      const list = state.messages[action.conversationId] ?? [];
      const index = list.findIndex((message) => message.id === action.messageId);
      if (index < 0) return state;
      const current = list[index];
      const next = list.slice();
      next[index] = { ...current, content: current.content + action.delta };
      return {
        ...state,
        messages: { ...state.messages, [action.conversationId]: next },
      };
    }

    case "catalog/refreshing":
      return { ...state, catalogRefreshing: action.value };

    case "catalog/loaded":
      return {
        ...state,
        catalog: action.entries,
        catalogUpdatedAt: action.updatedAt,
        catalogRefreshing: false,
      };

    case "preferences/loaded":
      return {
        ...state,
        preferences: action.preferences,
        modelSelection: syncSelection(action.preferences, state.modelSelection),
      };

    case "model/enabled": {
      const index = state.preferences.findIndex(
        (preference) => preference.model_id === action.modelId,
      );
      let preferences: ModelPreferenceEntry[];
      if (action.preference === null) {
        if (index < 0) return state;
        preferences = state.preferences.filter(
          (preference) => preference.model_id !== action.modelId,
        );
      } else if (index < 0) {
        if (!action.preference) return state;
        // A catalog model without a preference joins "My models".
        preferences = [
          ...state.preferences,
          { ...action.preference, enabled: action.enabled },
        ];
      } else {
        preferences = state.preferences.map((preference) =>
          preference.model_id === action.modelId
            ? { ...preference, enabled: action.enabled }
            : preference,
        );
      }

      // The selection must stay valid: a model that left "My models" can
      // neither stay the primary nor stay checked.
      const enabled = enabledIds(preferences);
      const defaultId = defaultModelId(preferences);
      const wasChecked = state.modelSelection.checked.includes(action.modelId);
      const checked = state.modelSelection.checked.filter((id) =>
        enabled.includes(id),
      );
      // The model that left "My models" took the last place: a remaining
      // enabled model takes over, so the send is not dead. A selection the
      // user emptied on purpose stays empty.
      const next =
        wasChecked && checked.length === 0 && enabled.length > 0
          ? [defaultId ?? enabled[0]]
          : checked;
      return {
        ...state,
        preferences,
        modelSelection: normalizeSelection(
          next,
          state.modelSelection.primary,
          defaultId,
        ),
      };
    }

    case "model/primary": {
      const enabled = enabledIds(state.preferences);
      const checked = state.modelSelection.checked.includes(action.modelId)
        ? state.modelSelection.checked
        : enabled.includes(action.modelId)
          ? [...state.modelSelection.checked, action.modelId]
          : state.modelSelection.checked;
      return {
        ...state,
        preferences: state.preferences.map((preference) => ({
          ...preference,
          is_default: preference.model_id === action.modelId,
        })),
        // The primary belongs to the selection — that is what makes it the
        // owner of the main branch.
        modelSelection: {
          parallel: checked.length > 1,
          checked,
          primary: action.modelId,
        },
      };
    }

    case "model/overrides":
      return {
        ...state,
        preferences: state.preferences.map((preference) =>
          preference.model_id === action.modelId
            ? { ...preference, overrides: action.overrides }
            : preference,
        ),
      };

    case "model/parallel": {
      // The parallel flag is derived from the selection — this action only
      // exists for the compatibility with the old callers and maps onto a
      // real selection.
      const enabled = enabledIds(state.preferences);
      const defaultId = defaultModelId(state.preferences);
      if (action.value) {
        // Several models: every enabled model answers.
        return {
          ...state,
          modelSelection: normalizeSelection(
            enabled,
            state.modelSelection.primary,
            defaultId,
          ),
        };
      }
      // One model: only the primary stays checked.
      const primary =
        state.modelSelection.primary && enabled.includes(state.modelSelection.primary)
          ? state.modelSelection.primary
          : (defaultId ?? enabled[0] ?? null);
      return {
        ...state,
        modelSelection: normalizeSelection(
          primary ? [primary] : [],
          primary,
          defaultId,
        ),
      };
    }

    case "model/checked": {
      const current = state.modelSelection.checked;
      const checked = action.checked
        ? current.includes(action.modelId)
          ? current
          : [...current, action.modelId]
        : current.filter((id) => id !== action.modelId);
      // Unchecking the last model leaves the selection empty on purpose:
      // the send is disabled instead of using another model behind the
      // user's back.
      return {
        ...state,
        modelSelection: normalizeSelection(
          checked,
          state.modelSelection.primary,
          defaultModelId(state.preferences),
        ),
      };
    }

    case "model/checkedSet":
      return {
        ...state,
        modelSelection: normalizeSelection(
          [...action.modelIds],
          state.modelSelection.primary,
          defaultModelId(state.preferences),
        ),
      };

    case "branch/selection":
      return { ...state, branchSelection: [...action.messageIds] };

    case "branch/toggle": {
      // Pure toggle: successive dispatches compose correctly.
      const selected = state.branchSelection.includes(action.messageId)
        ? state.branchSelection.filter((id) => id !== action.messageId)
        : [...state.branchSelection, action.messageId];
      return { ...state, branchSelection: selected };
    }

    case "composer/text":
      return { ...state, composerText: action.text };

    case "composer/attachments":
      return { ...state, composerAttachments: [...action.attachments] };

    case "composer/clear":
      // The normal composer only: the edit buffer belongs to the edit mode
      // and is cleared by `edit/clear`.
      return {
        ...state,
        composerText: "",
        composerAttachments: [],
      };

    case "edit/clear":
      return {
        ...state,
        editingLatest: null,
        editText: "",
        editAttachments: [],
      };

    case "draft/revision":
      return { ...state, draftRevision: action.revision };

    case "edit/start":
      return {
        ...state,
        editingLatest: action.messageId,
        editText: action.text,
        editAttachments: [...action.attachments],
      };

    case "edit/text":
      return { ...state, editText: action.text };

    case "edit/attachments":
      return { ...state, editAttachments: [...action.attachments] };

    case "edit/cancel":
      return { ...state, editingLatest: null, editText: "", editAttachments: [] };

    case "modal/set":
      return { ...state, modal: action.modal };

    case "sidebar/toggle":
      return {
        ...state,
        sidebarCollapsed: action.collapsed ?? !state.sidebarCollapsed,
      };

    case "sidebar/width":
      return { ...state, settings: { ...state.settings, sidebar_width: action.width } };

    case "conversation/search":
      return { ...state, conversationSearch: action.query };

    case "model/search":
      return { ...state, modelSearch: action.query };

    case "picker/search":
      return { ...state, pickerSearch: action.query };

    case "picker/toggle":
      return { ...state, pickerOpen: action.open ?? !state.pickerOpen };

    case "attempts/toggle":
      return {
        ...state,
        showAttempts: {
          ...state.showAttempts,
          [action.conversationId]: !state.showAttempts[action.conversationId],
        },
      };

    case "split/toggle":
      return {
        ...state,
        splitView: {
          ...state.splitView,
          [action.conversationId]: !state.splitView[action.conversationId],
        },
      };

    case "answer/selected":
      return {
        ...state,
        activeAnswer: {
          ...state.activeAnswer,
          [action.questionId]: action.messageId,
        },
      };

    case "usage/loaded":
      return {
        ...state,
        usage: { ...state.usage, [action.conversationId]: action.usage },
      };

    case "credentials/loaded":
      return { ...state, credentials: action.credentials, credentialsReloading: false };

    case "credentials/reloading":
      return { ...state, credentialsReloading: action.value };

    case "keyStatus/loaded":
      return { ...state, keyStatus: action.status, keyStatusLoading: false };

    case "keyStatus/loading":
      return { ...state, keyStatusLoading: action.value };

    case "settings/loaded":
      return { ...state, settings: action.settings };

    case "egress/loaded":
      return { ...state, egress: action.egress };

    case "egress/modal":
      return { ...state, egressModal: action.modal };

    case "toast/show":
      return {
        ...state,
        toast: {
          id: state.toastSeq + 1,
          text: action.text,
          kind: action.kind ?? "info",
        },
        toastSeq: state.toastSeq + 1,
      };

    case "toast/hide":
      return { ...state, toast: null };

    default:
      return state;
  }
}

export { initialState };
