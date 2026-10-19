-- 0003: 会话标题元数据 + 辅助请求台账(PLAN 7.1 / T08)
-- 旧非空标题保护为 manual;旧空标题进入自动流程。

ALTER TABLE conversations ADD COLUMN title_source TEXT NOT NULL DEFAULT 'empty';
ALTER TABLE conversations ADD COLUMN title_status TEXT NOT NULL DEFAULT 'needs_generation';
ALTER TABLE conversations ADD COLUMN title_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE conversations ADD COLUMN title_attempt_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE conversations ADD COLUMN title_attempted_at INTEGER;

-- 保守迁移:已有标题视为手动(不被自动流程覆盖)
UPDATE conversations SET title_source = 'manual', title_status = 'ready' WHERE title != '';

CREATE TABLE auxiliary_requests (
    request_token  TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    purpose        TEXT NOT NULL CHECK (purpose IN ('title')),
    turn_id        TEXT,
    provider       TEXT NOT NULL DEFAULT 'deepseek',
    model          TEXT NOT NULL,
    status         TEXT NOT NULL CHECK (status IN ('in_flight','completed','failed')),
    started_at     INTEGER NOT NULL,
    finished_at    INTEGER,
    prompt_tokens  INTEGER,
    completion_tokens INTEGER,
    cost_text      TEXT,
    cost_status    TEXT NOT NULL CHECK (cost_status IN ('known','unknown')),
    error_kind     TEXT,
    UNIQUE(conversation_id, purpose, turn_id)
);
CREATE INDEX idx_aux_requests_conversation ON auxiliary_requests(conversation_id);
