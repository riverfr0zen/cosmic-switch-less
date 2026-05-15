# User-level configuration: overlay width override

## Context

The app currently hard-codes the overlay width to `OVERLAY_WIDTH = 600.0` in `src/app.rs:23`. Different users have different screens, font sizes, and aesthetic preferences, so we want to let them override this without recompiling.

`src/config.rs` already contains inert scaffolding deriving `CosmicConfigEntry` (with a placeholder `demo: String` field), so the COSMIC-conventional path is partly set up but unused. This stage replaces the placeholder field with a real `overlay_width: f32` and wires it through `app.rs`. It also ships a small `cosmic-switch-less-reload` script so the user can pick up config changes without manually hunting the daemon PID (the daemon loads config once at startup, by design — no hot-reload).

This is the first stage to introduce real user configuration. The schema and approach established here will be the template for any future config fields.

## Approach

**Storage:** libcosmic's `cosmic_config` (already a transitive dep — no new crates). The config file lives at `~/.config/cosmic/com.github.riverfr0zen.cosmic-switch-less/v1/overlay_width` and contains a single RON-formatted `f32` value, e.g. `800.0`. RON comments (`// ...`) are allowed and preserved as long as the app only reads the file.

**Load semantics:** synchronously in `AppModel::init()`. If the config directory or the `overlay_width` file is missing, fall back silently to the built-in default (`600.0`). Malformed contents log a warning to stderr and fall back to the default. The runtime fallback is the source of truth even when the install recipe seeds a file — that way users who skip `install-local` (e.g. running from `just run`) and users who delete the file still get a working overlay.

**Config seeding:** `install-local` seeds `~/.config/cosmic/<app-id>/v1/overlay_width` on a user's machine so the file is discoverable for editing — idempotent (only writes if the file doesn't already exist) so re-running `install-local` never clobbers a user's override. The system `install` recipe (run by packagers with `rootdir`/`prefix`, typically as root with a staging dir) does **not** touch `$HOME`; downstream packagers handle user-state seeding via their own postinst hooks if they want it.

**Reload:** the app does not watch the config file. Instead, ship `scripts/cosmic-switch-less-reload`: kill the running daemon (next Alt+Tab triggers the cold-start path in `cosmic-switch-less-show`).

## Files modified

### `src/config.rs` — replace scaffolding with the real schema

- Replace `demo: String` with `pub overlay_width: f32`.
- Drop `Eq` from the derives (`f32` is not `Eq`); keep `PartialEq`, `Debug`, `Clone`, `CosmicConfigEntry`.
- Drop `Default` from the derives and add a manual `impl Default` returning `Self { overlay_width: 600.0 }` (deriving `Default` would yield `0.0`).
- Remove `#[allow(dead_code)]` — the struct is now used.
- Add a `Config::load()` associated function that wraps the `cosmic_config::Config::new(APP_ID, VERSION)` + `get_entry` dance: returns a `Config` always, logging a `eprintln!` for any per-field errors (mirrors how `cosmic-settings-daemon` handles partial loads — `get_entry` returns `Result<Config, (Vec<Error>, Config)>`, where the `Err` variant still carries a usable `Config` populated with defaults for the failed fields). The function takes `&str` for the app id so `app.rs` can pass `AppModel::APP_ID` and we don't duplicate the constant.

### `src/app.rs` — consume the config

- Delete `const OVERLAY_WIDTH: f32 = 600.0;` (`src/app.rs:23`).
- Add an `overlay_width: f32` field to `AppModel` (`src/app.rs:31`).
- In `AppModel::init()` (`src/app.rs:80`), call `Config::load(Self::APP_ID)` and store `config.overlay_width` on the model.
- Replace the two `OVERLAY_WIDTH` references with `self.overlay_width`:
  - `view_window` container width — `src/app.rs:140` (`.width(Length::Fixed(OVERLAY_WIDTH))`).
  - `summon_with_highlight` layer-surface `size_limits.max_width` — `src/app.rs:299`.

### `scripts/cosmic-switch-less-reload` — new file

Header-style comment matching `scripts/cosmic-switch-less-show` (purpose + usage). Body:

```bash
#!/usr/bin/env bash
# Reload cosmic-switch-less config by killing the daemon. The next Alt+Tab
# (via cosmic-switch-less-show) will cold-start a fresh process that reads
# the updated config.

if pid=$(pidof cosmic-switch-less) && [ -n "$pid" ]; then
    kill -TERM $pid
    exit 0
fi
# Not running: nothing to reload. Treat as success.
exit 0
```

Use `pidof` (not `pkill -x`) for the same reason as the show script — `/proc/<pid>/comm` truncates to 15 chars and `cosmic-switch-less` is 18.

### `justfile` — install the new script and seed user config

- Add `reload-script-dst := base-dir / 'bin' / (name + '-reload')` next to `show-script-dst` (`justfile:24`).
- In `install` (`justfile:70`), add: `install -Dm0755 {{ 'scripts' / (name + '-reload') }} {{ reload-script-dst }}`. **Do not touch `$HOME` from this recipe** — system install stays packaging-clean.
- In `install-local` (`justfile:65`), add:
  - `cp scripts/cosmic-switch-less-reload ~/.local/bin/`
  - `mkdir -p ~/.config/cosmic/{{ appid }}/v1`
  - A guarded write that only seeds the default if the file is absent, so re-running `install-local` doesn't clobber a user override. Something like:
    ```just
    test -e ~/.config/cosmic/{{ appid }}/v1/overlay_width || printf '// Width of the overlay in pixels (default: 600.0)\n600.0\n' > ~/.config/cosmic/{{ appid }}/v1/overlay_width
    ```
- In `uninstall` (`justfile:78`), append `{{ reload-script-dst }}` to the `rm` argument list. **Do not delete the user's config dir** — uninstalling shouldn't drop the user's settings.
- `bundle-release` (`justfile:106`) already does `cp ../../scripts/* ...` so it picks up the new script automatically — no change needed.

## Reused utilities / patterns

- `cosmic_config::Config::new(app_id, version)` + `Config::get_entry(&context)` — pattern lifted directly from `cosmic-research/cosmic-settings-daemon/cosmic-settings-daemon-config/src/lib.rs`. No need to invent anything.
- Script structure (shebang, comment header, `pidof` PID lookup, exit codes) mirrors `scripts/cosmic-switch-less-show`.

## Verification

Build and install locally:

```sh
just check          # clippy passes (pedantic)
just build-release  # release build compiles
just install-local  # installs binary + both scripts to ~/.local/bin
```

End-to-end checks:

1. **Runtime default (no config file):** delete `~/.config/cosmic/com.github.riverfr0zen.cosmic-switch-less/` if it exists. Run the daemon directly (`just run` — bypasses `install-local`'s seeding). Press Alt+Tab. Overlay should render at the existing 600 px width.
2. **install-local seeds the file:** delete the config dir again, then run `just install-local`. Confirm `~/.config/cosmic/com.github.riverfr0zen.cosmic-switch-less/v1/overlay_width` now exists with the seeded `// ...\n600.0\n` content. Press Alt+Tab via `cosmic-switch-less-show`. Overlay still renders at 600 px (same width).
3. **install-local is idempotent:** edit the seeded file to `800.0`. Run `just install-local` again. Confirm the file *still* contains `800.0` (not clobbered back to 600.0). Run `cosmic-switch-less-reload`, press Alt+Tab. Overlay is visibly wider.
4. **Comments survive read:** add another `// note ...` line at the top of the file, reload, summon. Overlay still renders at 800 px.
5. **Malformed value:** write `not-a-number` to the file, reload, summon. Overlay renders at the default 600 px and `cosmic-switch-less` stderr (visible if launched via terminal) shows a warning naming the failed field.
6. **Reload with no daemon running:** kill the daemon, then run `cosmic-switch-less-reload`. Script exits 0 without complaint.
7. **Uninstall preserves config:** run `just install` to a throwaway `rootdir` (e.g. `just rootdir=/tmp/csl-test prefix=/usr install`). Then `just rootdir=/tmp/csl-test prefix=/usr uninstall`. Confirm `~/.config/cosmic/com.github.riverfr0zen.cosmic-switch-less/` is untouched by both.

No unit tests — the project has no test suite (per `CLAUDE.md`); verification is manual.

## Out of scope

- Hot-reload via `cosmic_config` watch subscription (explicitly deferred — user chose restart-via-script).
- Any settings-UI panel for editing the value.
- Additional config fields (overlay height fraction, max list height, theme overrides, etc.) — schema is ready to grow but no other fields are added here.
- CLAUDE.md updates to the Merged stage-history list (user batches those separately). The architecture-section description of `src/config.rs` as "inert scaffolding" becomes inaccurate after this change, but per the user's preference that update is also deferred until the next CLAUDE.md batch.
