#!/bin/bash
# Build an AppImage for Canario.
#
# Bundles the binary + all shared libraries (including GTK4, Adwaita, etc.)
# into a self-contained AppImage. The model is NOT bundled — users download
# it from within the app on first launch (like Hex).
#
# Requirements:
#   - cargo build --release (already done)
#   - appimagetool in PATH (downloaded automatically if missing)
#
# Usage:
#   ./packaging/build-appimage.sh

set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="${1:-0.1.0}"
APP="Canario"
APPDIR="packaging/appimage/${APP}.AppDir"
ARCH="$(uname -m)"

echo "==> Building ${APP} AppImage v${VERSION} (${ARCH})"

# ── Check for release binary ────────────────────────────────────────
if [ ! -f "target/release/canario" ]; then
    echo "ERROR: target/release/canario not found. Run: cargo build --release --bin canario"
    exit 1
fi

# ── Create AppDir structure ─────────────────────────────────────────
rm -rf "${APPDIR}"
mkdir -p "${APPDIR}/usr/bin"
mkdir -p "${APPDIR}/usr/lib"
mkdir -p "${APPDIR}/usr/share/applications"
mkdir -p "${APPDIR}/usr/share/icons/hicolor/scalable/apps"
mkdir -p "${APPDIR}/usr/share/icons/hicolor/128x128/apps"

# ── Copy binary ─────────────────────────────────────────────────────
cp target/release/canario "${APPDIR}/usr/bin/"
strip --strip-unneeded "${APPDIR}/usr/bin/canario" 2>/dev/null || true

# ── Copy .desktop file ──────────────────────────────────────────────
cat > "${APPDIR}/com.canario.Canario.desktop" << 'EOF'
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

cp "${APPDIR}/com.canario.Canario.desktop" "${APPDIR}/usr/share/applications/"

# ── Copy icons ──────────────────────────────────────────────────────
cp assets/canario.svg "${APPDIR}/usr/share/icons/hicolor/scalable/apps/com.canario.Canario.svg"
cp assets/canario.svg "${APPDIR}/com.canario.Canario.svg"
# Also copy as DirIcon (AppImage requirement)
cp assets/canario.svg "${APPDIR}/.DirIcon"

# ── Generate AppRun ─────────────────────────────────────────────────
cat > "${APPDIR}/AppRun" << 'RUNEOF'
#!/bin/bash
APPDIR="$(dirname "$(readlink -f "$0")")"
export PATH="${APPDIR}/usr/bin:${PATH}"
export LD_LIBRARY_PATH="${APPDIR}/usr/lib:${APPDIR}/usr/lib/x86_64-linux-gnu:${LD_LIBRARY_PATH}"

# GTK settings
export GDK_PIXBUF_MODULEDIR="${APPDIR}/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders"
export GDK_PIXBUF_MODULE_FILE="${APPDIR}/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache"
export GTK_PATH="${APPDIR}/usr/lib/gtk-4.0"
export XDG_DATA_DIRS="${APPDIR}/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
export GSETTINGS_SCHEMA_DIR="${APPDIR}/usr/share/glib-2.0/schemas"

# SSL certs for model download
export SSL_CERT_FILE="/etc/ssl/certs/ca-certificates.crt"

exec "${APPDIR}/usr/bin/canario" "$@"
RUNEOF
chmod +x "${APPDIR}/AppRun"

# ── Bundle shared libraries ─────────────────────────────────────────
echo "==> Bundling shared libraries..."

# Get all dependencies
DEPS=$(ldd "target/release/canario" | grep "=>" | awk '{print $3}' | grep -v "^$" | sort -u)

for lib in ${DEPS}; do
    if [ -f "${lib}" ]; then
        cp -L "${lib}" "${APPDIR}/usr/lib/"
    fi
done

# Bundle additional GTK/GdkPixbuf data that's needed at runtime
echo "==> Bundling GTK resources..."

# GdkPixbuf loaders
LOADERS_DIR="/usr/lib/x86_64-linux-gnu/gdk-pixbuf-2.0/2.10.0/loaders"
if [ -d "${LOADERS_DIR}" ]; then
    mkdir -p "${APPDIR}/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders"
    cp -L ${LOADERS_DIR}/*.so "${APPDIR}/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders/" 2>/dev/null || true
    # Generate loaders.cache pointing to bundled paths
    if command -v gdk-pixbuf-query-loaders &>/dev/null; then
        GDK_PIXBUF_MODULEDIR="${APPDIR}/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders" \
            gdk-pixbuf-query-loaders > "${APPDIR}/usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache" 2>/dev/null || true
    fi
fi

# GSettings schemas
SCHEMAS_DIR="/usr/share/glib-2.0/schemas"
if [ -d "${SCHEMAS_DIR}" ]; then
    mkdir -p "${APPDIR}/usr/share/glib-2.0/schemas"
    # Copy schemas for GTK4, Adwaita, and common ones
    for schema in "${SCHEMAS_DIR}"/org.gtk.* "${SCHEMAS_DIR}"/org.gnome.desktop.* "${SCHEMAS_DIR}"/org.gnome.settings-daemon.*; do
        cp ${schema} "${APPDIR}/usr/share/glib-2.0/schemas/" 2>/dev/null || true
    done
    # Compile schemas
    glib-compile-schemas "${APPDIR}/usr/share/glib-2.0/schemas" 2>/dev/null || true
fi

# ── Download appimagetool if needed ─────────────────────────────────
if ! command -v appimagetool &>/dev/null; then
    echo "==> Downloading appimagetool..."
    ARCHIVE="appimagetool-${ARCH}.AppImage"
    wget -q "https://github.com/AppImage/AppImageKit/releases/download/continuous/${ARCHIVE}" \
        -O "/tmp/appimagetool"
    chmod +x "/tmp/appimagetool"
    APPIMAGETOOL="/tmp/appimagetool"
else
    APPIMAGETOOL="appimagetool"
fi

# ── Build the AppImage ──────────────────────────────────────────────
echo "==> Creating AppImage..."

OUTPUT="packaging/Canario-${VERSION}-${ARCH}.AppImage"

${APPIMAGETOOL} "${APPDIR}" "${OUTPUT}" 2>&1 || {
    echo ""
    echo "AppDir created at: ${APPDIR}"
    echo "appimagetool failed. You can try manually:"
    echo "  ${APPIMAGETOOL} ${APPDIR} ${OUTPUT}"
    echo ""
    echo "Alternatively, test the AppDir directly:"
    echo "  ${APPDIR}/AppRun"
    exit 1
}

echo ""
echo "✅ AppImage created: ${OUTPUT}"
echo "   Size: $(du -h "${OUTPUT}" | cut -f1)"
echo ""
echo "   Run it with: chmod +x ${OUTPUT} && ./${OUTPUT}"
echo "   No model bundled — it will prompt to download on first launch."
