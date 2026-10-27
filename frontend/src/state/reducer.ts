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
import { initialState, type AppState, type EgressModalState, type Panel, type ToastKind } from "./types";

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
  | { type: "model/enabled"; modelId: string; enabled: boolean }
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
  | { type: "draft/revision"; revision: number }
  | { type: "edit/start"; messageId: Id; text: string; attachments: Message["attachments"] }
  | { type: "edit/text"; text: string }
  | { type: "edit/attachments"; attachments: Message["attachments"] }
  | { type: "edit/cancel" }
  | { type: "panel/set"; panel: Panel }
  | { type: "sidebar/toggle"; collapsed?: boolean }
  | { type: "sidebar/width"; width: number }
  | { type: "conversation/search"; query: string }
  | { type: "model/search"; query: string }
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
        credentials: action.credentials,
        settings: action.settings,
        sidebarCollapsed: action.settings.sidebar_collapsed,
        egress: action.egress,
      };

    case "conversations/loaded":
      return { ...state, conversations: action.conversations };

    case "groups/loaded":
      return { ...state, groups: action.groups };

    case "conversation/loaded":
      return {
        ...state,
        conversations: upsertById(state.conversations, action.conversation),
        messages: {
          ...state.messages,
          [action.conversationId]: action.messages,
        },
        generations: {
          ...state.generations,
          [action.conversationId]: action.generations,
        },
        usage: { ...state.usage, [action.conversationId]: action.usage },
        draftRevision: action.draftRevision,
        editingLatest: action.editTargetId,
        editText: action.editText,
        editAttachments: action.editAttachments,
        editLatestEligible: action.editLatestEligible,
      };

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
      return { ...state, preferences: action.preferences };

    case "model/enabled":
      return {
        ...state,
        preferences: state.preferences.map((preference) =>
          preference.model_id === action.modelId
            ? { ...preference, enabled: action.enabled }
            : preference,
        ),
      };

    case "model/primary":
      return {
        ...state,
        preferences: state.preferences.map((preference) => ({
          ...preference,
          is_default: preference.model_id === action.modelId,
        })),
        modelSelection: {
          ...state.modelSelection,
          primary: action.modelId,
          checked: state.modelSelection.checked.includes(action.modelId)
            ? state.modelSelection.checked
            : [...state.modelSelection.checked, action.modelId],
        },
      };

    case "model/overrides":
      return {
        ...state,
        preferences: state.preferences.map((preference) =>
          preference.model_id === action.modelId
            ? { ...preference, overrides: action.overrides }
            : preference,
        ),
      };

    case "model/parallel":
      return { ...state, modelSelection: { ...state.modelSelection, parallel: action.value } };

    case "model/checked": {
      const checked = action.checked
        ? state.modelSelection.checked.includes(action.modelId)
          ? state.modelSelection.checked
          : [...state.modelSelection.checked, action.modelId]
        : state.modelSelection.checked.filter((id) => id !== action.modelId);
      return { ...state, modelSelection: { ...state.modelSelection, checked } };
    }

    case "model/checkedSet":
      return {
        ...state,
        modelSelection: { ...state.modelSelection, checked: [...action.modelIds] },
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
      return {
        ...state,
        composerText: "",
        composerAttachments: [],
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

    case "panel/set":
      return { ...state, panel: action.panel };

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
