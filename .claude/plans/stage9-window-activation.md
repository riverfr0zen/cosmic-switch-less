# Plan: Window activation (actually switch windows)

## Context

The switcher currently lists/sorts windows (MRU), shows a layer-shell overlay,
and cycles a `highlighted_index` via Tab/Shift+Tab and SIGUSR1/SIGUSR2 — but it
has **no ability to actually focus the selected window**. Alt-release and Escape
both just hide the overlay. This stage adds real window activation.

The hard part: the Wayland listener thread (`src/wayland.rs`) runs a `calloop`
event loop and currently has a **one-way** channel (wayland → app). Activating a
window requires the app's `update()` to send a command **back** into that thread.

**Decided UX:** Alt-release **and** Enter commit (activate highlighted window,
then hide). Escape cancels (hide, no switch). No mouse-click activation.

Reference implementation: `cosmic-research/pop-launcher/plugins/src/cosmic_toplevel/toplevel_handler.rs`
— implements `ToplevelManagerHandler` + `SeatState`, activates via
`manager.activate(&ZcosmicToplevelHandleV1, &WlSeat)` per seat, and uses a
`calloop::channel` for the reverse command path.

## Files to modify

- `src/wayland.rs` — reverse channel, manager/seat state, activation logic
- `src/app.rs` — store the command handle, split Hide into Confirm/Cancel
- `Cargo.toml` — no new deps expected (`calloop`, `cctk`, `cosmic-protocols` already present)

## Implementation steps

### 1. `WindowInfo` gets a stable identifier (`src/wayland.rs`)
- Add `pub identifier: String` to `WindowInfo` (lines 21-25).
- Populate from `t.identifier.clone()` in the `emit_window_list()` map closure
  (lines 81-89). This is the cctk-stable id already used for MRU keying;
  `title`/`app_id` are not unique.

### 2. Reverse-channel command type + Debug-safe sender newtype (`src/wayland.rs`)
```rust
pub enum WaylandRequest { Activate(String) } // identifier; enum leaves room for Close etc.

#[derive(Clone)]
pub struct WaylandHandle(calloop::channel::Sender<WaylandRequest>);

impl std::fmt::Debug for WaylandHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WaylandHandle")
    }
}
impl WaylandHandle {
    pub fn activate(&self, identifier: String) {
        let _ = self.0.send(WaylandRequest::Activate(identifier));
    }
}
```
The manual `Debug` impl is required because the iced `Message` enum derives
`Debug` and `calloop::channel::Sender` does not implement it. `Sender` is
`Clone`, satisfying `Message`'s `Clone` derive.

### 3. Add manager + seat state to `WaylandState` (`src/wayland.rs`)
- New fields: `toplevel_manager_state: ToplevelManagerState`, `seat_state: SeatState`.
- Construct in `run_wayland_thread` (lines 189-196): `ToplevelManagerState::new(&registry_state, &qh)`
  and `SeatState::new(&globals, &qh)`. **Build these before `registry_state` is
  moved into the struct literal** (reorder/bind locals — they borrow `&registry_state`/`&globals`).
- Note: `ToplevelManagerState::new` unwraps internally; on COSMIC the global is
  advertised so this is fine. If it ever needs to run on non-COSMIC, switch to a
  `try_new` + skip — out of scope here.

### 4. Implement the new handler traits (`src/wayland.rs`)
- `impl ToplevelManagerHandler`: `toplevel_manager_state()` returns the field;
  `capabilities(...)` empty body.
- `impl SeatHandler`: `seat_state()` returns the field; `new_seat` /
  `new_capability` / `remove_capability` / `remove_seat` all empty.
- Activation helper:
```rust
fn activate_by_identifier(&self, identifier: &str) {
    let Some(info) = self.toplevel_info_state.toplevels()
        .find(|t| t.identifier == identifier) else { return; };
    let Some(cosmic) = info.cosmic_toplevel.as_ref() else { return; };
    for seat in self.seat_state.seats() {
        self.toplevel_manager_state.manager.activate(cosmic, &seat);
    }
}
```
Scanning `.toplevels()` (vs `info(handle)`) because the app sends only the
identifier string.

### 5. Delegate macros + registry handlers (`src/wayland.rs`)
- After the existing delegate block (lines 165-168): add
  `cctk::delegate_toplevel_manager!(WaylandState);` and
  `cctk::sctk::delegate_seat!(WaylandState);`.
- Change `registry_handlers!(OutputState)` → `registry_handlers!(OutputState, SeatState)`.

### 6. Wire the calloop channel into the event loop (`src/wayland.rs`)
- `run_wayland_thread` takes a second arg: `calloop_rx: calloop::channel::Channel<WaylandRequest>`.
- Clone the connection **before** `WaylandSource::new(conn, event_queue)` consumes
  it: `let flush_conn = conn.clone();`.
- Insert the channel as a calloop source:
```rust
event_loop.handle().insert_source(calloop_rx, move |event, _, state| {
    if let calloop::channel::Event::Msg(WaylandRequest::Activate(id)) = event {
        state.activate_by_identifier(&id);
        let _ = flush_conn.flush();
    }
}).expect("insert calloop request channel");
```
The explicit `flush()` ensures the activate request reaches the compositor
immediately — the app hides right after committing, so we can't rely on a later
dispatch cycle to flush.

### 7. Subscription delivers the handle once at startup (`src/wayland.rs`)
```rust
#[derive(Clone, Debug)]
pub enum WaylandEvent {
    Ready(WaylandHandle),
    Windows(Vec<WindowInfo>),
}
```
- `subscribe()` returns `Subscription<WaylandEvent>`.
- In the `stream::channel` closure (lines 218-231):
  - `let (calloop_tx, calloop_rx) = calloop::channel::channel::<WaylandRequest>();`
  - send `WaylandEvent::Ready(WaylandHandle(calloop_tx))` first
  - `std::thread::spawn(move || run_wayland_thread(tx, calloop_rx));`
  - loop body sends `WaylandEvent::Windows(windows)`
- The internal tokio `mpsc` (thread → closure) still carries `Vec<WindowInfo>`;
  only the iced-facing type changes.

### 8. App side: store handle, handle Ready (`src/app.rs`)
- `AppModel` gains `wayland: Option<crate::wayland::WaylandHandle>`, init `None`
  (lines 71-79).
- `Message`: add `WaylandReady(crate::wayland::WaylandHandle)`; keep
  `WindowsUpdated(Vec<WindowInfo>)`. The Step 2 manual `Debug` impl makes the
  derive compile.
- `subscription()` (line 127): map `WaylandEvent::Ready → Message::WaylandReady`,
  `WaylandEvent::Windows → Message::WindowsUpdated`.
- `update()`: `Message::WaylandReady(h) => self.wayland = Some(h)`.
- Activation helper:
```rust
fn activate_highlighted(&self) {
    if let (Some(h), Some(w)) =
        (self.wayland.as_ref(), self.windows.get(self.highlighted_index))
    {
        h.activate(w.identifier.clone());
    }
}
```

### 9. App side: split Hide into Confirm / Cancel (`src/app.rs`)
Currently the `listen_raw` matcher (lines 139-148) maps **both** Escape and
Alt-release to `Message::Hide`. Split them:
- `KeyReleased { Alt }` → `Message::Confirm`
- `KeyPressed { Enter }` → `Message::Confirm`
- `KeyPressed { Escape }` → `Message::Cancel`
- `update()`: `Message::Cancel` = the existing Hide body (`shown=false`,
  `destroy_layer_surface`). `Message::Confirm` = `self.activate_highlighted();`
  then the same hide body. Remove the now-unused `Hide` variant.
- `summon_with_highlight` and the `CycleNext`/`CyclePrev` handlers are unchanged.

## Sequencing

Data types (1-2) → wayland capability (3-5) → reverse channel + subscription
type (6-7) → app side (8-9). The subscription item type change in Step 7 breaks
`app.rs` until 8-9 land, so do 7+8+9 in one pass before building.

## Verification

1. `just check` — clippy/pedantic clean.
2. `just run` under a COSMIC session.
3. Summon via Alt+Tab (or `kill -USR1 <pid>`), cycle the highlight with Tab /
   Shift+Tab.
4. **Alt-release** → highlighted window is focused and overlay disappears.
5. Re-summon, cycle, press **Enter** → highlighted window focused, overlay hides.
6. Re-summon, press **Escape** → overlay hides, focus unchanged (no switch).
7. Summon via `kill -USR1` (no Alt held) → Enter commits, Escape cancels.
8. Confirm focusing works across workspaces and that the just-activated window
   moves to MRU top on the next summon.
