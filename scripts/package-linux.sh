#!/usr/bin/env bash
# Packages a `flutter build linux --release` bundle as:
#   dist/omnidrop-linux-x64.tar.gz   portable bundle + install.sh (any distribution)
#   dist/omnidrop-linux-x64.deb      Debian / Ubuntu package
#   dist/PKGBUILD                    Arch Linux recipe for the tarball (sha256 filled in)
#
#   scripts/package-linux.sh <version> <bundle-dir> <dist-dir>
set -euo pipefail
VERSION="${1:?version}"
BUNDLE="$(cd "${2:?bundle dir}" && pwd)"
DIST="${3:?dist dir}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PKG="$ROOT/linux/packaging"
mkdir -p "$DIST"
DIST="$(cd "$DIST" && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

test -f "$BUNDLE/omnidrop" || { echo "missing $BUNDLE/omnidrop" >&2; exit 1; }
test -f "$BUNDLE/lib/libomnidrop_core.so" || { echo "missing libomnidrop_core.so in bundle" >&2; exit 1; }

# ---------------------------------------------------------------- tar.gz
TAR_ROOT="$WORK/omnidrop"
mkdir -p "$TAR_ROOT/share"
cp -a "$BUNDLE/." "$TAR_ROOT/"
cp -a "$PKG/com.eroideches.omnidrop.desktop" "$PKG/icons" "$PKG/polkit" "$TAR_ROOT/share/"
cp "$ROOT/LICENSE" "$TAR_ROOT/"
cat > "$TAR_ROOT/install.sh" <<'INSTALL'
#!/usr/bin/env bash
# Installs OmniDrop from this folder.
#   ./install.sh            per-user install in ~/.local (no root needed)
#   sudo ./install.sh       system-wide install in /opt/omnidrop (+ polkit rule for Wi-Fi Direct)
#   ./install.sh --uninstall
set -euo pipefail
SRC="$(cd "$(dirname "$0")" && pwd)"
if [ "$(id -u)" -eq 0 ]; then
  PREFIX=/opt/omnidrop; BIN=/usr/local/bin; SHARE=/usr/share
else
  PREFIX="$HOME/.local/opt/omnidrop"; BIN="$HOME/.local/bin"; SHARE="${XDG_DATA_HOME:-$HOME/.local/share}"
fi
if [ "${1:-}" = "--uninstall" ]; then
  rm -rf "$PREFIX" "$BIN/omnidrop" "$SHARE/applications/com.eroideches.omnidrop.desktop"
  find "$SHARE/icons/hicolor" -name omnidrop.png -delete 2>/dev/null || true
  [ "$(id -u)" -eq 0 ] && rm -f /usr/share/polkit-1/rules.d/50-omnidrop-networkmanager.rules \
    /etc/polkit-1/localauthority/50-local.d/50-omnidrop-networkmanager.pkla
  echo "OmniDrop removed."; exit 0
fi
mkdir -p "$PREFIX" "$BIN" "$SHARE/applications"
cp -a "$SRC/." "$PREFIX/"
rm -f "$PREFIX/install.sh"
ln -sf "$PREFIX/omnidrop" "$BIN/omnidrop"
cp "$SRC/share/com.eroideches.omnidrop.desktop" "$SHARE/applications/"
cp -a "$SRC/share/icons" "$SHARE/"
if [ "$(id -u)" -eq 0 ]; then
  install -Dm644 "$SRC/share/polkit/50-omnidrop-networkmanager.rules" /usr/share/polkit-1/rules.d/50-omnidrop-networkmanager.rules
  if [ -d /etc/polkit-1/localauthority ]; then
    install -Dm644 "$SRC/share/polkit/50-omnidrop-networkmanager.pkla" /etc/polkit-1/localauthority/50-local.d/50-omnidrop-networkmanager.pkla
  fi
fi
command -v update-desktop-database >/dev/null && update-desktop-database "$SHARE/applications" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$SHARE/icons/hicolor" || true
echo "OmniDrop installed in $PREFIX (command: omnidrop)."
INSTALL
chmod +x "$TAR_ROOT/install.sh"
tar -C "$WORK" -czf "$DIST/omnidrop-linux-x64.tar.gz" omnidrop

# ---------------------------------------------------------------- .deb
DEB="$WORK/deb"
mkdir -p "$DEB/DEBIAN" "$DEB/opt/omnidrop" "$DEB/usr/bin" "$DEB/usr/share/applications" \
  "$DEB/usr/share/polkit-1/rules.d" "$DEB/etc/polkit-1/localauthority/50-local.d" \
  "$DEB/usr/share/doc/omnidrop"
cp -a "$BUNDLE/." "$DEB/opt/omnidrop/"
ln -s /opt/omnidrop/omnidrop "$DEB/usr/bin/omnidrop"
cp "$PKG/com.eroideches.omnidrop.desktop" "$DEB/usr/share/applications/"
cp -a "$PKG/icons" "$DEB/usr/share/"
cp "$PKG/polkit/50-omnidrop-networkmanager.rules" "$DEB/usr/share/polkit-1/rules.d/"
cp "$PKG/polkit/50-omnidrop-networkmanager.pkla" "$DEB/etc/polkit-1/localauthority/50-local.d/"
cp "$ROOT/LICENSE" "$DEB/usr/share/doc/omnidrop/copyright"
INSTALLED_KB="$(du -sk "$DEB" | cut -f1)"
cat > "$DEB/DEBIAN/control" <<CONTROL
Package: omnidrop
Version: ${VERSION}
Section: net
Priority: optional
Architecture: amd64
Maintainer: Eroideches <180309617+Eroideches@users.noreply.github.com>
Installed-Size: ${INSTALLED_KB}
Depends: libc6 (>= 2.35), libgtk-3-0t64 | libgtk-3-0, libglib2.0-0t64 | libglib2.0-0, libstdc++6
Recommends: network-manager, bluez, xdg-utils
Homepage: https://github.com/Eroideches/OmniDrop
Description: Fast, end-to-end encrypted peer-to-peer file transfer
 OmniDrop sends files, folders, text and links between Android, Windows and
 Linux devices. Devices are found through mDNS, UDP broadcast and Bluetooth LE;
 Wi-Fi Direct links work without a router. Transfers use a Noise_XX handshake,
 AES-256-GCM or ChaCha20-Poly1305, per-block SHA-256 verification, parallel
 streams and automatic resume.
CONTROL
echo "/etc/polkit-1/localauthority/50-local.d/50-omnidrop-networkmanager.pkla" > "$DEB/DEBIAN/conffiles"
cat > "$DEB/DEBIAN/postinst" <<'POSTINST'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database -q /usr/share/applications || true; fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then gtk-update-icon-cache -q /usr/share/icons/hicolor || true; fi
exit 0
POSTINST
cp "$DEB/DEBIAN/postinst" "$DEB/DEBIAN/postrm"
chmod 755 "$DEB/DEBIAN/postinst" "$DEB/DEBIAN/postrm"
find "$DEB" -type d -exec chmod 755 {} +
dpkg-deb --root-owner-group --build "$DEB" "$DIST/omnidrop-linux-x64.deb"

# ---------------------------------------------------------------- Arch PKGBUILD
SHA="$(sha256sum "$DIST/omnidrop-linux-x64.tar.gz" | cut -d' ' -f1)"
sed -e "s/@VERSION@/${VERSION}/g" -e "s/@SHA256@/${SHA}/g" \
  "$ROOT/packaging/arch/PKGBUILD.in" > "$DIST/PKGBUILD"

ls -l "$DIST"
