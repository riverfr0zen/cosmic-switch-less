# User-level configuration: overlay width override

## Context

The app currently hard-codes the overlay width to `OVERLAY_WIDTH = 600.0` in `src/app.rs:23`. Different users have different screens, font sizes, and aesthetic preferences, so we want to let them override this without recompiling.

`src/config.rs` already contains inert scaffolding deriving `CosmicConfigEntry` (with a placeholder `demo: String` field), so the COSMIC-conventional path is partly set up but unused. This stage replaces the placeholder field with a real `overlay_width: f32` and wires it through `app.rs`. It also ships a small `cosmic-switch-less-reload` script so the user can pick up config changes without manually hunting the daemon PID (the daemon loads config once at startup, by design — no hot-reload).

This is the first stage to introduce real user configuration. The schema and approach established here will be the template for any future config fields.

## Approach

**Storage:** libcosmic's `cosmic_config` for path/version conventions, but used in single-key mode rather than via `CosmicConfigEntry` (which would write one file per top-level field). All settings live in one RON-formatted struct at `~/.config/cosmic/com.github.riverfr0zen.cosmic-switch-less/v1/settings`. `serde` is added as a direct dep so the struct derives `Serialize`/`Deserialize`. RON comments (`// ...`) are allowed and preserved as long as the app only reads the file. `#[serde(default)]` on the struct means missing fields fall back to per-type defaults — important for forward compatibility as more fields land.

Seeded file content looks like:

```ron
// All settings for cosmic-switch-less.
// Edit values then run `cosmic-switch-less-reload` to apply.
(
    // Width of the overlay in pixels.
    overlay_width: 600.0,
)
```

**Load semantics:** synchronously in `AppModel::init()`. If the config directory or the `settings` file is missing, fall back silently to `Config::default()`. Other read/parse errors log to stderr and fall back to the default. The runtime fallback is the source of truth even when the install recipe seeds a file — users who skip `install-local` (e.g. running from `just run`) and users who delete the file still get a working overlay.

**Config seeding:** `install-local` (and the binary-release `scripts/install.sh`) seed `~/.config/cosmic/<app-id>/v1/settings` so the file is discoverable for editing — idempotent (only writes if the file doesn't already exist) so re-running install never clobbers a user's override. The system `install` recipe (run by packagers with `rootdir`/`prefix`, typically as root with a staging dir) does **not** touch `$HOME`; downstream packagers handle user-state seeding via their own postinst hooks if they want it.

**Daemon-running install safety:** `install-local` and `install.sh` stop a running daemon before `cp`-ing the new binary, because Linux refuses `ETXTBSY` when overwriting a binary that is currently being executed. The daemon comes back on the next Alt+Tab via the show script's cold-start path.

**Reload:** the app does not watch the config file. Instead, ship `scripts/cosmic-switch-less-reload`: kill the running daemon and immediately cold-restart it (`setsid -f`) so the next Alt+Tab is instant.

## Files modified

### `src/config.rs` — single-key cosmic_config storage

- Drop the `CosmicConfigEntry` derive (it writes one file per field — not what we want).
- Derive `Serialize`, `Deserialize`, plus `Debug`, `Clone`. Apply `#[serde(default)]` so missing fields use defaults when newer code reads older config files.
- Drop the auto-derived `Default` and provide a manual `impl Default` returning `Self { overlay_width: 600.0 }` (the auto-derive would yield `0.0`).
- Module-level constants: `const VERSION: u64 = 1;` and `const KEY: &str = "settings";`. The version lives in the path (`/v1/`); the key is the filename.
- `Config::load(app_id: &str) -> Self`: opens a `cosmic_config::Config::new(app_id, VERSION)` helper, then `helper.get::<Self>(KEY)`. Pattern-matches the error: `cosmic_config::Error::GetKey(_, err)` with `err.kind() == NotFound` falls through silently (expected when the user hasn't created a file); other errors are logged to stderr. Either way, returns a usable `Config`.

### `src/app.rs` — consume the config

- Delete `const OVERLAY_WIDTH: f32 = 600.0;` (`src/app.rs:23`).
- Add an `overlay_width: f32` field to `AppModel` (`src/app.rs:31`).
- In `AppModel::init()` (`src/app.rs:80`), call `crate::config::Config::load(Self::APP_ID)` and store `config.overlay_width` on the model.
- Replace the two `OVERLAY_WIDTH` references with `self.overlay_width`:
  - `view_window` container width — `src/app.rs:140` (`.width(Length::Fixed(OVERLAY_WIDTH))`).
  - `summon_with_highlight` layer-surface `size_limits.max_width` — `src/app.rs:299`.

### `Cargo.toml` — add `serde`

Add `serde = { version = "1", features = ["derive"] }` as a direct dep so `Config` can derive `Serialize`/`Deserialize` without relying on transitive re-exports.

### `scripts/cosmic-switch-less-reload` — new file

Header-style comment matching `scripts/cosmic-switch-less-show`. Body: `pidof` → `kill -TERM` → poll up to 500 ms for the daemon to exit → `setsid -f cosmic-switch-less` to cold-start a fresh process. Uses `pidof` not `pkill -x` (same reason as the show script — `/proc/<pid>/comm` truncates to 15 chars and `cosmic-switch-less` is 18).

### `justfile` — install the new script, seed user config, stop daemon before cp

- Add `reload-script-dst := base-dir / 'bin' / (name + '-reload')` next to `show-script-dst` (`justfile:24`).
- In `install` (`justfile:70`), add: `install -Dm0755 {{ 'scripts' / (name + '-reload') }} {{ reload-script-dst }}`. **Do not touch `$HOME` from this recipe** — system install stays packaging-clean.
- In `install-local` (`justfile:65`):
  - Stop a running daemon first (`pidof` + `kill -TERM`, then a bounded poll for it to exit) so `cp` doesn't hit `ETXTBSY`.
  - `cp scripts/cosmic-switch-less-reload ~/.local/bin/` alongside the existing show-script copy.
  - `mkdir -p ~/.config/cosmic/{{ appid }}/v1`.
  - A guarded `printf` (`test -e ... || printf '...' > ...`) that only seeds the default `settings` RON file if it's absent, so re-running `install-local` doesn't clobber a user override.
- In `uninstall` (`justfile:78`), append `{{ reload-script-dst }}` to the `rm` argument list. **Do not delete the user's config dir.**
- `bundle-release` (`justfile:106`) already does `cp ../../scripts/* ...` so it picks up the new script automatically — no change needed.

### `scripts/install.sh` — binary-release installer

Mirrors `install-local`: stop a running daemon before `install -Dm0755` of the binary, install the reload script alongside the show script, then seed `~/.config/cosmic/<app-id>/v1/settings` (using a `cat <<'EOF'` heredoc for readability with the multi-line RON content). When run under sudo, `chown` the seeded directory back to `SUDO_USER` so they can edit it without root. Post-install summary lists all three scripts and points at the config path.

## Reused utilities / patterns

- `cosmic_config::Config::new(app_id, version)` + `helper.get::<T>(KEY)` (the `ConfigGet` trait) — pattern lifted from `cosmic-research/cosmic-settings-daemon/src/locale.rs:37` (used for the `xkb_config` struct, which lives at `~/.config/cosmic/com.system76.CosmicComp/v1/xkb_config` as a single RON struct — the same file shape we want).
- Script structure (shebang, comment header, `pidof` PID lookup, bounded poll, exit codes) mirrors `scripts/cosmic-switch-less-show`.

## Verification

Build and install locally:

```sh
just check          # clippy passes (pedantic)
just build-release  # release build compiles
just install-local  # stops daemon, installs binary + 3 scripts, seeds config
```

End-to-end checks (manual — most need a live Wayland session):

1. **Runtime default (no config file):** delete `~/.config/cosmic/com.github.riverfr0zen.cosmic-switch-less/` if it exists. Run the daemon directly (`just run` — bypasses `install-local`'s seeding). Press Alt+Tab. Overlay renders at the existing 600 px width.
2. **install-local seeds the file:** delete the config dir again, then run `just install-local`. Confirm `~/.config/cosmic/com.github.riverfr0zen.cosmic-switch-less/v1/settings` now exists with the seeded `(overlay_width: 600.0,)` block and comments. Press Alt+Tab via `cosmic-switch-less-show`. Overlay still renders at 600 px.
3. **install-local is idempotent:** edit the seeded file's `overlay_width` to `800.0`. Run `just install-local` again. Confirm the file *still* contains `800.0`. Run `cosmic-switch-less-reload`, press Alt+Tab. Overlay is visibly wider.
4. **Comments survive read:** add an extra `// note ...` line, reload, summon. Overlay still renders at 800 px.
5. **Malformed value:** write `(overlay_width: "nope")` to the file, reload, summon. Overlay renders at the default 600 px and `cosmic-switch-less` stderr (visible if launched via terminal) shows a warning.
6. **Forward-compat (`#[serde(default)]`):** write just `()` to the file, reload, summon. Overlay renders at 600 px (the field defaults filled in).
7. **Reload restarts the daemon:** `cosmic-switch-less-reload`; verify `pidof cosmic-switch-less` shows a fresh PID different from before.
8. **Install-while-running:** start the daemon (Alt+Tab once), then `just install-local`. Confirm it stops the daemon, completes the `cp`/seed without `Text file busy`, exits 0. Press Alt+Tab — daemon cold-starts on the fresh binary.
9. **Uninstall preserves config:** run `just install` to a throwaway `rootdir` (e.g. `just rootdir=/tmp/csl-test prefix=/usr install`). Then `just rootdir=/tmp/csl-test prefix=/usr uninstall`. Confirm `~/.config/cosmic/com.github.riverfr0zen.cosmic-switch-less/` is untouched by both.

No unit tests — the project has no test suite (per `CLAUDE.md`); verification is manual.

## Out of scope

- Hot-reload via `cosmic_config` watch subscription (explicitly deferred — user chose restart-via-script).
- Any settings-UI panel for editing the value.
- Additional config fields (overlay height fraction, max list height, theme overrides, etc.) — schema is ready to grow but no other fields are added here.
- CLAUDE.md updates to the Merged stage-history list (user batches those separately). The architecture-section description of `src/config.rs` as "inert scaffolding" becomes inaccurate after this change, but per the user's preference that update is also deferred until the next CLAUDE.md batch.
