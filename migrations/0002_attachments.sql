-- P7B(0.1.2):纯文本附件。
-- 附件随消息 / 会话草稿以 JSON 序列化存储;旧数据默认空数组,平滑迁移。

ALTER TABLE messages ADD COLUMN attachments_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE conversations ADD COLUMN draft_attachments_json TEXT NOT NULL DEFAULT '[]';
