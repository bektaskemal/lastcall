//! Normalized usage model shared by every provider.

use std::fmt;

use chrono::{DateTime, Duration, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    Claude,
    Codex,
}

impl ProviderKind {
    /// Display name used in terminal output and notifications.
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Claude => "Claude",
            ProviderKind::Codex => "Codex",
        }
    }

    /// Stable lowercase identifier used in config keys and state keys.
    pub fn slug(self) -> &'static str {
        match self {
            ProviderKind::Claude => "claude",
            ProviderKind::Codex => "codex",
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// One quota window as reported by a provider, normalized.
#[derive(Debug, Clone)]
pub struct UsageWindow {
    pub provider: ProviderKind,
    pub name: String,
    pub remaining_percent: f64,
    pub resets_at: DateTime<Utc>,
    /// How long the window itself spans, when the provider says. Short windows
    /// are shown but not evaluated — see `rules::is_last_call`.
    pub window_length: Option<Duration>,
}

/// Round to the nearest whole minute.
pub fn round_to_minute(at: DateTime<Utc>) -> DateTime<Utc> {
    let seconds = at.timestamp();
    let rounded = ((seconds + 30) / 60) * 60;
    DateTime::from_timestamp(rounded, 0).unwrap_or(at)
}

/// Render a duration the way a human reads a countdown: `2d 4h`, `19h`, `2h 11m`, `5m`.
pub fn humanize(d: Duration) -> String {
    let total_minutes = d.num_minutes().max(0);
    if total_minutes == 0 {
        return "now".to_string();
    }
    let days = total_minutes / (24 * 60);
    let hours = (total_minutes % (24 * 60)) / 60;
    let minutes = total_minutes % 60;

    if days > 0 {
        if hours > 0 {
            format!("{days}d {hours}h")
        } else {
            format!("{days}d")
        }
    } else if hours > 0 {
        if minutes > 0 {
            format!("{hours}h {minutes}m")
        } else {
            format!("{hours}h")
        }
    } else {
        format!("{minutes}m")
    }
}

/// Render a configured threshold the way a user would write it: `24h`, `12h`,
/// `90m`, `7d`. Unlike `humanize` this keeps hours until whole days read better.
pub fn format_span(d: Duration) -> String {
    let total_minutes = d.num_minutes().max(0);
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;

    if minutes != 0 {
        if hours == 0 {
            return format!("{minutes}m");
        }
        return format!("{hours}h {minutes}m");
    }
    if hours >= 48 && hours % 24 == 0 {
        return format!("{}d", hours / 24);
    }
    format!("{hours}h")
}

/// Phrase an elapsed duration as a suffix: `just now`, `3h ago`.
pub fn humanize_ago(d: Duration) -> String {
    if d < Duration::minutes(1) {
        return "just now".to_string();
    }
    format!("{} ago", humanize(d))
}

/// Coarser phrasing for notification bodies: `19 hours`, `1 hour`, `40 minutes`.
pub fn humanize_long(d: Duration) -> String {
    let total_minutes = d.num_minutes().max(0);
    let hours = total_minutes / 60;
    if hours == 0 {
        return plural(total_minutes, "minute");
    }
    let days = hours / 24;
    if days >= 2 {
        return plural(days, "day");
    }
    plural(hours, "hour")
}

fn plural(n: i64, unit: &str) -> String {
    if n == 1 {
        format!("{n} {unit}")
    } else {
        format!("{n} {unit}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("valid timestamp")
            .with_timezone(&Utc)
    }

    /// Providers report the same reset instant with sub-second jitter, which
    /// must round to one value.
    #[test]
    fn rounding_absorbs_sub_second_jitter() {
        let early = round_to_minute(at("2026-08-25T22:59:59.847101+00:00"));
        let late = round_to_minute(at("2026-08-25T23:00:00.279509+00:00"));
        assert_eq!(early, late);
        assert_eq!(early, at("2026-08-25T23:00:00Z"));
    }

    #[test]
    fn humanize_covers_each_granularity() {
        assert_eq!(humanize(Duration::minutes(0)), "now");
        assert_eq!(humanize(Duration::minutes(5)), "5m");
        assert_eq!(humanize(Duration::hours(19)), "19h");
        assert_eq!(humanize(Duration::minutes(131)), "2h 11m");
        assert_eq!(humanize(Duration::hours(52)), "2d 4h");
        assert_eq!(humanize(Duration::hours(48)), "2d");
    }

    #[test]
    fn humanize_clamps_negative_durations() {
        assert_eq!(humanize(Duration::hours(-3)), "now");
    }

    #[test]
    fn format_span_reads_like_config_input() {
        assert_eq!(format_span(Duration::hours(24)), "24h");
        assert_eq!(format_span(Duration::hours(12)), "12h");
        assert_eq!(format_span(Duration::minutes(90)), "1h 30m");
        assert_eq!(format_span(Duration::minutes(45)), "45m");
        assert_eq!(format_span(Duration::days(7)), "7d");
    }

    #[test]
    fn humanize_ago_reads_as_a_suffix() {
        assert_eq!(humanize_ago(Duration::seconds(4)), "just now");
        assert_eq!(humanize_ago(Duration::minutes(-2)), "just now");
        assert_eq!(humanize_ago(Duration::hours(3)), "3h ago");
    }

    #[test]
    fn humanize_long_reads_as_prose() {
        assert_eq!(humanize_long(Duration::hours(19)), "19 hours");
        assert_eq!(humanize_long(Duration::hours(1)), "1 hour");
        assert_eq!(humanize_long(Duration::minutes(40)), "40 minutes");
        assert_eq!(humanize_long(Duration::hours(50)), "2 days");
    }
}
