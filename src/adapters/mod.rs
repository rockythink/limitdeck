mod aider;
mod claude;
mod codex;
mod codex_usage;
mod gemini;
mod omp;
mod opencode;
mod pi;
mod usage_common;

use std::collections::HashSet;

use crate::adapter::{ModelUsageAdapter, PlanAdapter};

pub use aider::AiderModelUsageAdapter;
pub use claude::{
    ingest_stdin as ingest_claude_statusline, ClaudeModelUsageAdapter, ClaudeStatuslineAdapter,
};
pub use codex::CodexAppServerAdapter;
pub use codex_usage::CodexModelUsageAdapter;
pub use gemini::GeminiModelUsageAdapter;
pub use omp::{OmpCodexAdapter, OmpModelUsageAdapter};
pub use opencode::OpenCodeModelUsageAdapter;
pub use pi::PiModelUsageAdapter;

pub fn discover() -> Vec<Box<dyn PlanAdapter>> {
    let candidates: [Option<Box<dyn PlanAdapter>>; 3] = [
        ClaudeStatuslineAdapter::discover()
            .map(|adapter| Box::new(adapter) as Box<dyn PlanAdapter>),
        CodexAppServerAdapter::discover().map(|adapter| Box::new(adapter) as Box<dyn PlanAdapter>),
        OmpCodexAdapter::discover().map(|adapter| Box::new(adapter) as Box<dyn PlanAdapter>),
    ];
    let mut discovered = Vec::new();
    let mut ids = HashSet::new();
    for adapter in candidates.into_iter().flatten() {
        if ids.insert(adapter.identity().id) {
            discovered.push(adapter);
        }
    }
    discovered
}
pub fn discover_model_usage() -> Vec<Box<dyn ModelUsageAdapter>> {
    let candidates: [Option<Box<dyn ModelUsageAdapter>>; 7] = [
        OmpModelUsageAdapter::discover()
            .map(|adapter| Box::new(adapter) as Box<dyn ModelUsageAdapter>),
        ClaudeModelUsageAdapter::discover()
            .map(|adapter| Box::new(adapter) as Box<dyn ModelUsageAdapter>),
        CodexModelUsageAdapter::discover()
            .map(|adapter| Box::new(adapter) as Box<dyn ModelUsageAdapter>),
        GeminiModelUsageAdapter::discover()
            .map(|adapter| Box::new(adapter) as Box<dyn ModelUsageAdapter>),
        OpenCodeModelUsageAdapter::discover()
            .map(|adapter| Box::new(adapter) as Box<dyn ModelUsageAdapter>),
        PiModelUsageAdapter::discover()
            .map(|adapter| Box::new(adapter) as Box<dyn ModelUsageAdapter>),
        AiderModelUsageAdapter::discover()
            .map(|adapter| Box::new(adapter) as Box<dyn ModelUsageAdapter>),
    ];
    candidates.into_iter().flatten().collect()
}
