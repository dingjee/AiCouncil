-- Composer draft persistence (drafts across close/restart, per conversation).
--
-- `draft` / `draft_attachments_json` keep the normal composer buffer; the new
-- `edit_draft*` columns hold an independent buffer for "edit the latest
-- question" so that editing a question never overwrites the normal draft.
--
-- `draft_revision` counts user edits of either buffer and is the CAS token
-- the send transaction uses to consume a draft: a draft is cleared only when
-- the row still carries the revision that was frozen at submit time, so a
-- late receipt can never erase text typed after the send.

ALTER TABLE conversations ADD COLUMN draft_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE conversations ADD COLUMN edit_draft TEXT NOT NULL DEFAULT '';
ALTER TABLE conversations ADD COLUMN edit_draft_attachments_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE conversations ADD COLUMN edit_target_id TEXT;
CREATE INDEX IF NOT EXISTS idx_conversations_edit_target ON conversations(edit_target_id);
