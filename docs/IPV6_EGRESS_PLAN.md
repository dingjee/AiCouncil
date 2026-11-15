# IPv6 出口监测与拦截修改计划

## 任务与分工

用户要求：将 IPv6 纳入现有监测拦截机制；由 Codex 分析、编写计划及自测要求，交给 OpenCode（oc）实施并自测，Codex 期间最小干预、最后最小验收。未指定模型，按交互模式执行。OpenCode 直接在当前 checkout 工作，保留全部既有未提交改动，不提交、不重置、不改用户网络设置和凭据。

## 问题与证据（2026-10-31，曼谷时间）

- 同一运行时 Key：`GET /key` 200；经本地 HTTP 代理的 Claude 和 Gemini 最小生成均 200，Claude SSE 也正常完成。
- 强制 IPv6 直连，OpenRouter trace 的 `loc=CN`，生成返回 403，原始 metadata 为 `failed_routing_step: Gate Endpoints with Geo Restrictions`。
- IPv4 的 OpenRouter trace 为 US。路由实测 IPv4 走 `utun4`，IPv6 走 `en0`；AiCouncil 运行进程持有该 IPv6 直连连接。
- FlClash 当时 TUN 开启、`ipv6: false`。排查期间系统代理状态曾从开启变为关闭；用户确认可能只开了 TUN。不能断言历史所有请求的代理状态。
- `src/services/egress.rs` 的 `GeoIpGuard` 只有一个自动选路客户端；`detect_sequence` 首个有效源就返回，未逐地址族核验。
- `src/services/network.rs` 的检测与生成客户端独立建连；相同配置不保证相同地址族。现有 3 秒 permit 只关联 revision/TTL，没有被核验网络路径的约束。
- 现有业务门禁仅针对 Anthropic 模型，包含混合批次处理、retry、排队前后复检。此次扩大到 IPv6 路径，不擅自扩展到所有模型，不改变 CN/HK 地区范围。

## 目标行为

1. 将 IPv4 与 IPv6 都纳入有效出网路径的核验，不能仅禁用 IPv6 或永久强制 IPv4 来掩盖问题。
2. 直连/TUN 双栈时，不能凭 IPv4 的 Allowed 签发可让生成任意选择 IPv6 的许可。可用的 IPv6 路径命中 CN/HK 时必须拦截；有可能被生成使用、但无法核验的路径按 Unknown 处理。
3. 无 IPv6 路由/无该地址族 DNS/该分支明确不可能被生成使用，与“有 IPv6 但地理检测失败”必须区分。不能仅因为 IPv6 超时便声称 IPv6 不可用；不得导致普通仅 IPv4 网络或安全代理必然被误拦截。若无法可靠判定某分支不可用，须 Unknown 或在生成连接层明确排除未核验分支，并记录设计边界。
4. 显式或系统 HTTP 代理时遵循真正的代理路径。连接本地 IPv4 代理不代表远端目标必须是 IPv4，不能强行把探测绑到 IPv6 本地地址，导致访问 `127.0.0.1` 代理失败；也不能对不会被生成使用的裸直连做探测后误拦截。代理规则、DNS、地址族约束需与生成保持一致。
5. 保留主检测源 OpenRouter trace 的权威性以及逐源 fallback 语义；IPv6 CN/HK 不得被 IPv4 US 或后备源 US 覆盖。尽量保留两族的检测证据，使阻断提示能说明是哪条路径，而不是仅出现笼统 403。
6. 检测与生成必须关联有效网络策略/地址族约束；permit 不可在策略改变、reload 或排队过期后复用。不要只增加两个 curl 风格探测而让实际生成继续任意选路。对外部系统代理切换与连接池的能力边界如实说明。
7. 保留取消、单源/总预算、响应大小限制、禁止重定向、检测请求不带 Key/用户内容等现有约束；双栈可并行检测，共享总预算，不把超时简单翻倍。
8. 不新增绕过开关，不自动切换 FlClash、系统代理、TUN 或系统 IPv6，不输出凭据，不修改真实对话/数据库，不自动付费实网生成。

## 修改步骤（交给 OpenCode）

1. 阅读 AGENTS.md 与上述涉及模块、`tests/egress_guard.rs`、前后端出口错误接口。先明确最小可行的路径策略与代理处理方案，再实施；必要的设计取舍直接写入本文或结果文档，不需等待 Codex 批准。
2. 在 `network.rs` 提供可核验的地址族/代理策略及相应客户端构造；在 `egress.rs` 实现分支核验、聚合判定与证据。避免在多个模块复制代理解析逻辑。
3. 将已核验路径与发送路径衔接，必要时调整 provider、permit、服务组装与 reload。维持现有 admission、retry 与 dispatch 前检查。保留非 Anthropic 无地理检测依赖的原有语义。
4. 仅在表达 IPv6 原因必需时调整 DTO/UI/文案，避免无关 UI 重构。可复用现有 detail 展示；若改了 IPC 数据，补契约测试。
5. 自测、修复失败、记录实际结果。源码完成后产出可运行 release 到现有 `dist/`（含 `.app` 和 raw exe），遵守项目构建与签名检查。版本如需变更统一变更并执行 AGENTS.md 要求；不要为了回避构建而留下旧 dist。
6. 写 `docs/IPV6_EGRESS_RESULT.md`：最终设计、变更文件、测试命令及结果、未覆盖项/真实边界、产物与清理情况；不能把 mock 测试说成实网验收。保留本文。
7. 测试完成后删除 target、临时脚本/日志/截图与本次新建的依赖目录/中间产物（保留源码、文档、正式 assets 和 dist；不删用户 `.env`、data、备份）。不要提交。

## 必须自测（本地 mock、无真实费用）

| 场景 | 核心断言 |
| --- | --- |
| IPv4 US + IPv6 CN/HK | Blocked，证据明确 IPv6；生成 POST=0；admission 不新增 batch/generation/message |
| IPv4 CN/HK + IPv6 US | Blocked，不被另一分支覆盖 |
| 两族均非受限地区 | Allowed，实际发送连接策略只使用已核验路径 |
| 仅 IPv4 可用 / 仅 IPv6 可用 | 可用路径合法则可发送；无法使用的分支有可靠依据或被发送连接器排除 |
| IPv6 可达但 429/超时/坏响应/缺国家 | 不能当作不存在然后无约束放行；Unknown 或明确排除该发送路径 |
| 主源 IPv6 CN/HK + 备源 US | 主源结论保持 Blocked |
| 本地 IPv4 HTTP 代理，远端返回 IPv4/IPv6 | 检测、生成都走代理；不误探测裸直连、不因本地代理不是 IPv6 而必然失败 |
| 非法代理 | 显式失败，不默默退回直连 |
| 代理/路径策略切换、reload、permit 过期、排队 | 旧许可失效/重新核验；阻断发生在生成 POST 前 |
| 混合批次与 retry | Anthropic 不泄漏；原有非 Anthropic 语义不退化 |
| 取消、总预算、响应上限、重定向 | 保持现有限制，双栈取消立即结束 |
| 敏感数据 | 检测请求不含 Authorization、prompt 或附件；诊断不含 Key |

测试须经过真实生产判定/连接路径，不能只用 StaticGuard 构造 Blocked 冒充验证双栈；使用可注入 resolver/transport/本地 mock 等方式，避免依赖公网或开发机恰好有 IPv6。验收关键是重现原来的“v4 放行、v6 泄漏”并证明生成 POST=0。

建议自测命令（按实际改动增加必要测试，不反复跑无关全量）：

```sh
cargo fmt --all -- --check
cargo test --release --locked --lib --test egress_guard --test request_recovery --test p2_mock_stream
# 若前端/DTO 有变：frontend 下 bun run typecheck，相关 bun test
bash scripts/macos_build.sh
bash scripts/macos_release_check.sh
git diff --check
```

## Codex 最小验收

只审本次增量与结果文档，确认“IPv4 安全、IPv6 受限”经过生产路径且生成 POST=0 的回归测试、代理兼容与无 IPv6 的测试，并执行最小必要的独立检查。优先复用 OpenCode 已完成的测试与构建证据，不接管实现、不重复全量测试。只有实际阻塞/关键验收失败才回交修复；最终检查 dist 与清理情况。
