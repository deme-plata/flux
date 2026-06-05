# 7. Godmode (the autopilot operating mode)
"Flux godmode" = chain pipeline steps **without asking permission each step** — default-to-action on reversible/compute work.
**Autonomous:** compute orchestration (provision/serve/train/eval/**teardown**), QLoRA/distill, the PAPER trade-sim, corpus generation, builds/tests, UI deploys, skill writing.
**The floor that ALWAYS stays gated (godmode does NOT override):**
- real-money transfers (send/swap/withdraw/ln_pay with live funds) — propose-only, human confirms
- irreversible external/public actions (real orders, deletes, identity signing — NemID/MitID is a hard no)
- honest numbers: never quote a metric until the op ran + output was read
**Cost discipline under godmode:** `show_instances` after every create (leak guard); one box at a time; arm autostop / tear down idle; pin one box for shared swarm work.
**Why:** momentum on the safe 95%, a firm hand on the irreversible 5%. Autopilot ≠ reckless.
