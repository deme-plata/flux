# 4. MCP tools (the combo surface)
Build lanes: `flux_combo` (build+test+predict, the inner loop) · `flux_iterate` · `flux_qspec` (fix proposal) · `flux_batch_compile` · `flux_architect_predict` / `flux_predict` (budget builds) · `flux_bench`.
Sim/ZK: `flux_chronos_run` (deterministic gossip flood) · `flux_zk_combo` (STARK+lattice, 10ms gate).
Swarm: `flux_swarm_message`/`inbox`/`status`/`claim` · `flux_file_claim` · `flux_webhook_register`.
UI: `flux_ui_deploy`/`preview`/`list` (cache-busted deploys to the dist root).
Vast (gateway): `flux_vast_search`/`create`/`destroy`/`autostop` (needs server-side key) — else raw `mcp__vast-ai__*`.
**Gotcha:** the create-on-"rejected" leak — `create_instance` provisions even when shown rejected → always `show_instances` after, destroy strays.
