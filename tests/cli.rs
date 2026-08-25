//! Acceptance tests: run the real binary against the mock provider data.

use std::path::PathBuf;
use std::process::Command;

/// Each test gets its own config home, so writes stay isolated and the
/// developer's real configuration is never read or touched.
fn lastcall(config_home: &str) -> Command {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(config_home);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("creating the test config home");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_lastcall"));
    cmd.env("LASTCALL_MOCK", "1");
    cmd.env("XDG_CONFIG_HOME", &dir);
    cmd
}

/// Reuse a config home a previous command in the same test wrote to.
fn lastcall_reusing(config_home: &str) -> Command {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(config_home);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_lastcall"));
    cmd.env("LASTCALL_MOCK", "1");
    cmd.env("XDG_CONFIG_HOME", &dir);
    cmd
}

fn stdout_of(cmd: &mut Command) -> String {
    let output = cmd.output().expect("running lastcall");
    assert!(output.status.success(), "exit status: {}", output.status);
    String::from_utf8(output.stdout).expect("utf-8 output")
}

#[test]
fn bare_invocation_reports_every_window() {
    let out = stdout_of(&mut lastcall("show"));
    assert!(out.contains("Claude"), "{out}");
    assert!(out.contains("5h"), "{out}");
    assert!(out.contains("weekly"), "{out}");
    assert!(out.contains("Codex"), "{out}");
    assert!(out.contains("34% remaining"), "{out}");
}

#[test]
fn the_last_call_rule_fires_on_the_codex_window() {
    let out = stdout_of(&mut lastcall("fires"));
    let codex_weekly = out
        .lines()
        .skip_while(|line| !line.starts_with("Codex"))
        .find(|line| line.trim_start().starts_with("weekly"))
        .expect("a Codex weekly line");
    assert!(codex_weekly.contains("LAST CALL"), "{codex_weekly}");
}

/// A 5-hour window sits at full quota whenever it is untouched, so it is
/// displayed but never flagged.
#[test]
fn short_windows_are_shown_but_never_flagged() {
    stdout_of(lastcall("short").args(["config", "--remaining", "1", "--before", "48h"]));
    let out = stdout_of(&mut lastcall_reusing("short"));
    let five_hour = out
        .lines()
        .find(|line| line.trim_start().starts_with("5h"))
        .expect("a 5h line");
    assert!(!five_hour.contains("LAST CALL"), "{five_hour}");
}

#[test]
fn piped_output_is_plain_text() {
    let out = stdout_of(&mut lastcall("plain"));
    assert!(!out.contains('\u{1b}'), "unexpected ANSI escape: {out:?}");
}

#[test]
fn thresholds_are_not_accepted_on_the_bare_command() {
    let output = lastcall("bareflag")
        .args(["--remaining", "40"])
        .output()
        .expect("running lastcall");
    assert!(!output.status.success());
}

#[test]
fn config_shows_the_effective_thresholds_and_path() {
    let out = stdout_of(lastcall("shownocfg").arg("config"));
    assert!(out.contains("Using built-in defaults"), "{out}");
    assert!(out.contains("Claude  >=30% remaining within 24h"), "{out}");
    assert!(out.contains("Codex   >=30% remaining within 24h"), "{out}");
    assert!(out.contains("config.toml"), "{out}");
}

#[test]
fn a_config_write_persists_and_takes_effect() {
    let written = stdout_of(lastcall("write").args(["config", "--remaining", "90"]));
    assert!(written.contains(">=90% remaining within 24h"), "{written}");
    assert!(written.contains("Written to"), "{written}");

    // Read back in a fresh process.
    let shown = stdout_of(lastcall_reusing("write").arg("config"));
    assert!(shown.contains("Using configuration file"), "{shown}");
    assert!(shown.contains(">=90% remaining within 24h"), "{shown}");

    // A 90% threshold takes the badge off the 34% Codex window.
    let out = stdout_of(&mut lastcall_reusing("write"));
    assert!(!out.contains("LAST CALL"), "{out}");
}

/// Every window is named, even when a provider reports only one.
#[test]
fn windows_are_always_named() {
    let out = stdout_of(&mut lastcall("named"));
    assert!(out.lines().any(|line| line == "Codex"), "{out}");
    assert!(
        out.lines()
            .any(|line| line.trim_start().starts_with("weekly")),
        "{out}"
    );
}

#[test]
fn a_disabled_provider_disappears_from_the_output() {
    let hidden = stdout_of(lastcall("disabled").args(["config", "--claude", "--disable"]));
    assert!(
        hidden.contains("Claude  not checked (disabled)"),
        "{hidden}"
    );

    let out = stdout_of(&mut lastcall_reusing("disabled"));
    assert!(!out.contains("Claude"), "{out}");
    assert!(out.contains("Codex"), "{out}");
}

/// `before` values a timer could never service are refused at enable time.
#[test]
fn an_unserviceable_window_is_refused_by_enable() {
    stdout_of(lastcall("tiny").args(["config", "--before", "1m"]));
    let output = lastcall_reusing("tiny")
        .arg("enable")
        .output()
        .expect("running lastcall");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("at least 30m"), "{stderr}");
}

#[test]
fn a_provider_flag_scopes_the_write() {
    let out = stdout_of(lastcall("scoped").args(["config", "--codex", "--before", "12h"]));
    assert!(out.contains("Claude  >=30% remaining within 24h"), "{out}");
    assert!(out.contains("Codex   >=30% remaining within 12h"), "{out}");
}

/// A later partial write must not reset what an earlier one set.
#[test]
fn successive_writes_accumulate() {
    stdout_of(lastcall("accum").args(["config", "--before", "6h"]));
    let out = stdout_of(lastcall_reusing("accum").args(["config", "--remaining", "45"]));
    assert!(out.contains("Claude  >=45% remaining within 6h"), "{out}");
}

#[test]
fn an_invalid_threshold_is_rejected_with_a_usage_error() {
    let output = lastcall("badcfg")
        .args(["config", "--before", "whenever"])
        .output()
        .expect("running lastcall");
    assert!(!output.status.success());
    assert!(!PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("badcfg/lastcall/config.toml")
        .exists());
}
