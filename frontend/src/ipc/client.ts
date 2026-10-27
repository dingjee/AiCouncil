/**
 * Typed client over the IPC transport.
 *
 * The client owns *only* the wire mapping; state handling lives in
 * `src/state`. Command failures are normalised to `Error` with the backend
 * `code` preserved so the UI can branch on `admission.*` reasons.
 */

import {
  Commands,
  Events,
  type BootstrapResult,
  type CatalogResult,
  type ConversationDetailResult,
  type ConversationSummaryResult,
  type CredentialStatusResult,
  type DraftResult,
  type DraftRevisionResult,
  type EgressResult,
  type ExportArgs,
  type ExportResultPayload,
  type GroupResult,
  type KeyStatusResult,
  type PreferencesResult,
  type RenameConversationArgs,
  type SaveDraftArgs,
  type SaveEditBufferArgs,
  type SendMessageRequest,
  type SendReceiptResult,
  type SetModelPreferenceArgs,
  type SettingsResult,
} from "./commands";
import { IpcCommandError, toError, type Transport } from "./transport";
import type {
  CatalogState,
  ChatEvent,
  ConversationChangedEvent,
  ConversationsChangedEvent,
  Id,
  Settings,
  TitleEvent,
} from "./types";

/** Error carrying the backend `code` (e.g. `admission.cooldown`). */
export { IpcCommandError as BackendError };

/**
 * Normalise the catalogue payload.
 *
 * The backend returns `{ updated_at_ms, entries }`, but the frontend must
 * survive a shape difference instead of rendering a blank window: a missing
 * or unexpected catalogue degrades to an empty list.
 */
function normalizeCatalog(value: unknown): CatalogState {
  if (Array.isArray(value)) {
    return { entries: value as CatalogState["entries"], updated_at_ms: null };
  }
  if (value && typeof value === "object") {
    const record = value as Record<string, unknown>;
    const entries = Array.isArray(record.entries)
      ? (record.entries as CatalogState["entries"])
      : Array.isArray(record.models)
        ? (record.models as CatalogState["entries"])
        : [];
    const updated = record.updated_at_ms ?? record.updatedAtMs ?? null;
    return {
      entries,
      updated_at_ms: typeof updated === "number" ? updated : null,
    };
  }
  return { entries: [], updated_at_ms: null };
}

/** Normalise the bootstrap payload (catalogue + optional fields). */
function normalizeBootstrap(value: unknown): BootstrapResult {
  if (!value || typeof value !== "object") {
    throw new Error("bootstrap returned an unexpected payload");
  }
  const payload = value as BootstrapResult;
  return {
    ...payload,
    conversations: Array.isArray(payload.conversations) ? payload.conversations : [],
    groups: Array.isArray(payload.groups) ? payload.groups : [],
    preferences: Array.isArray(payload.preferences) ? payload.preferences : [],
    catalog: normalizeCatalog(payload.catalog),
    credentials:
      payload.credentials && typeof payload.credentials === "object"
        ? payload.credentials
        : {},
  };
}

export class IpcClient {
  constructor(private readonly transport: Transport) {}

  private async call<T>(
    command: string,
    args?: Record<string, unknown>,
  ): Promise<T> {
    try {
      return await this.transport.invoke<T>(command, args);
    } catch (error) {
      throw toError(error);
    }
  }

  // ----- bootstrap / settings -----

  bootstrap(): Promise<BootstrapResult> {
    return this.call(Commands.bootstrap).then(normalizeBootstrap);
  }

  getSettings(): Promise<SettingsResult> {
    return this.call(Commands.getSettings);
  }

  /** Update the settings; the patch is nested under the `patch` parameter. */
  updateSettings(patch: Partial<Settings>): Promise<SettingsResult> {
    return this.call(Commands.updateSettings, { patch });
  }

  checkEgress(): Promise<EgressResult> {
    return this.call(Commands.checkEgress);
  }

  // ----- conversations -----

  listConversations(): Promise<ConversationSummaryResult[]> {
    return this.call(Commands.listConversations);
  }

  createConversation(): Promise<ConversationSummaryResult> {
    return this.call(Commands.createConversation);
  }

  renameConversation(args: RenameConversationArgs): Promise<ConversationSummaryResult> {
    return this.call(Commands.renameConversation, { ...args });
  }

  deleteConversation(conversationId: Id): Promise<null> {
    return this.call(Commands.deleteConversation, {
      conversation_id: conversationId,
    });
  }

  setConversationGroup(conversationId: Id, groupId: Id | null): Promise<null> {
    return this.call(Commands.setConversationGroup, {
      conversation_id: conversationId,
      group_id: groupId,
    });
  }

  setSystemPrompt(conversationId: Id, systemPrompt: string): Promise<null> {
    return this.call(Commands.setSystemPrompt, {
      conversation_id: conversationId,
      system_prompt: systemPrompt,
    });
  }

  setMainLeaf(conversationId: Id, messageId: Id): Promise<null> {
    return this.call(Commands.setMainLeaf, {
      conversation_id: conversationId,
      message_id: messageId,
    });
  }

  // ----- groups -----

  createGroup(name: string): Promise<GroupResult> {
    return this.call(Commands.createGroup, { name });
  }

  renameGroup(groupId: Id, name: string): Promise<null> {
    return this.call(Commands.renameGroup, { group_id: groupId, name });
  }

  deleteGroup(groupId: Id): Promise<null> {
    return this.call(Commands.deleteGroup, { group_id: groupId });
  }

  // ----- conversation detail -----

  loadConversation(conversationId: Id): Promise<ConversationDetailResult> {
    return this.call(Commands.loadConversation, {
      conversation_id: conversationId,
    });
  }

  // ----- drafts -----

  getDraft(conversationId: Id): Promise<DraftResult> {
    return this.call(Commands.getDraft, { conversation_id: conversationId });
  }

  saveDraft(args: SaveDraftArgs): Promise<DraftRevisionResult> {
    return this.call(Commands.saveDraft, { ...args });
  }

  saveEditBuffer(args: SaveEditBufferArgs): Promise<DraftRevisionResult> {
    return this.call(Commands.saveEditBuffer, { ...args });
  }

  clearEditBuffer(conversationId: Id): Promise<DraftRevisionResult> {
    return this.call(Commands.clearEditBuffer, {
      conversation_id: conversationId,
    });
  }

  // ----- catalog / preferences -----

  refreshCatalog(): Promise<CatalogResult> {
    return this.call(Commands.refreshCatalog).then(normalizeCatalog);
  }

  listPreferences(): Promise<PreferencesResult> {
    return this.call(Commands.listPreferences);
  }

  setModelPreference(args: SetModelPreferenceArgs): Promise<null> {
    return this.call(Commands.setModelPreference, { ...args });
  }

  setPrimaryModel(providerId: Id, modelId: string): Promise<null> {
    return this.call(Commands.setPrimaryModel, {
      provider_id: providerId,
      model_id: modelId,
    });
  }

  checkKeyStatus(): Promise<KeyStatusResult> {
    return this.call(Commands.checkKeyStatus);
  }

  // ----- send / cancel / retry / remove -----

  /**
   * Send a message.
   *
   * The backend command takes one struct parameter named `request`, so the
   * payload is **nested** under that name (Tauri resolves command arguments
   * by parameter name).
   */
  sendMessage(request: SendMessageRequest): Promise<SendReceiptResult> {
    return this.call(Commands.sendMessage, { request });
  }

  cancelRequest(requestId: Id): Promise<null> {
    return this.call(Commands.cancelRequest, { request_id: requestId });
  }

  retryRequest(requestId: Id): Promise<SendReceiptResult> {
    return this.call(Commands.retryRequest, { request_id: requestId });
  }

  removeMessage(messageId: Id): Promise<null> {
    return this.call(Commands.removeMessage, { message_id: messageId });
  }

  // ----- export -----

  exportMarkdown(args: ExportArgs): Promise<ExportResultPayload> {
    return this.call(Commands.exportMarkdown, { ...args });
  }

  /**
   * Export straight to a file: the backend opens the native save panel and
   * writes the file atomically, and returns the written path.
   */
  exportMarkdownToFile(args: ExportArgs): Promise<{
    path: string;
    sections: number;
  }> {
    return this.call(Commands.exportMarkdownToFile, { ...args });
  }

  // ----- credentials (booleans only) -----

  credentialStatus(): Promise<CredentialStatusResult> {
    return this.call(Commands.credentialStatus);
  }

  /**
   * Reload the provider keys from the local `.env`.
   *
   * The backend re-reads the file and answers with the refreshed booleans.
   * Older backends without the reload command fall back to a plain status
   * read, so the button never turns into a dead control.
   */
  async reloadCredentials(): Promise<CredentialStatusResult> {
    try {
      return await this.call(Commands.reloadCredentials);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      const code =
        error && typeof error === "object" && "code" in error
          ? String((error as { code?: unknown }).code ?? "")
          : "";
      if (code === "not_found" || /unknown command/i.test(message)) {
        return this.credentialStatus();
      }
      throw error;
    }
  }

  // ----- events -----

  onChatEvent(handler: (payload: ChatEvent) => void): Promise<() => void> {
    return this.transport.listen(Events.chat, handler);
  }

  onTitleEvent(handler: (payload: TitleEvent) => void): Promise<() => void> {
    return this.transport.listen(Events.title, handler);
  }

  onConversationsChanged(
    handler: (payload: ConversationsChangedEvent) => void,
  ): Promise<() => void> {
    return this.transport.listen(Events.conversationsChanged, handler);
  }

  onConversationChanged(
    handler: (payload: ConversationChangedEvent) => void,
  ): Promise<() => void> {
    return this.transport.listen(Events.conversationChanged, handler);
  }

  onCatalogChanged(handler: (payload: CatalogState) => void): Promise<() => void> {
    return this.transport.listen(Events.catalogChanged, (payload) =>
      handler(normalizeCatalog(payload)),
    );
  }
}
