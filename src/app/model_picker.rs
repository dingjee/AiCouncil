//! 模型选择器分组 view model(REQUEST_RECOVERY_MODEL_PICKER_PLAN 6)。
//!
//! - 组按家族;组顺序 = 该家族在 enabled 顺序中的首次出现;
//!   组内保持原相对顺序(不因使用次数/搜索跳动)。
//! - 短名只用于展示;选择/持久化仍用原始 ID(`ModelSelectionState`
//!   保持唯一选择来源)。
//! - 轻量搜索:匹配家族、短名、完整名、目录原名与 raw ID,
//!   大小写不敏感;只显示有匹配项的组。

use crate::domain::model_identity::{disambiguate, presentation, ModelPresentation};
use crate::providers::CatalogEntry;

/// 组内选项。
#[derive(Debug, Clone, PartialEq)]
pub struct PickerItem {
    pub model_id: String,
    /// 组内短名(Opus 5.5)。
    pub label: String,
    /// 完整易读名(Claude Opus 5.5)。
    pub full_label: String,
}

/// 家族组。
#[derive(Debug, Clone, PartialEq)]
pub struct PickerGroup {
    /// 组标题(Claude / GPT / o series / 独立模型名)。
    pub label: String,
    pub items: Vec<PickerItem>,
}

fn to_item(p: &ModelPresentation) -> PickerItem {
    PickerItem {
        model_id: p.raw_id.clone(),
        label: p.item_label.clone(),
        full_label: p.full_label.clone(),
    }
}

/// 由 enabled 顺序 + 目录构建分组。同 family_key 才合并;
/// 未知单模型的 family_key 含 raw ID,天然各自独立成组。
pub fn build_groups(enabled: &[String], catalog: &[CatalogEntry]) -> Vec<PickerGroup> {
    let name_of = |id: &str| {
        catalog
            .iter()
            .find(|e| e.model_id == id)
            .map(|e| e.name.clone())
            .unwrap_or_default()
    };
    let mut presentations: Vec<ModelPresentation> = enabled
        .iter()
        .map(|m| presentation(m, &name_of(m)))
        .collect();
    disambiguate(&mut presentations);

    let mut groups: Vec<(String, PickerGroup)> = Vec::new();
    for p in &presentations {
        let item = to_item(p);
        if let Some((key, g)) = groups.iter_mut().find(|(key, _)| *key == p.family_key) {
            let _ = key;
            g.items.push(item);
        } else {
            groups.push((
                p.family_key.clone(),
                PickerGroup {
                    label: p.group_label.clone(),
                    items: vec![item],
                },
            ));
        }
    }
    groups.into_iter().map(|(_, g)| g).collect()
}

/// 大小写不敏感搜索过滤;空搜索返回全部。
pub fn filter_groups<'a>(groups: &'a [PickerGroup], search: &str) -> Vec<&'a PickerGroup> {
    let q = search.trim().to_lowercase();
    if q.is_empty() {
        return groups.iter().collect();
    }
    groups
        .iter()
        .filter(|g| {
            g.label.to_lowercase().contains(&q)
                || g.items.iter().any(|it| {
                    it.label.to_lowercase().contains(&q)
                        || it.full_label.to_lowercase().contains(&q)
                        || it.model_id.to_lowercase().contains(&q)
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat(id: &str, name: &str) -> CatalogEntry {
        CatalogEntry {
            model_id: id.into(),
            name: name.into(),
            context_window: None,
            max_output: None,
            pricing: Default::default(),
            capabilities: vec![],
            server_defaults: None,
            available: true,
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn groups_merge_same_family_and_keep_item_order() {
        // D01:Claude 两模型 + Gemini + 独立模型
        let catalog = vec![
            cat("google/gemini-2.0-flash", "Google: Gemini 2.0 Flash"),
            cat("anthropic/claude-opus-5.5", "Anthropic: Claude Opus 5.5"),
            cat("anthropic/claude-fable-5.1", "Anthropic: Claude Fable 5.1"),
            cat("weird/alpha-one", "Alpha One"),
        ];
        let enabled = ids(&[
            "anthropic/claude-opus-5.5",
            "google/gemini-2.0-flash",
            "anthropic/claude-fable-5.1",
            "weird/alpha-one",
        ]);
        let groups = build_groups(&enabled, &catalog);
        assert_eq!(groups.len(), 3, "{groups:?}");
        assert_eq!(groups[0].label, "Claude");
        assert_eq!(
            groups[0]
                .items
                .iter()
                .map(|i| i.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Opus 5.5", "Fable 5.1"]
        );
        assert_eq!(groups[1].label, "Gemini");
        assert_eq!(groups[2].label, "Alpha One"); // 单项独立组
        assert_eq!(groups[2].items[0].label, "Alpha One"); // 不显示空标签
    }

    #[test]
    fn search_matches_raw_id_family_and_short_name() {
        // D03
        let catalog = vec![
            cat("anthropic/claude-opus-5.5", "Anthropic: Claude Opus 5.5"),
            cat("openai/gpt-4o", "OpenAI: GPT-4o"),
            cat("weird/alpha-one", "Alpha One"),
        ];
        let enabled = ids(&[
            "anthropic/claude-opus-5.5",
            "openai/gpt-4o",
            "weird/alpha-one",
        ]);
        let groups = build_groups(&enabled, &catalog);

        let r = filter_groups(&groups, "opus");
        assert_eq!(r.len(), 1);
        let r = filter_groups(&groups, "GPT");
        assert_eq!(r.len(), 1);
        let r = filter_groups(&groups, "anthropic/");
        assert_eq!(r.len(), 1);
        let r = filter_groups(&groups, "不存在的模型");
        assert!(r.is_empty(), "无结果返回空");
        let r = filter_groups(&groups, "");
        assert_eq!(r.len(), groups.len());
    }

    #[test]
    fn many_single_family_groups_renderable() {
        // D04:30 个单项家族 → 30 组,组内单项
        let enabled: Vec<String> = (0..30).map(|i| format!("vendor{i}/model-x")).collect();
        let groups = build_groups(&enabled, &[]);
        assert_eq!(groups.len(), 30);
        assert!(groups.iter().all(|g| g.items.len() == 1));
    }

    #[test]
    fn display_only_selection_uses_raw_ids() {
        // D02:展示归一化不改变选择 ID
        let catalog = vec![cat("openai/gpt-4o", "OpenAI: GPT-4o")];
        let enabled = ids(&["openai/gpt-4o"]);
        let groups = build_groups(&enabled, &catalog);
        assert_eq!(groups[0].items[0].model_id, "openai/gpt-4o");
        assert_eq!(groups[0].items[0].label, "4o");
        assert_eq!(groups[0].items[0].full_label, "GPT 4o");
    }
}
