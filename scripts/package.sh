#!/bin/bash
# Build a self-contained LocalSearch.app: the Swift app, the Rust engine
# (liblocalsearch.dylib) and the content-extractor helper, code signed.
#
#   scripts/package.sh
#       Ad-hoc signature: runs on this Mac only.
#
#   CODESIGN_IDENTITY="Developer ID Application: Name (TEAMID)" scripts/package.sh
#       Developer ID signature with the hardened runtime.
#
#   CODESIGN_IDENTITY="…" NOTARY_PROFILE="profile" scripts/package.sh
#       Also notarizes and staples, so the app opens on other Macs without a
#       Gatekeeper warning. Create the profile once with:
#       xcrun notarytool store-credentials profile --apple-id … --team-id …
#
# Output: dist/LocalSearch.app and dist/LocalSearch-<version>.zip
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="LocalSearch"
APP="$REPO/dist/$APP_NAME.app"
IDENTITY="${CODESIGN_IDENTITY:--}"
# Version comes from Cargo.toml; the build number is the commit count
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$REPO/Cargo.toml" | head -1)"
BUILD="$(git -C "$REPO" rev-list --count HEAD 2>/dev/null || echo 1)"

echo "==> Building Rust engine and extractor (release)"
cargo build --release --workspace --manifest-path "$REPO/Cargo.toml"

echo "==> Building Swift app (release)"
swift build --configuration release --package-path "$REPO/frontend"
SWIFT_BIN="$(swift build --configuration release --package-path "$REPO/frontend" --show-bin-path)"

echo "==> Assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks" "$APP/Contents/Resources"

cp "$SWIFT_BIN/$APP_NAME" "$APP/Contents/MacOS/$APP_NAME"
# The engine is loaded from Contents/Frameworks (see FFIBackend.dylibCandidates)
cp "$REPO/target/release/liblocalsearch.dylib" "$APP/Contents/Frameworks/"
# The helper is looked up next to the executable (see extract/client.rs)
cp "$REPO/target/release/localsearch-extractor" "$APP/Contents/MacOS/"
cp "$REPO/frontend/Sources/Resources/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"
# SwiftPM resource bundle, found through Bundle.module
if [ -d "$SWIFT_BIN/${APP_NAME}_${APP_NAME}.bundle" ]; then
    cp -R "$SWIFT_BIN/${APP_NAME}_${APP_NAME}.bundle" "$APP/Contents/Resources/"
fi

# Info.plist: fill in the Xcode-style placeholders SwiftPM does not expand
sed -e "s/\$(EXECUTABLE_NAME)/$APP_NAME/g" \
    -e "s/\$(PRODUCT_NAME)/$APP_NAME/g" \
    -e "s/\$(DEVELOPMENT_LANGUAGE)/en/g" \
    -e "s/\$(PRODUCT_BUNDLE_PACKAGE_TYPE)/APPL/g" \
    -e "s/\$(MACOSX_DEPLOYMENT_TARGET)/14.0/g" \
    "$REPO/frontend/Sources/Resources/Info.plist" > "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $VERSION" "$APP/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $BUILD" "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist" > /dev/null

echo "==> Signing (identity: $IDENTITY)"
# Inside-out: nested code first, then the bundle
SIGN=(codesign --force --timestamp=none --sign "$IDENTITY")
if [ "$IDENTITY" != "-" ]; then
    SIGN=(codesign --force --options runtime --timestamp --sign "$IDENTITY")
fi
"${SIGN[@]}" "$APP/Contents/Frameworks/liblocalsearch.dylib"
"${SIGN[@]}" "$APP/Contents/MacOS/localsearch-extractor"
"${SIGN[@]}" "$APP"
codesign --verify --deep --strict "$APP"

ZIP="$REPO/dist/$APP_NAME-$VERSION.zip"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"

if [ -n "${NOTARY_PROFILE:-}" ]; then
    if [ "$IDENTITY" = "-" ]; then
        echo "error: notarization needs a Developer ID signature; set CODESIGN_IDENTITY" >&2
        exit 1
    fi
    echo "==> Notarizing (profile: $NOTARY_PROFILE)"
    xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" --wait
    xcrun stapler staple "$APP"
    # Re-zip so the archive carries the stapled ticket
    rm -f "$ZIP"
    ditto -c -k --keepParent "$APP" "$ZIP"
    spctl --assess --type execute --verbose "$APP"
fi

echo "==> Built $APP ($VERSION, build $BUILD)"
echo "==> Archive $ZIP"
