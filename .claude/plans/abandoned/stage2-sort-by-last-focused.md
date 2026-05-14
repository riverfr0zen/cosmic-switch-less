# Plan: Sort window list by last-focused (descending)

## Context

The window listing from Stage 1 returns windows in compositor-defined order. This stage sorts them so the most recently focused window appears first, giving the app-switcher behavior expected from an Alt-Tab replacement.

The `zcosmic_toplevel_handle_v1` protocol already delivers an `Activated` state flag (value 2) in `ToplevelInfo::state` each time a window gains focus. We can use these events to maintain a `last_activated` timestamp map inside the Wayland listener thread, then sort before emitting.

No changes are needed to `WindowInfo`, `app.rs`, or `main.rs` — all work is self-contained in `src/wayland.rs`.

---

## Implementation — `src/wayland.rs` only

### 1. Add import

Add `std::time::Instant` to the imports at the top of the file.

Also add `zcosmic_toplevel_handle_v1` to the use block (it's from `cosmic_protocols::zcosmic::zcosmic_toplevel_handle_v1`). Check that the existing imports already bring this in transitively; if not, add it explicitly.

### 2. Add `last_activated` field to `WaylandState`

```rust
struct WaylandState {
    // ... existing fields ...
    last_activated: HashMap<String, Instant>,  // keyed by ToplevelInfo::identifier
}
```

Initialize it as `HashMap::new()` in `run_wayland_thread` where `WaylandState` is constructed.

### 3. Update `new_toplevel` and `update_toplevel`

In both callbacks, after the existing body, check if the updated toplevel is currently activated and record the timestamp:

```rust
fn update_toplevel(&mut self, _: &Connection, _: &QueueHandle<Self>,
    handle: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1)
{
    if let Some(info) = self.toplevel_info_state.info(handle) {
        if info.state.contains(zcosmic_toplevel_handle_v1::State::Activated) {
            self.last_activated.insert(info.identifier.clone(), Instant::now());
        }
    }
    self.emit_window_list();
}
```

Apply the same pattern to `new_toplevel`.

### 4. Clean up in `toplevel_closed`

```rust
fn toplevel_closed(&mut self, _: &Connection, _: &QueueHandle<Self>,
    handle: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1)
{
    if let Some(info) = self.toplevel_info_state.info(handle) {
        self.last_activated.remove(&info.identifier);
    }
    self.emit_window_list();
}
```

Note: `toplevel_info_state.info(handle)` is still valid at this point — cctk calls the handler *before* cleaning up the internal entry.

### 5. Sort in `emit_window_list()`

Change `emit_window_list` to collect window tuples with their last-activated time, sort, then strip the timestamp:

```rust
fn emit_window_list(&self) {
    let active: HashSet<_> = /* unchanged */;

    let mut windows: Vec<(Option<Instant>, WindowInfo)> = self
        .toplevel_info_state
        .toplevels()
        .filter(|t| t.workspace.is_empty() || t.workspace.iter().any(|h| active.contains(h)))
        .map(|t| {
            let ts = self.last_activated.get(&t.identifier).copied();
            (ts, WindowInfo { title: t.title.clone(), app_id: t.app_id.clone() })
        })
        .collect();

    // Most-recently-activated first; never-activated windows go to the end.
    windows.sort_by(|(ta, _), (tb, _)| tb.cmp(ta));

    let _ = self.sender.blocking_send(windows.into_iter().map(|(_, w)| w).collect());
}
```

`Option<Instant>` sorts correctly with the default `Ord` impl: `None < Some(_)`, so reversing puts `Some` (known times) before `None` (never activated), which is correct.

---

## Critical files

- `src/wayland.rs` — only file modified
  - `WaylandState` struct (~line 26)
  - `emit_window_list()` (~line 35)
  - `ToplevelInfoHandler` impl: `new_toplevel`, `update_toplevel`, `toplevel_closed` (~lines 70-100)
  - `run_wayland_thread` struct initialization (~line 130+)

---

## Verification

1. `just check` — compiles cleanly, no clippy warnings
2. `just run` — app opens
3. Open three apps (e.g. terminal, browser, file manager) on the current workspace
4. Click each in turn to focus them; the last one clicked should appear first in the list
5. Focus the second app — it should move to the top of the list
6. Close one app — it should disappear; order of remaining windows stays correct
7. Switch workspaces and back — list resets to correct per-workspace windows, still sorted by last-focus

Also save this plan to `.claude/plans/stage2-sort-by-last-focused.md` in the project at the start of execution.
