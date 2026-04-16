#!/bin/bash
set -e

# Cleanup any existing instances (hidden or accessory) to allow a fresh build
pkill -f LocalSearch || true
killall LocalSearch >/dev/null 2>&1 || true

# 1. Build the executable
swift build --configuration release

# 2. Define App Bundle Structure
APP_NAME="LocalSearch"
APP_BUNDLE=".build/release/$APP_NAME.app"
APP_EXECUTABLE=".build/release/$APP_NAME"

echo "Creating bundle at $APP_BUNDLE..."

# Ensure stale bundle artifacts are removed before copying new binaries/resources.
rm -rf "$APP_BUNDLE"

mkdir -p "$APP_BUNDLE/Contents/MacOS"
mkdir -p "$APP_BUNDLE/Contents/Resources"

# 3. Copy Executable
cp "$APP_EXECUTABLE" "$APP_BUNDLE/Contents/MacOS/$APP_NAME"

# 4. Copy Metadata & Assets
cp Sources/Resources/Info.plist "$APP_BUNDLE/Contents/Info.plist"
cp Sources/Resources/AppIcon.icns "$APP_BUNDLE/Contents/Resources/AppIcon.icns"

# 5. Fix Info.plist placeholders (Manual replacement for SPM environments)
sed -i '' "s/\$(EXECUTABLE_NAME)/$APP_NAME/g" "$APP_BUNDLE/Contents/Info.plist"
sed -i '' "s/\$(PRODUCT_NAME)/$APP_NAME/g" "$APP_BUNDLE/Contents/Info.plist"

# 6. Fix executable permissions
chmod +x "$APP_BUNDLE/Contents/MacOS/$APP_NAME"

echo "Success! Built at $APP_BUNDLE"
echo "Executable: $(pwd)/$APP_EXECUTABLE"
echo "Executable mtime: $(stat -f '%Sm' "$APP_EXECUTABLE")"
echo "Bundle executable mtime: $(stat -f '%Sm' "$APP_BUNDLE/Contents/MacOS/$APP_NAME")"
echo "Launching..."

# 7. Open the app bundle
open -n "$APP_BUNDLE"