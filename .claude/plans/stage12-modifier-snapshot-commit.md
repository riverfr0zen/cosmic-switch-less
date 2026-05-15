# Plan: generalize commit-on-release to any held modifier

## Context

The overlay currently treats **Alt-release** as the commit gesture
(`src/app.rs:196-199` — `KeyEvent::KeyReleased { key: Named::Alt, .. } => Confirm`).
This works for `Alt+Tab` but breaks the UX for any user who binds the
switcher to a non-Alt shortcut (e.g. `Shift+Down`): the overlay opens but
release-to-commit does nothing, leaving Enter as the only way out.

Investigation showed the daemon has no information about which modifier
fired the binding — the launcher script signals via `SIGUSR1`/`SIGUSR2`,
which carry no payload. The fix has to derive the answer at runtime by
watching what the user is actually holding.

**Adopted UX (Niri-style):** snapshot whichever modifiers are held at
the moment the overlay first gains keyboard focus, then commit when any
of them is released. Zero config, works for Alt+Tab, Shift+Down,
Ctrl+anything, Super+Grave, etc. If no modifier is held at summon time
(e.g. the binding is just `F12`), no auto-commit — Enter remains the
only commit path. Mirrors how `cosmic-launcher` solves the same shape of
problem (it watches Alt **or** Super release at
`cosmic-research/cosmic-launcher/src/app.rs:1098-1101`); we generalize
to all four modifier flags.

## What changes

All edits in **`src/app.rs`** — no other files affected.

### 1. New import

```rust
use cosmic::iced::keyboard::Modifiers;
```

### 2. `AppModel` gains two fields

```rust
/// Modifiers held at the moment the overlay first received keyboard
/// focus after a summon. `None` while waiting for the first
/// `ModifiersChanged` event after summon (or while hidden).
/// `Some(Modifiers::empty())` after a no-modifier summon → no
/// auto-commit, Enter only.
summon_modifiers: Option<Modifiers>,

/// True between `summon_with_highlight` and the next
/// `ModifiersChanged` event — gates the snapshot capture so it only
/// happens once per summon.
awaiting_snapshot: bool,
```

`init()` sets both: `summon_modifiers: None, awaiting_snapshot: false`.

### 3. New `Message` variant

```rust
/// Live modifier state changed. Used both to capture the snapshot
/// (first event after a summon) and to detect release of any
/// snapshot-time modifier.
ModifiersChanged(Modifiers),
```

### 4. `subscription()` rewrite (around lines 181-206)

Replace the hard-coded `KeyReleased { Alt }` branch and add a
`ModifiersChanged` branch:

```rust
listen_raw(|event, _status, _id| match event {
    Event::Keyboard(KeyEvent::KeyPressed {
        key: Key::Named(Named::Tab),
        modifiers,
        ..
    }) => Some(if modifiers.shift() { Message::CyclePrev } else { Message::CycleNext }),
    Event::Keyboard(KeyEvent::KeyPressed {
        key: Key::Named(Named::Enter), ..
    }) => Some(Message::Confirm),
    Event::Keyboard(KeyEvent::KeyPressed {
        key: Key::Named(Named::Escape), ..
    }) => Some(Message::Cancel),
    Event::Keyboard(KeyEvent::ModifiersChanged(mods)) => {
        Some(Message::ModifiersChanged(mods))
    }
    _ => None,
})
```

The old `KeyReleased { Alt }` branch is removed entirely.

### 5. `update()` additions

New arm for `Message::ModifiersChanged`:

```rust
Message::ModifiersChanged(mods) => {
    if self.awaiting_snapshot {
        // Snapshot-on-summon: the Wayland protocol guarantees a
        // wl_keyboard.modifiers event right after wl_keyboard.enter,
        // so this fires reliably once the surface gains focus.
        self.summon_modifiers = Some(mods);
        self.awaiting_snapshot = false;
    } else if let Some(snap) = self.summon_modifiers {
        if !snap.is_empty()
            && ((snap.shift()   && !mods.shift())
             || (snap.alt()     && !mods.alt())
             || (snap.control() && !mods.control())
             || (snap.logo()    && !mods.logo()))
        {
            return self.update(Message::Confirm);
        }
    }
}
```

Inside `Message::Confirm` and `Message::Cancel` (around lines 226-238),
clear the snapshot alongside `self.shown = false;`:

```rust
self.shown = false;
self.summon_modifiers = None;
self.awaiting_snapshot = false;
```

### 6. `summon_with_highlight()` (around line 290)

At the top, alongside `self.shown = true;`, set
`self.awaiting_snapshot = true;`. **Only set to true here** — this is
the single fresh-summon path. Re-summon during cycling
(`CycleNext`/`CyclePrev` while `self.shown`) does *not* re-create the
surface, does *not* touch `awaiting_snapshot`, and so does *not*
re-snapshot — preserving the user's original modifier under their hand.

## Critical files

- `/home/irf/htdocs/cosmic-switch-less/src/app.rs` — all edits
- `/home/irf/htdocs/cosmic-switch-less/cosmic-research/cosmic-launcher/src/app.rs` — reference for the existing Alt/Super-release pattern

## Verification

`just check` (clippy/pedantic clean) and `just build-release` first.

Then manually, with `cosmic-switch-less-reload` between config-irrelevant tests:

1. **Alt+Tab regression** — default binding. Hold Alt, tap Tab, release Alt → overlay commits and hides. No behavioral change vs. today.
2. **Shift+Down (the bug)** — rebind to `Shift+Down` in cosmic-comp. Hold Shift, press Down → overlay opens with previous window highlighted. Release Shift → overlay commits.
3. **Ctrl + something / Super + something** — verify the control and logo branches of the release check.
4. **No-modifier binding** — bind to a plain `F12`. Press F12 → overlay opens. Release F12, nothing else → overlay stays open. Press Enter → commits. Press Esc → cancels.
5. **Re-summon during cycling** — hold Alt, tap Tab three times (cosmic-comp re-fires the binding, sending three SIGUSR1s). Highlight advances each time. Release Alt → commits the last-highlighted window. Confirms the snapshot is preserved across re-summon.
6. **Mixed modifier hold** — hold Alt, summon, then *additionally* press Shift while still holding Alt. Release Shift first → overlay should commit (Shift was *not* in the snapshot, but Alt still is — wait, this needs care, see "open question" below).

## Open question / risk

**The mixed-modifier case (test 6) deserves explicit thought.** The
snapshot is fixed at summon time. If the user pressed `Alt`, summoned,
then pressed `Shift`, the snapshot is `{Alt}`. Releasing `Shift` does
*not* drop a snapshot modifier (snapshot didn't include Shift), so the
overlay stays up. Releasing `Alt` drops `Alt` from the snapshot →
commit. **This is correct Niri-style behavior** and matches user
intent: "release the modifier you originally pressed". Documented here
in case it surprises during testing.

**Layer-event focus reliability.** The plan relies on the Wayland
protocol guarantee that `wl_keyboard.modifiers` follows `wl_keyboard.enter`
on focus. If during testing step (1) the snapshot fails to capture
(e.g. iced dedupes identical-value `ModifiersChanged` events and the
user's modifier state was already known before focus arrived), fall
back to also listening for `LayerEvent::Focused` and snapshotting
`current_modifiers` there. Would require adding a `current_modifiers:
Modifiers` mirror field and the `LayerEvent::Focused` branch — see the
Plan agent's notes for the import path.
