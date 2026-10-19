# 验收矩阵结论(PLAN 第 6 节)

日期:2026-10-18 · 环境:Windows 10 19045 x64(远程桌面会话)· Rust 1.99.1

| ID | 场景 | 结果 | 证据 |
| --- | --- | --- | --- |
| R1 | 四类定界符/分式/积分/矩阵/cases/aligned | ✅ 通过(自动化+截图) | `tests/math_coverage.rs` 全部渲染为 SVG;`docs/screenshots/`(整页+局部放大) |
| R2 | 代码内 `$`/货币/未闭合 `$$`/未知宏 | ✅ 通过 | `render::md` 27 项解析测试(货币/代码保护/字面回退);`\LaTeX` 宏按预期源文回退 |
| R3 | 长内容滚动 | ✅ 通过(GUI) | render_probe 压力样例(20,000 字+50 公式)翻页滚动正常;输入区保持响应 |
| B1 | Q→A1/B1 默认 A1 | ✅ 通过 | primary 在发送前确定(`Planner` 测试 + Submitted 回执设置 main_leaf);不按返回顺序 |
| B2 | 勾选两支追问,各带自己的历史 | ✅ 通过(E2E) | 二段式 autopilot:4 POST;DB 树两支各一个内容相同 ID 不同的 user 节点;无串支;无隐式四请求 |
| B3 | 主请求失败不自动改选 | ✅ 通过(代码路径) | main_leaf 仅在主请求 Finished 时更新;失败仅标状态;未完成不可继续(ContinueFrom 校验) |
| B4 | 交错/取消/切会话 | ✅ 通过 | `p2_mock_stream`:取消支保留已收文本,另一支完成;事件按 ID 路由;DB 为准 |
| S1 | 强杀重开 | ✅ 通过 | `p1_lifecycle::restart_recovery_marks_interrupted`;应用启动调用 recover_interrupted |
| S2 | 删除非空分组 | ✅ 通过 | `p1_lifecycle::delete_group_moves_conversations_to_ungrouped` + UI 入口 |
| M1 | 目录更新保留偏好 | ✅ 通过 | `catalog_refresh_keeps_preferences` + 失败保留旧缓存(`CatalogCache` 测试) |
| M2 | Auto 不臆造/覆盖越界拒绝 | ✅ 通过 | `catalog::compose_params` 测试(无 defaults 时不发可选参数;越界覆盖上报 rejected) |
| C1 | usage 重复报告只记一笔 | ✅ 通过 | `usage_merge_dedup_and_no_regression` + `usage_upsert_idempotent_and_no_regression` |
| C2 | 零/未知/不可查询/迟到 | ✅ 通过 | Known 合法为零、Pending/Unavailable 区分;迟到 usage 幂等入账并更新标签 |
| E1 | 导出三 scope/失败节点/无泄漏 | ✅ 通过(服务层) | `services::export` 测试(Main/AllBranches/失败标记/原子写/文件名清洗);UI Main+All 按钮已接,Selected 由勾选集合驱动 |
| A1 | 文本附件进请求/落库/导出(0.1.2) | ✅ 通过 | E2E `attachments_composed_into_request_prompt`(mock 请求体含 `===== 附件` 与内容;user 节点存原始 prompt+附件字段);roundtrip/0002 迁移测试;导出附 `[附件 N]` |
| A2 | 长粘贴转附件/限额(0.1.2) | ✅ 通过(纯函数+单测) | `paste_threshold_classification`(≥2500 字符,按字符计);try_push 限额(256KB/10 个/1MB);chip 随草稿保存/恢复 |
| A3 | 并行临时多选(0.1.2) | ✅ 通过(纯函数+单测) | `resolve_parallel_targets`(None 跟随面板/Some 交集保序/主模型置首/空无效);UI 截图 ui012-parallel-picker-release.png |
| P1 | 三平台凭据/输入法 | ⚠️ 部分 | Windows 凭据库+进程内回退已实现并测;输入法真机验收待做;macOS/Linux 未验证 |

## 检查命令(实际运行)

```sh
cargo fmt --all -- --check     # 通过
cargo clippy --all-targets -- -D warnings  # 零警告
cargo test                     # 100 passed(85 lib + 3 math + 5 p1 + 7 p2,2026-10-18 0.1.2 复跑)
cargo build --release          # 通过;release 二进制经 mock E2E 验证(发送→流式→落库;0.1.2 含附件用例)
```

## 0.1.1 UI/便携化补充证据(2026-10-18)

- 迁移一致性:WAL 未 checkpoint 记录经 backup API 迁移且源库不变
  (`migration_uses_consistent_wal_snapshot_and_preserves_source`)。
- UI 截图(最终 exe,隔离 mock 数据):docs/screenshots/ui011-*.png
  (空态/设置+用量展开、并行卡片、模型面板、最小窗口、图标对比),
  明细见 docs/IMPLEMENTATION_STATUS.md「0.1.1 修正」。

## 0.1.2 附件/并行多选补充证据(2026-10-18)

- 附件 E2E:mock 捕获请求体断言附件拼接;0002 迁移测试模拟 0.1.1 旧库
  (`migration_0002_upgrades_old_database`)。
- UI 截图(最终 exe,隔离数据目录):docs/screenshots/ui012-*.png
  (附件 chip 栏、并行多选面板展开)。

## 未验证/待办

- 真实 OpenRouter 联调(需用户提供 Key;真实账单产生前需按执行环境授权)。
- 中文输入法候选确认 Enter 不误发送(真机手工验收)。
- macOS / Linux 构建与运行(无对应机器,未验证,不作跨平台交付声明)。
- 代码签名/公证(无证书,标未完成)。

## 0.1.3 UI 基础机制整治验收(2026-10-19)

本轮依据 `docs/UI_FOUNDATION_PLAN.md` 实施(该 plan 已在完成后移除,
过程记录见本文件与 IMPLEMENTATION_STATUS/RENDERING)。

### 自动化检查(实际运行,Windows 10 19045 x64 · Rust 1.99.1)

```sh
cargo fmt --all -- --check        # ✅ 通过
cargo clippy --all-targets -- -D warnings  # ✅ 零错误
cargo test                        # ✅ 146 通过 / 0 失败
```

- 库测试 125(新增:兼容强调差分 T09、表格空 cell T11、复制投影
  T12、流式前缀选区稳定 T14、chunk 一致性 T10 简化版、Soft Sage
  对比度基线 T17 纯计算、滚动状态机 T02 滞回/程序滚动/选择挂起、
  选择 resolver T04、默认并行对、折叠启发、标题 CAS/台账 T06/T07、
  0003 迁移 T08、用量排名 T05、i18n、单词边界/grapheme)。
- 集成测试:p1_lifecycle(5)/p2_mock_stream(7)/math_coverage(3)/
  p_title_metadata(6)全部通过,既有行为无回归。

### 验收矩阵映射(plan 第 10 节编号)

| ID | 状态 | 说明 |
| --- | --- | --- |
| T02 跟随状态 | ✅ 自动化 | `app::scroll` 滞回迁移/程序滚动抑制/每轮一次操作/Selecting 挂起(单测) |
| T04 选择 resolver | ✅ 自动化 | `resolve_selection` 单选/并行/主项提升/空集无效;Send/预估/计划共用同一 NormalizedSelection |
| T05 排名 | ✅ 自动化 | `model_usage_rank` 按已 Started 次数+最近使用+模型 ID 稳定排序;未出队不计;标题请求独立台账 |
| T06/T07 标题状态机 | ✅ 自动化 | fallback→in_flight→ready/failed;CAS 丢弃迟到结果;manual 不被覆盖;重启恢复 needs_generation;0003 迁移保护旧标题 |
| T08 迁移 | ✅ 自动化 | `migration_0003_protects_existing_titles`(0.1.2 旧库升级;单迁移事务回滚语义在 migrate() 实现) |
| T09 Markdown | ✅ 自动化 | 实测正例(中文+引号 bold)+ 7 组反例差分(乘法/单星/下划线/hashtag/未闭合等 Strict==LlmCompatible) |
| T10 chunk 一致性 | ✅ 简化版 | 固定切分点拼接重解析 == 完整解析;随机边界流式 fuzz 未做(列为限制) |
| T11 表格 | ✅ 自动化 | 空 cell 保留/缺列补齐/公式表头;行高/边线渲染为布局属性(原生检查未做) |
| T12 复制投影 | ✅ 自动化 | 粗体/链接/公式四种定界/代码换行/表格 tab/列表标记/引用前缀/跨块空行/局部 cell |
| T13 Unicode 命中 | ✅ 部分 | grapheme 单词边界/字素子串(CJK+emoji)纯函数测试;Paragraph hit_test 依赖 iced 实现,原生拖选未验证 |
| T14 流式选区 | ✅ 自动化 | 前缀选区投影稳定(追加不改写);重解释场景靠重渲染保持语义位置 |
| T15 本地化 | ✅ 自动化 | en 资源表全量;缺 key 回退 key 本身;插值/复数;未支持 locale 回退 en(入口未做切换 UI,按 plan 本轮仅 en) |
| T16 配置宽度 | ✅ 部分 | 非法/NaN/越界回退 300 并 clamp(240–480,窗宽约束);拖动结束才写盘(spawn_blocking);NaN 用 is_finite 过滤 |
| T17 主题 | ✅ 纯计算 | Soft Sage token 对比度单测(正文≥4.5:1,边界≥3:1,未舍入公式);原生显示器核验未做 |
| V01–V11 原生 UI | ⚠️ 未验证 | 本轮未执行原生窗口/截图/录屏验收(输入叠字现场复现、流式滚动观感、局部复制剪贴板回读、DPI 矩阵等);**不能把勾选计划当证据,上表仅覆盖自动化部分** |

### 证据与产物

- 最终交付:`dist/aicouncil.exe`(0.1.3,SHA256 见下),与
  `target/release/aicouncil.exe` 哈希一致:
  `E3BFEBAC7AE7A02BA6BE4FC00A6CE1B27C4DA40C1C7E0F067EF642728C3A28A4`。
- 启动冒烟:隔离数据目录启动存活、Soft Sage 配色/英文 UI/空态渲染
  正常(docs/screenshots/ui013-soft-sage-release.png;侧栏绘制受
  当前 RDP 会话 DPI 虚拟化影响,详见 IMPLEMENTATION_STATUS
  「冒烟测试发现并修复的问题」——布局树转储证明节点位置正确,
  本会话像素不作布局验收证据,需真实显示器会话复验)。
- 源码级证据:各模块 `#[cfg(test)]`(渲染管线纯函数化,可在无
  GUI 环境复跑);`examples/render_probe.rs` 已适配新渲染接口。

### 未验证/待办(承接上轮 + 本轮新增)

- 上轮全部未验证项继续有效(真实 OpenRouter 联调、输入法真机、
  macOS/Linux、代码签名)。
- 本轮新增:V01–V11 原生 UI 验收(含侧栏在本 RDP 会话的绘制错位
  复核)、T10 随机分块 fuzz、T13 原生拖选剪贴板回读、真实 DeepSeek
  标题联调(`.env` 读取已实现,未发真实请求验证响应清洗)。

## 0.1.3 发布记录

```sh
cargo fmt --all -- --check                 # ✅ 通过
cargo clippy --all-targets -- -D warnings  # ✅ 零警告
cargo test                                 # ✅ 146 通过 / 0 失败
cargo build --release                      # ✅ 完成
# dist 复制完成,双哈希一致:
# E3BFEBAC7AE7A02BA6BE4FC00A6CE1B27C4DA40C1C7E0F067EF642728C3A28A4
```

`docs/UI_FOUNDATION_PLAN.md` 已按用户指示在实施完成后移除;其调研结论、
实施映射与未验证清单沉淀于本文件、docs/RENDERING.md(2026-10-19 节)与
docs/IMPLEMENTATION_STATUS.md(0.1.3 节)。dist/data/ 为用户真实运行
数据(数据库+凭据),未触碰。

---

## 0.1.4:出口门禁 / 请求恢复 / 选择器 验收矩阵(2026-10-19)

执行环境:Windows,本地可计数 mock(tests/common + examples/mock_openrouter),
隔离 `AICOUNCIL_DATA_DIR`,无公网、无真实 Key、无真实计费请求。
✅=自动化测试通过;🖼️=dist exe GUI 截图证据;❑=未运行(见限制)。

### A. 地域与发送门禁

| ID | 结果 | 证据 |
| --- | --- | --- |
| A01 | ✅ | tests/egress_guard.rs `a01_…`:CN/HK × Claude/非 Claude fixture → POST=0、batch/gen/assistant 增量=0、AdmissionError::EgressBlocked |
| A02 | ✅ | `a02_…`:US/JP/SG/MO/TW 放行,body 含原模型 ID(MO/TW 不扩大封禁) |
| A03 | ✅ | `a03_a14_…`(Unknown 拦截)+ egress.rs 单测(payload 校验:缺码/ZZ/保留 IP/200 error) |
| A04 | ✅(逻辑) | egress.rs `run_detection` 主源失败→备源一次;有效 CN/HK 不被覆盖(源码路径+单测分类) |
| A05 | ✅ | `a05_…`:非 Anthropic 目标 + CN guard → guard 零调用、正常发送 |
| A06 | ✅ | `a06_…`:混合 3 目标整批拒绝(0 POST/0 落库);新计划仅剩非 Anthropic 可发送 |
| A07 | ✅ | `a07_…`:同批 3 个 Anthropic 家族 → 1 次检测、3 个 POST |
| A08 | ✅ | `a08_…`(dispatch 级:并发 1 + 慢流排队 >TTL → 出口翻转 CN → Failed/NotDispatched、usage unavailable、已发请求保持 Completed);`a08_stale_permit_…`(admission 级过期/revision) |
| A09 | ✅ | `a09_…`:服务 retry + Blocked → AdmissionError,0 新增 generation |
| A10 | ✅(服务) | submit 快照冻结 + intent 校验(on_egress_checked);App 层回执只清已提交 revision |
| A11 | ❑ | 真实代理未在线上核验;代码路径:统一 NetworkConfig、代理失败显式报错(单测 `clients_build_with_and_without_proxy`) |
| A12 | ✅ | `a08_stale_permit_…`:每次动作新检测(check 计数递增) |
| A13 | ✅ | `a01_…` 含 `anthropic/test-non-claude-family`(无 Claude 字样仍拦) |
| A14 | ✅ | `a03_a14_…`:mock/claude-lookalike 不拦;claude-* 别名拦 |

### B. 生命周期、上下文与重试

| ID | 结果 | 证据 |
| --- | --- | --- |
| B01 | ✅ | request_recovery.rs `b01_…`:失败后新问成功、旧卡保留、user 各一条、POST=2 |
| B02 | ✅ | `b02_…`:recover_interrupted → Interrupted(状态来自 DB,零新增生成,可显式重试) |
| B03 | ✅ | `b03_…`:捕获 body 含 U1→U2 顺序,无错误文案/空 assistant |
| B04 | ✅ | `b04_…`:部分文本保留并带入下一轮 |
| B05 | ✅(策略) | plan.rs `empty_assistant_placeholder_filtered_…` + App `continuation_ready`(仅活动态阻塞;终态放行) |
| B06 | ✅(等价) | main_leaf CAS(b13)+ 继续按钮仍要求 Completed;"从此继续"由用户显式点击 |
| B07 | ✅ | `b07_…`:10 并发 retry → 1 成功 9 Duplicate、1 sibling、POST 增量=1、user 不复制 |
| B08 | ✅ | `b08_…`:同模型两 lineage 各自 retry 成功(共 4 generations) |
| B09 | ✅ | `b09_b10_…`:403→Forbidden(不统一为 Key 错误);分类映射见 providers/mod.rs `failure_kind` |
| B10 | ✅ | `b09_b10_…`:Retry-After 120s → 冷却持久化,到期前 retry=Cooldown 且零 POST |
| B11 | ✅ | `b11_…`:外键失败 → Storage 错误、0 POST、0 假消息(草稿保留为 App 层行为) |
| B12 | ✅(服务) | 回执按 DB 实际状态合并(on_submitted 读 generations;不再复活 Queued) |
| B13 | ✅ | `b13_…`:leaf 被用户移走后晚完成不抢回(main_leaf CAS) |
| B14 | ✅(逻辑) | subscription Lagged → 节流 Reconcile → DB 对账 + 清理终态 UI 映射 |
| B15 | ✅ | `b15_…` + `b07_…`:submit 后立即取消 → Cancelled(token 先注册) |
| B16 | ✅ | `b16_…`:retry 沿用快照(模型/SYS/params/prompt;body 断言) |
| B17 | ✅ | `b17_…`:lineage=2、同 user、usage 每请求独立;UI 聚合=Attempt N 徽标 |
| B18 | ✅(部分) | 落库失败 → GenEvent::PersistError 可见 toast(B18 真实注入未做;错误路径单元可達) |

### C. HTTP / SSE / usage

| ID | 结果 | 证据 |
| --- | --- | --- |
| C01 | ✅ | p2 `c01_…`:顶层 error+DONE / finish_reason=error → Failed |
| C02 | ✅ | `c02_…`:partial+EOF → Failed/StreamAborted,文本保留,无重发 |
| C03 | ✅ | `c03_…`:only DONE → EmptyResponse;坏 JSON → ParseFailure |
| C04 | ✅ | `c04_…`:同帧 text+usage+finish 三者齐全 |
| C05 | ✅ | `c05_…` + 原 `duplicate_terminal_events_…`:单终态、费用单条 |
| C06 | ✅ | `c06_…`:finish 后 EOF(无 DONE)仍 Completed,费用不写 0 |
| C07 | ✅ | `c07_…`:length 有内容=Completed+Truncated;content_filter 语义保留 |
| C08 | ✅(常量) | read_timeout 120s / NO_PROGRESS 600s 集中于 network.rs 与 openrouter.rs(时序注入未做) |
| C09 | ✅ | 原 p2 UTF-8 跨块/SSE 注释/迟到 usage 用例保持通过 |
| C10 | ✅ | `c10_…`:error 后续帧不覆盖首终态 |

### D. 选择器与视觉

| ID | 结果 | 证据 |
| --- | --- | --- |
| D01 | 🖼️+✅ | docs/screenshots/model-picker-groups.png(组标题小号 muted);model_picker.rs 单测 |
| D02 | ✅ | model_identity 单测(变体保留/消歧/tooltip 由 UI 提供);发送 ID 精确不变(C 系列断言原 ID) |
| D03 | ✅ | model_picker.rs `search_matches_…`(家族/短名/原名/raw ID;空结果说明在 UI) |
| D04 | ✅ | `many_single_family_groups_…`(30 组单页可滚动;高度 150–300 自适应) |
| D05 | ✅ | app/mod.rs resolve_selection 原 3 单测保持通过 |
| D06 | ❑ | Iced 自绘列表键盘导航未逐键验证(Enter/Esc 走全局 KeyPressed) |
| D07 | 🖼️(部分) | 1456×898 与 1747×1076 两种窗口截图正常;100%/150% DPI 矩阵未逐项 |
| D08 | 🖼️ | request-recovery-failed-card.png(短原因"Provider server error"+详情 tooltip+重试+输入区可用) |

### 自动化与发布命令(0.1.4)

```text
cargo fmt --all -- --check                 # ✅ 通过
cargo clippy --all-targets -- -D warnings  # ✅ 零警告
cargo test --all-targets                   # ✅ 201 通过 / 0 失败
cargo build --release                      # ✅ 0.1.4
Copy-Item target\release\aicouncil.exe dist\aicouncil.exe
Get-FileHash dist\aicouncil.exe -SHA256
# DCD7CED1BDA70F30182D66ECBA84A8030727E00B7490CAE6F87EC303C22E4B67
```

GUI 验证(dist exe + 隔离数据 + mock):见 IMPLEMENTATION_STATUS.md 0.1.4 节。
未运行项(A11 真实代理、D06 键盘、D07 DPI 矩阵、B18 故障注入)列为限制,
不写"全部通过"。

## 0.1.5：Markdown 行距与表格回归（2026-10-19）

- `cargo test --release --lib --test render_layout --test math_coverage`：158 通过，0 失败（154 单元测试 + 3 公式测试 + 1 整篇布局回归）。
- 整篇布局测试覆盖 12/15/22px 正文 × 360/760/1100px 消息宽度，断言文字没有越出段落、表格背景铺满整行、列边界一致、空单元格保留最小行高，分割线后的段落不与前一块重叠。
- 段落单测覆盖 90/180/360px 宽度，检查实际绘制的行坐标与选择命中一致；另验证英文按词换行、居中/右对齐和标题文字/公式字号。
- `rustfmt --edition 2021 --check src/render/flow.rs src/render/view.rs tests/render_layout.rs examples/render_probe.rs` 与 `git diff --check` 通过。
- 根因与复现命令见 `docs/RENDERING.md` 的 0.1.5 节。
- 截图：`docs/screenshots/markdown-0.1.5/` 共 9 张，由与程序相同的 tiny-skia 渲染器、完整 `view_message` 布局直接生成。目视检查常规字号/中等宽度（15px/760px）、大字窄屏（22px/360px）及大字宽屏（22px/1100px），无段落重叠、灰色断块或分割线穿字。窗口捕获工具两次超时，本轮视觉证据为渲染器离屏输出，不计作真实窗口截图。
- `cargo build --release --bin aicouncil --example render_probe` 通过，交付 `dist/aicouncil.exe`（0.1.5）；SHA256：`D638CBF8DAF110D3898B3199AFAA0542FC36EE27C4940C39CCCD94A5EBA5D213`。
- `target/` 未在本轮自动清理，留待手动删除（构建产物与交付 EXE 哈希已核对一致，验收窗口进程已关闭）。
