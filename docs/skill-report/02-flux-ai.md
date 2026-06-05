# 2. Flux AI (flux-moe + deepseek-money)
The agentic-money LLM kit — **route + train + serve**, the composition (not merged-weights) way.
- **flux-moe** (44 tests): router, trainer (GPU-QLoRA / CPU-LoRA dispatch), dataset (chronos), toolcorpus (246 tool-call SFT), eval (multi-format parser), serve (Vast/ollama/vLLM), blast (gossip model-propagation), distill (teacher→CPU student→Q4 GGUF), skillroute.
- **Measured (read-from-output):** Qwen3.6-27B = **90% zero-shot** agentic tool-calls; base Qwen2.5-1.5B 50%; a 1.5B QLoRA **overfit** to 30%; DeepSeek-V2-16B = 67% (right-tool 100%).
- **Lesson:** measure zero-shot FIRST; a 0/N score is usually a parser/format mismatch (Qwen emits XML `<function=…>`, not JSON).
- **Serving truth:** ollama 500s on Qwen3.6's new arch + can't load sharded GGUF (R1 671B) → use **vLLM** / llama.cpp for new-arch + sharded models.
- **deepseek-money** skill: DeepSeek picks the tool, flux-market/wallet execute, **propose-only**.
