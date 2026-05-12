# Stage 5 — show application icons next to window entries

## Context

The switcher currently renders each window as a single line of text:
`format!("{} — {}", w.title, w.app_id)` (`src/app.rs:200`, `src/app.rs:266`).
The built-in COSMIC alt-tab and launcher both display the application's
freedesktop icon to the left of the title, which makes scanning the list far
faster. This stage adds the same icon to each row.

The canonical resolution path is exactly what `pop-launcher`'s `cosmic_toplevel`
plugin does (it's the backend behind the shipping COSMIC alt-tab):

1. At startup, parse all `.desktop` files under `fde::default_paths()` once into a
   `Vec<DesktopEntry>` and cache it.
2. For each toplevel `app_id`, look up the entry via
   `fde::find_app_by_id(&entries, fde::unicase::Ascii::new(app_id))`.
3. If not found, fall back to `fde::DesktopEntry::from_appid(app_id.to_string())`
   (a synthetic entry constructed from the app_id alone).
4. Read the `Icon=` field via `entry.icon()`. If absent, use the freedesktop
   generic name `"application-x-executable"`.
5. Render with `cosmic::widget::icon::from_name(name).size(N)`.

Reference (read-only): `pop-launcher/plugins/src/cosmic_toplevel/mod.rs:127-205`.

## Dependency

Add to `Cargo.toml` under `[dependencies]`:

```toml
freedesktop-desktop-entry = "0.7"
```

(Same major version pop-launcher uses — `0.7.19`.)

## Code changes

All changes are in `src/app.rs` plus `Cargo.toml`. `src/wayland.rs` is unchanged
— `WindowInfo { title, app_id }` already carries everything we need; icon
resolution belongs on the UI side, not in the wayland listener thread.

### 1. Cache desktop entries in `AppModel`

Add two fields to the struct (`src/app.rs:26`):

```rust
locales: Vec<String>,
desktop_entries: Vec<freedesktop_desktop_entry::DesktopEntry>,
```

Populate them in `init()` (around `src/app.rs:111`) before constructing
`AppModel`:

```rust
use freedesktop_desktop_entry as fde;

let locales = fde::get_languages_from_env();
let desktop_entries = fde::Iter::new(fde::default_paths())
    .filter_map(|path| fde::DesktopEntry::from_path(path, Some(&locales)).ok())
    .collect::<Vec<_>>();
```

One-shot at startup; the overlay is transient so we don't need to invalidate on
.desktop file changes (pop-launcher doesn't either).

### 2. Add an icon-name helper on `AppModel`

In the `impl AppModel { ... }` block at `src/app.rs:368`:

```rust
fn icon_name_for(&self, app_id: &str) -> String {
    let key = fde::unicase::Ascii::new(app_id);
    let entry = fde::find_app_by_id(&self.desktop_entries, key)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| fde::DesktopEntry::from_appid(app_id.to_string()).to_owned());
    entry
        .icon()
        .map(str::to_owned)
        .unwrap_or_else(|| "application-x-executable".to_owned())
}
```

This mirrors `pop-launcher/plugins/src/cosmic_toplevel/mod.rs:195-205` exactly.

### 3. Render icon + text rows

Replace the single `widget::text(...)` per window in **both** views:

- `view_window()` — the actual layer-shell overlay (`src/app.rs:265-267`)
- `view()` Page1 — the windowed/dev path (`src/app.rs:198-203`)

with a row like:

```rust
let icon_name = self.icon_name_for(&w.app_id);
let row = widget::row::with_capacity(2)
    .push(widget::icon::from_name(icon_name).size(24))
    .push(widget::text(format!("{} — {}", w.title, w.app_id)))
    .spacing(space_s)
    .align_y(Alignment::Center);
list = list.push(row);
```

Icon size 24 — `cosmic-launcher` uses 32 for its result list, but a switcher
row is denser; 24 keeps the row height close to the current single-line layout.
Trivial to tweak later.

## Files modified

- `Cargo.toml` — add `freedesktop-desktop-entry = "0.7"`
- `src/app.rs` — two new fields on `AppModel`, init-time entry cache, helper
  method, row layout in both view sites

## Verification

1. `just check` — clippy passes.
2. `just run` — overlay appears with at least two windows visible. Each row
   shows an icon to the left of the existing `title — app_id` text.
3. Spot-check that well-known apps (terminal, file manager, browser) render
   their real icon, and that something with no .desktop entry (e.g. a process
   launched ad-hoc) falls back to the generic `application-x-executable` icon
   rather than blanking the row.
4. ESC still dismisses (no regression to the layer-shell input path).
