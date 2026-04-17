#!/bin/bash
# Build a .deb package for Canario.
#
# The .deb depends on system GTK4 and libadwaita packages (not bundled).
# The model is NOT bundled — users download it from within the app.
#
# Usage:
#   ./packaging/build-deb.sh [version]

set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="${1:-0.1.0}"
ARCH="amd64"
PKG_NAME="canario"
PKG_DIR="packaging/deb/${PKG_NAME}_${VERSION}_${ARCH}"

echo "==> Building ${PKG_NAME} .deb v${VERSION} (${ARCH})"

# ── Check for release binaries ──────────────────────────────────────
for bin in canario canario-cli; do
    if [ ! -f "target/release/${bin}" ]; then
        echo "ERROR: target/release/${bin} not found. Run: cargo build --release"
        exit 1
    fi
done

# ── Create package structure ────────────────────────────────────────
rm -rf "${PKG_DIR}"
mkdir -p "${PKG_DIR}/DEBIAN"
mkdir -p "${PKG_DIR}/usr/bin"
mkdir -p "${PKG_DIR}/usr/share/applications"
mkdir -p "${PKG_DIR}/usr/share/icons/hicolor/scalable/apps"
mkdir -p "${PKG_DIR}/usr/share/doc/canario"

# ── Copy binaries ───────────────────────────────────────────────────
cp target/release/canario "${PKG_DIR}/usr/bin/"
cp target/release/canario-cli "${PKG_DIR}/usr/bin/"

# ── Copy .desktop file ──────────────────────────────────────────────
cat > "${PKG_DIR}/usr/share/applications/com.canario.Canario.desktop" << 'EOF'
[Desktop Entry]
Type=Application
Name=Canario
GenericName=Voice to Text
Comment=Native Linux voice-to-text using Parakeet TDT
Exec=canario
Icon=com.canario.Canario
Terminal=false
Categories=Utility;AudioVideo;
Keywords=voice;speech;text;transcription;dictation;
StartupNotify=false
EOF

# ── Copy icon ───────────────────────────────────────────────────────
cp assets/canario.svg "${PKG_DIR}/usr/share/icons/hicolor/scalable/apps/com.canario.Canario.svg"

# ── Control file ────────────────────────────────────────────────────
cat > "${PKG_DIR}/DEBIAN/control" << EOF
Package: ${PKG_NAME}
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: ${ARCH}
Depends: libgtk-4-1 (>= 4.14), libadwaita-1-0 (>= 1.5), libasound2 (>= 1.2), libc6 (>= 2.35), libssl3 (>= 3.0)
Recommends: xdotool | wtype | ydotool
Suggests: evtest
Maintainer: Canario Contributors
Description: Native Linux voice-to-text using Parakeet TDT
 Canario transcribes your voice to text using the Parakeet TDT neural
 network model running locally on your CPU. It provides a system tray
 GUI with global hotkey support for X11 and Wayland.
 .
 On first launch, you'll be prompted to download the ASR model (~640MB).
 No data leaves your machine — all processing is done locally.
Homepage: https://github.com/user/canario
License: MIT
EOF

# ── Postinst: update icon cache ─────────────────────────────────────
cat > "${PKG_DIR}/DEBIAN/postinst" << 'EOF'
#!/bin/bash
set -e
# Update icon cache
if command -v gtk-update-icon-cache-4.0 &>/dev/null; then
    gtk-update-icon-cache-4.0 -q /usr/share/icons/hicolor 2>/dev/null || true
fi
# Update desktop database
if command -v update-desktop-database &>/dev/null; then
    update-desktop-database -q /usr/share/applications 2>/dev/null || true
fi
EOF
chmod 755 "${PKG_DIR}/DEBIAN/postinst"

# ── Copyright / license ─────────────────────────────────────────────
cp LICENSE "${PKG_DIR}/usr/share/doc/canario/" 2>/dev/null || echo "MIT License" > "${PKG_DIR}/usr/share/doc/canario/copyright"

# ── Changelog ───────────────────────────────────────────────────────
cat > "${PKG_DIR}/usr/share/doc/canario/changelog.Debian" << EOF
canario (${VERSION}) noble; urgency=low

  * Initial release.

 -- Canario Contributors  $(date -R)
EOF
gzip -9 "${PKG_DIR}/usr/share/doc/canario/changelog.Debian"

# ── Build the .deb ──────────────────────────────────────────────────
dpkg-deb --build --root-owner-group "${PKG_DIR}"

OUTPUT="packaging/${PKG_NAME}_${VERSION}_${ARCH}.deb"
mv "${PKG_DIR}.deb" "${OUTPUT}"

echo ""
echo "✅ .deb package created: ${OUTPUT}"
echo "   Size: $(du -h "${OUTPUT}" | cut -f1)"
echo ""
echo "   Install with:  sudo dpkg -i ${OUTPUT}"
echo "   Or:            sudo apt install ./${OUTPUT}"
echo ""
echo "   Dependencies (auto-installed with apt):"
echo "     libgtk-4-1, libadwaita-1-0, libasound2, libssl3"
echo "   Optional:      xdotool (X11 auto-paste), wtype (Wayland auto-paste)"
