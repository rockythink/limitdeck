use std::{
    io::{self, Write},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;

use crate::{
    adapter::{AdapterErrorKind, PlanAdapter},
    adapters,
    domain::{CodingPlan, UsageStatus, UsageWindow},
};

const COLLECTION_TIMEOUT: Duration = Duration::from_secs(18);

#[derive(Serialize)]
struct Snapshot {
    version: u8,
    collected_at: Option<u64>,
    plans: Vec<Plan>,
}

#[derive(Serialize)]
struct Plan {
    id: String,
    name: String,
    fetched_at: Option<u64>,
    windows: Vec<Window>,
    error: Option<&'static str>,
}

#[derive(Serialize)]
struct Window {
    label: String,
    remaining_percent: Option<u8>,
    resets_at: Option<u64>,
}

pub(crate) fn write_json(mut output: impl Write) -> io::Result<()> {
    crate::child_process::install_snapshot_cancellation();
    let snapshot = collect(adapters::discover(), COLLECTION_TIMEOUT);
    crate::child_process::stop_snapshot_children();
    serde_json::to_writer(&mut output, &snapshot)?;
    output.write_all(b"\n")
}

fn collect(adapters: Vec<Box<dyn PlanAdapter>>, timeout: Duration) -> Snapshot {
    let started = Instant::now();
    let (sender, receiver) = mpsc::channel();
    let mut plans = Vec::with_capacity(adapters.len());
    for (index, adapter) in adapters.into_iter().enumerate() {
        let identity = adapter.identity();
        plans.push(Plan {
            id: identity.id,
            name: identity.display_name,
            fetched_at: None,
            windows: Vec::new(),
            error: Some("timed_out"),
        });
        let sender = sender.clone();
        thread::spawn(move || {
            let _ = sender.send((index, adapter.fetch()));
        });
    }
    drop(sender);
    while !crate::child_process::cancelled() && started.elapsed() < timeout {
        let remaining = timeout
            .saturating_sub(started.elapsed())
            .min(Duration::from_millis(50));
        let (index, result) = match receiver.recv_timeout(remaining) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let plan = &mut plans[index];
        match result {
            Ok(fetched) => apply_plan(plan, fetched),
            Err(error) => plan.error = Some(error_code(error.kind)),
        }
    }
    Snapshot {
        version: 1,
        collected_at: epoch(SystemTime::now()),
        plans,
    }
}

fn apply_plan(plan: &mut Plan, fetched: CodingPlan) {
    plan.fetched_at = epoch(fetched.fetched_at);
    plan.error = None;
    plan.windows = fetched
        .windows
        .into_iter()
        .enumerate()
        .map(|(index, window)| Window {
            // Remote labels and IDs are deliberately not serialized: they can contain account data.
            label: window_label(&window, index),
            remaining_percent: (window.status == UsageStatus::Available)
                .then_some(window.remaining_percent.min(100)),
            resets_at: window.resets_at.and_then(epoch),
        })
        .collect();
}

fn window_label(window: &UsageWindow, index: usize) -> String {
    let prefix = match window.id.as_str() {
        "anthropic:spend-limit" => return "Spend limit".to_owned(),
        "openai:codex_spark:primary"
        | "openai:codex_spark:secondary"
        | "openai:codex-spark:primary"
        | "openai:codex-spark:secondary"
        | "openai-codex:spark:primary"
        | "openai-codex:spark:secondary" => "Spark · ",
        _ if window.label.contains("Spark") || window.label.contains("spark") => "Spark · ",
        _ => "",
    };
    match window.period.map(|period| period.as_secs()) {
        Some(seconds) if seconds > 0 && seconds % 86400 == 0 => {
            format!("{prefix}{}d", seconds / 86400)
        }
        Some(seconds) if seconds > 0 && seconds % 3600 == 0 => {
            format!("{prefix}{}h", seconds / 3600)
        }
        Some(seconds) if seconds > 0 && seconds % 60 == 0 => format!("{prefix}{}m", seconds / 60),
        Some(seconds) if seconds > 0 => format!("{prefix}{seconds}s"),
        _ => format!("{prefix}Window {}", index + 1),
    }
}

fn epoch(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_secs())
}

fn error_code(kind: AdapterErrorKind) -> &'static str {
    match kind {
        AdapterErrorKind::CommandNotFound => "command_not_found",
        AdapterErrorKind::NotAuthenticated => "not_authenticated",
        AdapterErrorKind::TimedOut => "timed_out",
        AdapterErrorKind::ProtocolChanged => "protocol_changed",
        AdapterErrorKind::SnapshotMissing => "snapshot_missing",
        AdapterErrorKind::SnapshotExpired => "snapshot_expired",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{adapter::AdapterError, domain::PlanIdentity};

    struct Source {
        id: &'static str,
        fail: bool,
        delay: Duration,
    }
    impl PlanAdapter for Source {
        fn identity(&self) -> PlanIdentity {
            PlanIdentity::new(self.id, "provider", "Plan")
        }
        fn fetch(&self) -> Result<CodingPlan, AdapterError> {
            thread::sleep(self.delay);
            if self.fail {
                return Err(AdapterError::new(
                    AdapterErrorKind::NotAuthenticated,
                    "secret@example.com",
                ));
            }
            Ok(CodingPlan {
                id: "secret@example.com".into(),
                provider_id: "/private/path".into(),
                display_name: "secret-token".into(),
                fetched_at: UNIX_EPOCH + Duration::from_secs(123),
                windows: vec![UsageWindow {
                    id: "/private/path".into(),
                    label: "secret@example.com".into(),
                    period: Some(Duration::from_secs(18000)),
                    remaining_percent: 90,
                    resets_at: Some(UNIX_EPOCH + Duration::from_secs(456)),
                    status: UsageStatus::Unavailable,
                }],
            })
        }
    }

    #[test]
    fn isolates_failure_timeout_and_private_source_fields() {
        let snapshot = collect(
            vec![
                Box::new(Source {
                    id: "ok",
                    fail: false,
                    delay: Duration::ZERO,
                }),
                Box::new(Source {
                    id: "failed",
                    fail: true,
                    delay: Duration::ZERO,
                }),
                Box::new(Source {
                    id: "slow",
                    fail: false,
                    delay: Duration::from_millis(300),
                }),
            ],
            Duration::from_millis(100),
        );
        let json = serde_json::to_value(snapshot).unwrap();
        assert_eq!(
            json["plans"][0]["windows"][0]["remaining_percent"],
            serde_json::Value::Null
        );
        assert_eq!(json["plans"][0]["windows"][0]["resets_at"], 456);
        assert_eq!(json["plans"][1]["error"], "not_authenticated");
        assert_eq!(json["plans"][2]["error"], "timed_out");
        let encoded = json.to_string();
        assert!(!encoded.contains("secret"));
        assert!(!encoded.contains("/private"));
    }
}
