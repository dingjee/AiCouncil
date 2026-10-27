/**
 * DTOs exchanged with the Rust backend.
 *
 * These mirror `docs/TAURI_IPC.md` (v1) exactly: snake_case field names,
 * epoch-millisecond timestamps with the `_ms` suffix, and monetary values as
 * decimal strings (never floats).
 *
 * Invariants that must survive the boundary:
 * - API keys are never returned: the credential payload is a boolean map.
 * - Costs are decimal strings; the UI rounds them with the same rule as the
 *   Rust ledger (`format_usd`, half away from zero) and never re-parses them
 *   into a float.
 */

export type Id = string;

export type TitleSource = "empty" | "fallback" | "generated" | "manual";
export type TitleStatus = "ready" | "needs_generation" | "in_flight";

export interface ConversationSummary {
  id: Id;
  title: string;
  group_id: Id | null;
  system_prompt: string | null;
  title_source: TitleSource;
  title_status: TitleStatus;
  title_revision: number;
  main_leaf_id: Id | null;
  draft_revision: number;
  created_at_ms: number;
  updated_at_ms: number;
}

export interface Group {
  id: Id;
  name: string;
  sort_order: number;
}

export interface Attachment {
  name: string;
  content: string;
}

export type Role = "user" | "assistant";

export interface Message {
  id: Id;
  conversation_id: Id;
  parent_id: Id | null;
  role: Role;
  content: string;
  created_at_ms: number;
  attachments: Attachment[];
}

export type GenerationStatus =
  | "queued"
  | "connecting"
  | "streaming"
  | "completed"
  | "cancelled"
  | "failed"
  | "interrupted";

export type DispatchPhase = "not_dispatched" | "attempted" | "remote_accepted";

export interface Generation {
  request_id: Id;
  batch_id: Id;
  user_message_id: Id;
  assistant_message_id: Id;
  provider_id: Id;
  model_id: string;
  status: GenerationStatus;
  system_prompt: string | null;
  /** Send-time price snapshot (decimal string per million tokens). */
  price_prompt: string | null;
  price_completion: string | null;
  remote_generation_id: string | null;
  error: string | null;
  retry_of: Id | null;
  retry_root_id: Id | null;
  retry_not_before_ms: number | null;
  dispatch_phase: DispatchPhase;
  failure_kind: string | null;
  failure_message: string | null;
  started_at_ms: number | null;
  finished_at_ms: number | null;
}

export type UsageStatus = "pending" | "known" | "unavailable";
export type CostSource = "api" | "generation_query" | null;

export interface UsageRecord {
  request_id: Id;
  prompt_tokens: number | null;
  completion_tokens: number | null;
  total_tokens: number | null;
  /** Exact decimal string from the ledger (e.g. "0.000123"), or null. */
  cost_text: string | null;
  cost_source: CostSource;
  status: UsageStatus;
}

export interface EditLatest {
  target_id: Id;
  content: string;
  attachments: Attachment[];
}

export interface ConversationDetail {
  conversation: ConversationSummary;
  messages: Message[];
  generations: Generation[];
  usage: UsageRecord[];
  edit_latest: EditLatest | null;
}

export interface CatalogEntry {
  id: string;
  provider_id: Id;
  name: string;
  family: string | null;
  context_length: number | null;
  prompt_price: string | null;
  completion_price: string | null;
  supports_streaming: boolean;
  supports_temperature: boolean;
}

export interface CatalogState {
  updated_at_ms: number | null;
  entries: CatalogEntry[];
}

export interface ModelPreferenceEntry {
  provider_id: Id;
  model_id: string;
  enabled: boolean;
  sort_order: number;
  is_default: boolean;
  overrides: Record<string, unknown> | null;
}

export interface ModelPreferencePatch {
  enabled: boolean;
  sort_order: number;
  is_default: boolean;
  overrides: Record<string, unknown> | null;
}

export interface Settings {
  max_concurrent_requests: number;
  assumed_output_tokens: number;
  connect_timeout_secs: number;
  read_idle_timeout_secs: number;
  body_text_size: number;
  locale: string;
  theme_id: string;
  sidebar_width: number;
  sidebar_collapsed: boolean;
}

export type EgressVerdict = "allowed" | "blocked" | "unknown";

export interface EgressState {
  verdict: EgressVerdict;
  country_code: string | null;
  detail: string;
}

export interface BootstrapState {
  app_version: string;
  settings: Settings;
  conversations: ConversationSummary[];
  groups: Group[];
  pending_new_conversation: Id | null;
  catalog: CatalogState;
  preferences: ModelPreferenceEntry[];
  /** Boolean map: `true` means a key exists for that provider. */
  credentials: Record<string, boolean>;
  egress: EgressState;
}

export interface Draft {
  text: string;
  attachments: Attachment[];
  edit_text: string;
  edit_attachments: Attachment[];
  edit_target_id: Id | null;
  revision: number;
}

export interface DraftRevision {
  revision: number;
}

export interface ModelTarget {
  provider_id: Id;
  model_id: string;
}

export interface BranchTarget {
  leaf: Id;
  target: ModelTarget;
}

export type SelectionKind = "single" | "same_question_multi" | "multi_branch";

export interface Selection {
  kind: SelectionKind;
  leaf?: Id | null;
  targets?: ModelTarget[];
  branches?: BranchTarget[];
  primary?: number;
}

export interface SendMessageRequest {
  conversation_id: Id;
  mode: "normal" | "edit";
  prompt: string;
  attachments: Attachment[];
  expected_draft_revision: number;
  selection: Selection;
}

export interface SendReceipt {
  conversation_id: Id;
  batch_id: Id;
  primary_request_id: Id;
  assistant_of: Record<Id, Id>;
  adopted_main_leaf: Id | null;
}

export interface KeyStatus {
  status: "ok" | "unsupported" | "permission_denied" | "error";
  label: string | null;
  usage: string | null;
  limit: string | null;
  limit_remaining: string | null;
  error: string | null;
}

export type ExportScope = "main" | "selected" | "all_branches";

export interface ExportResult {
  filename: string;
  content: string;
}

export interface IpcError {
  code: string;
  message: string;
  detail?: string;
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

export type GenEvent =
  | { type: "started" }
  | { type: "text_delta"; delta: string }
  | {
      type: "usage_updated";
      prompt_tokens: number | null;
      completion_tokens: number | null;
      cost: string | null;
    }
  | { type: "remote_generation_id"; id: string }
  | {
      type: "finished";
      remote_generation_id: string | null;
      finish_state: "stop" | "length" | "content_filter" | "other";
    }
  | {
      type: "failed";
      error: {
        kind: string;
        message: string;
        retry_after_secs: number | null;
      };
    }
  | { type: "cancelled" }
  | { type: "rejected"; detail: string }
  | { type: "persist_error"; error: string };

export interface ChatEvent {
  conversation_id: Id;
  batch_id: Id;
  request_id: Id;
  assistant_message_id: Id;
  event: GenEvent;
}

export interface TitleEvent {
  conversation_id: Id;
  request_token: string;
  outcome: "ok" | "failed";
  error_kind: string | null;
}

export interface ConversationsChangedEvent {
  /** empty payload */
}

export interface ConversationChangedEvent {
  conversation_id: Id;
}
