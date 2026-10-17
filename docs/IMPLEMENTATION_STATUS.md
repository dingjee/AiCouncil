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

## P1 领域模型、存储与发送规划 — 🚧 进行中

(ids.rs 已建;tree/plan/money/storage/context/planner 进行中)

## 下一步

P1.1 SQLite 迁移与仓储 → P1.2 ContextBuilder/Planner + 测试 →
P1.3 配置与凭据 → P1.4 批量持久化与恢复。
