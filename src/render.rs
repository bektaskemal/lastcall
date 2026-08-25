//! Terminal rendering. Only `LAST CALL` is styled; everything else is plain
//! text that stays readable when piped.

use chrono::{DateTime, Utc};

use crate::config::Config;
use crate::providers::ProviderError;
use crate::rules::is_last_call;
use crate::usage::{humanize, ProviderKind, UsageWindow};

const LABEL_WIDTH: usize = 7;
const RESET_WIDTH: usize = 16;
const BADGE: &str = "LAST CALL";

/// One provider's outcome for this invocation.
pub struct ProviderReport {
    pub provider: ProviderKind,
    pub result: Result<Vec<UsageWindow>, ProviderError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub color: bool,
}

impl Style {
    /// Colors are used only for an interactive terminal, and never when
    /// `NO_COLOR` is set.
    pub fn detect(is_tty: bool) -> Self {
        Self {
            color: is_tty && std::env::var_os("NO_COLOR").is_none(),
        }
    }

    fn badge(self) -> String {
        if self.color {
            format!("\u{1b}[1;33m{BADGE}\u{1b}[0m")
        } else {
            BADGE.to_string()
        }
    }
}

pub fn render(
    reports: &[ProviderReport],
    config: &Config,
    now: DateTime<Utc>,
    style: Style,
) -> String {
    let mut out = String::new();
    for report in reports {
        match &report.result {
            Err(err) => {
                push_line(
                    &mut out,
                    &format!(
                        "{:<LABEL_WIDTH$} {}",
                        report.provider.label(),
                        err.summary()
                    ),
                );
                if let Some(hint) = err.hint() {
                    push_line(&mut out, &format!("  {hint}"));
                }
            }
            Ok(windows) if windows.is_empty() => {
                push_line(
                    &mut out,
                    &format!(
                        "{:<LABEL_WIDTH$} no quota windows reported",
                        report.provider.label()
                    ),
                );
            }
            // Always two-level, even for a single window: collapsing it onto
            // the provider line would hide *which* limit is being reported.
            Ok(windows) => {
                push_line(&mut out, report.provider.label());
                let rule = config.rule_for(report.provider);
                for window in windows {
                    let triggered = is_last_call(window, &rule, now);
                    push_line(
                        &mut out,
                        &format!("  {}", row(&window.name, window, now, triggered, style)),
                    );
                }
            }
        }
    }
    out
}

fn row(
    label: &str,
    window: &UsageWindow,
    now: DateTime<Utc>,
    triggered: bool,
    style: Style,
) -> String {
    let countdown = format!("resets in {}", humanize(window.resets_at - now));
    let line = format!(
        "{:<LABEL_WIDTH$} {:>3.0}% remaining   {}",
        label, window.remaining_percent, countdown
    );
    if triggered {
        let padding = RESET_WIDTH.saturating_sub(countdown.chars().count());
        format!("{line}{:padding$}    {}", "", style.badge())
    } else {
        line
    }
}

fn push_line(out: &mut String, line: &str) {
    out.push_str(line.trim_end());
    out.push('\n');
}

/// True when at least one provider returned usable data.
pub fn any_provider_succeeded(reports: &[ProviderReport]) -> bool {
    reports.iter().any(|report| report.result.is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ProviderError;
    use chrono::Duration;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-25T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn window(
        provider: ProviderKind,
        name: &str,
        remaining: f64,
        resets_in: Duration,
    ) -> UsageWindow {
        sized(provider, name, remaining, resets_in, Duration::days(7))
    }

    fn sized(
        provider: ProviderKind,
        name: &str,
        remaining: f64,
        resets_in: Duration,
        window_length: Duration,
    ) -> UsageWindow {
        UsageWindow {
            provider,
            name: name.to_string(),
            remaining_percent: remaining,
            resets_at: now() + resets_in,
            window_length: Some(window_length),
        }
    }

    fn plain() -> Style {
        Style { color: false }
    }

    #[test]
    fn every_window_is_named_even_when_a_provider_reports_one() {
        let reports = vec![
            ProviderReport {
                provider: ProviderKind::Claude,
                result: Ok(vec![window(
                    ProviderKind::Claude,
                    "weekly",
                    42.0,
                    Duration::hours(52),
                )]),
            },
            ProviderReport {
                provider: ProviderKind::Codex,
                result: Ok(vec![window(
                    ProviderKind::Codex,
                    "weekly",
                    34.0,
                    Duration::hours(19),
                )]),
            },
        ];

        let out = render(&reports, &Config::default(), now(), plain());
        assert_eq!(
            out,
            "Claude\n\
             \u{20}\u{20}weekly   42% remaining   resets in 2d 4h\n\
             Codex\n\
             \u{20}\u{20}weekly   34% remaining   resets in 19h       LAST CALL\n"
        );
    }

    #[test]
    fn multi_window_providers_render_as_a_block() {
        let reports = vec![ProviderReport {
            provider: ProviderKind::Claude,
            result: Ok(vec![
                sized(
                    ProviderKind::Claude,
                    "5h",
                    63.0,
                    Duration::minutes(131),
                    Duration::hours(5),
                ),
                window(ProviderKind::Claude, "weekly", 42.0, Duration::hours(20)),
            ]),
        }];

        let out = render(&reports, &Config::default(), now(), plain());
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "Claude");
        // The 5h window is shown but never evaluated.
        assert_eq!(lines[1], "  5h       63% remaining   resets in 2h 11m");
        assert_eq!(
            lines[2],
            "  weekly   42% remaining   resets in 20h       LAST CALL"
        );
    }

    #[test]
    fn plain_output_contains_no_ansi_sequences() {
        let reports = vec![ProviderReport {
            provider: ProviderKind::Codex,
            result: Ok(vec![window(
                ProviderKind::Codex,
                "weekly",
                34.0,
                Duration::hours(19),
            )]),
        }];
        let out = render(&reports, &Config::default(), now(), plain());
        assert!(!out.contains('\u{1b}'));
        assert!(out.contains("LAST CALL"));
    }

    #[test]
    fn the_badge_is_styled_when_color_is_on() {
        let reports = vec![ProviderReport {
            provider: ProviderKind::Codex,
            result: Ok(vec![window(
                ProviderKind::Codex,
                "weekly",
                34.0,
                Duration::hours(19),
            )]),
        }];
        let out = render(&reports, &Config::default(), now(), Style { color: true });
        assert!(out.contains("\u{1b}[1;33mLAST CALL\u{1b}[0m"));
    }

    #[test]
    fn one_provider_failing_does_not_hide_the_other() {
        let reports = vec![
            ProviderReport {
                provider: ProviderKind::Claude,
                result: Err(ProviderError::NotAuthenticated {
                    hint: "Run Claude Code and sign in first.",
                }),
            },
            ProviderReport {
                provider: ProviderKind::Codex,
                result: Ok(vec![window(
                    ProviderKind::Codex,
                    "weekly",
                    34.0,
                    Duration::hours(19),
                )]),
            },
        ];

        let out = render(&reports, &Config::default(), now(), plain());
        assert!(out.contains("Claude  not authenticated"));
        assert!(out.contains("  Run Claude Code and sign in first."));
        assert!(out.contains("LAST CALL"));
        assert!(any_provider_succeeded(&reports));
    }

    #[test]
    fn all_providers_failing_is_reported_as_failure() {
        let reports = vec![ProviderReport {
            provider: ProviderKind::Codex,
            result: Err(ProviderError::Failed(anyhow::anyhow!("network down"))),
        }];
        let out = render(&reports, &Config::default(), now(), plain());
        assert!(out.contains("network down"));
        assert!(!any_provider_succeeded(&reports));
    }
}
