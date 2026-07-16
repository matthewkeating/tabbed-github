#!/usr/bin/env bash
#
# Build one of the site apps and deploy it to ~/Applications.
#
# Usage: deploy.sh [github|gemini]   (defaults to github)
#
# Steps:
#   1. Build the .app bundle via `pnpm tauri build --bundles app`, passing the
#      site's Cargo feature and (for non-default sites) its config overrides.
#   2. If any instance of the app is running, quit it (gracefully, then force).
#   3. Copy the freshly built .app into ~/Applications and clear quarantine.
#
set -euo pipefail

SITE="${1:-github}"

# Per-site build parameters. The shared Rust/config lives in one codebase; only
# these values (and the referenced tauri.<site>.conf.json / icons) differ.
case "$SITE" in
  github)
    APP_NAME="GitHub"
    BUNDLE_ID="com.matthewkeating.tabbed-github"
    BUILD_ARGS=()
    ;;
  gemini)
    APP_NAME="Gemini"
    BUNDLE_ID="com.matthewkeating.tabbed-gemini"
    BUILD_ARGS=(--features gemini --config src-tauri/tauri.gemini.conf.json)
    ;;
  *)
    echo "Error: unknown site \"$SITE\" (expected: github, gemini)" >&2
    exit 1
    ;;
esac

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC_APP="$ROOT_DIR/src-tauri/target/release/bundle/macos/${APP_NAME}.app"
DEST_DIR="$HOME/Applications"
DEST_APP="$DEST_DIR/${APP_NAME}.app"

# Matches the running executable of GitHub.app from ANY location, e.g.
# ".../GitHub.app/Contents/MacOS/GitHub". Specific enough not to match
# unrelated apps such as "GitHub Desktop.app". Runs headlessly with no
# permission prompt, so a `pnpm deploy` never stalls waiting on one.
PROC_PATTERN="${APP_NAME}.app/Contents/MacOS/"

echo "==> Building ${APP_NAME}.app ..."
pnpm tauri build --bundles app ${BUILD_ARGS[@]+"${BUILD_ARGS[@]}"}

if [ ! -d "$SRC_APP" ]; then
  echo "Error: expected build output not found at $SRC_APP" >&2
  exit 1
fi

if pgrep -f "$PROC_PATTERN" >/dev/null 2>&1; then
  echo "==> Running instance detected, quitting it ..."
  # Ask nicely first (by bundle id) so the app can shut down cleanly.
  osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true

  # Wait up to ~5s for a clean exit.
  for _ in $(seq 1 10); do
    pgrep -f "$PROC_PATTERN" >/dev/null 2>&1 || break
    sleep 0.5
  done

  # Force-kill anything still hanging around.
  if pgrep -f "$PROC_PATTERN" >/dev/null 2>&1; then
    echo "==> Force-killing remaining instance(s) ..."
    pkill -9 -f "$PROC_PATTERN" 2>/dev/null || true
    sleep 1
  fi
else
  echo "==> No running instance found."
fi

echo "==> Copying to $DEST_APP ..."
mkdir -p "$DEST_DIR"
rm -rf "$DEST_APP"
cp -R "$SRC_APP" "$DEST_APP"

# Unsigned local builds get flagged by Gatekeeper on first launch; clear the
# quarantine attribute so the app opens without the "unidentified developer"
# prompt.
echo "==> Clearing quarantine attribute ..."
xattr -dr com.apple.quarantine "$DEST_APP" 2>/dev/null || true

echo "==> Done. Deployed ${APP_NAME}.app to $DEST_DIR"
