# macOS 0.2.2 修复说明(出口检测与代理策略)

## 诊断(实测探针,2026-10)

- FlClash TUN 模式激活(utun4),无系统代理、无环境代理变量。
- 同一公网出口 `14.137.237.253` 下,三个检测服务返回相互矛盾的
  归属结论:
  - `api.ip.sb/geoip` → CN
  - `ipapi.co/json` → JP
  - `openrouter.ai/cdn-cgi/trace` → `ip=14.137.237.253`, `loc=JP`
- 0.2.1 的 `egress.rs` 把 `ip.sb` 设为主源并优先采信其有效 CN 结论,
  由此触发 Anthropic 误拦截。
- 观测到的根因:三者是**同一 IP 上的不同 GeoIP 数据库结论相互冲突**
  (既有实现对此无仲裁,按源优先级取了 ip.sb 的 CN)。证据未显示
  各域名走了不同出口或不同物理路径,也未经证实该出口的真实地理归属;
  按域名分流/多出口仅作为一般性限制考虑,不是本次证据所证明的成因。
- 另一缺陷:`network.rs` 两个客户端构造被硬编码 `.no_proxy()`,与
  注释矛盾;reqwest `default-features=false` 未启用 `system-proxy`,
  环境/系统代理实际被忽略;`OpenRouter::with_base_url` 用
  `NetworkConfig::default`(不读 `AICOUNCIL_PROXY`),配置失败时
  静默退回 `reqwest::Client::new()` 直连。

## 修复(后端)

1. **主源改为 `https://openrouter.ai/cdn-cgi/trace`**(业务提供方
   自身域名返回的归属结论,与下游看到同一请求来源归属的判定基准
   最接近,无需 API key):解析文本 `ip=` 与 `loc=`(非 `colo=`);
   严格拒绝字段缺失/重复、非法 IP、非法/哨兵(XX/ZZ)国家码,复用
   既有公网 IP 与国家码校验。主源有效结论(含 CN/HK 拦截)权威,
   不可被备源覆盖;仅主源不可用/数据无效时降级。
2. **备源顺序**:ipapi.co → ip.sb;单源超时 4s,总预算 8s 强制
   (0.2.1 未真正执行总预算);取消立即生效;响应体限量读取、
   禁止重定向保持不变;检测请求不含 Authorization/prompt/密钥。
3. **代理策略**:`network.rs` 移除 `.no_proxy()`;Cargo.toml reqwest
   增加 `system-proxy` 特性,环境/系统代理被一致尊重;
   `AICOUNCIL_PROXY` 现在同样作用于生成客户端(`with_base_url`
   与检测门禁读同一显式代理配置);配置非法时显式失败,
   不再静默直连。
4. **证据透出**:`check_egress` 详情与 ChatService admission 拦截
   详情均包含 `source/ip/country/checked_at`(复用 `evidence()`),
   并注明 GeoIP 结论为数据库判定,可能与上游实际路由/归属不一致。

## 保证边界

本模块落地的是"发请求前检查当前网络策略下的检测出口归属",依据是
各 GeoIP 数据库的判定;**不宣称证实真实物理地理位置,也不宣称保证
Anthropic 上游所见地理结论**。按域名分流、IPv4/IPv6 双栈、代理动态
换出口等仍可能导致不同域名的观测不同。

## 涉及文件

- `src/services/egress.rs`(检测源序列/解析/预算/取消/测试)
- `src/services/network.rs`(no_proxy 移除、注释)
- `src/providers/openrouter.rs`(生成客户端代理传播、去除静默 fallback)
- `src/desktop/commands.rs`、`src/services/chat.rs`(证据详情)
- `Cargo.toml`(版本 0.2.2、reqwest `system-proxy`)
- `tauri.conf.json`、`scripts/macos_release_check.sh`(版本 0.2.2)

## 前端交互修复

- 模型选择器、费用/导出浮层、移动菜单、新建分组、删除确认和共用 Modal
  接入即时关闭规则:外部 pointerdown、焦点移出、窗口失焦、Escape。
  弹窗内部点击保留展开,触发按钮不会关闭后再次误打开。
- 模型选择器始终显示复选框;勾选多个自动并行,一个为单模型,
  全部取消后禁止发送;主模型始终属于选中集合。
- 移除独立 Parallel 按钮;附件按钮位于发送/停止按钮之前。

## 验证结果(2026-10-30)

- 前端(另一负责人):修复已完成,210 个测试/类型检查/构建通过。
- 后端:已验证 — `cargo test --release --locked --lib --test egress_guard`
  全部通过(108 单元 + 9 集成);`cargo fmt` / diff 检查通过;
  `scripts/macos_build.sh` 完成,`scripts/macos_release_check.sh` 通过:
  arm64、0.2.2 版本、ad-hoc 签名、原始 exe/应用包前端嵌入一致性均通过。
- 使用新版 release 库调用实际 `GeoIpGuard`:同一出口 `14.137.237.253`
  返回 `Allowed / JP / openrouter.ai/cdn-cgi/trace`。
- 本次交互真实 Chrome 专测 22/22 通过(880×520、1150×680):附件顺序、
  单一模型入口、内部文字点击、多选、外部点击、焦点移出、窗口失焦,
  以及费用/导出浮层关闭。
- 较长的旧全流程 geometry 脚本仍发生 CDP 输入应答超时(已通过的阶段
  为 21–29 项);暂停/恢复调试器可临时推进,未观察到 JavaScript 死循环,
  完整旧脚本没有通过。本次专测未复现;未据此进行推测性的产品代码修改。
- 新版原生应用使用隔离数据目录与空凭据文件启动成功,未调用收费聊天接口。
- 产物:`dist/aicouncil`、`dist/AiCouncil.app`;既有数据和凭据未作修改。
