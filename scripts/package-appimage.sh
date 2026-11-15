#!/usr/bin/env bash
# Package a Linux release build as an AppImage:
#   scripts/package-appimage.sh VERSION [BINARY] [OUT_DIR]
# BINARY defaults to the release build and OUT_DIR to dist/. appimagetool is
# taken from $APPIMAGETOOL or PATH, else downloaded once into $CACHE_DIR.
# Build on an old distro (CI uses Ubuntu 22.04): the AppImage runs only where
# glibc is at least as new as the build machine's.
set -euo pipefail

VERSION="${1:?usage: package-appimage.sh VERSION [BINARY] [OUT_DIR]}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BINARY="${2:-$ROOT/crates/bobble-gpui/target/release/bobble-gpui}"
OUT_DIR="${3:-$ROOT/dist}"
CACHE_DIR="${CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/bobble-packaging}"
APP_ID="io.github.fr_anz.bobble"
ARCH="$(uname -m)"

[[ -x "$BINARY" ]] || { echo "No executable at $BINARY" >&2; exit 1; }

tool="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
if [[ -z "$tool" ]]; then
  tool="$CACHE_DIR/appimagetool-$ARCH.AppImage"
  if [[ ! -x "$tool" ]]; then
    mkdir -p "$CACHE_DIR"
    curl -fsSL -o "$tool.part" \
      "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$ARCH.AppImage"
    chmod +x "$tool.part"
    mv "$tool.part" "$tool"
  fi
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
app="$work/Bobble.AppDir"

install -Dm755 "$BINARY" "$app/usr/bin/bobble"
install -Dm644 "$ROOT/crates/bobble-gpui/assets/app-icon.png" \
  "$app/usr/share/icons/hicolor/1024x1024/apps/$APP_ID.png"
cp "$ROOT/crates/bobble-gpui/assets/app-icon.png" "$app/$APP_ID.png"
ln -s "$APP_ID.png" "$app/.DirIcon"

cat > "$app/$APP_ID.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Bobble
Comment=Native desktop app for coding agents
Exec=bobble %U
Icon=$APP_ID
Terminal=false
Categories=Development;IDE;
StartupWMClass=$APP_ID
X-AppImage-Version=$VERSION
EOF
install -Dm644 "$app/$APP_ID.desktop" "$app/usr/share/applications/$APP_ID.desktop"

# The binary runs as the AppImage's process, so the updater sees $APPIMAGE.
cat > "$app/AppRun" <<'EOF'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/bobble" "$@"
EOF
chmod +x "$app/AppRun"

mkdir -p "$OUT_DIR"
out="$OUT_DIR/Bobble-$VERSION-$ARCH.AppImage"
# Extract-and-run works without FUSE, as on CI runners and in containers.
APPIMAGE_EXTRACT_AND_RUN=1 ARCH="$ARCH" "$tool" --no-appstream "$app" "$out" >&2
echo "$out"
