# 渲染结论(P0)

日期:2026-10-17 · 测试机:Windows 10 19045 x64 · Rust 1.99.1 (x86_64-pc-windows-msvc)

## 实际依赖版本

| 依赖 | 版本 | 用途 |
| --- | --- | --- |
| iced | 0.14.0(features: markdown, highlighter, svg, tokio, advanced, wgpu, tiny-skia, thread-pool) | UI 框架 |
| iced_math | 0.5.0 | LaTeX → SVG(纯 Rust,内置 Latin Modern Math) |
| iced_highlighter | 0.14.0 | 代码块语法高亮(与 iced markdown 组件同引擎) |
| pulldown-cmark | 0.12.2 | Markdown 解析(定界符识别在预处理层完成) |

lockfile 已提交(Cargo.lock)。

## 架构

1. **解析层 `render::md`**:pulldown-cmark 之前先做**占位符预处理**——
   把 `$$…$$`、`\[…\]`、`$…$`、`\(…\)`、`\$` 替换为私用区字符
   (U+E000..U+E002)占位符再解析,事件层还原为 `Part::Math` /
   `Part::DisplayMath`。原因:pulldown 会把 `\[`、`\,` 等转义拆散成
   多个 Text 事件并反转义,直接在事件文本上无法识别定界符。
   - 围栏代码块整块保护;行内代码 span(等长反引号对)保护;
     缩进代码块在事件层还原占位符为原始源文。
   - 货币规则(KaTeX/pandoc 兼容):开 `$` 后非空白;闭 `$` 前非空白、
     后非数字;行内公式内部不允许换行;未闭合定界符保持字面
     (流式输出回退,R2)。
   - `align`/`align*`/`gather`/`gather*` 环境名映射为 `aligned`/`gathered`
     (iced_math 不支持带编号环境;编号本就不在首版范围,渲染语义一致)。
2. **公式渲染 `render::math`**:iced_math `MathRenderer` 输出 SVG,
   固有尺寸从 SVG 头部解析(垂直中心=数学轴)。缓存键 =
   (规范化源文, 字号 bits, 显示模式, 颜色 bits),跨消息去重。
   解析失败回退为红色等宽源文,不 panic。
3. **视图层 `render::view`**:无公式段落走 `rich_text` 快路径
   (完整粗斜体/删除线/链接/行内代码样式);代码块用 iced_highlighter
   高亮 + 横向滚动 + 圆角底色;表格用 grid;块级公式居中、超宽横向滚动。
4. **混排组件 `render::flow`(MathFlow)**:iced 无"文本中嵌行内组件"
   布局,自研贪心断行流式排版:
   - 原子 = 拉丁词 / 单个 CJK 字符 / 行内公式(不可分割);
   - 文本原子用 renderer 的 cosmic-text Paragraph 测量与绘制
     (与 iced 文本组件同栈,中文系统字体回退一致);
   - 公式 SVG 垂直中心对齐数学轴,基线 = 轴 + 0.25em;
     文本基线用 ascent ≈ 0.8×行高近似(cosmic-text 不暴露基线);
   - 测量缓存键 = (字体样式, 文本),固有宽度与容器宽度无关,
     仅内容版本变化时重测;布局节点即行盒,draw 直接按 Layout 摆放。

## 验证结果

- `cargo test --lib`:27 个解析测试通过(定界符四类、货币保护、
  代码保护、块级拆分、表格/列表/引用、流式未闭合回退)。
- `cargo test --test math_coverage`:R1 全部公式类别(分式、根式、
  积分、级数、矩阵、cases、aligned、align 映射、集合括号)渲染为
  SVG 图形,不回退;已知 `\LaTeX` 宏回退源文(记录在案)。
- 真实窗口(`cargo run --example render_probe`,1100×800,Light):
  - 中英混排清晰,无乱码/豆腐块(cosmic-text 系统字体回退);
  - 行内公式渲染为图形,基线与文字对齐良好(截图 crop_sec2.png);
  - 矩阵/cases/积分/分式块公式渲染为图形(截图 rp_page1.png);
  - Rust 代码块语法高亮 + 圆角背景正常;
  - 20,000 字 + 50 公式压力样例可正常翻页滚动(PGDN×20 无卡死);
  - 30ms 流式 tick 运行期间输入框可正常键入(截图验证输入内容
    "liushiceshi 123" 与发送按钮);
  - 长段落自动折行,无水平溢出、无内容挤出输入框。
  - 截图:docs/screenshots/(rp_page0/1 为整页,crop_sec2 为局部放大)。

## 已知缺口(不阻塞 P0 出口,记录待办)

1. **任意文字选择/复制**:iced 0.14 不支持渲染文本的选区(上游长期
   待办)。缓解:P3 起提供消息级"复制"按钮(设计稿本就要求);
   行内代码与代码块可另加复制按钮。首版验收以复制按钮为准。
2. `\LaTeX` 等个别文本宏回退源文(iced_math Tier-1 约 60 命令之外)。
3. MathFlow 段落内删除线暂不渲染(罕见组合;rich_text 快路径完整支持)。
4. HTML 实体(如 `&amp;`)在含公式段落按字面显示(quick path 的
   pulldown 解码不受影响)。
5. 基线为 0.8 近似,极端字体(超高 ascent)混排时可能有 ±1px 偏差。

## 结论

P0 出口条件满足:普通 Markdown 与常用公式可读,中文无缺字,
20,000 字 + 50 公式可滚动,流式追加不冻结输入。继续 P1。
