use crate::domain::{CodingPlan, PlanIdentity};

pub trait PlanAdapter: Send + Sync + 'static {
    fn identity(&self) -> PlanIdentity;
    fn fetch(&self) -> Result<CodingPlan, AdapterError>;
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
