# Changelog

All notable changes to this project are listed here.

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
