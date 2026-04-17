#!/bin/bash
# Build all distributable packages for Canario.
#
# Usage:
#   ./packaging/build-all.sh [version]
#
# Requires: cargo, dpkg-deb, and optionally flatpak-builder / appimagetool

set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="${1:-0.1.0}"

echo "╔══════════════════════════════════════════════════╗"
echo "║   Canario v${VERSION} — Build All Packages          ║"
echo "╚══════════════════════════════════════════════════╝"
echo ""

# ── Release build ───────────────────────────────────────────────────
echo "==> Building release binaries..."
cargo build --release

echo ""
echo "Binaries:"
echo "  canario:     $(du -h target/release/canario | cut -f1)"
echo "  canario-cli: $(du -h target/release/canario-cli | cut -f1)"
echo ""

# ── .deb (always works on Debian/Ubuntu) ────────────────────────────
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  Building .deb package..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
./packaging/build-deb.sh "${VERSION}"
echo ""

# ── AppImage (optional — needs appimagetool) ────────────────────────
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  Building AppImage..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if command -v appimagetool &>/dev/null || [ -f /tmp/appimagetool ]; then
    ./packaging/build-appimage.sh "${VERSION}"
else
    echo "  ⚠ Skipping AppImage (appimagetool not found)"
    echo "    Download from: https://github.com/AppImage/AppImageKit/releases"
fi
echo ""

# ── Flatpak (optional — needs flatpak-builder) ──────────────────────
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  Building Flatpak..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if command -v flatpak-builder &>/dev/null; then
    ./packaging/build-flatpak.sh "${VERSION}"
else
    echo "  ⚠ Skipping Flatpak (flatpak-builder not found)"
    echo "    Install with: sudo apt install flatpak-builder"
fi
echo ""

# ── Summary ─────────────────────────────────────────────────────────
echo "╔══════════════════════════════════════════════════╗"
echo "║   Build Complete                                 ║"
echo "╚══════════════════════════════════════════════════╝"
echo ""
echo "Output files in packaging/:"
ls -lh packaging/*.deb packaging/*.AppImage packaging/*.flatpak 2>/dev/null || echo "  (no packages found)"
echo ""
echo "cargo install:"
echo "  cargo install --path . --features static"
echo ""
echo "Note: No ASR model is bundled. Users download it"
echo "      from within the app on first launch (~640MB)."
