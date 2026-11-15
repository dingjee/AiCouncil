# macOS 0.2.3 修复说明(Connecting 卡死 / 生命周期 / 标题)

## 诊断(实测)

1. **所有回答卡在 Connecting**:0.2.2 的 `GenEventDto::Failed` 把
   `error` 字段 `#[serde(flatten)]` 到事件顶层,线格式为
   `{"type":"failed","kind":...,"message":...}`;前端契约期望嵌套
   `error`,处理器读 `event.error.message` 抛 TypeError,终态后的
   权威刷新(`load_conversation`)从不执行。DB 显示 5 条生成均
   ~0.8s 内失败(401 "User not found."),UI 却停在 Connecting。
2. **Key 401 根因(非代码)**:`.env` 路径优先级不一致——项目根
   `.env` 已更新为新 key,而 `Application Support/.env` 仍是 10-28
   旧 key(`/key` 401 "User not found")。已按用户确认把根 `.env` 的
   `OPENROUTER_API_KEY` 原子同步到 Application Support(mode 0600,
   其他条目未动);代码不做任何绕过/伪造。
3. **生命周期显示错误**:`ChatService` 收到 Started 只写 DB
   Connecting、不对外发 Started;首个远端接受/文本未把 DB 置
   Streaming。前端 `started` 事件创建空 ID/模型的骨架并过早标
   streaming。
4. **标题超时**:标题客户端独立 5s 连接/12s 总超时、独立代理配置;
   实测 DeepSeek 同请求最小化 POST 200 用时 4.89s(用户辅助请求
   失败记录 5.003s、error_kind=timeout)——恰是旧的 5s **连接**
   超时把可用请求判死;非模型/鉴权问题(DeepSeek key 有效,
   `GET /models` 200)。且 body 阶段超时被误分类为
   invalid_response。另:头部只要 `title_source=fallback` 就显示
   "自动标题失败",把 pending(in_flight/未尝试)也当失败。
   (真实失败 = fallback + needs_generation + 已有尝试 >0;
   TitleStatus 无 failed 值。)

## 修复

1. **DTO**:去掉 `Failed` 的 flatten,恢复嵌套
   `error:{kind,message,retry_after_secs}`。Rust 线格式回归与前端
   测试共用同一夹具文件
   `frontend/tests/fixtures/native_failed_event.json`(两侧不得各自
   发明事件形态)。
2. **前端**:事件处理全程 try/catch,单个畸形事件不再阻断终态刷新
   (不记录载荷,不泄密);failed 事件兼容旧扁平形态(规范化后
   toast,不抛错);`started` 以 `generation/progress` patch 合并进
   **现有**生成行(保留 model/provider/失败数据),只标 connecting;
   首个远端 id/文本才标 streaming;终态行不被迟到的进度复活;
   reducer 按 conv+request 对当前行做合并(同步连发 remote_id+
   delta 不互相覆盖,streaming 不回退 connecting)。
3. **后端生命周期**:Started 在 Connecting 落库后对外发出;首个
   RemoteGenerationId / TextDelta 把 DB 置 Streaming。
4. **标题客户端**:与 NetworkConfig/`AICOUNCIL_PROXY` 同一代理策略;
   具名常量 `TITLE_CONNECT_TIMEOUT=15s`、`TITLE_TOTAL_TIMEOUT=45s`
   (总预算强制,含 body 阶段);body 超时分类为 timeout;
   无自动付费重试。头部失败提示仅在实际失败(见诊断 4 判定)时
   显示;`title_attempt_count` 经既有 DTO 暴露,无 schema 迁移。
5. **版本**:0.2.3(Cargo.toml/Cargo.lock/tauri.conf.json/
   frontend/package.json/initialState/release-check)。

## 状态

- 后端:已验证 — `cargo test --release --locked --lib --test p2_mock_stream
  --test p_title_metadata` 全部通过(109 单元 + 17 SSE 集成 + 6 标题
  元数据 = 132);release 构建完成,release-check 通过(版本/签名/
  arm64/前端一致性)。
- 实网探针(用最终编译库 + 实际运行时 env,均通过):
  - `DeepSeekTitleClient` 返回 4 字标题(连接预算已放宽至 15s);
  - `OpenRouter::stream`(Kimi K3.1 Pro,max_tokens=16,reasoning
    disabled)收到文本增量并到达 Finished 终态。
- 前端:最终全量 214/214 通过(585 个断言);typecheck 通过。
  原生嵌套失败事件与旧版扁平事件均可刷新为失败状态;
  使用真实 App 组件断言 Connecting 时显示 Stop,收到失败后 Stop 消失。
  连续远端 ID/文本事件保留模型与远端 ID,迟到事件不复活终态。
- `cargo fmt --all -- --check` 与 `git diff --check` 通过。
- 已按 AGENTS.md 清理 target/、新建依赖目录和实网诊断临时文件。
