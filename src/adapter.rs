use crate::domain::{CodingPlan, PlanIdentity};

pub trait PlanAdapter: Send + Sync + 'static {
    fn identity(&self) -> PlanIdentity;
    fn fetch(&self) -> Result<CodingPlan, AdapterError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdapterError;
