# Clawd Tracker — Rust & architecture primer

A walkthrough of what this codebase is, the stack it uses, and the Rust
concepts you'll bump into while reading it. Written for someone coming
from web/frontend (vanilla JS, React) approaching Rust for the first time.

## 1. What we built

A **menu-bar app** (macOS) / **system-tray app** (Linux) that, once per
minute, hits the Anthropic API with a tiny token (~$0.0000008 each call)
and reads the `anthropic-ratelimit-unified-*` response headers to figure
out how much of your Claude Pro/Max quota you've used — 5-hour session
window + 7-day weekly cap. It shows both in the menu bar and a popover.

The clever bit: **it does not invent its own login flow**. It reuses the
credentials that `claude /login` already writes to the macOS Keychain or
to `~/.claude*/.credentials.json` on Linux. No forms inside the app.

## 2. Stack

| Layer | What it is | Why it's here |
|---|---|---|
| **Tauri 2** | Desktop-app framework: Rust backend + native webview (not Electron) | ~4 MB binaries, native tray icons, autostart, IPC out of the box |
| **Rust** | Backend language (`src-tauri/src/*.rs`) | No GC, compiles to a native binary. Required by Tauri. |
| **Vanilla JS + HTML + CSS** | Frontend (`src/*.html`, `src/app.js`) | The app is small — no need for React/Vue. Talks to Rust via `invoke()` (RPC over IPC) |
| **pnpm** | Node package manager | The frontend only depends on `@tauri-apps/cli` and `@tauri-apps/api` |
| **cargo** | Rust's "npm/pip" | Dependency management + builds |

## 3. Crates (Rust packages) we use

Dependencies live in `src-tauri/Cargo.toml`. The key ones:

```toml
tauri = { version = "2.1", features = ["tray-icon", "image-png"] }   # core framework
tauri-plugin-autostart = "2.0"                                       # launch at login
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }    # HTTP client (≈ fetch)
serde = { version = "1", features = ["derive"] }                     # ≈ JSON.stringify/parse via derive
serde_json = "1"
tokio = { version = "1", features = ["full"] }                       # async runtime (≈ event loop)
chrono = "0.4"                                                       # dates (≈ Date / Temporal)
anyhow = "1"                                                         # unified error type
sha2 = "0.10"                                                        # sha256 hashes (for the Keychain service name)
dirs = "5"                                                           # ~/Library, ~/.config, etc — cross-platform

[target.'cfg(target_os = "macos")'.dependencies]
security-framework = "3"                                             # Apple's Keychain API (macOS only)
```

The `cfg(target_os = "macos")` is like a build-time `#ifdef` — that crate
is only compiled into the macOS build.

## 4. Rust concepts you'll see in this repo

### `Option<T>` and `Result<T, E>`

In JS, "this might be undefined" or "this might throw" is implicit. In
Rust it's **encoded in the type**:

```rust
pub tray_label: Option<String>   // ≈ string | null, but the compiler enforces the null check
```

```rust
fn load() -> Result<Config> {    // ≈ might return Config or an Error — no silent throw
    ...
    Ok(cfg)                       // ≈ return cfg
}
```

### The `?` operator

A shortcut for "if this is an error, bubble it up". Replaces try/catch
when you want linear code:

```rust
let raw = std::fs::read_to_string(&path)?;   // if it fails, return Err(...) automatically
let cfg: Config = serde_json::from_str(&raw)?;
Ok(cfg)
```

### `#[derive(...)]` — Rust's "decorator"

Codegen at compile time. To serialize/deserialize JSON with `serde` you
write zero boilerplate:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    ...
}
```

You now get `Profile → JSON` and `JSON → Profile` for free. That's what
runs every time the app reads `config.json` or sends data to the frontend.

### `async fn` + `tokio`

Same `async`/`await` as JS. The difference: in Rust you have to pick a
**runtime** (`tokio` here) explicitly. Tauri bundles it for you.

```rust
async fn poll_one(profile: &Profile, ...) -> ProfileUpdate {
    let creds = load_credentials(profile)?;
    let state = probe_with(creds, ...).await;
    ...
}
```

### Ownership / borrowing

The central rule: every value has **exactly one owner**. To pass a value
to a function without giving it away, you **borrow** it with `&`
(immutable) or `&mut` (mutable).

Example from the repo:

```rust
pub fn update_tray_title(
    app: &AppHandle,                  // borrowed, not moved
    updates: &[ProfileUpdate],        // borrowed slice (≈ readonly array)
    visible_ids: &HashSet<String>,    // borrowed set
) { ... }
```

The compiler will reject any code that mutates a borrow held as `&`, or
that holds a `&mut` while other borrows exist. That kills an entire
class of race-condition bugs at compile time.

### `Arc<Mutex<T>>` — shared state across tasks

To share state between several concurrent async tasks:

```rust
pub type UsageStoreHandle = Arc<UsageStore>;   // Arc = atomic reference count, ≈ C++ shared_ptr
                                               // UsageStore wraps a Mutex<Vec<ProfileUpdate>>
```

`Arc` lets you clone *pointers* (not the data); `Mutex` forces you to
acquire a lock before reading/writing. The compiler also guarantees you
can't forget to release it — when the lock guard goes out of scope, it's
dropped automatically.

## 5. Repo layout

```
clawd-tracker/
├── src/                    ← frontend (what the webview renders)
│   ├── popover.html        ← 5h/7d cards + dismiss handlers
│   ├── settings.html       ← config UI
│   ├── app.js              ← shared helpers
│   └── styles.css
└── src-tauri/              ← Rust backend
    ├── Cargo.toml          ← dependencies
    ├── tauri.conf.json     ← windows, perms, bundle config
    ├── icons/              ← source SVG + generated PNGs + .icns
    └── src/
        ├── lib.rs          ← bootstrap + commands exposed to the frontend
        ├── poller.rs       ← loop that probes every 60s
        ├── probe.rs        ← the GET to api.anthropic.com + header parsing
        ├── credentials.rs  ← reads/writes Keychain (macOS) or file (Linux)
        ├── oauth_refresh.rs← refreshes the access_token when it expires
        ├── config.rs       ← reads/writes ~/.config/clawd-tracker/config.json
        └── tray.rs         ← icon, title, menu items
```

## 6. How frontend and backend talk

Like fetching your own server, but **in-process**:

**Backend** declares a command:
```rust
#[tauri::command]
async fn save_config(app: AppHandle, cfg: Config) -> Result<(), String> {
    config::save(&cfg)?;
    app.emit("config-changed", ids)?;   // pushes an event to the frontend
    Ok(())
}
```

**Frontend** calls it:
```js
import { invoke } from "@tauri-apps/api/core";
await invoke("save_config", { cfg });
```

Argument/return types are serialized automatically via serde (JSON under
the hood).

## 7. The "huh, that's interesting" findings from building this

- **Anthropic stores one Keychain entry per `CLAUDE_CONFIG_DIR`**, not one
  shared entry. The service name is
  `Claude Code-credentials-<sha256(absolute_path)[:8]>`. We figured this
  out with `security dump-keychain | grep Claude`. That's why you can
  have `claude-personal` and `claude-work` aliases and never log in twice.
- **Cloudflare/WAF** bounces the OAuth code-exchange with HTTP 500 from
  Anthropic's side — not our bug. That's why we dropped the in-app OAuth
  flow entirely.
- **Linux tray icons** use the AppIndicator/KSNI protocol, which **does
  not deliver click events** to the app — it only exposes "open the
  menu". So on Linux the popover opens via the **Show details** menu
  item, not left-click like on macOS.
- **macOS NSStatusItem** does deliver clicks with coordinates, which we
  use to position the popover right below the icon.

## 8. Tools used during the build

- `cargo check` / `cargo build` → validate/compile Rust
- `pnpm tauri dev` / `pnpm tauri build` → hot-reload or release
- `rsvg-convert` → SVG → PNG (for icons)
- `iconutil -c icns` → PNG iconset → `.icns` (macOS app icon)
- `gh release create` → publish the `.dmg` to GitHub Releases
- `security find-generic-password` → debug the Keychain from the terminal

## Further reading

- [The Rust Book](https://doc.rust-lang.org/book/) — official tutorial.
  Chapters 4 (ownership), 6 (enums + Option), 9 (Result) and 10 (traits)
  cover ~80% of what you see in this repo.
- [Tauri docs](https://v2.tauri.app/) — the **Commands** and **Events**
  sections are what we use the most.
- `rustlings` → short, no-IDE exercises to build muscle memory.
