-- 0004_request_recovery:失败恢复、请求快照与重试 lineage。
-- 向前增量迁移;不改已发布列的含义,新增列均可空/有默认,旧 exe 读新库
-- 只是忽略未知列(不直接让旧 exe 写已升级库;恢复前使用独立备份副本)。

ALTER TABLE generations ADD COLUMN failure_json TEXT;
ALTER TABLE generations ADD COLUMN request_snapshot_json TEXT;
ALTER TABLE generations ADD COLUMN retry_root_id TEXT;
ALTER TABLE generations ADD COLUMN retry_not_before_ms INTEGER;
ALTER TABLE generations ADD COLUMN dispatch_phase TEXT NOT NULL DEFAULT 'not_dispatched';

-- backfill:沿 retry_of 链找最早原请求;原请求 root 为自己。
-- 异常链(超深/断链)降级独立 root,不阻塞迁移。
WITH RECURSIVE chain(request_id, root, depth) AS (
    SELECT request_id, request_id, 0 FROM generations
    UNION ALL
    SELECT g.request_id, c.root, c.depth + 1
    FROM generations g JOIN chain c ON g.retry_of = c.request_id
    WHERE g.retry_of IS NOT NULL AND c.depth < 64
)
UPDATE generations SET retry_root_id = COALESCE(
    (SELECT root FROM chain WHERE request_id = generations.request_id
     ORDER BY depth DESC LIMIT 1),
    request_id
);

-- lineage 唯一活动约束:相同非空 retry_root_id 在活动态中最多一条
-- (事务防重的兜底;不能用 model_id 做 lineage,避免误伤同模型不同分支)。
CREATE UNIQUE INDEX IF NOT EXISTS idx_generations_lineage_active
    ON generations(retry_root_id)
    WHERE retry_root_id IS NOT NULL
      AND status IN ('queued','connecting','streaming');

CREATE INDEX IF NOT EXISTS idx_generations_retry_root ON generations(retry_root_id);
