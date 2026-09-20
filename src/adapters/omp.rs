#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    io::{self, Read},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;
use wait_timeout::ChildExt;

use crate::{
    adapter::{AdapterError, AdapterErrorKind, ModelUsageAdapter, PlanAdapter},
    adapters::usage_common::UsageAccumulator,
    domain::{CodingPlan, ModelUsage, ModelUsageSnapshot, PlanIdentity, UsageStatus, UsageWindow},
};

const OMP_TIMEOUT: Duration = Duration::from_secs(15);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const SOURCE: &str = "OMP CLI";

fn adapter_error(kind: AdapterErrorKind) -> AdapterError {
    AdapterError::new(kind, SOURCE)
}

fn protocol_error() -> AdapterError {
    adapter_error(AdapterErrorKind::ProtocolChanged)
}

fn spawn_error(error: &io::Error) -> AdapterError {
    let kind = if error.kind() == io::ErrorKind::NotFound {
        AdapterErrorKind::CommandNotFound
    } else {
        AdapterErrorKind::ProtocolChanged
    };
    adapter_error(kind)
}

pub struct OmpCodexAdapter;

impl OmpCodexAdapter {
    /// `known` comes from a shared OMP scan so callers do not shell out
    /// once per provider.
    pub fn discover_with(known: Option<bool>) -> Option<Self> {
        known.filter(|known| *known).map(|_| Self)
    }
}

impl PlanAdapter for OmpCodexAdapter {
    fn identity(&self) -> PlanIdentity {
        PlanIdentity::new("openai-codex", "openai", "Codex")
    }

    fn fetch(&self) -> Result<CodingPlan, AdapterError> {
        let output = run_command(
            "omp",
            &["usage", "--provider", "openai-codex", "--json", "--redact"],
            OMP_TIMEOUT,
        )?;
        parse_provider_report(&output, "openai-codex", "openai", "Codex")
    }
}

pub struct OmpKimiAdapter;

impl OmpKimiAdapter {
    pub fn discover_with(known: Option<bool>) -> Option<Self> {
        known.filter(|known| *known).map(|_| Self)
    }
}

impl PlanAdapter for OmpKimiAdapter {
    fn identity(&self) -> PlanIdentity {
        PlanIdentity::new("kimi-code", "moonshot", "Kimi")
    }

    fn fetch(&self) -> Result<CodingPlan, AdapterError> {
        let output = run_command(
            "omp",
            &["usage", "--provider", "kimi-code", "--json", "--redact"],
            OMP_TIMEOUT,
        )?;
        parse_provider_report(&output, "kimi-code", "moonshot", "Kimi")
    }
}

pub struct OmpModelUsageAdapter;

impl OmpModelUsageAdapter {
    pub fn discover() -> Option<Self> {
        run_command("omp", &["--version"], DISCOVERY_TIMEOUT)
            .ok()
            .map(|_| Self)
    }
}

impl ModelUsageAdapter for OmpModelUsageAdapter {
    fn source_id(&self) -> &'static str {
        "omp"
    }

    fn fetch(&self) -> Result<ModelUsageSnapshot, AdapterError> {
        let output = run_command("omp", &["stats", "--json"], OMP_TIMEOUT)?;
        parse_model_stats(&output, SystemTime::now())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OmpStats {
    by_model: Vec<OmpModelStats>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OmpModelStats {
    model: String,
    provider: String,
    total_requests: u64,
    failed_requests: u64,
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_cache_read_tokens: u64,
    total_cache_write_tokens: u64,
    total_cost: f64,
    first_timestamp: u64,
    last_timestamp: u64,
}

fn parse_model_stats(
    input: &[u8],
    fetched_at: SystemTime,
) -> Result<ModelUsageSnapshot, AdapterError> {
    let json = if input.first() == Some(&b'{') {
        input
    } else {
        let start = input
            .windows(2)
            .position(|window| window == b"\n{")
            .map(|index| index + 1)
            .ok_or_else(protocol_error)?;
        &input[start..]
    };
    let stats: OmpStats = serde_json::from_slice(json).map_err(|_| protocol_error())?;
    let mut usage = UsageAccumulator::default();
    for model in stats.by_model {
        if model.model.is_empty() || model.provider.is_empty() {
            continue;
        }
        usage.add(ModelUsage {
            agent_id: "omp".to_owned(),
            agent_name: "OMP".to_owned(),
            provider_id: model.provider,
            model_id: model.model,
            requests: model.total_requests,
            failed_requests: model.failed_requests,
            input_tokens: model.total_input_tokens,
            output_tokens: model.total_output_tokens,
            cache_read_tokens: model.total_cache_read_tokens,
            cache_write_tokens: model.total_cache_write_tokens,
            cost_usd: model.total_cost.is_finite().then_some(model.total_cost),
            first_used_at: optional_timestamp_millis(model.first_timestamp),
            last_used_at: optional_timestamp_millis(model.last_timestamp),
        });
    }
    Ok(ModelUsageSnapshot {
        source_id: "omp".to_owned(),
        fetched_at,
        models: usage.into_models(),
    })
}

fn optional_timestamp_millis(value: u64) -> Option<SystemTime> {
    UNIX_EPOCH.checked_add(Duration::from_millis(value))
}

pub(crate) fn run_command(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<Vec<u8>, AdapterError> {
    let started = Instant::now();
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    command.process_group(0);

    let mut child = command.spawn().map_err(|error| spawn_error(&error))?;
    crate::child_process::track(child.id());
    let Some(stdout) = child.stdout.take() else {
        stop_child(&mut child);
        return Err(protocol_error());
    };
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut output = Vec::new();
        let result = stdout
            .take((MAX_OUTPUT_BYTES + 1) as u64)
            .read_to_end(&mut output)
            .map(|_| output)
            .map_err(|_| protocol_error());
        let _ = sender.send(result);
    });

    let status = match child.wait_timeout(timeout) {
        Ok(Some(status)) => status,
        Ok(None) | Err(_) => {
            stop_child(&mut child);
            return Err(adapter_error(AdapterErrorKind::TimedOut));
        }
    };
    // Reap inherited pipe holders too; never join a potentially blocked reader.
    stop_child(&mut child);
    let output = receiver
        .recv_timeout(timeout.saturating_sub(started.elapsed()))
        .map_err(|_| adapter_error(AdapterErrorKind::TimedOut))??;

    if !status.success() {
        return Err(adapter_error(AdapterErrorKind::NotAuthenticated));
    }
    if output.len() > MAX_OUTPUT_BYTES {
        return Err(protocol_error());
    }
    Ok(output)
}
fn stop_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    if let Ok(process_group) = i32::try_from(child.id()) {
        // SAFETY: a negative PID asks kill(2) to signal the child's process group.
        let _ = unsafe { libc::kill(-process_group, libc::SIGKILL) };
    }
    let _ = child.kill();
    let _ = child.wait();
    crate::child_process::forget(child.id());
}

#[derive(Deserialize)]
struct UsageResponse {
    reports: Vec<UsageReport>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageReport {
    fetched_at: u64,
    limits: Vec<UsageLimit>,
}

#[derive(Deserialize)]
struct UsageLimit {
    id: String,
    label: String,
    window: SourceWindow,
    amount: SourceAmount,
    status: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceWindow {
    duration_ms: Option<u64>,
    resets_at: Option<u64>,
}

#[derive(Deserialize)]
struct SourceAmount {
    remaining: u8,
}
/// Result of one `omp usage --json --redact` pass: which providers have
/// authenticated accounts, with redacted identity hints.
pub(crate) struct OmpUsageScan {
    pub reports: Vec<OmpScanReport>,
    pub accounts_without_usage: Vec<String>,
}

pub(crate) struct OmpScanReport {
    pub provider: String,
    pub account_hint: Option<String>,
}

impl OmpUsageScan {
    pub(crate) fn knows(&self, provider: &str) -> bool {
        self.reports
            .iter()
            .any(|report| report.provider == provider)
            || self
                .accounts_without_usage
                .iter()
                .any(|candidate| candidate == provider)
    }

    pub(crate) fn account_hint(&self, provider: &str) -> Option<String> {
        self.reports
            .iter()
            .find(|report| report.provider == provider)
            .and_then(|report| report.account_hint.clone())
            .map(|hint| format!("account {hint}"))
    }
}

/// `Some(true)` when OMP reports an authenticated account for the provider.
/// The unfiltered usage scan omits providers without usage support, so
/// per-provider probes are the only way to see them.
pub(crate) fn provider_known(provider: &str) -> Option<bool> {
    let output = run_command(
        "omp",
        &["usage", "--provider", provider, "--json", "--redact"],
        OMP_TIMEOUT,
    )
    .ok()?;
    let scan: ProviderScan = serde_json::from_slice(&output).ok()?;
    Some(
        !scan.reports.is_empty()
            || scan
                .accounts_without_usage
                .iter()
                .any(|account| account.provider == provider),
    )
}

#[derive(Deserialize)]
struct ProviderScan {
    reports: Vec<ProviderScanReport>,
    #[serde(rename = "accountsWithoutUsage")]
    accounts_without_usage: Vec<ProviderScanAccount>,
}

#[derive(Deserialize)]
struct ProviderScanReport {
    provider: String,
    metadata: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Deserialize)]
struct ProviderScanAccount {
    provider: String,
}

/// Ask OMP which providers have authenticated accounts. Redacted output
/// keeps identity hints shareable; credential material is never touched.
pub(crate) fn scan() -> Option<OmpUsageScan> {
    let output = run_command("omp", &["usage", "--json", "--redact"], OMP_TIMEOUT).ok()?;
    let scan: ProviderScan = serde_json::from_slice(&output).ok()?;
    Some(OmpUsageScan {
        reports: scan
            .reports
            .into_iter()
            .map(|report| {
                let hint = report
                    .metadata
                    .as_ref()
                    .and_then(|metadata| {
                        metadata.get("accountId").or_else(|| metadata.get("email"))
                    })
                    .and_then(|value| value.as_str())
                    .map(str::to_owned);
                OmpScanReport {
                    provider: report.provider,
                    account_hint: hint,
                }
            })
            .collect(),
        accounts_without_usage: scan
            .accounts_without_usage
            .into_iter()
            .map(|account| account.provider)
            .collect(),
    })
}

pub(crate) fn parse_provider_report(
    input: &[u8],
    id: &str,
    provider_id: &str,
    display_name: &str,
) -> Result<CodingPlan, AdapterError> {
    let response: UsageResponse = serde_json::from_slice(input).map_err(|_| protocol_error())?;
    let report = response
        .reports
        .into_iter()
        .next()
        .ok_or_else(protocol_error)?;
    let fetched_at = timestamp_millis(report.fetched_at)?;
    let windows = report
        .limits
        .into_iter()
        .map(|limit| {
            let period = limit
                .window
                .duration_ms
                .map(Duration::from_millis)
                .or_else(|| codex_period(&limit.id));
            UsageWindow {
                period,
                id: limit.id,
                label: limit.label,
                remaining_percent: limit.amount.remaining.min(100),
                resets_at: limit
                    .window
                    .resets_at
                    .and_then(|value| timestamp_millis(value).ok()),
                status: if limit.status == "ok" {
                    UsageStatus::Available
                } else {
                    UsageStatus::Unavailable
                },
            }
        })
        .collect::<Vec<_>>();

    if windows.is_empty() {
        return Err(protocol_error());
    }

    Ok(CodingPlan {
        id: id.to_owned(),
        provider_id: provider_id.to_owned(),
        display_name: display_name.to_owned(),
        fetched_at,
        windows,
    })
}

fn codex_period(id: &str) -> Option<Duration> {
    match id {
        "openai-codex:primary" | "openai-codex:spark:secondary" => {
            Some(Duration::from_secs(7 * 24 * 60 * 60))
        }
        "openai-codex:spark:primary" => Some(Duration::from_secs(5 * 60 * 60)),
        _ => None,
    }
}

fn timestamp_millis(milliseconds: u64) -> Result<std::time::SystemTime, AdapterError> {
    UNIX_EPOCH
        .checked_add(Duration::from_millis(milliseconds))
        .ok_or_else(protocol_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    const USAGE_FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/openai_codex_usage.json");

    #[test]
    fn parses_only_the_usage_contract() {
        let plan = parse_provider_report(USAGE_FIXTURE, "openai-codex", "openai", "Codex")
            .expect("fixture should match the contract");

        assert_eq!(plan.id, "openai-codex");
        assert_eq!(plan.provider_id, "openai");
        assert_eq!(plan.windows.len(), 3);
        assert_eq!(plan.windows[0].remaining_percent, 76);
        assert_eq!(
            plan.windows[1].period,
            Some(Duration::from_secs(5 * 60 * 60))
        );

        let retained_data = format!("{plan:?}");
        for forbidden in [
            "private@example.com",
            "acct-secret",
            "org-secret",
            "Private Organization",
            "secret-plan",
            "Plus",
        ] {
            assert!(!retained_data.contains(forbidden));
        }
    }

    #[test]
    fn parses_kimi_usage_report_with_duration_windows() {
        let fixture = include_bytes!("../../tests/fixtures/kimi_usage.json");
        let plan = parse_provider_report(fixture, "kimi-code", "moonshot", "Kimi")
            .expect("kimi fixture should match the contract");

        assert_eq!(plan.id, "kimi-code");
        assert_eq!(plan.provider_id, "moonshot");
        assert_eq!(plan.windows.len(), 1);
        assert_eq!(plan.windows[0].label, "5h limit");
        assert_eq!(
            plan.windows[0].period,
            Some(Duration::from_secs(5 * 60 * 60))
        );
        assert_eq!(plan.windows[0].remaining_percent, 0);
        assert_eq!(plan.windows[0].status, UsageStatus::Unavailable);
        assert_eq!(
            plan.windows[0].resets_at,
            Some(UNIX_EPOCH + Duration::from_millis(1789869864643))
        );
    }

    #[test]
    fn model_stats_preserve_model_attribution_and_token_kinds() {
        let snapshot = parse_model_stats(
            b"Syncing session files...\n{\"byModel\":[{\"model\":\"gpt-test\",\"provider\":\"openai\",\"totalRequests\":3,\"failedRequests\":1,\"totalInputTokens\":100,\"totalOutputTokens\":20,\"totalCacheReadTokens\":80,\"totalCacheWriteTokens\":4,\"totalCost\":1.25,\"firstTimestamp\":1000,\"lastTimestamp\":2000}]}",
            UNIX_EPOCH,
        )
        .expect("OMP stats should parse");

        assert_eq!(snapshot.models.len(), 1);
        let usage = &snapshot.models[0];
        assert_eq!(
            (usage.agent_id.as_str(), usage.model_id.as_str()),
            ("omp", "gpt-test")
        );
        assert_eq!((usage.requests, usage.failed_requests), (3, 1));
        assert_eq!((usage.input_tokens, usage.output_tokens), (100, 20));
        assert_eq!((usage.cache_read_tokens, usage.cache_write_tokens), (80, 4));
        assert_eq!(usage.cost_usd, Some(1.25));
    }
    #[cfg(unix)]
    #[test]
    fn command_timeout_is_bounded() {
        let started = std::time::Instant::now();
        let result = run_command("/bin/sh", &["-c", "sleep 2"], Duration::from_millis(25));

        assert_eq!(result, Err(adapter_error(AdapterErrorKind::TimedOut)));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn command_failures_have_actionable_categories() {
        let missing = run_command("/definitely/missing/omp", &[], Duration::from_millis(25));
        let logged_out = run_command("/bin/sh", &["-c", "exit 3"], Duration::from_secs(1));

        assert_eq!(
            missing,
            Err(adapter_error(AdapterErrorKind::CommandNotFound))
        );
        assert_eq!(
            logged_out,
            Err(adapter_error(AdapterErrorKind::NotAuthenticated))
        );
    }
}
