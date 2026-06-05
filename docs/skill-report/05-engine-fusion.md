# 5. Engine fusion (the business model)
Bootstrap revenue without GPU burn:
- **Phase 1 (now):** engine = **Claude Code Max $200/mo** (already paid), Opus 4.8 max-effort. A **governor caps each customer at 20%** of the plan → ~5 customers share it fairly.
- **Pricing:** Starter $49 · Pro $199 · Business $499. Break-even = **2 Pro**. 4S+2P+1B → +$893/mo profit.
- **Billing:** CVR invoice + 25% moms (`flux-market::invoice`), customer pays your account — agent never auto-collects.
- **Phase 2 (later):** when a customer's volume passes the self-host break-even (~1B tok/mo, `flux-market::cost_model`: self-host $0.15 vs DeepSeek API $0.69 /Mtok), onboard them to their **own DeepSeek agent**. Reinvest, don't pre-spend.
