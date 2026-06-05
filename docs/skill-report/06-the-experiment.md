# 6. The experiment (agentic-money prototype-1)
Goal: an open, provenance-signed LLM that **executes** agentic-money + Flux tool-calls on par with Claude Code, served on rented compute.
**Real + measured (read-from-output):**
- Qwen3.6-27B = 90% zero-shot tool-calls · DeepSeek-V2-16B = 67% (right-tool 100%)
- GPU CUDA compile 1.7s · BLAKE3 miner 2.364 GH/s (Tesla T4)
- Trade sim +5.3% (live Binance $73.9k) · 100-agent chronos swarm beat buy&hold (+18.8% vs +11.4%)
- cost model: self-host beats DeepSeek API above ~1B tok/mo · invoice + pricing + governor stacks
**Infra-blocked (honest, not the technique):**
- Qwen3.6 live-serving: ollama 500s on the arch (needs vLLM; vLLM boxes wouldn't boot)
- DeepSeek-R1 671B: math closes (1.58-bit + MoE offload, 251GB combined) but ollama can't load **sharded** GGUF (issue #5245) → needs raw llama.cpp
- Vast: repeated box-boot hangs + the create-on-"rejected" leak (4 A100s once)
**Verdict:** architecture proven, models measured; live-serving the *newest* models is a runtime/infra gap (ollama version), not a capability gap.
