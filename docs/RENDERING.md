# 渲染结论(P0)

## 0.1.5：主页行距、表格与分割线回归修复（2026-10-19）

根因在原生布局坐标，不在 Markdown 解析：`MathFlow::layout` 同时在行节点和文字子节点上加了行的纵向偏移。iced 的 `Layout::children` 会累加父节点坐标，因此第 n 行实际绘制偏移为两倍，但段落只上报一倍高度。结果是段内行距增大、段尾越界覆盖下一段、分割线穿过文字；选区命中仍使用正常行坐标，也会与显示错位。

修复后子节点只保存行内局部坐标，行节点负责纵向定位；文字测量显式使用该块的字号和行高。标题在生成流式排版数据时应用层级字号，文字与公式使用同一字号。

表格另有独立问题：单元格容器各自按内容收缩，短单元格下方暴露网格底色，形成大块灰色区域和断开的竖线。现由内容层决定整行高度，下面的背景层按同一列宽铺满整行，保留连续 1px 网格。居中/右对齐在每行原子的坐标上应用，绘制、命中与高亮共用坐标；空单元格保留一行高度，表头按粗体度量。英文文本按空白切分单词，允许正常折行。

回归入口：

```powershell
cargo test --release --lib --test render_layout --test math_coverage
cargo run --release --example render_probe -- --spacing
```

也可直接由应用使用的 tiny-skia 渲染器生成整篇 PNG，无需操作窗口：

```powershell
$env:AICOUNCIL_RENDER_CAPTURE_DIR = 'docs/screenshots/markdown-0.1.5'
cargo test --release --test render_layout
```

此选项输出 9 张实际 `view_message` 渲染图（3 种字号 × 3 种宽度，1.5× 像素比例），与单元格和文字坐标断言共用同一份布局。

`tests/fixtures/render_spacing.md` 以一张三列表格复现缺陷场景，并包含空表格行、长段落、粗体、居中/右对齐、两条分割线、标题、硬换行及公式混排。布局回归覆盖 12/15/22px 正文、360/760/1100px 消息宽度；段落单测另覆盖更窄宽度，并检查实际绘制坐标、行盒范围与命中位置一致。

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
  - 矩阵/cases/积分/分式块公式渲染为图形(截图 rp_page0.png);
  - Rust 代码块语法高亮 + 圆角背景正常;
  - 20,000 字 + 50 公式压力样例可正常翻页滚动(PGDN×20 无卡死);
  - 30ms 流式 tick 运行期间输入框可正常键入(截图验证输入内容
    "liushiceshi 123" 与发送按钮);
  - 长段落自动折行,无水平溢出、无内容挤出输入框。
  - 截图:docs/screenshots/(rp_page0 为整页,crop_sec2 为局部放大)。

## 已知缺口(不阻塞 P0 出口,记录待办)

> **2026-10-19 更新**:第 1 条(正文局部选择)已在本轮落地,见下节
> 「可选择文档与复制投影(2026-10-19)」;其余缺口继续有效。

1. ~~**任意文字选择/复制**~~ → 本轮实现 `render::document` 选择管线
   (单消息跨块拖选/双击选词/三击整块/公式原子整体复制)。跨消息
   连续拖选暂不支持(见下方新缺口)。
2. `\LaTeX` 等个别文本宏回退源文(iced_math Tier-1 约 60 命令之外)。
3. MathFlow 段落内删除线暂不渲染(罕见组合;rich_text 快路径完整支持)。
4. HTML 实体(如 `&amp;`)在含公式段落按字面显示(quick path 的
   pulldown 解码不受影响)。
5. 基线为 0.8 近似,极端字体(超高 ascent)混排时可能有 ±1px 偏差。

## 可选择文档与复制投影(2026-10-19)

本轮按 `UI_FOUNDATION_PLAN.md` 第 4 节落地了文档渲染底座:

### 新架构

```
原始 Markdown(永不回写)
  → render::md(Strict / LlmCompatible 双策略,占位符预处理)
  → Document IR(Block/Inlines/Part;公式含 raw 原文 + tex 渲染文)
  → render::flow(MathFlow 全段落统一组件;CodeFlow 代码块组件)
  → render::document(FlatDoc leaf 扁平化 + PlainCopyProjection)
  → 显示绘制 + 选区高亮(render::view)
```

- **统一段落组件 MathFlow**:段落/标题/表格 cell 全部经同一组件渲染
  与选择(取代旧 rich_text 快路径 + MathFlow 双轨);块路径
  (`PathSeg`)取代旧的 `paras + para_idx` 顺序关联,标题公式不再
  被正文错取。标题公式按所在级别字号渲染。
- **CodeFlow**:代码块按 (line, col) 粒度选择,等宽字符宽度换算,
  高亮 spans 由 iced_highlighter 生成。
- **复制投影(T12 纯函数测试)**:粗体/链接取解码文字、行内代码取
  内容、公式返回**含原始定界符的 raw 源文**(`$…$`/`\(…\)`/`$$…$$`/
  `\[…\]` 四种原样)、跨段落空行、代码保留换行缩进、表格 tab 分列
  换行分行、列表项带 `- `/`N. ` 标记、引用行带 `> ` 前缀;局部 cell
  选区只复制被选文字。无主体选区时不清空剪贴板。
- **流式选区稳定(T14)**:选区按块路径+grapheme 定位,末尾追加不
  改变已选前缀的投影;块重解析由内容版本号驱动,选区随 IR 语义
  位置保持。

### LlmCompatible 兼容解析(T09)

- `ParsePolicy::Strict` = CommonMark 基线(用户原文默认);
  `ParsePolicy::LlmCompatible` 仅对 assistant 正文启用。
- 唯一放宽:解析后仍保持字面的 `**…**`,在**同一 Text part 内**且
  开/闭定界符紧邻中文标点/引号组合(如 `借用**"X"**的机制`)时构造
  强调节点;先合并相邻同样式 part 再配对,每字符有限次扫描,不做
  全局 regex。
- 反例全部保持严格:乘法 `3 * 4`、单星、snake_case、`##标题`、
  `#hashtag`、未闭合 `**`、跨 part/代码/链接内不配对;歧义场景
  Strict 与 LlmCompatible 输出一致(差分测试)。

### 表格(4.4 / T11)

- 空 cell 不再丢失(旧版 `TagEnd::TableCell` 仅在非空时 push,导致
  后续列左移);异常缺列按表头列数补空。
- 行高 = 内容高度 + 上下 6px padding(消除了 iced Grid 默认
  `AspectRatio(1.0)` 正方形单元格导致的"三列大间距");列边距 10px。
- 1px `line_subtle` 细线经背景色缝隙一次绘制(行间 1px 间隙 +
  外框 1px padding,无双倍深线);表头浅底(code_surface)加粗。
- 对齐应用于文字本身(Center 居中容器收紧内容 / Right 右对齐)。

### 字级(3.2)

- 标题倍率按 φ Fibonacci 极限插级:H1=φ²≈2.618 / H2=φ^1.5≈2.058 /
  H3=φ≈1.618 / H4=√φ / H5=φ^0.25 / H6=1(旧 2/1.7/1.45/1.25/1.1/1
  的顶部层级差异 18% → 27%)。H1–H4 bold,H6 次要色;标题行高 1.22、
  正文 1.55;块间距离按规则计算(标题前 0.9×字号/后 0.35×字号、
  段间 0.55b、表格前后 0.65b),取代旧固定 spacing(8)。
- 正文字号 12–22 可调(Settings slider,持久化 `general.
  body_text_size`);render 与 app 共享 `ui_style.rs`(禁止 render →
  app 依赖)。

### 本轮新已知缺口

1. **跨消息拖选**:选区目前为单消息内(消息卡片为选择边界);
   跨消息按阅读顺序连接列为后续工作(G0-S 全局 coordinator)。
2. **折叠判定**为字符/行启发式(`should_collapse`),未按实际布局
   高度测量;极端窄卡换行密度差异下可能提前/延后折叠。
3. **选区几何**在表格横向滚动、Reading 锚点恢复(相对底部偏移而非
   节点级锚点)场景为近似实现,滚动位置恢复误差可到行级。
4. `Ctrl+C` 的编辑器焦点判定依赖 `focusable::is_focused` 操作,
   异步一帧后生效;输入框内复制始终由编辑器自身处理,不受全局
   快捷键抢占。

## 结论

P0 出口条件满足:普通 Markdown 与常用公式可读,中文无缺字,
20,000 字 + 50 公式可滚动,流式追加不冻结输入。继续 P1。
