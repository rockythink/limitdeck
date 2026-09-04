use std::{
    collections::hash_map::DefaultHasher,
    env, fs,
    hash::{Hash, Hasher},
    io::{self, Read},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::{
    adapter::{AdapterError, AdapterErrorKind, ModelUsageAdapter, PlanAdapter},
    adapters::usage_common::cache_dir,
    domain::{CodingPlan, ModelUsage, ModelUsageSnapshot, PlanIdentity, UsageStatus, UsageWindow},
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
    let fetched_at = SystemTime::now();
    let plan = parse_statusline(&input, fetched_at)?;
    let path = cache_path().ok_or_else(|| adapter_error(AdapterErrorKind::SnapshotMissing))?;
    write_cache(&path, &plan)?;
    let _ = record_model_usage(&input, fetched_at);
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
        "Claude · 5 hours",
        Some(Duration::from_secs(5 * 60 * 60)),
        limits.five_hour,
    );
    push_window(
        &mut windows,
        "anthropic:seven-day",
        "Claude · 7 days",
        Some(Duration::from_secs(7 * 24 * 60 * 60)),
        limits.seven_day,
    );
    push_window(
        &mut windows,
        "anthropic:spend-limit",
        "Claude · Spend limit",
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

pub struct ClaudeModelUsageAdapter {
    cache_path: PathBuf,
}

impl ClaudeModelUsageAdapter {
    pub fn discover() -> Option<Self> {
        let cache_path = model_usage_cache_path()?;
        cache_path.is_file().then_some(Self { cache_path })
    }
}

impl ModelUsageAdapter for ClaudeModelUsageAdapter {
    fn source_id(&self) -> &'static str {
        "claude-code"
    }

    fn fetch(&self) -> Result<ModelUsageSnapshot, AdapterError> {
        read_model_usage(&self.cache_path)
    }
}

#[derive(Deserialize)]
struct ClaudeUsageInput {
    session_id: Option<String>,
    model: Option<ClaudeModel>,
    context_window: Option<ClaudeContextWindow>,
    cost: Option<ClaudeCost>,
}

#[derive(Deserialize)]
struct ClaudeModel {
    id: String,
}

#[derive(Deserialize)]
struct ClaudeContextWindow {
    #[serde(default)]
    total_input_tokens: u64,
    #[serde(default)]
    total_output_tokens: u64,
}

#[derive(Deserialize)]
struct ClaudeCost {
    #[serde(default)]
    total_cost_usd: f64,
}

#[derive(Default, Serialize, Deserialize)]
struct ClaudeUsageCache {
    #[serde(default)]
    cursors: Vec<ClaudeUsageCursor>,
    #[serde(default)]
    models: Vec<CachedModelUsage>,
}

#[derive(Serialize, Deserialize)]
struct ClaudeUsageCursor {
    session_hash: u64,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
    updated_at_millis: u64,
}

#[derive(Serialize, Deserialize)]
struct CachedModelUsage {
    model_id: String,
    requests: u64,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
    first_used_at_millis: u64,
    last_used_at_millis: u64,
}

fn record_model_usage(input: &[u8], fetched_at: SystemTime) -> Result<(), AdapterError> {
    let path =
        model_usage_cache_path().ok_or_else(|| adapter_error(AdapterErrorKind::SnapshotMissing))?;
    record_model_usage_at(input, fetched_at, &path)
}

fn record_model_usage_at(
    input: &[u8],
    fetched_at: SystemTime,
    path: &Path,
) -> Result<(), AdapterError> {
    let source: ClaudeUsageInput = serde_json::from_slice(input).map_err(|_| protocol_error())?;
    let (Some(session_id), Some(model), Some(context)) =
        (source.session_id, source.model, source.context_window)
    else {
        return Ok(());
    };
    if model.id.is_empty() {
        return Ok(());
    }
    let mut cache = read_usage_cache(path).unwrap_or_default();
    let timestamp = system_time_millis(fetched_at)?;
    let current_cost = source
        .cost
        .map(|cost| cost.total_cost_usd)
        .filter(|cost| cost.is_finite() && *cost >= 0.0)
        .unwrap_or(0.0);
    let session_hash = hash_session(&session_id);
    let (input_delta, output_delta, cost_delta) = if let Some(cursor) = cache
        .cursors
        .iter_mut()
        .find(|cursor| cursor.session_hash == session_hash)
    {
        let deltas = (
            context
                .total_input_tokens
                .saturating_sub(cursor.input_tokens),
            context
                .total_output_tokens
                .saturating_sub(cursor.output_tokens),
            (current_cost - cursor.cost_usd).max(0.0),
        );
        cursor.input_tokens = context.total_input_tokens;
        cursor.output_tokens = context.total_output_tokens;
        cursor.cost_usd = current_cost;
        cursor.updated_at_millis = timestamp;
        deltas
    } else {
        cache.cursors.push(ClaudeUsageCursor {
            session_hash,
            input_tokens: context.total_input_tokens,
            output_tokens: context.total_output_tokens,
            cost_usd: current_cost,
            updated_at_millis: timestamp,
        });
        (
            context.total_input_tokens,
            context.total_output_tokens,
            current_cost,
        )
    };
    if input_delta > 0 || output_delta > 0 || cost_delta > 0.0 {
        if let Some(usage) = cache
            .models
            .iter_mut()
            .find(|usage| usage.model_id == model.id)
        {
            usage.requests = usage.requests.saturating_add(1);
            usage.input_tokens = usage.input_tokens.saturating_add(input_delta);
            usage.output_tokens = usage.output_tokens.saturating_add(output_delta);
            usage.cost_usd += cost_delta;
            usage.last_used_at_millis = timestamp;
        } else {
            cache.models.push(CachedModelUsage {
                model_id: model.id,
                requests: 1,
                input_tokens: input_delta,
                output_tokens: output_delta,
                cost_usd: cost_delta,
                first_used_at_millis: timestamp,
                last_used_at_millis: timestamp,
            });
        }
    }
    cache
        .cursors
        .sort_unstable_by_key(|cursor| std::cmp::Reverse(cursor.updated_at_millis));
    cache.cursors.truncate(64);
    write_usage_cache(path, &cache)
}

fn hash_session(session_id: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    session_id.hash(&mut hasher);
    hasher.finish()
}

fn read_usage_cache(path: &Path) -> Result<ClaudeUsageCache, AdapterError> {
    let mut encoded = Vec::new();
    fs::File::open(path)
        .map_err(|error| snapshot_open_error(&error))?
        .take((MAX_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut encoded)
        .map_err(|_| protocol_error())?;
    if encoded.len() > MAX_INPUT_BYTES {
        return Err(protocol_error());
    }
    serde_json::from_slice(&encoded).map_err(|_| protocol_error())
}

fn write_usage_cache(path: &Path, cache: &ClaudeUsageCache) -> Result<(), AdapterError> {
    let parent = path.parent().ok_or_else(protocol_error)?;
    fs::create_dir_all(parent).map_err(|_| protocol_error())?;
    let encoded = serde_json::to_vec(cache).map_err(|_| protocol_error())?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, encoded).map_err(|_| protocol_error())?;
    fs::rename(&temporary, path).map_err(|_| {
        let _ = fs::remove_file(&temporary);
        protocol_error()
    })
}

fn read_model_usage(path: &Path) -> Result<ModelUsageSnapshot, AdapterError> {
    let cache = read_usage_cache(path)?;
    let mut models = Vec::with_capacity(cache.models.len());
    for usage in cache.models {
        models.push(ModelUsage {
            agent_id: "claude-code".to_owned(),
            agent_name: "Claude Code".to_owned(),
            provider_id: "anthropic".to_owned(),
            model_id: usage.model_id,
            requests: usage.requests,
            failed_requests: 0,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_usd: usage.cost_usd.is_finite().then_some(usage.cost_usd),
            first_used_at: timestamp_millis(usage.first_used_at_millis).ok(),
            last_used_at: timestamp_millis(usage.last_used_at_millis).ok(),
        });
    }
    Ok(ModelUsageSnapshot {
        source_id: "claude-code".to_owned(),
        fetched_at: SystemTime::now(),
        models,
    })
}

fn model_usage_cache_path() -> Option<PathBuf> {
    cache_dir().map(|directory| directory.join("claude-models.json"))
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
    #[test]
    fn model_usage_records_only_new_session_totals() {
        fn status(input: u64, output: u64, cost: f64) -> Vec<u8> {
            format!(
                r#"{{"session_id":"session","model":{{"id":"claude-test"}},"context_window":{{"total_input_tokens":{input},"total_output_tokens":{output}}},"cost":{{"total_cost_usd":{cost}}}}}"#
            )
            .into_bytes()
        }

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = env::temp_dir().join(format!("limitdeck-claude-model-{unique}"));
        let path = directory.join("usage.json");
        record_model_usage_at(
            &status(100, 10, 1.25),
            UNIX_EPOCH + Duration::from_secs(1),
            &path,
        )
        .unwrap();
        record_model_usage_at(
            &status(140, 15, 2.0),
            UNIX_EPOCH + Duration::from_secs(2),
            &path,
        )
        .unwrap();

        let snapshot = read_model_usage(&path).unwrap();
        let usage = &snapshot.models[0];
        assert_eq!(
            (usage.requests, usage.input_tokens, usage.output_tokens),
            (2, 140, 15)
        );
        assert_eq!(usage.cost_usd, Some(2.0));
        let encoded = fs::read_to_string(&path).unwrap();
        assert!(!encoded.contains("session_id"));
        let _ = fs::remove_dir_all(directory);
    }
}
