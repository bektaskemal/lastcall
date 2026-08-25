//! Codex usage.
//!
//! Same principle as the Claude provider: no login flow of our own. The token
//! written by `codex login` is read from `~/.codex/auth.json` and used once.

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use super::{
    clamp_percent, http_client, parse_timestamp, reset_from_offset, ProviderError, UsageProvider,
};
use crate::usage::{ProviderKind, UsageWindow};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/codex/usage";
const AUTH_HINT: &str = "Run: codex login";

pub struct CodexProvider;

impl CodexProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CodexProvider {
    fn default() -> Self {
        Self::new()
    }
}

struct CodexAuth {
    access_token: String,
    account_id: Option<String>,
}

#[async_trait]
impl UsageProvider for CodexProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Codex
    }

    fn auth_check(&self) -> Result<(), ProviderError> {
        read_auth().map(|_| ())
    }

    async fn usage(&self) -> Result<Vec<UsageWindow>, ProviderError> {
        let auth = read_auth()?;
        let mut request = http_client()
            .map_err(ProviderError::Failed)?
            .get(USAGE_URL)
            .bearer_auth(&auth.access_token);
        if let Some(account_id) = &auth.account_id {
            request = request.header("chatgpt-account-id", account_id);
        }

        let response = request
            .send()
            .await
            .context("requesting Codex usage")
            .map_err(ProviderError::Failed)?;

        if matches!(response.status().as_u16(), 401 | 403) {
            return Err(ProviderError::NotAuthenticated { hint: AUTH_HINT });
        }
        if response.status().as_u16() == 429 {
            return Err(ProviderError::Failed(anyhow!(
                "rate limited by the API; the next check will retry"
            )));
        }
        let response = response
            .error_for_status()
            .context("Codex usage request failed")
            .map_err(ProviderError::Failed)?;
        let body: Value = response
            .json()
            .await
            .context("decoding Codex usage response")
            .map_err(ProviderError::Failed)?;

        parse_usage(&body, Utc::now()).map_err(ProviderError::Failed)
    }
}

fn read_auth() -> Result<CodexAuth, ProviderError> {
    let path = dirs::home_dir()
        .ok_or_else(|| ProviderError::Failed(anyhow!("no home directory")))?
        .join(".codex")
        .join("auth.json");

    let raw = std::fs::read_to_string(&path)
        .map_err(|_| ProviderError::NotAuthenticated { hint: AUTH_HINT })?;
    let parsed: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parsing {}", path.display()))
        .map_err(ProviderError::Failed)?;

    let access_token = parsed
        .pointer("/tokens/access_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or(ProviderError::NotAuthenticated { hint: AUTH_HINT })?
        .to_string();
    let account_id = parsed
        .pointer("/tokens/account_id")
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok(CodexAuth {
        access_token,
        account_id,
    })
}

/// Normalize the Codex rate-limit snapshot.
///
/// The payload nests the windows under `rate_limit` as `primary_window` /
/// `secondary_window`, each carrying a percent used, a window length in
/// seconds, and both a relative and an absolute reset. Extra limit buckets
/// (`code_review_rate_limit`, `additional_rate_limits`) are picked up too when
/// the account has them.
fn parse_usage(body: &Value, now: DateTime<Utc>) -> Result<Vec<UsageWindow>> {
    let mut windows: Vec<UsageWindow> = collect_entries(body)
        .iter()
        .filter_map(|(key, value)| window_from_entry(key, value, now))
        .collect();

    if windows.is_empty() {
        return Err(anyhow!("no usage windows in Codex response"));
    }
    windows.sort_by_key(|window| window.resets_at);
    windows.dedup_by(|a, b| a.name == b.name && a.resets_at == b.resets_at);
    Ok(windows)
}

/// Gather every `(name, object)` pair that could describe a quota window.
fn collect_entries(body: &Value) -> Vec<(String, Value)> {
    let mut entries = Vec::new();

    // The windows normally live under `rate_limit`; fall back to the root so a
    // flatter payload still parses.
    let nested = body.get("rate_limit").and_then(Value::as_object);
    if let Some(map) = nested.or_else(|| body.as_object()) {
        entries.extend(map.iter().map(|(key, value)| (key.clone(), value.clone())));
    }

    if let Some(value) = body.get("code_review_rate_limit") {
        entries.push(("code review".to_string(), value.clone()));
    }
    if let Some(extra) = body.get("additional_rate_limits").and_then(Value::as_array) {
        for (index, value) in extra.iter().enumerate() {
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("limit {}", index + 1));
            entries.push((name, value.clone()));
        }
    }
    entries
}

fn window_from_entry(key: &str, value: &Value, now: DateTime<Utc>) -> Option<UsageWindow> {
    let entry = value.as_object()?;
    let used_percent = entry.get("used_percent").and_then(Value::as_f64)?;
    let resets_at = reset_instant(entry, now)?;
    let window_seconds = entry.get("limit_window_seconds").and_then(Value::as_i64);

    Some(UsageWindow {
        provider: ProviderKind::Codex,
        name: window_name(key, window_seconds),
        remaining_percent: clamp_percent(100.0 - used_percent),
        resets_at,
        window_length: window_seconds
            .filter(|seconds| *seconds > 0)
            .map(Duration::seconds),
    })
}

/// Prefer the absolute reset; fall back to the relative offset.
fn reset_instant(
    entry: &serde_json::Map<String, Value>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if let Some(instant) = entry.get("reset_at").and_then(parse_timestamp) {
        return Some(instant);
    }
    entry
        .get("reset_after_seconds")
        .and_then(Value::as_i64)
        .map(|seconds| reset_from_offset(now, seconds))
}

/// Prefer a label derived from the window length; fall back to the payload key.
fn window_name(key: &str, window_seconds: Option<i64>) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    const WEEK: i64 = 7 * DAY;

    match window_seconds.filter(|seconds| *seconds > 0) {
        Some(seconds) if seconds % WEEK == 0 => match seconds / WEEK {
            1 => "weekly".to_string(),
            weeks => format!("{weeks}-weekly"),
        },
        Some(seconds) if seconds % DAY == 0 => match seconds / DAY {
            1 => "daily".to_string(),
            days => format!("{days}d"),
        },
        Some(seconds) if seconds % HOUR == 0 => format!("{}h", seconds / HOUR),
        Some(seconds) if seconds % MINUTE == 0 => format!("{}m", seconds / MINUTE),
        _ => key
            .trim_end_matches("_window")
            .trim_end_matches("_rate_limit")
            .replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    /// Shape observed from the live endpoint.
    #[test]
    fn normalizes_the_live_rate_limit_payload() {
        let body = json!({
            "account_id": "acct",
            "additional_rate_limits": null,
            "code_review_rate_limit": null,
            "plan_type": "pro",
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {
                    "limit_window_seconds": 604800,
                    "reset_after_seconds": 481856,
                    "reset_at": 1788148949i64,
                    "used_percent": 50
                },
                "secondary_window": null
            }
        });

        let windows = parse_usage(&body, now()).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].name, "weekly");
        assert_eq!(windows[0].remaining_percent, 50.0);
        assert_eq!(windows[0].provider, ProviderKind::Codex);
        assert_eq!(windows[0].resets_at.timestamp(), 1788148949);
        assert_eq!(windows[0].window_length, Some(Duration::days(7)));
    }

    #[test]
    fn normalizes_both_windows_when_present() {
        let body = json!({
            "rate_limit": {
                "primary_window": {
                    "limit_window_seconds": 18000,
                    "reset_after_seconds": 7860,
                    "used_percent": 37.0
                },
                "secondary_window": {
                    "limit_window_seconds": 604800,
                    "reset_after_seconds": 68400,
                    "used_percent": 66.0
                }
            }
        });

        let windows = parse_usage(&body, now()).unwrap();
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].name, "5h");
        assert_eq!(windows[0].remaining_percent, 63.0);
        assert_eq!(windows[0].resets_at, now() + Duration::seconds(7860));
        assert_eq!(windows[0].window_length, Some(Duration::hours(5)));
        assert_eq!(windows[1].name, "weekly");
        assert_eq!(windows[1].remaining_percent, 34.0);
    }

    #[test]
    fn picks_up_the_extra_limit_buckets() {
        let body = json!({
            "rate_limit": {
                "primary_window": {
                    "limit_window_seconds": 604800,
                    "reset_after_seconds": 1000,
                    "used_percent": 10.0
                }
            },
            "code_review_rate_limit": {
                "limit_window_seconds": 86400,
                "reset_after_seconds": 2000,
                "used_percent": 20.0
            },
            "additional_rate_limits": [
                { "name": "cloud", "reset_after_seconds": 3000, "used_percent": 30.0 }
            ]
        });

        let names: Vec<String> = parse_usage(&body, now())
            .unwrap()
            .into_iter()
            .map(|window| window.name)
            .collect();
        assert_eq!(names, vec!["weekly", "daily", "cloud"]);
    }

    #[test]
    fn accepts_a_flat_payload_with_an_absolute_reset() {
        let body = json!({
            "weekly": { "used_percent": 10.0, "reset_at": "2026-08-27T18:00:00Z" }
        });
        let windows = parse_usage(&body, now()).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].name, "weekly");
        assert_eq!(windows[0].remaining_percent, 90.0);
    }

    #[test]
    fn window_names_follow_the_window_length() {
        assert_eq!(window_name("primary_window", Some(18000)), "5h");
        assert_eq!(window_name("secondary_window", Some(604800)), "weekly");
        assert_eq!(window_name("secondary_window", Some(86400)), "daily");
        assert_eq!(window_name("secondary_window", Some(5400)), "90m");
        assert_eq!(window_name("secondary_window", Some(0)), "secondary");
        assert_eq!(window_name("primary_window", None), "primary");
        assert_eq!(window_name("code_review_rate_limit", None), "code review");
    }

    #[test]
    fn errors_on_an_empty_or_unusable_payload() {
        assert!(parse_usage(
            &json!({ "rate_limit": { "secondary_window": null } }),
            now()
        )
        .is_err());
        assert!(parse_usage(&json!(42), now()).is_err());
    }
}
