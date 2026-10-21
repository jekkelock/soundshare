#!/bin/sh
set -eu

PROJECT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_ROOT"

VERSION="${PROJECT_VERSION:-$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')}"
BINARY_NAME="soundshare"
DIST_DIR="$PROJECT_ROOT/dist"
mkdir -p "$DIST_DIR"

build_release() {
    echo "Building release binary..."
    cargo build --release --bin "$BINARY_NAME"
}

package_linux() {
    build_release
    BIN_PATH="$PROJECT_ROOT/target/release/$BINARY_NAME"
    SIZE=$(wc -c < "$BIN_PATH")
    echo "Linux binary size: $SIZE bytes"

    TMP_DIR=$(mktemp -d)
    trap 'rm -rf "$TMP_DIR"' EXIT

    cp "$BIN_PATH" "$TMP_DIR/$BINARY_NAME"
    cat > "$TMP_DIR/$BINARY_NAME.desktop" << EOF
[Desktop Entry]
Name=SoundShare
Exec=$BINARY_NAME
Icon=audio-x-realaudio
Type=Application
Categories=AudioVideo;Audio;
EOF

    ARCHIVE_NAME="soundshare-${VERSION}-x86_64-unknown-linux-gnu.tar.gz"
    tar -czf "$DIST_DIR/$ARCHIVE_NAME" -C "$TMP_DIR" "$BINARY_NAME" "$BINARY_NAME.desktop"
    echo "Created: $DIST_DIR/$ARCHIVE_NAME"
}

package_macos() {
    build_release
    BIN_PATH="$PROJECT_ROOT/target/release/$BINARY_NAME"
    SIZE=$(wc -c < "$BIN_PATH")
    echo "macOS binary size: $SIZE bytes"

    APP_DIR="$DIST_DIR/SoundShare.app"
    mkdir -p "$APP_DIR/Contents/MacOS"
    mkdir -p "$APP_DIR/Contents/Resources"

    cp "$BIN_PATH" "$APP_DIR/Contents/MacOS/$BINARY_NAME"

    cat > "$APP_DIR/Contents/Info.plist" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>$BINARY_NAME</string>
    <key>CFBundleName</key>
    <string>SoundShare</string>
    <key>CFBundleIdentifier</key>
    <string>org.soundshare.app</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleIconFile</key>
    <string></string>
</dict>
</plist>
EOF

    ARCHIVE_NAME="soundshare-${VERSION}-x86_64-apple-darwin.dmg"
    if command -v hdiutil >/dev/null 2>&1; then
        hdiutil create -volname "SoundShare" -srcfolder "$APP_DIR" -ov -format UDZO "$DIST_DIR/$ARCHIVE_NAME"
        rm -rf "$APP_DIR"
        echo "Created: $DIST_DIR/$ARCHIVE_NAME"
    else
        echo "hdiutil not found; bundling .app instead"
        ARCHIVE_NAME="soundshare-${VERSION}-x86_64-apple-darwin.zip"
        (cd "$DIST_DIR" && zip -r "$ARCHIVE_NAME" "SoundShare.app")
        rm -rf "$APP_DIR"
        echo "Created: $DIST_DIR/$ARCHIVE_NAME"
    fi
}

package_windows() {
    build_release
    BIN_PATH="$PROJECT_ROOT/target/release/$BINARY_NAME.exe"
    SIZE=$(wc -c < "$BIN_PATH")
    echo "Windows binary size: $SIZE bytes"

    TMP_DIR=$(mktemp -d)
    trap 'rm -rf "$TMP_DIR"' EXIT

    cp "$BIN_PATH" "$TMP_DIR/$BINARY_NAME.exe"

    ARCHIVE_NAME="soundshare-${VERSION}-x86_64-pc-windows-msvc.zip"
    (cd "$TMP_DIR" && zip -r "$DIST_DIR/$ARCHIVE_NAME" "$BINARY_NAME.exe")
    echo "Created: $DIST_DIR/$ARCHIVE_NAME"
}

OS_TYPE=$(uname -s)
case "$OS_TYPE" in
    Linux*)
        package_linux
        ;;
    Darwin*)
        package_macos
        ;;
    MINGW*|MSYS*|CYGWIN*)
        package_windows
        ;;
    *)
        echo "Unsupported OS: $OS_TYPE"
        exit 1
        ;;
esac

echo "Done."
