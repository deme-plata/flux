#!/usr/bin/env python3
"""train_lora.py — QLoRA fine-tune flux-moe on the tool-call corpus.

Reads the function-calling JSONL (messages + tools), formats each example with the
tokenizer's chat template (tools included so the model learns to PICK + FILL), and
trains a 4-bit QLoRA adapter. Small base (Qwen2.5-1.5B-Instruct) so it fits an
RTX 3090 and trains in minutes. Output = a LoRA adapter dir (push to HF with the
write token). This is the "beat MiniCPM5-1B on agentic tool-calls" candidate.

  HF_TOKEN=... python3 train_lora.py --corpus toolcall-corpus.jsonl --out ./flux-moe-tool-lora
"""
import argparse, json, os, sys, time

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", default="toolcall-corpus.jsonl")
    ap.add_argument("--base", default="Qwen/Qwen2.5-1.5B-Instruct")
    ap.add_argument("--out", default="./flux-moe-tool-lora")
    ap.add_argument("--epochs", type=float, default=3.0)
    args = ap.parse_args()

    import torch
    from datasets import Dataset
    from transformers import AutoModelForCausalLM, AutoTokenizer, BitsAndBytesConfig
    from peft import LoraConfig, get_peft_model, prepare_model_for_kbit_training
    from trl import SFTTrainer, SFTConfig

    t0 = time.time()
    rows = [json.loads(l) for l in open(args.corpus) if l.strip()]
    print(f"loaded {len(rows)} examples from {args.corpus}", flush=True)

    tok = AutoTokenizer.from_pretrained(args.base)
    if tok.pad_token is None:
        tok.pad_token = tok.eos_token

    def fmt(r):
        # render the messages WITH tools so the model learns the function-calling format
        text = tok.apply_chat_template(r["messages"], tools=r.get("tools"),
                                       tokenize=False, add_generation_prompt=False)
        return {"text": text}
    ds = Dataset.from_list([fmt(r) for r in rows])

    bnb = BitsAndBytesConfig(load_in_4bit=True, bnb_4bit_quant_type="nf4",
                             bnb_4bit_compute_dtype=torch.bfloat16, bnb_4bit_use_double_quant=True)
    model = AutoModelForCausalLM.from_pretrained(args.base, quantization_config=bnb, device_map="auto")
    model = prepare_model_for_kbit_training(model)
    lora = LoraConfig(r=16, lora_alpha=32, lora_dropout=0.05, bias="none",
                      task_type="CAUSAL_LM",
                      target_modules=["q_proj","k_proj","v_proj","o_proj","gate_proj","up_proj","down_proj"])
    model = get_peft_model(model, lora)
    model.print_trainable_parameters()

    cuda = torch.cuda.is_available()
    use_bf16 = cuda and torch.cuda.is_bf16_supported()
    print(f"cuda={cuda} bf16={use_bf16}", flush=True)
    cfg = SFTConfig(output_dir=args.out, num_train_epochs=args.epochs,
                    per_device_train_batch_size=2, gradient_accumulation_steps=4,
                    learning_rate=2e-4, logging_steps=5, save_strategy="epoch",
                    bf16=use_bf16, fp16=(cuda and not use_bf16), use_cpu=not cuda,
                    max_length=2048, report_to=[])
    trainer = SFTTrainer(model=model, args=cfg, train_dataset=ds)
    trainer.train()
    trainer.save_model(args.out)
    tok.save_pretrained(args.out)
    print(f"FLUX_MOE_LORA_DONE {args.out} in {time.time()-t0:.0f}s", flush=True)

if __name__ == "__main__":
    main()
