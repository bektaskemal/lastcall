//! Optional `~/.config/lastcall/config.toml`, plus built-in defaults.
//!
//! Thresholds are configuration, not per-run flags: `lastcall config` reads and
//! writes this file, and every other command just reads it.

use std::fs;
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use chrono::Duration;
use serde::Deserialize;

use crate::rules::Rule;
use crate::usage::ProviderKind;

/// Per-provider settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProviderConfig {
    pub rule: Rule,
    /// A disabled provider is not queried, not displayed, and never warned
    /// about. This is how a Codex-only user stops hearing about Claude.
    pub enabled: bool,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            rule: Rule::default(),
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Config {
    pub claude: ProviderConfig,
    pub codex: ProviderConfig,
}

impl Config {
    pub fn rule_for(&self, provider: ProviderKind) -> Rule {
        self.provider(provider).rule
    }

    pub fn is_enabled(&self, provider: ProviderKind) -> bool {
        self.provider(provider).enabled
    }

    pub fn provider(&self, provider: ProviderKind) -> ProviderConfig {
        match provider {
            ProviderKind::Claude => self.claude,
            ProviderKind::Codex => self.codex,
        }
    }

    /// Load the config file if present; fall back to built-in defaults otherwise.
    pub fn load() -> Result<Self> {
        let Some(path) = config_path() else {
            return Ok(Self::default());
        };
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(err).with_context(|| format!("reading {}", path.display())),
        };
        Self::from_toml(&raw).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn from_toml(raw: &str) -> Result<Self> {
        let file: ConfigFile = toml::from_str(raw)?;
        Ok(Self {
            claude: file.claude.into_provider()?,
            codex: file.codex.into_provider()?,
        })
    }

    /// Change the settings of the selected providers.
    pub fn set(
        &mut self,
        (claude, codex): (bool, bool),
        remaining: Option<u32>,
        before: Option<Duration>,
        enabled: Option<bool>,
    ) {
        let mut targets = Vec::new();
        if claude {
            targets.push(&mut self.claude);
        }
        if codex {
            targets.push(&mut self.codex);
        }
        for target in targets {
            if let Some(remaining) = remaining {
                target.rule.remaining_threshold = f64::from(remaining);
            }
            if let Some(before) = before {
                target.rule.before_reset = before;
            }
            if let Some(enabled) = enabled {
                target.enabled = enabled;
            }
        }
    }

    /// Serialize the full configuration, so a partial change never drops the
    /// settings it did not touch.
    pub fn to_toml(self) -> String {
        format!(
            "# Written by `lastcall config`.\n\n{}\n{}",
            section("claude", self.claude),
            section("codex", self.codex)
        )
    }

    /// Write the configuration, creating the directory if needed.
    pub fn save(&self) -> Result<PathBuf> {
        let path = config_path().ok_or_else(|| anyhow!("no XDG config directory"))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&path, self.to_toml()).with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    }
}

fn section(name: &str, provider: ProviderConfig) -> String {
    format!(
        "[{name}]\nenabled = {}\nremaining = {:.0}\nbefore = \"{}\"\n",
        provider.enabled,
        provider.rule.remaining_threshold,
        crate::usage::format_span(provider.rule.before_reset)
    )
}

pub fn config_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("lastcall").join("config.toml"))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    #[serde(default)]
    claude: ProviderSection,
    #[serde(default)]
    codex: ProviderSection,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderSection {
    /// Whole percent. Fractions of a percent of a quota are noise, and
    /// accepting them would mean displaying and storing them too.
    remaining: Option<u32>,
    before: Option<String>,
    enabled: Option<bool>,
}

impl ProviderSection {
    fn into_provider(self) -> Result<ProviderConfig> {
        let mut provider = ProviderConfig::default();
        if let Some(remaining) = self.remaining {
            provider.rule.remaining_threshold = f64::from(validate_percent(remaining)?);
        }
        if let Some(before) = self.before {
            provider.rule.before_reset = parse_duration(&before)?;
        }
        if let Some(enabled) = self.enabled {
            provider.enabled = enabled;
        }
        Ok(provider)
    }
}

pub fn validate_percent(value: u32) -> Result<u32> {
    if value > 100 {
        bail!("`remaining` must be between 0 and 100, got {value}");
    }
    Ok(value)
}

/// Parse a human duration such as `24h`, `90m`, `1d`, or `1d 8h`.
///
/// Every arithmetic step is checked: a caller-supplied number large enough to
/// overflow a `Duration` is a validation error, not a panic.
pub fn parse_duration(input: &str) -> Result<Duration> {
    let text = input.trim();
    if text.is_empty() {
        bail!("empty duration");
    }

    let mut total = Duration::zero();
    let mut digits = String::new();
    let mut saw_unit = false;

    for ch in text.chars() {
        match ch {
            '0'..='9' => digits.push(ch),
            ' ' | '\t' | '_' => {}
            'd' | 'h' | 'm' | 's' => {
                if digits.is_empty() {
                    bail!("`{input}` has a unit without a number");
                }
                total = add_part(total, &digits, ch, input)?;
                digits.clear();
                saw_unit = true;
            }
            other => bail!("unexpected character `{other}` in duration `{input}`"),
        }
    }

    if !digits.is_empty() {
        // A bare number is read as hours, so `--before 12` behaves as expected.
        total = add_part(total, &digits, 'h', input)?;
        saw_unit = true;
    }
    if !saw_unit || total <= Duration::zero() {
        bail!("`{input}` is not a positive duration");
    }
    Ok(total)
}

fn add_part(total: Duration, digits: &str, unit: char, input: &str) -> Result<Duration> {
    let value: i64 = digits
        .parse()
        .map_err(|_| anyhow!("`{digits}` in `{input}` is too large"))?;
    let part = match unit {
        'd' => Duration::try_days(value),
        'h' => Duration::try_hours(value),
        'm' => Duration::try_minutes(value),
        _ => Duration::try_seconds(value),
    }
    .ok_or_else(|| anyhow!("`{input}` is longer than any usable duration"))?;

    total
        .checked_add(&part)
        .ok_or_else(|| anyhow!("`{input}` is longer than any usable duration"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_and_compound_units() {
        assert_eq!(parse_duration("24h").unwrap(), Duration::hours(24));
        assert_eq!(parse_duration("90m").unwrap(), Duration::minutes(90));
        assert_eq!(parse_duration("1d").unwrap(), Duration::days(1));
        assert_eq!(
            parse_duration("1d 8h").unwrap(),
            Duration::days(1) + Duration::hours(8)
        );
        assert_eq!(parse_duration("12").unwrap(), Duration::hours(12));
    }

    #[test]
    fn rejects_nonsense_durations() {
        for bad in ["", "h", "0h", "-4h", "12x", "abc"] {
            assert!(parse_duration(bad).is_err(), "`{bad}` should be rejected");
        }
    }

    /// A number large enough to overflow must be a validation error, never a
    /// panic.
    #[test]
    fn rejects_durations_that_would_overflow() {
        for bad in [
            "9223372036854775807d",
            "99999999999999999999d",
            "9223372036854775807h",
            "106751991167300d 24h",
        ] {
            assert!(parse_duration(bad).is_err(), "`{bad}` should be rejected");
        }
    }

    #[test]
    fn missing_sections_keep_defaults() {
        let config = Config::from_toml("").unwrap();
        assert_eq!(config, Config::default());
        assert!(config.is_enabled(ProviderKind::Claude));
        assert!(config.is_enabled(ProviderKind::Codex));
    }

    #[test]
    fn per_provider_overrides_are_independent() {
        let config = Config::from_toml(
            r#"
            [claude]
            remaining = 25
            before = "12h"

            [codex]
            remaining = 40
            enabled = false
            "#,
        )
        .unwrap();

        assert_eq!(config.claude.rule.remaining_threshold, 25.0);
        assert_eq!(config.claude.rule.before_reset, Duration::hours(12));
        assert!(config.claude.enabled);
        assert_eq!(config.codex.rule.remaining_threshold, 40.0);
        assert_eq!(config.codex.rule.before_reset, Duration::hours(24));
        assert!(!config.codex.enabled);
    }

    #[test]
    fn rejects_out_of_range_and_unknown_keys() {
        assert!(Config::from_toml("[claude]\nremaining = 140").is_err());
        assert!(Config::from_toml("[claude]\nthreshold = 30").is_err());
    }

    /// Fractions of a percent are rejected rather than silently rounded.
    #[test]
    fn rejects_fractional_thresholds() {
        assert!(Config::from_toml("[claude]\nremaining = 30.5").is_err());
    }

    #[test]
    fn set_applies_to_the_named_providers_only() {
        let mut config = Config::default();
        config.set((false, true), Some(40), Some(Duration::hours(6)), None);
        assert_eq!(config.claude, ProviderConfig::default());
        assert_eq!(config.codex.rule.remaining_threshold, 40.0);
        assert_eq!(config.codex.rule.before_reset, Duration::hours(6));
    }

    #[test]
    fn set_leaves_untouched_fields_alone() {
        let mut config = Config::from_toml("[claude]\nbefore = \"6h\"").unwrap();
        config.set((true, true), Some(40), None, None);
        assert_eq!(config.claude.rule.remaining_threshold, 40.0);
        assert_eq!(config.claude.rule.before_reset, Duration::hours(6));
    }

    #[test]
    fn set_can_disable_a_provider() {
        let mut config = Config::default();
        config.set((true, false), None, None, Some(false));
        assert!(!config.is_enabled(ProviderKind::Claude));
        assert!(config.is_enabled(ProviderKind::Codex));
    }

    /// A partial write must round-trip everything, not just what changed.
    #[test]
    fn written_config_round_trips() {
        let mut config = Config::default();
        config.set((true, false), Some(25), Some(Duration::minutes(90)), None);
        config.set((false, true), None, None, Some(false));

        let reloaded = Config::from_toml(&config.to_toml()).unwrap();
        assert_eq!(reloaded, config);
        assert_eq!(reloaded.claude.rule.before_reset, Duration::minutes(90));
        assert!(!reloaded.codex.enabled);
    }
}
