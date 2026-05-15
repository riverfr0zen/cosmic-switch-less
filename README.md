# Cosmic Switch Less

An alternative app switcher for COSMIC Desktop that only shows windows from the *current* workspace.

Rationale: As described in [Github #1777](https://github.com/pop-os/cosmic-epoch/issues/1777), the app launcher shipping with COSMIC provides switching functionality but shows windows from across all workspaces. This becomes unwieldy when many apps are running on the desktop. 

**AI Disclosure:** This app was developed with Claude Code through a series of planned iterations. See [AI Usage Notes](#ai-usage-notes) for more details.

# Quick Install from binary

A tarball is provided in [Releases](https://github.com/riverfr0zen/cosmic-switch-less/releases) with a pre-compiled binary for `x86_64`. For other platforms, or if you are getting glibc errors, you will have to [build](#building).

## Install the binary and the signalling script

1. Download the latest tarball from [Releases](https://github.com/riverfr0zen/cosmic-switch-less/releases)
2. Unpack and access the `cosmic-switch-less-<version>` folder
3. Run `install.sh` -OR- copy the following files into your PATH (e.g. `~/.local/bin/`):
    - `cosmic-switch-less`
    - `cosmic-switch-less-show`
    - `cosmic-switch-less-reload`

## Add keybindings in COSMIC Settings

1. Open *COSMIC Settings* on your desktop
2. In *COSMIC Settings → Input → Keyboard → Shortcuts → Custom shortcuts*, add the primary binding:
    - For example: `Alt+Tab` → `cosmic-switch-less-show`
3. Add a second custom binding for the backward-cycle:
    - For example: `Alt+Shift+Tab` → `cosmic-switch-less-show --back`
4. (Optional) Add `cosmic-switch-less` to autostart so the daemon is always alive. Otherwise the wrapper cold-starts it on first press, adding ~50–500 ms of latency.
5. Press `Alt+Tab` — The switcher overlay should appear. The previously-focused window should be highlighted. Keep Alt held and tap Tab repeatedly to walk further back through MRU; tap Shift+Tab to walk forward. Release Alt to hide.
    - As mentioned in step 3 above, there may be some latency if `cosmic-switch-less` was not already running

## Configuration

As of release `0.2.0`, users can configure some settings.

If you ran `install.sh` from the tarball, the configuration file will be at:
`~/.config/cosmic/com.github.riverfr0zen.cosmic-switch-less/settings`

If you did not, then copy the `settings` file in the tarball to that location.

Modify the `settings` as desired, then run `cosmic-switch-less-reload` to refresh the daemon.


# How it works

`cosmic-switch-less` runs as a background daemon — it has no visible UI at idle. It listens for two signals:

- `SIGUSR1` — if hidden, show the overlay with the **previous window** (next in MRU order) highlighted; if shown, cycle the highlight forward.
- `SIGUSR2` — if hidden, show with the **least-recent window** highlighted; if shown, cycle backward.

The shipped wrapper `cosmic-switch-less-show` sends the right signal: bare invocation → `SIGUSR1`, with `--back` → `SIGUSR2`. Wayland does not let a client register global hotkeys, so you bind the wrapper to a key combo in cosmic-comp. cosmic-comp re-fires the same shortcut on every press while held, which is exactly what makes Tab cycling work: each re-trigger advances the highlight by one.

While the overlay is up:

- Releasing `Alt` or pressing `Enter` commits the selection: the highlighted window is activated and the overlay hides.
- Pressing `Esc` cancels: the overlay hides without switching.
- Either way the daemon keeps running and can be re-summoned.
- In-app `Tab` / `Shift+Tab` also cycle the highlight (handy when the overlay was summoned from a terminal rather than a cosmic-comp binding).

# AI Usage Notes

The app was developed with Claude Code through a series of planned iterations.

My original plan was to use this project as a way to learn [libcosmic app development](https://github.com/pop-os/libcosmic). However, as I dug in, it quickly became apparent that developing an app switcher would cover much more than a regular desktop application. It would involve a deeper review of the compositor, its protocols, and how it works with Wayland.

These aren't topics I'm familiar with. Furthermore, [cosmic-comp](https://github.com/pop-os/cosmic-comp) and [cosmic-protocols](https://github.com/pop-os/cosmic-protocols) are still in rapid iteration, and documentation is minimal. All of this meant that, even as a seasoned developer with a few Rust projects under my belt, it would take more time than I had on my hands.

As someone who's been developing software for more than 25 years, I am of course concerned about and wary of AI slop. My hope here is that process and oversight will minimize it (though of course I may not catch everything).

Each iteration went through a planning process and was developed on a separate branch. All plans are available for review under [.claude/plans](.claude/plans). An [iteration history](CLAUDE.md#stage-history) is also kept.

# Building

**NOTE:** This project was originally generated from the [COSMIC application template](https://github.com/pop-os/cosmic-app-template) using `cargo generate gh:pop-os/cosmic-app-template`. See [autogenerated README](README.autogenerated.md) for basic dev commands and info.

**Pre-Reqs:** Before building, you will probably have to install [libcosmic dev dependencies](https://github.com/pop-os/libcosmic/#dependencies).

Building:

```sh
just build-release   # release binary at ./target/release/cosmic-switch-less
just check           # clippy with pedantic warnings
```

# Manual testing without installing

```sh
# 1. Start the daemon (invisible) — leave this running.
setsid -f ./target/release/cosmic-switch-less >/tmp/cosmic-switch-less.log 2>&1

# 2. Summon the overlay (forward). Re-run to cycle forward while shown.
./scripts/cosmic-switch-less-show

# 3. Cycle backward (or summon backward when hidden).
./scripts/cosmic-switch-less-show --back

# 4. Inside the overlay:
#    - in-app Tab / Shift+Tab        → cycle forward / backward
#    - release Alt OR press Enter    → activate highlighted window, then hide
#    - press Esc                     → hide without switching

# 5. Stop the daemon when done.
kill $(pidof cosmic-switch-less)

# Optional: tail the log if something looks off.
tail -f /tmp/cosmic-switch-less.log
```

# End-user experience from build (install + add keybindings in cosmic-comp)

1. `just install-local` — installs `cosmic-switch-less` and `cosmic-switch-less-show` to `~/.local/bin/`.
2. Add keybindings: Follow the instructions in [Add keybindings in COSMIC Settings](#add-keybindings-in-cosmic-settings)


## Edge cases worth poking at

- Re-summon after hide → daemon stays alive, highlight resets to the previous-window slot (MRU index 1).
- Cycle past the visible window → list scrolls so the highlight stays in view.
- Open/close apps while the daemon is hidden → next summon reflects the new window list (the Wayland subscription keeps running at idle).
- Single window or zero windows → cycling is a no-op (no panic).
