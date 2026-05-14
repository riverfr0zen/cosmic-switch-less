# Plan: Autosize the switcher overlay to fit the window list

## Context

The Alt+Tab overlay renders as a layer-shell surface with a **fixed 600×400**
size; when there are more windows than fit, an inner `scrollable` handles
overflow. For an app switcher the better experience is to **size the overlay's
height to the window list** — show as many windows as possible at once — and
only fall back to scrolling once the list would exceed a fraction of the
monitor height. Width stays fixed at 600.

**Decided:** the scroll-fallback cap is **80% of the monitor height** (not a
fixed pixel constant). That requires surfacing output geometry out of the
`src/wayland.rs` listener thread (its `OutputState` is currently tracked but
never read). On multi-monitor setups we use the **smallest** output height so
the overlay always fits whichever monitor the compositor places it on.

### Why `size: None` works now when it didn't before

CLAUDE.md's Stage 4 note says "autosize via `size: None` left the surface
unmapped." Root cause: the old content is `Length::Fill` wrapping a
`Length::Fill` scrollable — under the autosize measurement (`Limits::NONE`) a
`Fill` element resolves to `max.height` (≈0/∞), so the surface had no finite
natural size. The fix (confirmed against libcosmic source and cosmic-launcher's
alt-tab mode): wrap content in libcosmic's **`autosize` widget** and use
`Length::Shrink` / `Length::Fixed` content — never `Fill`.

Validated against libcosmic `b105e9a`:
- `widget/autosize.rs` — measures child with `Limits::NONE`, adopts child size,
  emits `RequestResize` → `request_logical_window_size(bounds)` so the layer
  surface tracks content size. Re-runs every frame, so content changes
  auto-resize the surface — **no explicit `set_size` call needed**.
- `iced/widget/src/container.rs` — `container.max_height(h)` injects `max_height`
  into the `Limits` *before* measuring the child, and `Limits::resolve` for a
  `Shrink` child returns `min(content_height, max_height)`. So
  `container(scrollable(list)).max_height(H)` shrinks to content when short and
  caps at `H` (scrollable engages) when tall — even under autosize.
- cosmic-launcher's alt-tab does exactly this: `get_layer_surface { size: None,
  size_limits: …max_width(600.0) }`, content wrapped in `autosize::autosize`,
  inner column `width(Length::Fixed(600.))` + `height(Length::Shrink)`,
  `container(scrollable(...)).max_height(504)`.

## Files to modify

- `src/wayland.rs` — read output geometry, emit smallest monitor height
- `src/app.rs` — autosize the overlay, apply screen-fraction max height
- `CLAUDE.md` — add the Stage 10 history entry on completion

## Implementation steps

### 1. `src/wayland.rs` — emit monitor height

The internal tokio channel currently carries only `Vec<WindowInfo>`. Generalise
it:
```rust
enum ThreadEvent {
    Windows(Vec<WindowInfo>),
    ScreenHeight(u32),
}
```
- `WaylandState.sender` becomes `mpsc::Sender<ThreadEvent>`; `emit_window_list`
  sends `ThreadEvent::Windows(...)`.
- Add `WaylandState::emit_screen_height(&self)`: iterate `output_state.outputs()`,
  `output_state.info(&o)` for each, take `info.logical_size.map(|(_, h)| h as u32)`
  with a fallback to the current `info.modes` entry
  (`dimensions.1 / scale_factor.max(1)`), take the **`.min()`**, and
  `blocking_send(ThreadEvent::ScreenHeight(h))`.
- Implement the currently-empty `OutputHandler` callbacks (`new_output`,
  `update_output`, `output_destroyed` — wayland.rs ~153-161) to each call
  `self.emit_screen_height()`. `OutputState` is already in `registry_handlers!`
  and `delegate_output!`, so output events already flow; sctk calls `new_output`
  only after the output's `done`, so `info()` is populated.
- Add `WaylandEvent::ScreenHeight(u32)` variant (alongside `Ready` / `Windows`).
- In `subscribe()`, the bridge maps `ThreadEvent::Windows → WaylandEvent::Windows`
  and `ThreadEvent::ScreenHeight → WaylandEvent::ScreenHeight`.

### 2. `src/app.rs` — module constants

After imports, before `pub struct AppModel`:
```rust
use std::sync::LazyLock;

static AUTOSIZE_ID: LazyLock<cosmic::widget::Id> =
    LazyLock::new(|| cosmic::widget::Id::new("cosmic-app-switcher-autosize"));
const OVERLAY_WIDTH: f32 = 600.0;
const SCREEN_HEIGHT_FRACTION: f32 = 0.8;
/// Header + paddings + column spacing, subtracted from the screen-fraction so
/// the *total* overlay (not just the list) stays within the fraction.
const OVERLAY_CHROME_HEIGHT: f32 = 96.0;
/// Used until the first `ScreenHeight` event arrives.
const DEFAULT_MAX_LIST_HEIGHT: f32 = 600.0;
```

### 3. `src/app.rs` — track max list height

- `AppModel` gains `max_list_height: f32`, initialised to
  `DEFAULT_MAX_LIST_HEIGHT` in `init()`.
- `Message` gains `ScreenHeight(u32)`.
- `subscription()` map: add
  `WaylandEvent::ScreenHeight(h) => Message::ScreenHeight(h)`.
- `update()`: `Message::ScreenHeight(h) =>` set
  `self.max_list_height = ((h as f32 * SCREEN_HEIGHT_FRACTION) - OVERLAY_CHROME_HEIGHT).max(120.0)`
  (floor at 120px so a tiny/garbage value never collapses the list).

### 4. `src/app.rs` — `summon_with_highlight` (~252-261)

In the `SctkLayerSurfaceSettings`:
- `size: Some((Some(600), Some(400)))` → `size: None`
- `size_limits: Limits::NONE.min_width(1.0).min_height(1.0)` →
  `… .min_height(1.0).max_width(OVERLAY_WIDTH)`
- Keep `anchor: Anchor::empty()` (centred), `exclusive_zone: -1`, everything else
  unchanged.

### 5. `src/app.rs` — rewrite `view_window` (~98-130)

Keep the `id != self.window_id` guard and the list-building loop unchanged.
Replace the returned element:
- **List section:** wrap the scrollable in a capped container and **remove**
  `.height(Length::Fill)` from the scrollable:
  ```rust
  widget::container(
      widget::scrollable(list.spacing(space_s)).id(self.scrollable_id.clone())
  )
  .max_height(self.max_list_height)
  ```
  (no explicit height → defaults to `Shrink`).
- **Inner column:** `column::with_capacity(2)` → push `header`, push the list
  section → `.spacing(space_s)` → `.width(Length::Fixed(OVERLAY_WIDTH))`
  → `.height(Length::Shrink)`.
- **Outer styled container:** keep `.padding(space_s)` and
  `.class(cosmic::theme::Container::Background)`; drop `.width(Length::Fill)` /
  `.height(Length::Fill)` (default `Shrink`).
- **Wrap the whole thing:**
  `cosmic::widget::autosize::autosize(outer_container, AUTOSIZE_ID.clone()).into()`.

### 6. `scroll_to_highlighted` / `SyncScroll` — no change

Still needed for the scroll-fallback case (list > `max_list_height`); a no-op
when the list is short. The 50 ms deferred `SyncScroll` rationale (scrollable
`Id` not registered until after the first `view()`) is unaffected.

## Critical files / reference

- `src/wayland.rs`, `src/app.rs` — the changes above
- `cosmic-research/cosmic-launcher/src/app.rs` — reference autosize + alt-tab pattern
- libcosmic `b105e9a` `src/widget/autosize.rs`, `iced/widget/src/container.rs` — verified behaviour
- sctk 0.19.2 `output.rs` — `OutputState::outputs()` / `info()`, `OutputInfo.logical_size` / `.modes` / `.scale_factor`

## Uncertainties / fallbacks

1. **`Anchor::empty()` with autosize** — cosmic-launcher only exercises
   `Anchor::TOP` with this pattern. Centred placement *should* work (compositor
   re-centres on each size commit), but is unverified against cosmic-comp. If the
   surface fails to map or jumps while resizing, switch to `anchor: Anchor::TOP`
   (one-line change).
2. **Which monitor** — `Anchor::empty()` lets the compositor pick the output;
   we can't know which. Using the smallest output height guarantees a fit but
   slightly under-utilises a larger monitor. Acceptable for v1.
3. **`logical_size` availability** — depends on `wl_output` v4 logical events;
   cosmic-comp sends them. The mode-dimensions/scale fallback covers older cases.
4. **Resize flicker** — `CycleNext`/`CyclePrev` only restyle rows (constant row
   count → constant measured size → no resize). Resizes happen only on
   `WindowsUpdated` / `ScreenHeight`.

## Verification

1. `just check` — clippy/pedantic clean (pre-existing `mutable key type` warning
   in `emit_window_list` aside).
2. `just build-release`, then `setsid -f ./target/release/cosmic-app-switcher
   >/tmp/cosmic-app-switcher.log 2>&1`.
3. Summon with **1–2 windows** → short overlay, no scrollbar, sized to content.
4. Summon with **many windows** (open enough to exceed 80% screen height) →
   overlay caps, inner list scrolls, highlighted row scrolls into view; cycle
   past the end → highlight wraps and scroll follows.
5. Confirm the surface actually maps (the Stage 4 failure mode) — if not, apply
   fallback #1.
6. Multi-monitor (if available): overlay height respects the smaller monitor.
7. Check `/tmp/cosmic-app-switcher.log` for Wayland errors; `kill` the daemon.
