# Plan: Sort window list by activation (MRU Vec approach)

## Context

The first Stage 2 plan (`stage2-sort-by-last-focused.md`) tried to sort by a `HashMap<String, Instant>` of timestamps updated whenever a toplevel event arrived. That implementation lives on branch `f/sort-windows` (commits `21111ad`, `7a12a9a`, `b7b5ad2`) and has a persistent bug: some non-focused windows get pushed to the top, and the "fix" only nudges them down by one position, leaving the overall ordering wrong.

Root cause of the bug: every toplevel state event re-delivers the **full** state set. When the currently-active window has any field change (title, geometry, output), its state still contains `Activated`, so the heuristic that scans `toplevels()` looking for "first activated whose identifier differs from `prev_activated`" reassigns the timestamp based on `toplevels()`'s creation-order iteration — not on what the user actually focused. Order then drifts at every unrelated event.

The shipping COSMIC Alt+Tab does not use timestamps. Tracing it back:

- Alt+Tab in `cosmic-comp` (`data/keybindings.ron`) spawns the configured `cosmic-launcher alt-tab` command
- `cosmic-launcher` (`src/app.rs:518-530`) just reverses the list pop-launcher gives it
- `pop-os/launcher` `plugins/src/cosmic_toplevel/mod.rs:73-87` is where the ordering lives:

```rust
if let Some(pos) = app.toplevels.iter().position(|t| t.foreign_toplevel == info.foreign_toplevel) {
    if info.state.contains(&State::Activated) {
        app.toplevels.remove(pos);
        app.toplevels.push(Box::new(info));   // move to end
    } else {
        app.toplevels[pos] = Box::new(info);   // update in place
    }
} else {
    app.toplevels.push(Box::new(info));        // new
}
```

Why this works where the timestamp approach failed: only the toplevel that received an event is touched. The currently-focused window's repeated events keep moving it to the end (a no-op since it's already there). When focus shifts from A to B, B's `Activated` event moves B to the end, and A's update (now without `Activated`) is applied in place — so A stays at the second-to-last slot, which is correct. No timestamps, no global scanning, no dependency on `toplevels()` iteration order.

## Strategy

Replace the timestamp map with a single `mru_order: Vec<String>` of toplevel identifiers, ordered most-recently-activated **last**. Each per-toplevel handler mutates this Vec directly. `emit_window_list` only sorts by position in the Vec — never reads activation state itself for sorting.

We don't mirror `Vec<ToplevelInfo>` like pop-launcher does, since `ToplevelInfoState` already owns the canonical data. We only need our own ordering hint keyed by `identifier`.

## Implementation — `src/wayland.rs` only

Start a fresh branch from `main` (`f/sort-windows-v2`); the failing `f/sort-windows` can be kept for reference or deleted later.

### 1. Replace ordering fields on `WaylandState`

Add a single field to the struct:

```rust
struct WaylandState {
    // ... existing fields ...
    mru_order: Vec<String>, // toplevel identifiers, most-recently-activated LAST
}
```

Initialize as `Vec::new()` in `run_wayland_thread`. No `Instant` / `HashMap` for ordering.

### 2. Add a helper that applies the MRU rule per-toplevel

```rust
impl WaylandState {
    fn note_toplevel_update(
        &mut self,
        handle: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
    ) {
        let Some(info) = self.toplevel_info_state.info(handle) else { return; };
        let id = info.identifier.clone();
        let pos = self.mru_order.iter().position(|x| x == &id);
        if info.state.contains(&zcosmic_toplevel_handle_v1::State::Activated) {
            if let Some(p) = pos { self.mru_order.remove(p); }
            self.mru_order.push(id);
        } else if pos.is_none() {
            // First sighting and not currently activated (e.g. window existed
            // before our app started). Place at the front so all observed
            // activations rank above it.
            self.mru_order.insert(0, id);
        }
        // else: known toplevel, not activated → leave position unchanged.
    }
}
```

### 3. Wire the helper into the three `ToplevelInfoHandler` callbacks

```rust
fn new_toplevel(
    &mut self, _: &Connection, _: &QueueHandle<Self>,
    handle: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
) {
    self.note_toplevel_update(handle);
    self.emit_window_list();
}

fn update_toplevel(
    &mut self, _: &Connection, _: &QueueHandle<Self>,
    handle: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
) {
    self.note_toplevel_update(handle);
    self.emit_window_list();
}

fn toplevel_closed(
    &mut self, _: &Connection, _: &QueueHandle<Self>,
    handle: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
) {
    if let Some(info) = self.toplevel_info_state.info(handle) {
        let id = info.identifier.clone();
        self.mru_order.retain(|x| x != &id);
    }
    self.emit_window_list();
}
```

Both `new_toplevel` and `update_toplevel` must take the handle by name (not `_`) — on `main` they currently ignore it.

### 4. Sort `emit_window_list` by position in `mru_order`

```rust
fn emit_window_list(&self) {
    let active: HashSet<_> = self
        .workspace_state
        .workspaces()
        .filter(|w| w.state.contains(ext_workspace_handle_v1::State::Active))
        .map(|w| w.handle.clone())
        .collect();

    // Higher index in mru_order = more recently activated. Convert to a
    // sort key where smaller = appears earlier in the emitted list.
    let rank = |id: &str| -> usize {
        match self.mru_order.iter().rposition(|x| x == id) {
            Some(p) => self.mru_order.len() - 1 - p,
            None => usize::MAX,
        }
    };

    let mut windows: Vec<(usize, WindowInfo)> = self
        .toplevel_info_state
        .toplevels()
        .filter(|t| t.workspace.is_empty() || t.workspace.iter().any(|h| active.contains(h)))
        .map(|t| (
            rank(&t.identifier),
            WindowInfo { title: t.title.clone(), app_id: t.app_id.clone() },
        ))
        .collect();

    windows.sort_by_key(|(r, _)| *r);
    let _ = self.sender.blocking_send(windows.into_iter().map(|(_, w)| w).collect());
}
```

`emit_window_list` becomes `&self`-only (no longer needs `&mut`).

### 5. Self-exclusion (orthogonal)

`f/sort-windows` commit `7a12a9a` ("hide the app switcher itself from the window list") is independent of sorting. Cherry-pick or re-implement separately if wanted; don't bundle with this fix.

## Critical files

- `src/wayland.rs` — only file modified
  - `WaylandState` struct definition
  - new helper `WaylandState::note_toplevel_update`
  - `ToplevelInfoHandler` impl: `new_toplevel`, `update_toplevel`, `toplevel_closed`
  - `WaylandState::emit_window_list`
  - `run_wayland_thread` struct init (one new field)

No changes to `WindowInfo`, `app.rs`, `main.rs`, or `Cargo.toml`.

## Verification

1. `just check` — compiles, no clippy warnings.
2. `just run` — app launches and shows the current window list.
3. Open three apps on the active workspace (e.g. Terminal, Files, Firefox).
4. Click each in turn. Confirm the last-clicked window appears first.
5. Re-focus an earlier window. Confirm it jumps to top; previous top drops to second.
6. **Regression check for the v1 bug:** change the title of an unfocused window (e.g. navigate in an unfocused browser, run a command in an unfocused terminal). Confirm that window does **not** move toward the top; the currently-focused window stays on top.
7. Close one of the listed windows. Confirm it disappears, remaining order preserved.
8. Switch workspaces and back. Confirm per-workspace listing correct and order persists.

Step 6 is the canary for the timestamp-based v1 failure mode.

## References

- Shipping COSMIC Alt+Tab MRU logic: `pop-os/launcher` → `plugins/src/cosmic_toplevel/mod.rs:73-87`
- `cosmic-launcher` view-side reversal: `pop-os/cosmic-launcher` → `src/app.rs:518-530`
- Stage 1 baseline: `src/wayland.rs` on `main`
- Previous failed plan: `.claude/plans/abandoned/stage2-sort-by-last-focused.md`
- Previous failed branch: `f/sort-windows` (commits `21111ad`, `7a12a9a`, `b7b5ad2`)
