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

# Stop the running daemon, if any, so install can overwrite the binary
# (Linux refuses with ETXTBSY otherwise). Next Alt+Tab cold-starts the fresh
# binary via cosmic-switch-less-show.
if pid=$(pidof cosmic-switch-less); then
    kill -TERM $pid
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        sleep 0.05
        pidof cosmic-switch-less >/dev/null || break
    done
fi

install -Dm0755 "$here/cosmic-switch-less" "$bindir/cosmic-switch-less"
install -Dm0755 "$here/cosmic-switch-less-show" "$bindir/cosmic-switch-less-show"
install -Dm0755 "$here/cosmic-switch-less-reload" "$bindir/cosmic-switch-less-reload"

# Seed the user config so the file is discoverable for editing. Skip if it
# already exists so we don't clobber an override the user has set. Use the
# invoking user's $HOME when running under sudo, otherwise $HOME.
target_home="${SUDO_USER:+$(getent passwd "$SUDO_USER" | cut -d: -f6)}"
target_home="${target_home:-$HOME}"
appid="com.github.riverfr0zen.cosmic-switch-less"
configdir="$target_home/.config/cosmic/$appid/v1"
configfile="$configdir/settings"
if [ ! -e "$configfile" ]; then
    mkdir -p "$configdir"
    cat > "$configfile" <<'EOF'
// All settings for cosmic-switch-less.
// Edit values then run `cosmic-switch-less-reload` to apply.
(
    // Width of the overlay in pixels.
    overlay_width: 600.0,
)
EOF
    # When running under sudo, hand ownership back to the invoking user so
    # they can edit the file without root.
    if [ -n "${SUDO_USER:-}" ]; then
        chown -R "$SUDO_USER" "$target_home/.config/cosmic/$appid"
    fi
fi

echo "Installed to $bindir:"
echo "  cosmic-switch-less"
echo "  cosmic-switch-less-show"
echo "  cosmic-switch-less-reload"
echo
echo "Config: $configfile"
echo "  Edit this file then run cosmic-switch-less-reload to apply."

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
