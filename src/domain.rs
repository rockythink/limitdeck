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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodingPlan {
    pub id: String,
    pub provider_id: String,
    pub display_name: String,
    pub fetched_at: SystemTime,
    pub windows: Vec<UsageWindow>,
}

impl CodingPlan {
    pub fn is_older_than(&self, max_age: Duration) -> bool {
        SystemTime::now()
            .duration_since(self.fetched_at)
            .map(|age| age > max_age)
            .unwrap_or(false)
    }
}
