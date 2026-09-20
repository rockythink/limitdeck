#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    collections::BTreeMap,
    io::{self, BufRead, BufReader, Read, Write},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;
use wait_timeout::ChildExt;

use crate::{
    adapter::{AdapterError, AdapterErrorKind, PlanAdapter},
    domain::{CodingPlan, PlanIdentity, UsageStatus, UsageWindow},
};

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
const SOURCE: &str = "Codex CLI";

fn adapter_error(kind: AdapterErrorKind) -> AdapterError {
    AdapterError::new(kind, SOURCE)
}

fn protocol_error() -> AdapterError {
    adapter_error(AdapterErrorKind::ProtocolChanged)
}

fn spawn_error(error: &io::Error) -> AdapterError {
    let kind = if error.kind() == io::ErrorKind::NotFound {
        AdapterErrorKind::CommandNotFound
    } else {
        AdapterErrorKind::ProtocolChanged
    };
    adapter_error(kind)
}

fn classify_remote_error(message: &str) -> AdapterErrorKind {
    const AUTH_MARKERS: [&str; 5] = ["auth", "login", "sign in", "credential", "unauthorized"];
    if AUTH_MARKERS
        .iter()
        .any(|marker| contains_ascii_case_insensitive(message, marker))
    {
        AdapterErrorKind::NotAuthenticated
    } else {
        AdapterErrorKind::ProtocolChanged
    }
}

fn contains_ascii_case_insensitive(haystack: &str, needle: &str) -> bool {
    haystack.as_bytes().windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle.bytes())
            .all(|(left, right)| left.eq_ignore_ascii_case(&right))
    })
}
const INITIALIZE_MESSAGE: &str = concat!(
    r#"{"method":"initialize","id":1,"params":{"clientInfo":{"name":"limitdeck","title":"LimitDeck","version":""#,
    env!("CARGO_PKG_VERSION"),
    r#""}}}"#
);

pub struct CodexAppServerAdapter;

impl CodexAppServerAdapter {
    pub fn discover() -> Option<Self> {
        command_succeeds("codex", &["login", "status"], DISCOVERY_TIMEOUT).then_some(Self)
    }
}

impl PlanAdapter for CodexAppServerAdapter {
    fn identity(&self) -> PlanIdentity {
        PlanIdentity::new("openai-codex", "openai", "Codex")
    }

    fn fetch(&self) -> Result<CodingPlan, AdapterError> {
        fetch_from_app_server()
    }
}

fn fetch_from_app_server() -> Result<CodingPlan, AdapterError> {
    let mut command = Command::new("codex");
    command
        .args(["app-server", "--listen", "stdio://"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().map_err(|error| spawn_error(&error))?;
    crate::child_process::track(child.id());
    let Some(mut stdin) = child.stdin.take() else {
        stop_child(&mut child);
        return Err(protocol_error());
    };
    let Some(stdout) = child.stdout.take() else {
        stop_child(&mut child);
        return Err(protocol_error());
    };
    let (messages, receiver) = mpsc::sync_channel(4);
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_bounded_line(&mut reader) {
                Ok(Some(line)) => {
                    if messages.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = messages.send(Err(error));
                    break;
                }
            }
        }
    });

    let result = (|| {
        write_message(&mut stdin, INITIALIZE_MESSAGE)?;

        let started = Instant::now();
        loop {
            let remaining = REQUEST_TIMEOUT
                .checked_sub(started.elapsed())
                .ok_or_else(|| adapter_error(AdapterErrorKind::TimedOut))?;
            let line = receiver
                .recv_timeout(remaining)
                .map_err(|error| match error {
                    mpsc::RecvTimeoutError::Timeout => adapter_error(AdapterErrorKind::TimedOut),
                    mpsc::RecvTimeoutError::Disconnected => protocol_error(),
                })??;
            let response: ResponseId = serde_json::from_str(&line).map_err(|_| protocol_error())?;
            if let Some(remote_error) = response.error {
                return Err(adapter_error(classify_remote_error(&remote_error.message)));
            }
            match response.id {
                Some(1) => {
                    write_message(&mut stdin, r#"{"method":"initialized","params":{}}"#)?;
                    write_message(&mut stdin, r#"{"method":"account/rateLimits/read","id":2}"#)?;
                }
                Some(2) => {
                    break parse_rate_limit_response(line.as_bytes(), SystemTime::now());
                }
                _ => {}
            }
        }
    })();

    drop(stdin);
    drop(receiver);
    stop_child(&mut child);
    result
}
fn read_bounded_line(reader: &mut impl BufRead) -> Result<Option<String>, AdapterError> {
    let mut bytes = Vec::new();
    let read = reader
        .take((MAX_MESSAGE_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| protocol_error())?;
    if read == 0 {
        return Ok(None);
    }
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(protocol_error());
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| protocol_error())
}

fn write_message(stdin: &mut impl Write, message: &str) -> Result<(), AdapterError> {
    stdin
        .write_all(message.as_bytes())
        .map_err(|_| protocol_error())?;
    stdin.write_all(b"\n").map_err(|_| protocol_error())?;
    stdin.flush().map_err(|_| protocol_error())
}

fn stop_child(child: &mut Child) {
    #[cfg(unix)]
    if let Ok(group) = i32::try_from(child.id()) {
        // SAFETY: this child was started in its own process group.
        unsafe {
            libc::kill(-group, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    crate::child_process::forget(child.id());
}

fn command_succeeds(program: &str, args: &[&str], timeout: Duration) -> bool {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    command.process_group(0);
    let Ok(mut child) = command.spawn() else {
        return false;
    };
    crate::child_process::track(child.id());
    match child.wait_timeout(timeout) {
        Ok(Some(status)) => {
            stop_child(&mut child);
            status.success()
        }
        Ok(None) | Err(_) => {
            stop_child(&mut child);
            false
        }
    }
}

#[derive(Deserialize)]
struct ResponseId {
    id: Option<u64>,
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    message: String,
}

#[derive(Deserialize)]
struct RateLimitRpcResponse {
    id: u64,
    result: Option<RateLimitResponse>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitResponse {
    rate_limits: RateLimitSnapshot,
    rate_limits_by_limit_id: Option<BTreeMap<String, RateLimitSnapshot>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitSnapshot {
    limit_name: Option<String>,
    primary: Option<SourceWindow>,
    secondary: Option<SourceWindow>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceWindow {
    used_percent: i32,
    window_duration_mins: Option<u64>,
    resets_at: Option<i64>,
}

fn parse_rate_limit_response(
    input: &[u8],
    fetched_at: SystemTime,
) -> Result<CodingPlan, AdapterError> {
    let response: RateLimitRpcResponse =
        serde_json::from_slice(input).map_err(|_| protocol_error())?;
    if response.id != 2 {
        return Err(protocol_error());
    }
    let response = response.result.ok_or_else(protocol_error)?;
    let mut windows = Vec::new();
    let buckets = response
        .rate_limits_by_limit_id
        .filter(|buckets| !buckets.is_empty())
        .unwrap_or_else(|| BTreeMap::from([("codex".to_owned(), response.rate_limits)]));

    for (bucket_id, snapshot) in buckets {
        let label = snapshot.limit_name.unwrap_or_else(|| {
            if bucket_id == "codex" {
                "Codex".to_owned()
            } else {
                bucket_id.clone()
            }
        });
        push_window(
            &mut windows,
            &bucket_id,
            &label,
            "primary",
            snapshot.primary,
        );
        push_window(
            &mut windows,
            &bucket_id,
            &label,
            "secondary",
            snapshot.secondary,
        );
    }
    if windows.is_empty() {
        return Err(protocol_error());
    }

    Ok(CodingPlan {
        id: "openai-codex".to_owned(),
        provider_id: "openai".to_owned(),
        display_name: "Codex".to_owned(),
        fetched_at,
        windows,
    })
}

fn push_window(
    windows: &mut Vec<UsageWindow>,
    bucket_id: &str,
    label: &str,
    position: &str,
    source: Option<SourceWindow>,
) {
    let Some(source) = source else {
        return;
    };
    let period = source
        .window_duration_mins
        .and_then(|minutes| minutes.checked_mul(60))
        .map(Duration::from_secs);
    let resets_at = source
        .resets_at
        .and_then(|seconds| u64::try_from(seconds).ok())
        .and_then(|seconds| UNIX_EPOCH.checked_add(Duration::from_secs(seconds)));
    let used = source.used_percent.clamp(0, 100) as u8;
    windows.push(UsageWindow {
        id: format!("openai:{bucket_id}:{position}"),
        label: format!("{label} · {}", period_label(period)),
        period,
        remaining_percent: 100 - used,
        resets_at,
        status: UsageStatus::Available,
    });
}

fn period_label(period: Option<Duration>) -> String {
    let Some(period) = period else {
        return "Quota".to_owned();
    };
    let hours = period.as_secs() / 3600;
    if hours >= 24 && hours % 24 == 0 {
        format!("{} days", hours / 24)
    } else {
        format!("{hours} hours")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESPONSE: &[u8] = br#"{
        "id":2,
        "result":{
            "account":{"email":"private@example.com","id":"secret-account"},
            "rateLimits":{"planType":"plus","primary":{"usedPercent":35,"windowDurationMins":10080,"resetsAt":2000000000}},
            "rateLimitsByLimitId":{
                "codex":{"planType":"plus","primary":{"usedPercent":35,"windowDurationMins":10080,"resetsAt":2000000000}},
                "codex_spark":{"limitName":"Codex Spark","primary":{"usedPercent":20,"windowDurationMins":300,"resetsAt":2000100000},"secondary":{"usedPercent":55,"windowDurationMins":10080,"resetsAt":2000200000}}
            }
        }
    }"#;

    #[test]
    fn parses_official_rate_limits_without_identity_or_plan_metadata() {
        let plan = parse_rate_limit_response(RESPONSE, UNIX_EPOCH + Duration::from_secs(10))
            .expect("official response should parse");

        assert_eq!(plan.windows.len(), 3);
        assert_eq!(plan.windows[0].remaining_percent, 65);
        assert_eq!(plan.windows[1].period, Some(Duration::from_secs(300 * 60)));
        assert_eq!(plan.windows[2].remaining_percent, 45);
        let retained = format!("{plan:?}");
        for forbidden in ["private@example.com", "secret-account", "plus"] {
            assert!(!retained.contains(forbidden));
        }
    }
    #[test]
    fn rejects_an_oversized_app_server_message() {
        let input = vec![b'x'; MAX_MESSAGE_BYTES + 1];
        let mut reader = BufReader::new(input.as_slice());

        assert_eq!(read_bounded_line(&mut reader), Err(protocol_error()));
    }

    #[test]
    fn falls_back_to_the_legacy_single_bucket() {
        let response = br#"{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":10,"windowDurationMins":300,"resetsAt":null}},"rateLimitsByLimitId":null}}"#;
        let plan = parse_rate_limit_response(response, SystemTime::now())
            .expect("legacy response should remain supported");

        assert_eq!(plan.windows.len(), 1);
        assert_eq!(plan.windows[0].remaining_percent, 90);
    }

    #[test]
    fn remote_auth_errors_are_classified_without_retaining_the_message() {
        let secret = "Unauthorized for private@example.com with token sk-secret";
        let kind = classify_remote_error(secret);
        let error = adapter_error(kind);

        assert_eq!(error.kind, AdapterErrorKind::NotAuthenticated);
        assert!(!format!("{error:?}").contains(secret));
    }
}
