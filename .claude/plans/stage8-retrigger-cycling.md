# Stage 8 — Re-trigger cycling via cosmic-comp + summon-on-previous

## Context

Stage 7 shipped a daemon that listens for `SIGUSR1` to show the overlay, with Tab / Shift+Tab cycling handled inside the layer surface. When `Alt+Tab` is bound in cosmic-comp to `cosmic-app-switcher-show`, the summon works — but subsequent `Tab` taps (while Alt is still held) never reach the daemon: cosmic-comp's keybinding system intercepts every `Alt+Tab` keypress and re-invokes the script, so the keyboard event is consumed before our layer surface can see it.

This stage embraces that re-trigger behavior instead of fighting it. Each re-fired script invocation becomes the cycling signal:

- `SIGUSR1` → if hidden, show the overlay with the **previous window** (MRU index 1) highlighted; if already shown, cycle forward by one.
- `SIGUSR2` → if hidden, show with the **least-recent window** (MRU index `len-1`) highlighted; if already shown, cycle backward by one.
- The wrapper script accepts an optional `--back` flag and chooses the signal accordingly.
- cosmic-comp keybindings: `Alt+Tab → cosmic-app-switcher-show`, `Alt+Shift+Tab → cosmic-app-switcher-show --back`.

Initial-highlight default moves from index 0 → index 1, matching standard GNOME/Windows/macOS Alt+Tab muscle memory (one Alt+Tab lands on the previously focused window). The in-app `Tab` / `Shift+Tab` listen_raw fallback stays so manual / terminal-based summons still cycle.

Feature branch: `f/stage8-retrigger-cycling`.

## Files to modify

### `src/app.rs`

**`Message` enum** — collapse `Show` into the cycle variants:
- Remove `Message::Show`.
- Repurpose `Message::CycleNext` to mean "summon-with-prev OR cycle forward" (depending on `self.shown`).
- Repurpose `Message::CyclePrev` to mean "summon-with-last OR cycle backward".
- Keep `Message::Hide` and `Message::WindowsUpdated` unchanged.

**`update()`** — rewrite the cycle handlers to branch on `self.shown`:

```rust
Message::CycleNext => {
    if !self.shown {
        return self.summon_with_highlight(1);
    }
    if !self.windows.is_empty() {
        self.highlighted_index = (self.highlighted_index + 1) % self.windows.len();
        return self.scroll_to_highlighted();
    }
}
Message::CyclePrev => {
    if !self.shown {
        return self.summon_with_highlight(self.windows.len().saturating_sub(1));
    }
    if !self.windows.is_empty() {
        let len = self.windows.len();
        self.highlighted_index = (self.highlighted_index + len - 1) % len;
        return self.scroll_to_highlighted();
    }
}
```

Extract a helper that lifts the surface-creation block currently inline in `Show`:

```rust
fn summon_with_highlight(&mut self, idx: usize) -> Task<cosmic::Action<Message>> {
    self.shown = true;
    self.highlighted_index = idx.min(self.windows.len().saturating_sub(1));
    get_layer_surface(SctkLayerSurfaceSettings { /* same settings block */ })
}
```

The `idx.min(len-1)` clamp covers the 1-or-0 window case automatically — `summon_with_highlight(1)` lands on 0 when only one window exists, and `summon_with_highlight(usize::saturating_sub)` already handles empty.

**`subscription()`** — rename and rewrite the signal subscription to listen for both signals via `tokio::select!`. Replace `sigusr1_subscription()` with:

```rust
fn signals_subscription() -> Subscription<Message> {
    Subscription::run(|| {
        cosmic::iced::stream::channel(
            4,
            |mut output: cosmic::iced::futures::channel::mpsc::Sender<Message>| async move {
                let mut sig1 = signal(SignalKind::user_defined1())
                    .expect("install SIGUSR1 handler");
                let mut sig2 = signal(SignalKind::user_defined2())
                    .expect("install SIGUSR2 handler");
                loop {
                    tokio::select! {
                        Some(_) = sig1.recv() => {
                            let _ = output.send(Message::CycleNext).await;
                        }
                        Some(_) = sig2.recv() => {
                            let _ = output.send(Message::CyclePrev).await;
                        }
                        else => break,
                    }
                }
            },
        )
    })
}
```

The in-app `listen_raw` Tab / Shift+Tab handlers stay as-is — they emit `CycleNext` / `CyclePrev`, which now do the right thing in both states. (They only ever fire when the surface is shown, so the `!self.shown` branches are dormant on this path.)

### `scripts/cosmic-app-switcher-show`

Accept an optional `--back` flag and send the matching signal. Otherwise unchanged.

```bash
#!/usr/bin/env bash
# Trigger the cosmic-app-switcher overlay. Bind to Alt+Tab in cosmic-comp
# keybindings, and Alt+Shift+Tab to `cosmic-app-switcher-show --back`.

sig="-SIGUSR1"
if [ "$1" = "--back" ]; then
    sig="-SIGUSR2"
fi

# Daemon already running: signal it.
if pid=$(pidof cosmic-app-switcher) && [ -n "$pid" ]; then
    kill "$sig" $pid
    exit 0
fi

# Not running: start it, then signal once it's up.
setsid -f cosmic-app-switcher >/dev/null 2>&1
for _ in 1 2 3 4 5 6 7 8 9 10; do
    sleep 0.05
    if pid=$(pidof cosmic-app-switcher) && [ -n "$pid" ]; then
        kill "$sig" $pid
        exit 0
    fi
done
exit 1
```

### `README.md`

Update the "End-user setup" section to instruct binding both Alt+Tab → `cosmic-app-switcher-show` and Alt+Shift+Tab → `cosmic-app-switcher-show --back`. Note the new initial-highlight semantics ("first Alt+Tab lands on the previously-focused window").

### `Cargo.toml`, `src/wayland.rs`, `src/main.rs`, `justfile`

No changes.

## Critical APIs to reuse

- `tokio::signal::unix::{SignalKind, signal}` — already imported in `src/app.rs:18`. `SignalKind::user_defined2()` is the SIGUSR2 constructor.
- `tokio::select!` — pulled in by tokio's `macros` feature, already enabled via `features = ["full"]` in `Cargo.toml`.
- `self.scroll_to_highlighted()` — already in place from stage 7's auto-scroll work; both cycle handlers continue to use it.
- All layer-shell + listen_raw plumbing from stage 7 stays untouched.

## Highlight semantics change

The initial highlight on summon moves from index 0 → the previous-window slot (index 1 via SIGUSR1, index `len-1` via SIGUSR2), clamped down to 0 when there's only one window. This is the behavior change end users will perceive.

## Verification

Compile gates:
- `just build-release` — clean build.
- `just check` — clippy with pedantic warnings clean.

Manual end-to-end:
1. Stop any existing daemon (`kill $(pidof cosmic-app-switcher)`), build, restart it detached.
2. In COSMIC Settings → Input → Keyboard → Shortcuts → Custom: bind `Alt+Tab` → `cosmic-app-switcher-show` and `Alt+Shift+Tab` → `cosmic-app-switcher-show --back`.
3. Open 4+ apps so the MRU has depth.
4. Tap `Alt+Tab` (release immediately) — overlay should flash up with the **second** row (previous window) highlighted, then hide as Alt releases. (Real switching is still deferred, so nothing actually changes focus.)
5. Hold `Alt`, tap `Tab` repeatedly — highlight advances one row per tap, auto-scrolls when it leaves the viewport.
6. Hold `Alt`, tap `Shift+Tab` repeatedly — highlight moves backward.
7. From hidden state, press `Alt+Shift+Tab` — overlay shows with the **last** row highlighted (least-recent window).
8. Release Alt or press Esc — overlay hides, daemon stays alive (`pgrep -f cosmic-app-switcher`).
9. Sanity: summon via `./scripts/cosmic-app-switcher-show` from a terminal, then press in-app `Tab` directly (no Alt held) — the listen_raw fallback should still cycle.
10. Single-window edge: close all but one app, summon — highlight clamps to 0, cycling is a no-op, no panic.

## Out of scope (next stages)

- Activating the highlighted window on dismissal (still deferred from stage 7).
- DBus / portal-based activation surface (replaces the SIGUSR1/2 mechanism if and when COSMIC ships a GlobalShortcuts portal).
- Cancellation on `Esc` vs commitment on Alt-release distinction (currently both just hide).
