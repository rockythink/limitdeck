use std::{
    fs,
    io::{BufRead, BufReader},
    path::PathBuf,
    time::SystemTime,
};

use serde::Deserialize;

use crate::{
    adapter::{AdapterError, AdapterErrorKind, ModelUsageAdapter},
    adapters::usage_common::{home_dir, recent_files, UsageAccumulator, MAX_USAGE_FILE_BYTES},
    domain::{ModelUsage, ModelUsageSnapshot},
};

const SOURCE: &str = "Codex sessions";

pub struct CodexModelUsageAdapter {
    sessions_dir: PathBuf,
}

impl CodexModelUsageAdapter {
    pub fn discover() -> Option<Self> {
        let root = std::env::var_os("CODEX_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|home| home.join(".codex")))?;
        let sessions_dir = root.join("sessions");
        sessions_dir.is_dir().then_some(Self { sessions_dir })
    }
}

impl ModelUsageAdapter for CodexModelUsageAdapter {
    fn source_id(&self) -> &'static str {
        "codex"
    }

    fn fetch(&self) -> Result<ModelUsageSnapshot, AdapterError> {
        let fetched_at = SystemTime::now();
        let mut usage = UsageAccumulator::default();
        for (path, modified) in recent_files(&self.sessions_dir, ".jsonl") {
            parse_rollout(&path, modified, &mut usage)?;
        }
        Ok(ModelUsageSnapshot {
            source_id: "codex".to_owned(),
            fetched_at,
            models: usage.into_models(),
        })
    }
}

#[derive(Deserialize)]
struct TurnEnvelope {
    payload: TurnContext,
}

#[derive(Deserialize)]
struct TurnContext {
    model: String,
}

#[derive(Deserialize)]
struct TokenEnvelope {
    payload: TokenPayload,
}

#[derive(Deserialize)]
struct TokenPayload {
    info: Option<TokenInfo>,
}

#[derive(Deserialize)]
struct TokenInfo {
    last_token_usage: TokenBreakdown,
}

#[derive(Deserialize)]
struct TokenBreakdown {
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    cache_write_input_tokens: u64,
    output_tokens: u64,
}

fn parse_rollout(
    path: &std::path::Path,
    modified: SystemTime,
    usage: &mut UsageAccumulator,
) -> Result<(), AdapterError> {
    let file = fs::File::open(path)
        .map_err(|_| AdapterError::new(AdapterErrorKind::ProtocolChanged, SOURCE))?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut model = None;
    loop {
        line.clear();
        let bytes = reader
            .read_until(b'\n', &mut line)
            .map_err(|_| AdapterError::new(AdapterErrorKind::ProtocolChanged, SOURCE))?;
        if bytes == 0 {
            break;
        }
        if line.len() as u64 > MAX_USAGE_FILE_BYTES {
            continue;
        }
        if contains(&line, b"turn_context") {
            if let Ok(envelope) = serde_json::from_slice::<TurnEnvelope>(&line) {
                if !envelope.payload.model.is_empty() {
                    model = Some(envelope.payload.model);
                }
            }
        } else if contains(&line, b"token_count") {
            let Some(model_id) = model.as_ref() else {
                continue;
            };
            let Ok(envelope) = serde_json::from_slice::<TokenEnvelope>(&line) else {
                continue;
            };
            let Some(info) = envelope.payload.info else {
                continue;
            };
            usage.add(ModelUsage {
                agent_id: "codex".to_owned(),
                agent_name: "Codex".to_owned(),
                provider_id: "openai".to_owned(),
                model_id: model_id.clone(),
                requests: 1,
                failed_requests: 0,
                input_tokens: info.last_token_usage.input_tokens,
                output_tokens: info.last_token_usage.output_tokens,
                cache_read_tokens: info.last_token_usage.cached_input_tokens,
                cache_write_tokens: info.last_token_usage.cache_write_input_tokens,
                cost_usd: None,
                first_used_at: Some(modified),
                last_used_at: Some(modified),
            });
        }
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
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn rollout_parser_attributes_each_response_to_the_active_model() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("limitdeck-codex-{unique}.jsonl"));
        let fixture = concat!(
            "{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-test\"}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":100,\"cached_input_tokens\":80,\"cache_write_input_tokens\":4,\"output_tokens\":20}}}}\n",
            "{\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":50,\"cached_input_tokens\":40,\"output_tokens\":10}}}}\n"
        );
        fs::write(&path, fixture).unwrap();
        let mut accumulator = UsageAccumulator::default();
        parse_rollout(&path, UNIX_EPOCH + Duration::from_secs(1), &mut accumulator).unwrap();

        let models = accumulator.into_models();
        assert_eq!(models.len(), 1);
        assert_eq!((models[0].requests, models[0].input_tokens), (2, 150));
        assert_eq!(
            (models[0].output_tokens, models[0].cache_read_tokens),
            (30, 120)
        );
        assert_eq!(models[0].cache_write_tokens, 4);
        let _ = fs::remove_file(path);
    }
}
