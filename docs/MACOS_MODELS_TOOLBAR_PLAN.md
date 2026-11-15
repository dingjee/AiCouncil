# Models 与工具栏修复执行计划

日期：2026-10-28。用户要求由 OpenCode 中的模型实施、自检；Codex 主协调仅提供计划、处理阻塞并做最小验收。本计划是本轮任务范围，旧修复计划仅供背景参考。

## 目标与验收标准

1. Models 弹窗：目录中任何可用模型都能加入 / 移出 My models，包括尚无 preference 的模型。即时更新，关闭重开及重新 bootstrap 后仍保留；写入失败恢复原状态并提示。保留已有排序、默认模型和 overrides。搜索必须实际过滤目录，底部条目可滚动到达、可点击。
2. 输入框模型选择器：仅列出 My models（enabled preferences），搜索、单选、并行勾选、默认星标、全选都只能作用于该集合；不得通过它悄悄启用完整目录中的模型。空集合给出简短提示和打开 Models 管理的入口。移除当前模型后发送目标必须回退到有效已启用模型；零模型时阻止发送，不能发送已移除的模型。已启用但暂不在目录的模型应使用 ID 兜底显示，不丢失用户配置。
3. 删除右上角 Models、Settings 两个入口。左下角两个按钮并排同一行，仍分别打开原弹窗。侧栏折叠后仍可通过现有展开按钮访问这两个入口；不增加另一个重复入口。
4. 右上角顺序：会话金额，然后 Export。金额直接显示当前会话的已知费用总和（美元），而不是只显示 Usage。覆盖本会话所有 usage records（包括其他分支、重试），不得变成 API key 总用量或仅当前可见分支。保留详情中的 tokens、pending / unavailable 信息；部分费用未结算时明确标记，未知不能伪装为 $0.00；真正空会话显示 $0.00，无会话用占位。切换会话、usage_updated、迟到费用均正确更新。沿用十进制字符串精确求和，不能转浮点再相加。沿用现有账本口径，不在本轮扩展辅助标题计费 IPC。
5. Export 移至金额后面，保留主分支 / 所选分支 / 全部分支和原生保存、取消、失败行为。输入框不再重复出现 Export、Usage 或费用摘要。弹层定位于新入口，Escape / 点外关闭、焦点行为正常，切换会话关闭旧弹层。

## 已定位的问题与实施建议（须自行验证）

- `frontend/src/state/store.tsx` 的 `setModelEnabled` 在 preference 不存在时直接 return；`model/enabled` reducer 也只更新已有项。应根据 catalog 的 provider_id 构建完整新 preference，通过现有 `set_model_preference` upsert 持久化，并正确同步 / 回滚。不要通过刷新时启用所有目录项来掩盖问题。快速点击同一模型时避免迟到请求回滚新选择或持久化旧值。
- `frontend/src/components/ModelPicker.tsx` 直接 `groupModels(state.catalog, state.preferences)`，所以显示完整目录；并行 checkbox 是 readOnly 且阻止行 click，也应验证直接点击方框能切换一次。筛选到 enabled 集合，同时保留脱离目录 preference 的显示兜底。ModelsPanel 保留完整目录作为管理入口。
- `ModelsPanel.tsx` 搜索框更新 `modelSearch`，但 groups 没有应用搜索。修复即可，不另做状态系统重构。
- 主要改动预计涉及 ModelsPanel、ModelPicker、store/reducer/selectors、App、Sidebar、Composer、app.css、strings；可抽出小型 ConversationTools 组件承载金额 / 导出，避免重复逻辑。只做必要修改，保留草稿、IME、流式取消、分支与弹窗布局。
- 当前 `usageSummary` / `sumCosts` 已提供精确求和，记录来自当前 conversation；新 UI 应复用。不要更改费用账本或协议，不要增加新依赖。

## 执行分工与顺序

### A — MM-F：实施 + 自检

使用 interactive `opencode mini --model minimax-cn-coding-plan/MiniMax-M3.1-Flash`。读取本计划与 AGENTS.md，自行完成全部源码改动和必要回归验证。不要派生其他 agents，不启动发布构建，与 B 串行。结束前写 `docs/MACOS_MODELS_TOOLBAR_RESULT.md` 的 A 段：修复、实际运行命令和结果、浏览器验证结果、尚存问题。不要提交 git commit / push。

必要行为回归（使用现有测试设施，避免重复或仅检查源码文字的测试）：

- catalog 中一个无 preference 的模型，加入后 UI、IPC payload 与重启后的偏好一致；移除正确；持久化失败回滚；已有 overrides / sort_order 保留；快速切换不被迟到结果覆盖。
- picker 显示已启用集合、缺目录兜底、空态；搜索后不露出未启用模型；直接点并行 checkbox / 行 / 键盘各只切换一次；移除主模型后 selection 与发送目标有效。
- 新工具栏金额按会话隔离、聚合所有分支记录，已知 / 混合 pending / 全未知 / 已知零 / 空会话正确；usage_updated 会刷新。导出三种 scope 仍调用原 action。
- 更新原 `.main__toolbar` Models / Settings 测试和 browser geometry harness 的定位器，改为真实左下角入口，不能删除原有效验收。
- `cd frontend && bun run typecheck && bun test && bun run build`。
- 运行真实浏览器 mock harness，至少 880×520、1150×680：滚到 Models 目录底部实际点击加入；关闭弹窗后 picker 仅出现所选模型；两个底部入口同行、右上金额位于 Export 左侧；弹层不越界、可关闭；输入框仍可用，侧栏折叠展开正常。优先扩展并运行现有 `frontend/dev/geometry.ts`，无真实用户数据、无付费 API 请求。证据写文本结果，截图若需要只能临时使用并在结束时删除。

### B — GLM-F：复核 + 本地交付

A 完成后，使用 interactive `opencode mini --model zhipuai-coding-plan/glm-5.3-flash`。读取 A 的结果与实际 diff。只修复本轮已证实的问题；有修改则重跑相关验证；不重复做无变化的全量检查。核对四条用户要求及 A 的测试覆盖，尤其新增 preference、失败回滚、picker 集合、直接 checkbox 点击、unknown 金额语义。

源码与自检通过后，运行 `scripts/macos_build.sh` 更新 `dist/aicouncil` 和 `dist/AiCouncil.app`（该脚本包含 frontend build、`cargo build --release`、复制 exe 与嵌入资源身份检查）；运行 `scripts/macos_release_check.sh`。本轮无需改版本号，保持 0.2.1，不修改图标。使用临时 mock 和隔离数据；不要终止用户正在运行的应用或替换 /Applications 中安装。

写结果文档 B 段，记录复核结论、必要修正、构建和 release check 实际结果。完成后删除本轮 target/、gen/、临时脚本 / 截图 / 浏览器 profile / 日志，停止本轮启动的测试进程；保留源码、文档、正式 assets、dist。不要触碰既有真实用户 data / backups 或其它进程。不要提交 commit / push。

## 共同边界

- 只使用用户授权的 oc 模型；interactive 模式，不用 `opencode run`。每位执行者负责自己的修改和自检，主协调不接管编码。
- 不读取、打印或上传真实 .env、凭据、用户数据库、dist/data 与备份内容；只用假 fixtures / mock providers。本轮不做真实付费发送。
- 不重构无关逻辑、不改主题 / 图标、不新增依赖、不自动升级工具链。不要弱化既有测试。
- 任务结束清理临时产物；截图永不 git add。严守仓库 AGENTS.md。

## 主协调最小验收

检查最终 diff 是否满足四项范围，抽查 preference upsert 与 picker 数据源、金额聚合口径；读 A/B 的真实自检结果，核对 dist 构建与 release check、清理状态。有证据不足或明确失败才退回对应 oc session；不另起全面重做、不对外声称未经验证的行为。
