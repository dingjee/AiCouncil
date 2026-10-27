# AiCouncil Tauri IPC contract (v1)

Owner: K4C. Consumers: MM-F (frontend), GLM (storage/credentials).
Date: 2026-10-28. Status: **implemented backend commands match this document**.

This is the exact wire contract between the Tauri webview frontend and the
Rust backend. Command names, argument shapes and field names are normative
(snake_case everywhere: command names, argument keys, DTO fields). IDs are
UUID strings. Timestamps are epoch milliseconds UTC (`i64`). Monetary values
are decimal strings (e.g. `"0.000123"`) or `null`; never floats.

## Conventions

- Invoke: `invoke('command_name', { arg_key: value, ... })`. Every argument
  key below is the exact Rust parameter name. All backend commands are
  declared with `rename_all = "snake_case"`, so invoke dictionaries use
  snake_case keys exactly as written below (no camelCase conversion).
- Every command returns `Promise<T>` and rejects with
  `IpcError { code: string, message: string, detail?: string }`.
- `code` values (stable, frontend may branch on them):
  - `storage` — SQLite/IO failure (`message` is safe to display, English).
  - `not_found` — referenced conversation/message/group/generation missing.
  - `admission.egress_blocked` — Anthropic egress blocked; `detail` = country.
  - `admission.egress_unknown` — egress unverifiable; `detail` explains.
  - `admission.duplicate` — active attempt / lineage conflict.
  - `admission.cooldown` — `detail` = cooldown deadline epoch ms (`until_ms`).
  - `admission.needs_action` — `detail` = remedy id
    (`switch_ip|credentials|balance|edit_input|pick_model|none`).
  - `admission.invalid_branch` — selection/branch ineligible.
  - `invalid_input` — empty prompt, bad index, over attachment limits, etc.
  - `provider` — network/API failure during non-send operations.
  - `credentials_missing` — no API key configured for the requested action.
  - `cancelled` — user dismissed a native panel (export save).
- API keys are **never** returned by any command or event. Credential
  commands take the key as input and return only success/failure.

## Events (backend → frontend)

Subscribe with `listen('name', handler)` (Tauri event API).

### `chat-event`

```jsonc
{
  "conversation_id": "uuid-string",
  "batch_id": "uuid-string",
  "request_id": "uuid-string",
  "assistant_message_id": "uuid-string",
  "event": { /* one GenEvent variant, see below */ }
}
```

Route by IDs only — never by "current view". Variants (`event.type`):

- `{ "type": "started" }`
- `{ "type": "text_delta", "delta": "string" }` — append to that assistant
  message's content. Throttled to ~300 ms / 4 KiB by the backend.
- `{ "type": "usage_updated", "prompt_tokens": number|null, "completion_tokens": number|null, "cost": "decimal-string"|null }`
- `{ "type": "remote_generation_id", "id": "string" }`
- `{ "type": "finished", "remote_generation_id": "string"|null, "finish_state": "stop"|"length"|"content_filter"|"other" }`
- `{ "type": "failed", "error": { "kind": "unauthorized|forbidden|payment_required|rate_limited|model_unavailable|context_length|invalid_request|network|timeout|server|stream_aborted|empty_response|parse|protocol|cancelled", "message": "string", "retry_after_secs": number|null } }`
- `{ "type": "cancelled" }`
- `{ "type": "rejected", "detail": "string" }` — admission rejection, no request dispatched
- `{ "type": "persist_error", "error": "string" }` — terminal/content persist failure

### `title-event`

```jsonc
{ "conversation_id": "uuid-string", "request_token": "string",
  "outcome": "ok" | "failed", "error_kind": "string|null" }
```
On `ok` (or after any title change) refetch the conversation list.

### `conversations-changed`

Payload: `{}` (empty object). Emitted after any mutation affecting the
sidebar list (create/rename/delete conversation, group ops, title settle,
draft-relevant ordering). Refetch `list_conversations`.

### `catalog-changed`

Payload: `CatalogState` (same shape as `get_catalog`). Emitted after a
catalog refresh completes (including the startup background refresh).
Refetch or adopt the payload directly.

### `conversation-changed`

```jsonc
{ "conversation_id": "uuid-string" }
```
Emitted when a conversation's messages/generations/draft were persisted
(send persisted a batch, retry, message removal, edit-latest commit).
Refetch `load_conversation` if it is the open conversation.

## Bootstrap & settings

### `bootstrap()` → `BootstrapState`

```jsonc
{
  "app_version": "0.2.0",
  "settings": { /* Settings, see get_settings */ },
  "conversations": [ /* ConversationSummary[] */ ],
  "groups": [ /* Group[] */ ],
  "pending_new_conversation": "uuid-string|null",
  "catalog": { /* CatalogState */ },
  "preferences": [ /* ModelPreferenceEntry[] */ ],
  "credentials": { "openrouter": false, "deepseek": false },
  "egress": { /* EgressState, see check_egress */ }
}
```

### `get_settings()` / `update_settings(patch)` → `Settings`

`Settings`:
```jsonc
{
  "max_concurrent_requests": 4,
  "assumed_output_tokens": 1000,
  "connect_timeout_secs": 15,
  "read_idle_timeout_secs": 120,
  "body_text_size": 15,          // 12–22
  "locale": "en",
  "theme_id": "soft_sage",
  "sidebar_width": 300,          // 240–480
  "sidebar_collapsed": false
}
```
`update_settings` takes a partial object (any subset of keys) and returns the
merged `Settings`.

### `check_egress()` → `EgressState`

```jsonc
{ "verdict": "allowed|blocked|unknown",
  "country_code": "US|null", "detail": "string" }
```
Frontend should call this before `send_message` when any selected model is
Anthropic and show a blocking dialog on `blocked`/`unknown`. The backend
caches a short-lived permit internally; `send_message` re-validates
server-side regardless.

## Conversations & groups

DTOs:
```jsonc
// ConversationSummary
{ "id": "uuid", "title": "string", "group_id": "uuid|null",
  "system_prompt": "string|null",
  "title_source": "empty|fallback|generated|manual",
  "title_status": "ready|needs_generation|in_flight",
  "title_revision": 0,
  "main_leaf_id": "uuid|null",
  "draft_revision": 0,
  "created_at_ms": 0, "updated_at_ms": 0 }

// Group
{ "id": "uuid", "name": "string", "sort_order": 0 }
```

- `list_conversations()` → `ConversationSummary[]` (sorted `updated_at` desc)
- `create_conversation()` → `ConversationSummary`. Reuses the pending-new
  slot when one exists unsent; otherwise creates and records a new slot.
  Emits `conversations-changed`.
- `rename_conversation(conversation_id, title, expected_title_revision)` →
  `ConversationSummary`. Optimistic-lock on `title_revision`; `409`-style
  failure returns code `invalid_input` with detail `title_revision_conflict`
  — refetch and retry.
- `delete_conversation(conversation_id)` → `null`. Emits `conversations-changed`.
- `set_conversation_group(conversation_id, group_id: string|null)` → `null`
- `set_system_prompt(conversation_id, system_prompt: string)` → `null`
- `set_main_leaf(conversation_id, message_id)` → `null` ("continue from here")
- `create_group(name)` → `Group`
- `rename_group(group_id, name)` → `null`
- `delete_group(group_id)` → `null` (conversations become ungrouped)

## Conversation detail (messages, generations, usage)

### `load_conversation(conversation_id)` → `ConversationDetail`

```jsonc
{
  "conversation": { /* ConversationSummary */ },
  "messages": [ { "id": "uuid", "conversation_id": "uuid",
                  "parent_id": "uuid|null", "role": "user|assistant",
                  "content": "string", "created_at_ms": 0,
                  "attachments": [ /* Attachment */ ] } ],
  "generations": [ /* Generation */ ],
  "usage": [ /* UsageRecord */ ],
  "edit_latest": { "target_id": "uuid", "content": "string",
                   "attachments": [ /* Attachment */ ] } | null
}
```

- `Attachment`: `{ "name": "string", "content": "string" }` (text files only)
- `Generation`:
```jsonc
{ "request_id": "uuid", "batch_id": "uuid",
  "user_message_id": "uuid", "assistant_message_id": "uuid",
  "provider_id": "uuid", "model_id": "string",
  "status": "queued|connecting|streaming|completed|cancelled|failed|interrupted",
  "system_prompt": "string|null",
  "price_prompt": "decimal-string|null", "price_completion": "decimal-string|null",
  "remote_generation_id": "string|null", "error": "string|null",
  "retry_of": "uuid|null", "retry_root_id": "uuid|null",
  "retry_not_before_ms": 0, "dispatch_phase": "not_dispatched|attempted|remote_accepted",
  "failure_kind": "string|null", "failure_message": "string|null",
  "started_at_ms": 0, "finished_at_ms": 0 }
```
  (`price_prompt`/`price_completion` come from the send-time price snapshot;
  display costs from `usage`.) Non-applicable timestamps are `null`.
- `UsageRecord`:
```jsonc
{ "request_id": "uuid", "prompt_tokens": 0|null, "completion_tokens": 0|null,
  "total_tokens": 0|null, "cost_text": "decimal-string|null",
  "cost_source": "api"|"generation_query"|null, "status": "pending|known|unavailable" }
```
- `edit_latest`: present when the latest user question on the main branch is
  editable (no active request on its lineage, it is the deepest visible user
  message). Absent otherwise — branch eligibility is enforced here and again
  in `send_message`.

## Drafts (composer)

```jsonc
// Draft
{ "text": "string", "attachments": [ /* Attachment */ ],
  "edit_text": "string", "edit_attachments": [ /* Attachment */ ],
  "edit_target_id": "uuid|null", "revision": 0 }
```

- `get_draft(conversation_id)` → `Draft`
- `save_draft(conversation_id, text, attachments, changed: boolean)` →
  `{ "revision": 0 }`. `changed=true` bumps `draft_revision` even when the
  text equals the stored one (send-CAS relies on it). `changed=false` +
  identical content is a no-op.
- `save_edit_buffer(conversation_id, text, attachments, target_id: string|null, changed)` → `{ "revision": 0 }`
- `clear_edit_buffer(conversation_id)` → `{ "revision": 0 }`
- Draft text whitespace is preserved byte-exact; trim only for emptiness checks.

Attachment limits (backend-enforced, code `invalid_input`): ≤ 256 KiB per
file, ≤ 10 files, ≤ 1 MiB total, text only.

## Catalog & model preferences

```jsonc
// CatalogState
{ "updated_at_ms": 0 | null, "entries": [ /* CatalogEntry */ ] }

// CatalogEntry
{ "id": "string", "provider_id": "uuid", "name": "string",
  "family": "string|null", "context_length": 0,
  "prompt_price": "decimal-string|null", "completion_price": "decimal-string|null",
  "supports_streaming": true, "supports_temperature": true }

// ModelPreferenceEntry
{ "provider_id": "uuid", "model_id": "string",
  "enabled": false, "sort_order": 0, "is_default": false,
  "overrides": {} | null }
```

- `refresh_catalog()` → `CatalogState` (24 h TTL unless forced; failure keeps
  old data and returns it; requires OpenRouter key else `credentials_missing`)
- `list_preferences()` → `ModelPreferenceEntry[]`
- `set_model_preference(provider_id, model_id, preference: { enabled, sort_order, is_default, overrides })` → `null`.
  Setting `is_default: true` clears the previous default.
- `set_primary_model(provider_id, model_id)` → `null` (convenience alias)
- `check_key_status()` → `{ "status": "ok|unsupported|permission_denied|error", "label": "string|null", "usage": "decimal-string|null", "limit": "decimal-string|null", "limit_remaining": "decimal-string|null", "error": "string|null" }`

## Send / cancel / retry / remove

### `send_message(request)` → `SendReceipt`

```jsonc
// SendMessageRequest
{
  "conversation_id": "uuid",
  "mode": "normal" | "edit",        // edit = replace latest question turn
  "prompt": "string",
  "attachments": [ /* Attachment */ ],
  "expected_draft_revision": 0,     // CAS token:前端快照发送内容时的草稿版本
  "selection": {
    "kind": "single" | "same_question_multi" | "multi_branch",
    // single / same_question_multi:
    "leaf": "uuid|null",
    // same_question_multi / single targets:
    "targets": [ { "provider_id": "uuid", "model_id": "string" } ],
    // multi_branch:
    "branches": [ { "leaf": "uuid", "target": { "provider_id": "uuid", "model_id": "string" } } ],
    "primary": 0                    // index into targets (or branches)
  }
}
```
Semantics:
- `single`: one branch at `leaf`, one model.
- `same_question_multi`: one shared user node at `leaf`, one request per
  target; tab strip order = `targets` order; `primary` gets the main leaf.
- `multi_branch`: each `leaf` gets its own user node + one model.
- `mode: "edit"`: replaces the turn rooted at the edit buffer's target
  (must equal `edit_latest.target_id` from `load_conversation`); the normal
  draft is untouched and the edit buffer is consumed on success.
- **Draft consumption (CAS):** the sent draft is consumed only if the stored
  `draft_revision` still equals `expected_draft_revision` — i.e. the value
  the frontend froze when it snapshotted the text it is sending (the
  `revision` from `get_draft`/`save_draft`). If the user typed again after
  that snapshot, the newer text is **kept** (not wiped by the older send).
  Frontend rule: persist the current composer text with `save_draft`
  first (flush), then send with the revision you just received.

```jsonc
// SendReceipt
{ "conversation_id": "uuid", "batch_id": "uuid",
  "primary_request_id": "uuid",
  "assistant_of": { "uuid-request": "uuid-message" },
  "adopted_main_leaf": "uuid|null" }
```
The receipt is advisory; UI reconciles against `conversation-changed` /
`load_conversation` (DB is authoritative). On admission/storage failure the
send was NOT persisted — nothing to reconcile.

- `cancel_request(request_id)` → `null`
- `retry_request(request_id)` → `SendReceipt` (same-shape receipt).
  Eligibility (terminal, lineage inactive, cooldown, remedy) enforced with
  latest persisted data; failures use `admission.*` codes.
- `remove_message(message_id)` → `null` — hides a message from future
  context (keeps generations/usage ledger); refuses while a request on it is
  active (`invalid_input`).

## Export

- `export_markdown(conversation_id, scope: "main" | "selected" | "all_branches", message_ids?: string[])` →
  `{ "filename": "string", "content": "string" }`.
  `selected` requires `message_ids` (branch leafs). Returns the rendered
  Markdown; the frontend saves it (browser fallback).
- `export_markdown_to_file(conversation_id, scope, message_ids?)` →
  `{ "path": "string", "sections": number }`.
  Opens the **native macOS save panel** (NSSavePanel), then writes the file
  atomically (temp file + rename). This is the command to use for real
  exports: **show the success toast only after it resolves**, because a
  browser anchor download is not verified to produce a file in WKWebView.
  User cancels the panel → rejects with code `cancelled` (not an error to
  display).

## Credentials

`.env` is the **sole authoritative credential store** for both providers
(named variables `OPENROUTER_API_KEY` and `DEEPSEEK_API_KEY`, read from the
stable runtime data directory; agents never read or print values). There is
**no key-entry UI and no save/delete IPC** — the parent provisions `.env`.

- `credential_status()` → `{ "openrouter": false, "deepseek": false }`
  (`true` = the named variable is present and non-empty; the value itself is
  never returned)
- `reload_credentials()` → same status object. Rebuilds provider/title
  clients from `.env`; refuses (code `storage`, detail explains) while
  requests are in flight. Call after the parent updates `.env`.

## Cancellation & lifecycle notes

- The backend service outlives conversation switches; all events carry IDs.
- `bootstrap` must be called once after `listen` subscriptions are attached;
  events are not replayed, and `load_conversation` always returns
  authoritative persisted state (safe to refetch on any `*-changed` event).
- IME composition Enter must be handled by the frontend composer (do not
  send during composition); `Shift+Enter` = newline; `Cmd+Enter` sends.
- Frontend must ignore events for conversations it does not render and must
  never apply async responses to a switched-away conversation/draft without
  an ID + revision check.
