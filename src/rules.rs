//! The Last Call rule: plenty left, but the window is about to reset.

use chrono::{DateTime, Duration, Utc};

use crate::usage::UsageWindow;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rule {
    pub remaining_threshold: f64,
    pub before_reset: Duration,
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            remaining_threshold: 30.0,
            before_reset: Duration::hours(24),
        }
    }
}

/// Shortest window the rule bothers with.
///
/// A 5-hour window that the user simply has not touched sits at 100% remaining
/// and resets within the 24h horizon, so it would match on every single check
/// and notify several times a day. Only windows at least this long carry a
/// meaningful "you are about to lose this" signal. Short windows are still
/// displayed.
pub const SHORTEST_EVALUATED_WINDOW: Duration = Duration::hours(24);

/// Which of the two warnings a matching window is due.
///
/// The window is split in half so the user gets a heads-up and, if they still
/// have not used the quota, a final call as the reset closes in. Halves rather
/// than a fixed final stretch because the poll interval is `before / 2`: any
/// smaller final window could fall between two checks and never fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Warning {
    Early,
    Final,
}

impl Warning {
    pub fn slug(self) -> &'static str {
        match self {
            Warning::Early => "early",
            Warning::Final => "final",
        }
    }
}

pub fn warning(usage: &UsageWindow, rule: &Rule, now: DateTime<Utc>) -> Warning {
    if usage.resets_at - now > rule.before_reset / 2 {
        Warning::Early
    } else {
        Warning::Final
    }
}

pub fn is_last_call(usage: &UsageWindow, rule: &Rule, now: DateTime<Utc>) -> bool {
    is_evaluated(usage)
        && usage.remaining_percent >= rule.remaining_threshold
        && usage.resets_at > now
        && usage.resets_at - now <= rule.before_reset
}

/// A window with an unknown length is evaluated: better a signal we cannot
/// classify than silently dropping a provider's only window.
fn is_evaluated(usage: &UsageWindow) -> bool {
    match usage.window_length {
        Some(length) => length >= SHORTEST_EVALUATED_WINDOW,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::ProviderKind;

    fn window(remaining: f64, resets_in: Duration, now: DateTime<Utc>) -> UsageWindow {
        sized(remaining, resets_in, now, Some(Duration::days(7)))
    }

    fn sized(
        remaining: f64,
        resets_in: Duration,
        now: DateTime<Utc>,
        window_length: Option<Duration>,
    ) -> UsageWindow {
        UsageWindow {
            provider: ProviderKind::Codex,
            name: "weekly".to_string(),
            remaining_percent: remaining,
            resets_at: now + resets_in,
            window_length,
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-25T12:00:00Z")
            .expect("fixed timestamp parses")
            .with_timezone(&Utc)
    }

    #[test]
    fn triggers_when_both_conditions_hold() {
        let n = now();
        assert!(is_last_call(
            &window(34.0, Duration::hours(19), n),
            &Rule::default(),
            n
        ));
    }

    #[test]
    fn does_not_trigger_below_remaining_threshold() {
        let n = now();
        assert!(!is_last_call(
            &window(14.0, Duration::hours(19), n),
            &Rule::default(),
            n
        ));
    }

    #[test]
    fn does_not_trigger_when_reset_is_far_away() {
        let n = now();
        assert!(!is_last_call(
            &window(80.0, Duration::hours(52), n),
            &Rule::default(),
            n
        ));
    }

    #[test]
    fn does_not_trigger_on_an_already_elapsed_window() {
        let n = now();
        assert!(!is_last_call(
            &window(80.0, Duration::hours(-1), n),
            &Rule::default(),
            n
        ));
    }

    /// An untouched 5h window is always full and always resetting soon; it must
    /// not trigger.
    #[test]
    fn short_windows_are_not_evaluated() {
        let n = now();
        let five_hour = sized(100.0, Duration::hours(4), n, Some(Duration::hours(5)));
        assert!(!is_last_call(&five_hour, &Rule::default(), n));
    }

    #[test]
    fn windows_of_unknown_length_are_still_evaluated() {
        let n = now();
        let unknown = sized(34.0, Duration::hours(19), n, None);
        assert!(is_last_call(&unknown, &Rule::default(), n));
    }

    #[test]
    fn a_daily_window_is_long_enough() {
        let n = now();
        let daily = sized(34.0, Duration::hours(19), n, Some(Duration::hours(24)));
        assert!(is_last_call(&daily, &Rule::default(), n));
    }

    #[test]
    fn the_warning_tier_follows_the_half_of_the_window() {
        let n = now();
        let rule = Rule::default();

        // 24h window: the first half is 24h..12h out, the second 12h..0.
        assert_eq!(
            warning(&window(34.0, Duration::hours(20), n), &rule, n),
            Warning::Early
        );
        assert_eq!(
            warning(&window(34.0, Duration::hours(6), n), &rule, n),
            Warning::Final
        );
        // Exactly at half time counts as final.
        assert_eq!(
            warning(&window(34.0, Duration::hours(12), n), &rule, n),
            Warning::Final
        );
    }

    #[test]
    fn the_tier_scales_with_a_custom_window() {
        let n = now();
        let rule = Rule {
            before_reset: Duration::hours(6),
            ..Rule::default()
        };
        assert_eq!(
            warning(&window(34.0, Duration::hours(4), n), &rule, n),
            Warning::Early
        );
        assert_eq!(
            warning(&window(34.0, Duration::hours(2), n), &rule, n),
            Warning::Final
        );
    }

    #[test]
    fn boundaries_are_inclusive() {
        let n = now();
        let rule = Rule::default();
        assert!(is_last_call(
            &window(30.0, Duration::hours(24), n),
            &rule,
            n
        ));
        assert!(!is_last_call(
            &window(29.9, Duration::hours(24), n),
            &rule,
            n
        ));
        assert!(!is_last_call(
            &window(30.0, Duration::minutes(24 * 60 + 1), n),
            &rule,
            n
        ));
    }
}
