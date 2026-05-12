# Plan (CANCELLED): Exclude the app switcher from its own window list

## Status

**Cancelled in favor of stage 4 (layer-shell overlay).** No plan file was finalized and no code was written; this document exists only to record what the stage was meant to do and why it was dropped.

## What was proposed

The switcher currently runs as a regular `xdg_toplevel` window, so it appears in its own window listing. Stage 3 was going to filter it out. Two approaches were considered:

1. **In `src/app.rs::view()`** — build a `visible` Vec by filtering `self.windows` where `app_id != Self::APP_ID`. Mirrors the approach on the failing `f/sort-windows` branch (commit `7a12a9a`). ~6 lines, one file.
2. **In `src/wayland.rs`** — thread `AppModel::APP_ID` through `subscribe() → run_wayland_thread → WaylandState`, then filter in `emit_window_list` and `note_toplevel_update`. Cleaner (single source of truth), but ~15-20 lines of plumbing across two files.

## Why cancelled

`cctk::toplevel_info` is built on the `ext_foreign_toplevel_list_v1` protocol, which only enumerates `xdg_toplevel` windows. Wayland layer-shell surfaces (the primitive used by panels, OSDs, popovers, and by `cosmic-launcher` for its Alt+Tab overlay) are explicitly **not** exposed by that protocol.

Once stage 4 converts this app to a layer-shell overlay (`cosmic::app::Settings::no_main_window(true)` + `get_layer_surface(...)`), the switcher's surface won't appear in `toplevel_info_state.toplevels()` at all. Any self-exclusion filter written in stage 3 would be filtering against an `app_id` that no longer corresponds to a toplevel — dead code from day one of stage 4.

Promoting the layer-shell work is more economical than writing scaffolding slated for removal.

## Branch disposition

`f/stage3-exclude-self` was created from `f/sort-windows-v2` but contains no functional changes (it was a fresh branch off the merge commit). Safe to delete after stage 4 lands.

## References

- Discussion that led to the cancellation: cosmic-launcher uses `no_main_window(true)` and `get_layer_surface(...)` in its `src/app.rs` — layer-shell, not xdg_toplevel.
- `wlr_layer_shell_v1` protocol summary (excludes layer surfaces from `ext_foreign_toplevel_list_v1`).
- Previous failed self-exclusion attempt: `f/sort-windows` commit `7a12a9a`.
