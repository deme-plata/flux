# Flux Archive — 80TB Storage

**Path:** `/home/storage/flux-archive/`
**Created:** 2026-05-27 | **Agent:** DeepSeek V4

## Directory Structure

```
/home/storage/flux-archive/
├── builds/          Compiled binaries (q-api-server, fluxc, etc.)
├── source/          Source snapshots (tar.gz per version)
├── benchmarks/      Benchmark history (JSON per run)
├── releases/        Versioned release tarballs
└── README.md        This file
```

## Setup

```bash
mkdir -p /home/storage/flux-archive/{builds,source,benchmarks,releases}
```

## Archival Commands

### Archive a source snapshot
```bash
cd /home/storage/deepseek-codewhale
tar -czf /home/storage/flux-archive/source/flux-v$(date +%Y%m%d-%H%M%S).tar.gz \
  --exclude='target' --exclude='.git' flux/
```

### Archive a build
```bash
VERSION=v0.8.0
cp target/release/fluxc /home/storage/flux-archive/builds/fluxc-$VERSION-$(date +%Y%m%d)
sha256sum /home/storage/flux-archive/builds/fluxc-$VERSION-* > /home/storage/flux-archive/builds/fluxc-$VERSION.sha256
```

### Archive benchmarks
```bash
cp ~/.flux/stats.json /home/storage/flux-archive/benchmarks/stats-$(date +%Y%m%d-%H%M%S).json
```

### Create a release tarball
```bash
VERSION=v0.8.0
cd /home/storage/deepseek-codewhale
tar -czf /home/storage/flux-archive/releases/flux-$VERSION.tar.gz \
  flux/target/release/fluxc flux/ARCHIVE.md flux/instructions.md
sha256sum /home/storage/flux-archive/releases/flux-$VERSION.tar.gz > /home/storage/flux-archive/releases/flux-$VERSION.tar.gz.sha256
```

## Auto-Archive Script

```bash
# /home/storage/deepseek-codewhale/flux/flux_archive.sh
# Run after each release: archives source + binary + benchmarks
```
