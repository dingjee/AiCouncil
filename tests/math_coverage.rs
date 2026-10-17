//! P0.4 公式覆盖测试:验收矩阵 R1/R2 的公式子集必须全部可渲染。
//! 不依赖 GUI,直接验证 iced_math 输出。

use aicouncil::render::math::{normalize_tex, MathCache};

/// R1 要求正常显示的公式(渲染为 SVG 图形,不回退源文)。
const MUST_RENDER: &[&str] = &[
    // 行内基础
    "E = mc^2",
    "x^2 + y^2 = z^2",
    r"\gamma = \frac{1}{\sqrt{1 - v^2/c^2}}",
    r"\vec{F} = m\vec{a}",
    "e^{i\\pi} + 1 = 0",
    r"\sum_{i=1}^{n} i = \frac{n(n+1)}{2}",
    // 四类定界符内容
    r"\tan x = \frac{\sin x}{\cos x}",
    r"\int_{-\infty}^{\infty} e^{-x^2} \, dx = \sqrt{\pi}",
    r"\oint_{\partial \Omega} \vec{F} \cdot d\vec{s} = \int_{\Omega} \nabla \cdot \vec{F} \, dV",
    // 分式/根式/积分/级数
    r"\frac{\partial^2 u}{\partial t^2} = c^2 \frac{\partial^2 u}{\partial x^2}",
    r"\sqrt[3]{x^2 + y^2} \le \sqrt{x^2 + y^2 + z^2}",
    r"\prod_{k=1}^{n} k = n!",
    r"\lim_{x \to 0^+} \frac{\ln x}{x} = -\infty",
    "f(x) = \\sum_{n=0}^{\\infty} \\frac{f^{(n)}(a)}{n!}(x-a)^n",
    // 矩阵与多行
    r"A = \begin{pmatrix} a_{11} & a_{12} \\ a_{21} & a_{22} \end{pmatrix}",
    r"\det A = a_{11}a_{22} - a_{12}a_{21}",
    r"\begin{cases} x + y = 3 \\ 2x - y = 0 \end{cases}",
    r"\begin{aligned} \nabla \cdot \vec{E} &= \frac{\rho}{\varepsilon_0} \\ \nabla \times \vec{B} &= \mu_0 \vec{J} + \mu_0 \varepsilon_0 \frac{\partial \vec{E}}{\partial t} \end{aligned}",
    // align -> aligned 映射后
    r"\begin{align} f(x) &= (x+a)(x+b) \\ &= x^2 + (a+b)x + ab \end{align}",
    // 其他
    r"\{x \mid x > 0\}",
    "a \\le b \\ge c",
];

/// 已知不支持、允许源文回退的复杂宏(记录在 docs/RENDERING.md)。
const KNOWN_FALLBACK: &[&str] = &[r"\LaTeX"];

#[test]
fn r1_formulas_all_render() {
    let mut cache = MathCache::new();
    let mut failures = Vec::new();
    for tex in MUST_RENDER {
        let normalized = normalize_tex(tex);
        let m = cache.get_or_render(&normalized, 16.0, true, iced::Color::BLACK);
        if m.error.is_some() {
            failures.push(normalized);
        }
    }
    assert!(
        failures.is_empty(),
        "以下公式未能渲染:\n{}",
        failures.join("\n---\n")
    );
}

#[test]
fn unsupported_macro_falls_back_gracefully() {
    let mut cache = MathCache::new();
    for tex in KNOWN_FALLBACK {
        let m = cache.get_or_render(tex, 16.0, false, iced::Color::BLACK);
        assert!(m.error.is_some(), "{tex} 应走源文回退");
    }
}

#[test]
fn svg_sizes_reasonable() {
    let mut cache = MathCache::new();
    let m = cache.get_or_render(
        r"\begin{pmatrix} 1 & 2 \\ 3 & 4 \end{pmatrix}",
        17.0,
        true,
        iced::Color::BLACK,
    );
    assert!(m.error.is_none(), "矩阵必须渲染");
    assert!(m.size.width > 20.0 && m.size.height > 20.0, "{:?}", m.size);
}
