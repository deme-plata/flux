import json, torch, eval_headtohead as E
from transformers import AutoModelForCausalLM, AutoTokenizer
tools = json.loads(open('/root/combined-corpus.jsonl').readline())['tools']
tok = AutoTokenizer.from_pretrained('Qwen/Qwen3.6-27B')
m = AutoModelForCausalLM.from_pretrained('Qwen/Qwen3.6-27B', dtype=torch.bfloat16, device_map='auto')
for goal, gt, ga in E.HELD[:2]:
    for think in (True, False):
        try:
            p = tok.apply_chat_template([{'role':'user','content':goal}], tools=tools,
                                        tokenize=False, add_generation_prompt=True, enable_thinking=think)
        except TypeError:
            p = tok.apply_chat_template([{'role':'user','content':goal}], tools=tools,
                                        tokenize=False, add_generation_prompt=True)
        ids = tok(p, return_tensors='pt').to(m.device)
        out = m.generate(**ids, max_new_tokens=256, do_sample=False, pad_token_id=tok.eos_token_id)
        txt = tok.decode(out[0][ids.input_ids.shape[1]:], skip_special_tokens=True)
        print(f'=== GOAL: {goal[:40]} | want {gt} | enable_thinking={think} ===')
        print('RAW:', repr(txt[:500]))
