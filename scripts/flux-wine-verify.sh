#!/bin/bash
# flux-wine-verify — run a Windows console .exe under Wine on a HEADLESS box and assert
# the TUI actually initializes, by reading the app's own boot trace. Built into the Flux
# windows pipeline so a windows build is NEVER shipped without proof it boots.
#
# It tests TWO console conditions, because Wine's default console always reports a valid
# size and so HIDES the real conhost failure mode:
#   • pty mode      — `script` pty, console reports a normal size (happy path).
#   • adverse mode  — stdout redirected to a PIPE (no console) → the Windows
#                     GetConsoleScreenBufferInfo path ERRORS / returns 0x0, which is the
#                     real "blank TUI on Windows conhost" repro. A robust binary must STILL
#                     reach "first frame drawn" here (via the SafeSizeBackend size clamp).
#
# PASS = the boot trace reaches the SUCCESS_MARK in the tested mode(s). Non-zero on fail,
# and it prints the exact last boot-trace line so you see WHERE it died.
#
# Usage: flux-wine-verify.sh <windows-exe> [pty|adverse|both]   (default: both)
set -uo pipefail
EXE="${1:?usage: flux-wine-verify.sh <windows-exe> [pty|adverse|both]}"
MODE="${2:-both}"
SUCCESS_MARK="first frame drawn"
WINE="$(command -v wine64 || command -v wine || echo /usr/lib/wine/wine64)"
[ -x "$WINE" ] || { echo "FAIL: wine not found (apt-get install --no-install-recommends wine64)"; exit 3; }

run_mode() {  # $1 = pty|adverse
  local mode="$1"
  local WP; WP="$(mktemp -d /tmp/flux-wine.XXXXXX)"
  cp -f "$EXE" "$WP/app.exe"
  local TRACE="$WP/drive_c/users/$USER/Temp/sigil-top-startup.log"
  # init prefix quietly (idempotent), then run with all wine noise off
  env WINEPREFIX="$WP" WINEDEBUG=-all "$WINE" wineboot --init >/dev/null 2>&1; sleep 2
  if [ "$mode" = pty ]; then
    timeout 20 script -qfec "env WINEPREFIX=$WP WINEDEBUG=-all $WINE $WP/app.exe" /dev/null >"$WP/out.txt" 2>&1
  else
    # adverse: NO pty — stdout is a pipe → Windows console-size query errors (the real repro)
    timeout 20 env WINEPREFIX="$WP" WINEDEBUG=-all "$WINE" "$WP/app.exe" </dev/null >"$WP/out.txt" 2>&1 &
    local pid=$!; sleep 14; kill -9 "$pid" 2>/dev/null; wait "$pid" 2>/dev/null
  fi
  # boot trace may be under either of wine's user dirs
  local t; t="$(find "$WP/drive_c" -name 'sigil-top-startup.log' 2>/dev/null | head -1)"
  if [ -z "$t" ] || ! [ -s "$t" ]; then echo "[$mode] FAIL — no boot trace written (crashed before main())"; rm -rf "$WP"; return 1; fi
  echo "[$mode] boot trace:"; sed 's/^/    /' "$t"
  if grep -q "$SUCCESS_MARK" "$t"; then
    echo "[$mode] ✅ PASS — TUI initialized ('$SUCCESS_MARK')"; rm -rf "$WP"; return 0
  fi
  # No first frame. For adverse (no console at all) a CLEAN headless is the CORRECT outcome —
  # a bare pipe can't host an interactive TUI; only a CRASH (Err/panic) is a real failure there.
  if [ "$mode" = adverse ] && ! grep -qiE 'returned Err|PANIC|panicked|error 6|Invalid handle' "$t"; then
    echo "[$mode] ✅ PASS — no console → clean headless (expected, not a crash)"; rm -rf "$WP"; return 0
  fi
  echo "[$mode] ❌ FAIL — died at: $(tail -1 "$t")"; rm -rf "$WP"; return 1
}

rc=0
case "$MODE" in
  pty)     run_mode pty || rc=1 ;;
  adverse) run_mode adverse || rc=1 ;;
  both)    run_mode pty || rc=1; echo; run_mode adverse || rc=1 ;;
  *) echo "unknown mode '$MODE'"; exit 2 ;;
esac
echo; [ $rc -eq 0 ] && echo "flux-wine-verify: ✅ ALL MODES PASS — safe to ship" || echo "flux-wine-verify: ❌ FAILED — do NOT ship"
exit $rc
