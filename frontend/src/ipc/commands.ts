/**
 * Canonical command names and argument builders.
 *
 * Every name here is normative and taken from `docs/TAURI_IPC.md` (v1):
 * snake_case command names, snake_case argument keys, DTO-shaped payloads.
 */

import type {
  BranchTarget,
  CatalogState,
  ConversationDetail,
  ConversationSummary,
  Draft,
  DraftRevision,
  ExportResult,
  ExportScope,
  Group,
  Id,
  KeyStatus,
  ModelPreferenceEntry,
  ModelPreferencePatch,
  ModelTarget,
  SendMessageRequest,
  SendReceipt,
  Settings,
} from "./types";

export const Commands = {
  bootstrap: "bootstrap",
  getSettings: "get_settings",
  updateSettings: "update_settings",
  checkEgress: "check_egress",

  listConversations: "list_conversations",
  createConversation: "create_conversation",
  renameConversation: "rename_conversation",
  deleteConversation: "delete_conversation",
  setConversationGroup: "set_conversation_group",
  setSystemPrompt: "set_system_prompt",
  setMainLeaf: "set_main_leaf",

  createGroup: "create_group",
  renameGroup: "rename_group",
  deleteGroup: "delete_group",

  loadConversation: "load_conversation",

  getDraft: "get_draft",
  saveDraft: "save_draft",
  saveEditBuffer: "save_edit_buffer",
  clearEditBuffer: "clear_edit_buffer",

  refreshCatalog: "refresh_catalog",
  listPreferences: "list_preferences",
  setModelPreference: "set_model_preference",
  setPrimaryModel: "set_primary_model",
  checkKeyStatus: "check_key_status",

  sendMessage: "send_message",
  cancelRequest: "cancel_request",
  retryRequest: "retry_request",
  removeMessage: "remove_message",

  exportMarkdown: "export_markdown",
  /**
   * Export straight to a file: opens the native macOS save panel and writes
   * the file atomically. The frontend falls back to `export_markdown` (content
   * + browser download) when the backend does not implement it yet.
   */
  exportMarkdownToFile: "export_markdown_to_file",

  credentialStatus: "credential_status",
  /**
   * Re-read the provider keys from the local `.env` (the sole source) and
   * return the refreshed configured booleans.
   */
  reloadCredentials: "reload_credentials",
} as const;

export type CommandName = (typeof Commands)[keyof typeof Commands];

/** Event names (Tauri event API). */
export const Events = {
  chat: "chat-event",
  title: "title-event",
  conversationsChanged: "conversations-changed",
  conversationChanged: "conversation-changed",
  /** Emitted after a catalog refresh; payload is the CatalogState. */
  catalogChanged: "catalog-changed",
} as const;

export type EventName = (typeof Events)[keyof typeof Events];

export interface RenameConversationArgs {
  conversation_id: Id;
  title: string;
  expected_title_revision: number;
}

export interface SaveDraftArgs {
  conversation_id: Id;
  text: string;
  attachments: Draft["attachments"];
  changed: boolean;
}

export interface SaveEditBufferArgs {
  conversation_id: Id;
  text: string;
  attachments: Draft["edit_attachments"];
  target_id: Id | null;
  changed: boolean;
}

export interface SetModelPreferenceArgs {
  provider_id: Id;
  model_id: string;
  preference: ModelPreferencePatch;
}

export interface ExportArgs {
  conversation_id: Id;
  scope: ExportScope;
  message_ids?: Id[];
}

export type BootstrapResult = import("./types").BootstrapState;
export type ConversationDetailResult = ConversationDetail;
export type ConversationSummaryResult = ConversationSummary;
export type GroupResult = Group;
export type CatalogResult = CatalogState;
export type PreferencesResult = ModelPreferenceEntry[];
export type DraftResult = Draft;
export type DraftRevisionResult = DraftRevision;
export type SettingsResult = Settings;
export type SendReceiptResult = SendReceipt;
export type KeyStatusResult = KeyStatus;
export type ExportResultPayload = ExportResult;
export type CredentialStatusResult = Record<string, boolean>;
export type EgressResult = import("./types").EgressState;

export type { BranchTarget, ModelTarget, SendMessageRequest };
