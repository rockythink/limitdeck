use std::{
    env, fs,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::domain::{CodingPlan, UsageStatus};

const FORMAT_VERSION: u8 = 1;
const MAX_INPUT_BYTES: usize = 1024 * 1024;
const MAX_SAMPLES_PER_SERIES: usize = 2048;
const MIN_UNCHANGED_INTERVAL: Duration = Duration::from_secs(15 * 60);
const RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Debug, Deserialize)]
struct HistoryFile {
    version: u8,
    series: Vec<HistorySeries>,
}

#[derive(Serialize)]
struct HistoryFileRef<'a> {
    version: u8,
    series: &'a [HistorySeries],
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct HistorySeries {
    provider_id: String,
    plan_id: String,
    window_id: String,
    samples: Vec<HistorySample>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct HistorySample {
    pub(crate) at_millis: u64,
    pub(crate) remaining_percent: u8,
}

#[derive(Debug, Default)]
pub(crate) struct UsageHistory {
    path: Option<PathBuf>,
    series: Vec<HistorySeries>,
}

impl UsageHistory {
    pub(crate) fn load_default() -> Self {
        let Some(path) = default_path() else {
            return Self::default();
        };
        let series = read_history(&path).unwrap_or_default();
        Self {
            path: Some(path),
            series,
        }
    }

    #[cfg(test)]
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    pub(crate) fn record(&mut self, plan: &CodingPlan) -> io::Result<bool> {
        let at_millis = system_time_millis(plan.fetched_at)?;
        let cutoff = at_millis.saturating_sub(RETENTION.as_millis() as u64);
        let mut changed = false;

        self.series.retain_mut(|series| {
            let before = series.samples.len();
            series.samples.retain(|sample| sample.at_millis >= cutoff);
            changed |= series.samples.len() != before;
            !series.samples.is_empty()
        });

        for window in plan
            .windows
            .iter()
            .filter(|window| window.status == UsageStatus::Available)
        {
            let series = if let Some(series) = self.series.iter_mut().find(|series| {
                series.provider_id == plan.provider_id
                    && series.plan_id == plan.id
                    && series.window_id == window.id
            }) {
                series
            } else {
                self.series.push(HistorySeries {
                    provider_id: plan.provider_id.clone(),
                    plan_id: plan.id.clone(),
                    window_id: window.id.clone(),
                    samples: Vec::new(),
                });
                changed = true;
                self.series.last_mut().expect("history series was inserted")
            };

            let has_bootstrap_samples = series.samples.len() >= 2;
            if let Some(last) = series.samples.last_mut() {
                if at_millis < last.at_millis {
                    continue;
                }
                if at_millis == last.at_millis {
                    if last.remaining_percent != window.remaining_percent {
                        last.remaining_percent = window.remaining_percent;
                        changed = true;
                    }
                    continue;
                }
                if has_bootstrap_samples
                    && last.remaining_percent == window.remaining_percent
                    && at_millis - last.at_millis < MIN_UNCHANGED_INTERVAL.as_millis() as u64
                {
                    continue;
                }
            }

            series.samples.push(HistorySample {
                at_millis,
                remaining_percent: window.remaining_percent.min(100),
            });
            if series.samples.len() > MAX_SAMPLES_PER_SERIES {
                let excess = series.samples.len() - MAX_SAMPLES_PER_SERIES;
                series.samples.drain(..excess);
            }
            changed = true;
        }

        if changed {
            self.save()?;
        }
        Ok(changed)
    }

    pub(crate) fn samples(
        &self,
        provider_id: &str,
        plan_id: &str,
        window_id: &str,
    ) -> &[HistorySample] {
        self.series
            .iter()
            .find(|series| {
                series.provider_id == provider_id
                    && series.plan_id == plan_id
                    && series.window_id == window_id
            })
            .map_or(&[], |series| series.samples.as_slice())
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "history path has no parent")
        })?;
        fs::create_dir_all(parent)?;
        let encoded = serde_json::to_vec(&HistoryFileRef {
            version: FORMAT_VERSION,
            series: &self.series,
        })?;
        if encoded.len() > MAX_INPUT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "history exceeds size limit",
            ));
        }
        let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
        fs::write(&temporary, encoded)?;
        fs::rename(&temporary, path).inspect_err(|_| {
            let _ = fs::remove_file(&temporary);
        })
    }
}

fn read_history(path: &Path) -> io::Result<Vec<HistorySeries>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let mut encoded = Vec::new();
    File::open(path)?
        .take((MAX_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut encoded)?;
    if encoded.len() > MAX_INPUT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "history exceeds size limit",
        ));
    }
    let mut history: HistoryFile = serde_json::from_slice(&encoded)?;
    if history.version != FORMAT_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported history version",
        ));
    }
    history.series.retain_mut(|series| {
        series
            .samples
            .retain(|sample| sample.remaining_percent <= 100);
        if series.samples.len() > MAX_SAMPLES_PER_SERIES {
            let excess = series.samples.len() - MAX_SAMPLES_PER_SERIES;
            series.samples.drain(..excess);
        }
        !series.provider_id.is_empty()
            && !series.plan_id.is_empty()
            && !series.window_id.is_empty()
            && !series.samples.is_empty()
    });
    Ok(history.series)
}

fn default_path() -> Option<PathBuf> {
    if let Some(directory) = env::var_os("XDG_CACHE_HOME") {
        return Some(PathBuf::from(directory).join("limitdeck/history.json"));
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".cache/limitdeck/history.json"))
}

fn system_time_millis(value: SystemTime) -> io::Result<u64> {
    value
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "time predates Unix epoch"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{UsageStatus, UsageWindow};

    fn plan(at_millis: u64, remaining_percent: u8) -> CodingPlan {
        CodingPlan {
            id: "openai-codex".to_owned(),
            provider_id: "openai".to_owned(),
            display_name: "private@example.com".to_owned(),
            fetched_at: UNIX_EPOCH + Duration::from_millis(at_millis),
            windows: vec![UsageWindow {
                id: "weekly".to_owned(),
                label: "7 day".to_owned(),
                period: Some(Duration::from_secs(7 * 24 * 60 * 60)),
                remaining_percent,
                resets_at: None,
                status: UsageStatus::Available,
            }],
        }
    }

    fn temporary_path(name: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "limitdeck-history-{name}-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn records_real_samples_and_bootstraps_a_flat_trend() {
        let mut history = UsageHistory::empty();
        assert!(history.record(&plan(1_000, 61)).unwrap());
        assert!(history.record(&plan(2_000, 61)).unwrap());
        assert!(!history.record(&plan(3_000, 61)).unwrap());
        assert!(history.record(&plan(4_000, 60)).unwrap());

        let samples = history.samples("openai", "openai-codex", "weekly");
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[0].remaining_percent, 61);
        assert_eq!(samples[2].remaining_percent, 60);
    }

    #[test]
    fn persisted_history_contains_only_quota_identifiers_and_values() {
        let path = temporary_path("privacy");
        let mut history = UsageHistory {
            path: Some(path.clone()),
            series: Vec::new(),
        };
        history.record(&plan(1_000, 61)).unwrap();

        let encoded = fs::read_to_string(&path).unwrap();
        assert!(encoded.contains("openai-codex"));
        assert!(encoded.contains("remaining_percent"));
        assert!(!encoded.contains("private@example.com"));
        assert!(!encoded.contains("7 day"));

        let loaded = read_history(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].samples[0].remaining_percent, 61);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn out_of_range_samples_are_discarded_on_load() {
        let path = temporary_path("invalid");
        fs::write(
            &path,
            r#"{"version":1,"series":[{"provider_id":"openai","plan_id":"codex","window_id":"weekly","samples":[{"at_millis":1,"remaining_percent":255}]}]}"#,
        )
        .unwrap();
        assert!(read_history(&path).unwrap().is_empty());
        fs::remove_file(path).unwrap();
    }
}
