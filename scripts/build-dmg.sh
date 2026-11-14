#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v dmgbuild >/dev/null; then
  echo 'Activate the packaging virtual environment described in README.md first.' >&2
  exit 1
fi

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-src-tauri/target}"
case "$CARGO_TARGET_DIR" in
  /*) ;;
  *) CARGO_TARGET_DIR="$PWD/$CARGO_TARGET_DIR" ;;
esac

env -u APPLE_SIGNING_IDENTITY -u APPLE_CERTIFICATE -u APPLE_CERTIFICATE_PASSWORD \
  -u APPLE_ID -u APPLE_PASSWORD -u APPLE_TEAM_ID -u APPLE_API_KEY \
  -u APPLE_API_ISSUER -u APPLE_API_KEY_PATH \
  npm run tauri -- build --ci --bundles app --target aarch64-apple-darwin \
  --config '{"bundle":{"macOS":{"signingIdentity":"-"}}}' -- --locked

bundle_dir="$CARGO_TARGET_DIR/aarch64-apple-darwin/release/bundle"
app="$bundle_dir/macos/Mini System Monitor.app"
version=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")
dmg_dir="$bundle_dir/dmg"
dmg_name="mini-system-monitor_${version}_aarch64.dmg"

mkdir -p "$dmg_dir"
work_dir=$(mktemp -d "$dmg_dir/.build.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT

dmgbuild -s scripts/dmg-settings.py -D "app=$app" \
  "Mini System Monitor" "$work_dir/$dmg_name"
hdiutil verify "$work_dir/$dmg_name"
mv -f "$work_dir/$dmg_name" "$dmg_dir/$dmg_name"
echo "Built $dmg_dir/$dmg_name"
