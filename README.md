# lastcall

Lastcall shows your Claude Code and Codex quota windows and can send desktop
notifications before a large unused allowance resets.

```console
$ lastcall
Claude
  5h       63% remaining   resets in 2h 11m
  weekly   18% remaining   resets in 20h
Codex
  weekly   34% remaining   resets in 19h       LAST CALL
```

`LAST CALL` means there is still plenty of quota left, but the window is close
to resetting. Running `lastcall` only prints the current state. Notifications
are opt-in and run in the background without a daemon of Lastcall's own.

## Install

Lastcall is not on crates.io yet. You need Rust 1.75 or newer:

```bash
git clone https://github.com/bektaskemal/lastcall.git
cargo install --path lastcall
```

The terminal command should work anywhere the project builds. Automatic
notifications currently require Linux, a systemd user session, and
`notify-send` (`libnotify` or `libnotify-bin`).

## Use

Sign in through Claude Code or run `codex login`, then:

```bash
lastcall            # show current quota windows
lastcall enable     # enable desktop notifications
lastcall status     # check automation and authentication health
lastcall disable    # disable notifications
lastcall config     # show or change settings
```

`lastcall enable` detects which providers are signed in and skips the others.
It installs a user-level systemd timer; nothing runs as root.

A matching window can notify at most twice: once in the first half of the
configured warning period and once in the second. Repeated checks within either
half are deduplicated.

```text
Lastcall: Codex weekly quota

34% remaining.
Resets in 19 hours.
```

Use `lastcall status` to see when the background check last ran. Detailed logs
are available with `journalctl --user -u lastcall`.

## Configure

By default, Lastcall warns when at least 30% remains and the reset is no more
than 24 hours away. Short rolling windows such as the five-hour limit are shown
for context but do not trigger notifications.

```bash
lastcall config --remaining 25 --before 12h   # both providers
lastcall config --codex --before 6h           # Codex only
lastcall config --claude --disable            # stop checking Claude
lastcall config --claude --enable             # check Claude again
```

Settings are stored in `~/.config/lastcall/config.toml`. Percentages are whole
numbers. Durations accept `d`, `h`, `m`, and `s`, including combinations such
as `1d 8h`. Automatic warning periods must be at least 30 minutes.

## Credentials and privacy

Lastcall has no login flow, stores no credentials, and has no telemetry. It
reads the sessions already created by Claude Code and Codex:

- Claude: `~/.claude/.credentials.json`, or `CLAUDE_CODE_OAUTH_TOKEN`
- Codex: `~/.codex/auth.json`

Tokens are read only when making a request and are never copied, cached, or
logged. Lastcall does not refresh them; the official CLIs remain responsible
for their sessions.

Configuration lives under `~/.config/lastcall`. Notification deduplication
state—provider, window, warning tier, and reset timestamp—is stored under
`~/.local/state/lastcall`.

## Caveats

Lastcall is an independent project and is not affiliated with Anthropic or
OpenAI. It relies on the usage endpoints and local session formats used by
their CLIs. These are not stable public APIs and may change without notice.

Automatic notifications are Linux-only for now. macOS support would require a
`launchd` timer and a native notification backend.

## Development

```bash
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
LASTCALL_MOCK=1 cargo run
```

## License

MIT
