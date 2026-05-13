# Clawd Tracker

A menu-bar / tray app for **macOS** and **Ubuntu** that shows live
**5-hour session** and **7-day weekly** utilization for one or more
Claude Pro/Max accounts.

Inspired by [Clawdmeter](https://github.com/HermannBjorgvin/Clawdmeter):
probe `api.anthropic.com/v1/messages` once per minute per account with a single
Haiku token (~$0.0000008) and read the `anthropic-ratelimit-unified-*`
response headers.

```
┌────────────────────────────────────┐
│  jere                          37% │
│  5H ████░░░░░░░░░░░░░  37%         │
│  7D ██░░░░░░░░░░░░░░░  12%         │
├────────────────────────────────────┤
│  ravel                          0% │
│  login required — Open Settings    │
└────────────────────────────────────┘
```

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
curl -LO https://github.com/<your-user>/clawd-tracker/releases/latest/download/clawd-tracker_<version>_amd64.deb
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

### Logging in from inside the app (no CLI required)

The app ships with a built-in OAuth flow:

1. Settings → pick the profile → **Login**.
2. The system browser opens at `claude.ai/oauth/authorize`. Approve.
3. Anthropic's console renders a code (looks like `xxxx#yyyy`).
4. Copy the **entire** string (code + `#` + state) into the app input → **Submit**.
5. The button flips to **✓ Logged in** and `~/.claude-<profile>/.credentials.json`
   is written with `0600` perms.

If you see **HTTP 429**, wait ~10 minutes — the OAuth endpoint cools down
after repeated attempts. The app shows a friendly message in this case.

---

## Daily use

- **Left-click tray icon** → popover with all enabled accounts.
- **Right-click tray icon** → Refresh / Settings / Quit.
- The widget (toggleable from the popover header) is a draggable, always-on-top
  pill if you want a permanent corner indicator.

The tray title shows the **highest** of (5H, 7D) percentages across all
profiles, so you see the worst case at a glance.

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

```bash
VERSION=0.1.0
gh release create v$VERSION \
  --title "v$VERSION" \
  --notes "Release notes here" \
  src-tauri/target/release/bundle/dmg/*.dmg \
  src-tauri/target/release/bundle/deb/*.deb
```

Binaries live in **GitHub Releases**, not in the git history.

---

## Verify the probe manually

Sanity-check that your account actually returns the unified headers (only
Pro/Max accounts do):

```bash
TOKEN=$(jq -r .claudeAiOauth.accessToken ~/.claude-personal/.credentials.json)
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
    poller.rs               periodic probe runner
    probe.rs                api.anthropic.com call + header parsing
    oauth_login.rs          manual-paste OAuth flow
    oauth_refresh.rs        token refresh
    credentials.rs          .credentials.json read/write (0600)
    config.rs               app config (~/.config/clawd-tracker/)
    tray.rs                 menu bar / tray icon
  tauri.conf.json
  Cargo.toml
```

---

## Troubleshooting

| Symptom                                            | Fix                                                                              |
|----------------------------------------------------|----------------------------------------------------------------------------------|
| Login → "HTTP 429"                                 | Wait ~10 min. Anthropic rate-limits the OAuth token endpoint after retries.      |
| Login → "Invalid request format"                   | Make sure you're on app version ≥ 0.1.0; older versions sent wrong scopes.       |
| Popover stuck at "login required" after success    | Click **Refresh** in the right-click menu, or wait one poll interval (≤60s).     |
| Tray icon missing on Ubuntu/GNOME                  | Install `gnome-shell-extension-appindicator` and re-login.                       |
| `not Pro/Max` on a clearly Max account             | The OAuth scope grant didn't include `user:inference`. Re-login from Settings.   |
