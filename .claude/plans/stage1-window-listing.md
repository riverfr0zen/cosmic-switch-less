# Plan: Get a list of open windows in the current workspace

## Context

This is Stage 1 of building a COSMIC Desktop app switcher. The goal of this stage is purely informational: establish a working pipeline that retrieves the list of open windows filtered to the currently active workspace, and displays them in the app UI as text (title + app_id). No window switching, no thumbnails — just proving we can query the compositor.

COSMIC Desktop runs on Wayland. The compositor exposes open windows and workspace state through two custom Wayland protocols:
- `ext_foreign_toplevel_handle_v1` — announces open windows (toplevels) and their properties
- `ext_workspace_handle_v1` / `zcosmic_workspace_handle_v1` — announces workspace state (which workspace is active)

The `cctk` (cosmic-client-toolkit) crate provides high-level Rust handler traits for both protocols, built on top of `wayland-client` and `calloop`.

Because libcosmic internally manages its own Wayland connection for the UI, we open a **separate** Wayland connection dedicated to protocol queries. This is the same pattern used by `cosmic-workspaces-epoch` and `cosmic-applets`. This secondary connection runs on a **dedicated OS thread** with a `calloop` event loop; it communicates back to the iced application via an `mpsc` channel bridged into an iced `Subscription`.

Reference implementations:
- `cosmic-applets/cosmic-applet-workspaces/src/wayland_subscription.rs` — shows the channel + subscription pattern
- `cosmic-workspaces-epoch/src/backend/wayland/mod.rs` — shows ToplevelInfoHandler + WorkspaceHandler dispatch

---

## Step 1 — Add dependencies to `Cargo.toml`

```toml
wayland-client = "0.31"
calloop = "0.14"
calloop-wayland-source = "0.3"
cosmic-protocols = { git = "https://github.com/pop-os/cosmic-protocols", features = ["client"] }
cctk = { package = "cosmic-client-toolkit", git = "https://github.com/pop-os/cosmic-protocols" }
```

`wayland-client` is already a transitive dependency (via libcosmic), but we need it as a direct dep to use it in our code. The `calloop` version must match what `smithay-client-toolkit` (already in the tree) uses — verify in `Cargo.lock` after adding.

---

## Step 2 — Define shared types in `src/wayland.rs` (new file)

```rust
pub struct WindowInfo {
    pub title: String,
    pub app_id: String,
}
```

---

## Step 3 — Implement the Wayland listener thread in `src/wayland.rs`

The listener is the bulk of the new code. Structure:

1. **`WaylandState` struct** — holds:
   - `toplevels: HashMap<ToplevelHandle, ToplevelData>` (title, app_id, which workspaces it's on)
   - `workspaces: HashMap<WorkspaceHandle, WorkspaceData>` (name, is_active)
   - `sender: mpsc::Sender<Vec<WindowInfo>>` — channel back to iced

2. **Implement `cctk::toplevel_info::ToplevelInfoHandler` on `WaylandState`**:
   - `new_toplevel` / `update_toplevel` — insert/update the toplevels map
   - `toplevel_closed` — remove from map
   - After each update, call a helper `emit_window_list()` that filters toplevels to those whose workspace is currently active and sends the result

3. **Implement `cctk::workspace::WorkspaceHandler` on `WaylandState`**:
   - Track active workspace state as workspace events arrive
   - After each update, also call `emit_window_list()`

4. **`subscribe()` function** — creates the iced `Subscription`:
   ```rust
   pub fn subscribe() -> iced::Subscription<Vec<WindowInfo>> {
       iced::Subscription::run(|| {
           iced::stream::channel(64, |mut output| async move {
               let (tx, mut rx) = tokio::sync::mpsc::channel(64);
               std::thread::spawn(move || run_wayland_thread(tx));
               while let Some(windows) = rx.recv().await {
                   let _ = output.send(windows).await;
               }
           })
       })
   }
   ```

5. **`run_wayland_thread(tx)`** — blocking function that:
   - Opens a `wayland_client::Connection`
   - Creates an `EventQueue` and `WaylandState`
   - Sets up a `calloop::EventLoop` with `WaylandSource`
   - Binds registry globals for toplevel info and workspace protocols via `cctk`
   - Runs `event_loop.run(None, &mut state, None)` indefinitely

---

## Step 4 — Wire into `src/app.rs`

**Add to `AppModel`:**
```rust
windows: Vec<crate::wayland::WindowInfo>,
```

**Add to `Message`:**
```rust
WindowsUpdated(Vec<crate::wayland::WindowInfo>),
```

**In `subscription()`**, add:
```rust
crate::wayland::subscribe().map(Message::WindowsUpdated),
```

**In `update()`**, handle:
```rust
Message::WindowsUpdated(windows) => {
    self.windows = windows;
}
```

**In `view()`** on Page1, replace the timer widget with (or add after it) a list of window entries — one row per `WindowInfo` showing title and app_id.

---

## Step 5 — Add module to `src/main.rs`

```rust
mod wayland;
```

---

## Verification

1. Run `just check` — should compile cleanly with no clippy errors
2. Run `just run` — app window opens
3. Open a few other apps (terminal, browser, etc.) on the current workspace
4. Page 1 of the app switcher should display their titles and app_ids
5. Switch to a different workspace — the list should update to show only windows on the new active workspace
6. Open a window on a second workspace, switch back — only that workspace's windows should appear
