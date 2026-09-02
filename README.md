<div align="center">

# LimitDeck

**Know what you have left—before the limit hits.**

A compact, privacy-safe terminal dashboard for AI coding subscription limits.

[![Release](https://img.shields.io/github/v/release/rockythink/limitdeck?style=flat-square&label=release&color=8b5cf6)](https://github.com/rockythink/limitdeck/releases/latest)
[![crates.io](https://img.shields.io/crates/v/limitdeck?style=flat-square&color=10b981)](https://crates.io/crates/limitdeck)
[![CI](https://img.shields.io/github/actions/workflow/status/rockythink/limitdeck/ci.yml?branch=main&style=flat-square&label=build)](https://github.com/rockythink/limitdeck/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/rockythink/limitdeck?style=flat-square&color=64748b)](LICENSE)

[English](README.md) · [简体中文](README.zh-CN.md)

</div>

<p align="center">
  <img src="assets/limitdeck.gif" alt="LimitDeck showing Codex and Claude subscription limits in the Rainbow theme" width="800">
</p>

<p align="center">
  <code>brew install rockythink/tap/limitdeck</code>
</p>

---

## One dashboard. Only the limits that matter.

<table>
<tr>
<td width="33%" valign="top">
<strong>Private by design</strong><br><br>
Uses official local interfaces and stores quota-only snapshots. No copied credentials, browser cookies, subscription-page scraping, or Codex <code>auth.json</code> access.
</td>
<td width="33%" valign="top">
<strong>Signal over noise</strong><br><br>
See remaining percentages, reset times, cached state, and local quota history without opening several apps or account pages.
</td>
<td width="33%" valign="top">
<strong>Made for the terminal</strong><br><br>
Fast keyboard control, compact narrow layouts, three runtime themes, and prebuilt binaries for macOS and Linux.
</td>
</tr>
</table>

## Install

### Homebrew — recommended on macOS

```bash
brew install rockythink/tap/limitdeck
```

### Cargo

Requires Rust 1.88 or newer.

```bash
cargo install limitdeck
```

### Prebuilt binaries

Download a macOS or Linux archive and `SHA256SUMS` from the [latest GitHub Release](https://github.com/rockythink/limitdeck/releases/latest).

Then start the dashboard from any terminal:

```bash
limitdeck
```

LimitDeck automatically discovers an installed and authenticated Codex CLI. There is no separate LimitDeck login.

## Data sources

| Plan | Local source | Support |
| --- | --- | :---: |
| Codex | Official Codex App Server, `account/rateLimits/read` | Built in |
| Claude | Official Claude Code status-line JSON | Built in |
| Codex fallback | OMP redacted usage output | Optional |

The Codex App Server is preferred whenever both it and the OMP fallback are available.

```text
official local protocol ─┐
quota-only snapshot ─────┼─> normalized usage windows ─> LimitDeck
optional redacted output ┘
```

## Claude Code setup

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

The command writes a quota-only snapshot to:

- `$XDG_CACHE_HOME/limitdeck/claude.json`, or
- `~/.cache/limitdeck/claude.json` when `XDG_CACHE_HOME` is unset.

Claude Code provides `rate_limits` only for eligible subscriptions and only after the first API response in a session.

> This setting replaces an existing custom Claude Code status line. If you already use one, call `limitdeck ingest claude` from that script and pass the original JSON through stdin.

## The interface

### Themes

Press <kbd>t</kbd> to cycle through:

| Theme | Character |
| --- | --- |
| **Rainbow** | Default. Deep background with green, violet, pink, orange, and blue accents inspired by OMP |
| **Midnight** | Restrained, cool-toned dark palette |
| **Mono** | High-contrast grayscale |

Theme changes apply immediately to the list and detail views for the current session. LimitDeck respects the [`NO_COLOR`](https://no-color.org/) convention; unset it to display theme colors.

### Controls

| Key | Action |
| --- | --- |
| <kbd>↑</kbd> / <kbd>↓</kbd>, <kbd>k</kbd> / <kbd>j</kbd> | Select a plan |
| <kbd>Enter</kbd> | Open or close plan details |
| <kbd>Esc</kbd> | Return to the list, then exit |
| <kbd>r</kbd> | Refresh sources |
| <kbd>t</kbd> | Cycle themes |
| <kbd>q</kbd> | Exit |

The layout collapses to compact percentages in narrow terminals and keeps the selected plan visible when the list exceeds the viewport.

### Local quota history

Open a plan to see locally sampled history when the terminal has enough rows.

- A changing current-cycle history with at least three samples becomes a compact Braille trend line.
- The trend includes its real time span, start and end values, and delta.
- Flat or sparse history remains textual rather than drawing a misleading chart.
- A quota increase starts a new visual cycle.

History is stored at `$XDG_CACHE_HOME/limitdeck/history.json`, or `~/.cache/limitdeck/history.json` when `XDG_CACHE_HOME` is unset. LimitDeck retains at most 30 days and 2,048 samples per quota window. After the first two real samples, unchanged values are sampled no more than once every 15 minutes.

## Privacy, by design

LimitDeck retains the minimum state needed to draw the dashboard.

| Retained locally | Deliberately ignored |
| --- | --- |
| Provider, plan, and quota-window identifiers | Account email and account ID |
| Display labels and window durations | Organization and plan tier |
| Remaining percentages and reset times | Billing data |
| History timestamps and availability state | Raw credentials and provider responses |

Parser inputs and subprocess output are size-bounded. Child processes have explicit timeouts and are reaped on exit.

Diagnostics keep only a fixed source label and a safe failure category. Raw stderr, provider payloads, credentials, account fields, email addresses, and local paths are never retained in application state.

## When a source cannot refresh

A failing source stays visible. If a previous snapshot exists, LimitDeck marks it as cached; otherwise the row shows `不可用 · Enter 查看原因` (unavailable · press Enter for the reason).

| Reason | Next action |
| --- | --- |
| Source command missing | Install the named CLI, confirm it is on `PATH`, then press <kbd>r</kbd> |
| Not authenticated | Log in to the named source, then press <kbd>r</kbd> |
| Timed out | Check the network and retry with <kbd>r</kbd> |
| Provider protocol changed | Upgrade LimitDeck; open an issue if the failure remains |
| Claude snapshot missing | Configure `limitdeck ingest claude` as the Claude Code status line |
| Cached snapshot expired | Refresh the source with <kbd>r</kbd> |

## Architecture

The core stays intentionally small:

```text
official protocol / safe snapshot / optional fallback
                         │
                    PlanAdapter
                         │
           CodingPlan ─> UsageWindow
                         │
             App state + local history ─> TUI
```

Adapters own provider-specific parsing and timeouts. The domain and UI do not depend on Codex, Claude, or OMP response formats.

## Development

```bash
cargo fmt --check
cargo test --locked --all-targets --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo build --release --locked
```

## Project notes

LimitDeck is an independent open-source project and is not affiliated with or endorsed by OpenAI, Anthropic, or OMP. Provider interfaces and subscription limits may change.

Released under the [MIT License](LICENSE).
