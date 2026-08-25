//! lastcall — warn before a Claude Code or Codex quota resets unused.

mod cli;
mod config;
mod notify;
mod providers;
mod render;
mod rules;
mod state;
mod systemd;
mod usage;

use std::io::{IsTerminal, Write};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{bail, Result};
use chrono::Utc;
use clap::Parser;

use crate::cli::{Cli, Command};
use crate::config::Config;
use crate::render::{any_provider_succeeded, ProviderReport, Style};
use crate::rules::is_last_call;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => return fail(&anyhow::Error::from(err)),
    };

    match runtime.block_on(run(cli)) {
        Ok(code) => code,
        Err(err) => fail(&err),
    }
}

fn fail(err: &anyhow::Error) -> ExitCode {
    eprintln!("lastcall: {err:#}");
    ExitCode::FAILURE
}

async fn run(cli: Cli) -> Result<ExitCode> {
    let config = Config::load()?;

    match cli.command {
        Some(Command::Enable) => enable_command(config).await,
        Some(Command::Disable) => {
            systemd::disable()?;
            println!("Automatic Lastcall notifications disabled.");
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Status) => status_command(&config),
        Some(Command::Config(args)) => config_command(config, args),
        None if cli.notify => notify_run(&config).await,
        None => show(&config).await,
    }
}

/// Fetch every enabled provider concurrently; one failure never blocks another.
async fn collect(config: &Config) -> Vec<ProviderReport> {
    let handles: Vec<_> = providers::enabled(config)
        .into_iter()
        .map(|provider| {
            let provider: Arc<dyn providers::UsageProvider + Send + Sync> = Arc::from(provider);
            tokio::spawn(async move {
                ProviderReport {
                    provider: provider.kind(),
                    result: provider.usage().await,
                }
            })
        })
        .collect();

    let mut reports = Vec::with_capacity(handles.len());
    for handle in handles {
        match handle.await {
            Ok(report) => reports.push(report),
            // A panicking provider must not take the other one down.
            Err(err) => eprintln!("lastcall: provider task failed: {err}"),
        }
    }
    reports
}

async fn show(config: &Config) -> Result<ExitCode> {
    let reports = collect(config).await;
    let style = Style::detect(std::io::stdout().is_terminal());
    let output = render::render(&reports, config, Utc::now(), style);

    let mut stdout = std::io::stdout().lock();
    stdout.write_all(output.as_bytes())?;
    stdout.flush()?;

    Ok(if any_provider_succeeded(&reports) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Background mode: same fetch, same rule, notifications deduplicated per
/// quota window. Diagnostics go to the journal via stdout/stderr.
async fn notify_run(config: &Config) -> Result<ExitCode> {
    let reports = collect(config).await;
    let now = Utc::now();
    let path = state::state_path();
    let mut state = state::State::load(&path)?;
    state.prune(now);

    let mut sent = 0usize;
    for report in &reports {
        match &report.result {
            Err(err) => {
                eprintln!("{}: {}", report.provider, err.summary());
                // A transient failure heals itself; an expired session does
                // not, so say so out loud — at most once a day.
                if let Some(hint) = err.hint() {
                    let key = state::auth_key(report.provider, now);
                    if !state.was_seen(&key) {
                        match notify::send_auth_warning(report.provider, hint) {
                            Ok(()) => {
                                state.mark_seen(key);
                                sent += 1;
                                println!("warned: {} is not signed in", report.provider);
                            }
                            Err(err) => eprintln!("notification failed: {err:#}"),
                        }
                    }
                }
            }
            Ok(windows) => {
                let rule = config.rule_for(report.provider);
                for window in windows {
                    if !is_last_call(window, &rule, now) {
                        continue;
                    }
                    // One warning per half of the window: a heads-up, then a
                    // final call if the quota is still sitting unused.
                    let warning = rules::warning(window, &rule, now);
                    if state.was_notified(window, warning) {
                        continue;
                    }
                    match notify::send(window, now) {
                        Ok(()) => {
                            state.mark_notified(window, warning);
                            sent += 1;
                            println!(
                                "notified: {} {} {} ({:.0}% remaining)",
                                report.provider,
                                window.name,
                                warning.slug(),
                                window.remaining_percent
                            );
                        }
                        Err(err) => eprintln!("notification failed: {err:#}"),
                    }
                }
            }
        }
    }

    state.save(&path)?;
    let succeeded = any_provider_succeeded(&reports);
    if sent == 0 && succeeded {
        println!("no windows matched the last call rule");
    }
    Ok(if succeeded {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Turning automation on is also the moment to work out which providers are
/// worth checking: nagging daily about a provider the user never signed into is
/// the fastest way to get notifications muted.
async fn enable_command(mut config: Config) -> Result<ExitCode> {
    let candidates = providers::enabled(&config);
    let unauthenticated: Vec<usage::ProviderKind> = candidates
        .iter()
        .filter(|provider| provider.auth_check().is_err())
        .map(|provider| provider.kind())
        .collect();

    // Refuse before touching the config: disabling everything and then failing
    // would leave the user worse off than when they started.
    if !candidates.is_empty() && unauthenticated.len() == candidates.len() {
        bail!(
            "not signed in to any provider, so there would be nothing to check; \
             sign in to Claude Code or run `codex login` first"
        );
    }

    for provider in &unauthenticated {
        config.set(
            (
                *provider == usage::ProviderKind::Claude,
                *provider == usage::ProviderKind::Codex,
            ),
            None,
            None,
            Some(false),
        );
    }
    if !unauthenticated.is_empty() {
        config.save()?;
    }

    let interval = systemd::enable(&config)?;
    println!("Automatic Lastcall notifications enabled.");
    println!("Checking every {}.", usage::format_span(interval));
    for provider in &unauthenticated {
        println!(
            "Skipping {provider}: not signed in. Run `lastcall config --{} --enable` to include it.",
            provider.slug()
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// `lastcall status`: is automation running, when did it last run, and can each
/// provider still be reached — without anyone needing to know systemctl.
fn status_command(config: &Config) -> Result<ExitCode> {
    let status = systemd::status();
    let now = Utc::now();

    println!(
        "{:<14} {}",
        "Notifications",
        if status.active { "enabled" } else { "disabled" }
    );

    if status.active {
        let interval = systemd::poll_interval(config);
        println!("{:<14} every {}", "Checking", usage::format_span(interval));
        match (status.last_run, status.last_ok) {
            (Some(last), Some(ok)) => println!(
                "{:<14} {} {}",
                "Last check",
                if ok { "succeeded" } else { "failed" },
                usage::humanize_ago(now - last)
            ),
            _ => println!("{:<14} not yet", "Last check"),
        }
        match status.next_run(interval) {
            Some(next) if next > now => {
                println!("{:<14} in {}", "Next check", usage::humanize(next - now))
            }
            Some(_) => println!("{:<14} due now", "Next check"),
            None => println!("{:<14} unknown", "Next check"),
        }
    }

    for provider in providers::all() {
        let kind = provider.kind();
        let state = if !config.is_enabled(kind) {
            "disabled".to_string()
        } else {
            match provider.auth_check() {
                Ok(()) => "signed in".to_string(),
                Err(err) => err.summary(),
            }
        };
        println!("{:<14} {state}", kind.label());
    }
    Ok(ExitCode::SUCCESS)
}

/// `lastcall config` shows the thresholds; with `--remaining`/`--before` it
/// changes them.
fn config_command(mut config: Config, args: cli::ConfigArgs) -> Result<ExitCode> {
    if !args.is_write() {
        print_config(&config);
        return Ok(ExitCode::SUCCESS);
    }

    config.set(
        args.targets(),
        args.remaining,
        args.before,
        args.enablement(),
    );
    let path = config.save()?;
    print_config(&config);
    println!("\nWritten to {}.", path.display());

    // The polling interval is derived from `before`, so a change has to reach
    // the timer as well. Doing it here is what keeps the two from drifting.
    if systemd::is_active() {
        let interval = systemd::install(&config)?;
        println!(
            "Automatic notifications re-armed: checking every {}.",
            usage::format_span(interval)
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn print_config(config: &Config) {
    let path = config::config_path();
    let exists = path.as_ref().is_some_and(|path| path.exists());

    if exists {
        println!("Using configuration file:\n");
    } else {
        println!("Using built-in defaults:\n");
    }
    for provider in [usage::ProviderKind::Claude, usage::ProviderKind::Codex] {
        let rule = config.rule_for(provider);
        if config.is_enabled(provider) {
            println!(
                "{:<7} >={:.0}% remaining within {}",
                provider.label(),
                rule.remaining_threshold,
                usage::format_span(rule.before_reset)
            );
        } else {
            println!("{:<7} not checked (disabled)", provider.label());
        }
    }
    if let Some(path) = path {
        println!("\nConfig file:\n{}", path.display());
    }
}
