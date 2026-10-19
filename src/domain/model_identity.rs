//! 模型身份:开发商归属与家族分组。
//!
//! - 门禁按**开发商**判断(Anthropic 归属覆盖 `anthropic/*` 全部家族与
//!   受控 `claude-*` 别名),与传输网关(OpenRouter)身份分离;
//!   禁止从显示名或任意 substring 推断归属。
//! - UI 按家族分组;`ModelPresentation` 只负责展示,HTTP body、偏好、
//!   计费、重试与导出审计仍使用原始模型 ID。

/// 模型开发商(门禁判定维度)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Developer {
    Anthropic,
    OpenAi,
    Google,
    DeepSeek,
    MoonshotAi,
    Meta,
    Mistral,
    Alibaba,
    Microsoft,
    XAI,
    Unknown,
}

/// OpenRouter 命名空间 → 开发商。未识别命名空间为 Unknown,
/// 不猜测(不因显示名含某家族词而归属)。
fn developer_from_namespace(vendor: &str) -> Developer {
    match vendor {
        "anthropic" => Developer::Anthropic,
        "openai" => Developer::OpenAi,
        "google" | "google-ai" => Developer::Google,
        "deepseek" | "deepseek-ai" => Developer::DeepSeek,
        "moonshotai" | "moonshot" => Developer::MoonshotAi,
        "meta-llama" | "meta" | "llama" => Developer::Meta,
        "mistralai" | "mistral" => Developer::Mistral,
        "qwen" | "alibaba" | "qwen-alibaba" => Developer::Alibaba,
        "microsoft" => Developer::Microsoft,
        "x-ai" | "xai" => Developer::XAI,
        _ => Developer::Unknown,
    }
}

/// 受控别名表:无命名空间前缀、但明确归属 Anthropic 的 ID 形态
/// (OpenAI 兼容端点的 `claude-*` 家族)。新增别名必须显式登记。
const ANTHROPIC_ALIAS_PREFIXES: &[&str] = &["claude-", "claude"];

/// 模型开发商归属。参数是原始模型 ID(路由用,非显示名)。
pub fn developer_of(model_id: &str) -> Developer {
    let id = model_id.trim().to_lowercase();
    if let Some((vendor, _rest)) = id.split_once('/') {
        let dev = developer_from_namespace(vendor);
        if dev != Developer::Unknown {
            return dev;
        }
    }
    // 受控别名(仅显式登记的形态;未知 ID 一律 Unknown,不猜测)
    if ANTHROPIC_ALIAS_PREFIXES
        .iter()
        .any(|p| id == *p || (p.ends_with('-') && id.starts_with(p)))
    {
        return Developer::Anthropic;
    }
    Developer::Unknown
}

/// Anthropic 出口门禁判定:覆盖其全部模型家族(不限 Claude),
/// 含未来新 ID;显示名不参与判定(A14)。
pub fn is_anthropic(model_id: &str) -> bool {
    developer_of(model_id) == Developer::Anthropic
}

/// 家族 key → 展示标签(组标题大小写)。"o" 家族展示为 "o series"。
fn family_display(key: &str) -> &'static str {
    match key {
        "claude" => "Claude",
        "gpt" => "GPT",
        "gemini" => "Gemini",
        "deepseek" => "DeepSeek",
        "kimi" => "Kimi",
        "qwen" => "Qwen",
        "llama" => "Llama",
        "mistral" => "Mistral",
        "grok" => "Grok",
        "palm" => "PaLM",
        "o" => "o series",
        _ => "",
    }
}

/// 家族 ID 前缀表:(id 前缀, 家族 key)。
fn family_from_id_prefix(model_id: &str) -> Option<(&'static str, &'static str)> {
    const TABLE: &[(&str, &str)] = &[
        ("anthropic/claude", "claude"),
        ("openai/gpt", "gpt"),
        ("openai/o1", "o"),
        ("openai/o3", "o"),
        ("openai/o4", "o"),
        ("google/gemini", "gemini"),
        ("google/paLM", "palm"),
        ("deepseek/", "deepseek"),
        ("moonshotai/kimi", "kimi"),
        ("moonshot/kimi", "kimi"),
        ("qwen/", "qwen"),
        ("qwen2", "qwen"),
        ("qwen3", "qwen"),
        ("meta-llama/", "llama"),
        ("meta/", "llama"),
        ("llama", "llama"),
        ("mistralai/", "mistral"),
        ("mistral/", "mistral"),
        ("x-ai/grok", "grok"),
        ("xai/grok", "grok"),
        ("grok-", "grok"),
        ("gpt-", "gpt"),
        ("gemini-", "gemini"),
        ("claude-", "claude"),
        ("kimi-", "kimi"),
    ];
    let id = model_id.trim().to_lowercase();
    // 长前缀优先匹配,避免短前缀误伤
    let mut best: Option<(usize, &'static str)> = None;
    for (prefix, family) in TABLE {
        if id.starts_with(prefix) {
            let len = prefix.len();
            if best.map(|(l, _)| len > l).unwrap_or(true) {
                best = Some((len, family));
            }
        }
    }
    best.map(|(_, family)| (family, family))
}

/// 已知公司目录名前缀(仅这些可确认地移除;不按第一个冒号盲切,
/// 因为 `:free/:thinking/:nitro` 等后缀有含义)。
const KNOWN_COMPANY_PREFIXES: &[&str] = &[
    "Anthropic:",
    "OpenAI:",
    "Google:",
    "DeepSeek:",
    "Moonshot AI:",
    "Moonshot:",
    "Meta:",
    "Mistral AI:",
    "Mistral:",
    "Alibaba:",
    "Microsoft:",
    "xAI:",
    "Qwen:",
];

fn strip_known_company_prefix(name: &str) -> &str {
    for p in KNOWN_COMPANY_PREFIXES {
        if let Some(rest) = name.strip_prefix(p) {
            return rest.trim_start();
        }
    }
    name
}

/// 家族关键词(用于目录名识别,如 "Anthropic: Claude Opus 5.5" → Claude)。
fn family_from_name(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    const WORDS: &[(&str, &str)] = &[
        ("claude", "claude"),
        ("gpt", "gpt"),
        ("gemini", "gemini"),
        ("deepseek", "deepseek"),
        ("kimi", "kimi"),
        ("qwen", "qwen"),
        ("llama", "llama"),
        ("mistral", "mistral"),
        ("grok", "grok"),
    ];
    for (word, family) in WORDS {
        if lower.contains(word) {
            return Some(family);
        }
    }
    None
}

/// ID 家族段去除(组内短名用):`claude-opus-5.5` → `opus-5.5`。
fn strip_family_segment(tail: &str, family: &str) -> String {
    let mut out = tail.to_string();
    // 仅去除作为前导家族段的精确匹配(避免误伤家族名出现在变体名中的情况)
    let prefix = format!("{family}-");
    if let Some(rest) = out.strip_prefix(&prefix) {
        out = rest.to_string();
    } else if out == family {
        out = String::new();
    } else if let Some((head, rest)) = out.split_once('-') {
        if head == family {
            out = rest.to_string();
        }
    }
    out
}

/// 首字母大写的版本号/单词美化:`opus-5.5-preview` → `Opus 5.5 Preview`。
/// 保留 `:free` 等变体后缀为 `(free)`;版本号(4o、2.5)保留原样;
/// o 系列的"字母+数字"形态(o3)保留原样,其他家族同类形态首字母
/// 大写(k2 → K2)。
fn humanize(raw: &str, o_family: bool) -> String {
    // 变体后缀(:free/:thinking/...)单独处理,展示为 (free)
    let (base, mut suffix) = match raw.split_once(':') {
        Some((b, s)) => (b, Some(s.to_string())),
        None => (raw, None),
    };
    if let Some(s) = &mut suffix {
        *s = format!("({s})");
    }
    let mut words: Vec<String> = base
        .split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let has_digit = w.chars().any(|c| c.is_ascii_digit());
            if has_digit && (o_family || w.starts_with(|c: char| c.is_ascii_digit())) {
                return w.to_string(); // 版本号 / o 系列形态
            }
            let mut c = w.chars();
            match c.next() {
                Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect();
    if let Some(s) = suffix {
        words.push(s);
    }
    words.join(" ")
}

/// 展示归一化结果(仅用于 UI;选择/持久化仍按原始 ID)。
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPresentation {
    pub raw_id: String,
    /// 目录原名(可为空)。
    pub catalog_name: String,
    /// 家族 key(分组依据;单项家族也独立成组)。
    pub family_key: String,
    /// 组标题(如 Claude / GPT / o series / 独立模型名)。
    pub group_label: String,
    /// 组内选项短名(Opus 5.5 / 3.5 Sonnet / V3)。
    pub item_label: String,
    /// 完整易读名(Claude Opus 5.5)。
    pub full_label: String,
}

/// 由原始 ID + 目录名(可空)构建展示。稳定优先级:
/// 显式家族 ID 前缀 → 已知公司前缀去除后的目录名 → 保守 fallback。
pub fn presentation(model_id: &str, catalog_name: &str) -> ModelPresentation {
    let id = model_id.trim();
    let id_lower = id.to_lowercase();
    let name = catalog_name.trim();
    let stripped_name = strip_known_company_prefix(name);

    // (family_key, tail, single):single = 未知单模型(独立组,完整短名)
    let (family_key, tail, single) = if let Some((key, _)) = family_from_id_prefix(&id_lower) {
        let tail = id
            .split_once('/')
            .map(|(_, t)| t.to_string())
            .unwrap_or_else(|| id.to_string());
        (key.to_string(), tail, false)
    } else if name.is_empty() {
        // 无目录名:ID 尾段的人类可读名作为独立组
        let tail = id
            .split_once('/')
            .map(|(_, t)| t.to_string())
            .unwrap_or_else(|| id.to_string());
        (format!("single:{id}"), humanize(&tail, false), true)
    } else if let Some(family) = family_from_name(stripped_name) {
        // 目录名识别家族:组内短名用 ID 尾段(比目录名更稳定地保留变体)
        let tail = id
            .split_once('/')
            .map(|(_, t)| t.to_string())
            .unwrap_or_else(|| id.to_string());
        (family.to_string(), tail, false)
    } else {
        // 未知单模型:人类可读独立组,完整短名(不显示空标签)
        let label = if stripped_name.is_empty() {
            humanize(
                &id.split_once('/')
                    .map(|(_, t)| t.to_string())
                    .unwrap_or_else(|| id.to_string()),
                false,
            )
        } else {
            stripped_name.to_string()
        };
        (format!("single:{id}"), label, true)
    };

    let family_label = if single {
        // 单项家族的组标题:tail 已是人类可读短名(或目录名)
        tail.clone()
    } else {
        family_display(&family_key).to_string()
    };

    let item_raw = if single {
        String::new()
    } else {
        strip_family_segment(&tail, &family_key)
    };
    // 单项家族短名 = 家族名本身(不显示空标签);o 系列等家族 key 与
    // ID 段不同名由 family_display 处理
    let item_label = if item_raw.trim().is_empty() {
        family_label.clone()
    } else {
        humanize(&item_raw, family_key == "o")
    };
    // 完整名:单项家族/家族名即短名(o3)时不再重复前缀
    let full_label =
        if single || family_key == "o" || item_label.eq_ignore_ascii_case(&family_label) {
            item_label.clone()
        } else {
            format!("{family_label} {item_label}")
        };

    ModelPresentation {
        raw_id: id.to_string(),
        catalog_name: name.to_string(),
        family_key,
        group_label: family_label,
        item_label,
        full_label,
    }
}

/// 组内同名消歧:多个原始 ID 规范化后同名时,追加最短必要区分
/// (变体后缀/日期/提供商)。原地修改 item_label/full_label。
pub fn disambiguate(items: &mut [ModelPresentation]) {
    use std::collections::HashMap;
    // (family, item_label) → 出现次数
    let mut counts: HashMap<(String, String), usize> = HashMap::new();
    for it in items.iter() {
        *counts
            .entry((it.family_key.clone(), it.item_label.clone()))
            .or_insert(0) += 1;
    }
    if counts.values().all(|n| *n == 1) {
        return;
    }
    // 重名项追加原始变体后缀(:free 等)或尾段
    let mut seen: HashMap<(String, String), usize> = HashMap::new();
    for it in items.iter_mut() {
        let key = (it.family_key.clone(), it.item_label.clone());
        if counts.get(&key).copied().unwrap_or(0) <= 1 {
            continue;
        }
        let n = seen.entry(key.clone()).or_insert(0);
        *n += 1;
        if *n == 1 {
            continue; // 第一个保持短名
        }
        let tail = it.raw_id.rsplit('/').next().unwrap_or(&it.raw_id);
        let suffix = tail.rsplit(':').next().unwrap_or(tail);
        let extra = if suffix != tail {
            format!(" ({suffix})")
        } else {
            format!(" ({tail})")
        };
        it.item_label.push_str(&extra);
        it.full_label.push_str(&extra);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_covers_all_families_and_future_ids() {
        // A13/A01:不限 Claude;含未来家族、日期/路由后缀
        for id in [
            "anthropic/claude-opus-5.5",
            "anthropic/claude-fable-5.1",
            "anthropic/claude-3.5-sonnet",
            "anthropic/claude-sonnet-4",
            "anthropic/claude-sonnet-4-20250514",
            "anthropic/test-non-claude-family",
            "anthropic/anything-new",
        ] {
            assert!(is_anthropic(id), "{id} 应按开发商归属 Anthropic");
        }
        // 受控别名(无命名空间)
        assert!(is_anthropic("claude-3-5-sonnet-20241022"));
        assert!(is_anthropic("claude"));
    }

    #[test]
    fn non_anthropic_not_misjudged() {
        // A14:显示名含 Claude 不代表 ID 是 Anthropic;由 ID 判定
        for id in [
            "openai/gpt-4o",
            "google/gemini-2.0-flash",
            "mock/claude-lookalike",
            "somevendor/not-claude",
            "deepseek/deepseek-chat",
        ] {
            assert!(!is_anthropic(id), "{id} 不应归属 Anthropic");
        }
        // 未识别命名空间 → Unknown(不猜测),但不被门禁拦截
        assert_eq!(developer_of("unknown-vendor/model"), Developer::Unknown);
    }

    #[test]
    fn grouping_examples_from_plan() {
        // 计划 6.1 示例(fixture;不代表线上模型存在)
        let p = presentation("anthropic/claude-opus-5.5", "Anthropic: Claude Opus 5.5");
        assert_eq!(p.group_label, "Claude");
        assert_eq!(p.item_label, "Opus 5.5");
        assert_eq!(p.full_label, "Claude Opus 5.5");

        let p = presentation("anthropic/claude-fable-5.1", "Anthropic: Claude Fable 5.1");
        assert_eq!(p.group_label, "Claude");
        assert_eq!(p.item_label, "Fable 5.1");

        let p = presentation("openai/gpt-4o", "OpenAI: GPT-4o");
        assert_eq!(p.group_label, "GPT");
        assert_eq!(p.item_label, "4o");
        assert_eq!(p.full_label, "GPT 4o");

        let p = presentation("openai/o3", "OpenAI: o3");
        assert_eq!(p.group_label, "o series");
        assert_eq!(p.item_label, "o3");
        assert_eq!(p.full_label, "o3");

        let p = presentation("google/gemini-2.5-pro", "Google: Gemini 2.5 Pro");
        assert_eq!(p.group_label, "Gemini");
        assert_eq!(p.item_label, "2.5 Pro");
        assert_eq!(p.full_label, "Gemini 2.5 Pro");

        let p = presentation("deepseek/deepseek-chat", "DeepSeek: Chat");
        assert_eq!(p.group_label, "DeepSeek");
        assert_eq!(p.item_label, "Chat");
        assert_eq!(p.full_label, "DeepSeek Chat");

        let p = presentation("moonshotai/kimi-k2", "Moonshot AI: Kimi K2");
        assert_eq!(p.group_label, "Kimi");
        assert_eq!(p.item_label, "K2");
    }

    #[test]
    fn real_legacy_names_keep_versions() {
        // 旧 Claude 命名保留真实版本,不重命名为示例版本
        let p = presentation(
            "anthropic/claude-3.5-sonnet",
            "Anthropic: Claude 3.5 Sonnet",
        );
        assert_eq!(p.item_label, "3.5 Sonnet");
        let p = presentation("anthropic/claude-sonnet-4", "Anthropic: Claude Sonnet 4");
        assert_eq!(p.item_label, "Sonnet 4");
        let p = presentation(
            "anthropic/claude-sonnet-4-20250514",
            "Anthropic: Claude Sonnet 4 (2025-05-14)",
        );
        assert_eq!(p.item_label, "Sonnet 4 20250514");
    }

    #[test]
    fn variant_suffixes_preserved() {
        // Free/Thinking/日期等不可丢弃;同 ID 不同变体不合并
        let p = presentation("deepseek/deepseek-r1:free", "DeepSeek: R1 (free)");
        assert_eq!(p.group_label, "DeepSeek");
        assert!(p.item_label.contains("(free)"), "{}", p.item_label);
        let p = presentation("deepseek/deepseek-r1", "DeepSeek: R1");
        assert!(!p.item_label.contains('('), "{}", p.item_label);
    }

    #[test]
    fn unknown_single_models_get_own_group_with_full_name() {
        // 未知单模型各自独立成组;不塞"其他";不显示空标签
        let p = presentation("weirdvendor/special-model-x", "Special Model X");
        assert_eq!(p.group_label, "Special Model X");
        assert_eq!(p.item_label, "Special Model X");
        assert_eq!(p.full_label, "Special Model X");

        let p2 = presentation("another/thing", "");
        assert!(!p2.group_label.is_empty());
        assert!(!p2.item_label.is_empty());
    }

    #[test]
    fn colon_suffix_not_mistaken_for_company_prefix() {
        // 不按第一个冒号切分:`:free` 后缀保留
        let p = presentation("deepseek/deepseek-chat:free", "DeepSeek: Chat (free)");
        assert_eq!(p.group_label, "DeepSeek");
        assert!(p.item_label.contains("(free)"), "{}", p.item_label);
    }

    #[test]
    fn disambiguation_appends_shortest_suffix() {
        // 同家族同名不同 ID:第二项追加最短必要区分,不合并成一个选项
        let a = presentation("openai/gpt-4o", "OpenAI: GPT-4o");
        let b = presentation("openai/gpt-4o-2024-08-06", "OpenAI: GPT-4o");
        let mut items = vec![a, b];
        disambiguate(&mut items);
        assert_eq!(items[0].item_label, "4o");
        assert!(items[1].item_label != items[0].item_label, "应消歧");
        assert!(
            items[1].item_label.contains("2024"),
            "{}",
            items[1].item_label
        );
        // 不同家族同名不互相干扰
        let c = presentation("vendorx/4o", "4o");
        let mut items2 = vec![presentation("openai/gpt-4o", ""), c];
        disambiguate(&mut items2);
        assert_eq!(items2[1].item_label, "4o");
    }
}
