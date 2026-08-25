//! Claude Code usage.
//!
//! lastcall never logs in. It reuses the OAuth session that Claude Code already
//! wrote to `~/.claude/.credentials.json` (or `CLAUDE_CODE_OAUTH_TOKEN`), reads
//! the token, and never copies or persists it anywhere.

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::Value;

use super::{clamp_percent, http_client, parse_timestamp, ProviderError, UsageProvider};
use crate::usage::{ProviderKind, UsageWindow};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const TOKEN_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";
const AUTH_HINT: &str = "Run Claude Code and sign in first.";

pub struct ClaudeProvider;

impl ClaudeProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ClaudeProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl UsageProvider for ClaudeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Claude
    }

    fn auth_check(&self) -> Result<(), ProviderError> {
        access_token().map(|_| ())
    }

    async fn usage(&self) -> Result<Vec<UsageWindow>, ProviderError> {
        let token = access_token()?;
        let response = http_client()
            .map_err(ProviderError::Failed)?
            .get(USAGE_URL)
            .bearer_auth(&token)
            .header("anthropic-beta", OAUTH_BETA)
            .send()
            .await
            .context("requesting Claude usage")
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
            .context("Claude usage request failed")
            .map_err(ProviderError::Failed)?;
        let body: Value = response
            .json()
            .await
            .context("decoding Claude usage response")
            .map_err(ProviderError::Failed)?;

        parse_usage(&body).map_err(ProviderError::Failed)
    }
}

/// Read the Claude Code OAuth access token without storing it.
fn access_token() -> Result<String, ProviderError> {
    if let Ok(token) = std::env::var(TOKEN_ENV) {
        if !token.trim().is_empty() {
            return Ok(token);
        }
    }

    let path = dirs::home_dir()
        .ok_or_else(|| ProviderError::Failed(anyhow!("no home directory")))?
        .join(".claude")
        .join(".credentials.json");

    let raw = std::fs::read_to_string(&path)
        .map_err(|_| ProviderError::NotAuthenticated { hint: AUTH_HINT })?;
    let parsed: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parsing {}", path.display()))
        .map_err(ProviderError::Failed)?;

    // Claude Code refreshes this token whenever it runs. lastcall deliberately
    // does not: refreshing would rotate the token behind Claude Code's back and
    // means writing credentials back to disk. An already-expired token is
    // reported as such instead of being sent on a doomed request.
    if let Some(expires_at) = parsed
        .pointer("/claudeAiOauth/expiresAt")
        .and_then(Value::as_i64)
    {
        if expires_at <= Utc::now().timestamp_millis() {
            return Err(ProviderError::NotAuthenticated { hint: AUTH_HINT });
        }
    }

    parsed
        .pointer("/claudeAiOauth/accessToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .ok_or(ProviderError::NotAuthenticated { hint: AUTH_HINT })
}

/// Normalize the usage payload.
///
/// The response is an object of named windows. Rather than hard-coding the set,
/// accept every entry that carries a utilization and a reset timestamp, so a
/// newly added window shows up without a code change.
fn parse_usage(body: &Value) -> Result<Vec<UsageWindow>> {
    let root = body
        .get("usage")
        .and_then(Value::as_object)
        .or_else(|| body.as_object())
        .ok_or_else(|| anyhow!("unexpected Claude usage payload"))?;

    let mut windows: Vec<UsageWindow> = root
        .iter()
        .filter_map(|(key, value)| window_from_entry(key, value))
        .collect();

    if windows.is_empty() {
        return Err(anyhow!("no usage windows in Claude response"));
    }
    windows.sort_by_key(|window| window.resets_at);
    Ok(windows)
}

fn window_from_entry(key: &str, value: &Value) -> Option<UsageWindow> {
    let entry = value.as_object()?;
    let resets_at = entry.get("resets_at").and_then(parse_timestamp)?;
    let utilization = entry.get("utilization").and_then(Value::as_f64)?;

    Some(UsageWindow {
        provider: ProviderKind::Claude,
        name: window_name(key),
        remaining_percent: clamp_percent(100.0 - utilization),
        resets_at,
        window_length: window_length(key),
    })
}

/// Claude names its windows rather than reporting their length, so read the
/// length off the key.
fn window_length(key: &str) -> Option<Duration> {
    if key.starts_with("five_hour") || key.starts_with("fiveHour") {
        return Some(Duration::hours(5));
    }
    if key.starts_with("seven_day") || key.starts_with("sevenDay") {
        return Some(Duration::days(7));
    }
    None
}

/// Map API window keys onto the short labels used in the output.
fn window_name(key: &str) -> String {
    match key {
        "five_hour" | "fiveHour" => "5h".to_string(),
        "seven_day" | "sevenDay" => "weekly".to_string(),
        other => other
            .trim_start_matches("seven_day_")
            .replace('_', " ")
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_every_reported_window() {
        let body = json!({
            "five_hour": { "utilization": 37, "resets_at": "2026-08-25T14:11:00Z" },
            "seven_day": { "utilization": 82, "resets_at": "2026-08-26T08:00:00Z" },
            "seven_day_opus": { "utilization": 69, "resets_at": "2026-08-26T08:00:00Z" }
        });

        let windows = parse_usage(&body).unwrap();
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].name, "5h");
        assert_eq!(windows[0].remaining_percent, 63.0);
        assert_eq!(windows[0].provider, ProviderKind::Claude);
        assert_eq!(windows[0].window_length, Some(Duration::hours(5)));
        let names: Vec<&str> = windows.iter().map(|w| w.name.as_str()).collect();
        assert!(names.contains(&"weekly"));
        assert!(names.contains(&"opus"));
    }

    #[test]
    fn accepts_a_usage_wrapper_and_a_numeric_reset() {
        let body = json!({
            "usage": {
                "seven_day": { "utilization": 66.0, "resets_at": 1787000000i64 }
            }
        });
        let windows = parse_usage(&body).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].remaining_percent, 34.0);
        assert_eq!(windows[0].resets_at.timestamp(), 1787000000);
        assert_eq!(windows[0].window_length, Some(Duration::days(7)));
    }

    #[test]
    fn skips_entries_that_are_not_usage_windows() {
        let body = json!({
            "account_uuid": "abc",
            "five_hour": { "utilization": 10, "resets_at": "2026-08-25T14:00:00Z" }
        });
        assert_eq!(parse_usage(&body).unwrap().len(), 1);
    }

    #[test]
    fn errors_when_no_window_is_present() {
        assert!(parse_usage(&json!({ "account_uuid": "abc" })).is_err());
        assert!(parse_usage(&json!("nope")).is_err());
    }

    #[test]
    fn windows_are_ordered_by_reset_time() {
        let body = json!({
            "seven_day": { "utilization": 10, "resets_at": "2026-08-27T08:00:00Z" },
            "five_hour": { "utilization": 10, "resets_at": "2026-08-25T14:00:00Z" }
        });
        let windows = parse_usage(&body).unwrap();
        assert_eq!(windows[0].name, "5h");
    }
}
