#!/bin/bash

set -e

# Default parameter values
RELEASE_BUILD=false
NEW_VERSION=""

# Parse command-line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --release)
            RELEASE_BUILD=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# For release builds, bump the version
if [ "$RELEASE_BUILD" = true ]; then
    # Read the current version from Cargo.toml
    CURRENT_VERSION=$(grep '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')

    # Bump the version (simply increment the patch component)
    VERSION_PARTS=(${CURRENT_VERSION//./ })
    MAJOR=${VERSION_PARTS[0]}
    MINOR=${VERSION_PARTS[1]}
    PATCH=${VERSION_PARTS[2]}
    NEW_PATCH=$((PATCH + 1))
    NEW_VERSION="${MAJOR}.${MINOR}.${NEW_PATCH}"

    echo "Updating version from $CURRENT_VERSION to $NEW_VERSION"

    # Update the version in Cargo.toml
    sed -i "s/^version = \".*\"/version = \"$NEW_VERSION\"/" Cargo.toml
    
    PACKAGE_NAME="collection-framework-v$NEW_VERSION"
else
    PACKAGE_NAME="collection-framework-$(date +%Y%m%d-%H%M%S)"
fi

echo "Building project..."
if [ "$RELEASE_BUILD" = true ]; then
    cargo build --release
else
    cargo build
fi

echo "Creating package directory..."
PACKAGE_DIR="/tmp/$PACKAGE_NAME"
mkdir -p "$PACKAGE_DIR"

echo "Copying files to package directory..."

if [ "$RELEASE_BUILD" = true ]; then
    cp target/release/CollectionFramework "$PACKAGE_DIR/"
else
    cp target/debug/CollectionFramework "$PACKAGE_DIR/"
fi
cp -r src/plugins/pyki/pyki_dev_dir/ "$PACKAGE_DIR/pyki_dir"
cp src/plugins/pykiLoader/libloader.so "$PACKAGE_DIR/"
cp src/third_party/cupti/libcupti.so.2024.3.2 "$PACKAGE_DIR/"
cp src/plugins/cuprof/build/libcuprof.so "$PACKAGE_DIR/"

cp config.yaml "$PACKAGE_DIR/"

if [ -f "Readme.md" ]; then
    cp Readme.md "$PACKAGE_DIR/"
fi

echo "Creating tar.gz package..."
tar -czf "$PACKAGE_NAME.tar.gz" -C /tmp "$PACKAGE_NAME"

echo "Package created: $PACKAGE_NAME.tar.gz"

rm -rf "$PACKAGE_DIR"

if [ "$RELEASE_BUILD" = true ]; then
    echo "Done! Release package is ready: $PACKAGE_NAME.tar.gz"
else
    echo "Done! Package is ready: $PACKAGE_NAME.tar.gz"
fi