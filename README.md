# AiCouncil

极简原生 Rust 桌面 AI 聊天:OpenRouter 自动模型目录、统一模型选择器
(单选/同题多模型并行同一入口)、多选回答分支追问、文本附件与长粘贴
自动转附件、LaTeX 公式渲染、**正文局部选择与公式原文复制**、
Soft Sage 阅读配色、可调正文字号与右栏宽度、自动会话标题(DeepSeek,
可选)、英文界面、本地 SQLite 持久化、Markdown 导出与费用标签。

当前状态见 [docs/IMPLEMENTATION_STATUS.md](docs/IMPLEMENTATION_STATUS.md);
渲染结论与已知缺口见 [docs/RENDERING.md](docs/RENDERING.md);
验收矩阵与未验证项见 [docs/ACCEPTANCE.md](docs/ACCEPTANCE.md)。

## 运行(Windows)

```sh
cargo run --release
```

依赖:Rust 1.88+(工具链由 rust-toolchain.toml 固定);Windows 10/11。
首次启动自动创建数据目录与数据库。

## 数据位置

- 数据目录:`<aicouncil.exe 所在目录>\data\`
  - `aicouncil.db` — 会话、消息树、生成记录、费用账目、标题元数据与
    辅助请求台账(SQLite/WAL;旧库自动迁移,已有非空标题保护为手动)
  - `config.json` — 非敏感配置(通用设置:正文字号等;窗口:侧栏宽度)
- 首次运行在本地数据库缺失时会从旧 `%APPDATA%\AiCouncil` 使用 SQLite 在线备份迁移，旧文件保留；已有本地数据库不会被覆盖。
- API Key:Windows 使用当前用户 DPAPI 加密后保存在 `data\credentials.dpapi`，并会从旧凭据管理器导入；文件不能由其他 Windows 用户解密。其他平台仅在本次运行内保存。**Key 不写入普通配置、不进数据库、不进导出。**
- Windows 数据目录应位于当前用户可写目录。把 `data` 文件夹复制到另一台电脑可迁移聊天记录，但加密凭据仍绑定原 Windows 用户。

## 正文局部选择与复制(0.1.3)

- 鼠标拖动在**单条消息内**跨段落/标题/列表/代码块/表格选择;双击选词、
  三击选整块;选区期间暂停自动跟随,Esc 或点击清除。
- `Ctrl+C` / 复制的是**纯文本投影**(非 Markdown 源文):粗体/链接取
  解码文字,行内代码取内容,**公式返回含原始定界符的源文**
  (`$x^2$` 原样复制,原文是 `\(x^2\)` 就保留该形式);跨段落空行,
  表格以 Tab 分列,代码保留换行缩进,列表项带标记。
- 消息卡片的"复制"按钮(tooltip: Copy source)仍复制整条 Markdown
  原文,与局部复制明确区分。输入框内的 Ctrl+C 由编辑器自己处理,
  不被抢占。

## 统一模型选择器(0.1.4 家族分组)

- 输入区左下为唯一选择器:按**模型家族分组**(如 Claude 组内显示
  "Opus 5.5"、"Fable 5.1";GPT/Gemini/Kimi/独立单模型同理),组标题
  为小号灰字、不可点,与选项明显区分;`:free`/`:thinking`/日期等
  变体信息保留。
- 选择器内置搜索框(匹配家族、短名、完整名、目录原名与 raw ID,
  大小写不敏感);列表高度随窗口自适应;关闭选择器即清空搜索。
- 单模型模式点击即选;打开"Parallel"后同一列表变复选,星标指定主
  模型(主分支归属)。默认勾选"当前 + 最常用另一个"。
- 展示与选择分层:界面显示归一化名(raw ID 见 tooltip),HTTP 请求、
  偏好、计费、重试与导出审计仍使用原始模型 ID。
- 多支追问勾选生效期间,并行切换禁用并显示提示(不产生分支×模型
  隐式笛卡尔积)。

## Anthropic 模型出口门禁(0.1.4)

- 发送或重试任何 **Anthropic 模型**(全部 `anthropic/*` 家族与
  `claude-*` 别名,不限 Claude 命名)前检查当前网络公网出口:
  **中国大陆(CN)/香港(HK)拦截**,明确有效的其他国家/地区放行,
  **无法确认(Unknown)同样拦截**。非 Anthropic 模型不做该检查。
- 拦截时弹出单一提示框:显示本次检测观测(IP/国家/来源/时间),
  可"Re-check"(只重新检测,不自动发送)或"Cancel";若本轮目标
  混有非 Anthropic 模型,另有"移除 Anthropic 模型后发送"。编辑内容、
  附件与模型勾选全部保留;未发出任何请求、不产生任何记录。
- 排队超过许可有效期(3 秒)或网络配置变化时,真正发出前会重检;
  重检被拒的请求标记失败且费用不进入"查询中"。
- 检测服务:ip.sb(主)→ ipapi.co(备),总预算 8 秒;结果仅短期
  内存,不持久化公网 IP。可用 `AICOUNCIL_PROXY=http://host:port`
  统一固定检测与生成的出口(配置失败会显式报错,不静默直连)。
- 边界:GeoIP 查到的是检测服务所见出口;按域名分流/双栈/代理动态
  换出口仍可能与上游不一致,本功能不宣称保证上游所见 IP。

## 失败恢复与重试(0.1.4)

- 失败不再锁死对话:失败/取消/中断的回答下可直接输入新问题继续;
  上下文自动过滤空失败占位(不把"未回答"伪装成已回答),部分输出
  的失败回答会保留并带入后续上下文。
- 失败卡显示简短原因(详细错误在 tooltip,不进模型上下文);SSE
  错误帧、空响应、截断(length/content_filter)分别标注,不再把
  失败伪装成空成功。
- 重试按钮受资格控制:冷却中显示倒计时(429 尊重 Retry-After,
  网络/5xx 类按 2/4/8…最多 30 秒退避,重启不归零);凭据/余额/
  参数/模型类错误先提示补救,不鼓励无效连点。
- 同一回答连点重试只产生一次有效尝试(界面防重 + 数据库唯一约束
  兜底);重试默认沿用原请求快照(模型/参数/上下文/系统提示词),
  多次尝试聚合为"Attempt N",旧尝试默认隐藏、可展开。
- 提交失败(如磁盘/数据库错误)不产生假消息,草稿与附件自动恢复。

## 自动会话标题(0.1.3,可选)

- 首次发送成功后立即以首问生成 fallback 标题,并在后台请求 DeepSeek
  生成简短标题;失败(无凭据/超时/限流)保持 fallback,**下次提问
  再试一次**,不在本轮内自动连续重试。
- 凭据:进程环境 `DEEPSEEK_API_KEY` 优先;其次读取 `.env` 中的同名
  变量(开发=项目根,发布=exe 同级;可用 `AICOUNCIL_ENV_PATH` 显式
  指定)。**只取命名变量,不导入整个 .env;密钥不进日志/数据库/UI。**
- 手动改名(主区标题旁铅笔图标或侧栏)始终优先;自动结果晚到时按
  数据库版本号丢弃,不覆盖手动标题。

## 界面与配置(0.1.3)

- Soft Sage 配色:暖灰纸面 + 米白正文 + 低饱和灰绿强调(整个 App,
  含代码高亮底色与公式颜色);Settings 内正文滑杆 12–22px,即时生效
  并持久化。
- 右栏宽度可拖拽(240–480,双击复位 300),结束拖动才写盘;折叠后
  重启恢复。
- 全部 UI 文案为英文(资源表集中于 `src/app/i18n.rs`;用户内容、
  模型 ID、provider 原始错误不翻译)。

## 文本附件与长粘贴(0.1.2)

- 输入区"Attach"按钮可选择多个文本/代码文件(单文件 ≤256KB,总数
  ≤10 个/总计 ≤1MB,超限跳过并在 toast 提示;非 UTF-8 文件报错跳过)。
- 粘贴 ≥2500 字符的长文本时自动转为附件(与 Claude.ai 阈值一致),
  不再撑爆输入框。
- 附件为**纯文本拼接**:发送时以 `===== 附件 N: 名称 =====` 分隔符拼进
  prompt,对全部模型透明;随消息与会话草稿本地持久化;导出 Markdown 时
  附件全文附在用户消息后。

## 离线开发与验收

```sh
# 本地 mock(目录 + SSE 流式 + 费用),不产生真实费用
cargo run --example mock_openrouter -- 8901
# 另一终端
AICOUNCIL_BASE_URL=http://127.0.0.1:8901/v1 cargo run

# 渲染验收窗口(公式/中英混排/压力样例)
cargo run --example render_probe

# 自动化端到端(mock):=multi 双模型并行;=multi2 再加多支追问
AICOUNCIL_BASE_URL=http://127.0.0.1:8901/v1 AICOUNCIL_E2E=multi cargo run

# 截图/演示辅助(独立数据目录,不迁移真实数据):起始面板与用量展开
AICOUNCIL_DATA_DIR=/tmp/demo-data AICOUNCIL_DEMO_MODE=1 \
AICOUNCIL_DEMO_PANEL=settings AICOUNCIL_DEMO_USAGE=1 cargo run

# 截图辅助:=chips 预置附件 chip;=parallel 再展开并行多选面板
AICOUNCIL_DATA_DIR=/tmp/demo-data AICOUNCIL_DEMO_MODE=1 \
AICOUNCIL_DEMO_ATTACH=parallel cargo run
```

## 常规检查

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## 已知限制

- 渲染器使用 tiny-skia 软件渲染(本机远程桌面会话下 wgpu 不出帧;
  GPU 环境可在 Cargo.toml 恢复 wgpu feature)。
- 正文局部选择当前为**单消息内**;跨消息连续拖选未实现(见
  docs/RENDERING.md 新缺口)。
- 自动标题的真实 DeepSeek 联调未执行(实现经离线口径核对;响应清洗
  有单测,无真实请求)。
- 真实 OpenRouter 联调需用户提供 Key;当前全部功能经本地 mock 验证。
- macOS / Linux 未验证(代码无平台特化,凭据/对话框走跨平台库)。
- 出口门禁的一致性边界与真实代理双栈行为、选择器键盘导航逐键矩阵、
  100%/150% DPI 逐项截图未在本轮验证(见 docs/ACCEPTANCE.md 0.1.4
  未运行清单)。
