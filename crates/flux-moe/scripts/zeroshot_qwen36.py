import json, torch, eval_headtohead as E
from transformers import AutoModelForCausalLM, AutoTokenizer
from peft import PeftModel
E.AutoTokenizer = AutoTokenizer
tools = json.loads(open('/root/combined-corpus.jsonl').readline())['tools']
r = E.run_model('Qwen/Qwen3.6-27B', None, tools, None, torch, AutoModelForCausalLM, PeftModel)
n = len(E.HELD)
print('QWEN36_ZEROSHOT_EXACT', r['exact'], '/', n, '=', round(r['exact']/n*100), 'pct  | tool', r['exact']+r['toolonly'], '/', n)
