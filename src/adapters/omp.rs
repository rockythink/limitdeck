use std::{
    io::Read,
    process::{Command, Stdio},
    thread,
    time::{Duration, UNIX_EPOCH},
};

use serde::Deserialize;
use wait_timeout::ChildExt;

use crate::{
    adapter::{AdapterError, PlanAdapter},
    domain::{CodingPlan, PlanIdentity, UsageStatus, UsageWindow},
};

const OMP_TIMEOUT: Duration = Duration::from_secs(15);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;

pub struct OmpCodexAdapter;

impl OmpCodexAdapter {
    pub fn discover() -> Option<Self> {
        run_command("omp", &["--version"], DISCOVERY_TIMEOUT)
            .ok()
            .map(|_| Self)
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
        parse_openai_codex(&output)
    }
}

fn run_command(program: &str, args: &[&str], timeout: Duration) -> Result<Vec<u8>, AdapterError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| AdapterError)?;
    let Some(stdout) = child.stdout.take() else {
        stop_child(&mut child);
        return Err(AdapterError);
    };
    let output_reader = thread::spawn(move || {
        let mut output = Vec::new();
        stdout
            .take((MAX_OUTPUT_BYTES + 1) as u64)
            .read_to_end(&mut output)
            .map(|_| output)
            .map_err(|_| AdapterError)
    });

    let status = match child.wait_timeout(timeout) {
        Ok(Some(status)) => status,
        Ok(None) | Err(_) => {
            stop_child(&mut child);
            let _ = output_reader.join();
            return Err(AdapterError);
        }
    };
    let output = output_reader.join().map_err(|_| AdapterError)??;

    if !status.success() || output.len() > MAX_OUTPUT_BYTES {
        return Err(AdapterError);
    }
    Ok(output)
}
fn stop_child(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
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
    resets_at: Option<u64>,
}

#[derive(Deserialize)]
struct SourceAmount {
    remaining: u8,
}

pub(crate) fn parse_openai_codex(input: &[u8]) -> Result<CodingPlan, AdapterError> {
    let response: UsageResponse = serde_json::from_slice(input).map_err(|_| AdapterError)?;
    let report = response.reports.into_iter().next().ok_or(AdapterError)?;
    let fetched_at = timestamp_millis(report.fetched_at)?;
    let windows = report
        .limits
        .into_iter()
        .map(|limit| UsageWindow {
            period: codex_period(&limit.id),
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
        })
        .collect::<Vec<_>>();

    if windows.is_empty() {
        return Err(AdapterError);
    }

    Ok(CodingPlan {
        id: "openai-codex".to_owned(),
        provider_id: "openai".to_owned(),
        display_name: "Codex".to_owned(),
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
        .ok_or(AdapterError)
}

#[cfg(test)]
mod tests {
    use super::*;

    const USAGE_FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/openai_codex_usage.json");

    #[test]
    fn parses_only_the_usage_contract() {
        let plan = parse_openai_codex(USAGE_FIXTURE).expect("fixture should match the contract");

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

    #[cfg(unix)]
    #[test]
    fn command_timeout_is_bounded() {
        let started = std::time::Instant::now();
        let result = run_command("/bin/sh", &["-c", "sleep 2"], Duration::from_millis(25));

        assert_eq!(result, Err(AdapterError));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
