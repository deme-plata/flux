import time, torch
from transformers import AutoModelForCausalLM, AutoTokenizer
tok = AutoTokenizer.from_pretrained('Qwen/Qwen3.6-27B')
t_load = time.time()
m = AutoModelForCausalLM.from_pretrained('Qwen/Qwen3.6-27B', dtype=torch.bfloat16, device_map='auto')
print(f"A100_LOAD {time.time()-t_load:.1f}s  gpu_mem={torch.cuda.memory_allocated()/1e9:.1f}GB")
prompt = ("Write a detailed technical section of the SIGIL chain whitepaper: BLAKE3 proof-of-work, "
          "the four committed state roots, 10ms tip verification via STARK, and the hard 21M supply cap.")
p = tok.apply_chat_template([{'role':'user','content':prompt}], tokenize=False, add_generation_prompt=True, enable_thinking=False)
ids = tok(p, return_tensors='pt').to(m.device)
t0 = time.time()
out = m.generate(**ids, max_new_tokens=512, do_sample=False, pad_token_id=tok.eos_token_id)
dt = time.time()-t0
n = out.shape[1]-ids.input_ids.shape[1]
txt = tok.decode(out[0][ids.input_ids.shape[1]:], skip_special_tokens=True)
print(f"A100_BENCH gen_tok={n} time={dt:.1f}s tok_per_s={n/dt:.1f}")
print("SIGIL_SAMPLE:", txt[:400].replace(chr(10),' '))
