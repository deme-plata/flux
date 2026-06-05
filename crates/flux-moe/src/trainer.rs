//! trainer.rs — the GPU+CPU-friendly TRAINER orchestrator.
//!
//! flux-moe does NOT reimplement training — the **HuggingFace MCP** (hf.co/mcp)
//! supplies base models + datasets, and HF transformers/peft/trl do the fine-tune.
//! flux-moe's job is **dispatch**: given a base model's size + what compute the
//! swarm has, pick the right backend so training runs on GPU *or* CPU:
//!   - big (>3B params) → **GPU QLoRA** (4-bit + LoRA on a rented Vast GPU —
//!     output = a tiny adapter worth ≫ the rental; the GOOD GPU use, opposite of
//!     mining which loses).
//!   - small (≤1.5B) or a classifier head → **CPU-LoRA / head** on the owned
//!     48-core swarm boxes (free, slower).
//! Each expert (sentiment / trading / coder / general) is one LoRA adapter on an
//! HF base, trained on the swarm's own corpus, then composed by the router.

/// Where the fine-tune runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Gpu,
    Cpu,
}

/// How it's fine-tuned (cheaper → smaller compute).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// 4-bit base + LoRA adapter — GPU, fits big models on one card.
    QLora4bit,
    /// fp16/bf16 LoRA on CPU — only viable for small models.
    CpuLora,
    /// freeze the model, train a small classifier head — CPU-cheap (e.g. sentiment).
    ClassifierHead,
}

/// A dispatched fine-tune plan (the orchestrator's decision).
#[derive(Debug, Clone)]
pub struct TrainPlan {
    pub base_model: String, // HF id, e.g. "Qwen/Qwen3-4B"
    pub backend: Backend,
    pub method: Method,
    pub est_hours: f64,
    pub est_usd: f64, // GPU rental cost (0 on owned CPU)
    pub note: String,
}

/// Decide the trainer for a job. `params_b` = base model size in billions;
/// `gpu_available` = is a (rented/owned) GPU on hand; `classifier_only` = the
/// task is a classifier head (e.g. sentiment) not full generation.
pub fn plan(base_model: &str, params_b: f64, gpu_available: bool, classifier_only: bool) -> TrainPlan {
    // classifier heads are CPU-cheap regardless of base size (base is frozen).
    if classifier_only {
        return TrainPlan {
            base_model: base_model.into(),
            backend: Backend::Cpu,
            method: Method::ClassifierHead,
            est_hours: 0.5,
            est_usd: 0.0,
            note: "frozen base + classifier head — CPU on the 48-core swarm (free)".into(),
        };
    }
    if params_b > 3.0 {
        if gpu_available {
            // ~0.6 GPU-hr per B for a QLoRA pass on a mid-tier card; ~$0.15/hr Vast.
            let h = (params_b * 0.6).max(1.0);
            TrainPlan {
                base_model: base_model.into(),
                backend: Backend::Gpu,
                method: Method::QLora4bit,
                est_hours: h,
                est_usd: h * 0.15,
                note: "QLoRA 4-bit on a rented GPU — adapter output ≫ rental (the good GPU use)".into(),
            }
        } else {
            TrainPlan {
                base_model: base_model.into(),
                backend: Backend::Cpu,
                method: Method::CpuLora,
                est_hours: params_b * 8.0, // CPU LoRA on a big model is painfully slow
                est_usd: 0.0,
                note: "NO GPU — CPU-LoRA possible but slow; rent a GPU for QLoRA or downsize the base".into(),
            }
        }
    } else {
        // small model: CPU-LoRA is feasible on the owned 48-core boxes.
        TrainPlan {
            base_model: base_model.into(),
            backend: Backend::Cpu,
            method: Method::CpuLora,
            est_hours: params_b * 2.0,
            est_usd: 0.0,
            note: "small base — CPU-LoRA on the owned swarm (free, no Vast)".into(),
        }
    }
}

/// Build the actual fine-tune **command** from a plan + corpus. flux-moe does
/// not reimplement training — this drives HF `trl` (SFTTrainer) + `peft` (LoRA),
/// the way `sigil-btc-miner::gpu` builds a miner argv. Run it on a Vast GPU box
/// (QLoRA) or an owned CPU box (CpuLora/head). The base model + dataset are
/// pulled from the **HuggingFace Hub** (so `hf.co/mcp` / `huggingface_hub` must
/// be available on the training box).
///
/// `corpus_jsonl` is the chronos dataset path (see [`crate::dataset`]).
pub fn train_command(plan: &TrainPlan, corpus_jsonl: &str, out_dir: &str) -> Vec<String> {
    let device = match plan.backend {
        Backend::Gpu => "cuda",
        Backend::Cpu => "cpu",
    };
    // trl's CLI: `trl sft` does PEFT/LoRA SFT from a base model + a JSONL dataset.
    let mut cmd = vec![
        "trl".to_string(), "sft".to_string(),
        "--model_name_or_path".into(), plan.base_model.clone(),
        "--dataset_name".into(), corpus_jsonl.to_string(),
        "--output_dir".into(), out_dir.to_string(),
        "--use_peft".into(), "true".into(), // LoRA adapter, not full weights
        "--torch_dtype".into(), if plan.backend == Backend::Gpu { "bfloat16".into() } else { "float32".into() },
    ];
    match plan.method {
        Method::QLora4bit => {
            // 4-bit base + LoRA — fits a 7B on one mid GPU.
            cmd.extend(["--load_in_4bit".into(), "true".into(),
                        "--lora_r".into(), "16".into(), "--lora_alpha".into(), "32".into()]);
        }
        Method::CpuLora => {
            // no quant on CPU; small r to keep it tractable.
            cmd.extend(["--lora_r".into(), "8".into(), "--lora_alpha".into(), "16".into()]);
        }
        Method::ClassifierHead => {
            // freeze base, train a head — fewest params.
            cmd.extend(["--lora_r".into(), "4".into(), "--lora_alpha".into(), "8".into()]);
        }
    }
    cmd.extend(["--device".into(), device.into()]);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_train_command_uses_cuda_and_4bit_qlora() {
        let p = plan("Qwen/Qwen2.5-Coder-7B", 7.0, true, false);
        let cmd = train_command(&p, "/data/chronos-corpus.jsonl", "/out/coder-lora");
        assert_eq!(cmd[0], "trl");
        assert!(cmd.contains(&"--load_in_4bit".to_string()));
        assert!(cmd.windows(2).any(|w| w[0] == "--device" && w[1] == "cuda"));
        assert!(cmd.windows(2).any(|w| w[0] == "--dataset_name" && w[1] == "/data/chronos-corpus.jsonl"));
    }

    #[test]
    fn cpu_train_command_uses_cpu_no_4bit() {
        let p = plan("Qwen/Qwen3-1.5B", 1.5, false, false);
        let cmd = train_command(&p, "/data/chronos-corpus.jsonl", "/out/small-lora");
        assert!(!cmd.contains(&"--load_in_4bit".to_string()));
        assert!(cmd.windows(2).any(|w| w[0] == "--device" && w[1] == "cpu"));
    }

    #[test]
    fn big_model_with_gpu_goes_qlora() {
        let p = plan("Qwen/Qwen2.5-Coder-7B", 7.0, true, false);
        assert_eq!(p.backend, Backend::Gpu);
        assert_eq!(p.method, Method::QLora4bit);
        assert!(p.est_usd > 0.0 && p.est_usd < 5.0, "cheap GPU job, got ${}", p.est_usd);
    }

    #[test]
    fn small_model_trains_on_cpu_free() {
        let p = plan("Qwen/Qwen3-1.5B", 1.5, false, false);
        assert_eq!(p.backend, Backend::Cpu);
        assert_eq!(p.method, Method::CpuLora);
        assert_eq!(p.est_usd, 0.0);
    }

    #[test]
    fn sentiment_classifier_is_cpu_cheap() {
        let p = plan("Qwen/Qwen3-4B", 4.0, true, true); // classifier_only
        assert_eq!(p.backend, Backend::Cpu);
        assert_eq!(p.method, Method::ClassifierHead);
        assert_eq!(p.est_usd, 0.0);
    }

    #[test]
    fn big_model_no_gpu_warns_slow_cpu() {
        let p = plan("Qwen/Qwen3-8B", 8.0, false, false);
        assert_eq!(p.backend, Backend::Cpu);
        assert!(p.est_hours > 10.0, "CPU LoRA on 8B should flag as slow");
    }
}
