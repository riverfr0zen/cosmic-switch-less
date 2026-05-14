#!/usr/bin/env bash
# Install cosmic-switch-less from the release tarball.
#
# Usage:
#   ./install.sh                    # install into ~/.local/bin
#   PREFIX=/usr sudo ./install.sh   # system-wide install
#
# After installing, bind the launcher in COSMIC settings
# (Settings -> Input -> Keyboard -> Shortcuts):
#   Alt+Tab        -> cosmic-switch-less-show
#   Alt+Shift+Tab  -> cosmic-switch-less-show --back

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bindir="${PREFIX:-$HOME/.local}/bin"

install -Dm0755 "$here/cosmic-switch-less" "$bindir/cosmic-switch-less"
install -Dm0755 "$here/cosmic-switch-less-show" "$bindir/cosmic-switch-less-show"

echo "Installed to $bindir:"
echo "  cosmic-switch-less"
echo "  cosmic-switch-less-show"

case ":$PATH:" in
    *":$bindir:"*) ;;
    *)
        echo
        echo "warning: $bindir is not on your PATH." >&2
        echo "         The keybinding launcher will not work until it is." >&2
        ;;
esac

echo
echo "Next: bind Alt+Tab -> cosmic-switch-less-show in COSMIC keyboard shortcuts."
