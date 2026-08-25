//! Desktop notifications via `notify-send`.

use std::process::Command;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};

use crate::usage::{humanize_long, ProviderKind, UsageWindow};

const BINARY: &str = "notify-send";

/// Compose the notification body. Kept separate from sending so it can be tested.
pub fn body(window: &UsageWindow, now: DateTime<Utc>) -> String {
    format!(
        "{:.0}% remaining.\nResets in {}.",
        window.remaining_percent,
        humanize_long(window.resets_at - now)
    )
}

/// A provider can report several windows, so the title names the one that
/// triggered rather than just the provider.
pub fn window_summary(window: &UsageWindow) -> String {
    format!("Lastcall: {} {} quota", window.provider, window.name)
}

pub fn summary(provider: ProviderKind) -> String {
    format!("Lastcall: {provider}")
}

/// Body of the "your session expired" warning.
pub fn auth_warning_body(hint: &str) -> String {
    format!("Quota checks stopped: not signed in.\n{hint}")
}

/// Warn that background checks cannot run any more. Without this, an expired
/// session turns `lastcall enable` into silence and the user never finds out.
pub fn send_auth_warning(provider: ProviderKind, hint: &str) -> Result<()> {
    notify_send(&summary(provider), &auth_warning_body(hint))
}

pub fn send(window: &UsageWindow, now: DateTime<Utc>) -> Result<()> {
    notify_send(&window_summary(window), &body(window, now))
}

fn notify_send(summary: &str, body: &str) -> Result<()> {
    let status = Command::new(BINARY)
        .arg("--app-name=lastcall")
        .arg("--urgency=normal")
        .arg(summary)
        .arg(body)
        .status()
        .with_context(|| format!("running {BINARY}"))?;

    if !status.success() {
        bail!("{BINARY} exited with {status}");
    }
    Ok(())
}

/// `lastcall enable` refuses to set up a timer that could never notify.
pub fn ensure_available() -> Result<()> {
    let found = Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {BINARY}"))
        .output()
        .with_context(|| format!("looking for {BINARY}"))?
        .status
        .success();

    if !found {
        bail!("{BINARY} was not found. Install it (libnotify / libnotify-bin) and try again.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::ProviderKind;
    use chrono::Duration;

    #[test]
    fn body_matches_the_documented_notification() {
        let now = Utc::now();
        let window = UsageWindow {
            provider: ProviderKind::Codex,
            name: "weekly".to_string(),
            remaining_percent: 34.0,
            resets_at: now + Duration::hours(19),
            window_length: Some(Duration::days(7)),
        };
        assert_eq!(summary(window.provider), "Lastcall: Codex");
        assert_eq!(body(&window, now), "34% remaining.\nResets in 19 hours.");
    }
}
