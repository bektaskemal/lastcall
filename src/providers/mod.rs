//! Provider layer. Everything provider-specific lives below this module; the
//! rule engine and the renderer only ever see `UsageWindow` values.

pub mod claude;
pub mod codex;

use std::fmt;
use std::time::Duration as StdDuration;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

use crate::config::Config;
use crate::usage::{ProviderKind, UsageWindow};

/// Environment variable that swaps in fixed sample data instead of live
/// providers. Used by the acceptance tests and handy for demos.
pub const MOCK_ENV: &str = "LASTCALL_MOCK";

#[async_trait]
pub trait UsageProvider {
    fn kind(&self) -> ProviderKind;
    async fn usage(&self) -> Result<Vec<UsageWindow>, ProviderError>;

    /// Whether a usable session exists, without making a network request.
    /// Used by `lastcall status`.
    fn auth_check(&self) -> Result<(), ProviderError>;
}

/// A provider failure. Authentication problems are separated out because they
/// are the only ones with a useful next step for the user.
#[derive(Debug)]
pub enum ProviderError {
    NotAuthenticated { hint: &'static str },
    Failed(anyhow::Error),
}

impl ProviderError {
    pub fn summary(&self) -> String {
        match self {
            ProviderError::NotAuthenticated { .. } => "not authenticated".to_string(),
            // `{:#}` keeps the source chain, so a bare "request failed" never
            // hides the status code that explains it.
            ProviderError::Failed(err) => format!("{err:#}"),
        }
    }

    pub fn hint(&self) -> Option<&'static str> {
        match self {
            ProviderError::NotAuthenticated { hint } => Some(hint),
            ProviderError::Failed(_) => None,
        }
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.summary())
    }
}

impl From<anyhow::Error> for ProviderError {
    fn from(err: anyhow::Error) -> Self {
        ProviderError::Failed(err)
    }
}

/// The providers lastcall knows about, in display order.
pub fn all() -> Vec<Box<dyn UsageProvider + Send + Sync>> {
    if std::env::var_os(MOCK_ENV).is_some() {
        return vec![
            Box::new(MockProvider::new(ProviderKind::Claude)),
            Box::new(MockProvider::new(ProviderKind::Codex)),
        ];
    }
    vec![
        Box::new(claude::ClaudeProvider::new()),
        Box::new(codex::CodexProvider::new()),
    ]
}

/// Only the providers the user has left enabled. A disabled provider is never
/// queried, displayed, or warned about.
pub fn enabled(config: &Config) -> Vec<Box<dyn UsageProvider + Send + Sync>> {
    all()
        .into_iter()
        .filter(|provider| config.is_enabled(provider.kind()))
        .collect()
}

pub(crate) fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(StdDuration::from_secs(15))
        .user_agent(concat!("lastcall/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// Clamp a provider-reported percentage into `0.0..=100.0`.
pub(crate) fn clamp_percent(value: f64) -> f64 {
    value.clamp(0.0, 100.0)
}

/// Turn a `resets_in_seconds`-style relative offset into an absolute instant.
pub(crate) fn reset_from_offset(now: DateTime<Utc>, seconds: i64) -> DateTime<Utc> {
    now + Duration::seconds(seconds.max(0))
}

/// Parse the timestamp forms providers actually emit: RFC 3339 strings and
/// numeric unix seconds (or milliseconds).
pub(crate) fn parse_timestamp(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    if let Some(text) = value.as_str() {
        if let Ok(parsed) = DateTime::parse_from_rfc3339(text) {
            return Some(parsed.with_timezone(&Utc));
        }
        if let Ok(seconds) = text.parse::<i64>() {
            return from_unix(seconds);
        }
        return None;
    }
    let seconds = value.as_i64()?;
    from_unix(seconds)
}

fn from_unix(value: i64) -> Option<DateTime<Utc>> {
    // Values past the year 3000 are milliseconds, not seconds.
    let seconds = if value > 32_503_680_000 {
        value / 1000
    } else {
        value
    };
    DateTime::from_timestamp(seconds, 0)
}

/// Fixed sample data for `LASTCALL_MOCK`, matching the README example.
struct MockProvider {
    kind: ProviderKind,
}

impl MockProvider {
    fn new(kind: ProviderKind) -> Self {
        Self { kind }
    }
}

#[async_trait]
impl UsageProvider for MockProvider {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    fn auth_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }

    async fn usage(&self) -> Result<Vec<UsageWindow>, ProviderError> {
        let now = Utc::now();
        let windows = match self.kind {
            ProviderKind::Claude => vec![
                UsageWindow {
                    provider: self.kind,
                    name: "5h".to_string(),
                    remaining_percent: 63.0,
                    resets_at: now + Duration::minutes(131),
                    window_length: Some(Duration::hours(5)),
                },
                UsageWindow {
                    provider: self.kind,
                    name: "weekly".to_string(),
                    remaining_percent: 18.0,
                    resets_at: now + Duration::hours(20),
                    window_length: Some(Duration::days(7)),
                },
            ],
            ProviderKind::Codex => vec![UsageWindow {
                provider: self.kind,
                name: "weekly".to_string(),
                remaining_percent: 34.0,
                resets_at: now + Duration::hours(19),
                window_length: Some(Duration::days(7)),
            }],
        };
        Ok(windows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_rfc3339_and_unix_timestamps() {
        let expected = DateTime::parse_from_rfc3339("2026-08-27T18:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            parse_timestamp(&json!("2026-08-27T18:00:00Z")),
            Some(expected)
        );
        assert_eq!(
            parse_timestamp(&json!(expected.timestamp())),
            Some(expected)
        );
        assert_eq!(
            parse_timestamp(&json!(expected.timestamp() * 1000)),
            Some(expected)
        );
        assert_eq!(parse_timestamp(&json!("not a date")), None);
        assert_eq!(parse_timestamp(&json!(null)), None);
    }

    #[test]
    fn clamps_out_of_range_percentages() {
        assert_eq!(clamp_percent(-4.0), 0.0);
        assert_eq!(clamp_percent(140.0), 100.0);
        assert_eq!(clamp_percent(33.5), 33.5);
    }
}
