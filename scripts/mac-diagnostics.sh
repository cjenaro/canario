#!/bin/bash
# mac-diagnostics.sh — one-shot evidence capture for Canario macOS QA.
#
# Run on the Mac AFTER hitting a bug (works whether or not the app is
# still alive) and paste the printed report (or the saved file) into
# the bug report. Collects ONLY non-sensitive data: NO history.json
# (transcripts are private), NO credentials (they never touch disk
# anyway — safeStorage only).
#
# Usage:  bash mac-diagnostics.sh

set -u
OUT="/tmp/canario-diagnostics-$(date +%Y%m%d-%H%M%S).txt"
APP="/Applications/Canario.app"
DATA="$HOME/Library/Application Support/canario"

section() { printf '\n===== %s =====\n' "$1" >> "$OUT"; }

: > "$OUT"
echo "Canario macOS diagnostics — $(date)" >> "$OUT"

section "system"
sw_vers >> "$OUT" 2>&1
uname -a >> "$OUT" 2>&1

section "app"
if [ -d "$APP" ]; then
  /usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' \
    "$APP/Contents/Info.plist" >> "$OUT" 2>&1
  codesign -dv "$APP" 2>> "$OUT" | head -5 >> "$OUT"
  xattr -l "$APP" >> "$OUT" 2>&1 || echo "(no xattrs — quarantine cleared)"
else
  echo "NOT FOUND at $APP (running from elsewhere?)" >> "$OUT"
fi

section "processes"
ps aux | grep -i canario | grep -v grep >> "$OUT" || echo "(none running)" >> "$OUT"

section "sidecar (direct ping)"
if [ -x "$APP/Contents/Resources/sidecar/canario-electron" ]; then
  echo '{"cmd":"ping","id":"diag"}' | timeout 5 \
    "$APP/Contents/Resources/sidecar/canario-electron" >> "$OUT" 2>&1
  echo "exit=$?" >> "$OUT"
  codesign -dv "$APP/Contents/Resources/sidecar/canario-electron" 2>> "$OUT" | head -3 >> "$OUT"
else
  echo "sidecar binary missing" >> "$OUT"
fi

section "config (sanitized)"
if [ -f "$DATA/config.json" ]; then
  # Redact nothing is needed (no secrets live here), but keep it short.
  cat "$DATA/config.json" >> "$OUT"
else
  echo "(no config.json — app never got past early boot)" >> "$OUT"
fi

section "sidecar logs (last 60 lines of newest 3)"
ls -t "$DATA/logs/" 2>/dev/null | head -3 | while read -r f; do
  echo "--- $f" >> "$OUT"
  tail -60 "$DATA/logs/$f" >> "$OUT" 2>&1
done
[ -d "$DATA/logs" ] || echo "(no logs dir — sidecar never started)" >> "$OUT"

section "macOS crash reports (newest 3, first 120 lines each)"
ls -t "$HOME/Library/Logs/DiagnosticReports/" 2>/dev/null \
  | grep -iE 'canario|electron' | head -3 | while read -r f; do
  echo "--- $f" >> "$OUT"
  head -120 "$HOME/Library/Logs/DiagnosticReports/$f" >> "$OUT" 2>&1
done

section "launch log (if launched via tee)"
[ -f /tmp/canario-launch.log ] && tail -80 /tmp/canario-launch.log >> "$OUT" \
  || echo "(no /tmp/canario-launch.log — launch from Finder?)" >> "$OUT"

echo
echo "=========================================================="
echo " Report saved to: $OUT"
echo " Paste its contents into the bug report:"
echo "   cat $OUT | pbcopy   # copies it to the clipboard"
echo "=========================================================="
