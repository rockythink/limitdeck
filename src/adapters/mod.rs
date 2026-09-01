mod claude;
mod codex;
mod omp;

use std::collections::HashSet;

use crate::adapter::PlanAdapter;

pub use claude::ingest_stdin as ingest_claude_statusline;
pub use claude::ClaudeStatuslineAdapter;
pub use codex::CodexAppServerAdapter;
pub use omp::OmpCodexAdapter;

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
