# Stage 7 — Background daemon + Alt+Tab keybindings

## Context

Today the switcher launches its layer-shell overlay immediately on `just run` and the ESC handler tears down the surface *and* exits the process. The end-user product is a background daemon: invisible at idle, summoned on Alt+Tab, dismissed when Alt is released, and re-summonable without restarting the process.

This stage delivers that complete lifecycle — *except* for actually activating the highlighted window on dismissal (window-switching is deferred). What we ship here:

- App starts as a background daemon (no visible surface).
- `SIGUSR1` shows the overlay; the user wires `Alt+Tab → cosmic-app-switcher-show` (a shipped wrapper) in cosmic-comp keybindings to get the real Alt+Tab UX.
- While the overlay is shown, `Tab` cycles a highlighted row forward, `Shift+Tab` cycles backward.
- Releasing `Alt` (or pressing `Esc`) hides the surface; the process stays alive.

Wayland does not allow a client to register a global Alt+Tab — only the compositor can. SIGUSR1 is the production-shape bridge between cosmic-comp and our daemon; it's a few lines of code and matches how lightweight services like this are typically driven.

Feature branch: `f/stage7-daemon-keybindings`.

## Files to modify

### `src/app.rs`

**`AppModel` struct (line 18–23)** — add two fields:
- `shown: bool` — tracks whether the layer-shell surface currently exists. Guards against duplicate `Show` / `Hide` work.
- `highlighted_index: usize` — selection cursor into `self.windows`.

**`Message` enum (line 26–29)** — replace `Dismiss` with explicit lifecycle variants:
- `Show` — emitted by the SIGUSR1 subscription.
- `Hide` — emitted on Alt-release or Esc.
- `CycleNext` — emitted on Tab (no Shift).
- `CyclePrev` — emitted on Shift+Tab.

**`init()` (line 46–76)** — remove the `get_layer_surface(...)` call. Return `Task::none()`. The wayland subscription continues running via `subscription()`, so the MRU list is fresh when the user summons.

**`update()` (line 128–141)**:
- `Show` → if `!self.shown`: set `self.shown = true`, `self.highlighted_index = 0`, return the existing `get_layer_surface(...)` task (same `SctkLayerSurfaceSettings` block currently in `init()`). If already shown: `Task::none()`.
- `Hide` → if `self.shown`: set `self.shown = false`, return `destroy_layer_surface(self.window_id)`. **Do not** call `cosmic::iced::exit()`. If `!self.shown`: `Task::none()` (lets us emit `Hide` from the keyboard subscription unconditionally without leaking).
- `CycleNext` → guard `!windows.is_empty()`; `self.highlighted_index = (self.highlighted_index + 1) % len`.
- `CyclePrev` → guard `!windows.is_empty()`; `self.highlighted_index = (self.highlighted_index + len - 1) % len`.
- `WindowsUpdated` → keep existing behavior; clamp `highlighted_index` to `< windows.len()` after the update.

**`subscription()` (line 115–126)** — three subscriptions, batched:

1. Existing wayland window-list subscription (unchanged).
2. New SIGUSR1 subscription emitting `Message::Show` (see snippet below).
3. Expanded `listen_raw` keyboard handler:
   - `KeyEvent::KeyPressed { key: Key::Named(Named::Tab), modifiers, .. }` where `modifiers.shift()` → `CyclePrev`
   - `KeyEvent::KeyPressed { key: Key::Named(Named::Tab), .. }` → `CycleNext`
   - `KeyEvent::KeyPressed { key: Key::Named(Named::Escape), .. }` → `Hide`
   - `KeyEvent::ModifiersChanged(mods)` where `!mods.alt()` → `Hide` (the Alt-release detector — `update()` ignores it when `!self.shown`, so this stays self-contained inside the closure)

**`window_row()` (line 158–181)** — add a `highlighted: bool` parameter. When `true`, wrap the row in a themed container with a tinted background (try `cosmic::theme::Container::Primary` first; fall back to inline `style(|theme| ...)` if no built-in class looks right). Caller `view_window()` (line 85–113) passes `i == self.highlighted_index` on each iteration.

### `src/wayland.rs`

No changes. `WindowInfo` and the MRU-ordered `Vec` are sufficient.

### `src/main.rs`

No changes. `no_main_window(true).exit_on_close(false)` is already correct for daemon mode.

### `Cargo.toml`

No changes. `tokio = { version = "1.48.0", features = ["full"] }` already includes the `signal` feature.

### New file: `scripts/cosmic-app-switcher-show`

A small wrapper the end user binds `Alt+Tab` to in cosmic-comp keybindings. Handles both "daemon already running" and "daemon not yet started" cases so the user only ever needs to configure one keybind:

```bash
#!/usr/bin/env bash
# Trigger the cosmic-app-switcher overlay. Bind this to Alt+Tab in
# cosmic-comp keybindings (System Settings → Input → Keyboard → Shortcuts).

# Daemon already running: signal it to show the overlay.
if pkill -SIGUSR1 -x cosmic-app-switcher 2>/dev/null; then
    exit 0
fi

# Not running: start it, then signal once it's up.
setsid -f cosmic-app-switcher >/dev/null 2>&1
for _ in 1 2 3 4 5 6 7 8 9 10; do
    sleep 0.05
    if pkill -SIGUSR1 -x cosmic-app-switcher 2>/dev/null; then
        exit 0
    fi
done
exit 1
```

Notes:
- `pkill -x` matches the exact basename to avoid signalling other processes that happen to contain "cosmic-app-switcher" in their cmdline.
- The retry loop covers the race between `setsid` spawning the daemon and the daemon installing its SIGUSR1 handler. ~500 ms total wait is plenty under load.
- `setsid -f` detaches the daemon from the shell that spawned it so the keybinding command doesn't carry a live child.

### `justfile`

Extend the `install` recipe to also install the script to `$prefix/bin/`, mirroring the `install -Dm0755` pattern already used for the main binary. Add a corresponding `rm` line to `uninstall`.

## SIGUSR1 subscription

Mirror the `Subscription::run` + `cosmic::iced::stream::channel` pattern that `src/wayland.rs::subscribe()` (line 218–231) already establishes:

```rust
fn sigusr1_subscription() -> Subscription<Message> {
    use cosmic::iced::futures::SinkExt;
    use tokio::signal::unix::{signal, SignalKind};

    Subscription::run(|| {
        cosmic::iced::stream::channel(4, |mut output| async move {
            let mut sig = signal(SignalKind::user_defined1())
                .expect("install SIGUSR1 handler");
            while sig.recv().await.is_some() {
                let _ = output.send(Message::Show).await;
            }
        })
    })
}
```

## Highlight design choice

`AppModel.highlighted_index` defaults to `0` on each `Show`. Since `self.windows` is MRU-sorted (index 0 = most recently focused), this matches "the list item representing the last focused window is highlighted" literally. Once real window-activation lands in a later stage, we'll likely want to default to index `1` (the *previous* window, which is the standard Alt+Tab landing). Flag this trade-off during implementation but don't change it here.

## Critical APIs to reuse

- `cosmic::iced::event::listen_raw` — already imported in `src/app.rs:5`. Extended, not replaced.
- `get_layer_surface` / `destroy_layer_surface` — imported in `src/app.rs:8-10`. Move the existing surface settings block from `init()` to the `Show` handler in `update()`.
- `Subscription::run` + `cosmic::iced::stream::channel` — pattern lives in `src/wayland.rs::subscribe()` (line 218–231).
- `cosmic::iced::keyboard::{Modifiers, key::Named}` — already imported.

## Verification

Compile gates:
- `just build-release` — must build clean.
- `just check` — clippy with pedantic warnings must be clean.

Manual end-to-end (simulates the eventual cosmic-comp keybinding):
1. `just run &` — process starts; **no overlay** visible; no errors on stderr.
2. Open 3+ apps in the COSMIC session so the window list has multiple entries.
3. `pkill -SIGUSR1 cosmic-app-switcher` — overlay appears, top row highlighted.
4. Hold `Alt`, press `Tab` — highlight moves down one row per press, wraps last → first.
5. Hold `Alt+Shift`, press `Tab` — highlight moves up one row, wraps first → last.
6. Release `Alt` — overlay disappears. Confirm process still alive with `pgrep cosmic-app-switcher`.
7. `pkill -SIGUSR1 cosmic-app-switcher` again — overlay re-appears with `highlighted_index = 0` (verifies recreate-after-destroy path works).
8. Inside overlay, press `Esc` — overlay disappears; process still alive.
9. End-user simulation via the shipped wrapper:
   - `./scripts/cosmic-app-switcher-show` while the daemon is already running → overlay appears (signal path).
   - Kill the daemon, then run `./scripts/cosmic-app-switcher-show` from a terminal — daemon spawns and overlay appears within ~500ms (cold-start path).
   - In cosmic-comp keybindings, bind `Alt+Tab` → `cosmic-app-switcher-show` and exercise the full UX (overlay summons, Tab/Shift+Tab cycles, releasing Alt hides).

Risk to verify during implementation: re-creating the layer surface with the same `SurfaceId` after `destroy_layer_surface`. If reusing the id fails, generate a fresh `SurfaceId` per `Show` and store it back in `self.window_id`.

## Out of scope (next stages)

- Activating the highlighted window on Alt-release (the actual window-switching action).
- Replacing SIGUSR1 with DBus or a portal-based activation surface.
- Documenting the cosmic-comp keybinding setup for end users.
- Multi-output / multi-monitor placement.
