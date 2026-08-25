//! Command line surface: `lastcall`, plus `enable`, `disable`, `config`.

use chrono::Duration;
use clap::{Parser, Subcommand};

use crate::config::parse_duration;

#[derive(Debug, Parser)]
#[command(
    name = "lastcall",
    version,
    about = "A tiny Linux CLI that reminds you before your Claude Code or Codex quota resets unused.",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Internal: run the background check and send notifications.
    #[arg(long, hide = true)]
    pub notify: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Enable automatic desktop notifications (user-level systemd timer).
    Enable,
    /// Disable automatic desktop notifications.
    Disable,
    /// Show whether automation is running and when it last checked.
    Status,
    /// Show the settings, or change them.
    Config(ConfigArgs),
}

#[derive(Debug, clap::Args)]
pub struct ConfigArgs {
    /// Set the remaining-quota threshold, in whole percent.
    #[arg(long, value_name = "PERCENT", value_parser = percent)]
    pub remaining: Option<u32>,

    /// Set how close to the reset counts as last call, e.g. 12h.
    #[arg(long, value_name = "DURATION", value_parser = duration)]
    pub before: Option<Duration>,

    /// Apply only to Claude (default: both providers).
    #[arg(long)]
    pub claude: bool,

    /// Apply only to Codex (default: both providers).
    #[arg(long)]
    pub codex: bool,

    /// Check this provider again.
    #[arg(long, conflicts_with = "disable")]
    pub enable: bool,

    /// Stop checking this provider entirely.
    #[arg(long)]
    pub disable: bool,
}

impl ConfigArgs {
    /// True when the invocation asks to change something.
    pub fn is_write(&self) -> bool {
        self.remaining.is_some() || self.before.is_some() || self.enable || self.disable
    }

    /// The requested enablement change, if any.
    pub fn enablement(&self) -> Option<bool> {
        match (self.enable, self.disable) {
            (true, _) => Some(true),
            (_, true) => Some(false),
            _ => None,
        }
    }

    /// Which providers the write applies to. Naming neither means both.
    pub fn targets(&self) -> (bool, bool) {
        if self.claude || self.codex {
            (self.claude, self.codex)
        } else {
            (true, true)
        }
    }
}

/// Whole percent only: a fraction of a percent of a quota is noise, and
/// accepting one would mean storing and displaying it too.
fn percent(raw: &str) -> Result<u32, String> {
    let value: u32 = raw
        .parse()
        .map_err(|_| format!("`{raw}` is not a whole number of percent"))?;
    crate::config::validate_percent(value).map_err(|err| err.to_string())
}

fn duration(raw: &str) -> Result<Duration, String> {
    parse_duration(raw).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn bare_invocation_has_no_subcommand_and_no_notify() {
        let cli = Cli::parse_from(["lastcall"]);
        assert!(cli.command.is_none());
        assert!(!cli.notify);
    }

    /// Thresholds are configuration, not per-run flags.
    #[test]
    fn thresholds_are_not_accepted_on_the_bare_command() {
        assert!(Cli::try_parse_from(["lastcall", "--remaining", "40"]).is_err());
        assert!(Cli::try_parse_from(["lastcall", "--before", "12h"]).is_err());
    }

    #[test]
    fn config_without_arguments_only_shows() {
        let Some(Command::Config(args)) = Cli::parse_from(["lastcall", "config"]).command else {
            panic!("expected config");
        };
        assert!(!args.is_write());
        assert_eq!(args.targets(), (true, true));
    }

    #[test]
    fn config_parses_a_write() {
        let Some(Command::Config(args)) =
            Cli::parse_from(["lastcall", "config", "--remaining", "25", "--before", "12h"]).command
        else {
            panic!("expected config");
        };
        assert!(args.is_write());
        assert_eq!(args.remaining, Some(25));
        assert_eq!(args.before, Some(Duration::hours(12)));
        assert_eq!(args.targets(), (true, true));
    }

    #[test]
    fn a_provider_flag_narrows_the_write() {
        let Some(Command::Config(args)) =
            Cli::parse_from(["lastcall", "config", "--codex", "--remaining", "40"]).command
        else {
            panic!("expected config");
        };
        assert_eq!(args.targets(), (false, true));
    }

    #[test]
    fn invalid_thresholds_are_rejected() {
        assert!(Cli::try_parse_from(["lastcall", "config", "--remaining", "140"]).is_err());
        assert!(Cli::try_parse_from(["lastcall", "config", "--before", "soon"]).is_err());
    }

    /// Fractions are rejected rather than silently rounded.
    #[test]
    fn fractional_thresholds_are_rejected() {
        assert!(Cli::try_parse_from(["lastcall", "config", "--remaining", "30.5"]).is_err());
    }

    #[test]
    fn enablement_flags_parse_and_conflict() {
        let Some(Command::Config(args)) =
            Cli::parse_from(["lastcall", "config", "--claude", "--disable"]).command
        else {
            panic!("expected config");
        };
        assert!(args.is_write());
        assert_eq!(args.enablement(), Some(false));
        assert_eq!(args.targets(), (true, false));

        assert!(Cli::try_parse_from(["lastcall", "config", "--enable", "--disable"]).is_err());
    }

    #[test]
    fn subcommands_parse() {
        assert!(matches!(
            Cli::parse_from(["lastcall", "enable"]).command,
            Some(Command::Enable)
        ));
        assert!(matches!(
            Cli::parse_from(["lastcall", "disable"]).command,
            Some(Command::Disable)
        ));
        assert!(matches!(
            Cli::parse_from(["lastcall", "status"]).command,
            Some(Command::Status)
        ));
    }
}
