#!/bin/bash
# Build a Flatpak for Canario.
#
# The model is NOT bundled — users download it from within the app.
# Uses the Freedesktop runtime with GTK4/Adwaita extensions.
#
# Requirements:
#   - flatpak in PATH
#   - flatpak-builder in PATH
#   - org.freedesktop.Sdk//24.08 runtime installed
#
# Usage:
#   ./packaging/build-flatpak.sh [version]

set -euo pipefail
cd "$(dirname "$0")/.."

VERSION="${1:-0.1.0}"

echo "==> Building Canario Flatpak v${VERSION}"

# ── Ensure runtime is installed ─────────────────────────────────────
RUNTIME="org.freedesktop.Sdk"
RUNTIME_VERSION="24.08"

if ! flatpak info "${RUNTIME}//${RUNTIME_VERSION}" &>/dev/null; then
    echo "==> Installing ${RUNTIME} ${RUNTIME_VERSION}..."
    flatpak install -y flathub "${RUNTIME}//${RUNTIME_VERSION}" || {
        echo "ERROR: Failed to install runtime. Run:"
        echo "  flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo"
        echo "  flatpak install flathub ${RUNTIME}//${RUNTIME_VERSION}"
        exit 1
    }
fi

# ── Generate manifest from Cargo.lock ───────────────────────────────
echo "==> Generating cargo sources..."
mkdir -p packaging/flatpak/generated-sources

# Use flatpak-cargo-generator if available, otherwise manual approach
if command -v flatpak-cargo-generator.py &>/dev/null; then
    flatpak-cargo-generator.py Cargo.lock -o packaging/flatpak/generated-sources/cargo-sources.json
else
    echo "NOTE: flatpak-cargo-generator.py not found."
    echo "      Install it with: pip install flatpak-builder-tools"
    echo "      Falling back to local-source-only build."
fi

# ── Write the manifest ──────────────────────────────────────────────
cat > packaging/flatpak/com.canario.Canario.json << MANIFEST
{
    "app-id": "com.canario.Canario",
    "runtime": "${RUNTIME}",
    "runtime-version": "${RUNTIME_VERSION}",
    "sdk": "${RUNTIME}",
    "command": "canario",
    "finish-args": [
        "--share=network",
        "--share=ipc",
        "--socket=fallback-x11",
        "--socket=wayland",
        "--socket=pulseaudio",
        "--device=dri",
        "--filesystem=xdg-data/canario:create",
        "--filesystem=xdg-config/canario:create",
        "--talk-name=org.freedesktop.Notifications",
        "--talk-name=org.kde.StatusNotifierWatcher"
    ],
    "cleanup": [
        "/include",
        "/lib/pkgconfig",
        "/share/doc",
        "/share/gtk-doc",
        "/share/man",
        "*.la",
        "*.a"
    ],
    "modules": [
        {
            "name": "canario",
            "buildsystem": "simple",
            "build-commands": [
                "cargo build --release --bin canario --bin canario-cli",
                "install -Dm755 target/release/canario /app/bin/canario",
                "install -Dm755 target/release/canario-cli /app/bin/canario-cli",
                "install -Dm644 assets/canario.svg /app/share/icons/hicolor/scalable/apps/com.canario.Canario.svg",
                "install -Dm644 packaging/flatpak/com.canario.Canario.desktop /app/share/applications/com.canario.Canario.desktop",
                "install -Dm644 packaging/flatpak/com.canario.Canario.metainfo.xml /app/share/metainfo/com.canario.Canario.metainfo.xml"
            ],
            "sources": [
                {
                    "type": "dir",
                    "path": "../../"
                }
            ]
        }
    ]
}
MANIFEST

# ── Write .desktop file for Flatpak ─────────────────────────────────
mkdir -p packaging/flatpak
cat > packaging/flatpak/com.canario.Canario.desktop << 'EOF'
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

# ── Write AppStream metainfo ────────────────────────────────────────
cat > packaging/flatpak/com.canario.Canario.metainfo.xml << METAINFO
<?xml version="1.0" encoding="UTF-8"?>
<component type="desktop-application">
  <id>com.canario.Canario</id>
  <name>Canario</name>
  <summary>Native Linux voice-to-text using Parakeet TDT</summary>
  <metadata_license>MIT</metadata_license>
  <project_license>MIT</project_license>
  <description>
    <p>
      Canario transcribes your voice to text using the Parakeet TDT neural
      network model running locally on your CPU. All processing happens on
      your machine — no data is sent to the cloud.
    </p>
    <p>
      Features:
    </p>
    <ul>
      <li>System tray GUI with recording indicator</li>
      <li>Global hotkey support (X11 and Wayland)</li>
      <li>Auto-paste transcriptions into any app</li>
      <li>Multilingual support (English, Spanish, French, German, and more)</li>
      <li>Word remapping and removal post-processing</li>
      <li>Transcription history</li>
      <li>Sound effects</li>
    </ul>
    <p>
      On first launch, you'll be prompted to download the ASR model (~640MB).
    </p>
  </description>
  <launchable type="desktop-id">com.canario.Canario.desktop</launchable>
  <url type="homepage">https://github.com/user/canario</url>
  <provides>
    <binary>canario</binary>
    <binary>canario-cli</binary>
  </provides>
  <content_rating type="oars-1.1" />
  <releases>
    <release version="${VERSION}" date="$(date +%Y-%m-%d)">
      <description>
        <p>Initial release.</p>
      </description>
    </release>
  </releases>
</component>
METAINFO

# ── Build ────────────────────────────────────────────────────────────
echo "==> Building Flatpak..."
flatpak-builder \
    --force-clean \
    --user \
    --install \
    --state-dir=packaging/flatpak/.flatpak-builder \
    packaging/flatpak/build-dir \
    packaging/flatpak/com.canario.Canario.json

# ── Export to .flatpak file ─────────────────────────────────────────
OUTPUT="packaging/Canario-${VERSION}-x86_64.flatpak"
echo "==> Exporting to ${OUTPUT}..."
flatpak build-bundle packaging/flatpak/repo "${OUTPUT}" com.canario.Canario

echo ""
echo "✅ Flatpak created: ${OUTPUT}"
echo "   Size: $(du -h "${OUTPUT}" | cut -f1)"
echo ""
echo "   Install with:  flatpak install ${OUTPUT}"
echo "   Run with:      flatpak run com.canario.Canario"
