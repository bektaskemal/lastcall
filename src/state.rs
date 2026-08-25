//! Notification deduplication state.
//!
//! Terminal output is always current. Desktop notifications fire once per
//! warning tier and quota window, keyed by provider + window + tier + reset.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};

use crate::rules::Warning;
use crate::usage::{round_to_minute, ProviderKind, UsageWindow};

#[derive(Debug, Default)]
pub struct State {
    notified: BTreeMap<String, bool>,
}

impl State {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
        };
        // A corrupt state file must never block a notification run; the worst
        // case is one duplicate notification.
        Ok(Self {
            notified: serde_json::from_str(&raw).unwrap_or_default(),
        })
    }

    pub fn was_notified(&self, window: &UsageWindow, warning: Warning) -> bool {
        self.was_seen(&window_key(window, warning))
    }

    pub fn mark_notified(&mut self, window: &UsageWindow, warning: Warning) {
        self.mark_seen(window_key(window, warning));
    }

    pub fn was_seen(&self, key: &str) -> bool {
        self.notified.get(key).copied().unwrap_or(false)
    }

    pub fn mark_seen(&mut self, key: String) {
        self.notified.insert(key, true);
    }

    /// Drop entries whose reset instant has passed, so the file cannot grow
    /// without bound.
    pub fn prune(&mut self, now: DateTime<Utc>) {
        self.notified.retain(|key, _| match reset_of(key) {
            Some(resets_at) => resets_at > now,
            None => false,
        });
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(&self.notified)?;
        fs::write(path, body).with_context(|| format!("writing {}", path.display()))
    }
}

/// Key for one warning about one quota window.
///
/// The reset instant is rounded to the nearest minute first, because providers
/// report it with sub-second jitter that would otherwise look like a new window
/// on every single check. The trailing tier lets the early and final warnings
/// deduplicate independently.
///
/// Fields are `|`-separated so the RFC 3339 instant, which contains colons, is
/// always the last field and trivially recoverable.
pub fn window_key(window: &UsageWindow, warning: Warning) -> String {
    format!(
        "{}|{}|{}|{}",
        window.provider.slug(),
        window.name,
        warning.slug(),
        round_to_minute(window.resets_at).format("%Y-%m-%dT%H:%M:00Z")
    )
}

/// The reset instant is the last `|`-separated field.
fn reset_of(key: &str) -> Option<DateTime<Utc>> {
    let timestamp = key.rsplit('|').next()?;
    DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

/// Dedup key for "your session expired" warnings: at most one per provider per
/// day. The embedded instant is tomorrow midnight so `prune` expires it exactly
/// when a new day makes it stale.
pub fn auth_key(provider: ProviderKind, now: DateTime<Utc>) -> String {
    let tomorrow = (now + Duration::days(1)).date_naive().and_hms_opt(0, 0, 0);
    let expires = tomorrow
        .map(|naive| naive.and_utc())
        .unwrap_or(now + Duration::days(1));
    format!(
        "{}|auth|{}",
        provider.slug(),
        expires.format("%Y-%m-%dT%H:%M:00Z")
    )
}

pub fn state_path() -> PathBuf {
    let base = dirs::state_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("state")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("lastcall").join("state.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(name: &str, resets_at: DateTime<Utc>) -> UsageWindow {
        UsageWindow {
            provider: ProviderKind::Codex,
            name: name.to_string(),
            remaining_percent: 34.0,
            resets_at,
            window_length: Some(Duration::days(7)),
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn each_warning_is_sent_once() {
        let mut state = State::default();
        let w = window("weekly", now() + Duration::hours(19));

        assert!(!state.was_notified(&w, Warning::Early));
        state.mark_notified(&w, Warning::Early);
        assert!(state.was_notified(&w, Warning::Early));
    }

    /// The final call must not be suppressed by the early one.
    #[test]
    fn the_two_tiers_deduplicate_independently() {
        let mut state = State::default();
        let w = window("weekly", now() + Duration::hours(19));

        state.mark_notified(&w, Warning::Early);
        assert!(!state.was_notified(&w, Warning::Final));
        state.mark_notified(&w, Warning::Final);
        assert!(state.was_notified(&w, Warning::Final));
    }

    #[test]
    fn a_new_reset_instant_is_a_new_cycle() {
        let mut state = State::default();
        let first = window("weekly", now() + Duration::hours(19));
        state.mark_notified(&first, Warning::Final);
        let next = window("weekly", now() + Duration::days(7));
        assert!(!state.was_notified(&next, Warning::Final));
    }

    /// Providers report the reset instant with sub-second jitter, which must
    /// not look like a new window.
    #[test]
    fn the_key_absorbs_sub_second_jitter() {
        let at = |text: &str| {
            DateTime::parse_from_rfc3339(text)
                .unwrap()
                .with_timezone(&Utc)
        };
        let early = window("weekly", at("2026-08-25T22:59:59.847101+00:00"));
        let late = window("weekly", at("2026-08-25T23:00:00.279509+00:00"));

        assert_eq!(
            window_key(&early, Warning::Final),
            window_key(&late, Warning::Final)
        );
        assert_eq!(
            window_key(&early, Warning::Final),
            "codex|weekly|final|2026-08-25T23:00:00Z"
        );
    }

    #[test]
    fn round_trips_through_a_file() {
        let dir = std::env::temp_dir().join(format!("lastcall-state-{}", std::process::id()));
        let path = dir.join("state.json");
        let _ = fs::remove_dir_all(&dir);

        let w = window("weekly", now() + Duration::hours(19));
        let mut state = State::default();
        state.mark_notified(&w, Warning::Final);
        state.save(&path).unwrap();

        let reloaded = State::load(&path).unwrap();
        assert!(reloaded.was_notified(&w, Warning::Final));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_and_corrupt_files_load_as_empty() {
        let missing = std::env::temp_dir().join("lastcall-does-not-exist/state.json");
        assert!(State::load(&missing).is_ok());

        let path = std::env::temp_dir().join(format!("lastcall-bad-{}.json", std::process::id()));
        fs::write(&path, "{ not json").unwrap();
        let state = State::load(&path).unwrap();
        assert!(!state.was_notified(&window("weekly", now()), Warning::Final));
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn auth_warnings_are_deduplicated_per_day() {
        let morning = now();
        let evening = now() + Duration::hours(9);
        let next_day = now() + Duration::days(1);

        assert_eq!(
            auth_key(ProviderKind::Claude, morning),
            auth_key(ProviderKind::Claude, evening)
        );
        assert_ne!(
            auth_key(ProviderKind::Claude, morning),
            auth_key(ProviderKind::Claude, next_day)
        );
        assert_ne!(
            auth_key(ProviderKind::Claude, morning),
            auth_key(ProviderKind::Codex, morning)
        );
    }

    /// The embedded instant must survive today's prune and expire tomorrow.
    #[test]
    fn an_auth_warning_expires_when_the_day_does() {
        let mut state = State::default();
        let key = auth_key(ProviderKind::Claude, now());
        state.mark_seen(key.clone());

        state.prune(now());
        assert!(state.was_seen(&key));

        state.prune(now() + Duration::days(1));
        assert!(!state.was_seen(&key));
    }

    #[test]
    fn prune_drops_elapsed_and_unparsable_entries() {
        let mut state = State::default();
        let stale = window("weekly", now() - Duration::hours(1));
        let live = window("5h", now() + Duration::hours(2));
        state.mark_notified(&stale, Warning::Final);
        state.mark_notified(&live, Warning::Final);
        state.notified.insert("garbage".to_string(), true);

        state.prune(now());
        assert!(!state.was_notified(&stale, Warning::Final));
        assert!(state.was_notified(&live, Warning::Final));
        assert_eq!(state.notified.len(), 1);
    }
}
