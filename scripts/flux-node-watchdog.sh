#!/usr/bin/env bash
# flux-node-watchdog.sh — self-healing node keeper for the owned fleet.
#
# The agentic-money node-maintenance loop (see skill: node-maintenance).
# Polls every box in PARALLEL each cycle, classifies GREEN/AMBER/RED, and
# (with --heal) takes the smallest corrective action. Emits status with
# R>=2 redundancy (the chronos lesson: single-send loses ~10% at 10% loss).
#
# Usage:
#   flux-node-watchdog.sh --once                 # one observe-only cycle
#   flux-node-watchdog.sh --interval 30          # loop every 30s, observe-only
#   flux-node-watchdog.sh --interval 30 --heal   # loop + auto-heal RED nodes
#   flux-node-watchdog.sh --once --webhook URL   # also POST status (sent twice = R2)
#
# Health is process + disk + listening-port + (best-effort) height stall.
# Rule 0: a RED node is fixed before anything else.
set -uo pipefail

# ---- fleet ------------------------------------------------------------------
# name:ip:service:port   (service = systemd unit to restart on --heal; '-' = none)
FLEET=(
  "epsilon:89.149.241.126:q-api-server:8080"
  "beta:185.182.185.227:q-api-server:8080"
  "delta:5.79.79.158:q-api-server:8080"
  "gemma:109.205.176.60:sigil-node:-"   # Gamma runs sigil-node, NOT q-api-server (stopped 2026-05-21). port '-' = skip port check
)
# Health process regex (matches any chain node we run).
HEALTH_PROC='q-api-server|sigil-node|flux-p2p-test'
# Boxes we must NEVER `systemctl restart` directly (use ha-deploy instead).
NO_DIRECT_RESTART="beta epsilon"

# ---- args -------------------------------------------------------------------
INTERVAL=0; HEAL=0; WEBHOOK=""; ONCE=0
LOG=/home/storage/logs/flux-node-watchdog.log
STATE=/tmp/flux-watchdog-state   # remembers last height per node for stall detection
mkdir -p "$(dirname "$LOG")" "$STATE"
while [ $# -gt 0 ]; do case "$1" in
  --once) ONCE=1;;
  --interval) INTERVAL="$2"; shift;;
  --heal) HEAL=1;;
  --webhook) WEBHOOK="$2"; shift;;
  *) echo "unknown arg: $1"; exit 2;;
esac; shift; done

# ---- R>=2 redundant status emit --------------------------------------------
emit() {  # emit "<line>"  -> local log (sink 1) + webhook twice (sinks 2,3)
  local line="$1"; local ts; ts="$(date -u +%H:%M:%S)"
  echo "[$ts] $line" | tee -a "$LOG"
  if [ -n "$WEBHOOK" ]; then
    for _ in 1 2; do curl -s -m 4 -X POST -d "$line" "$WEBHOOK" >/dev/null 2>&1 || true; done
  fi
}

# Local IPs of THIS box — probe self locally, never SSH to our own public IP.
SELF_IPS=" $(hostname -I 2>/dev/null) 127.0.0.1 "

# ---- per-node health probe (local for self, one SSH round-trip otherwise) ---
# Authoritative liveness = `systemctl is-active <unit>` (the box's intended node
# service), NOT a hardcoded port — different boxes run different nodes
# (epsilon/beta/delta = q-api-server:8080, gemma = sigil-node). Port + height are
# best-effort detail only.
probe() {  # probe name ip unit port -> echoes "name|status|detail"
  local name="$1" ip="$2" unit="$3" port="$4"
  local check="
    proc=\$(pgrep -fc '$HEALTH_PROC' 2>/dev/null || echo 0)
    active=\$(systemctl is-active $unit 2>/dev/null || echo unknown)
    disk=\$(df / | awk 'NR==2{gsub(/%/,\"\",\$5); print \$5}')
    availg=\$(df -BG / | awk 'NR==2{gsub(/G/,\"\",\$4); print \$4}')
    if [ '$port' = '-' ]; then listen=NA; else listen=\$(ss -tlnp 2>/dev/null | grep -c ':$port '); fi
    echo \"\${proc:-0};\${active:-unknown};\${disk:-0};\${availg:-0};\${listen:-NA}\"
  "
  local r=""
  if echo "$SELF_IPS" | grep -q " $ip "; then
    r=$(bash -c "$check" 2>/dev/null)                       # self: run locally
  else
    # Rule 2 (R>=2): retry the SSH probe before declaring unreachable —
    # a single missed probe (slow/rate-limited sshd, e.g. beta) is not a down node.
    local attempt
    for attempt in 1 2; do
      r=$(timeout -s KILL 18 ssh -o BatchMode=yes -o ConnectTimeout=10 \
            -o StrictHostKeyChecking=accept-new root@"$ip" "$check" 2>/dev/null)
      [ -n "$r" ] && break
      sleep 2
    done
  fi
  if [ -z "$r" ]; then echo "$name|RED|unreachable(ssh x2)"; return; fi
  local proc active disk availg listen; IFS=';' read -r proc active disk availg listen <<<"$r"
  # classify — service-active is authoritative; free-GB is the disk signal
  local status="GREEN" detail="$unit=$active proc=$proc disk=${disk}%/${availg}Gfree listen=$listen"
  if [ "$active" != "active" ]; then status="RED"; detail="$detail NODE_DOWN($active)"; fi
  if [ "$listen" != "NA" ] && [ "${listen:-0}" -lt 1 ] && [ "$active" = "active" ]; then
    status="AMBER"; detail="$detail PORT_DOWN"; fi
  if [ "${availg:-999}" -lt 10 ] && [ "$status" = "GREEN" ]; then status="AMBER"; detail="$detail DISK_TIGHT"; fi
  if [ "${availg:-999}" -lt 3 ] || [ "${disk:-0}" -ge 97 ]; then status="RED"; detail="$detail DISK_CRIT"; fi
  echo "$name|$status|$detail"
}

# ---- heal action (only with --heal) ----------------------------------------
heal() {  # heal name ip unit port detail
  local name="$1" ip="$2" unit="$3" port="$4" detail="$5"
  if echo "$detail" | grep -q NODE_DOWN; then
    if echo " $NO_DIRECT_RESTART " | grep -q " $name "; then
      emit "  ↳ $name NODE_DOWN but restart-protected (use ha-deploy.sh) — NOT auto-restarting"
      return
    fi
    emit "  ↳ HEAL $name: systemctl restart $unit"
    timeout -s KILL 30 ssh -o BatchMode=yes root@"$ip" "systemctl restart $unit" 2>&1 | tail -1 | sed 's/^/      /'
    sleep 3
    emit "  ↳ post-heal $(probe "$name" "$ip" "$unit" "$port")"
  fi
}

# ---- one cycle: probe all in parallel, then classify -----------------------
cycle() {
  local tmp; tmp=$(mktemp -d)
  for entry in "${FLEET[@]}"; do
    IFS=':' read -r name ip svc port <<<"$entry"
    probe "$name" "$ip" "$svc" "$port" > "$tmp/$name" &
  done
  wait
  local red=0 amber=0 green=0
  for entry in "${FLEET[@]}"; do
    IFS=':' read -r name ip svc port <<<"$entry"
    local line; line=$(cat "$tmp/$name" 2>/dev/null)
    local status; status=$(echo "$line" | cut -d'|' -f2)
    local detail; detail=$(echo "$line" | cut -d'|' -f3)
    case "$status" in
      RED)   red=$((red+1));   emit "🔴 $line";;
      AMBER) amber=$((amber+1)); emit "🟡 $line";;
      *)     green=$((green+1)); emit "🟢 $line";;
    esac
    if [ "$HEAL" = 1 ] && [ "$status" = "RED" ]; then heal "$name" "$ip" "$svc" "$port" "$detail"; fi
  done
  emit "── cycle summary: 🟢$green 🟡$amber 🔴$red  (heal=$HEAL)"
  rm -rf "$tmp"
  return $red
}

# ---- main -------------------------------------------------------------------
emit "flux-node-watchdog start (interval=${INTERVAL}s heal=$HEAL webhook=$([ -n "$WEBHOOK" ] && echo yes || echo no))"
if [ "$ONCE" = 1 ] || [ "$INTERVAL" = 0 ]; then
  cycle; exit $?
fi
while true; do cycle; sleep "$INTERVAL"; done
