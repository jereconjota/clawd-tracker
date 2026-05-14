# Clawd Tracker

A menu-bar / tray app for **macOS** and **Ubuntu** that shows live
**5-hour session** and **7-day weekly** utilization for one or more
Claude Pro/Max accounts.

Inspired by [Clawdmeter](https://github.com/HermannBjorgvin/Clawdmeter):
probe `api.anthropic.com/v1/messages` once per minute per account with a single
Haiku token (~$0.0000008) and read the `anthropic-ratelimit-unified-*`
response headers.

> **Disclaimer** — This is an **unofficial** third-party tool. It is not
> built, endorsed, or supported by Anthropic. It reuses the same OAuth
> `client_id` and credentials store that the official Claude Code CLI
> creates on your machine when you run `claude /login`, so a valid
> Claude Pro/Max subscription on the active account is required. Use at
> your own risk; the project is released under the MIT license (see
> `LICENSE`), with no warranty of fitness for any purpose.

**Menu bar** shows one entry per enabled account, colored by load
(🟢 < 60 %, 🟡 60–85 %, 🔴 ≥ 85 %). The number is the current **5h** percent:

```
🟡 58% · 🟢 0%
```

Click the icon for the popover:

```
┌──────────────────────────────────────┐
│  jereconjota                         │
│  5H ████████░░░░░░░░░░  58%          │
│  7D ███████████░░░░░░░  66%          │
│  resets 5h in 2h 34m · 7d in 15h 34m │
├──────────────────────────────────────┤
│  Ravel                               │
│  5H ░░░░░░░░░░░░░░░░░░   0%          │
│  7D ██░░░░░░░░░░░░░░░░  11%          │
│  resets 5h in 3h 44m · 7d in 5d  5h  │
└──────────────────────────────────────┘
```

The 5H / 7D numbers and bars share the same color scale as the menu-bar dot.

---

## Install

### macOS

1. Download `Clawd.Tracker_<version>_aarch64.dmg` (Apple Silicon) or
   `..._x64.dmg` (Intel) from the [Releases page](../../releases/latest).
2. Open the `.dmg`, drag **Clawd Tracker** to `/Applications`.
3. First launch may be blocked by Gatekeeper (unsigned). Right-click the app
   → **Open** → confirm. After that, launch normally.
4. The icon appears in the menu bar with a `%` indicator.

### Ubuntu / Debian

```bash
# 1. install runtime deps (only needed once)
sudo apt install libwebkit2gtk-4.1-0 libayatana-appindicator3-1

# 2. download and install the .deb
curl -LO https://github.com/jereconjota/clawd-tracker/releases/latest/download/clawd-tracker_<version>_amd64.deb
sudo dpkg -i clawd-tracker_<version>_amd64.deb

# 3. launch
clawd-tracker &
```

The icon appears in the system tray (GNOME requires the *AppIndicator*
extension; KDE works out of the box).

---

## First-time setup

Each Claude Pro/Max account needs its own config directory (so credentials
don't clobber each other):

```bash
# Add as many as you want — name them however you like
CLAUDE_CONFIG_DIR=~/.claude-personal claude /login
CLAUDE_CONFIG_DIR=~/.claude-work     claude /login
```

Then open **Clawd Tracker** → tray icon → **Settings**:

- Click **Auto-detect** to pull every `~/.claude*` directory that already has
  a `.credentials.json`.
- Or click **+ Add profile** and point the `config_dir` to a fresh path.

### How credentials work (no app login needed)

Clawd Tracker reads the credentials Claude Code already writes when you run
`claude /login`. There is no separate login flow inside the app.

- **macOS** — Claude Code stores credentials in your login Keychain, with
  one entry per `CLAUDE_CONFIG_DIR` (service name
  `Claude Code-credentials-<sha256(abs_path)[:8]>`). The app reads each
  profile's slot directly. Multi-account works out of the box.
- **Ubuntu** — Claude Code writes `<config_dir>/.credentials.json`.
  Auto-detect picks them up.

Token refresh is automatic. On macOS the new token is written back to
Keychain so the CLI and the app stay in sync.

If a profile says "login required", run the exact command shown in
Settings → that profile's hint, then click **Refresh** in the popover.

---

## Daily use

- **Left-click tray icon** → popover with all enabled accounts (5h + 7d
  bars, both colored by load, plus reset countdowns).
- **Right-click tray icon** → Refresh / Settings / Quit.
- The widget (toggleable from the popover header) is a draggable, always-on-top
  pill if you want a permanent corner indicator.

### What's in the menu bar

Each enabled profile renders as `<dot> <5h-percent>` joined by ` · `:

| Symbol | Load (`max(5h, 7d)`) |
|--------|-----------------------|
| 🟢      | < 60 %               |
| 🟡      | 60 %–85 %            |
| 🔴      | ≥ 85 %               |

In Settings each profile has a **show in menu bar** checkbox so you can
hide noisy accounts while still tracking them in the popover.

---

## Build from source

### Prerequisites

| OS     | Install                                                                                                |
|--------|--------------------------------------------------------------------------------------------------------|
| macOS  | Xcode CLT (`xcode-select --install`), [pnpm](https://pnpm.io), Rust (`brew install rustup-init && rustup-init`) |
| Ubuntu | `sudo apt install build-essential libwebkit2gtk-4.1-dev libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev curl pkg-config`, [pnpm](https://pnpm.io), Rust (rustup) |

### Run in dev mode

```bash
pnpm install
pnpm tauri dev
```

### Produce release artifacts

```bash
pnpm tauri build
```

Artifacts land in `src-tauri/target/release/bundle/`:

- macOS: `dmg/*.dmg`, `macos/*.app`
- Ubuntu: `deb/*.deb`, `appimage/*.AppImage`

### Cut a GitHub release (binaries only, no commit bloat)

`pnpm tauri build` only produces artifacts for the OS it runs on, so a
multi-platform release is a two-step ritual:

```bash
# Step 1 — on macOS, create the release with the .dmg
VERSION=0.1.1
pnpm tauri build
gh release create v$VERSION \
  --title "v$VERSION" \
  --notes-file release-notes.md \
  "src-tauri/target/release/bundle/dmg/Clawd Tracker_${VERSION}_aarch64.dmg"

# Step 2 — on Ubuntu, build the .deb and upload to the same release
git pull
pnpm install
pnpm tauri build
gh release upload v$VERSION \
  src-tauri/target/release/bundle/deb/clawd-tracker_${VERSION}_amd64.deb
```

The same release tag now hosts both binaries; `..../releases/latest/download/...`
URLs in the README install commands resolve to whichever OS asset matches.

Binaries live in **GitHub Releases**, not in the git history.

---

## Verify the probe manually

Sanity-check that your account actually returns the unified headers (only
Pro/Max accounts do):

```bash
# Ubuntu / Linux (file-based credentials):
TOKEN=$(jq -r .claudeAiOauth.accessToken ~/.claude-personal/.credentials.json)

# macOS (Keychain-based credentials — replace the path with the absolute one):
# DIR=/Users/<you>/.claude-personal
# HASH=$(printf '%s' "$DIR" | shasum -a 256 | cut -c1-8)
# TOKEN=$(security find-generic-password -s "Claude Code-credentials-$HASH" -w | jq -r .claudeAiOauth.accessToken)

curl -sD - -o /dev/null https://api.anthropic.com/v1/messages \
  -H "Authorization: Bearer $TOKEN" \
  -H "anthropic-version: 2023-06-01" \
  -H "anthropic-beta: oauth-2025-04-20" \
  -H "User-Agent: claude-code/2.1.5" \
  -H "Content-Type: application/json" \
  -d '{"model":"claude-haiku-4-5-20251001","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}' \
| grep -i ratelimit-unified
```

If you see five `anthropic-ratelimit-unified-*` headers, the probe works.

---

## Cost

1 Haiku token × N accounts × 1440 polls/day ≈ **$0.002/day for 2 accounts**.

---

## File layout

```
src/                      → frontend (vanilla JS + HTML, no framework)
  popover.html              tray pop-up
  widget.html               draggable always-on-top pill
  settings.html             config window
  app.js                    shared rendering helpers
  styles.css

src-tauri/
  src/
    lib.rs                  Tauri commands & app bootstrap
    poller.rs               periodic probe runner; macOS reads/writes Keychain,
                            Ubuntu reads/writes file
    probe.rs                api.anthropic.com call + header parsing
    oauth_refresh.rs        token refresh (used in background by the poller)
    credentials.rs          per-profile Keychain access (macOS, sha256-suffixed
                            service name) + .credentials.json read/write (0600)
    config.rs               app config (~/.config/clawd-tracker/)
    tray.rs                 menu-bar icon, semaphore-colored title,
                            per-profile filtering via show_in_tray
  tauri.conf.json
  Cargo.toml
```

The app does **not** ship its own login flow — it reads whatever
`claude /login` already wrote (Keychain on macOS, file on Linux). Token
refresh happens transparently in the background and is written back to the
same store.

---

## Troubleshooting

| Symptom                                            | Fix                                                                              |
|----------------------------------------------------|----------------------------------------------------------------------------------|
| Profile shows "login required" (macOS)             | Run `CLAUDE_CONFIG_DIR=<that dir> claude /login` — the app picks it up within 60s. |
| Profile shows "login required" (Ubuntu)            | Same: `CLAUDE_CONFIG_DIR=<dir> claude /login` writes `.credentials.json`.        |
| Popover stuck after re-login                       | Click **Refresh** in the right-click menu, or wait one poll interval (≤60s).     |
| Tray icon missing on Ubuntu/GNOME                  | Install `gnome-shell-extension-appindicator` and re-login.                       |
| `not Pro/Max` on a clearly Max account             | The OAuth scope grant didn't include `user:inference`. Re-run `claude /login`.   |
| Auto-detect missed a macOS account                 | Make sure you've logged into it via the CLI at least once with the matching `CLAUDE_CONFIG_DIR` — the app keys off the Keychain entry created by that login. |
| Menu bar shows fewer entries than expected         | Open Settings and check the **show in menu bar** box for the missing profile. |
| Want a single global indicator instead of per-account dots | Disable **show in menu bar** for every account except one. |
