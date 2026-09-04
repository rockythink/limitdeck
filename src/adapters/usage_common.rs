use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::domain::ModelUsage;

pub const MAX_USAGE_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_USAGE_FILES: usize = 2_048;
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

pub fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn cache_dir() -> Option<PathBuf> {
    env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".cache")))
        .map(|root| root.join("limitdeck"))
}

pub fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    let mut input = Vec::new();
    fs::File::open(path)?
        .take(MAX_USAGE_FILE_BYTES + 1)
        .read_to_end(&mut input)?;
    if input.len() as u64 > MAX_USAGE_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "usage file too large",
        ));
    }
    Ok(input)
}

pub fn recent_files(root: &Path, suffix: &str) -> Vec<(PathBuf, SystemTime)> {
    let mut pending = vec![root.to_owned()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(path);
            } else if metadata.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with(suffix))
                && metadata.len() <= MAX_USAGE_FILE_BYTES
            {
                files.push((path, metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
            }
        }
    }
    files.sort_unstable_by_key(|(_, modified)| std::cmp::Reverse(*modified));
    files.truncate(MAX_USAGE_FILES);
    let mut total = 0u64;
    files
        .into_iter()
        .take_while(|(path, _)| {
            let length = fs::metadata(path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            total = total.saturating_add(length);
            total <= MAX_TOTAL_BYTES
        })
        .collect()
}

#[derive(Default)]
pub struct UsageAccumulator {
    entries: BTreeMap<(String, String), ModelUsage>,
}

impl UsageAccumulator {
    pub fn add(&mut self, usage: ModelUsage) {
        let key = (usage.provider_id.clone(), usage.model_id.clone());
        if let Some(existing) = self.entries.get_mut(&key) {
            existing.merge(&usage);
        } else {
            self.entries.insert(key, usage);
        }
    }

    pub fn into_models(self) -> Vec<ModelUsage> {
        self.entries.into_values().collect()
    }
}

pub fn provider_for_model(model: &str) -> String {
    if let Some((provider, _)) = model.split_once('/') {
        return provider.to_owned();
    }
    let lower = model.to_ascii_lowercase();
    if lower.contains("claude") {
        "anthropic".to_owned()
    } else if lower.contains("gemini") {
        "google".to_owned()
    } else if lower.starts_with("gpt-") || lower.starts_with('o') {
        "openai".to_owned()
    } else {
        "unknown".to_owned()
    }
}
