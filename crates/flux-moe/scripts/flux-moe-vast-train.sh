#!/usr/bin/env bash
# flux-moe-vast-train.sh — ONE-SHOT QLoRA on a Vast box, all 2026-05-31 fixes baked in.
#
# Launch DETACHED via the vast MCP `ssh_execute_background_command` (NOT nohup-over-
# ssh_execute_command — that returns -1 and the child dies when the channel closes).
# Then watch with `ssh_check_background_task` or the log-poll watcher (see SKILL.md).
#
# Encodes the hard-won lessons:
#   • PEP 668: system Python refuses pip → `--break-system-packages`.
#   • torch CUDA build MUST match the box's NVIDIA driver. `pip install torch` grabs
#     the NEWEST (cu130) which fails on a 12.2 driver ("driver too old"). Detect the
#     driver's CUDA and pick the matching cuXXX wheel.
#   • transformers (latest) needs `torch.distributed.tensor.device_mesh` → torch ≥2.5.
#     So: torch 2.5.1 on the right cuXXX index (2.5.1 has device_mesh AND cu121 wheels).
#   • NEVER suppress pip errors (no >/dev/null) — that's how the first run "succeeded"
#     with no torch installed.
#
# Usage (on the box):
#   HF_TOKEN=hf_... CORPUS=/root/toolcall-corpus.jsonl OUT=/root/flux-moe-tool-lora \
#     BASE=Qwen/Qwen2.5-1.5B-Instruct ./flux-moe-vast-train.sh
set -uo pipefail
T0=$(date +%s)
: "${CORPUS:=/root/toolcall-corpus.jsonl}" "${OUT:=/root/flux-moe-tool-lora}"
: "${BASE:=Qwen/Qwen2.5-1.5B-Instruct}" "${TORCH_VER:=2.5.1}"
export HF_HOME="${HF_HOME:-/root/hf}"

echo "=== driver / CUDA detect ==="
DRV_CUDA=$(nvidia-smi | grep -oE 'CUDA Version: [0-9]+\.[0-9]+' | grep -oE '[0-9]+\.[0-9]+' | head -1)
echo "driver CUDA: ${DRV_CUDA:-unknown}"
# pick the largest cuXXX wheel index that does NOT exceed the driver's CUDA
case "$DRV_CUDA" in
  12.0|12.1|12.2|12.3) CU=cu121 ;;
  12.4|12.5|12.6|12.7|12.8) CU=cu124 ;;
  13.*) CU=cu124 ;;            # cu124 wheels run fine on a 13.x driver (forward-compat)
  11.*) CU=cu118; TORCH_VER=2.5.1 ;;
  *)    CU=cu121 ;;            # safe default for modern boxes
esac
echo "→ torch ${TORCH_VER} + ${CU}"

echo "=== install (PEP668-safe, errors visible) ==="
python3 -m pip install --break-system-packages -q \
  "torch==${TORCH_VER}" --index-url "https://download.pytorch.org/whl/${CU}" || { echo "TORCH_INSTALL_FAIL"; exit 11; }
python3 -m pip install --break-system-packages -q \
  transformers peft trl bitsandbytes datasets accelerate || { echo "ML_STACK_FAIL"; exit 12; }

echo "=== sanity ==="
python3 - <<'PY'
import torch, transformers
print("torch", torch.__version__, "cuda", torch.cuda.is_available(),
      "bf16", torch.cuda.is_bf16_supported() if torch.cuda.is_available() else False,
      "tf", transformers.__version__)
assert torch.cuda.is_available(), "CUDA NOT AVAILABLE — wrong torch/driver match"
PY
[ $? -ne 0 ] && { echo "CUDA_SANITY_FAIL"; exit 13; }

echo "=== train $(date +%T) ==="
python3 /root/train_lora.py --corpus "$CORPUS" --out "$OUT" --base "$BASE"
RC=$?
echo "TRAIN_RC=$RC  total=$(( $(date +%s) - T0 ))s"
exit $RC
