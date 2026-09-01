# LimitDeck

[中文说明](README.zh-CN.md)

A compact, privacy-safe terminal dashboard for AI coding subscription limits.

```text
Codex   5h ███████░ 71%  7d █████░░░ 52%
Claude  5h ██████░░ 63%  7d ███████░ 82%
```

LimitDeck reuses official local login surfaces when they exist. It does not copy credentials, read browser cookies, scrape subscription pages, or read Codex `auth.json`.

## Supported data sources

| Plan | Source | Status |
| --- | --- | --- |
| Codex | Official Codex App Server, `account/rateLimits/read` | Built in |
| Claude | Official Claude Code status-line JSON | Built in |
| Codex fallback | OMP redacted usage output | Optional |

Codex App Server is preferred over the OMP fallback when both are available.

## Install

Rust 1.88 or newer is required.

```bash
cargo install --git https://github.com/rockythink/limitdeck
```

Run it from any terminal:

```bash
limitdeck
```

LimitDeck automatically discovers an installed and authenticated Codex CLI. No additional LimitDeck login is required.

## Claude setup

Claude Code exposes subscription rate limits through its official status-line input. Add this to `~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "limitdeck ingest claude",
    "refreshInterval": 60
  }
}
```

This command keeps a quota-only snapshot at `$XDG_CACHE_HOME/limitdeck/claude.json`, or `~/.cache/limitdeck/claude.json` when `XDG_CACHE_HOME` is unset. Claude Code provides `rate_limits` only for eligible subscriptions and only after the first API response in a session.

This setting replaces an existing custom Claude Code status line. If you already use one, call `limitdeck ingest claude` from your existing status-line script and pass the original JSON through stdin.

## Controls

| Key | Action |
| --- | --- |
| `Up` / `Down`, `k` / `j` | Select a plan |
| `Enter` | Open or close plan details |
| `Esc` | Return to the list, then exit |
| `r` | Refresh |
| `q` | Exit |

The layout degrades to compact percentages in narrow terminals and keeps the selected plan visible when the list is taller than the viewport.

## Privacy model

LimitDeck retains only:

- provider, plan, and quota-window identifiers
- display labels and window durations
- remaining percentages and reset times
- snapshot timestamps and availability state

It deliberately ignores account email, account ID, organization, plan tier, billing data, raw credentials, and raw provider responses. Parser inputs and subprocess output are size-bounded. Child processes have explicit timeouts and are reaped on exit.

## Development

```bash
cargo fmt --check
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo build --release
```

The architecture is intentionally small:

```text
official protocol / safe snapshot / optional fallback
                         |
                    PlanAdapter
                         |
           CodingPlan -> UsageWindow
                         |
                  App state -> TUI
```

Adapters own provider-specific parsing and timeouts. The domain and UI do not depend on Codex, Claude, or OMP response formats.

## Disclaimer

LimitDeck is an independent project and is not affiliated with or endorsed by OpenAI, Anthropic, or OMP. Provider interfaces and subscription limits may change.

## License

[MIT](LICENSE)
