#!/usr/bin/env bash
# flux-moe-vast-up.sh — bring the flux-moe LLM up on a Vast.ai box in ~50s.
#
# Speed comes from: (1) the `ollama/ollama` image already runs `ollama serve`
# (no install), (2) a 20Gbit box pulls a 1.5B GGUF in seconds, (3) the model is
# pulled DIRECTLY FROM HUGGINGFACE via Ollama's hf.co/ source (the HF combo) —
# authed with the read token, no separate download.
#
# Usage (on the box, via flux-ssh / ssh_execute_command):
#   HF_TOKEN=hf_... MODEL=hf.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF:Q4_K_M ./flux-moe-vast-up.sh
set -euo pipefail
T0=$(date +%s.%N)

MODEL="${MODEL:-hf.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF:Q4_K_M}"
export OLLAMA_HOST="0.0.0.0:11434"
[ -n "${HF_TOKEN:-}" ] && export HF_TOKEN   # Ollama forwards this to hf.co pulls

# 0. ensure ollama exists (no-op on the ollama/ollama image; ~15s on a bare CUDA image)
command -v ollama >/dev/null 2>&1 || curl -fsSL https://ollama.com/install.sh | sh

# 1. ensure the server is up (image entrypoint usually already started it)
if ! curl -fsS localhost:11434/api/version >/dev/null 2>&1; then
  nohup ollama serve >/var/log/ollama.log 2>&1 &
  for i in $(seq 1 20); do curl -fsS localhost:11434/api/version >/dev/null 2>&1 && break; sleep 0.5; done
fi
echo "ollama: $(curl -fsS localhost:11434/api/version)"

# 2. pull the model straight from HuggingFace (the HF combo)
echo "pulling $MODEL from HuggingFace ..."
ollama pull "$MODEL"

# 3. prove it actually answers (one real generate — read-from-output, not assumed)
echo "── live generate ──"
curl -fsS localhost:11434/api/generate -d "{\"model\":\"$MODEL\",\"prompt\":\"In one sentence: what is agentic money?\",\"stream\":false}" \
  | python3 -c "import sys,json;print(json.load(sys.stdin)['response'].strip())"

T1=$(date +%s.%N)
echo "FLUX_MOE_VAST_READY in $(python3 -c "print(f'{$T1-$T0:.1f}s')")"
echo "point the router at it:  export FLUX_MOE_OLLAMA=http://<this-box-ip>:11434"
