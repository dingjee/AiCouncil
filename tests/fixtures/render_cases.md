# 渲染验收样例(P0)

中文与 English 混排、公式、代码、表格综合用例。此文件被 `examples/render_probe.rs` 内嵌加载,不连接任何 API。

## 1. 中英混排与行内公式

质能方程 $E = mc^2$ 是物理学中最著名的公式之一。当 speed 接近光速 $c$ 时,相对论因子 $\gamma = \frac{1}{\sqrt{1 - v^2/c^2}}$ 急剧增大。向量 $\vec{F} = m\vec{a}$ 描述牛顿第二定律,而欧拉公式 $e^{i\pi} + 1 = 0$ 被誉为最美公式。行内公式 $\sum_{i=1}^{n} i = \frac{n(n+1)}{2}$ 需要正确处理上下标与求和符号的基线对齐,并且在窗口缩放时保持可读的换行行为。

## 2. 四类公式定界符

- 美元行内:`$a^2 + b^2 = c^2$`
- 圆括号行内:`\( \tan x = \frac{\sin x}{\cos x} \)`
- 双美元块:

$$
\int_{-\infty}^{\infty} e^{-x^2} \, dx = \sqrt{\pi}
$$

- 方括号块:

\[
\oint_{\partial \Omega} \vec{F} \cdot d\vec{s} = \int_{\Omega} \nabla \cdot \vec{F} \, dV
\]

## 3. 分式、根式、积分与级数

$$ \frac{\partial^2 u}{\partial t^2} = c^2 \frac{\partial^2 u}{\partial x^2} \qquad \sqrt[3]{x^2 + y^2} \le \sqrt{x^2 + y^2 + z^2} $$

$$ \prod_{k=1}^{n} k = n! \qquad \lim_{x \to 0^+} \frac{\ln x}{x} = -\infty $$

泰勒展开 $f(x) = \sum_{n=0}^{\infty} \frac{f^{(n)}(a)}{n!}(x-a)^n$ 在 $x = a$ 处收敛。

## 4. 矩阵与多行对齐

$$ A = \begin{pmatrix} a_{11} & a_{12} \\ a_{21} & a_{22} \end{pmatrix}, \quad \det A = a_{11}a_{22} - a_{12}a_{21} $$

$$ \begin{cases} x + y = 3 \\ 2x - y = 0 \end{cases} \implies x = 1, \; y = 2 $$

$$
\begin{aligned}
\nabla \cdot \vec{E} &= \frac{\rho}{\varepsilon_0} \\
\nabla \times \vec{B} &= \mu_0 \vec{J} + \mu_0 \varepsilon_0 \frac{\partial \vec{E}}{\partial t}
\end{aligned}
$$

`align` 环境映射为 aligned(iced_math 尚未支持带编号的 align):

$$
\begin{align}
f(x) &= (x+a)(x+b) \\
     &= x^2 + (a+b)x + ab
\end{align}
$$

## 5. 代码块与 `$` 保护

```rust
fn price_of(s: &str) -> Option<f64> {
    // 字符串里的 $5.00 不应被解析为公式定界符
    let raw = r"$10 and $20 stay literal inside code";
    raw.find('$').map(|_| 5.00)
}
```

行内代码 `cost = $total_usd` 中的美元符号同样必须保持原样。普通文本中「这件商品 \$10,那件 \$20」使用转义;未转义的货币 `价格是 $100 左右` 不应触发公式解析(后面没有配对的 `$`)。

未闭合的公式 `$x^2 + ` 在流式输出中按源文显示;渲染失败(如未知宏 `\unknownmacro{x}`)回退为红色源文,不崩溃。

## 6. 表格与列表

| 模型 | 上下文 | 输入价 | 输出价 |
| --- | --- | --- | --- |
| example/mini | 32k | $0.05/M | $0.20/M |
| example/max | 200k | $1.25/M | $10.00/M |

1. 有序列表第一项,含行内公式 $\alpha + \beta$
2. 第二项,引用见下

> 引用块:数学是科学的皇后。—— 高斯 $\int e^{x} dx = e^x + C$

- [x] 复选框
- [ ] 待办

## 7. 长行与换行压力测试

这是一段特别长的中文文本用于验证自动换行:渲染器必须在任意窗口宽度下把过长的段落折行到下一行,而不是把内容挤出输入框或出现水平滚动条,行间距应当一致,标点符号不应出现在行首,英文单词与中文混排时的断行位置应当自然,例如 when mixing English words with Chinese text the line breaking should treat each CJK character as a break opportunity while keeping Latin words intact,并且行内公式 like $\omega = 2\pi f$ 参与整体折行时作为一个不可分割的整体移动到下一行。

---

*斜体*、**粗体**、***粗斜体***、`code`、[链接](https://example.com)、~~删除线~~ 与 $\LaTeX$ 宏。
