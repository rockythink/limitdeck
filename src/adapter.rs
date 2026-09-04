use crate::domain::{CodingPlan, ModelUsageSnapshot, PlanIdentity};

pub trait PlanAdapter: Send + Sync + 'static {
    fn identity(&self) -> PlanIdentity;
    fn fetch(&self) -> Result<CodingPlan, AdapterError>;
}
pub trait ModelUsageAdapter: Send + Sync + 'static {
    fn source_id(&self) -> &'static str;
    fn fetch(&self) -> Result<ModelUsageSnapshot, AdapterError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterErrorKind {
    CommandNotFound,
    NotAuthenticated,
    TimedOut,
    ProtocolChanged,
    SnapshotMissing,
    SnapshotExpired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdapterError {
    pub kind: AdapterErrorKind,
    pub source: &'static str,
}

impl AdapterError {
    pub const fn new(kind: AdapterErrorKind, source: &'static str) -> Self {
        Self { kind, source }
    }
}
