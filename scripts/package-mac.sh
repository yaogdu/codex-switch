#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

APP_NAME="Codex Switch"
PACKAGE_NAME="codex-switch"
VERSION="$(node -p "require('./package.json').version")"
TAURI_VERSION="$(node -p "require('./src-tauri/tauri.conf.json').version")"
ROOT_CARGO_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
APP_CARGO_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' src-tauri/Cargo.toml | head -1)"
HOST="$(rustc -vV | sed -n 's/^host: //p')"
ARCH="${HOST%%-*}"
OUT_DIR="$ROOT/release"
APP_PATH="$ROOT/src-tauri/target/release/bundle/macos/$APP_NAME.app"
DMG_PATH="$ROOT/src-tauri/target/release/bundle/dmg/${APP_NAME}_${VERSION}_${ARCH}.dmg"
DMG_OUT_PATH="$OUT_DIR/${PACKAGE_NAME}-${VERSION}-macos-${ARCH}.dmg"
ZIP_PATH="$OUT_DIR/${PACKAGE_NAME}-${VERSION}-macos-${ARCH}.zip"
SUMS_PATH="$OUT_DIR/SHA256SUMS.txt"

if [[ "$VERSION" != "$TAURI_VERSION" || "$VERSION" != "$ROOT_CARGO_VERSION" || "$VERSION" != "$APP_CARGO_VERSION" ]]; then
  echo "error: version mismatch: package=$VERSION tauri=$TAURI_VERSION root-cargo=$ROOT_CARGO_VERSION app-cargo=$APP_CARGO_VERSION" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
rm -f "$OUT_DIR"/*.dmg "$OUT_DIR"/*.zip "$SUMS_PATH"

echo "==> checks"
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
npm run typecheck
cargo test --manifest-path Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml

echo "==> build dmg"
if npm run tauri -- build --bundles dmg && [[ -f "$DMG_PATH" ]]; then
  cp "$DMG_PATH" "$DMG_OUT_PATH"
  hdiutil verify "$DMG_OUT_PATH"
else
  echo "warn: dmg build failed; keeping app zip as fallback"
  npm run tauri -- build --bundles app
fi

if [[ ! -d "$APP_PATH" ]]; then
  echo "==> build app"
  npm run tauri -- build --bundles app
fi

echo "==> verify app"
codesign --verify --deep --strict "$APP_PATH"
ditto -c -k --sequesterRsrc --keepParent "$APP_PATH" "$ZIP_PATH"

echo "==> checksums"
(
  cd "$OUT_DIR"
  find . -maxdepth 1 -type f \( -name '*.zip' -o -name '*.dmg' \) -print0 \
    | sort -z \
    | xargs -0 shasum -a 256 > "$SUMS_PATH"
)

echo "release artifacts:"
find "$OUT_DIR" -maxdepth 1 -type f -print
