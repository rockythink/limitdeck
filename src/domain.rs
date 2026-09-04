use std::time::{Duration, SystemTime};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanIdentity {
    pub id: String,
    pub provider_id: String,
    pub display_name: String,
}

impl PlanIdentity {
    pub fn new(
        id: impl Into<String>,
        provider_id: impl Into<String>,
        display_name: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            provider_id: provider_id.into(),
            display_name: display_name.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsageStatus {
    Available,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageWindow {
    pub id: String,
    pub label: String,
    pub period: Option<Duration>,
    pub remaining_percent: u8,
    pub resets_at: Option<SystemTime>,
    pub status: UsageStatus,
}

impl UsageWindow {
    pub fn remaining_time_percent_at(&self, now: SystemTime) -> Option<u8> {
        let period = self.period?;
        let resets_at = self.resets_at?;
        if period.is_zero() {
            return None;
        }

        let remaining = resets_at.duration_since(now).unwrap_or(Duration::ZERO);
        let period_nanos = period.as_nanos();
        let rounded = remaining
            .as_nanos()
            .saturating_mul(100)
            .saturating_add(period_nanos / 2)
            / period_nanos;
        Some(rounded.min(100) as u8)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodingPlan {
    pub id: String,
    pub provider_id: String,
    pub display_name: String,
    pub fetched_at: SystemTime,
    pub windows: Vec<UsageWindow>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ModelUsage {
    pub agent_id: String,
    pub agent_name: String,
    pub provider_id: String,
    pub model_id: String,
    pub requests: u64,
    pub failed_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: Option<f64>,
    pub first_used_at: Option<SystemTime>,
    pub last_used_at: Option<SystemTime>,
}

impl ModelUsage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }

    pub fn merge(&mut self, other: &Self) {
        debug_assert_eq!(self.agent_id, other.agent_id);
        debug_assert_eq!(self.provider_id, other.provider_id);
        debug_assert_eq!(self.model_id, other.model_id);
        self.requests = self.requests.saturating_add(other.requests);
        self.failed_requests = self.failed_requests.saturating_add(other.failed_requests);
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(other.cache_write_tokens);
        self.cost_usd = match (self.cost_usd, other.cost_usd) {
            (Some(left), Some(right)) => Some(left + right),
            (left, right) => left.or(right),
        };
        self.first_used_at = earliest(self.first_used_at, other.first_used_at);
        self.last_used_at = latest(self.last_used_at, other.last_used_at);
    }
}

fn earliest(left: Option<SystemTime>, right: Option<SystemTime>) -> Option<SystemTime> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    }
}

fn latest(left: Option<SystemTime>, right: Option<SystemTime>) -> Option<SystemTime> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (left, right) => left.or(right),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelUsageSnapshot {
    pub source_id: String,
    pub fetched_at: SystemTime,
    pub models: Vec<ModelUsage>,
}

impl CodingPlan {
    pub fn is_older_than(&self, max_age: Duration) -> bool {
        SystemTime::now()
            .duration_since(self.fetched_at)
            .map(|age| age > max_age)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(period: Option<Duration>, resets_at: Option<SystemTime>) -> UsageWindow {
        UsageWindow {
            id: "test".to_owned(),
            label: "Test".to_owned(),
            period,
            remaining_percent: 50,
            resets_at,
            status: UsageStatus::Available,
        }
    }

    #[test]
    fn remaining_time_percent_matches_the_same_remaining_scale_as_quota() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let period = Duration::from_secs(100);
        assert_eq!(
            window(Some(period), Some(now + Duration::from_secs(50)))
                .remaining_time_percent_at(now),
            Some(50)
        );
        assert_eq!(
            window(Some(period), Some(now - Duration::from_secs(1))).remaining_time_percent_at(now),
            Some(0)
        );
        assert_eq!(
            window(Some(period), Some(now + Duration::from_secs(120)))
                .remaining_time_percent_at(now),
            Some(100)
        );
    }

    #[test]
    fn remaining_time_percent_requires_a_nonzero_period_and_reset_time() {
        let now = SystemTime::UNIX_EPOCH;
        assert_eq!(window(None, Some(now)).remaining_time_percent_at(now), None);
        assert_eq!(
            window(Some(Duration::from_secs(1)), None).remaining_time_percent_at(now),
            None
        );
        assert_eq!(
            window(Some(Duration::ZERO), Some(now)).remaining_time_percent_at(now),
            None
        );
    }
}
