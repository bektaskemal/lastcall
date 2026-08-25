# lastcall

Lastcall is a small, open-source CLI that shows your Claude Code and Codex
quota windows and can send desktop notifications before a large unused
allowance resets.

```console
$ lastcall
Claude
  5h       63% remaining   resets in 2h 11m
  weekly   18% remaining   resets in 20h
Codex
  weekly   34% remaining   resets in 19h       LAST CALL
```

`LAST CALL` means there is still plenty of quota left in a long-running window,
but the window is close to resetting. Enable background checks once with
`lastcall enable`, and Lastcall sends an early desktop reminder followed by a
final reminder if the quota is still unused. Running `lastcall` itself only
prints the current state.

## Why

Claude Code and Codex expose rolling quota windows, but it is easy to forget
about a weekly allowance until it resets. Lastcall makes that state visible in
the terminal and can send an early reminder followed by a final reminder near
the end of each window.

It is deliberately small: one Rust binary, no daemon, no account setup of its
own, and no telemetry.

## Install

Lastcall is not on crates.io yet. You need Rust 1.75 or newer and Git:

```bash
git clone https://github.com/bektaskemal/lastcall.git
cargo install --path lastcall
```

This installs `lastcall` into Cargo's binary directory, normally
`~/.cargo/bin`. Make sure that directory is on your `PATH`.

The terminal command should work anywhere the project builds, but automatic
notifications currently target Linux. They require a systemd user session and
`notify-send` (usually provided by `libnotify` or `libnotify-bin`).

## Quick start

First, sign in with at least one of the supported CLIs:

- Claude: open Claude Code and sign in.
- Codex: run `codex login`.

Then inspect your quota:

```bash
lastcall
```

To turn on automatic desktop notifications:

```bash
lastcall enable
lastcall status
```

Lastcall detects which providers are signed in when automation is enabled and
skips the others. Nothing runs as root.

The complete command surface is:

```text
lastcall            fetch and print current quota windows
lastcall enable     enable automatic desktop notifications
lastcall disable    disable them and remove Lastcall's user units
lastcall status     show automation and authentication health
lastcall config     show or change notification settings
```

## Notifications

`lastcall enable` installs a user-level systemd timer. A matching window
produces a notification like:

```text
Lastcall: Codex weekly quota

34% remaining.
Resets in 19 hours.
```

Each quota window warns at most twice: once in the first half of the configured
`before` period and once in the second half. Repeated checks within either half
are deduplicated. Authentication warnings are deduplicated too. The terminal
output always shows the current state and never sends a notification by itself.

The polling interval is derived from the tightest configured warning window:
12 hours with the defaults, or 30 minutes for `before = "1h"`. Check its health
without knowing systemd commands:

```console
$ lastcall status
Notifications  enabled
Checking       every 12h
Last check     succeeded 3h ago
Next check     in 9h
Claude         signed in
Codex          signed in
```

Detailed background logs are available through:

```bash
journalctl --user -u lastcall
```

## When does `LAST CALL` appear?

The default rule is:

```text
remaining >= 30%
reset     <= 24h away
```

Short rolling windows such as the five-hour limit are shown for context but do
not trigger notifications.

Change the rule with `lastcall config`:

```bash
lastcall config --remaining 25 --before 12h   # both providers
lastcall config --codex --before 6h           # Codex only
lastcall config --claude --disable            # stop checking Claude
lastcall config --claude --enable             # check Claude again
lastcall config                               # show effective settings
```

Settings are stored in `~/.config/lastcall/config.toml`:

```toml
[claude]
enabled = true
remaining = 25
before = "12h"

[codex]
enabled = true
remaining = 30
before = "24h"
```

Missing files and keys use the built-in defaults. Percentages are whole
numbers. Durations accept `d`, `h`, `m`, and `s`, including combinations such
as `1d 8h`; a bare number means hours. Automatic notification windows must be
at least 30 minutes.

If you edit the TOML while notifications are enabled, run `lastcall enable`
again so the timer picks up any new polling interval. Changes made through
`lastcall config` update the timer automatically.

## Credentials and privacy

Lastcall has no login flow and stores no credentials. It reads the sessions
already created by Claude Code and Codex:

- Claude: `~/.claude/.credentials.json`, or `CLAUDE_CODE_OAUTH_TOKEN`
- Codex: `~/.codex/auth.json`

Tokens are read when making a request and are never copied, cached, or logged.
Lastcall does not refresh them; the official CLIs remain responsible for their
own sessions. If a session expires, the terminal shows an actionable error and
enabled automation sends a deduplicated sign-in warning rather than silently
stopping.

Apart from configuration, Lastcall writes notification bookkeeping—provider,
window name, and reset timestamp—to
`~/.local/state/lastcall/state.json`. There is no telemetry.

One provider failing does not hide the other, and the process exits non-zero
only when every enabled provider fails.

## Caveats

Lastcall is an independent project and is not affiliated with or endorsed by
Anthropic or OpenAI. It uses the usage endpoints and local session formats used
by their respective CLIs; these are not stable public APIs and may change or
stop working without notice. Please open an issue if that happens.

Automatic notifications are Linux-only for now. A macOS implementation would
need a `launchd` timer and a native notification backend.

## Development

```bash
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
LASTCALL_MOCK=1 cargo run
```

Issues and small, focused pull requests are welcome.

## License

MIT
