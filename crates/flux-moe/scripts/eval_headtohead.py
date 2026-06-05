#!/usr/bin/env python3
"""eval_headtohead.py — agentic tool-call accuracy: tuned adapter vs base (vs MiniCPM).

Runs the 20 HELD-OUT goals (disjoint from training, mirrors flux-moe eval.rs) through
each model with the SAME tool schemas used in training, parses the emitted tool_call,
and scores Exact / ToolOnly / Miss. Prints accuracy + who beats whom. This is the
"beat MiniCPM5-1B on agentic tool-calls" number — read from real model output.
"""
import json, re, sys, argparse

# 20 held-out cases — (goal, gold_tool, required-arg subset that must match)
HELD = [
 ("What is my current QUG balance?", "get_balance", {}),
 ("Transfer 42 QUG over to Codex", "send_qug", {"amount":"42"}),
 ("Airdrop 250 USDS to Viktor", "send_token", {"amount":"250","token":"USDS"}),
 ("Give me a price quote to convert 75 QUG to QUGUSD", "dex_get_quote", {"token_in":"QUG","token_out":"QUGUSD"}),
 ("Trade 200 USDS back into QUG right now", "dex_swap", {"token_in":"USDS","token_out":"QUG"}),
 ("Are there any arbitrage plays open?", "arb_scan", {}),
 ("Check the live Ethereum price and spread", "market_scan", {"symbol":"ETHUSDT"}),
 ("Settle this lightning bill: lnbc500n1...", "ln_pay", {"invoice":"lnbc500n1..."}),
 ("Pull a fresh bitcoin deposit address for me", "btc_generate_deposit_address", {}),
 ("Cash out 0.05 BTC to bc1qcoldwallet", "btc_withdraw", {"address":"bc1qcoldwallet","amount":"0.05"}),
 ("Mint a new coin named Sigil Star, ticker SIGS, total 5000000", "deploy_token", {"symbol":"SIGS"}),
 ("Break down everything I'm holding", "portfolio_overview", {}),
 ("Build and run the test suite for flux-bridge", "flux_combo", {"package":"flux-bridge"}),
 ("Estimate how long flux-recursive-proofs takes to build", "flux_predict", {"package":"flux-recursive-proofs"}),
 ("sigil-oracle is throwing a compile error, suggest a fix", "flux_qspec", {"package":"sigil-oracle"}),
 ("Simulate gossip across 50 peers at 200ms", "flux_chronos_run", {"nodes":"50"}),
 ("Run the ZK verification gate", "flux_zk_combo", {}),
 ("DCA 100 into Bitcoin", "btc_dca_buy", {"amount":"100"}),
 ("Route 500 QUG of profit into the BTC stack", "treasury_route_to_btc", {"amount":"500"}),
 ("Order food from ILD.PIZZA and pay from my BTC", "bitrefill_order", {"merchant":"ILD.PIZZA"}),
]

def parse_call(text):
    # strip any <think>...</think> reasoning block first (thinking models)
    if '</think>' in text:
        text = text.split('</think>')[-1]
    # FORMAT A — Qwen XML: <function=NAME> <parameter=KEY>\nVAL\n</parameter> ...
    fm = re.search(r'<function=([A-Za-z0-9_]+)\s*>', text)
    if fm:
        args = {}
        for pk, pv in re.findall(r'<parameter=([A-Za-z0-9_]+)\s*>\s*(.*?)\s*</parameter>', text, re.S):
            args[pk] = pv.strip()
        return fm.group(1), args
    # FORMAT B — JSON (Qwen2.5 / Hermes / OpenAI): <tool_call>{...}</tool_call> or bare {...}
    m = re.search(r'<tool_call>\s*(\{.*?\})\s*</tool_call>', text, re.S) or re.search(r'\{.*?"name".*?\}', text, re.S)
    if m:
        try:
            j = json.loads(m.group(1) if m.lastindex else m.group(0))
            name = j.get("name") or (j.get("function") or {}).get("name")
            args = j.get("arguments") or (j.get("function") or {}).get("arguments") or {}
            if isinstance(args, str):
                args = json.loads(args)
            return name, args
        except Exception:
            pass
    return None

def grade(pred, gold_tool, gold_args):
    if not pred: return "miss"
    name, args = pred
    if name != gold_tool: return "miss"
    for k, v in gold_args.items():
        if str(args.get(k, "")).strip() != str(v): return "toolonly"
    return "exact"

def run_model(model_id, adapter, tools, tok, torch, AutoModelForCausalLM, PeftModel):
    print(f"\n=== {model_id} {'+ '+adapter if adapter else '(base)'} ===", flush=True)
    model = AutoModelForCausalLM.from_pretrained(model_id, torch_dtype=torch.bfloat16, device_map="auto")
    if adapter:
        model = PeftModel.from_pretrained(model, adapter)
    t = AutoTokenizer.from_pretrained(model_id)
    res = {"exact":0,"toolonly":0,"miss":0}
    for goal, gt, ga in HELD:
        msgs = [{"role":"user","content":goal}]
        try:  # thinking models (Qwen3+): disable the <think> block so it emits the call directly
            prompt = t.apply_chat_template(msgs, tools=tools, tokenize=False, add_generation_prompt=True, enable_thinking=False)
        except TypeError:
            prompt = t.apply_chat_template(msgs, tools=tools, tokenize=False, add_generation_prompt=True)
        ids = t(prompt, return_tensors="pt").to(model.device)
        out = model.generate(**ids, max_new_tokens=256, do_sample=False, pad_token_id=t.eos_token_id)
        text = t.decode(out[0][ids.input_ids.shape[1]:], skip_special_tokens=True)
        g = grade(parse_call(text), gt, ga)
        res[g] += 1
        print(f"  [{g:8}] {goal[:42]:42} → want {gt}", flush=True)
    n = len(HELD)
    print(f"  → exact {res['exact']}/{n} ({res['exact']/n*100:.0f}%) · tool {(res['exact']+res['toolonly'])}/{n} · miss {res['miss']}", flush=True)
    del model
    torch.cuda.empty_cache()
    return res

if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default="Qwen/Qwen2.5-1.5B-Instruct")
    ap.add_argument("--adapter", default="/root/flux-moe-tool-lora")
    ap.add_argument("--corpus", default="/root/toolcall-corpus.jsonl")
    a = ap.parse_args()
    import torch
    from transformers import AutoModelForCausalLM, AutoTokenizer
    from peft import PeftModel
    globals()['AutoTokenizer'] = AutoTokenizer
    tools = json.loads(open(a.corpus).readline())["tools"]
    tok = None
    base = run_model(a.base, None, tools, tok, torch, AutoModelForCausalLM, PeftModel)
    tuned = run_model(a.base, a.adapter, tools, tok, torch, AutoModelForCausalLM, PeftModel)
    n = len(HELD)
    print("\n================ HEAD-TO-HEAD (exact tool-call accuracy) ================")
    print(f"  base  Qwen2.5-1.5B   : {base['exact']}/{n}  ({base['exact']/n*100:.0f}%)")
    print(f"  TUNED flux-moe-lora  : {tuned['exact']}/{n}  ({tuned['exact']/n*100:.0f}%)")
    verdict = "TUNED WINS" if tuned['exact'] > base['exact'] else ("TIE" if tuned['exact']==base['exact'] else "base wins")
    print(f"  → {verdict}  (Δ {tuned['exact']-base['exact']:+d} exact)")
