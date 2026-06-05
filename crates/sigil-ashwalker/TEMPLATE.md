# ASHWALKER — Flux-native game template (mold)  ·  `sigil-ashwalker`  ·  prototype 6

> A SIGIL terminal ARPG, stamped in the same boilerplate format as the Flux Money-Factory molds:
> ~50 features in 10 thematic clusters, FLUXFOOD discipline (std-only core, libp2p only in the p2p
> adapter), fluxc-verified per module, name-derived heroes, a chronos-style scripted bout + adversarial
> tests. **Content rail, not a money rail:** the run feeds a House into **Crown & Ash** (the strategy
> sibling) — the cross-game **Merkle root** is the shared, committed state between the two games.
>
> Built by the swarm in parallel (one module per lane). State key: ✅ shipped+tested · 🔧 sibling in-flight · ⬜ open lane.

| cluster | crate module(s) |
|---|---|
| A Character & Body · B MCP-Combat · C World · D Enemies/Bosses · E Items · F Companions · G Progression · H Multiplayer · I Skill/AI · J Agent/Render | `traits avatar body / lib(Mcp,combo) skill rt / lib(World) companion campaign / bestiary merkle ai / item / companion / lib(ascend) merkle / net / skill / avatar net dialogue` |

## The 50 features

**A · Character & Body** — 1 deterministic creator (name→FNV seed) ✅ · 2 Origins ×4 ✅ · 3 Traits ×6 (play-changing) ✅ · 4 rich-text 24-bit ANSI avatar ✅ · 5 simulated body (muscle force + fatigue + stamina + reflex + balance) ✅
**B · MCP-Combat** — 6 six MCP spells (flux_combo/dex_swap/…) ✅ · 7 combo fusion (Blink-Nova/Hashfire/Warded-Rally) ✅ · 8 combo action-timers (Perfect/Good/Late/Whiff, agility-gated) ✅ · 9 real-time Witcher-feel combat (dodge/light/heavy/parry/signs) 🔧 · 10 archery: calibrate + aim-while-moving (mounted hardest) + target lead ✅
**C · World & Movement** — 11 3D isometric world ✅ · 12 elevation + ramps (real z) ✅ · 13 terrain (floor/wall/ramp/ash-lava/gate) ✅ · 14 mount travel (2–3 tiles, hazard-crossing) ✅ · 15 campaign map + biomes 🔧
**D · Enemies & Bosses** — 16 bestiary of foes 🔧 · 17 **Merkle boss-gen** (decisions→root→unique boss) ✅ · 18 Merkle proofs (committed weakness-combo) ✅ · 19 adaptive AI brain (DeepSeek-authored) ✅ · 20 **battle-tested learning** — better every fight, asymptotic ✅
**E · Items & Economy** — 21 tiers Ashen<Iron<Sigil<Mythic<Relic ✅ · 22 slots + sets + 2-piece bonuses ✅ · 23 store (low tiers only) ✅ · 24 wild / quest finds ✅ · 25 boss loot (always drops, root-deterministic, **gear-gated** bosses) ✅
**F · Companions** — 26 pets ×4 (regen/dmg/ward/boss-sight) ✅ · 27 mounts ×4 (speed/hazard/favour) ✅ · 28 Merkle-Owl pet reveals boss weakness ✅ · 29 tame/find (deterministic) ✅ · 30 Crown-Elk ascension favour ✅
**G · Progression & Ascension** — 31 xp / level (hp+sigil growth) ✅ · 32 **Crown & Ash ascension** (House/title/crown/ash/levies from run-style) ✅ · 33 cross-game Merkle ledger ✅ · 34 ascension seed → boot a real C&A House ⬜ · 35 C&A turns → `merkle::Decision::crown_ash` (closes the loop) ⬜
**H · Multiplayer** — 36 co-op protocol (NetMsg) ✅ · 37 Session + party roster ✅ · 38 Transport trait + Loopback (tested) ✅ · 39 **flux-p2p gossipsub transport** adapter 🔧 · 40 shared-boss damage sync (protocol) ✅
**I · Skill & AI** — 41 attributes agility/dexterity/strength ✅ · 42 reflex reaction-time (fatigue-scaled) ✅ · 43 aim calibration (converges to floor) ✅ · 44 fast-then-diminishing mastery ✅ · 45 boss dialogue engine (qwen3.6@:11434 proven, deterministic fallback) ⬜
**J · Agent & Render** — 46 isometric ASCII renderer ✅ · 47 ANSI half-block avatar ✅ · 48 MCP-tool surface (every spell IS an MCP tool name) ✅ · 49 `flux_game_scaffold` one-call mold stamp ⬜ · 50 demo bins (`ashwalker` scripted · `ashwalker-live` real-time) ✅🔧

## Build discipline (binding)
- **FLUXFOOD**: std-only core; the ONLY heavy dep (libp2p) lives in the p2p transport adapter, outside the core.
- **fluxc-verified per module** (never raw cargo; `flux_combo` 0/0 = test bin didn't build → `flux_test`). Full-crate green currently blocked by the in-flight `campaign.rs` (sibling) — each shipped module is independently tested.
- **Deterministic + tested**: seeds reproduce heroes/bosses/loot; merkle proofs make decisions auditable; a scripted bout is the chronos happy-path.
- **Cross-game contract**: the Merkle root is the shared seed — Ashwalker writes decisions, Crown & Ash reads them and writes back.

## Scaffold (the mold stamp)
`flux_game_scaffold` (⬜ open, cluster-J #49): one MCP call stamps a fresh `sigil-<name>` ARPG from this mold — name-derived identity, the 10 module skeletons, FLUXFOOD Cargo, a scripted-bout test — so any agent can mint a new game the way `flux_*_scaffold` stamps a money mold.
