#!/bin/sh
# Installs scrpad for the current user: the binary, a menu entry and the icon. Wayland
# compositors find the window icon through the menu entry, which matches the app id "scrpad".
#
#   ./install.sh               install into ~/.local (or into $PREFIX)
#   ./install.sh --uninstall   remove those files again; notes in ~/.config/scrpad are kept
set -eu
prefix=${PREFIX:-$HOME/.local}
here=$(cd "$(dirname "$0")" && pwd)
desktop=share/applications/scrpad.desktop
icons="share/icons/hicolor/scalable/apps/scrpad.svg share/icons/hicolor/256x256/apps/scrpad.png"

if [ "${1:-}" = "--uninstall" ]; then
  for f in bin/scrpad $desktop $icons; do rm -f "$prefix/$f"; done
  echo "Removed scrpad from $prefix. Your notes in ~/.config/scrpad were kept."
else
  install -Dm755 "$here/scrpad" "$prefix/bin/scrpad"
  for f in $icons; do install -Dm644 "$here/$f" "$prefix/$f"; done
  # An absolute Exec works even when $prefix/bin is not on PATH.
  mkdir -p "$prefix/share/applications"
  sed "s|^Exec=.*|Exec=$prefix/bin/scrpad|" "$here/$desktop" > "$prefix/$desktop"
  echo "Installed scrpad into $prefix."
fi
command -v update-desktop-database >/dev/null && update-desktop-database -q "$prefix/share/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" || true
