use std::{
    fs,
    io::{BufRead, BufReader},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;

use crate::{
    adapter::{AdapterError, AdapterErrorKind, ModelUsageAdapter},
    adapters::usage_common::{home_dir, recent_files, UsageAccumulator, MAX_USAGE_FILE_BYTES},
    domain::{ModelUsage, ModelUsageSnapshot},
};

const SOURCE: &str = "Pi sessions";

pub struct PiModelUsageAdapter {
    sessions_dir: PathBuf,
}

impl PiModelUsageAdapter {
    pub fn discover() -> Option<Self> {
        let sessions_dir = home_dir()?.join(".pi/agent/sessions");
        sessions_dir.is_dir().then_some(Self { sessions_dir })
    }
}

impl ModelUsageAdapter for PiModelUsageAdapter {
    fn source_id(&self) -> &'static str {
        "pi"
    }

    fn fetch(&self) -> Result<ModelUsageSnapshot, AdapterError> {
        let mut usage = UsageAccumulator::default();
        for (path, _) in recent_files(&self.sessions_dir, ".jsonl") {
            parse_session(&path, &mut usage)?;
        }
        Ok(ModelUsageSnapshot {
            source_id: "pi".to_owned(),
            fetched_at: SystemTime::now(),
            models: usage.into_models(),
        })
    }
}

#[derive(Deserialize)]
struct PiEnvelope {
    #[serde(rename = "type")]
    kind: String,
    message: Option<PiMessage>,
}

#[derive(Deserialize)]
struct PiMessage {
    role: String,
    provider: Option<String>,
    model: Option<String>,
    usage: Option<PiUsage>,
    timestamp: Option<u64>,
    #[serde(rename = "stopReason")]
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct PiUsage {
    #[serde(default)]
    input: u64,
    #[serde(default)]
    output: u64,
    #[serde(default, rename = "cacheRead")]
    cache_read: u64,
    #[serde(default, rename = "cacheWrite")]
    cache_write: u64,
    cost: Option<PiCost>,
}

#[derive(Deserialize)]
struct PiCost {
    total: f64,
}

fn parse_session(path: &std::path::Path, usage: &mut UsageAccumulator) -> Result<(), AdapterError> {
    let file = fs::File::open(path)
        .map_err(|_| AdapterError::new(AdapterErrorKind::ProtocolChanged, SOURCE))?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        let bytes = reader
            .read_until(b'\n', &mut line)
            .map_err(|_| AdapterError::new(AdapterErrorKind::ProtocolChanged, SOURCE))?;
        if bytes == 0 {
            break;
        }
        if line.len() as u64 > MAX_USAGE_FILE_BYTES || !contains(&line, b"assistant") {
            continue;
        }
        let Ok(envelope) = serde_json::from_slice::<PiEnvelope>(&line) else {
            continue;
        };
        let Some(message) = envelope.message else {
            continue;
        };
        if envelope.kind != "message" || message.role != "assistant" {
            continue;
        }
        let (Some(provider_id), Some(model_id), Some(tokens)) =
            (message.provider, message.model, message.usage)
        else {
            continue;
        };
        if provider_id.is_empty() || model_id.is_empty() {
            continue;
        }
        let timestamp = message
            .timestamp
            .and_then(|value| UNIX_EPOCH.checked_add(Duration::from_millis(value)));
        usage.add(ModelUsage {
            agent_id: "pi".to_owned(),
            agent_name: "Pi".to_owned(),
            provider_id,
            model_id,
            requests: 1,
            failed_requests: u64::from(message.stop_reason.as_deref() == Some("error")),
            input_tokens: tokens.input,
            output_tokens: tokens.output,
            cache_read_tokens: tokens.cache_read,
            cache_write_tokens: tokens.cache_write,
            cost_usd: tokens
                .cost
                .map(|cost| cost.total)
                .filter(|cost| cost.is_finite()),
            first_used_at: timestamp,
            last_used_at: timestamp,
        });
    }
    Ok(())
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
    fn session_imports_only_assistant_usage_metadata() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("limitdeck-pi-{unique}.jsonl"));
        fs::write(
            &path,
            concat!(
                "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"private\",\"timestamp\":1000}}\n",
                "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"provider\":\"openai-codex\",\"model\":\"gpt-test\",\"content\":[{\"type\":\"text\",\"text\":\"private\"}],\"usage\":{\"input\":100,\"output\":20,\"cacheRead\":80,\"cacheWrite\":4,\"cost\":{\"total\":1.25}},\"timestamp\":2000,\"stopReason\":\"stop\"}}\n"
            ),
        )
        .unwrap();
        let mut accumulator = UsageAccumulator::default();
        parse_session(&path, &mut accumulator).unwrap();

        let models = accumulator.into_models();
        assert_eq!(models.len(), 1);
        let model = &models[0];
        assert_eq!(
            (model.agent_id.as_str(), model.model_id.as_str()),
            ("pi", "gpt-test")
        );
        assert_eq!((model.requests, model.failed_requests), (1, 0));
        assert_eq!((model.input_tokens, model.output_tokens), (100, 20));
        assert_eq!((model.cache_read_tokens, model.cache_write_tokens), (80, 4));
        assert_eq!(model.cost_usd, Some(1.25));
        assert_eq!(
            model.first_used_at,
            UNIX_EPOCH.checked_add(Duration::from_secs(2))
        );
        let _ = fs::remove_file(path);
    }
}
