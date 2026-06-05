#!/bin/bash
# flux_archive.sh — Auto-archive Flux builds, source, and benchmarks to 80TB archive
# Usage: ./flux_archive.sh [version]
#   version: optional version tag (default: date-based)

set -euo pipefail

ARCHIVE_ROOT="/home/storage/flux-archive"
SOURCE_DIR="/home/storage/deepseek-codewhale"
VERSION="${1:-v$(date +%Y%m%d-%H%M%S)}"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)

echo "⚡ Flux Archive — $VERSION ($TIMESTAMP)"
echo "   Archive root: $ARCHIVE_ROOT"

# Ensure archive directories exist
mkdir -p "$ARCHIVE_ROOT"/{builds,source,benchmarks,releases}

# 1. Archive source snapshot
echo "   📦 Source snapshot..."
cd "$SOURCE_DIR"
tar -czf "$ARCHIVE_ROOT/source/flux-$VERSION.tar.gz" \
    --exclude='target' --exclude='.git' \
    flux/ 2>/dev/null
echo "      → $ARCHIVE_ROOT/source/flux-$VERSION.tar.gz"

# 2. Archive binary (if built)
if [ -f flux/target/release/fluxc ]; then
    cp flux/target/release/fluxc "$ARCHIVE_ROOT/builds/fluxc-$VERSION"
    chmod 755 "$ARCHIVE_ROOT/builds/fluxc-$VERSION"
    sha256sum "$ARCHIVE_ROOT/builds/fluxc-$VERSION" > "$ARCHIVE_ROOT/builds/fluxc-$VERSION.sha256"
    echo "   ⚙️  Binary: fluxc-$VERSION"
fi

# Also archive debug binary
if [ -f flux/target/debug/fluxc ]; then
    cp flux/target/debug/fluxc "$ARCHIVE_ROOT/builds/fluxc-$VERSION-debug"
    chmod 755 "$ARCHIVE_ROOT/builds/fluxc-$VERSION-debug"
    echo "   ⚙️  Binary (debug): fluxc-$VERSION-debug"
fi

# 3. Archive benchmark stats
STATS_FILE="$HOME/.flux/stats.json"
if [ -f "$STATS_FILE" ]; then
    cp "$STATS_FILE" "$ARCHIVE_ROOT/benchmarks/stats-$VERSION.json"
    echo "   📊 Benchmarks archived"
else
    echo "   ⚠ No stats file at $STATS_FILE"
fi

# 4. Create release tarball (binary + docs)
if [ -f flux/target/release/fluxc ]; then
    cd "$SOURCE_DIR"
    tar -czf "$ARCHIVE_ROOT/releases/flux-$VERSION.tar.gz" \
        flux/target/release/fluxc \
        flux/ARCHIVE.md \
        flux/instructions.md \
        flux/CHANGELOG.md \
        2>/dev/null
    sha256sum "$ARCHIVE_ROOT/releases/flux-$VERSION.tar.gz" > "$ARCHIVE_ROOT/releases/flux-$VERSION.tar.gz.sha256"
    echo "   📦 Release tarball: flux-$VERSION.tar.gz"
fi

# 5. Report
echo ""
echo "✓ Archive complete: $VERSION"
echo ""
echo "Archive contents:"
du -sh "$ARCHIVE_ROOT"/*/ 2>/dev/null || true
echo ""
echo "Recent archives:"
ls -lt "$ARCHIVE_ROOT/source/" | head -5
