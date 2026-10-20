ALTER TABLE messages ADD COLUMN removed_at INTEGER;
CREATE INDEX IF NOT EXISTS idx_messages_visible ON messages(conversation_id, removed_at);
