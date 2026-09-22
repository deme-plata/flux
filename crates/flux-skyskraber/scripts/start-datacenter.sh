#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$root"
config="${QUILLON_DATACENTER_CONFIG:-$root/crates/flux-skyskraber/datacenter.json}"
fluxc="${FLUXC_BIN:-$root/target/debug/fluxc}"
"$fluxc" build -p flux-skyskraber --features datacenter --bin quillon-datacenter
binary="${CARGO_TARGET_DIR:-$root/target}/debug/quillon-datacenter"
if [[ ! -f "$config" ]]; then "$binary" init "$config"; fi
exec "$binary" start "$config" "$@"
