# 实施状态

按 PLAN.md 阶段推进。每项记录:完成项、验证命令/结果、未解决项、下一步。

## P0 工程骨架与渲染验证 — ✅ 完成(2026-10-17)

### 完成项

- P0.1 Cargo package(aicouncil,lib + bin)、rust-toolchain.toml 固定
  1.99.1、依赖锁定(Cargo.lock 已提交)、最小 CI(.github/workflows/ci.yml,
  ubuntu+windows 的 fmt/clippy/test/release,尚未在 GitHub Actions 实际运行——
  本仓库未推送远端,以本地等价命令验证)。
- P0.2 `examples/render_probe.rs`(真实原生窗口,无网络)+ 
  `tests/fixtures/render_cases.md`(内嵌加载)。
- P0.3 中英混排/代码高亮/表格/链接/滚动/缩放/字体回退验证(截图见
  docs/screenshots/);文字选区为 iced 0.14 固有限制,消息级复制按钮
  在 P3 落地(见 docs/RENDERING.md 已知缺口)。
- P0.4 四类定界符、分式/积分/根式/级数/矩阵/cases/aligned(含
  align→aligned 映射)全部渲染为图形;`tests/math_coverage.rs` 自动化
  断言;`$` 不进入不受控 TeX 执行器(iced_math 纯 Rust 无 JS)。
- P0.5 渲染缓存:公式 SVG 缓存键 =(源文,字号,模式,颜色);文本
  原子测量缓存键 =(字体样式,文本),固有宽度与容器宽度无关;
  无公式段落走 rich_text 快路径,流式仅重建尾部段落。
- P0.6 docs/RENDERING.md 记录版本、架构、验证与缺口。

### 验证命令与结果(本机 Windows 10 19045 / Rust 1.99.1)

```sh
cargo check --all-targets        # 通过,0 警告
cargo test --lib                 # 27 passed; 0 failed
cargo test --test math_coverage  # 3 passed(含 \LaTeX 回退断言)
cargo run --example render_probe # 真实窗口运行,截图核验(见 docs)
```

### 未解决项

- CI 仅具备配置,未在远端运行(无远端仓库)。
- iced 0.14 无文本选区;已知缺口记录于 docs/RENDERING.md。

## P1 领域模型、存储与发送规划 — ✅ 完成(2026-10-17)

### 完成项

- P1.1 领域实体(ids/tree/plan/money)、SQLite 初始迁移(外键、WAL、
  索引)、平台数据目录、临时数据库测试。
- P1.2 Planner(单支/同题多模型/多支追问)+ 分支隔离、重复分支拒绝、
  原样重试、空提示拒绝测试。
- P1.3 配置文件(非敏感,原子写)+ 凭据(keyring 优先,进程内回退,
  探测缓存,不落明文)。
- P1.4 批次单事务落库(失败整体回滚)、拓扑树重载、启动恢复
  (Queued/Connecting/Streaming → Interrupted,幂等)、用量独立状态
  (Pending/Known/Unavailable;幂等入账不回退、不重复计费)。

### 验证

```sh
cargo test --lib             # 62 passed(含 tree/plan/money/credentials/catalog)
cargo test --test p1_lifecycle  # 5 passed(完整分支生命周期/恢复/回滚/S2)
```

## P2 OpenRouter、模型目录与流式调度 — ✅ 完成(2026-10-17)

### 完成项

- P2.1/P2.2 OpenRouter 适配器:`/models`、`/chat/completions`(SSE)、
  `/generations` 补查、`/key` 状态;`/v1` 去重;错误分类(401/402/429/
  模型/上下文/网络/超时);取消令牌;金额全部 Decimal(字符串字面解析,
  无 f64 中间噪声)。
- P2.3 目录缓存(TTL 24h、并发刷新合并、失败保留旧数据)、偏好分离、
  Auto/Override 参数合成(不伪造可选参数;覆盖越界拒绝并上报)、
  有效输出上限(模型上限 × 上下文余量)。
- P2.4 ChatService:semaphore=4、排队可取消、先落库后发请求、
  300ms/4KiB 节流落库、终态只落一次、迟到 usage 入账并转发 UI。
- P2.5 本地 TCP mock(7 字节块切分 + 块间延迟):流隔离、UTF-8 跨块、
  事件边界、取消互不影响(保留部分文本)、401 分类、重复终态、
  目录刷新保留偏好。

### 验证

```sh
cargo test --test p2_mock_stream  # 6 passed
cargo clippy --all-targets -- -D warnings  # 零警告
```

### 未解决项

- 交错测试中两路请求共用同一 mock(ChatService 单 provider);跨
  provider 并行(不同 base_url)在 P4 多模型 UI 联调时补验。
- 真实 OpenRouter 联调待用户提供 Key 后进行(当前全部 mock 验证)。

## P3 极简原生 UI — 🚧 骨架完成(2026-10-17)

### 完成项

- **渲染器决策**:wgpu 在本机远程桌面会话下无法出帧(view 正常调用、
  无 panic、画面全白;`WGPU_BACKEND=dx12/vulkan/gl` 均同),切换为
  **tiny-skia 软件渲染**(iced default-features=false + tiny-skia),
  验证渲染完整正常。对文档型 UI 性能足够;真机 GPU 环境可在
  Cargo.toml 恢复 wgpu feature。
- P3.1 主区(header:导出 MD/全部分支/收起侧栏;消息流滚动区;
  底部输入区:模型选择器/多行编辑器/发送)+ 右侧栏(会话/模型/设置
  三标签,可折叠)——真实窗口截图验证布局正确(docs/screenshots/
  app_main_dpi.png)。
- P3.2 新建会话/搜索框/单层分组(未分组+自定义组)/分组改名删除/
  会话改名删除/移动到分组,UI 与 DB 持久化接通。
- P3.3 模型面板:API Key 输入(secure,保存到凭据库或进程内)、
  测试连接、目录刷新(失败保留缓存)、模型目录勾选/设默认/排序、
  模型搜索。
- P3.4-3.5(部分)发送管线接通:Planner → ChatService.submit →
  broadcast 事件 → UI 按 ID 更新;滚动跟随(mouse_area 检测)。
- P3.6 导出服务(export.rs):Main/Selected/AllBranches 三 scope、
  原子写、文件名清洗,单测通过;UI 导出按钮已接(文件对话框)。
- 启动恢复:遗留未终态 → Interrupted(S1);当前会话 UI 状态持久化。

### 验证

```sh
cargo test               # 80 passed(66 lib + 3 math + 5 p1 + 6 p2)
cargo clippy --all-targets -D warnings  # 零警告
cargo run                # 真实窗口:布局/侧栏/输入区渲染正常(截图)
```

### 未解决项(P3 收尾)

- GUI 自动化点击坐标在 DPI 缩放下难以稳定命中;发送→流式→渲染的
  **端到端手工验收**待做(需真实 Key 或 mock base_url 注入设置项)。
- 中文输入法候选确认 Enter 不误发送(P1 验收矩阵)待真机验证。
- pick_list 在 tiny-skia 下的渲染待确认。

## P3 单支闭环 — ✅ 完成(2026-10-18)

- `AICOUNCIL_BASE_URL` 注入本地 mock;mock_openrouter 示例
  (/models + SSE 流式 + /key + 费用)。
- 发送管线端到端:输入 → Planner → 单事务落库 → SSE 流式 →
  300ms/4KiB 节流落库 → 事件按四元组路由 → 卡片渲染 → 完成后
  main_leaf 指向主回答。
- 侧栏:当前会话高亮/改名/删除/移动分组;输入区:模型选择器。
- 截图:docs/screenshots/p3_single_chat_e2e.png。

## P4 并行与分支 — ✅ 完成(2026-10-18)

- 同题多模型:勾选"同题多模型"发送给全部已启用模型,主模型排第一;
  并行回答并排卡片(mock/beta + mock/alpha 截图
  p4_parallel_cards.png)。
- 多支追问:完成卡片"选此支"复选 → MultiBranch 发送(各支当前
  模型,禁止笛卡尔积);发送后清空多选。
- "从此继续":切换主分支到该完成回答;未完成拒绝(B3)。
- 二段式 E2E(AICOUNCIL_E2E=multi2):4 POST,DB 树验证两支各含
  内容相同 ID 不同的追问 user 节点(B2 隔离);main_leaf 指向主支
  第二轮回答。
- 独立停止/重试:每卡片粒度,取消令牌独立。

## P5 费用 — ✅ 核心完成(2026-10-18)

- 费用标签:输入框右下"本会话已知 $x · N 次待查询 · M 次不可查询";
  Decimal 精确累计(mock 两路 $0.00042×2=$0.00084 截图
  p5_cost_label.png);usage 缓存随 refresh_from_db 刷新。
- 已有:usage 事件幂等入账、重复报告去重、Pending/Known/Unavailable
  状态机、重启恢复待查询记录(`pending_usage`)。
- 待精化(不阻塞验收):预估 300ms 防抖与展示、generation 补查
  2/5/15s 调度接入 UI、展开逐请求详情面板。

## P6 交付 — ✅ Windows 完成(2026-10-18)

- release 构建通过;release 二进制经 mock E2E 验证(发送→流式→落库)。
- README(运行/数据位置/凭据/离线验收/已知限制)。
- 验收矩阵结论:docs/ACCEPTANCE.md(R1-R3/B1-B4/S1-S2/M1-M2/C1-C2/
  E1 通过;P1 部分分;三平台中仅 Windows 验证)。
- CI 配置就绪(未在远端运行);代码签名无证书,标未完成。

## 0.1.1 修正(UI 极简/便携化/图标)— ✅ Windows 完成(2026-10-18)

计划与逐项核验:docs/UI_PORTABLE_PLAN.md(全部检查框已勾选并附核验方式)。

### 修改内容

- 启动:`src/main.rs` 声明 Windows GUI subsystem(双击无 CLI 窗口,
  release exe 实测 PE subsystem=2);数据目录不可写时改为 rfd 可见
  错误对话框,不再静默回退 CWD。
- 数据/凭据:默认根目录 `current_exe().parent()/data`;旧 ProjectDirs
  数据经 SQLite backup API 迁移(WAL 一致快照,测试覆盖未 checkpoint
  记录且源库不变),已有本地库不覆盖;Windows 凭据 DPAPI 加密存
  `data\credentials.dpapi`(从旧 keyring 导入一次),其他平台仅本次
  运行有效,均写入 README。
- UI:消息头部/输入区改为小图标+tooltip(`src/app/icons.rs`,Lucide
  SVG,许可随库);API Key 移到设置;模型面板只留搜索/勾选,价格仅
  tooltip;费用入口收敛为"用量"展开(已知/待查询/不可查询+Key 额度);
  空态居中;分组改名修复(RenameGroupApply 实际落库)。
- 图标:`logo.svg` viewBox 裁到主体,重生成 logo.png 与 7 尺寸 ICO
  (16-256,主体约占画布 90-95%);`build.rs` 跟踪三个图标输入,缺图标
  或嵌入失败直接 panic。
- 窗口:新增最小尺寸 880×520 逻辑像素——低于该宽度侧栏(300)+输入
  工具栏会把发送按钮挤出可视区(验收中发现并定向修正)。

### 检查结果

```sh
cargo fmt --all -- --check                 # 通过
cargo clippy --all-targets -- -D warnings  # 零警告
cargo test                                 # 84 passed(70 lib + 3 math + 5 p1 + 6 p2)
cargo build --release                      # 通过;subsystem=2,图标资源存在
```

### 截图证据(docs/screenshots/,均出自最终交付 exe,隔离数据目录,mock 内容)

- ui011-settings-release.png — 空态主窗口+设置+用量展开(数据目录显示
  exe 同级 data)
- ui011-parallel-release-final.png — 双模型并行卡片
- ui011-models-release.png — 模型面板(无常驻价格/额度)
- ui011-narrow-release.png — 最小尺寸窗口:发送按钮可见、卡片头部按
  组件级换行
- ui011-icons-compare.png — 新旧图标 16/32/256 对比

### 已知观察与未验证项

- 本机远程桌面会话下,刚启动窗口的首帧偶发底部区域未合成(PrintWindow
  捕获可见);移动窗口/后续重绘即恢复,实机待观察,非逻辑缺陷。
- macOS/Linux、真机输入法、真实 OpenRouter 联调仍未验证(见
  docs/ACCEPTANCE.md)。

## P7 附件与并行多选 — ✅ Windows 完成(2026-10-18,0.1.2)

### P7B 文本附件 + 长粘贴转附件

- 领域层:`src/domain/attachment.rs` — `Attachment{name,content}`、
  `compose_prompt`(格式 `{prompt}\n\n===== 附件 N: name =====\n{content}`,
  纯文本拼接,不改 OpenRouter 多模态格式,对全部模型透明)、限额纯函数
  (单文件 ≤256KB / ≤10 个 / 总计 ≤1MB,超限跳过+toast)。
- `Message.attachments` / `Conversation.draft_attachments` 字段;迁移
  `0002_attachments.sql`(messages/conversations 各加 JSON 列,旧库经
  `storage::open` 自动迁移,测试覆盖 0.1.1 旧库路径)。
- 发送管线:`SendPlan.attachments`;`Planner::prepare` 放宽为"prompt 与
  附件均为空才无效";`build_history` / `export::history_of` 对历史 user
  消息组合附件(追问轮上下文可见);`ImmutableRequest::from_planned`
  组合最终 prompt;重试按 `compose_prompt(user.content, attachments)`
  重建;多支追问各 user 节点自动带同一附件。
- UI:输入区"附件"按钮(rfd 多选,常用文本/代码扩展名+所有文件,
  `read_to_string` 天然校验 UTF-8);粘贴 ≥2500 字符(常量,与 Claude.ai
  一致)自动转附件;editor 与 toolbar 之间的 chip 栏(名称/大小/移除);
  chip 随会话草稿保存/恢复,发送后清空;user 消息卡下方只读附件 chip;
  导出在 user 文本后附 `[附件 N: name]` + 全文。

### P7A 并行临时多选

- `parallel_selection: Option<Vec<String>>`(None=跟随模型面板启用集,
  默认;Some=本次临时勾选集);纯函数 `resolve_parallel_targets`
  (None→全部启用;Some→按启用集过滤交集保序;主模型在结果内置首;
  空=无效发送并 toast)。
- UI:勾选"并行"后出现"N 模型"入口,展开面板逐模型复选("主"徽标)+
  "全选 / 跟随面板";在模型面板禁用模型时自动移出临时勾选;
  `AICOUNCIL_E2E=multi` 等既有 E2E 路径不受影响(默认 None)。

### 检查结果与证据

```sh
cargo fmt --all -- --check                 # 通过
cargo clippy --all-targets -- -D warnings  # 零警告
cargo test                                 # 100 passed(85 lib + 3 math + 5 p1 + 7 p2)
cargo build --release                      # 通过;exe 已复制 dist/
```

- 新增测试:compose_prompt/限额(5)、resolve_parallel_targets(3)、
  粘贴阈值分类、附件存储 roundtrip 与 0002 迁移(2)、导出附件、
  E2E `attachments_composed_into_request_prompt`(捕获 mock 请求体,
  断言含附件分隔符与内容、user 节点保存原始 prompt+附件字段)。
- 截图(docs/screenshots/,最终交付 exe,隔离数据目录): 
  ui012-attachments-release.png(附件 chip 栏)、
  ui012-parallel-picker-release.png(并行多选面板展开,含"主"徽标)。

## 下一步

- 真实 OpenRouter 联调(用户提供 Key 后)。
- P5 待精化项(预估防抖/补查调度/详情面板)。
- macOS/Linux 真机验证;输入法真机验收。


## 0.1.3 UI 基础机制整治 — ✅ 完成(2026-10-19)

依据 `docs/UI_FOUNDATION_PLAN.md` 实施(11 项要求收敛为四个基础机制)。
**该 plan 文件已于完成后移除**;本节与 docs/RENDERING.md「可选择文档与
复制投影(2026-10-19)」、docs/ACCEPTANCE.md「0.1.3 验收」为主要过程
与反馈记录,供后续调查接手。

### 交付内容(按 plan 要求编号)

- **1 输入区 / 5.1 Composer**:稳定 `text_editor::Content`(仅切换
  会话/发送后重建,不再逐帧 with_text 重置);min 76px / max
  `min(220, 0.30×窗高)`;显式行高 1.55、Word wrap、独立 widget id。
  叠字根因未现场复现(plan G0-E 未执行),动态高度+显式参数为
  防御性改造。
- **2 模型选择**:删除旧「pick_list + 常驻面板」双入口,新增
  `ModelSelectionState`(parallel/checked/primary)+ 纯函数
  `resolve_selection` + `default_parallel_pair`(当前+最常用,无历史
  按面板顺序);Send disabled 原因/费用预估/SendPlan 共用
  `NormalizedSelection`;primary 星标显式指定;多支追问模式下并行
  切换禁用。
- **3 滚动**:`app::scroll::ScrollController`(Following/Reading/
  Selecting,24/48px 滞回,程序滚动 suppress 计数,每轮至多一次
  操作,会话锚点内存保存);`dirty_stream` 改为 HashSet,一次 tick
  刷新全部脏消息再算一次滚动;scrollable `on_scroll(Viewport)` 按
  实际视口距离判定,删除按 dy 猜测的旧逻辑。
- **4 折叠**:长 user 消息折叠(>320 字符或 >8 行;显示层
  `clip(true)` 固定高度 + 底部 chevron,Expand/Collapse question
  tooltip;per-conversation 展开集合内存态)。实际布局高度测量
  未做(启发式,已知限制)。
- **5 字号**:φ 字级(2.618/2.058/1.618/1.272/1.128/1)+ 正文
  12–22 slider 持久化;标题/正文行高与块间距规则在 view 层落地。
- **6 表格**:空 cell 保留 + 缺列补齐;行高内容化;1px 缝隙线;
  表头浅底;对齐作用于文字。
- **7 宽容 Markdown**:`ParsePolicy::{Strict, LlmCompatible}`;保守
  中文标点 `**…**` 补识别(仅同 part、仅实测组合、复杂度有界、
  歧义反例差分)。
- **8 自动标题**:`migrations/0003_title_metadata.sql`(title_source/
  status/revision/attempt_count + auxiliary_requests 台账(唯一约束
  (conversation,purpose,turn)));字段级 CAS
  (`rename_conversation_title` / `apply_generated_title` /
  `set_fallback_title`,草稿/leaf 更新不再携带旧 title 全量覆盖);
  `providers/deepseek.rs`(显式 env > `.env` 同名变量,endpoint 可
  注入,timeout 12s,thinking disabled,80 tokens,响应清洗≤48
  grapheme,多段/引号/空响应拒绝)+ `services/title.rs`(同会话单
  在途、跨会话信号量 2、成功落库后才触发、失败下次提问再试、
  不自动连续重试、事件按 ConversationId 路由)。真实 API 未联调。
- **9 侧栏宽度**:divider(mouse_area 6px 命中/1px 视觉/水平
  resize 光标/双击复位 300/拖动结束一次性写盘);config
  `sidebar_width` clamp 240–480 且受窗宽约束;非法/NaN 回退默认。
- **10 icon/tooltip/英文/圆角**:`ui_style.rs` 语义 token( Soft Sage
  全 token)+ `design.rs` 控件样式(quiet/accent/active/danger
  button、text_input focus 环、卡片/chip/侧栏/composer/分隔条);
  `i18n.rs` 全量英文资源表(约 130 key,缺 key 回退 key,插值/复数
  单入口);icons 改 token 取色 + 新增 edit/star;全部操作按钮
  icon+英文 tooltip。
- **11 局部选择**:`render/document.rs`(PathSeg 稳定路径 + FlatDoc
  leaf 扁平化 + 投影)、`render/flow.rs`(MathFlow 选择表面:
  hit_test/grapheme 定位/选区高亮/双击/三击/公式原子不可分割)、
  `render/code_flow.rs`(代码 (line,col) 选择)、App 级
  SelectionCoordinator(单消息跨块;Ctrl+C 焦点判定;公式复制 raw
  源文;无选区不清剪贴板)。跨消息拖选未做(已知限制)。

### 冒烟测试发现并修复的问题

- **侧栏首次启动不可见**(0.1.3 发布前发现):boot 清洗后的
  `sidebar_width` 局部变量未写回 `config.window.sidebar_width`,
  新配置默认 0.0 → `Length::Fixed(0)` 隐藏。修复:boot 内直接
  清洗并写回(非法/NaN/越界 → 300)。教训:清洗结果必须落在
  实际消费的字段上,而不是并行的局部变量。
- **当前 RDP 会话下侧栏在屏幕截图不可见**(环境问题,代码已证明
  正确):本机 RDP 会话 Display 虚拟化 DPI 与 System DPI(96)不一致,
  iced 0.14 布局树转储(Operation 遍历)证实 root=1150×680、
  main=844×680、sidebar 节点存在于 850..1150,即**布局正确**;
  但屏幕像素采样显示主区内容绘制越过 844 边界铺满全窗,与布局树
  不符 —— tiny-skia 帧与逻辑布局之间出现了缩放/裁剪错位。对照
  实验:[Fixed,Fill] 行渲染正确,[Fill,Fixed…] 行在本会话不可见
  (纯文本亦可复现)。0.1.2 交付时的会话(150% DPI)无此现象。
  处置:保留标准行结构(与 0.1.2 相同的 [Fill, Fixed] 形态),
  在真实显示器会话重新验收(V01–V11);本会话截图不作 UI 验收
  证据(配色/文案/启动冒烟仍有效)。
- 隔离数据目录启动验证:进程存活、窗口标题正常、Soft Sage 配色与
  英文 UI 生效(截图中主区与空态渲染正常;侧栏部分受上述环境
  问题影响,不作为布局证据)。

### 版本与检查

- **侧栏首次启动不可见**(0.1.3 发布前发现):boot 清洗后的
  `sidebar_width` 局部变量未写回 `config.window.sidebar_width`,
  新配置默认 0.0 → `Length::Fixed(0)` 隐藏。修复:boot 内直接
  清洗并写回(非法/NaN/越界 → 300)。教训:清洗结果必须落在
  实际消费的字段上,而不是并行的局部变量。
- 隔离数据目录启动验证:进程存活、窗口标题正常、Soft Sage 配色与
  英文 UI 生效(截图 ui013-soft-sage-release.png;修复后重截)。

### 版本与检查

- 版本 0.1.2 → **0.1.3**;`cargo build --release` 后复制
  `dist/aicouncil.exe`(SHA256 见 ACCEPTANCE 发布记录)。
- `cargo fmt --all -- --check` / `clippy --all-targets -D warnings` /
  `cargo test`:全绿(146 测试)。

### 移除 plan 文件的说明

`docs/UI_FOUNDATION_PLAN.md` 为一次性实施计划(其第 0 节明确"本轮
交付是本方案"),完成后按用户指示移除;必要过程/反馈已沉淀到:
- docs/RENDERING.md → 渲染架构变更 + 兼容解析 + 表格 + 字级 + 新缺口
- docs/ACCEPTANCE.md → T 编号自动化结果 + 未验证清单
- docs/IMPLEMENTATION_STATUS.md → 本节(交付映射/决策/限制)
- README.md → 用户可见行为(选择器/自动标题/.env/字号/侧栏/英文 UI)

### 已知限制与后续建议(接手者从这里开始)

1. V01–V11 原生 UI 验收未执行(输入叠字现场复现是第一优先)。
2. 跨消息拖选、折叠按实际布局高度、Reading 节点级锚点、随机分块
   fuzz(T10)为渲染层后续工作。
3. 标题 DeepSeek 真实联调(提供 `.env` 后);用量详情面板目前仅
   汇总计数(plan 的"标明标题辅助调用"最小实现)。
4. 并行卡片窄主区纵排逻辑为占位(窗口尺寸已知但布局分支未生效,
   仍并排;需在卡片行处按可用宽度切换 column)。

---

## 0.1.4(2026-10-19):出口门禁 / 请求恢复 / 家族分组选择器

依据 `docs/REQUEST_RECOVERY_MODEL_PICKER_PLAN.md` 实施(计划文件保留)。

### 新增模块

- `src/domain/model_identity.rs`:开发商归属(Anthropic 判定覆盖全部
  `anthropic/*` 与受控 `claude-*` 别名,不受显示名影响)+ 家族分组
  展示(`presentation`:组标题/短名/完整名;`:free` 等变体保留)。
- `src/domain/recovery.rs`:FailureInfo(版本化 JSON,未知降级)、
  DispatchPhase、RetryDecision(资格/冷却/补救)、退避 2/4/8…30s。
- `src/services/egress.rs`:GeoIP 门禁(ip.sb 主源 → ipapi.co 备源;
  CN/HK 拦截、其余有效国家放行、Unknown 拦截;64KiB 上限、UA、
  8s 预算;许可 TTL 3s + network_revision)。
- `src/services/network.rs`:统一网络配置(生成:连接 15s/读取停滞
  120s;检测与生成同配置,显式代理 `AICOUNCIL_PROXY`,失败不静默直连)。
- `src/app/model_picker.rs`:分组 view model(组序=enabled 首现;搜索
  匹配家族/短名/完整名/原名/raw ID)。
- `migrations/0004_request_recovery.sql`:failure_json /
  request_snapshot_json / retry_root_id / retry_not_before_ms /
  dispatch_phase + lineage 唯一活动部分索引 + root backfill。
- `scripts/winshot.ps1`:窗口截图工具(自动移到副屏)。

### 行为变更(要点)

- 发送流:预检(admission)→ 单事务落库(含 intended main_leaf CAS)
  → dispatch(许可过期重检)→ 终态 CAS;回执按 DB 权威状态对账。
  预检/落库失败零回滚(不插树/不清草稿/不动 leaf)。
- 失败恢复:终态失败不阻塞新轮次;空失败占位不进模型上下文,部分
  输出保留;EOF 无成功终态判失败(不再保守写 Completed);错误帧
  (顶层 error / choice.error / finish_reason=error)一律失败。
- 重试:同 user 下兄弟节点;lineage 唯一活动约束(索引兜底)+ 冷却
  (Retry-After/指数退避,持久化绝对时间)+ UI 防重;旧尝试默认隐藏
  (Attempt N 徽标,可展开);默认沿用原快照(模型/参数/上下文)。
- 门禁弹窗:Blocked/Unknown 单弹窗;重新检测(不自动发送)/取消/
  混合目标"移除 Anthropic 模型后发送";Esc 关闭;说明覆盖全部
  Anthropic 模型(不只 Claude)。
- 模型选择器:家族分组(组标题 10px muted 非交互)+ 搜索框 +
  自适应高度(150–300px);消息卡/侧栏使用归一化名,raw ID 进 tooltip;
  选择/持久化/计费仍用原 ID。
- 状态 view model:`status_of` 从 generations 权威状态构建(历史/
  重启/晚完成失败均显示);broadcast lag 触发节流 DB 对账。

### 验证

- `cargo fmt --check` ✅;`cargo clippy --all-targets -D warnings` ✅;
  `cargo test --all-targets`:201 通过 / 0 失败
  (lib 151 + egress_guard 9 + request_recovery 12 + p2 15 + 其余)。
- 验收矩阵(A/B/C/D)与证据:见 docs/ACCEPTANCE.md 0.1.4 节。
- GUI(dist exe + 隔离数据 + 本地 mock):model-picker-groups /
  anthropic-egress-unknown-modal(零生成 POST)/
  request-recovery-success-card / request-recovery-failed-card。
- `dist/aicouncil.exe` 0.1.4 SHA256:
  DCD7CED1BDA70F30182D66ECBA84A8030727E00B7490CAE6F87EC303C22E4B67

### 已知限制(本轮边界)

1. 出口一致性边界:GeoIP 查到的是检测服务出口;按域名分流/双栈/
   代理动态换出口仍可能与上游不一致(计划第 3 节明示,不宣称保证)。
   可用 `AICOUNCIL_PROXY` 统一固定出口。
2. 真实代理双栈行为未在线上核验(A11 记为未运行;仅代码路径与
   单元覆盖);真实 401/402/429 账户语义未用真实 Key 验证。
3. D06/D07 键盘导航与 DPI 矩阵、最小窗口布局未逐项截图验证;
   选择器键盘行为依赖 Iced 自绘列表(上下键/Enter/Esc)未显式断言。
4. 弹窗为"替代输入区"形态(非遮罩 modal);多显示器 DPI 混合下
   截图脚本按物理像素裁剪。
5. B14 广播积压对账为节流全量刷新;C08 keepalive 10 分钟无进展
   上限未做时序测试(常量可调)。
