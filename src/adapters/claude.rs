use std::{
    env, fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    adapter::{AdapterError, AdapterErrorKind, PlanAdapter},
    domain::{CodingPlan, PlanIdentity, UsageStatus, UsageWindow},
};

const MAX_INPUT_BYTES: usize = 1024 * 1024;
const SOURCE: &str = "Claude statusline";

fn adapter_error(kind: AdapterErrorKind) -> AdapterError {
    AdapterError::new(kind, SOURCE)
}

fn protocol_error() -> AdapterError {
    adapter_error(AdapterErrorKind::ProtocolChanged)
}

fn snapshot_open_error(error: &io::Error) -> AdapterError {
    let kind = if error.kind() == io::ErrorKind::NotFound {
        AdapterErrorKind::SnapshotMissing
    } else {
        AdapterErrorKind::ProtocolChanged
    };
    adapter_error(kind)
}

pub struct ClaudeStatuslineAdapter {
    cache_path: PathBuf,
}

impl ClaudeStatuslineAdapter {
    pub fn discover() -> Option<Self> {
        let cache_path = cache_path()?;
        cache_path.is_file().then_some(Self { cache_path })
    }

    #[cfg(test)]
    fn at_path(cache_path: PathBuf) -> Self {
        Self { cache_path }
    }
}

impl PlanAdapter for ClaudeStatuslineAdapter {
    fn identity(&self) -> PlanIdentity {
        PlanIdentity::new("anthropic-claude", "anthropic", "Claude")
    }

    fn fetch(&self) -> Result<CodingPlan, AdapterError> {
        read_cache(&self.cache_path)
    }
}

pub fn ingest_stdin() -> Result<String, AdapterError> {
    let mut input = Vec::new();
    io::stdin()
        .take((MAX_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut input)
        .map_err(|_| protocol_error())?;
    if input.len() > MAX_INPUT_BYTES {
        return Err(protocol_error());
    }
    let plan = parse_statusline(&input, SystemTime::now())?;
    let path = cache_path().ok_or_else(|| adapter_error(AdapterErrorKind::SnapshotMissing))?;
    write_cache(&path, &plan)?;
    Ok(compact_statusline(&plan))
}

#[derive(Deserialize)]
struct StatuslineInput {
    rate_limits: Option<RateLimits>,
}

#[derive(Deserialize)]
struct RateLimits {
    five_hour: Option<RateLimitWindow>,
    seven_day: Option<RateLimitWindow>,
    spend_limit: Option<RateLimitWindow>,
}

#[derive(Deserialize)]
struct RateLimitWindow {
    used_percentage: f64,
    resets_at: u64,
}

fn parse_statusline(input: &[u8], fetched_at: SystemTime) -> Result<CodingPlan, AdapterError> {
    let input: StatuslineInput = serde_json::from_slice(input).map_err(|_| protocol_error())?;
    let limits = input
        .rate_limits
        .ok_or_else(|| adapter_error(AdapterErrorKind::NotAuthenticated))?;
    let mut windows = Vec::with_capacity(4);
    push_window(
        &mut windows,
        "anthropic:five-hour",
        "Claude · 5 小时",
        Some(Duration::from_secs(5 * 60 * 60)),
        limits.five_hour,
    );
    push_window(
        &mut windows,
        "anthropic:seven-day",
        "Claude · 7 天",
        Some(Duration::from_secs(7 * 24 * 60 * 60)),
        limits.seven_day,
    );
    push_window(
        &mut windows,
        "anthropic:spend-limit",
        "Claude · 消费限额",
        None,
        limits.spend_limit,
    );
    if windows.is_empty() {
        return Err(protocol_error());
    }

    Ok(CodingPlan {
        id: "anthropic-claude".to_owned(),
        provider_id: "anthropic".to_owned(),
        display_name: "Claude".to_owned(),
        fetched_at,
        windows,
    })
}

fn push_window(
    windows: &mut Vec<UsageWindow>,
    id: &str,
    label: &str,
    period: Option<Duration>,
    source: Option<RateLimitWindow>,
) {
    let Some(source) = source else {
        return;
    };
    if !source.used_percentage.is_finite() {
        return;
    }
    let used = source.used_percentage.clamp(0.0, 100.0).round() as u8;
    windows.push(UsageWindow {
        id: id.to_owned(),
        label: label.to_owned(),
        period,
        remaining_percent: 100u8.saturating_sub(used),
        resets_at: UNIX_EPOCH.checked_add(Duration::from_secs(source.resets_at)),
        status: UsageStatus::Available,
    });
}

#[derive(Serialize, Deserialize)]
struct CachedPlan {
    fetched_at_millis: u64,
    windows: Vec<CachedWindow>,
}

#[derive(Serialize, Deserialize)]
struct CachedWindow {
    id: String,
    label: String,
    period_seconds: Option<u64>,
    remaining_percent: u8,
    resets_at_millis: Option<u64>,
    available: bool,
}

fn write_cache(path: &Path, plan: &CodingPlan) -> Result<(), AdapterError> {
    let parent = path.parent().ok_or_else(protocol_error)?;
    fs::create_dir_all(parent).map_err(|_| protocol_error())?;
    let cached = CachedPlan {
        fetched_at_millis: system_time_millis(plan.fetched_at)?,
        windows: plan
            .windows
            .iter()
            .map(|window| {
                Ok(CachedWindow {
                    id: window.id.clone(),
                    label: window.label.clone(),
                    period_seconds: window.period.map(|period| period.as_secs()),
                    remaining_percent: window.remaining_percent,
                    resets_at_millis: window.resets_at.map(system_time_millis).transpose()?,
                    available: window.status == UsageStatus::Available,
                })
            })
            .collect::<Result<Vec<_>, AdapterError>>()?,
    };
    let encoded = serde_json::to_vec(&cached).map_err(|_| protocol_error())?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, encoded).map_err(|_| protocol_error())?;
    fs::rename(&temporary, path).map_err(|_| {
        let _ = fs::remove_file(&temporary);
        protocol_error()
    })
}

fn read_cache(path: &Path) -> Result<CodingPlan, AdapterError> {
    let mut encoded = Vec::new();
    fs::File::open(path)
        .map_err(|error| snapshot_open_error(&error))?
        .take((MAX_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut encoded)
        .map_err(|_| protocol_error())?;
    if encoded.len() > MAX_INPUT_BYTES {
        return Err(protocol_error());
    }
    let cached: CachedPlan = serde_json::from_slice(&encoded).map_err(|_| protocol_error())?;
    if cached.windows.is_empty() {
        return Err(protocol_error());
    }
    Ok(CodingPlan {
        id: "anthropic-claude".to_owned(),
        provider_id: "anthropic".to_owned(),
        display_name: "Claude".to_owned(),
        fetched_at: timestamp_millis(cached.fetched_at_millis)?,
        windows: cached
            .windows
            .into_iter()
            .map(|window| UsageWindow {
                id: window.id,
                label: window.label,
                period: window.period_seconds.map(Duration::from_secs),
                remaining_percent: window.remaining_percent.min(100),
                resets_at: window
                    .resets_at_millis
                    .and_then(|value| timestamp_millis(value).ok()),
                status: if window.available {
                    UsageStatus::Available
                } else {
                    UsageStatus::Unavailable
                },
            })
            .collect(),
    })
}

fn compact_statusline(plan: &CodingPlan) -> String {
    let values = plan
        .windows
        .iter()
        .map(|window| {
            format!(
                "{} {}%",
                period_label(window.period),
                window.remaining_percent
            )
        })
        .collect::<Vec<_>>()
        .join(" · ");
    format!("Claude {values}")
}

fn period_label(period: Option<Duration>) -> String {
    let Some(period) = period else {
        return "quota".to_owned();
    };
    let hours = period.as_secs() / 3600;
    if hours >= 24 && hours % 24 == 0 {
        format!("{}d", hours / 24)
    } else {
        format!("{hours}h")
    }
}

fn cache_path() -> Option<PathBuf> {
    if let Some(directory) = env::var_os("XDG_CACHE_HOME") {
        return Some(PathBuf::from(directory).join("limitdeck/claude.json"));
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".cache/limitdeck/claude.json"))
}

fn system_time_millis(value: SystemTime) -> Result<u64, AdapterError> {
    let millis = value
        .duration_since(UNIX_EPOCH)
        .map_err(|_| protocol_error())?
        .as_millis();
    u64::try_from(millis).map_err(|_| protocol_error())
}

fn timestamp_millis(milliseconds: u64) -> Result<SystemTime, AdapterError> {
    UNIX_EPOCH
        .checked_add(Duration::from_millis(milliseconds))
        .ok_or_else(protocol_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUSLINE_FIXTURE: &[u8] = br#"{
        "session_id":"secret-session",
        "account":{"email":"private@example.com"},
        "rate_limits":{
            "five_hour":{"used_percentage":29.4,"resets_at":2000000000},
            "seven_day":{"used_percentage":48.6,"resets_at":2000500000},
            "spend_limit":{"used_percentage":62.8,"resets_at":2001000000}
        }
    }"#;

    #[test]
    fn statusline_parser_keeps_only_rate_limit_windows() {
        let plan = parse_statusline(STATUSLINE_FIXTURE, UNIX_EPOCH + Duration::from_secs(10))
            .expect("statusline fixture should parse");

        assert_eq!(plan.windows.len(), 3);
        assert_eq!(plan.windows[0].remaining_percent, 71);
        assert_eq!(plan.windows[1].remaining_percent, 51);
        assert_eq!(plan.windows[2].remaining_percent, 37);
        assert_eq!(plan.windows[2].period, None);
        let retained = format!("{plan:?}");
        assert!(!retained.contains("secret-session"));
        assert!(!retained.contains("private@example.com"));
    }
    #[test]
    fn cache_reader_rejects_oversized_files() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!("limitdeck-oversized-{unique}.json"));
        fs::write(&path, vec![b'x'; MAX_INPUT_BYTES + 1]).unwrap();

        assert_eq!(read_cache(&path), Err(protocol_error()));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn cache_round_trip_is_a_real_adapter_input() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = env::temp_dir().join(format!("limitdeck-{unique}"));
        let path = directory.join("claude.json");
        let plan = parse_statusline(STATUSLINE_FIXTURE, UNIX_EPOCH + Duration::from_secs(10))
            .expect("statusline fixture should parse");
        write_cache(&path, &plan).expect("cache should write atomically");

        let loaded = ClaudeStatuslineAdapter::at_path(path.clone())
            .fetch()
            .expect("adapter should load the cached snapshot");

        assert_eq!(loaded, plan);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn missing_rate_limits_means_claude_is_not_authenticated() {
        let error = parse_statusline(br#"{"session_id":"secret"}"#, SystemTime::now())
            .expect_err("statusline without rate limits must not look healthy");

        assert_eq!(error.kind, AdapterErrorKind::NotAuthenticated);
        assert!(!format!("{error:?}").contains("secret"));
    }
}
