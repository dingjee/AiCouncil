import { describe, expect, test } from "bun:test";

import { IpcClient } from "../src/ipc/client";
import { Commands, Events } from "../src/ipc/commands";
import type { Transport } from "../src/ipc/transport";
import type {
  BootstrapState,
  ConversationDetail,
  ConversationSummary,
  Draft,
  Generation,
  Message,
  SendReceipt,
} from "../src/ipc/types";

/**
 * The command names and argument keys are normative
 * (docs/TAURI_IPC.md). These tests fail as soon as the frontend drifts
 * away from the contract.
 */
describe("IPC contract", () => {
  test("command names match the contract", () => {
    expect(Commands.bootstrap).toBe("bootstrap");
    expect(Commands.getSettings).toBe("get_settings");
    expect(Commands.updateSettings).toBe("update_settings");
    expect(Commands.checkEgress).toBe("check_egress");
    expect(Commands.listConversations).toBe("list_conversations");
    expect(Commands.createConversation).toBe("create_conversation");
    expect(Commands.renameConversation).toBe("rename_conversation");
    expect(Commands.deleteConversation).toBe("delete_conversation");
    expect(Commands.setConversationGroup).toBe("set_conversation_group");
    expect(Commands.setSystemPrompt).toBe("set_system_prompt");
    expect(Commands.setMainLeaf).toBe("set_main_leaf");
    expect(Commands.createGroup).toBe("create_group");
    expect(Commands.renameGroup).toBe("rename_group");
    expect(Commands.deleteGroup).toBe("delete_group");
    expect(Commands.loadConversation).toBe("load_conversation");
    expect(Commands.getDraft).toBe("get_draft");
    expect(Commands.saveDraft).toBe("save_draft");
    expect(Commands.saveEditBuffer).toBe("save_edit_buffer");
    expect(Commands.clearEditBuffer).toBe("clear_edit_buffer");
    expect(Commands.refreshCatalog).toBe("refresh_catalog");
    expect(Commands.listPreferences).toBe("list_preferences");
    expect(Commands.setModelPreference).toBe("set_model_preference");
    expect(Commands.setPrimaryModel).toBe("set_primary_model");
    expect(Commands.checkKeyStatus).toBe("check_key_status");
    expect(Commands.sendMessage).toBe("send_message");
    expect(Commands.cancelRequest).toBe("cancel_request");
    expect(Commands.retryRequest).toBe("retry_request");
    expect(Commands.removeMessage).toBe("remove_message");
    expect(Commands.exportMarkdown).toBe("export_markdown");
    expect(Commands.credentialStatus).toBe("credential_status");
    expect(Commands.reloadCredentials).toBe("reload_credentials");
    // Coordinated addition (docs/MMF_STATUS.md, K4C section): the backend
    // writes the exported file through the native save panel.
    expect(Commands.exportMarkdownToFile).toBe("export_markdown_to_file");
  });

  test("event names match the contract", () => {
    expect(Events.chat).toBe("chat-event");
    expect(Events.title).toBe("title-event");
    expect(Events.conversationsChanged).toBe("conversations-changed");
    expect(Events.conversationChanged).toBe("conversation-changed");
    expect(Events.catalogChanged).toBe("catalog-changed");
  });

  test("sends the documented argument keys", async () => {
    const calls: { command: string; args: Record<string, unknown> }[] = [];
    const transport: Transport = {
      async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
        calls.push({ command, args: args ?? {} });
        return undefined as T;
      },
      async listen() {
        return () => undefined;
      },
    };
    const client = new IpcClient(transport);

    await client.renameConversation({
      conversation_id: "c1",
      title: "New",
      expected_title_revision: 3,
    });
    await client.setConversationGroup("c1", "g1");
    await client.setMainLeaf("c1", "m1");
    await client.saveDraft({
      conversation_id: "c1",
      text: "hello",
      attachments: [],
      changed: true,
    });
    await client.cancelRequest("r1");
    await client.retryRequest("r1");
    await client.removeMessage("m1");
    await client.exportMarkdown({ conversation_id: "c1", scope: "all_branches" });
    await client.setModelPreference({
      provider_id: "p1",
      model_id: "m1",
      preference: { enabled: true, sort_order: 0, is_default: false, overrides: null },
    });

    expect(calls[0]).toEqual({
      command: "rename_conversation",
      args: {
        conversation_id: "c1",
        title: "New",
        expected_title_revision: 3,
      },
    });
    expect(calls[1].args).toEqual({ conversation_id: "c1", group_id: "g1" });
    expect(calls[2].args).toEqual({ conversation_id: "c1", message_id: "m1" });
    expect(calls[3].args).toEqual({
      conversation_id: "c1",
      text: "hello",
      attachments: [],
      changed: true,
    });
    expect(calls[4].args).toEqual({ request_id: "r1" });
    expect(calls[5].args).toEqual({ request_id: "r1" });
    expect(calls[6].args).toEqual({ message_id: "m1" });
    expect(calls[7].args).toEqual({
      conversation_id: "c1",
      scope: "all_branches",
    });
    expect(calls[8].args).toEqual({
      provider_id: "p1",
      model_id: "m1",
      preference: { enabled: true, sort_order: 0, is_default: false, overrides: null },
    });
  });

  test("the send request uses the documented shape", async () => {
    const calls: Record<string, unknown>[] = [];
    const transport: Transport = {
      async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
        if (command === "send_message") calls.push(args ?? {});
        return undefined as T;
      },
      async listen() {
        return () => undefined;
      },
    };
    const client = new IpcClient(transport);
    await client.sendMessage({
      conversation_id: "c1",
      mode: "normal",
      prompt: "hello",
      attachments: [],
      expected_draft_revision: 7,
      selection: {
        kind: "same_question_multi",
        leaf: "m1",
        targets: [{ provider_id: "p1", model_id: "a" }],
        primary: 0,
      },
    });
    // The command takes one struct parameter named `request` → nested.
    expect(calls[0]).toEqual({
      request: {
        conversation_id: "c1",
        mode: "normal",
        prompt: "hello",
        attachments: [],
        expected_draft_revision: 7,
        selection: {
          kind: "same_question_multi",
          leaf: "m1",
          targets: [{ provider_id: "p1", model_id: "a" }],
          primary: 0,
        },
      },
    });
  });

  test("the credential reload falls back to the status read", async () => {
    const commands: string[] = [];
    const transport: Transport = {
      async invoke<T>(command: string): Promise<T> {
        commands.push(command);
        if (command === "reload_credentials") {
          throw { code: "not_found", message: "unknown command" };
        }
        return { openrouter: true, deepseek: false } as T;
      },
      async listen() {
        return () => undefined;
      },
    };
    const client = new IpcClient(transport);
    const status = await client.reloadCredentials();
    expect(status).toEqual({ openrouter: true, deepseek: false });
    expect(commands).toEqual(["reload_credentials", "credential_status"]);
  });
});

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

export function fixtureConversation(
  id: string,
  title: string,
  mainLeafId: string | null = null,
): ConversationSummary {
  return {
    id,
    title,
    group_id: null,
    system_prompt: null,
    title_source: "manual",
    title_status: "ready",
    title_revision: 1,
    main_leaf_id: mainLeafId,
    draft_revision: 0,
    created_at_ms: 0,
    updated_at_ms: 0,
  };
}

export function fixturePreferences() {
  return [
    {
      provider_id: "p1",
      model_id: "vendor/alpha",
      enabled: true,
      sort_order: 0,
      is_default: true,
      overrides: null,
    },
  ];
}

export function fixtureBootstrap(
  conversations: ConversationSummary[],
): BootstrapState {
  return {
    app_version: "0.2.1",
    settings: {
      max_concurrent_requests: 4,
      assumed_output_tokens: 1000,
      connect_timeout_secs: 15,
      read_idle_timeout_secs: 120,
      body_text_size: 15,
      locale: "en",
      theme_id: "soft_sage",
      sidebar_width: 300,
      sidebar_collapsed: false,
    },
    conversations,
    groups: [],
    pending_new_conversation: null,
    catalog: { entries: [], updated_at_ms: null },
    preferences: fixturePreferences(),
    credentials: { openrouter: true, deepseek: false },
    egress: { verdict: "allowed", country_code: "US", detail: "" },
  };
}

export function fixtureDetail(
  conversationId: string,
  messages: Message[],
  title = "Chat",
): ConversationDetail {
  return {
    conversation: fixtureConversation(
      conversationId,
      title,
      messages.length > 0 ? messages[messages.length - 1].id : null,
    ),
    messages,
    generations: [],
    usage: [],
    edit_latest: null,
  };
}

export function fixtureDraft(text: string): Draft {
  return {
    text,
    attachments: [],
    edit_text: "",
    edit_attachments: [],
    edit_target_id: null,
    revision: 1,
  };
}

export function fixtureMessage(
  id: string,
  conversationId: string,
  content: string,
): Message {
  return {
    id,
    conversation_id: conversationId,
    parent_id: null,
    role: "assistant",
    content,
    attachments: [],
    created_at_ms: 0,
  };
}

export function fixtureGeneration(
  requestId: string,
  assistantId: string,
): Generation {
  return {
    request_id: requestId,
    batch_id: "b1",
    user_message_id: "u1",
    assistant_message_id: assistantId,
    provider_id: "p1",
    model_id: "vendor/alpha",
    status: "completed",
    system_prompt: null,
    price_prompt: null,
    price_completion: null,
    remote_generation_id: null,
    error: null,
    retry_of: null,
    retry_root_id: null,
    retry_not_before_ms: null,
    dispatch_phase: "remote_accepted",
    failure_kind: null,
    failure_message: null,
    started_at_ms: 0,
    finished_at_ms: 0,
  };
}

export function fixtureReceipt(): SendReceipt {
  return {
    conversation_id: "c1",
    batch_id: "b1",
    primary_request_id: "r1",
    assistant_of: { r1: "a1" },
    adopted_main_leaf: "a1",
  };
}
