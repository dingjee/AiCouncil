/**
 * Browser mock backend (development harness only).
 *
 * This file is **not** part of the production bundle: `vite.config.ts` uses
 * `dev/index.html` as the build input only in `--mode mock`, and the
 * production entry (`index.html` → `src/main.tsx`) never imports it. It
 * exists so the UI can be exercised in a browser without the native shell.
 *
 * The command names, argument keys and DTO shapes follow
 * `docs/TAURI_IPC.md` (v1) exactly, so the mock is a faithful stand-in for
 * the Rust backend.
 *
 * The data below is fabricated fixture data; no real user data, no keys.
 */

import type {
  Attachment,
  BootstrapState,
  CatalogEntry,
  ConversationDetail,
  ConversationSummary,
  Draft,
  EgressState,
  Generation,
  Group,
  Id,
  Message,
  ModelPreferenceEntry,
  SendMessageRequest,
  SendReceipt,
  Settings,
  UsageRecord,
} from "../src/ipc/types";
import type { Transport } from "../src/ipc/transport";
import { longFixture, type FixtureName } from "./fixtures";

const now = Date.now();

function uuid(seed: string): Id {
  let hash = 0;
  for (let i = 0; i < seed.length; i += 1) {
    hash = (hash * 31 + seed.charCodeAt(i)) >>> 0;
  }
  const hex = hash.toString(16).padStart(8, "0");
  return `${hex}-0000-4000-8000-000000000000`;
}

const PROVIDER_ID = uuid("provider-openrouter");

/** True when a generation status no longer changes (native parity). */
function isTerminal(status: string): boolean {
  return ["completed", "failed", "cancelled", "interrupted"].includes(status);
}

const CATALOG: CatalogEntry[] = [
  {
    id: "anthropic/claude-sonnet-4.5",
    provider_id: PROVIDER_ID,
    name: "Anthropic: Claude Sonnet 4.5",
    family: "claude",
    context_length: 200000,
    prompt_price: "3.00",
    completion_price: "15.00",
    supports_streaming: true,
    supports_temperature: true,
  },
  {
    id: "anthropic/claude-opus-4.1",
    provider_id: PROVIDER_ID,
    name: "Anthropic: Claude Opus 4.1",
    family: "claude",
    context_length: 200000,
    prompt_price: "15.00",
    completion_price: "75.00",
    supports_streaming: true,
    supports_temperature: true,
  },
  {
    id: "openai/gpt-4.1-mini",
    provider_id: PROVIDER_ID,
    name: "OpenAI: GPT-4.1 mini",
    family: "gpt",
    context_length: 1000000,
    prompt_price: "0.40",
    completion_price: "1.60",
    supports_streaming: true,
    supports_temperature: true,
  },
  {
    id: "openai/o3-mini",
    provider_id: PROVIDER_ID,
    name: "OpenAI: o3-mini",
    family: "o",
    context_length: 200000,
    prompt_price: "1.10",
    completion_price: "4.40",
    supports_streaming: true,
    supports_temperature: false,
  },
  {
    id: "google/gemini-2.5-flash",
    provider_id: PROVIDER_ID,
    name: "Google: Gemini 2.5 Flash",
    family: "gemini",
    context_length: 1000000,
    prompt_price: "0.30",
    completion_price: "2.50",
    supports_streaming: true,
    supports_temperature: true,
  },
  {
    id: "deepseek/deepseek-chat",
    provider_id: PROVIDER_ID,
    name: "DeepSeek: DeepSeek V3",
    family: "deepseek",
    context_length: 64000,
    prompt_price: "0.27",
    completion_price: "1.10",
    supports_streaming: true,
    supports_temperature: true,
  },
  {
    id: "qwen/qwen3-max",
    provider_id: PROVIDER_ID,
    name: "Alibaba: Qwen3 Max",
    family: "qwen",
    context_length: 262144,
    prompt_price: "1.20",
    completion_price: "6.00",
    supports_streaming: true,
    supports_temperature: true,
  },
];

const PREFERENCES: ModelPreferenceEntry[] = [
  {
    provider_id: PROVIDER_ID,
    model_id: "anthropic/claude-sonnet-4.5",
    enabled: true,
    sort_order: 0,
    is_default: true,
    overrides: null,
  },
  {
    provider_id: PROVIDER_ID,
    model_id: "openai/gpt-4.1-mini",
    enabled: true,
    sort_order: 1,
    is_default: false,
    overrides: null,
  },
  {
    provider_id: PROVIDER_ID,
    model_id: "google/gemini-2.5-flash",
    enabled: false,
    sort_order: 2,
    is_default: false,
    overrides: null,
  },
];

const GROUPS: Group[] = [
  { id: uuid("group-1"), name: "Research", sort_order: 0 },
  { id: uuid("group-2"), name: "Code review", sort_order: 1 },
];

const SAMPLE_ANSWER = `Here is a short answer about **Soft Sage**.

## Details

1. The palette keeps a warm canvas (#F4F3EE).
2. Accent is a muted green (#496B55).

\`\`\`ts
export const accent = "#496b55";
\`\`\`

An inline formula $E = mc^2$ and a display formula:

$$
\\int_0^1 x^2 \\, dx = \\frac{1}{3}
$$

> A block quote keeps the muted colour.
`;

function seedConversations(): ConversationSummary[] {
  return [
    {
      id: uuid("conv-1"),
      title: "Welcome to AiCouncil",
      group_id: null,
      system_prompt: null,
      title_source: "manual",
      title_status: "ready",
      title_revision: 1,
      title_attempt_count: 0,
      main_leaf_id: uuid("msg-a1"),
      draft_revision: 0,
      created_at_ms: now - 7200_000,
      updated_at_ms: now - 600_000,
    },
    {
      id: uuid("conv-2"),
      title: "Pricing question",
      group_id: GROUPS[0].id,
      system_prompt: null,
      title_source: "generated",
      title_status: "ready",
      title_revision: 1,
      title_attempt_count: 0,
      main_leaf_id: null,
      draft_revision: 3,
      created_at_ms: now - 3600_000,
      updated_at_ms: now - 3600_000,
    },
  ];
}

/**
 * The preferences survive a reload of the page.
 *
 * The real backend keeps them in its database; the mock keeps them in the
 * session storage of the browser session, so a reload of the harness page
 * behaves like a restart of the app. Only model ids and their parameters are
 * stored — never a key, never a message.
 */
const PREFERENCES_KEY = "aicouncil-dev-preferences";

function loadPreferences(): ModelPreferenceEntry[] | null {
  try {
    const raw = window.sessionStorage.getItem(PREFERENCES_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as ModelPreferenceEntry[];
    return Array.isArray(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

function savePreferences(preferences: ModelPreferenceEntry[]): void {
  try {
    window.sessionStorage.setItem(PREFERENCES_KEY, JSON.stringify(preferences));
  } catch {
    // A browser without a session storage is fine: the mock simply forgets.
  }
}

export class MockBackend {
  private conversations = seedConversations();
  private groups = GROUPS;
  private preferences = PREFERENCES;
  private settings: Settings = {
    max_concurrent_requests: 4,
    assumed_output_tokens: 1000,
    connect_timeout_secs: 15,
    read_idle_timeout_secs: 120,
    body_text_size: 15,
    locale: "en",
    theme_id: "soft_sage",
    sidebar_width: 300,
    sidebar_collapsed: false,
  };
  private credentials: Record<string, boolean> = {
    openrouter: true,
    deepseek: false,
  };
  private egress: EgressState = {
    verdict: "allowed",
    country_code: "US",
    detail: "mock fixture",
  };
  private messages = new Map<Id, Message[]>();
  private generations = new Map<Id, Generation[]>();
  private usage = new Map<Id, UsageRecord[]>();
  private drafts = new Map<Id, Draft>();
  private editBuffers = new Map<Id, { target_id: Id | null; text: string; attachments: Attachment[] }>();
  private listeners = new Map<string, Set<(payload: unknown) => void>>();
  private cancelled = new Set<Id>();

  constructor(fixture: FixtureName = "default") {
    if (fixture === "long") {
      const data = longFixture();
      this.conversations = data.conversations;
      this.messages = data.messages;
      this.generations = data.generations;
      this.usage = data.usage;
      for (const conversation of this.conversations) {
        this.drafts.set(conversation.id, {
          text: "",
          attachments: [],
          edit_text: "",
          edit_attachments: [],
          edit_target_id: null,
          revision: 0,
        });
      }
      // A reload of the page keeps the preferences of the session.
      const stored = loadPreferences();
      if (stored) this.preferences = stored;
      return;
    }
    const first = this.conversations[0];
    this.messages.set(first.id, [
      {
        id: uuid("msg-q1"),
        conversation_id: first.id,
        parent_id: null,
        role: "user",
        content: "What is the Soft Sage palette?",
        attachments: [],
        created_at_ms: now - 7200_000,
      },
      {
        id: uuid("msg-a1"),
        conversation_id: first.id,
        parent_id: uuid("msg-q1"),
        role: "assistant",
        content: SAMPLE_ANSWER,
        attachments: [],
        created_at_ms: now - 7100_000,
      },
    ]);
    this.generations.set(first.id, [
      {
        request_id: uuid("gen-1"),
        batch_id: uuid("batch-1"),
        user_message_id: uuid("msg-q1"),
        assistant_message_id: uuid("msg-a1"),
        provider_id: PROVIDER_ID,
        model_id: "anthropic/claude-sonnet-4.5",
        status: "completed",
        system_prompt: null,
        price_prompt: "3.00",
        price_completion: "15.00",
        remote_generation_id: null,
        error: null,
        retry_of: null,
        retry_root_id: null,
        retry_not_before_ms: null,
        dispatch_phase: "remote_accepted",
        failure_kind: null,
        failure_message: null,
        started_at_ms: now - 7200_000,
        finished_at_ms: now - 7100_000,
      },
    ]);
    this.usage.set(first.id, [
      {
        request_id: uuid("gen-1"),
        prompt_tokens: 1200,
        completion_tokens: 480,
        total_tokens: 1680,
        cost_text: "0.0108",
        cost_source: "api",
        status: "known",
      },
      {
        request_id: uuid("gen-2"),
        prompt_tokens: null,
        completion_tokens: null,
        total_tokens: null,
        cost_text: null,
        cost_source: null,
        status: "unavailable",
      },
    ]);
    this.drafts.set(first.id, {
      text: "",
      attachments: [],
      edit_text: "",
      edit_attachments: [],
      edit_target_id: null,
      revision: 0,
    });
    this.drafts.set(this.conversations[1].id, {
      text: "Draft that survives a restart",
      attachments: [],
      edit_text: "",
      edit_attachments: [],
      edit_target_id: null,
      revision: 3,
    });
    const stored = loadPreferences();
    if (stored) this.preferences = stored;
  }

  private emit(event: string, payload: unknown): void {
    this.listeners.get(event)?.forEach((handler) => handler(payload));
  }

  /** Write a known cost into the ledger of a conversation. */
  private recordUsage(
    conversationId: Id,
    requestId: Id,
    promptTokens: number,
    completionTokens: number,
    cost: string,
  ): void {
    const records = this.usage.get(conversationId) ?? [];
    const record: UsageRecord = {
      request_id: requestId,
      prompt_tokens: promptTokens,
      completion_tokens: completionTokens,
      total_tokens: promptTokens + completionTokens,
      cost_text: cost,
      cost_source: "api",
      status: "known",
    };
    const index = records.findIndex((entry) => entry.request_id === requestId);
    if (index < 0) records.push(record);
    else records[index] = record;
    this.usage.set(conversationId, records);
  }

  private bootstrapState(): BootstrapState {
    return {
      app_version: "0.2.1",
      settings: this.settings,
      conversations: this.conversations,
      groups: this.groups,
      pending_new_conversation: null,
      catalog: { entries: CATALOG, updated_at_ms: now },
      preferences: this.preferences,
      credentials: this.credentials,
      egress: this.egress,
    };
  }

  private detail(conversationId: Id): ConversationDetail {
    const conversation = this.conversations.find((entry) => entry.id === conversationId);
    if (!conversation) throw new Error("conversation not found");
    const editBuffer = this.editBuffers.get(conversationId);
    const messages = this.messages.get(conversationId) ?? [];
    const questions = messages.filter((message) => message.role === "user");
    const editLatest =
      editBuffer && editBuffer.target_id
        ? {
            target_id: editBuffer.target_id,
            content: editBuffer.text,
            attachments: editBuffer.attachments,
          }
        : null;
    return {
      conversation,
      messages,
      generations: this.generations.get(conversationId) ?? [],
      usage: this.usage.get(conversationId) ?? [],
      edit_latest: editLatest ?? (questions.length ? null : null),
    };
  }

  private streamAnswer(
    conversationId: Id,
    assistantId: Id,
    requestId: Id,
    batchId: Id,
  ): void {
    const words = SAMPLE_ANSWER.split(/(\s+)/);
    let index = 0;
    const emit = (event: unknown) =>
      this.emit("chat-event", {
        conversation_id: conversationId,
        batch_id: batchId,
        request_id: requestId,
        assistant_message_id: assistantId,
        event,
      });

    const tick = () => {
      if (this.cancelled.has(requestId)) {
        this.cancelled.delete(requestId);
        emit({ type: "cancelled" });
        this.emit("conversation-changed", { conversation_id: conversationId });
        return;
      }
      if (index >= words.length) {
        emit({
          type: "finished",
          remote_generation_id: null,
          finish_state: "stop",
        });
        // The ledger of the conversation: the cost becomes known.
        this.recordUsage(conversationId, requestId, 120, 260, "0.0045");
        emit({
          type: "usage_updated",
          prompt_tokens: 120,
          completion_tokens: 260,
          cost: "0.0045",
        });
        this.emit("conversation-changed", { conversation_id: conversationId });
        return;
      }
      const delta = words[index];
      index += 1;
      emit({ type: "text_delta", delta });
      setTimeout(tick, 22);
    };
    emit({ type: "started" });
    setTimeout(tick, 120);
  }

  async invoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
    switch (command) {
      case "bootstrap":
        return this.bootstrapState() as T;

      case "get_settings":
        return this.settings as T;

      case "update_settings": {
        this.settings = { ...this.settings, ...(args as Partial<Settings>) };
        return this.settings as T;
      }

      case "check_egress":
        return this.egress as T;

      case "list_conversations":
        return this.conversations.slice().sort(
          (a, b) => b.updated_at_ms - a.updated_at_ms,
        ) as T;

      case "create_conversation": {
        const id = uuid(`conv-${this.conversations.length + 1}`);
        const conversation: ConversationSummary = {
          id,
          title: "New chat",
          group_id: null,
          system_prompt: null,
          title_source: "empty",
          title_status: "needs_generation",
          title_revision: 0,
          title_attempt_count: 0,
          main_leaf_id: null,
          draft_revision: 0,
          created_at_ms: Date.now(),
          updated_at_ms: Date.now(),
        };
        this.conversations = [conversation, ...this.conversations];
        this.messages.set(id, []);
        this.generations.set(id, []);
        this.usage.set(id, []);
        this.drafts.set(id, {
          text: "",
          attachments: [],
          edit_text: "",
          edit_attachments: [],
          edit_target_id: null,
          revision: 0,
        });
        this.emit("conversations-changed", {});
        return conversation as T;
      }

      case "rename_conversation": {
        const conversation = this.conversations.find(
          (entry) => entry.id === (args.conversation_id as Id),
        );
        if (!conversation) throw new Error("conversation not found");
        const expected = Number(args.expected_title_revision ?? 0);
        if (expected !== conversation.title_revision) {
          const error = new Error("title_revision_conflict") as Error & {
            code?: string;
            detail?: string;
          };
          error.code = "invalid_input";
          error.detail = "title_revision_conflict";
          throw error;
        }
        conversation.title = String(args.title ?? "");
        conversation.title_source = "manual";
        conversation.title_status = "ready";
        conversation.title_revision += 1;
        conversation.updated_at_ms = Date.now();
        this.emit("conversations-changed", {});
        return conversation as T;
      }

      case "delete_conversation": {
        this.conversations = this.conversations.filter(
          (entry) => entry.id !== (args.conversation_id as Id),
        );
        this.emit("conversations-changed", {});
        return null as T;
      }

      case "set_conversation_group": {
        const conversation = this.conversations.find(
          (entry) => entry.id === (args.conversation_id as Id),
        );
        if (!conversation) throw new Error("conversation not found");
        conversation.group_id = (args.group_id as Id | null) ?? null;
        conversation.updated_at_ms = Date.now();
        this.emit("conversations-changed", {});
        return null as T;
      }

      case "set_system_prompt": {
        const conversation = this.conversations.find(
          (entry) => entry.id === (args.conversation_id as Id),
        );
        if (conversation) conversation.system_prompt = String(args.system_prompt ?? "");
        return null as T;
      }

      case "set_main_leaf": {
        const conversation = this.conversations.find(
          (entry) => entry.id === (args.conversation_id as Id),
        );
        if (!conversation) throw new Error("conversation not found");
        conversation.main_leaf_id = (args.message_id as Id) ?? null;
        conversation.updated_at_ms = Date.now();
        this.emit("conversation-changed", { conversation_id: conversation.id });
        return null as T;
      }

      case "create_group": {
        const group: Group = {
          id: uuid(`group-${this.groups.length + 1}`),
          name: String(args.name ?? ""),
          sort_order: this.groups.length,
        };
        this.groups = [...this.groups, group];
        this.emit("conversations-changed", {});
        return group as T;
      }

      case "rename_group": {
        const group = this.groups.find((entry) => entry.id === (args.group_id as Id));
        if (!group) throw new Error("group not found");
        group.name = String(args.name ?? "");
        this.emit("conversations-changed", {});
        return null as T;
      }

      case "delete_group": {
        this.groups = this.groups.filter((entry) => entry.id !== (args.group_id as Id));
        this.conversations = this.conversations.map((conversation) =>
          conversation.group_id === (args.group_id as Id)
            ? { ...conversation, group_id: null }
            : conversation,
        );
        this.emit("conversations-changed", {});
        return null as T;
      }

      case "load_conversation":
        return this.detail(args.conversation_id as Id) as T;

      case "get_draft": {
        const id = args.conversation_id as Id;
        const draft =
          this.drafts.get(id) ??
          ({
            text: "",
            attachments: [],
            edit_text: "",
            edit_attachments: [],
            edit_target_id: null,
            revision: 0,
          } satisfies Draft);
        return draft as T;
      }

      case "save_draft": {
        const id = args.conversation_id as Id;
        const draft = this.drafts.get(id) ?? {
          text: "",
          attachments: [],
          edit_text: "",
          edit_attachments: [],
          edit_target_id: null,
          revision: 0,
        };
        const changed = Boolean(args.changed);
        if (changed || draft.text !== String(args.text ?? "")) draft.revision += 1;
        draft.text = String(args.text ?? "");
        draft.attachments = (args.attachments as Attachment[]) ?? [];
        this.drafts.set(id, draft);
        return { revision: draft.revision } as T;
      }

      case "save_edit_buffer": {
        const id = args.conversation_id as Id;
        const current = this.editBuffers.get(id) ?? {
          target_id: null,
          text: "",
          attachments: [],
        };
        const changed = Boolean(args.changed);
        if (changed) current.target_id = (args.target_id as Id | null) ?? current.target_id;
        current.text = String(args.text ?? "");
        current.attachments = (args.attachments as Attachment[]) ?? [];
        this.editBuffers.set(id, current);
        const draft = this.drafts.get(id);
        if (draft) draft.revision += 1;
        return { revision: draft?.revision ?? 0 } as T;
      }

      case "clear_edit_buffer": {
        const id = args.conversation_id as Id;
        this.editBuffers.set(id, { target_id: null, text: "", attachments: [] });
        const draft = this.drafts.get(id);
        if (draft) draft.revision += 1;
        return { revision: draft?.revision ?? 0 } as T;
      }

      case "refresh_catalog": {
        const catalog = { entries: CATALOG, updated_at_ms: Date.now() };
        this.emit("catalog-changed", catalog);
        return catalog as T;
      }

      case "list_preferences":
        return this.preferences as T;

      case "set_model_preference": {
        const modelId = String(args.model_id ?? "");
        const patch = args.preference as {
          enabled: boolean;
          sort_order: number;
          is_default: boolean;
          overrides: Record<string, unknown> | null;
        };
        // The backend upserts: a model that has no preference yet gets one.
        const existing = this.preferences.find(
          (preference) => preference.model_id === modelId,
        );
        if (existing) {
          this.preferences = this.preferences.map((preference) =>
            preference.model_id === modelId ? { ...preference, ...patch } : preference,
          );
        } else {
          this.preferences = [
            ...this.preferences,
            {
              provider_id: String(args.provider_id ?? PROVIDER_ID),
              model_id: modelId,
              ...patch,
            },
          ];
        }
        this.preferences = this.preferences.map((preference) =>
          patch.is_default
            ? { ...preference, is_default: false }
            : preference,
        );
        savePreferences(this.preferences);
        return null as T;
      }

      case "set_primary_model": {
        const modelId = String(args.model_id ?? "");
        this.preferences = this.preferences.map((preference) => ({
          ...preference,
          is_default: preference.model_id === modelId,
        }));
        savePreferences(this.preferences);
        return null as T;
      }

      case "check_key_status":
        return {
          status: "ok",
          label: "mock fixture",
          usage: "12.34",
          limit: "100.00",
          limit_remaining: "87.66",
          error: null,
        } as T;

      case "send_message": {
        const request = args as unknown as SendMessageRequest;
        const conversation = this.conversations.find(
          (entry) => entry.id === request.conversation_id,
        );
        if (!conversation) throw new Error("conversation not found");
        const list = this.messages.get(conversation.id) ?? [];
        const generations = this.generations.get(conversation.id) ?? [];
        const batchId = uuid("batch");
        const parentId =
          request.selection.kind === "multi_branch"
            ? null
            : (request.selection.leaf ?? conversation.main_leaf_id ?? null);
        const userId = uuid(`msg-q-${list.length}`);
        list.push({
          id: userId,
          conversation_id: conversation.id,
          parent_id: parentId,
          role: "user",
          content: request.prompt,
          attachments: request.attachments,
          created_at_ms: Date.now(),
        });

        const targets =
          request.selection.kind === "multi_branch"
            ? (request.selection.branches ?? []).map((branch) => branch.target)
            : (request.selection.targets ?? []);

        const assistantOf: Record<string, string> = {};
        targets.forEach((target, index) => {
          const assistantId = uuid(`msg-a-${list.length}-${index}`);
          const requestId = uuid(`gen-${list.length}-${index}`);
          assistantOf[requestId] = assistantId;
          list.push({
            id: assistantId,
            conversation_id: conversation.id,
            parent_id: userId,
            role: "assistant",
            content: "",
            attachments: [],
            created_at_ms: Date.now() + index,
          });
          generations.push({
            request_id: requestId,
            batch_id: batchId,
            user_message_id: userId,
            assistant_message_id: assistantId,
            provider_id: target.provider_id,
            model_id: target.model_id,
            // Native parity: the row starts connecting; the events (started /
            // first delta) drive it to streaming through generation/progress.
            status: "connecting",
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
          });
          this.streamAnswer(conversation.id, assistantId, requestId, batchId);
        });

        this.messages.set(conversation.id, list);
        this.generations.set(conversation.id, generations);
        const firstAssistant = Object.values(assistantOf)[0];
        if (firstAssistant) conversation.main_leaf_id = firstAssistant;
        conversation.updated_at_ms = Date.now();
        if (!conversation.title) {
          conversation.title = request.prompt.slice(0, 40) || "New chat";
          conversation.title_source = "generated";
        }
        this.emit("conversations-changed", {});

        // The send transaction consumes the draft — but only when the
        // revision still equals the frozen one (the native compare-and-swap).
        const draft = this.drafts.get(conversation.id);
        if (draft && draft.revision === request.expected_draft_revision) {
          if (request.mode === "edit") {
            draft.edit_text = "";
            draft.edit_attachments = [];
            draft.edit_target_id = null;
          } else {
            draft.text = "";
            draft.attachments = [];
          }
        }

        const receipt: SendReceipt = {
          conversation_id: conversation.id,
          batch_id: batchId,
          primary_request_id: Object.keys(assistantOf)[0] ?? "",
          assistant_of: assistantOf,
          adopted_main_leaf: firstAssistant ?? null,
        };
        return receipt as T;
      }

      case "cancel_request": {
        this.cancelled.add(args.request_id as Id);
        return null as T;
      }

      case "retry_request": {
        // Faithful to the native service: a retry creates a NEW assistant
        // sibling under the same question, linked by retry_of / retry_root,
        // and refuses when the lineage already has a live attempt. The UI
        // folds the attempts into one slot.
        const requestId = String(args.request_id ?? "");
        let original: Generation | undefined;
        let conversation: ConversationSummary | undefined;
        for (const entry of this.conversations) {
          const found = (this.generations.get(entry.id) ?? []).find(
            (candidate) => candidate.request_id === requestId,
          );
          if (found) {
            original = found;
            conversation = entry;
            break;
          }
        }
        if (!original || !conversation) {
          throw { code: "not_found", message: "generation not found" };
        }
        const root = original.retry_root_id ?? original.request_id;
        const lineage = this.generations.get(conversation.id) ?? [];
        if (lineage.some((g) => g.retry_root_id === root && !isTerminal(g.status))) {
          throw {
            code: "admission.duplicate",
            message: "An attempt is already in flight",
            detail: "retry_in_flight",
          };
        }
        const list = this.messages.get(conversation.id) ?? [];
        const batchId = uuid("batch");
        const assistantId = uuid(`msg-retry-${list.length}`);
        const newRequestId = uuid(`gen-retry-${list.length}`);
        list.push({
          id: assistantId,
          conversation_id: conversation.id,
          parent_id: original.user_message_id,
          role: "assistant",
          content: "",
          attachments: [],
          created_at_ms: Date.now(),
        });
        lineage.push({
          ...original,
          request_id: newRequestId,
          batch_id: batchId,
          assistant_message_id: assistantId,
          status: "connecting",
          remote_generation_id: null,
          error: null,
          retry_of: original.request_id,
          retry_root_id: root,
          failure_kind: null,
          failure_message: null,
          started_at_ms: Date.now(),
          finished_at_ms: null,
        });
        this.messages.set(conversation.id, list);
        this.generations.set(conversation.id, lineage);
        // Receipt CAS: the leaf follows the new attempt only while it still
        // points at the replaced card (same rule as the native backend).
        if (
          conversation.main_leaf_id === original.assistant_message_id ||
          conversation.main_leaf_id === original.user_message_id
        ) {
          conversation.main_leaf_id = assistantId;
        }
        this.emit("conversation-changed", { conversation_id: conversation.id });
        this.streamAnswer(conversation.id, assistantId, newRequestId, batchId);
        const receipt: SendReceipt = {
          conversation_id: conversation.id,
          batch_id: batchId,
          primary_request_id: newRequestId,
          assistant_of: { [newRequestId]: assistantId },
          adopted_main_leaf: assistantId,
        };
        return receipt as T;
      }

      case "remove_message": {
        const messageId = args.message_id as Id;
        for (const [conversationId, list] of this.messages) {
          const index = list.findIndex((entry) => entry.id === messageId);
          if (index >= 0) {
            list.splice(index, 1);
            this.messages.set(conversationId, list);
            this.emit("conversation-changed", { conversation_id: conversationId });
            break;
          }
        }
        return null as T;
      }

      case "export_markdown": {
        const conversation = this.conversations.find(
          (entry) => entry.id === (args.conversation_id as Id),
        );
        const title = conversation?.title || "chat";
        return {
          filename: `${title.replace(/[^\w-]+/g, "-")}.md`,
          content: `# ${title}\n\n(mock export fixture)\n`,
        } as T;
      }

      case "export_markdown_to_file": {
        // The native shell writes the file; the mock reports where it would
        // have gone (no file is created in the browser harness).
        const filename = String(args.filename ?? "export.md");
        return { path: `/tmp/${filename} (mock fixture)`, sections: 1 } as T;
      }

      case "credential_status":
        return this.credentials as T;

      case "reload_credentials":
        // The backend re-reads the local .env; the mock keeps its fixture.
        return this.credentials as T;

      default:
        throw new Error(`mock backend: unknown command "${command}"`);
    }
  }

  async listen<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
    const set = this.listeners.get(event) ?? new Set();
    set.add(handler as (payload: unknown) => void);
    this.listeners.set(event, set);
    return () => set.delete(handler as (payload: unknown) => void);
  }
}

export function createMockTransport(fixture: FixtureName = "default"): Transport {
  const backend = new MockBackend(fixture);
  return {
    invoke: (command, args) => backend.invoke(command, args ?? {}),
    listen: (event, handler) => backend.listen(event, handler),
  };
}
