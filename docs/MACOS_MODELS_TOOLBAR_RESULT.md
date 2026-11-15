# Models 与工具栏修复 — 执行结果

日期：2026-10-28。计划：`docs/MACOS_MODELS_TOOLBAR_PLAN.md`。
本文件记录 **A 段（MM-F：实施 + 自检）** 的真实结果；B 段（GLM-F：复核 + 本地交付）由 B 追加。

---

## A — MM-F：实施 + 自检

执行者：OpenCode `minimax-cn-coding-plan/MiniMax-M3.1-Flash`（interactive）。
分支：`codex/tauri-macos`，版本保持 **0.2.1**。本轮未提交 git commit / push，未运行 release 构建。

### A.1 改了什么（源码）

| 文件 | 改动 |
| --- | --- |
| `frontend/src/state/store.tsx` | `setModelEnabled` 改为真正的 **upsert**：目录里没有 preference 的模型按 catalog 的 `provider_id` 构造完整 preference（`sort_order` 追加到末尾、`is_default=false`、`overrides=null`），经既有 `set_model_preference` 持久化；写失败回滚原状态并提示。同一模型的写按队列串行 + 每请求序号，快速连点不会被迟到结果回滚或持久化旧值。`handleChatEvent` 新增 `usage_updated` 分支 → 重新读取会话账本。`send()` 用“最新 preference 列表”构造请求，避免 provider 查找用陈旧快照。事件订阅改为每次 attach 注册**独立包装函数**，修掉 StrictMode 双挂载时旧 cleanup 误删新 handler 的隐患（此前 mock/happy-dom 下 chat-event 监听会被清空）。 |
| `frontend/src/state/reducer.ts` | `model/enabled` 支持插入新 preference / 删除（回滚）两种形态，并同步模型选择（被移除的模型既不能是 primary 也不能留在 checked；parallel 下 checked 清空时回落到首个已启用模型）。新增 `preferences/loaded`、`boot/loaded` 用 `syncSelection` 统一选择状态：启动时 primary 取默认模型，默认模型不可用时回落到首个已启用模型。 |
| `frontend/src/state/selectors.ts` | 新增 `groupEnabledModels()`（按“已启用 preference”分组，脱离目录的模型以 **ID 兜底**）、`conversationCost()`（会话账本精确求和 + `$0.00` / `$X+` / `$?` 语义）。 |
| `frontend/src/state/types.ts` | 新增 `pickerSearch`（picker 与 Models 面板各自的搜索词）。 |
| `frontend/src/components/ModelsPanel.tsx` | 搜索**真正过滤目录**（`filterModelGroups`），无匹配时给出提示；“My models” 同样按搜索过滤。 |
| `frontend/src/components/ModelPicker.tsx` | 重写为只列 **My models**（enabled preferences）：搜索、单选、并行勾选、默认星标、全选都只作用于该集合；**删除**了行内的 “enabled” 开关（不再能从 picker 启用目录模型）；空集合给出简短提示 + “Manage models” 入口（打开 Models 面板）；脱离目录的模型用 ID 兜底显示。 |
| `frontend/src/components/ConversationTools.tsx`（新） | 右上角新组件：先 **会话金额**、后 **Export**。金额＝当前会话全部 usage records 的已知费用精确求和（十进制字符串，未转浮点）；详情弹层保留 pending / unavailable / tokens；`$0.00`（空会话、已知零）、`$X+`（部分未结算）、`$?`（全未知）、无会话占位 `—`。弹层锚定新入口，Escape / 点外关闭并把焦点还给按钮，切换会话自动关闭。 |
| `frontend/src/App.tsx` | 删除右上角 Models、Settings 两个入口；工具栏换为 `<ConversationTools />`。 |
| `frontend/src/components/Composer.tsx` | 删除输入框里的 Export、Usage 按钮与费用摘要行（金额只在上方工具栏出现一次）；picker 传入 `onManageModels`。 |
| `frontend/src/components/Sidebar.tsx` | 未改逻辑：左下角两个入口保持原按钮，改为同一行（CSS）。侧栏折叠后仍只能用**现有展开按钮**访问，未新增重复入口。 |
| `frontend/src/styles/app.css` | `.sidebar__footer` 改 `flex-direction: row`（两个按钮同行、等宽）；新增 `.toolbar-popover` / `.toolbar__cost`；`.modal__body > *`、`.panel-section > *` 设 `flex: none`，避免弹层内容被压扁（目录底部条目可滚动到达、可点击）。 |
| `frontend/src/i18n/strings.ts` | 新增 `selector.empty/empty_hint/manage_models/my_models`、`toolbar.cost/cost_tooltip/cost_details/no_conversation/cost_partial_hint`、`toast.model_unknown/model_update_failed`。 |
| `frontend/dev/mock-transport.ts` | mock 的 `set_model_preference` 改为 **upsert**（与 Rust `upsert_preference` 一致）；`set_primary_model` 落盘；mock 账本在流结束时写入已知费用并发 `usage_updated`；preferences 存 sessionStorage，使**刷新页面=重启**时偏好仍在（仅模型 id 与参数，无 key、无消息）。 |
| `frontend/dev/geometry.ts` | 弹窗入口定位器由 `.main__toolbar .btn` 改为真实的 `.sidebar__footer button`；新增 8 组检查（见 A.4）。 |

未改动：Rust 后端、账本口径、IPC 协议、费用算法（`sumCosts`/`usageSummary` 复用）、草稿/IME/流式取消/分支逻辑、主题与图标、依赖。

### A.2 回归命令与实际结果

全部在 `frontend/` 下执行（依赖用 `bun install` 装好，未新增任何依赖）：

```
$ bun run typecheck        # tsc --noEmit
（无输出 = 通过）

$ bun test
bun test v1.4.4 (5d9cc0f4b)
 199 pass
 0 fail
 538 expect() calls
Ran 199 tests across 17 files. [11.13s]
（基线为 164 pass / 0 fail；本轮新增 35 个断言用例）

$ bun run build            # tsc --noEmit && vite build
✓ built in 1.07s
dist/assets/index-BtkYtS2o.css   46.94 kB │ gzip:  12.25 kB
dist/assets/index-Dq5ttKNQ.js   714.05 kB │ gzip: 232.95 kB
```

测试文件变更：`tests/layout-ui.test.tsx`（原 `.main__toolbar` 定位器改为真实左下角入口，并新增“入口同行 / 头部无重复入口 / 输入框不再重复 Export 与费用”断言）、`tests/composer-ui.test.tsx`（export 菜单改到工具栏入口）；新增 `tests/models-panel.test.tsx`、`tests/model-picker.test.tsx`、`tests/conversation-tools.test.tsx`。

覆盖到的关键行为（均为**运行真实组件 / 真实 store**的断言，不是源码文字检查）：

- 无 preference 的目录模型加入后：store 里出现该 preference（provider 取自 catalog、sort_order 追加、is_default=false、overrides=null），IPC payload 完全一致（`{enabled:true, sort_order:1, is_default:false, overrides:null}`），后端偏好列表同样更新 → **重新 bootstrap 后仍在 My models**。
- 移除：payload 保留原 `sort_order` / `is_default` / `overrides`（`{enabled:false, sort_order:0, is_default:true, overrides:{temperature:0.2}}`）。
- 写入失败：`state` 回滚（新模型不留残条、已知模型恢复 enabled），toast 为 error 且带原因。
- 快速连点（enable→disable，第一笔写迟到失败）：**不回滚**新选择，且第二笔写在第一笔之后发出、值为最新（`enabled:false`）。
- Picker：只列已启用模型；脱离目录的模型以 ID 显示；空集合提示 + “Manage models” 可打开 Models 面板；搜索 `gamma` 不暴露未启用模型；并行 checkbox 点击 / 行点击 / 键盘 Space **各只切换一次**；“Select all” 只选中已启用集合；picker 内已无 “enabled” 开关。
- 移除主模型后：`modelSelection.primary` 回落到 `vendor/beta`，`send()` 的 target 为 `{provider_id:"p1", model_id:"vendor/beta"}`；零模型时 `send_message` **未发出**且 toast 报错。
- 金额：按会话隔离；已知合计 `$0.02`（0.0108+0.0045 精确求和）；全部分支记录聚合 `$0.06`；部分 pending → `$0.02+`；全未知 → `$?`（**不是** `$0.00`）；已知零 / 空记录 → `$0.00`；切换会话切换金额；`usage_updated` 迟到费用后金额由 `$?` 变 `$0.07`；详情保留 pending / unavailable / tokens。
- Export：三个 scope 仍调用原 `export_markdown_to_file`（main / all_branches / selected+message_ids）。

### A.3 浏览器自检（真实 Chrome，mock harness）

命令：`cd frontend && bun dev/geometry.ts`（真实 Chrome + CDP + 真实鼠标/滚轮/键盘事件，只用 mock fixture，无真实用户数据、无付费 API 请求）。

**最终结果：36/36 通过**（含 1150×680 与 880×520 两档）。关键新增项实际输出：

```
PASS  entries/one-row/1150x680          models top=644 bottom=672 left=12 right=147.5 | settings top=644 bottom=672 left=151.5 right=287
PASS  toolbar/cost-before-export/1150x680 cost right=1053.16 ($0.04) | export left=1057.16
PASS  toolbar/export-popover/1150x680    menu {top:40,bottom:116,left:914,right:1134}
PASS  toolbar/export-popover/escape/1150x680  closed=true
PASS  toolbar/cost-popover/1150x680      text="Known cost $0.04 · pending 0 · unavailable 0 / 4,200 prompt · 1,800 completion tokens"
PASS  toolbar/cost-popover/outside-click/1150x680 closed=true
PASS  entries/one-row/880x520           同上（top=484 bottom=512）
PASS  toolbar/cost-before-export/880x520  cost right=783.16 ($0.04) | export left=787.16
PASS  toolbar/export-popover/880x520     menu {top:40,bottom:116,left:644,right:864}
PASS  toolbar/cost-popover/880x520       popover {top:40,bottom:143,right:864}
PASS  entries/collapsed-reachable        collapsed {sidebarVisible:false, footerVisible:false, headerEntries:0} → expanded {footerVisible:true, models:true, settings:true}
PASS  catalog/bottom-reachable           row={"title":"qwen/qwen3-max","top":529.7,"bottom":556.9,"checked":false,"clickable":true}
PASS  catalog/add-model                  checked=true inMine=true
PASS  catalog/persisted-after-restart    rebooted=true persisted=true
PASS  picker/enabled-only                rows=3（不含未启用的 gemini-2.5-flash），rect 在窗口内
```

原有验收全部保留且仍通过：`shell/initial`、`sidebar/20-cycles`（20 次折叠/展开，主列宽 850..1150、composer 未丢、草稿保留、滚动位置不变）、`composer/880x520|1150x680|1280x800`、`thread/wheel`、`question/expand|collapse`、`actions/placement`、`modal/models|settings|escape|close-button|focus-trap|preserves-chat|selection-after-close`、`controls/single-model|parallel-model`。

覆盖了用户要求的关键交互：**滚到 Models 目录底部实际点击加入**（真实鼠标事件点到 `qwen/qwen3-max` 的复选框 → 进入 My models）、**关闭弹窗后 picker 仅出现所选模型**、**两个底部入口同行**、**右上金额位于 Export 左侧**、**弹层不越界且可关闭**、**输入框仍可用**、**侧栏折叠/展开正常**。

### A.4 harness 新增检查清单（`frontend/dev/geometry.ts`）

1. `entries/one-row/{1150x680,880x520}`：左下角两个入口同一行（top/bottom 对齐、左右不重叠）。
2. `toolbar/cost-before-export/{1150x680,880x520}`：金额在 Export 左侧，金额为该会话精确合计 `$0.04`。
3. `toolbar/export-popover{,-escape}/{…}`：Export 弹层挂在按钮下方、不越界、Escape 可关。
4. `toolbar/cost-popover{,-outside-click}/{…}`：金额详情弹层不越界、点外可关、内容含已知合计与 token 明细。
5. `entries/collapsed-reachable`：折叠后侧栏与两个入口不可见、头部**没有**第二组 Models/Settings；用现有展开按钮展开后两个入口恢复可用。
6. `catalog/bottom-reachable` / `catalog/add-model` / `catalog/persisted-after-restart`：目录滚到底 → 真实点击复选框加入 → 出现在 My models → **刷新页面（=重启）后仍在**。
7. `picker/enabled-only`：picker 只有 3 个已启用模型，未启用的 `gemini-2.5-flash` 不出现，弹层在窗口内。

### A.5 尚存问题 / 需要 B 注意

1. **`usage_updated` 目前靠重新拉取会话快照刷新金额**（`load_conversation`）。IPC 协议本轮不扩展，所以没有更轻的“只读 usage”命令；代价是每次 `usage_updated` 会多一次会话详情往返（每个生成 1~2 次，可接受）。若后续要更省，可在协议里加 `list_usage`。
2. **mock 的 preferences 用 sessionStorage 持久化**（仅 dev harness，`frontend/dev/mock-transport.ts`）。生产 bundle 不含该文件；`tests/composer-ui.test.tsx` 的“浏览器不写 localStorage/sessionStorage”断言针对真实 app，不受影响。若 B 认为 dev mock 不应写 storage，可改为仅内存 + 放弃 `catalog/persisted-after-restart` 这一条浏览器断言（真实持久化已有单元测试覆盖 IPC payload 一致性）。
3. **金额未知时的显示是 `$?`**（不是 `$0.00`）。这是按计划“未知不能伪装为 $0.00”的取舍；若用户更希望看到 `—` 或 `n/a`，属文案级改动。
4. **`modelSelection.primary` 现在在启动时取后端默认模型**（`is_default` 且已启用），此前为 null、由 `resolveSelection` 临时兜底。行为对用户等价（发送目标一致），但 `primary` 字段语义更明确了；B 复核时请确认这一处语义变化可接受。
5. **`preferences/loaded` / `boot/loaded` 会按 enabled 集合裁剪 `modelSelection`**：若后端返回的 preference 列表里某模型被禁用而它正好是用户刚选的 primary，会回落到首个已启用模型（计划要求的行为）。
6. **事件订阅包装函数修复**（`store.tsx`）：StrictMode 下旧 attach 的 cleanup 不再误删新 attach 的 handler。这是行为修复，单元测试与浏览器 harness 均已覆盖（`catalog-changed`、`usage_updated`）。
7. 本轮**没有**动 Rust 端：`set_model_preference` 早已是 `upsert_preference`，前端补齐后端语义一致；费用账本、协议、辅助标题计费 IPC 均未扩展。
8. `frontend/node_modules/`（`bun install` 装的依赖）保留在本地以便 B 复跑测试；`frontend/dist/`（我这次 `bun run build` 的产物）已删除，B 的 `scripts/macos_build.sh` 会重新生成。

### A.6 清理状态

- harness 临时目录 `/private/var/folders/.../T/opencode/aicouncil-ui-A-*`（5 个，含 captures PNG）已全部删除；仓库内无新增 PNG/JPG。
- harness 启动的 Chrome / Vite 进程已退出（`ps` 检查无残留）。
- `frontend/dist/`（本次 build 产物）已删除；`target/`、`gen/` 本轮未生成。
- 未提交 git commit / push，未运行 `scripts/macos_build.sh` / `scripts/macos_release_check.sh`（属 B 段）。
- 未读取、未打印、未上传任何真实 `.env`、凭据、用户数据库或 `dist/data` 内容；全部用 mock fixture 与假 provider。

---

## B — GLM-F：复核 + 本地交付

执行者：OpenCode `zhipuai-coding-plan/glm-5.3-flash`（interactive）。
分支：`codex/tauri-macos`，版本保持 **0.2.1**。本轮未提交 git commit / push。

### B.1 diff 复核结论（逐项核对）

对 A 的全部实际 diff（14 个修改 + 4 个新文件）做了针对性复核，逐项结论：

| 核对项 | 结论 |
| --- | --- |
| preference upsert | ✅ `setModelEnabled` 对无 preference 的目录模型按 catalog `provider_id` 构造完整 preference（`sort_order` 追加、`is_default=false`、`overrides=null`），经既有 `set_model_preference` 持久化；已知模型的 `sort_order` / `is_default` / `overrides` 不被触碰。测试断言的 IPC payload 与后端偏好列表一致性均真实覆盖。 |
| 失败回滚 | ✅ 写失败回滚 state（新模型不留残条 `preference:null`、已知模型恢复 enabled）并 toast 报错；`models-panel.test.tsx` 的 `ModelsBackend` 用真实 `throw` 注入失败，非源码文字检查。 |
| 快速切换 | ✅ 每模型写队列串行 + 每请求序号，迟到失败只允许最新请求回滚；deferred 写真实模拟了 enable→disable、第一笔迟到失败的时序。队列存 `catch` 后的 promise，不会产生未处理 rejection；每笔写的错误由各自的调用方处理，不吞错。 |
| picker 仅 enabled | ✅ 数据源换成 `groupEnabledModels(preferences, catalog)`，搜索/单选/并行勾选/星标/全选都只作用于已启用集合；脱离目录的 preference 以 ID 兜底显示；行内 enabled 开关已删除（无法从 picker 启用目录模型）；空集合有提示 + "Manage models" 入口。 |
| checkbox / 行 / 键盘单次 | ✅ checkbox `tabIndex={-1}` + `onClick` stopPropagation + `onChange` 切换；行 `onClick` / `onKeyDown`（Enter/Space，`preventDefault`）各只触发一次；测试对三条路径各有"只切换一次"断言，浏览器 `controls/parallel-model` 亦通过。 |
| 金额聚合 / unknown / 迟到事件 | ✅ `conversationCost` 复用 `usageSummary`/`sumCosts` 十进制字符串精确求和；语义为 `$0.00`（空/已知零）、`$X+`（部分未结算）、`$?`（全未知，非 $0.00）、`—`（无会话）；记录来自当前会话账本（全分支、全重试），非 API key 总量。mock 的 chat 事件信封含 `conversation_id`，`usage_updated` → `loadConversation` 重读账本，迟到费用 `$?`→`$0.07` 有断言。 |
| 四项布局 | ✅ 右上角 Models/Settings 入口已删（`App.tsx` 换 `<ConversationTools />`）；左下角两按钮 `.sidebar__footer` 改同行等宽；金额在 Export 左侧；Export 弹层锚定新入口、Escape/点外关闭、切会话关闭；Composer 不再重复 Export/Usage/费用；侧栏折叠仍只经现有展开按钮访问。`layout-ui.test.tsx` 定位器改为真实 `.sidebar__footer button`，原验收未删。 |

### B.2 修正（本轮唯一实质问题）

**问题**：`tests/store.test.tsx` 的 `FakeBackend` 缺 `list_preferences` 分支，落入 `default: return undefined`。store 的 `refreshPreferences` 新增防御分支 `if (!Array.isArray(preferences)) return null;` 把这个不完整 mock 的 `undefined` **静默**吸收（`catalog-changed` 测试会经新的 `onCatalog` 处理器触发 `refreshPreferences`）——正是"防御分支为不完整 mock 吞掉真实错误"的形态。

**修法（修 mock 合同 + 消除静默，不弱化测试）**：

1. `tests/store.test.tsx`：`FakeBackend` 补 `list_preferences` → 返回 `fixturePreferences()`（与其 bootstrap 一致的完整契约）。
2. `src/state/store.tsx`：该防御分支保持"不清空列表"的语义（Rust 端 `list_preferences` 恒返回 `Vec`，垃圾响应只能是桥接异常，清空用户模型列表是破坏性的），但改为 `console.warn` 后再返回 `null`——真实异常留痕，未来不完整 mock 也会立刻暴露，不再无声。

其余 diff 未发现问题，未做额外改动。

### B.3 复核后的验证（源码有变，重跑相关验证）

`store.tsx`（新增 warn 分支）与 `store.test.tsx`（补 mock 契约）有改动，重跑：

```
$ cd frontend && bun run typecheck && bun test
tsc --noEmit            # 通过（无输出）
bun test v1.4.4 (5d9cc0f4f)
  199 pass
  0 fail
  538 expect() calls
Ran 199 tests across 17 files. [11.14s]
```

与 A 段结果一致（199/199、538 断言；本次修改不增减断言）。**浏览器 harness 未重跑**：本轮唯一源码改动是"仅当后端答非数组才触发的 warn 分支"，浏览器 mock（`mock-transport.ts`）本就返回真实数组，该分支不可达；`geometry.ts` 及其余源码自 A 的 36/36 通过后未变。无变化的检查不重复。

### B.4 构建与 release check 实际结果

```
$ scripts/macos_build.sh
== 1/4 frontend build (Bun) ==           # bun install --frozen-lockfile + bun run build
== 2/4 cargo build --release ==          # Finished `release` profile in 1m 24s
dist/aicouncil                            # sha256: 5470bad0…c4e60（写入 dist/aicouncil.sha256）
== 3/4 .app bundle ==                    # tauri --bundles app + ad-hoc 重签，codesign --verify 通过
bundle staged: dist/AiCouncil.app         # 20.52 MiB
== 4/4 embedded frontend identity ==
embedded frontend identity OK: raw exe (1 JS bundle(s))
embedded frontend identity OK: bundle binary (1 JS bundle(s))
OK: macos_build

$ scripts/macos_release_check.sh
version: 0.2.1（bundle plist 与 exe 内嵌 AiCouncil/0.2.1 一致）
arch: arm64 ok（app 与 exe）
codesign: adhoc 结构校验通过；无 iced/gtk/winit 遗留链接
native smoke harness absent in release (AICOUNCIL_NATIVE_SMOKE refs: 0)
embedded JS bundle(s): assets/index-b1Kb06-Z.js（与 dist manifest 一致）
RESULT: OK (version 0.2.1)
```

未改版本号（0.2.1）、未改图标、未替换 /Applications 安装、未触碰用户运行中的进程。

### B.5 清理状态

- `target/`（1.3G，含 release 构建与 bundle 中间产物）已删除；`gen/`（tauri schemas）已删除；`frontend/dist/`（本轮 build 中间产物，可由构建脚本再生）已删除。
- `frontend/node_modules/`（本轮/A 由 `bun install` 装入）已删除。
- 无临时脚本/截图/浏览器 profile/日志；opencode 临时目录无 `aicouncil-*` 残留；本轮未启动长驻测试进程。
- `dist/` 只新增交付物（`aicouncil`、`AiCouncil.app`、`*.sha256`、`frontend-embed.sha256`）；用户数据 `dist/data` 与 `dist/pc-data-backup` 原样保留、未读取。
- 未提交 git commit / push；工作区仅余源码、测试与文档改动。

### B.6 对 A.5 遗留项的复核意见

1. `usage_updated` 经 `load_conversation` 刷新金额：本轮不改协议，可接受（每生成 1~2 次往返）。
2. dev mock preferences 用 sessionStorage：保留。仅 dev harness、只存模型 id 与参数，生产 bundle 不含该文件；它支撑 `catalog/persisted-after-restart` 这条真实浏览器断言，值得保留。
3. `$?` 语义：符合"未知不能伪装为 $0.00"，保留。
4. `modelSelection.primary` 启动时取后端默认模型：发送目标与此前等价，语义更明确，接受。
5. 事件订阅包装函数与按 enabled 裁剪 selection：实现与计划一致，接受。
