# Changelog

All notable changes to this project are listed here.

## 0.3.0

### Fixed
- [Windows floating to the top of the list (issue #1)](https://github.com/riverfr0zen/cosmic-switch-less/issues/1): the MRU order was corrupted in two distinct ways.
  - Stale `Activated` events from the previous window (arriving after focus had already moved) were being treated as fresh promotions, so the just-defocused window kept jumping above the newly focused one. MRU promotion is now gated on false→true `Activated` transitions only.
  - cosmic-comp occasionally flashes `Activated` onto a window nobody switched to (reproduces with server-side-decorated apps such as Konsole, git-cola, KeepassXC). Promotion is now debounced by 300ms; brief flashes no longer poison the order. The just-committed window is also pinned to the top of the list for 1s so the debounce lag is invisible on quick re-summon.
- Two summon/commit races:
  - Re-summoning within ~150ms of a commit showed the pre-switch order (with the current window pre-selected) because the compositor's state round-trip hadn't landed yet. The committed window is now moved to the top locally on commit.
  - A very fast tap could release the binding's modifier before the overlay gained keyboard focus, leaving the modifier snapshot empty and the overlay stuck open until Enter/Escape.

### Added
- `instant_commit_no_modifiers` config option (default off): when the modifier snapshot is empty, commit immediately instead of waiting for Enter. Addresses the fast-tap race above; distinct from the 0.2.1 fix for issue #3 (where the snapshot held the wrong modifiers rather than being empty).

Thanks to @swinglejohn for the fixes ([PR #5](https://github.com/riverfr0zen/cosmic-switch-less/pull/5)).


## 0.2.1

### Fixed
- [UX issue due to hardcoded keybinding](https://github.com/riverfr0zen/cosmic-switch-less/issues/3): the overlay used to commit only when the **Alt** key was released, leaving non-Alt bindings (e.g. `Shift+Down`) stuck open until Enter was pressed. The release-to-commit gesture now adapts to whichever modifiers (Alt, Shift, Ctrl, Super) are held when the overlay first gains keyboard focus — releasing any of them commits. Bindings with no modifier (e.g. a plain `F12`) still work; Enter remains the commit path in that case.


## 0.2.0

### Added
- User config file (RON) support
- User-configurable settings:
  - Overlay width override
  - List item icon size
  - List item font size
- Config documentation.
- Arch packaging support in the `justfile` bundle target, plus a license file.

## 0.1.0

Initial release introducing core app switching functionality.
