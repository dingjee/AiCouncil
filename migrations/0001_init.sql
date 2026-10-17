-- AiCouncil 初始模式(PLAN 3.1)
-- 时间一律 UTC epoch 毫秒;金额十进制存 TEXT;密钥不入库。

CREATE TABLE groups (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE conversations (
    id            TEXT PRIMARY KEY,
    title         TEXT NOT NULL DEFAULT '',
    group_id      TEXT REFERENCES groups(id) ON DELETE SET NULL,
    system_prompt TEXT NOT NULL DEFAULT '',
    main_leaf_id  TEXT,
    draft         TEXT NOT NULL DEFAULT '',
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);
CREATE INDEX idx_conversations_group ON conversations(group_id);

CREATE TABLE messages (
    id              TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    parent_id       TEXT,
    role            TEXT NOT NULL CHECK (role IN ('user','assistant')),
    content         TEXT NOT NULL DEFAULT '',
    created_at      INTEGER NOT NULL,
    FOREIGN KEY (parent_id) REFERENCES messages(id)
);
CREATE INDEX idx_messages_conversation ON messages(conversation_id);
CREATE INDEX idx_messages_parent ON messages(parent_id);

CREATE TABLE send_batches (
    id                 TEXT PRIMARY KEY,
    conversation_id    TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    primary_request_id TEXT NOT NULL,
    sent_at            INTEGER NOT NULL
);

CREATE TABLE generations (
    request_id          TEXT PRIMARY KEY,
    batch_id            TEXT NOT NULL REFERENCES send_batches(id) ON DELETE CASCADE,
    user_message_id     TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    assistant_message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    provider_id         TEXT NOT NULL,
    model_id            TEXT NOT NULL,
    status              TEXT NOT NULL CHECK (status IN
        ('queued','connecting','streaming','completed','cancelled','failed','interrupted')),
    params_json         TEXT,
    system_prompt       TEXT,
    context_json        TEXT NOT NULL DEFAULT '[]',
    price_json          TEXT,
    remote_generation_id TEXT,
    error               TEXT,
    retry_of            TEXT REFERENCES generations(request_id),
    started_at          INTEGER,
    finished_at         INTEGER
);
CREATE INDEX idx_generations_batch ON generations(batch_id);
CREATE INDEX idx_generations_assistant ON generations(assistant_message_id);
CREATE INDEX idx_generations_remote ON generations(provider_id, remote_generation_id);

CREATE TABLE providers (
    id             TEXT PRIMARY KEY,
    kind           TEXT NOT NULL CHECK (kind IN ('openrouter','openai_compatible')),
    base_url       TEXT NOT NULL,
    credential_ref TEXT
);

CREATE TABLE catalog_models (
    provider_id     TEXT NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
    model_id        TEXT NOT NULL,
    name            TEXT NOT NULL,
    context_window  INTEGER,
    max_output      INTEGER,
    pricing_json    TEXT,
    capabilities_json TEXT,
    defaults_json   TEXT,
    available       INTEGER NOT NULL DEFAULT 1,
    updated_at      INTEGER NOT NULL,
    raw_json        TEXT NOT NULL,
    PRIMARY KEY (provider_id, model_id)
);

CREATE TABLE model_preferences (
    provider_id    TEXT NOT NULL,
    model_id       TEXT NOT NULL,
    enabled        INTEGER NOT NULL DEFAULT 0,
    sort_order     INTEGER NOT NULL DEFAULT 0,
    is_default     INTEGER NOT NULL DEFAULT 0,
    overrides_json TEXT,
    PRIMARY KEY (provider_id, model_id)
);

CREATE TABLE usage_records (
    request_id          TEXT PRIMARY KEY REFERENCES generations(request_id) ON DELETE CASCADE,
    prompt_tokens       INTEGER,
    completion_tokens   INTEGER,
    total_tokens        INTEGER,
    cost_text           TEXT,
    cost_source         TEXT CHECK (cost_source IN ('api','generation_query')),
    status              TEXT NOT NULL CHECK (status IN ('pending','known','unavailable')),
    remote_generation_id TEXT,
    queried_at          INTEGER
);

CREATE TABLE ui_state (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
