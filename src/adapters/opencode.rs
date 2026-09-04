use std::{
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;
use wait_timeout::ChildExt;

use crate::{
    adapter::{AdapterError, AdapterErrorKind, ModelUsageAdapter},
    adapters::usage_common::{home_dir, UsageAccumulator},
    domain::{ModelUsage, ModelUsageSnapshot},
};

const SOURCE: &str = "OpenCode database";
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const QUERY: &str = r#"
SELECT
  json_extract(data, '$.modelID') AS model,
  json_extract(data, '$.providerID') AS provider,
  COUNT(*) AS requests,
  SUM(CASE WHEN json_extract(data, '$.finish') = 'error' THEN 1 ELSE 0 END) AS failed_requests,
  SUM(COALESCE(json_extract(data, '$.tokens.input'), 0)) AS input_tokens,
  SUM(COALESCE(json_extract(data, '$.tokens.output'), 0) + COALESCE(json_extract(data, '$.tokens.reasoning'), 0)) AS output_tokens,
  SUM(COALESCE(json_extract(data, '$.tokens.cache.read'), 0)) AS cache_read_tokens,
  SUM(COALESCE(json_extract(data, '$.tokens.cache.write'), 0)) AS cache_write_tokens,
  SUM(COALESCE(json_extract(data, '$.cost'), 0)) AS cost,
  MIN(time_created) AS first_timestamp,
  MAX(time_updated) AS last_timestamp
FROM message
WHERE json_extract(data, '$.role') = 'assistant'
  AND json_extract(data, '$.modelID') IS NOT NULL
  AND json_extract(data, '$.providerID') IS NOT NULL
GROUP BY provider, model
"#;

pub struct OpenCodeModelUsageAdapter {
    database: PathBuf,
}

impl OpenCodeModelUsageAdapter {
    pub fn discover() -> Option<Self> {
        let data_root = std::env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|home| home.join(".local/share")))?;
        let database = data_root.join("opencode/opencode.db");
        database.is_file().then_some(Self { database })
    }
}

impl ModelUsageAdapter for OpenCodeModelUsageAdapter {
    fn source_id(&self) -> &'static str {
        "opencode"
    }

    fn fetch(&self) -> Result<ModelUsageSnapshot, AdapterError> {
        let output = run_query(&self.database)?;
        parse_rows(&output, SystemTime::now())
    }
}

fn run_query(database: &std::path::Path) -> Result<Vec<u8>, AdapterError> {
    let mut child = Command::new("sqlite3")
        .args(["-readonly", "-json"])
        .arg(database)
        .arg(QUERY)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| AdapterError::new(AdapterErrorKind::CommandNotFound, SOURCE))?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(AdapterError::new(AdapterErrorKind::ProtocolChanged, SOURCE));
    };
    let output_reader = thread::spawn(move || {
        let mut output = Vec::new();
        stdout
            .take((MAX_OUTPUT_BYTES + 1) as u64)
            .read_to_end(&mut output)?;
        Ok::<_, std::io::Error>(output)
    });

    let status = match child.wait_timeout(COMMAND_TIMEOUT) {
        Ok(Some(status)) => status,
        Ok(None) | Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = output_reader.join();
            return Err(AdapterError::new(AdapterErrorKind::TimedOut, SOURCE));
        }
    };
    let output = output_reader
        .join()
        .map_err(|_| AdapterError::new(AdapterErrorKind::ProtocolChanged, SOURCE))?
        .map_err(|_| AdapterError::new(AdapterErrorKind::ProtocolChanged, SOURCE))?;
    if !status.success() || output.len() > MAX_OUTPUT_BYTES {
        return Err(AdapterError::new(AdapterErrorKind::ProtocolChanged, SOURCE));
    }
    Ok(output)
}
#[derive(Deserialize)]
struct OpenCodeRow {
    model: String,
    provider: String,
    requests: u64,
    failed_requests: u64,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    cost: f64,
    first_timestamp: u64,
    last_timestamp: u64,
}

fn parse_rows(input: &[u8], fetched_at: SystemTime) -> Result<ModelUsageSnapshot, AdapterError> {
    let rows: Vec<OpenCodeRow> = serde_json::from_slice(input)
        .map_err(|_| AdapterError::new(AdapterErrorKind::ProtocolChanged, SOURCE))?;
    let mut usage = UsageAccumulator::default();
    for row in rows {
        usage.add(ModelUsage {
            agent_id: "opencode".to_owned(),
            agent_name: "OpenCode".to_owned(),
            provider_id: row.provider,
            model_id: row.model,
            requests: row.requests,
            failed_requests: row.failed_requests,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_read_tokens: row.cache_read_tokens,
            cache_write_tokens: row.cache_write_tokens,
            cost_usd: row.cost.is_finite().then_some(row.cost),
            first_used_at: timestamp_millis(row.first_timestamp),
            last_used_at: timestamp_millis(row.last_timestamp),
        });
    }
    Ok(ModelUsageSnapshot {
        source_id: "opencode".to_owned(),
        fetched_at,
        models: usage.into_models(),
    })
}

fn timestamp_millis(value: u64) -> Option<SystemTime> {
    UNIX_EPOCH.checked_add(Duration::from_millis(value))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_rows_map_to_agent_and_model_usage() {
        let snapshot = parse_rows(
            br#"[{"model":"claude-test","provider":"anthropic","requests":4,"failed_requests":1,"input_tokens":100,"output_tokens":20,"cache_read_tokens":80,"cache_write_tokens":5,"cost":1.5,"first_timestamp":1000,"last_timestamp":2000}]"#,
            UNIX_EPOCH,
        )
        .unwrap();

        let usage = &snapshot.models[0];
        assert_eq!(
            (usage.agent_id.as_str(), usage.model_id.as_str()),
            ("opencode", "claude-test")
        );
        assert_eq!((usage.requests, usage.failed_requests), (4, 1));
        assert_eq!((usage.input_tokens, usage.output_tokens), (100, 20));
        assert_eq!((usage.cache_read_tokens, usage.cache_write_tokens), (80, 5));
        assert_eq!(usage.cost_usd, Some(1.5));
    }
}
