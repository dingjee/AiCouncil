# IPv6 出口门禁实施结果(IPV6_EGRESS_PLAN 实施)

日期:2026-10-31(曼谷时间)。实施:OpenCode(GLM)。上游分析与计划:`docs/IPV6_EGRESS_PLAN.md`(保留未动)。
本文为最终实现、变更、测试证据、真实边界与清理记录。所有测试为本地 mock / 只读实网探测,无真实生成、无费用、无网络设置或凭据改动;未提交、未重置。

## 一句话结论

原缺陷"IPv4 US 放行许可 + 生成任意选路经 IPv6 CN 泄漏(403)"已在生产判定/连接路径上修复并回归:许可现在携带**已核验地址族集合**,发送连接器对未核验族物理不可达;实网探测(本机当前即 IPv4 US + IPv6 CN)生产判定为 **Blocked**、证据明确指向 v6 路径;本地 IPv4 HTTP 代理、无 IPv6 网络均不受误拦。dist 已按 AGENTS.md 重建并通过 release 检查。

## 最终设计

### 1. 地址族连接约束(`src/services/network.rs`)

- 新增 `RouteFamilies { v4, v6 }`(DUAL / V4_ONLY / V6_ONLY):检测与生成**共用**的连接约束类型。
- 新增 `FamilyFilterResolver`(实现 `reqwest::dns::Resolve`):先走系统 getaddrinfo(与默认 GaiResolver 同源,遵循 /etc/hosts),再按允许族过滤;过滤后为空显式报错(该族"解析层面不可用"),不静默换族。
- `NetworkConfig::detection_client_for / generation_client_for(families)`:与原有单参构造同一路径(同一代理解析、同一超时),仅叠加族过滤;`DUAL` 保持默认解析器(行为与旧版完全一致)。无逻辑复制。
- **按构造一致**:生成客户端与同族检测客户端使用同一过滤,因此"许可授权的族 = 实际发送可用族"不靠事后断言。

### 2. 双族并行检测与聚合(`src/services/egress.rs`)

- `GeoIpGuard` 持 v4/v6 两个限族检测客户端 + 每族独立检测源序列(生产:同一组 URL,族由客户端解析约束决定)。
- `run_detection`:两族并行,各自独立走"OpenRouter trace 主源 → ipapi.co → ip.sb"序列(族内主源权威、备源逐级降级语义不变);共享**同一总预算截止期**(默认 8s,不翻倍);取消立即结束两族。
- **截止期只作用于未完成分支**:每族 future 单独与截止期竞争(`join` + 每族 `sleep_until(deadline)`),先完成的族结论一律保留;截至时未完成分支记为 Unknown("…分支未完成")。(Codex 阻塞①修复,见"测试-反证"。)
- `aggregate` 聚合:
  - 任一族 Blocked(CN/HK)→ 整体 Blocked,obs 为该族证据(两族同时受限优先呈现 v6),note 保留另一族状态;
  - 否则任一族 Allowed → 整体 Allowed,`families` = 已核验族并集;未核验族的原因写入 note("… 未核验,已从发送连接排除"),**不是当作不存在无约束放行**;
  - 两族都无结论 → Unknown(两族原因并列)。
- `EgressObservation` 增加 `families`、`note`;`evidence()` 含 `families=v4/v6/v4+v6` 与 note,阻断提示能说明是哪条路径。

### 3. 许可携带族 + 发送连接器排除未核验族

- `EgressPermit` 增加 `families`;admission/dispatch/check_egress 签发的许可都携带当次检测核验的族。
- `ImmutableRequest` 增加 `route: RouteFamilies`(默认 DUAL):ChatService 仅对门禁模型(Anthropic)用许可的族盖章;非门禁模型保持 DUAL(原语义,无地理检测依赖)。dispatch 前重检通过时以**新核验族**重盖章。
- `OpenRouter` 持三个客户端(auto/v4/v6,`with_client` 注入时三者同源),`stream` 按请求 `route` 选择;未核验族对生成连接不可达。

### 4. 代理兼容(显式与系统 HTTP 代理)

- 设计上**不区分**直连/代理模式:族过滤统一作用于"本次连接的对端"(直连=目标域,代理=代理 host),检测与生成同一过滤,任何模式下都按构造一致。
- 本地 IPv4 代理(host 为 IP 字面量)不被 v6 过滤误伤(见边界 a);远端出口族由代理决定,门禁核验的就是"经该代理的路径"。集成测试覆盖:检测/生成都经代理、无裸直连探测(XFF 断言)、代理远端 CN → 拦截 POST=0、代理远端 US → 放行 POST=1。

### 5. reload 一致性(Codex 阻塞②修复)

- `GeoIpGuard::rebuild_clients(&NetworkConfig)`:以与 `generation_client*` 相同构造路径重建 v4/v6 检测客户端,**先构建后替换**(失败保持旧客户端并显式报错),成功即 bump revision。
- `Desktop::reload_providers`:先以新网络策略快照(重新读取 `AICOUNCIL_PROXY`)rebuild 检测客户端,再重建 provider(生成客户端同一快照来源),随后清空 `Desktop.permit`。检测与生成策略不再分裂;旧许可经 revision 失效 + 显式清空双重保险。

### 6. 保持不变的原有约束

主源权威/备源降级、单源 4s 与总预算 8s、响应体 64KiB 上限、禁重定向、检测请求无 Authorization/prompt/附件、非 Anthropic 零检测依赖、admission/retry/dispatch 三层门禁、permit TTL 3s + revision、取消、混合批次整批拒绝、费用/落库语义。UI/DTO 无结构变化(`EgressStateDto.detail` 为自由文本,证据串扩展信息直接可见,无 IPC 契约变更,前端零改动)。

## 变更文件(本次增量)

- `src/services/network.rs`:`RouteFamilies`、`FamilyFilterResolver`、`*_client_for` 族构造;测试。
- `src/services/egress.rs`:双族并行检测、每族截止期、`aggregate`、`GeoIpGuard` 客户端内部原子替换(`rebuild_clients`)、`EgressObservation/EgressPermit` 扩展、注入构造 `with_sources`;单测(含聚合、取消、族过滤语义)。
- `src/providers/mod.rs`:`ImmutableRequest.route`。
- `src/providers/openrouter.rs`:三客户端 + `client_for(route)`。
- `src/services/chat.rs`:admission 签发带族许可、请求盖章、dispatch 重检刷新 route。
- `src/desktop/mod.rs`:`reload_providers` 同步 rebuild 检测客户端 + 清 permit。
- `src/desktop/commands.rs`:`check_egress` 许可携带族。
- `tests/egress_dual_stack.rs`(新增,15 用例)、`tests/egress_guard.rs`、`tests/request_recovery.rs`(适配新字段,断言不变)。

## 测试命令与结果

```sh
cargo fmt --all -- --check          # OK
cargo test --release --locked       # 13 个套件全部 test result: ok(0 failed)
cargo test --release --locked --lib --test egress_guard --test request_recovery --test p2_mock_stream
                                    # 117 + 9 + 12 + 17 全过
git diff --check                    # OK
bash scripts/macos_build.sh         # OK(frontend bun build + release + .app + 嵌入前端身份)
bash scripts/macos_release_check.sh # RESULT: OK (version 0.2.3)
```

### 计划自测表 → 对应用例(`tests/egress_dual_stack.rs`,均走真实 GeoIpGuard + OpenRouter + ChatService;族差异用 `localhost` 域名 + 按 127.0.0.1/[::1] 单族绑定的本地 mock 实现,不依赖公网 IPv6)

| 计划场景 | 用例与结果 |
| --- | --- |
| IPv4 US + IPv6 CN/HK:Blocked、证据指明 v6、POST=0、零落库 | `v4_us_v6_cn_blocks_with_zero_generation_post` ✅(证据含 `families=v6`、v6 出口 IP) |
| IPv4 CN/HK + IPv6 US:不被另一族覆盖 | `v4_cn_v6_us_blocked_not_overridden` ✅ |
| 两族均非受限:放行且发送只用已核验路径 | `both_families_verified_allowed_and_send_succeeds` ✅;族约束物理生效见下两行 |
| 仅 IPv4 可用 / 仅 IPv6 可用 | `v6_unverified_excluded_from_send_connector` ✅(生成目标 v6-only → 发送必失败、目标零连接)、`v6_only_verified_cannot_use_unverified_v4` ✅(对称)、`v4_only_usable_network_not_misblocked_on_v6_429` ✅ |
| IPv6 可达但 429/超时:Unknown 或明确排除 | `v6_unverified_excluded_from_send_connector`(超时)与 429 用例 ✅:许可只含 v4,note 记录"未核验,已从发送连接排除" |
| v6 主源 CN 权威,不被备源/v4 US 覆盖 | `primary_v6_cn_authoritative_over_fallback_and_v4` ✅(主源有效时备源零访问) |
| 本地 IPv4 HTTP 代理:都走代理、不裸直连、不因本地非 v6 失败 | `local_ipv4_http_proxy_probes_via_proxy_not_direct` ✅(XFF 全量断言)、`local_ipv4_http_proxy_allowed_send_goes_through_proxy` ✅(检测 2 + 生成 1 次全经代理) |
| 非法代理显式失败 | `clients_build_with_and_without_proxy`(network.rs)✅(含 `*_client_for`) |
| 策略切换/reload/permit 过期/排队 | `reload_rebuilds_detection_clients_shared_with_service` ✅(真实客户端切换,见下);permit TTL/revision:`permit_ttl_and_revision` + A08 系列 ✅ |
| 混合批次与 retry | `mixed_batch_blocked_all_or_nothing_with_real_guard` ✅ + A05/A06/A09(StaticGuard,语义不变)✅ |
| 取消、总预算、上限、重定向 | `run_detection_cancel_ends_both_families_immediately` ✅、挂起源单源超时序列测试 ✅、总预算回归见下 ✅ |
| 敏感数据 | `detection_requests_carry_no_credentials` ✅(GET /trace、无 Authorization、无 Key、UA 正确) |

### Codex 阻塞修复回归

- **① 总预算丢弃已完成族**:`v4_allowed_survives_v6_exceeding_total_budget`(单源超时 5s > 总预算 700ms,只有截止期能结束 v6;判定保留 Allowed{v4} 且发送成功 POST=1)、`v6_blocked_survives_v4_exceeding_total_budget`(v6 CN Blocked 保留,POST=0 零落库)。
  **反证**:临时还原旧 `timeout(total, join(v4,v6))` 实现后,上述两用例**均失败**(v4 用例提交被 Unknown 拦截;v6 用例丢失 CN 结论),证明回归确实捕获旧行为;随后恢复新实现,15/15 通过。
- **② reload 策略分裂**:`reload_rebuilds_detection_clients_shared_with_service`:guard 与 ChatService 共享同一 `Arc<GeoIpGuard>`(生产同款装配);初始经转发代理 P1(远端 US)发送成功;`rebuild_clients` 切到策略不同的 P2(直接返回 CN)后:revision 恰好 +1、同一实例检测随即经 P2 得 CN Blocked、P1 计数停在前两次(不再走旧客户端)、reload 前签发的旧 permit 重检拒绝、无新 POST。非 StaticGuard bump,验证的是真实 reqwest 客户端与共享实例传播。

### 实网只读验证(无生成、无费用;探针脚本为临时文件,已删除)

修复后终验(生产 `GeoIpGuard::default()` 对真实检测源):

```
LIVE_PROBE elapsed=3.110140875s
LIVE_PROBE verdict=blocked evidence=source=openrouter.ai/cdn-cgi/trace
  ip=2408:893a:9221:a881:c14d:ebe2:edd8:f21b country=CN families=v6
  checked_at=2026-10-31T12:14:27.482094+00:00
  note=IPv4 核验通过:ip=216.234.143.22 country=US source=openrouter.ai/cdn-cgi/trace
```

本机当前即为计划描述的 **IPv4 US + IPv6 CN** 状态:两族均经 OpenRouter 主源核验,生产判定 Blocked、证据明确 v6 路径;旧实现下该状态会凭 v4 US 签发 Allowed 并允许生成经 v6 CN 泄漏。修复前首验亦一致(v4 主源超时经 ipapi.co 备源得 US,5.08s,同样 Blocked)。"生成 POST=0"的实网证明以真实 ChatService/dispatch 路径的 mock e2e 为准(计划禁止自动付费实网生成,未做真实生成)。

## 真实边界(如实)

- a. **IP 字面量主机不做族过滤**:hyper-util 对 `127.0.0.1` 等字面量跳过解析器直连。生产检测源与业务域名均为域名,不受影响;本机代理 host 为字面量时反而保证 v4/v6 检测都能到达本地代理。已在 `FamilyFilterResolver` 文档与测试注释标明。
- b. **代理模式下"族"指本地连接族**:经代理时族过滤约束的是到代理的连接,远端出口由代理决定;许可语义为"发送只能使用已核验的连接路径",不宣称远端出口族。
- c. **外部系统不可感知**:代理自身动态换出口、系统级网络切换(Wi-Fi/TUN 变化)、代理规则变更不触发 revision;许可 TTL 3s 内的窗口与旧版一致。reload(`AICOUNCIL_PROXY`/凭据变化)已同步重建检测与生成。
- d. **连接池复用**:reqwest 连接池空闲期(默认 90s)可能复用许可周期内的同族连接;族过滤保证不跨族复用,同族复用受池策略管理。reload 重建 provider 客户端,池随之丢弃。
- e. **reload 读取策略快照两次**:检测 rebuild 与 provider 构造各自读取 `AICOUNCIL_PROXY` 环境变量,若用户恰在两次读取之间改环境变量,理论上存在窗口;.env 编辑走 UI 的 reload 流程,实际风险极小,记录不展开处理。
- f. **检测成本**:IPv6 黑洞(有 AAAA 无路由)网络上每次动作的门禁最多消耗一个总预算(8s)后才放行(仅 v4);截止期修复保证只影响延迟,不影响正确性。
- g. **GeoIP 本义边界**(承计划):GeoIP 结论是数据库对同一出口的归属判定,不证实真实物理地理,不宣称保证上游所见 IP;本实现保证的是"发送连接只能使用已核验路径(族)"。
- h. **单机 mock 双族**:集成测试用 macOS 本地 loopback 双族(127.0.0.1/[::1])模拟族差异,不依赖也不验证公网 IPv6 路由质量;实网探测覆盖真实路由一次(见上)。

## 产物与清理

- `dist/`:已重建(`dist/aicouncil` + `dist/AiCouncil.app` + `dist/frontend-embed.sha256`),`scripts/macos_release_check.sh` → `RESULT: OK (version 0.2.3)`;版本未变更(沿用工作区既有 0.2.3,本次为同一版本内的行为修复,未做版本 bump)。
- 已清理:`target/`(构建后按 AGENTS.md 删除)、临时实网探针 `tests/live_egress_probe.rs`(证据已录入本文)、无截图/日志/其他临时产物;未新增依赖(`Cargo.toml`/`Cargo.lock` 无本次改动);用户 `.env`、data、既有未提交改动全部保留;未执行任何 git 提交/重置。

## Codex 最小验收

- 对照任务开始时的源码副本，只审本次增量；确认关键回归经过真实 GeoIpGuard、ChatService 和生成端点，断言 IPv4 US + IPv6 CN 时生成 POST=0、零消息/请求落库。
- 回交并复查两项缺陷：总预算到期保留已完成分支；reload 同步重建检测客户端并使旧许可失效。对应新增回归与旧实现反证结果见上。
- 独立执行 `cargo fmt --all -- --check`、`git diff --check`、`bash scripts/macos_release_check.sh`，全部通过；raw exe 与 `.app` 内可执行文件均晚于最终源码，正式产物已更新。
- 复用 OpenCode 的测试结果，未重复编译整套测试；未发送真实生成请求，未修改用户网络或凭据，未提交。
- 最终清理包括源码对照临时副本、`frontend/node_modules/` 与 Tauri 自动生成的 `gen/`；保留正式前端 assets、源码/文档、`dist/`、用户配置和数据。
