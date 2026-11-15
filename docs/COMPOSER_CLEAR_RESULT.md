# 输入框发送后清除：实施、自检与本地交付结果

日期：2026-11-01。计划：`docs/COMPOSER_CLEAR_PLAN.md`。
执行者：OpenCode `minimax-cn-coding-plan/MiniMax-M3.1-Flash`（interactive，单会话，未派生 agents）。
分支：`codex/tauri-macos`，版本保持 **0.2.3**。本轮**未** commit / push / stash / reset；工作区既有未提交改动全部保留。
本轮**未改动 Rust 后端与 IPC 协议**（理由见 A.3），未替换 `/Applications` 安装，未终止用户运行中的进程，未读取 `dist/data`、备份或任何真实凭据/用户数据，未发起任何付费 API 请求。

---

## A. 结论摘要

输入框“发送后不清空”的根因是**前端草稿生命周期缺少统一的所有权边界**，而不是单一条件判断：

1. 发送前 flush 保存了快照，但 600ms 自动保存定时器**没有被取消**、其回调也**没有在执行时重新校验内容**，于是同一个快照被第二次以 `changed=true` 写回；后端 `draft_revision` 前进，发送事务按旧 revision 的 CAS 清除失败，行内仍留旧正文，下一次刷新把旧正文回填输入框。
2. `loadConversation` 只有 selectionToken 与会话 id 检查，**没有同会话内的请求顺序 / 发送清除代际检查**：发送前发起的读取、两次乱序的 refetch 都能覆盖新状态。
3. `dispatchSend` 无论普通/编辑模式都拿**普通 composer**与提交快照比较：编辑发送既不清编辑缓冲（普通缓冲也被误比），`composer/clear` 又同时清两个缓冲；`applyConversationDetail` 无条件覆盖编辑缓冲。
4. 空草稿被自动保存直接跳过：删掉全部文字/附件后切换会话会恢复旧草稿。

本轮把“本地内容 / 后端已知内容 / 本地编辑代次 / 写队列 / 读取代次 / 已消费快照”收敛为**每会话一条 composer 生命周期记录**，所有写入与读取都在同一套规则下判定。修复全部落在前端；后端与 IPC 未改。

---

## A.1 真实根因（复现与证据）

后端语义（未改动，作为判定依据）：

- `src/storage/mod.rs::save_draft_content`：内容不同**或** `changed=true` → `draft_revision + 1` 并写库；内容相同且 `changed=false` → 不写库、不涨版本。
- `src/storage/mod.rs` 发送事务（与批次同事务）：`UPDATE conversations SET draft='' ... WHERE id=? AND draft_revision = expected`——**版本仍等于冻结版本才清空**，清空本身不涨版本。
- `src/desktop/commands.rs::send_message`：CAS 用**前端冻结的** `expected_draft_revision`。

因此：**发送前任何一次多余的内容写入都会让发送事务的 CAS 失败**（消息照常落库，草稿留在库里）。这正是计划里描述的病因，前端无法靠“忽略保存回执”绕过（保存可能已经写回数据库），只能让“同一快照只被写一次、且写序可控”。

## A.2 最终修复（源码改动）

### A.2.1 `frontend/src/state/store.tsx`（核心）

新增**每会话 composer 生命周期记录** `ConvDraft`（替代原 `draftBaselines` / `lastSavedDrafts` 两个 map）：

| 字段 | 作用 |
| --- | --- |
| `normal` / `edit`（`DraftSlot`） | 两个互相独立的缓冲：`local`（用户当前内容，唯一真相）、`saved`（后端已知内容）、`version`（本地编辑计数）、`syncedVersion`（上次与后端同步时的版本）、`dirty`（后端尚未计入的真实变化）、`expectEmpty`（发送已消费该缓冲 → 行内必须为空）、`timer` |
| `consumed` / `consumedEdit` | 已被发送消费的快照：任何读取都不得把它放回输入框 |
| `queue` | 该会话的写队列（写入串行化） |
| `readSeq` / `appliedRead` | 同会话读取的发起序与已应用序（乱序读取不回滚新状态） |
| `clearGen` | 每次发送成功 +1：发送前发起的读取因此被判为 **stale** |
| `revision` | 该会话后端 revision（CAS 令牌） |

关键规则：

1. **写入全部串行化且执行时重新校验**：`enqueueDraftWrite` 按会话排队；`persistSlot` 在**真正执行时**读取 `slot.local`（而不是被安排时的快照），并在“后端已持有相同内容且不是新编辑”时**完全跳过写入**。→ 计划里“发送前处理已排队与进行中的同内容草稿保存，避免同快照重复 changed=true；回调执行前重新校验”两条同时满足。
2. **`dirty` 表达“真实新输入”**：本地每次编辑都 `version+1` 且 `dirty=true`；写入时 `changed = dirty`。于是“重新输入同样文字”仍按后端语义算一次新版本，而“重复保存同一快照”不再 `changed=true`。
3. **发送前 flush 在写队列内执行**：`flushBeforeSend` 先写普通草稿、后写编辑缓冲（两者共用后端 `draft_revision`，顺序反了会让编辑的 CAS 失效），返回写完后的权威 revision；整体 5s 上限，超时则用已知 revision 继续发送（发送不被慢后端阻塞）。
4. **发送快照冻结“内容 + 本地版本”**：`consumeSnapshot` 只在 `version` 未移动且内容仍相等时清空**对应模式**的缓冲；普通发送清普通缓冲，编辑发送清编辑缓冲，普通草稿不受影响；另一会话永不被触碰。`expectEmpty=true` 标记“该行必须为空”。
5. **读取带代次**：`loadConversation` 记录 `readSeq`/`clearGen`；`seq < appliedRead` 的旧快照整份丢弃；`clearGen` 变化 → 该读取标记 stale。`preserveEditBuffer`（stale / 已消费 / 本地编辑更新）让 reducer 保留本地编辑缓冲，不再被无条件覆盖。
6. **已消费快照与残留行**：读取拿回已消费内容时既不回填输入框，也把行重写为当前内容（修复 CAS 失败的残留）。
7. **迟到写入可修复**：被放弃（超时）的写入若仍然落库，落库后比较当前本地内容，不同即重写该行 → 迟到保存不会复活已发送内容，也不会把旧版本写回。
8. **发送失败与刷新失败分离**：`send_message` 被拒 → 草稿与附件原样保留 + “Send failed (draft kept)”；发送成功但刷新失败 → 只提示 “Could not refresh the chat”，**不再误报发送失败**。
9. **state 里的 revision 属于当前会话**：`draft/revision` 仅在写入/清除属于当前打开会话时派发，另一会话的保存回执不会覆盖它。
10. **切换会话**：离开的会话先入队保存其草稿，再清空输入框（避免上一会话正文出现在新会话，并避免把旧正文写进新会话的槽位）。
11. 空缓冲也是草稿：自动保存不再跳过空内容（删光文字/附件会被持久化为空）。

### A.2.2 `frontend/src/state/reducer.ts`

- `composer/clear` 只清**普通** composer；新增 `edit/clear` 只清**编辑**缓冲（两者不再互相清空）。
- `conversation/loaded` 新增 `preserveEditBuffer`：本地编辑缓冲比后端新时保留本地内容（只更新 `editLatestEligible`）。

### A.2.3 `frontend/src/components/Composer.tsx`

编辑缓冲改走 store 动作 `actions.setEditText` / `actions.setEditAttachments`（原先直接 `dispatch`），使 store **立刻**看到编辑输入（版本计数与发送快照不再有渲染滞后）。IME、Enter / Shift+Enter / Cmd+Enter、附件、多模型/分支、Stop 均未改动。

### A.2.4 `frontend/dev/mock-transport.ts`（开发态 mock，非生产包）

`send_message` 补上与后端一致的草稿 CAS（revision 相同才清空草稿），使浏览器 harness 的行为与原生一致。

### A.2.5 测试

- 新增 `frontend/tests/composer-lifecycle.test.tsx`（20 个生命周期用例，加上导入执行的 5 个 IPC 契约用例，共 25 项）：**有状态 mock** 实现 `save_draft` 的版本计数与发送事务的 CAS，用 deferred promise 控制读写完成顺序。
- `frontend/tests/store.test.tsx`：`FakeBackend` 升级为有状态（草稿行 + 发送 CAS）；“draft race”用例改为断言新的不变量——同一会话的写入串行（revision 是 CAS 令牌，重叠写会让旧值获胜）。
- `frontend/tests/reducer.test.ts`：`composer/clear` 用例改为断言**普通/编辑缓冲相互隔离**（原用例断言“同时清两个”，正是计划点名的缺陷）。

## A.3 为什么没有改后端 / IPC

计划的边界是“优先改前端；只有证明前端无法满足正确性才改最小后端”。本次逐条核对后端语义后确认：**后端已有正确且足够的语义**（`changed` 计数、CAS 消费、编辑缓冲独立列），全部竞态都能在前端用“写入串行化 + 执行时重校验 + 代次判定”消除，因此**未改 Rust、未改 IPC 协议、未新增依赖**（`bun.lock` 未变）。前端新依赖的后端语义只有两条，且都已在 `src/storage/mod.rs` 注释中写明：`changed=false` 且内容相同 → 不写库不涨版本；发送事务按冻结 revision CAS 消费。

## A.4 红转绿证据

### A.4.1 原实现上的红（`bun test tests/composer-lifecycle.test.tsx`，修复前）

```
bun test v1.4.4 (5d9cc0f4b)
 10 pass
 8 fail
 103 expect() calls
Ran 18 tests across 1 file. [10.95s]

(fail) ... a fast send across the autosave window writes the snapshot only once
       expect(backend.changedWrites("c1")).toBe(1)   → Received: 2
(fail) ... an autosave that is already running does not write the snapshot twice
       expect(backend.changedWrites("c1")).toBe(1)   → Received: 2
(fail) ... a successful send with a failing refresh is not reported as a failure
       toast "Send failed (draft kept): could not read the chat"
(fail) ... a draft read that returns after the clear does not refill the composer
       expect(composerText).toBe("")                → Received: "consumed text"
(fail) ... two refetches of the same chat that answer out of order keep the newer snapshot
       expect(messages.c1[0].content).toBe("new answer") → Received: "stale answer"
(fail) ... switching to another chat during the send keeps that chat intact
       expect(composerText).toBe("")                → Received: "second chat draft"
(fail) ... an edit that is changed during the send survives the receipt and the refresh
       expect(editingLatest).toBe("q1")             → Received: null
(fail) ... deleting the text and the attachments is persisted as an empty draft
       expect(row.text).toBe("")                    → Received: "old draft"
```

这 8 条红分别对应计划的第 1/2/3/4 条根因（重复写导致 CAS 失败、迟到读回填、刷新失败误报、编辑缓冲被覆盖、空草稿不持久化）。

### A.4.2 最小验收指出的两处缺口的红（修复前，本轮后段）

最小验收发现两处缺口，均已补测试并修复：

1. `applyConversationDetail` 在判定 stale 之前就已 dispatch `conversation/loaded`，且 `preserveEditBuffer` 未包含 stale / 已消费编辑；编辑发送清空后 `version == syncedVersion`，**发送前迟到的 edit draft 会重新开启编辑并填回旧文字**；`consumedEdit` 也未用于读回防护。
2. `persistSlot` 超时后的 repair 只在“迟到内容 == 已消费内容”时执行：旧自动保存 A 挂起、用户改为 B 并成功发送后，A 迟到落库不会被修复，可能回填 A；且另一会话的 revision 可能被会话外的保存回执覆盖。

对应回归测试（新增于 `frontend/tests/composer-lifecycle.test.tsx`）：

- `a late edit read does not re-open the edit mode of an accepted edit send`
- `an edit buffer that the backend still holds is corrected, not refilled`
- `a save that lands after the send does not refill the composer`
- `the reply of a save of another chat does not overwrite the revision of the open chat`

**逐条 mutation 验证（临时禁用修复 → 期望红 → 立即恢复）**：

| 临时禁用 | 结果 |
| --- | --- |
| `preserveEditBuffer` 去掉 `stale / editConsumed / editStaleRow` | `23 pass / 2 fail`：`a late edit read does not re-open the edit mode of an accepted edit send`、`an edit buffer that the backend still holds is corrected, not refilled` 双双变红 |
| 迟到写 repair 退回“仅 consumed 内容”条件 | `24 pass / 1 fail`：`a save that lands after the send does not refill the composer` 变红 |
| 去掉“revision 只属于当前会话”的守卫（`dispatch draft/revision` 无条件派发） | `the reply of a save of another chat does not overwrite the revision of the open chat` 变红（实测 `draftRevision` 由期望的 `0` 变为 `3`） |

三次禁用均已完整恢复（源码中已无 `TEMP`/调试输出，见 A.6 的核对命令）。

### A.4.3 修复后（绿）

```
$ cd frontend && bun run typecheck      # tsc --noEmit
（无输出 = 通过）

$ bun test
bun test v1.4.4 (5d9cc0f4b)
 242 pass
 0 fail
 714 expect() calls
Ran 242 tests across 19 files. [47.15s]
（基线 217 pass / 0 fail；本轮新增 25 个用例，其中 8 个在原实现上为红）

$ bun run build                         # tsc --noEmit && vite build
dist/assets/index-BtkYtS2o.css         46.94 kB │ gzip:  12.25 kB
dist/assets/index-7xZhR4Xe.js         721.59 kB │ gzip: 236.17 kB
✓ built in 996ms
```

## A.5 逐项自检矩阵（真实断言，均为运行真实 store + 有状态 mock，非源码字符串检查）

| 场景 | 用例 | 实际结果 |
| --- | --- | --- |
| 普通文本 + 附件成功发送 | `a successful send consumes the draft > the submitted text and attachments disappear and stay gone` | payload `prompt`/`attachments`/`mode` 正确；composer 清空；行内 `draft=''`；切走再切回仍为空 ✅ |
| 快输入立即发送，发送跨过 600ms | `a fast send across the autosave window writes the snapshot only once` | `changed=true` 写入**恰好 1 次**；CAS 成功（行内空）；后续 `conversation-changed` 刷新不回填 ✅ |
| 自动保存已开始再发送，保存回复延迟 | `an autosave that is already running does not write the snapshot twice` | 排队后只写 1 次；行内被消费；composer 空 ✅ |
| flush 超时及迟到完成 | `a timed out flush does not block the send and a late write does not revive it` | 发送未被卡死（<9s）；迟到写入落库后行被修正为空、composer 不复活 ✅ |
| 发送前 get_draft 延迟到清空后返回 | `a draft read that returns after the clear does not refill the composer` | 迟到读取不回填 ✅ |
| 同会话两个 refetch 乱序 | `two refetches of the same chat that answer out of order keep the newer snapshot` | 旧快照不覆盖新消息 ✅ |
| 发送期间输入 / 改附件 / 改分支 | `text, attachments and the branch selection survive the receipt` | 新正文、附件、分支选择全部保留且被持久化 ✅ |
| 发送期间切换另一会话再返回 | `switching to another chat during the send keeps that chat intact` | 另一会话草稿未被覆盖；原会话成功后内容不复活 ✅ |
| 编辑发送，普通缓冲已有草稿 | `an edit send only consumes the edit buffer` | 只清编辑快照；普通正文/附件保持 ✅ |
| 编辑期间的新改动不被旧回执/刷新覆盖 | `an edit that is changed during the send survives the receipt and the refresh` | `editingLatest`/`editText` 保留 ✅ |
| 编辑迟到读不清缓冲 | `a late edit read does not re-open the edit mode of an accepted edit send` | 迟到 edit draft 不重开编辑、不填回旧字 ✅ |
| 残留 CAS（编辑）回填 | `an edit buffer that the backend still holds is corrected, not refilled` | 行被修正为空，输入框不回填 ✅ |
| 迟到写（A 落库）不回填 | `a save that lands after the send does not refill the composer` | 行被修正、composer 为空、刷新不回填 ✅ |
| send 拒绝 / 门禁失败 | `a rejected send keeps the text and the attachments`、`the network region gate keeps the draft and sends nothing` | 草稿与附件保留；提示正确（“Send failed (draft kept)”）；门禁时不发请求 ✅ |
| 成功但 refetch 失败 | `a successful send with a failing refresh is not reported as a failure` | 不出现 “Send failed”；提示刷新失败；composer 已清 ✅ |
| 手工清空文字 / 移除最后附件后切回 | `deleting the text and the attachments is persisted as an empty draft` | 持久化为空；切回不恢复旧内容 ✅ |
| 取消编辑 / 重新选择会话 | `cancelling the edit leaves the normal draft alone`、`the edit buffer of a chat comes back when the chat is opened again` | 取消编辑不动普通草稿；切走再切回编辑缓冲与编辑模式恢复 ✅ |
| 另一会话回执不覆盖当前 revision | `the reply of a save of another chat does not overwrite the revision of the open chat` | 当前会话 revision 不变 ✅ |
| IME 与现有发送快捷键 | `frontend/tests/composer-ui.test.tsx`（既有） | 原用例全绿，无额外发送 ✅ |
| 既有行为（多模型/分支、Stop、费用、Models 工具栏、布局） | 其余既有测试 | 全绿（242/242）✅ |

## A.6 构建与本地交付（真实输出）

```
$ scripts/macos_build.sh
== 1/4 frontend build (Bun) ==
== 2/4 cargo build --release ==
   Finished `release` profile [optimized] target(s) in 1m 08s
   Built application at: /Users/jee/Documents/Projects/AiCouncil/target/release/aicouncil
   Bundling AiCouncil.app (…/target/release/bundle/macos/AiCouncil.app) (20.66 MiB)
== sha256 (dist/aicouncil) ==
== 3/4 .app bundle ==
target/release/bundle/macos/AiCouncil.app: replacing existing signature
bundle staged: dist/AiCouncil.app
== 4/4 embedded frontend identity (raw exe + bundle binary) ==
embedded frontend identity OK: raw exe (1 JS bundle(s))
embedded frontend identity OK: bundle binary (1 JS bundle(s))
OK: macos_build

$ scripts/macos_release_check.sh
== .app bundle: dist/AiCouncil.app ==
version: 0.2.3
arch: arm64 ok
== codesign (structure only; ad-hoc expected, NOT notarization) ==
   Info.plist entries=14 / TeamIdentifier=not set / Sealed Resources version=2 rules=13 files=1
== linked libraries (must not contain obsolete iced/gtk/winit stacks) ==
   （只有系统框架：AppKit / WebKit / Foundation / CoreGraphics / libSystem 等；无 iced/gtk/gdk/wayland/x11）
bundle size: 21M
== raw exe: dist/aicouncil ==
980c3f2dc037daffbfa6dcb200c8d0a71161a74d7347096f97eb551dfe54bd1c  dist/aicouncil
arch: arm64 ok
version string AiCouncil/0.2.3 present
native smoke harness absent in release (AICOUNCIL_NATIVE_SMOKE refs: 0)
embedded frontend manifest: dist/frontend-embed.sha256
embedded JS bundle(s): assets/index-7xZhR4Xe.js
RESULT: OK (version 0.2.3)
```

构建告警只有一条既有告警（`src/services/egress.rs:625` 的 `unused variable: deadline`，属上一轮未提交改动，本轮未触碰该文件）。

**交付物**：`dist/aicouncil`（release 可执行文件，sha256 `980c3f2d…4bd1c`）、`dist/AiCouncil.app`（21M，ad-hoc 签名，未公证）、`dist/aicouncil.sha256`、`dist/frontend-embed.sha256`（62 个前端资产）。版本保持 **0.2.3**，未替换 `/Applications` 安装，未触碰用户数据目录 `dist/data` 与 `dist/pc-data-backup`。

## A.7 未通过 / 未验证项（真实记录）

1. **浏览器几何 harness 未跑完**：`cd frontend && bun dev/geometry.ts` → **25/26 通过**，最后一项在“点击 Models 弹层的目录复选框”处中止：`CDP timeout: Input.dispatchMouseEvent`（20s 超时）。已按主协调要求**停止扩展调查**、未改 `ModelsPanel` 或其他无关代码；**该失败与本轮改动的归因未证实**（本轮改动集中在 store/reducer/Composer 的草稿生命周期，未触及目录弹层）。这是本轮唯一未通过的检查。
2. **未做真机（原生 app）验证**：本轮所有结论来自真实 store + 有状态 mock 的自动化用例与浏览器 mock harness，**没有**在安装后的原生 app 中实机操作输入框。本轮自动化与构建验收不涵盖原生界面的手工发送确认。
3. **未跑 Rust 测试**：本轮未改任何 Rust 文件，按计划“无改动不另跑全量 Rust 测试”。
4. **单会话写队列的副作用（有界）**：同一会话的写入严格串行；每次写入的超时上限为 5s，多个排队写入的等待可能累加；保存队列按会话划分。切换会话会等待离开会话的草稿写入，实际等待可能超过单次的 5s。
5. **切换会话时输入框会短暂清空**：为避免上一会话正文出现在新会话、并避免把旧正文写进新会话的槽位，切换时清空 composer（原实现会保留上一会话正文直到新草稿到达）。这是有意的行为变化。
6. **重新输入相同文字的保留语义**：用户在发送后重新输入与已发送完全相同的文字时，该输入被保留（版本前进 → 不清空），符合“旧回执不能删除新内容”；代价是这类输入需要用户自行再删一次才能看到清空效果。

## A.8 清理状态

- 本轮生成的 `target/`（cargo release 与 bundle 中间产物）、`gen/`（tauri schemas）、`frontend/dist/`（前端构建中间产物）已删除；`frontend/node_modules/` 已删除（复跑请先 `bun install --frozen-lockfile`）。
- 浏览器 harness 的临时目录与 captures（PNG）已删除；仓库内无新增 PNG/JPG；`/tmp` 下本轮的临时备份脚本已删除。
- 未提交 git commit / push；工作区仅余源码、回归测试、文档与 `dist/` 交付物；用户真实数据（`dist/data`、`dist/pc-data-backup`）原样保留、未读取。
- 未读取 `.env`、凭据、用户数据库；全程只用 mock fixture，未发起付费 API 请求。

## A.9 复核建议（给最小验收）

1. 只看本轮 diff：`frontend/src/state/store.tsx`、`frontend/src/state/reducer.ts`、`frontend/src/components/Composer.tsx`、`frontend/dev/mock-transport.ts`、`frontend/tests/composer-lifecycle.test.tsx`（新）、`frontend/tests/store.test.tsx`、`frontend/tests/reducer.test.ts`。
2. 重点抽查三处：保存调度与迟到读取的失效边界（`persistSlot` 执行时重校验 + `readSeq/appliedRead/clearGen`）、普通/编辑缓冲的清除隔离（`consumeSnapshot` + `composer/clear` / `edit/clear`）、以及“revision 只属于当前会话”的守卫。
3. 独立复跑：`cd frontend && bun install --frozen-lockfile && bun run typecheck && bun test`（期望 242 pass / 0 fail）。
4. 确认 `scripts/macos_build.sh` 与 `scripts/macos_release_check.sh` 的真实结果（见 A.6）以及 `target/` 已清理。
5. 浏览器 harness 的 1 项 CDP 超时（未通过、归因未证实）需要判断是否单独排查。


## B. Codex 最小验收

- 对照本轮开始时的文件快照抽查本轮变更，未发现修改无关业务文件；既有未提交的其他源码改动保持。
- 抽查保存调度、读取失效和普通/编辑缓冲隔离。退回的编辑迟到读取、旧 A 保存迟于新 B 发送的问题已由同一 OpenCode 会话补测并修复；未由 Codex 接管业务编码。
- 独立运行 `cd frontend && bun run typecheck && bun test ./tests/composer-lifecycle.test.tsx`：类型检查通过；**25 pass / 0 fail / 154 assertions**（20 个生命周期用例与 5 个 IPC 契约用例）。
- 独立运行 `scripts/macos_release_check.sh`：**RESULT: OK (version 0.2.3)**，签名结构、架构和嵌入资源身份检查通过。
- 核对 `target/`、`gen/`、`frontend/node_modules/`、`frontend/dist/` 已删除；本轮 Codex 临时基线和审计文件已清理。
- 本轮源码修复与本地打包通过最小验收。浏览器目录点击 CDP 超时仍未归因，原生界面未手工验证；不声称上述两项已通过。
