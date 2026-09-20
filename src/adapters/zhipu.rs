use std::{
    env, fs,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;

use crate::{
    adapter::{AdapterError, AdapterErrorKind, PlanAdapter},
    adapters::omp,
    domain::{CodingPlan, PlanIdentity, UsageStatus, UsageWindow},
};

const SOURCE: &str = "Zhipu GLM";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const TOKEN_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_BODY_BYTES: usize = 1024 * 1024;
const GLOBAL_ORIGIN: &str = "https://api.z.ai";
const CN_ORIGIN: &str = "https://open.bigmodel.cn";
const QUOTA_PATH: &str = "/api/monitor/usage/quota/limit";
const OMP_PROVIDER: &str = "zhipu-coding-plan";

fn adapter_error(kind: AdapterErrorKind) -> AdapterError {
    AdapterError::new(kind, SOURCE)
}

fn protocol_error() -> AdapterError {
    adapter_error(AdapterErrorKind::ProtocolChanged)
}

/// Where the coding-plan credential comes from. The token itself is only
/// resolved at fetch time for sources that are expensive to read.
#[derive(Clone, Debug)]
enum TokenSource {
    /// Token captured during discovery (env var or Claude Code settings).
    Ready { token: String, origin: &'static str },
    /// `omp token zhipu-coding-plan` — resolved on fetch; OMP hands the
    /// credential over its own CLI, we never inspect its credential store.
    Omp,
}

pub struct ZhipuCodingPlanAdapter {
    source: TokenSource,
}

impl ZhipuCodingPlanAdapter {
    /// `omp_known` is `Some(true)` when an OMP account for the provider
    /// exists, `Some(false)` when it does not, `None` when OMP could not
    /// be asked. Lets callers reuse a single OMP scan.
    pub fn discover_with(omp_known: Option<bool>) -> Option<Self> {
        Self::from_env()
            .or_else(Self::from_claude_settings)
            .or_else(|| {
                omp_known.unwrap_or(false).then_some(Self {
                    source: TokenSource::Omp,
                })
            })
    }

    fn from_env() -> Option<Self> {
        let token = [
            "Z_AI_API_KEY",
            "ZHIPU_API_KEY",
            "ZHIPUAI_API_KEY",
            "GLM_API_KEY",
        ]
        .iter()
        .find_map(|key| env::var(key).ok())
        .and_then(non_empty)?;
        Some(Self {
            source: TokenSource::Ready {
                token,
                origin: GLOBAL_ORIGIN,
            },
        })
    }

    fn from_claude_settings() -> Option<Self> {
        let (token, origin) = claude_settings_anthropic_endpoint()?;
        Some(Self {
            source: TokenSource::Ready { token, origin },
        })
    }
}

impl PlanAdapter for ZhipuCodingPlanAdapter {
    fn identity(&self) -> PlanIdentity {
        PlanIdentity::new("zhipu-glm", "zhipu", "GLM")
    }

    fn fetch(&self) -> Result<CodingPlan, AdapterError> {
        let (token, origin) = match &self.source {
            TokenSource::Ready { token, origin } => (token.clone(), *origin),
            TokenSource::Omp => {
                let output =
                    omp::run_command("omp", &["token", OMP_PROVIDER, "--raw"], TOKEN_TIMEOUT)
                        .map_err(|_| adapter_error(AdapterErrorKind::NotAuthenticated))?;
                let token = non_empty(String::from_utf8_lossy(&output).trim().to_owned())
                    .ok_or_else(|| adapter_error(AdapterErrorKind::NotAuthenticated))?;
                (token, GLOBAL_ORIGIN)
            }
        };

        let fetch_at = |origin: &'static str| -> Result<CodingPlan, AdapterError> {
            let body = fetch_quota_body(&token, origin)?;
            parse_quota_response(body.as_bytes(), SystemTime::now())
        };
        match fetch_at(origin) {
            // A token bound to the mainland region is rejected by the global
            // endpoint, with either a transport status or a business error.
            Err(error)
                if origin == GLOBAL_ORIGIN
                    && matches!(
                        error.kind,
                        AdapterErrorKind::ProtocolChanged | AdapterErrorKind::NotAuthenticated
                    ) =>
            {
                fetch_at(CN_ORIGIN)
            }
            result => result,
        }
    }
}

fn fetch_quota_body(token: &str, origin: &str) -> Result<String, AdapterError> {
    let url = format!("{origin}{QUOTA_PATH}");
    let response = ureq::get(&url)
        .timeout(REQUEST_TIMEOUT)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json")
        .call();
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(401, _)) | Err(ureq::Error::Status(403, _)) => {
            return Err(adapter_error(AdapterErrorKind::NotAuthenticated));
        }
        Err(ureq::Error::Status(_, _)) | Err(ureq::Error::Transport(_)) => {
            return Err(adapter_error(AdapterErrorKind::TimedOut));
        }
    };
    if response.status() != 200 {
        return Err(adapter_error(AdapterErrorKind::TimedOut));
    }
    let body = response
        .into_string()
        .map_err(|_| adapter_error(AdapterErrorKind::ProtocolChanged))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(protocol_error());
    }
    Ok(body)
}

#[derive(Deserialize)]
struct QuotaEnvelope {
    code: Option<i64>,
    success: Option<bool>,
    data: Option<QuotaData>,
}

#[derive(Deserialize)]
struct QuotaData {
    limits: Vec<QuotaLimit>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuotaLimit {
    #[serde(rename = "type")]
    kind: String,
    unit: Option<i64>,
    number: Option<i64>,
    usage: Option<i64>,
    current_value: Option<i64>,
    remaining: Option<i64>,
    percentage: Option<i64>,
    next_reset_time: Option<u64>,
}

fn parse_quota_response(input: &[u8], fetched_at: SystemTime) -> Result<CodingPlan, AdapterError> {
    let envelope: QuotaEnvelope = serde_json::from_slice(input).map_err(|_| protocol_error())?;
    let business_ok = envelope.success.unwrap_or(false) || envelope.code == Some(200);
    if !business_ok {
        return Err(adapter_error(AdapterErrorKind::NotAuthenticated));
    }
    let data = envelope.data.ok_or_else(protocol_error)?;
    let windows = data
        .limits
        .iter()
        .filter(|limit| {
            matches!(
                limit.kind.as_str(),
                "TOKENS_LIMIT" | "CREDIT_LIMIT" | "TIME_LIMIT"
            )
        })
        .filter_map(|limit| {
            let minutes = window_minutes(&limit.kind, limit.unit, limit.number)?;
            let remaining = remaining_percent(limit)?;
            Some(UsageWindow {
                id: format!("zhipu-glm:{minutes}m"),
                label: window_label(&limit.kind, minutes),
                period: Some(Duration::from_secs(u64::try_from(minutes).ok()? * 60)),
                remaining_percent: remaining,
                resets_at: limit
                    .next_reset_time
                    .and_then(|value| UNIX_EPOCH.checked_add(Duration::from_millis(value))),
                status: if remaining == 0 {
                    UsageStatus::Unavailable
                } else {
                    UsageStatus::Available
                },
            })
        })
        .collect::<Vec<_>>();

    if windows.is_empty() {
        return Err(protocol_error());
    }

    Ok(CodingPlan {
        id: "zhipu-glm".to_owned(),
        provider_id: "zhipu".to_owned(),
        display_name: "GLM".to_owned(),
        fetched_at,
        windows,
    })
}

/// Window length in minutes from the provider's unit table:
/// 1 = days (1440), 3 = hours (60), 5 = minutes (1), 6 = weeks (10080).
/// `TIME_LIMIT` with unit 5 and number 1 is a monthly MCP sentinel.
fn window_minutes(kind: &str, unit: Option<i64>, number: Option<i64>) -> Option<i64> {
    let number = number.unwrap_or(1);
    if kind == "TIME_LIMIT" && unit == Some(5) && number == 1 {
        return Some(30 * 24 * 60);
    }
    let minutes_per_unit: i64 = match unit? {
        1 => 1440,
        3 => 60,
        5 => 1,
        6 => 10080,
        _ => return None,
    };
    Some(minutes_per_unit.saturating_mul(number.max(1)))
}

fn window_label(kind: &str, minutes: i64) -> String {
    let length = if minutes >= 10080 && minutes % 10080 == 0 {
        format!("{}w", minutes / 10080)
    } else if minutes >= 1440 && minutes % 1440 == 0 {
        format!("{}d", minutes / 1440)
    } else if minutes >= 60 && minutes % 60 == 0 {
        format!("{}h", minutes / 60)
    } else {
        format!("{minutes}m")
    };
    if kind == "TIME_LIMIT" {
        format!("MCP {length}")
    } else {
        length
    }
}

/// `percentage` reports usage; derive the remaining share from either the
/// percentage or the absolute counters (`used = max(usage - remaining,
/// current_value)`), clamped to 0-100.
fn remaining_percent(limit: &QuotaLimit) -> Option<u8> {
    let used = if let Some(percentage) = limit.percentage {
        percentage.clamp(0, 100)
    } else {
        let total = limit.usage?;
        if total <= 0 {
            return None;
        }
        let from_remaining = total.saturating_sub(limit.remaining.unwrap_or(0));
        let used_counts = from_remaining.max(limit.current_value.unwrap_or(0));
        used_counts.saturating_mul(100) / total
    };
    Some((100 - used).clamp(0, 100) as u8)
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim().to_owned();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn claude_settings_files() -> Vec<PathBuf> {
    let Some(home) = env::var_os("HOME") else {
        return Vec::new();
    };
    let base = PathBuf::from(home).join(".claude");
    ["settings.json", "settings.local.json"]
        .iter()
        .map(|name| base.join(name))
        .filter(|path| path.is_file())
        .collect()
}

/// Claude Code lets users route the Anthropic client at GLM's compatible
/// endpoint via `ANTHROPIC_BASE_URL` + `ANTHROPIC_AUTH_TOKEN` in settings.
fn claude_settings_anthropic_endpoint() -> Option<(String, &'static str)> {
    for path in claude_settings_files() {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(settings) = serde_json::from_str::<serde_json::Value>(&content) else {
            continue;
        };
        let Some(env_block) = settings.get("env") else {
            continue;
        };
        let base_url = env_block.get("ANTHROPIC_BASE_URL").and_then(|v| v.as_str());
        let token = env_block
            .get("ANTHROPIC_AUTH_TOKEN")
            .and_then(|value| value.as_str())
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if let (Some(base_url), Some(token)) = (base_url, token) {
            if let Some(origin) = origin_for_base_url(base_url) {
                return Some((token, origin));
            }
        }
    }
    None
}

fn origin_for_base_url(base_url: &str) -> Option<&'static str> {
    if base_url.contains("api.z.ai") {
        Some(GLOBAL_ORIGIN)
    } else if base_url.contains("open.bigmodel.cn") || base_url.contains("dev.bigmodel.cn") {
        Some(CN_ORIGIN)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUOTA_FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/zhipu_quota.json");

    #[test]
    fn parses_credit_limits_with_window_periods() {
        let plan = parse_quota_response(QUOTA_FIXTURE, UNIX_EPOCH)
            .expect("fixture should match the contract");

        assert_eq!(plan.id, "zhipu-glm");
        assert_eq!(plan.provider_id, "zhipu");
        assert_eq!(plan.windows.len(), 2);
        assert_eq!(plan.windows[0].label, "5h");
        assert_eq!(
            plan.windows[0].period,
            Some(Duration::from_secs(5 * 60 * 60))
        );
        assert_eq!(plan.windows[0].remaining_percent, 93);
        assert_eq!(plan.windows[0].status, UsageStatus::Available);
        assert_eq!(
            plan.windows[0].resets_at,
            Some(UNIX_EPOCH + Duration::from_millis(1789873609631))
        );
        assert_eq!(plan.windows[1].label, "1w");
        assert_eq!(
            plan.windows[1].period,
            Some(Duration::from_secs(7 * 24 * 60 * 60))
        );
        assert_eq!(plan.windows[1].remaining_percent, 99);
    }

    #[test]
    fn maps_legacy_types_and_the_monthly_mcp_sentinel() {
        let body = br#"{"code":200,"success":true,"data":{"limits":[
            {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":40,"nextResetTime":1000},
            {"type":"TIME_LIMIT","unit":5,"number":1,"percentage":10,"nextResetTime":2000}
        ]}}"#;
        let plan = parse_quota_response(body, UNIX_EPOCH).expect("should parse");

        assert_eq!(plan.windows[0].label, "5h");
        assert_eq!(plan.windows[0].remaining_percent, 60);
        assert_eq!(plan.windows[1].label, "MCP 30d");
        assert_eq!(
            plan.windows[1].period,
            Some(Duration::from_secs(30 * 24 * 60 * 60))
        );
    }

    #[test]
    fn derives_percent_from_counters_when_absent() {
        let body = br#"{"success":true,"data":{"limits":[
            {"type":"CREDIT_LIMIT","unit":3,"number":5,"usage":2000,"currentValue":140,"remaining":1859,"nextResetTime":1000}
        ]}}"#;
        let plan = parse_quota_response(body, UNIX_EPOCH).expect("should parse");

        assert_eq!(plan.windows[0].remaining_percent, 93);
    }

    #[test]
    fn marks_exhausted_windows_unavailable() {
        let body = br#"{"success":true,"data":{"limits":[
            {"type":"CREDIT_LIMIT","unit":3,"number":5,"percentage":100,"nextResetTime":1000}
        ]}}"#;
        let plan = parse_quota_response(body, UNIX_EPOCH).expect("should parse");

        assert_eq!(plan.windows[0].remaining_percent, 0);
        assert_eq!(plan.windows[0].status, UsageStatus::Unavailable);
    }

    #[test]
    fn business_failures_are_authentication_errors() {
        let error = parse_quota_response(
            br#"{"code":500,"msg":"no plan","success":false}"#,
            UNIX_EPOCH,
        )
        .unwrap_err();

        assert_eq!(error.kind, AdapterErrorKind::NotAuthenticated);
        assert_eq!(error.source, SOURCE);
    }

    #[test]
    fn malformed_or_empty_payloads_are_protocol_errors() {
        assert_eq!(
            parse_quota_response(b"not json", UNIX_EPOCH)
                .unwrap_err()
                .kind,
            AdapterErrorKind::ProtocolChanged
        );
        assert_eq!(
            parse_quota_response(br#"{"success":true,"data":{"limits":[]}}"#, UNIX_EPOCH)
                .unwrap_err()
                .kind,
            AdapterErrorKind::ProtocolChanged
        );
    }

    #[test]
    fn unknown_limit_kinds_and_units_are_ignored() {
        let body = br#"{"success":true,"data":{"limits":[
            {"type":"RATE_LIMIT","unit":3,"number":5,"percentage":1},
            {"type":"CREDIT_LIMIT","unit":99,"number":5,"percentage":1}
        ]}}"#;
        assert!(parse_quota_response(body, UNIX_EPOCH).is_err());
    }
}
