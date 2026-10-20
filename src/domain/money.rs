//! 金额与用量(PLAN 3.1 / 4.4)。
//!
//! - USD 金额用 `rust_decimal::Decimal`(十进制定点),禁止浮点累计。
//! - 实际费用与估算分别保存;`Known` 可合法为零,未知绝不能显示为零。
//! - request_id 唯一去重;同一请求重复入账只记一笔。

use chrono::{DateTime, Utc};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};

use super::ids::GenerationId;

/// Format a USD amount for compact UI display, rounding only after all precise
/// ledger values have been summed.
pub fn format_usd(value: Decimal) -> String {
    let rounded = value.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
    format!("{rounded:.2}")
}

/// 模型价格快照(USD / 每百万 token;None = 未知,不能当零)。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Pricing {
    pub prompt_per_mtok: Option<Decimal>,
    pub completion_per_mtok: Option<Decimal>,
    /// 已知按次费用(如每请求附加费)。
    pub request_fee: Option<Decimal>,
}

impl Pricing {
    /// 逐请求预估:`输入 token 估算 × 输入单价 + 假定输出 token × 输出单价 + 已知按次费用`。
    /// 假定输出长度与实际 max_tokens 分离(默认 1000,调用方传入)。
    /// 任一价格缺失时返回 None(调用方显示“无法估算”,不显示零)。
    pub fn estimate(&self, prompt_tokens: u64, assumed_completion_tokens: u64) -> Option<Decimal> {
        let input = self.prompt_per_mtok?;
        let output = self.completion_per_mtok?;
        let million = Decimal::from(1_000_000u64);
        let mut total = Decimal::from(prompt_tokens) * input / million
            + Decimal::from(assumed_completion_tokens) * output / million;
        if let Some(fee) = self.request_fee {
            total += fee;
        }
        Some(total)
    }

    /// 已知部分和:仅当存在至少一个价格时返回。
    pub fn has_any_price(&self) -> bool {
        self.prompt_per_mtok.is_some()
            || self.completion_per_mtok.is_some()
            || self.request_fee.is_some()
    }
}

/// 用量记录状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UsageStatus {
    /// 已结束但费用尚未取回(等待 usage 事件或补查)。
    #[default]
    Pending,
    /// 费用已知(可为合法的零,如免费模型)。
    Known,
    /// 无法查询(无远端 ID / 提供方不支持 / 权限不足)。
    Unavailable,
}

impl UsageStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Known => "known",
            Self::Unavailable => "unavailable",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "known" => Self::Known,
            "unavailable" => Self::Unavailable,
            _ => return None,
        })
    }
}

/// 实际费用来源(估算不写入 UsageRecord,仅用于预估展示)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostSource {
    /// API 响应内联 usage/cost(优先来源)。
    ApiReported,
    /// generation 补查接口返回。
    GenerationQuery,
}

impl CostSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ApiReported => "api",
            Self::GenerationQuery => "generation_query",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "api" => Self::ApiReported,
            "generation_query" => Self::GenerationQuery,
            _ => return None,
        })
    }
}

/// 单次请求的用量与费用账目(request_id 唯一)。
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRecord {
    pub request_id: GenerationId,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    /// 实际费用(USD);Known 状态下必有。
    pub cost: Option<Decimal>,
    pub cost_source: Option<CostSource>,
    pub status: UsageStatus,
    pub remote_generation_id: Option<String>,
    pub queried_at: Option<DateTime<Utc>>,
}

impl UsageRecord {
    pub fn pending(request_id: GenerationId) -> Self {
        Self {
            request_id,
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            cost: None,
            cost_source: None,
            status: UsageStatus::Pending,
            remote_generation_id: None,
            queried_at: None,
        }
    }

    /// 入账(幂等:同一请求只保留一笔)。已知数据不回退为未知。
    pub fn merge_known(
        &mut self,
        prompt_tokens: Option<u64>,
        completion_tokens: Option<u64>,
        cost: Option<Decimal>,
        source: CostSource,
        remote_generation_id: Option<String>,
        now: DateTime<Utc>,
    ) {
        if self.status == UsageStatus::Known && cost.is_none() {
            // 已有实际费用,迟到的部分数据不覆盖
            self.prompt_tokens = self.prompt_tokens.or(prompt_tokens);
            self.completion_tokens = self.completion_tokens.or(completion_tokens);
            return;
        }
        self.prompt_tokens = prompt_tokens.or(self.prompt_tokens);
        self.completion_tokens = completion_tokens.or(self.completion_tokens);
        if let Some(c) = cost {
            self.cost = Some(c);
            self.cost_source = Some(source);
            self.status = UsageStatus::Known;
        }
        if remote_generation_id.is_some() {
            self.remote_generation_id = remote_generation_id;
        }
        self.queried_at = Some(now);
    }

    pub fn mark_unavailable(&mut self, now: DateTime<Utc>) {
        if self.status == UsageStatus::Pending {
            self.status = UsageStatus::Unavailable;
            self.queried_at = Some(now);
        }
    }
}

/// 会话累计:按请求去重求和;区分“已知合计”与“未知请求数”。
#[derive(Debug, Clone, Default)]
pub struct CostSummary {
    pub known_total: Decimal,
    pub pending_count: usize,
    pub unavailable_count: usize,
}

impl CostSummary {
    pub fn from_records<'a>(records: impl Iterator<Item = &'a UsageRecord>) -> Self {
        let mut s = Self {
            known_total: Decimal::ZERO,
            ..Default::default()
        };
        for r in records {
            match r.status {
                UsageStatus::Known => s.known_total += r.cost.unwrap_or(Decimal::ZERO),
                UsageStatus::Pending => s.pending_count += 1,
                UsageStatus::Unavailable => s.unavailable_count += 1,
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Decimal {
        use std::str::FromStr;
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn estimate_basic() {
        let p = Pricing {
            prompt_per_mtok: Some(d("1.50")),
            completion_per_mtok: Some(d("3.00")),
            request_fee: None,
        };
        // 1000 prompt + 1000 assumed completion = 1.5*0.001 + 3*0.001
        let e = p.estimate(1000, 1000).unwrap();
        assert_eq!(e, d("0.0045"));
    }

    #[test]
    fn estimate_missing_price_is_none() {
        let p = Pricing {
            prompt_per_mtok: None,
            completion_per_mtok: Some(d("1")),
            request_fee: None,
        };
        // 输入价缺失:整体无法估算,但"存在部分价格"为真
        assert!(p.estimate(100, 100).is_none());
        assert!(p.has_any_price());
        let empty = Pricing::default();
        assert!(!empty.has_any_price());
    }

    #[test]
    fn usage_merge_dedup_and_no_regression() {
        let mut r = UsageRecord::pending(GenerationId::new());
        let now = Utc::now();
        r.merge_known(
            Some(10),
            Some(20),
            Some(d("0.01")),
            CostSource::ApiReported,
            Some("gen1".into()),
            now,
        );
        assert_eq!(r.status, UsageStatus::Known);
        // 重复报告同一请求:只记一笔,迟到数据不覆盖
        r.merge_known(None, None, None, CostSource::GenerationQuery, None, now);
        assert_eq!(r.cost, Some(d("0.01")));
        assert_eq!(r.prompt_tokens, Some(10));
        // 已知后不再变未知
        r.mark_unavailable(now);
        assert_eq!(r.status, UsageStatus::Known);
    }

    #[test]
    fn zero_is_known_not_unknown() {
        let mut r = UsageRecord::pending(GenerationId::new());
        r.merge_known(
            Some(1),
            Some(1),
            Some(Decimal::ZERO),
            CostSource::ApiReported,
            None,
            Utc::now(),
        );
        assert_eq!(r.status, UsageStatus::Known);
        assert_eq!(r.cost, Some(Decimal::ZERO));
    }

    #[test]
    fn cost_summary_counts() {
        let mut a = UsageRecord::pending(GenerationId::new());
        a.merge_known(
            Some(1),
            Some(1),
            Some(d("0.10")),
            CostSource::ApiReported,
            None,
            Utc::now(),
        );
        let b = UsageRecord::pending(GenerationId::new());
        let mut c = UsageRecord::pending(GenerationId::new());
        c.mark_unavailable(Utc::now());
        let s = CostSummary::from_records([&a, &b, &c].into_iter());
        assert_eq!(s.known_total, d("0.10"));
        assert_eq!(s.pending_count, 1);
        assert_eq!(s.unavailable_count, 1);
    }

    #[test]
    fn usd_display_rounds_to_two_places_without_changing_ledger_precision() {
        assert_eq!(format_usd(d("0.0049")), "0.00");
        assert_eq!(format_usd(d("0.1250")), "0.13");
        assert_eq!(format_usd(d("1.2")), "1.20");
        assert_eq!(format_usd(d("-0.005")), "-0.01");
        assert_eq!(d("0.0049"), Decimal::new(49, 4));
    }
}
