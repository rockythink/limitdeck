use std::{
    fs,
    io::{BufRead, BufReader},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;

use crate::{
    adapter::{AdapterError, AdapterErrorKind, ModelUsageAdapter},
    adapters::usage_common::{
        cache_dir, provider_for_model, UsageAccumulator, MAX_USAGE_FILE_BYTES,
    },
    domain::{ModelUsage, ModelUsageSnapshot},
};

const SOURCE: &str = "Aider analytics log";

pub struct AiderModelUsageAdapter {
    analytics_log: PathBuf,
}

impl AiderModelUsageAdapter {
    pub fn discover() -> Option<Self> {
        let analytics_log = std::env::var_os("AIDER_ANALYTICS_LOG")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| cache_dir().map(|directory| directory.join("aider.jsonl")))?;
        analytics_log.is_file().then_some(Self { analytics_log })
    }
}

impl ModelUsageAdapter for AiderModelUsageAdapter {
    fn source_id(&self) -> &'static str {
        "aider"
    }

    fn fetch(&self) -> Result<ModelUsageSnapshot, AdapterError> {
        let file = fs::File::open(&self.analytics_log)
            .map_err(|_| AdapterError::new(AdapterErrorKind::SnapshotMissing, SOURCE))?;
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        let mut usage = UsageAccumulator::default();
        loop {
            line.clear();
            let bytes = reader
                .read_until(b'\n', &mut line)
                .map_err(|_| AdapterError::new(AdapterErrorKind::ProtocolChanged, SOURCE))?;
            if bytes == 0 {
                break;
            }
            if line.len() as u64 > MAX_USAGE_FILE_BYTES || !contains(&line, b"message_send") {
                continue;
            }
            let Ok(event) = serde_json::from_slice::<AiderEvent>(&line) else {
                continue;
            };
            if event.kind != "message_send" {
                continue;
            }
            let Some(model_id) = event.properties.main_model else {
                continue;
            };
            if model_id.is_empty() {
                continue;
            }
            let timestamp = UNIX_EPOCH.checked_add(Duration::from_secs(event.time));
            usage.add(ModelUsage {
                agent_id: "aider".to_owned(),
                agent_name: "Aider".to_owned(),
                provider_id: provider_for_model(&model_id),
                model_id,
                requests: 1,
                failed_requests: 0,
                input_tokens: event.properties.prompt_tokens,
                output_tokens: event.properties.completion_tokens,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_usd: event.properties.cost.filter(|cost| cost.is_finite()),
                first_used_at: timestamp,
                last_used_at: timestamp,
            });
        }
        Ok(ModelUsageSnapshot {
            source_id: "aider".to_owned(),
            fetched_at: SystemTime::now(),
            models: usage.into_models(),
        })
    }
}

#[derive(Deserialize)]
struct AiderEvent {
    #[serde(rename = "event")]
    kind: String,
    properties: AiderProperties,
    time: u64,
}

#[derive(Deserialize)]
struct AiderProperties {
    main_model: Option<String>,
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    cost: Option<f64>,
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analytics_log_imports_only_completed_message_usage() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("limitdeck-aider-{unique}.jsonl"));
        std::fs::write(
            &path,
            concat!(
                "{\"event\":\"message_send_starting\",\"properties\":{},\"time\":1}\n",
                "{\"event\":\"message_send\",\"properties\":{\"main_model\":\"openai/gpt-test\",\"prompt_tokens\":100,\"completion_tokens\":20,\"cost\":0.5},\"time\":2}\n"
            ),
        )
        .unwrap();
        let snapshot = AiderModelUsageAdapter {
            analytics_log: path.clone(),
        }
        .fetch()
        .unwrap();

        assert_eq!(snapshot.models.len(), 1);
        let usage = &snapshot.models[0];
        assert_eq!(
            (usage.provider_id.as_str(), usage.model_id.as_str()),
            ("openai", "openai/gpt-test")
        );
        assert_eq!(
            (usage.requests, usage.input_tokens, usage.output_tokens),
            (1, 100, 20)
        );
        assert_eq!(usage.cost_usd, Some(0.5));
        let _ = std::fs::remove_file(path);
    }
}
