use std::{path::PathBuf, time::SystemTime};

use serde::Deserialize;

use crate::{
    adapter::{AdapterError, AdapterErrorKind, ModelUsageAdapter},
    adapters::usage_common::{home_dir, read_bounded, recent_files, UsageAccumulator},
    domain::{ModelUsage, ModelUsageSnapshot},
};

const SOURCE: &str = "Gemini CLI sessions";

pub struct GeminiModelUsageAdapter {
    sessions_root: PathBuf,
}

impl GeminiModelUsageAdapter {
    pub fn discover() -> Option<Self> {
        let root = std::env::var_os("GEMINI_CLI_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|home| home.join(".gemini")))?;
        let sessions_root = root.join("tmp");
        sessions_root.is_dir().then_some(Self { sessions_root })
    }
}

impl ModelUsageAdapter for GeminiModelUsageAdapter {
    fn source_id(&self) -> &'static str {
        "gemini-cli"
    }

    fn fetch(&self) -> Result<ModelUsageSnapshot, AdapterError> {
        let fetched_at = SystemTime::now();
        let mut usage = UsageAccumulator::default();
        for (path, modified) in recent_files(&self.sessions_root, ".json") {
            if !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("session-"))
            {
                continue;
            }
            let input = read_bounded(&path)
                .map_err(|_| AdapterError::new(AdapterErrorKind::ProtocolChanged, SOURCE))?;
            let Ok(session) = serde_json::from_slice::<GeminiSession>(&input) else {
                continue;
            };
            for message in session.messages {
                let (Some(model), Some(tokens)) = (message.model, message.tokens) else {
                    continue;
                };
                if message.kind.as_deref() != Some("gemini") || model.is_empty() {
                    continue;
                }
                usage.add(ModelUsage {
                    agent_id: "gemini-cli".to_owned(),
                    agent_name: "Gemini CLI".to_owned(),
                    provider_id: "google".to_owned(),
                    model_id: model,
                    requests: 1,
                    failed_requests: 0,
                    input_tokens: tokens.input,
                    output_tokens: tokens.output.saturating_add(tokens.thoughts),
                    cache_read_tokens: tokens.cached,
                    cache_write_tokens: 0,
                    cost_usd: None,
                    first_used_at: Some(modified),
                    last_used_at: Some(modified),
                });
            }
        }
        Ok(ModelUsageSnapshot {
            source_id: "gemini-cli".to_owned(),
            fetched_at,
            models: usage.into_models(),
        })
    }
}

#[derive(Deserialize)]
struct GeminiSession {
    #[serde(default)]
    messages: Vec<GeminiMessage>,
}

#[derive(Deserialize)]
struct GeminiMessage {
    #[serde(rename = "type")]
    kind: Option<String>,
    model: Option<String>,
    tokens: Option<GeminiTokens>,
}

#[derive(Deserialize)]
struct GeminiTokens {
    #[serde(default)]
    input: u64,
    #[serde(default)]
    output: u64,
    #[serde(default)]
    cached: u64,
    #[serde(default)]
    thoughts: u64,
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::ModelUsageAdapter;

    #[test]
    fn session_import_keeps_only_gemini_usage_metadata() {
        let unique = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("limitdeck-gemini-{unique}"));
        let chats = root.join("project/chats");
        std::fs::create_dir_all(&chats).unwrap();
        std::fs::write(
            chats.join("session-test.json"),
            br#"{"messages":[{"type":"user","content":"private prompt"},{"type":"gemini","model":"gemini-test","content":"private answer","tokens":{"input":100,"output":20,"cached":50,"thoughts":5}}]}"#,
        )
        .unwrap();
        let snapshot = GeminiModelUsageAdapter {
            sessions_root: root.clone(),
        }
        .fetch()
        .unwrap();

        assert_eq!(snapshot.models.len(), 1);
        let usage = &snapshot.models[0];
        assert_eq!((usage.input_tokens, usage.output_tokens), (100, 25));
        assert_eq!(usage.cache_read_tokens, 50);
        let retained = format!("{usage:?}");
        assert!(!retained.contains("private"));
        let _ = std::fs::remove_dir_all(root);
    }
}
