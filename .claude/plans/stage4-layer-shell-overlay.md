# Plan: Convert the switcher to a layer-shell overlay

## Context

The app currently runs as an `xdg_toplevel` window — fine for developing the window-listing logic in stages 1 and 2, but the eventual product is an Alt+Tab replacement that pops up over everything on demand, like rofi/fuzzel or COSMIC's own launcher.

Wayland's `wlr_layer_shell` (and the libcosmic wrappers in `cosmic::iced::platform_specific::shell::commands::layer_surface`) is the right primitive: panels, OSDs, app launchers all use it. Layer-shell surfaces sit above regular windows, can grab exclusive keyboard focus, and — crucially — don't appear in `ext_foreign_toplevel_list_v1`, which is what `cctk::toplevel_info` enumerates. Converting now also makes the self-exclusion problem from cancelled stage 3 go away.

Reference: `pop-os/cosmic-launcher` `src/app.rs` uses exactly this pattern. Its Alt+Tab mode is the closest analogue.

## Scope

**In scope for stage 4:**

- App launches as a layer-shell overlay (centered, on top of regular windows, exclusive keyboard).
- Existing window-list view (MRU-sorted via stage 2) renders inside the overlay.
- ESC dismisses the overlay and exits the process.

**Explicitly out of scope (later stages):**

- Global keybind activation via cosmic-comp `system_actions` (the app is still launched manually via `just run`).
- Persistent-background process toggling visibility on repeat invocations (cosmic-launcher's model).
- Selecting a row to focus the chosen window (`zcosmic_toplevel_manager_v1::activate`).
- Visual polish, icons, scrolling, keyboard navigation between rows, nav-bar removal.

## Implementation

### 1. `src/main.rs` — disable the default toplevel

Replace the Settings chain with:

```rust
let settings = cosmic::app::Settings::default()
    .no_main_window(true)
    .exit_on_close(false);
```

Drop the `size_limits` lines (they configured a toplevel we no longer have; size limits move to the layer surface itself). `exit_on_close(false)` keeps iced alive after we destroy the layer surface so our explicit exit task can run cleanly.

### 2. `src/app.rs` — surface id, layer-surface task, view_window, ESC handling

**Imports** (verify exact paths against the libcosmic git rev — cosmic-launcher's `src/app.rs` is the canonical reference):

```rust
use cosmic::iced::core::window::Id as SurfaceId;
use cosmic::iced::platform_specific::runtime::wayland::layer_surface::SctkLayerSurfaceSettings;
use cosmic::iced::platform_specific::shell::commands::layer_surface::{
    Anchor, KeyboardInteractivity, destroy_layer_surface, get_layer_surface,
};
use cosmic::iced::core::layout::Limits;
use cosmic::iced::keyboard::{self, Key};
```

**Add to `AppModel`:**

```rust
window_id: SurfaceId,
```

**In `init()`** — generate a unique surface id, build the model, and return a Task that creates the layer surface:

```rust
let window_id = SurfaceId::unique();
let app = AppModel {
    // ... existing fields ...
    window_id,
    windows: Vec::new(),
};
let task = get_layer_surface(SctkLayerSurfaceSettings {
    id: window_id,
    keyboard_interactivity: KeyboardInteractivity::Exclusive,
    anchor: Anchor::empty(),                  // no edge anchor = centered
    namespace: "cosmic-app-switcher".into(),
    size: None,
    size_limits: Limits::NONE
        .min_width(360.0).min_height(180.0)
        .max_width(800.0).max_height(600.0),
    exclusive_zone: -1,
    ..Default::default()
});
(app, task)
```

`Anchor::empty()` plus no explicit size = compositor centers the surface using the size_limits. `Exclusive` keyboard interactivity routes all keys (including ESC) to us.

**Add `view_window`** (libcosmic calls this instead of `view()` when `no_main_window(true)` is set):

```rust
fn view_window(&self, id: SurfaceId) -> Element<Self::Message> {
    if id != self.window_id {
        return widget::vertical_space().height(Length::Fixed(1.0)).into();
    }
    // Render the windows list. Lift the Page1 body from the current view()
    // implementation — header + column-of-windows, no nav bar.
    // ...
}
```

Leave the existing `view()` body in place but expect it not to be called. (If the compiler/runtime complains, return an empty element from `view()`.)

**Add `Message::Dismiss`** and route ESC into it:

```rust
// In Message enum:
Dismiss,

// In subscription():
cosmic::iced::keyboard::on_key_press(|key, _modifiers| match key {
    Key::Named(keyboard::key::Named::Escape) => Some(Message::Dismiss),
    _ => None,
}),

// In update():
Message::Dismiss => {
    return Task::batch([
        destroy_layer_surface(self.window_id),
        cosmic::iced::exit(),
    ]);
}
```

Verify the exact name of the exit helper (`cosmic::iced::exit()` vs `cosmic::app::exit()` vs an iced `window::close` variant) — match what cosmic-launcher's `hide()` does for its surface lifecycle.

### 3. Nav-bar / template scaffolding — defer

The current `view()` builds a multi-page nav-bar from the template (`Page1`/`Page2`/`Page3`). For a focused overlay the nav-bar is noise, but stripping it is orthogonal to the layer-shell move. Keep the structs; just don't render them inside `view_window`. Cleanup can come in a follow-up stage.

## Critical files

- `src/main.rs` — Settings chain rewritten (~6 lines).
- `src/app.rs` — new imports, `window_id` field, `init()` return wiring, new `view_window`, ESC subscription, `Message::Dismiss` (~40-60 lines added/changed).

No changes expected in `src/wayland.rs`, `src/config.rs`, `src/i18n.rs`.

## Verification

1. `just check` — clippy clean.
2. `just run` — a centered overlay appears on top of the desktop with the MRU-sorted window list. Other windows remain visible behind the overlay (not minimized).
3. Confirm the app does **not** create a taskbar entry (visible only as the overlay).
4. Open and close another app while the overlay is up — the list updates live (stage 2 sorting still works).
5. Press ESC — overlay disappears and the `just run` process exits cleanly.
6. Re-run `just run` from a different focused window — the just-focused window appears at the top of the new overlay's list.

## Reference implementation pointers

- `pop-os/cosmic-launcher` `src/app.rs`:
  - Settings chain with `no_main_window(true)` + `exit_on_close(false)`
  - `init()` returning `(state, get_layer_surface(...))`
  - `SctkLayerSurfaceSettings { keyboard_interactivity: Exclusive, anchor: Anchor::TOP, namespace: "launcher", exclusive_zone: -1, ..Default::default() }`
  - `hide()` calling `destroy_layer_surface(self.window_id)`
  - `view_window(id)` routing by surface id
- libcosmic types: `cosmic::iced::platform_specific::shell::commands::layer_surface::*`, `cosmic::iced::platform_specific::runtime::wayland::layer_surface::SctkLayerSurfaceSettings`.
- Cancelled stage 3 context: `.claude/plans/abandoned/stage3-exclude-self-cancelled.md`.
