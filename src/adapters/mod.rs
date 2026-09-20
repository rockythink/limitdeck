mod aider;
mod claude;
mod codex;
mod codex_usage;
mod gemini;
mod omp;
mod opencode;
mod pi;
mod usage_common;
mod zhipu;

use std::{collections::HashSet, thread};

use crate::adapter::{ModelUsageAdapter, PlanAdapter};

pub use aider::AiderModelUsageAdapter;
pub use claude::{
    ingest_stdin as ingest_claude_statusline, ClaudeModelUsageAdapter, ClaudeStatuslineAdapter,
};
pub use codex::CodexAppServerAdapter;
pub use codex_usage::CodexModelUsageAdapter;
pub use gemini::GeminiModelUsageAdapter;
pub use omp::{OmpCodexAdapter, OmpKimiAdapter, OmpModelUsageAdapter};
pub use opencode::OpenCodeModelUsageAdapter;
pub use pi::PiModelUsageAdapter;
pub use zhipu::ZhipuCodingPlanAdapter;

/// One subscription source found on this machine during a discovery scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryEntry {
    pub label: String,
    pub provider_id: String,
    pub source: &'static str,
    pub monitored: bool,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiscoveryReport {
    pub entries: Vec<DiscoveryEntry>,
}

pub fn discover() -> Vec<Box<dyn PlanAdapter>> {
    discover_with_report().0
}

pub fn discover_with_report() -> (Vec<Box<dyn PlanAdapter>>, DiscoveryReport) {
    let mut discovered: Vec<Box<dyn PlanAdapter>> = Vec::new();
    let mut entries: Vec<DiscoveryEntry> = Vec::new();
    let settings_endpoints = claude_settings_base_urls();
    let settings_has = |fragment: &str| {
        settings_endpoints
            .iter()
            .any(|(url, ..)| url.contains(fragment))
    };

    if let Some(adapter) = ClaudeStatuslineAdapter::discover() {
        discovered.push(Box::new(adapter));
        entries.push(DiscoveryEntry {
            label: "Claude".to_owned(),
            provider_id: "anthropic".to_owned(),
            source: "Claude Code status-line cache",
            monitored: true,
            note: None,
        });
    }

    // Probe the Codex CLI and the OMP account list concurrently; both are
    // shell-outs with their own bounded timeouts.
    let omp_scan = thread::scope(|scope| {
        let codex_probe = scope.spawn(CodexAppServerAdapter::discover);
        let scan = omp::scan();
        let codex_cli = codex_probe.join().ok().flatten().is_some();
        (scan, codex_cli)
    });
    let (omp_scan, codex_cli) = omp_scan;
    if codex_cli {
        discovered.push(Box::new(CodexAppServerAdapter));
    }
    let omp_codex_known = omp_scan.as_ref().map(|scan| scan.knows("openai-codex"));
    if !codex_cli {
        // OMP usage resolves only authenticated accounts; never inspect its credentials.
        if let Some(adapter) = OmpCodexAdapter::discover_with(omp_codex_known) {
            discovered.push(Box::new(adapter));
        }
    }
    if codex_cli || omp_codex_known.unwrap_or(false) {
        entries.push(DiscoveryEntry {
            label: "Codex".to_owned(),
            provider_id: "openai".to_owned(),
            source: if codex_cli {
                "Codex CLI login"
            } else {
                "OMP account"
            },
            monitored: true,
            note: omp_scan
                .as_ref()
                .and_then(|scan| scan.account_hint("openai-codex")),
        });
    }

    let kimi_known = omp_scan.as_ref().map(|scan| scan.knows("kimi-code"));
    let kimi_in_settings = settings_has("kimi.com") || settings_has("moonshot.cn");
    if let Some(adapter) = OmpKimiAdapter::discover_with(kimi_known) {
        discovered.push(Box::new(adapter));
    }
    if kimi_known.unwrap_or(false) || kimi_in_settings {
        entries.push(DiscoveryEntry {
            label: "Kimi".to_owned(),
            provider_id: "moonshot".to_owned(),
            source: if kimi_known.unwrap_or(false) {
                "OMP account"
            } else {
                "Claude Code settings"
            },
            monitored: kimi_known.unwrap_or(false),
            note: omp_scan
                .as_ref()
                .and_then(|scan| scan.account_hint("kimi-code")),
        });
    }

    let zhipu_omp_known = omp_scan
        .as_ref()
        .map(|scan| scan.knows("zhipu-coding-plan"))
        .filter(|known| *known)
        .or_else(|| omp::provider_known("zhipu-coding-plan"));
    let zhipu_monitored = ZhipuCodingPlanAdapter::discover_with(zhipu_omp_known);
    if let Some(adapter) = zhipu_monitored {
        discovered.push(Box::new(adapter));
        entries.push(DiscoveryEntry {
            label: "GLM Coding Plan".to_owned(),
            provider_id: "zhipu".to_owned(),
            source: if zhipu_omp_known.unwrap_or(false) {
                "OMP credential"
            } else {
                "Claude settings or environment"
            },
            monitored: true,
            note: None,
        });
    }

    for (base_url, label, provider_id, source) in settings_endpoints {
        if provider_id == "zhipu" || entries.iter().any(|entry| entry.label == label) {
            continue;
        }
        entries.push(DiscoveryEntry {
            label: label.to_owned(),
            provider_id: provider_id.to_owned(),
            source,
            monitored: false,
            note: Some(base_url),
        });
    }

    // One card per plan identity; later sources yield to earlier ones.
    let mut seen = HashSet::new();
    discovered.retain(|adapter| seen.insert(adapter.identity().id));

    (discovered, DiscoveryReport { entries })
}

/// Anthropic-compatible endpoints configured in Claude Code settings.
/// Reveals subscriptions LimitDeck cannot monitor yet; returned as
/// `(base_url, label, provider_id, source)` tuples.
fn claude_settings_base_urls() -> Vec<(String, &'static str, &'static str, &'static str)> {
    let mut results = Vec::new();
    let Some(home) = std::env::var_os("HOME") else {
        return results;
    };
    let base = std::path::PathBuf::from(home).join(".claude");
    for name in ["settings.json", "settings.local.json"] {
        let Ok(content) = std::fs::read_to_string(base.join(name)) else {
            continue;
        };
        let Ok(settings) = serde_json::from_str::<serde_json::Value>(&content) else {
            continue;
        };
        let Some(base_url) = settings
            .get("env")
            .and_then(|env| env.get("ANTHROPIC_BASE_URL"))
            .and_then(|value| value.as_str())
        else {
            continue;
        };
        let classified = if base_url.contains("volces.com") {
            ("Volcano Ark Coding Plan", "volcengine")
        } else if base_url.contains("kimi.com") || base_url.contains("moonshot.cn") {
            ("Kimi", "moonshot")
        } else if base_url.contains("api.z.ai")
            || base_url.contains("open.bigmodel.cn")
            || base_url.contains("dev.bigmodel.cn")
        {
            ("GLM Coding Plan", "zhipu")
        } else {
            ("Anthropic-compatible endpoint", "anthropic")
        };
        results.push((
            base_url.to_owned(),
            classified.0,
            classified.1,
            "Claude Code settings",
        ));
    }
    results
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_report_never_leaks_credentials_and_keeps_labels_unique() {
        let (_, report) = discover_with_report();

        let serialized = format!("{report:?}");
        for forbidden in ["ark-c45157f5", "28cf574df6f04b53", "ANTHROPIC_AUTH_TOKEN"] {
            assert!(!serialized.contains(forbidden));
        }
        let mut labels = report
            .entries
            .iter()
            .map(|entry| entry.label.as_str())
            .collect::<Vec<_>>();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(
            labels.len(),
            report.entries.len(),
            "duplicate labels in {report:?}"
        );
    }
}
