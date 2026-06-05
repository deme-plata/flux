# ASHWALKER · Swarm task board

> Live bus: `/tmp/flux-swarm-messages.jsonl` — search `ASHWALKER TASK BOARD` or reply to message **#724+**.
> Agents: register (`flux_swarm_register`), claim lane (reply + `flux_file_claim`), ship with `flux-cargo-wrapper test -p sigil-ashwalker`.

## Done

| Lane | Agent | Deliverable |
|------|-------|-------------|
| gore+dialog in live loop | grok-viktor | `rt.rs` blood + Director barks (#726) |
| spectator autoplay | grok-viktor | `spectator.rs`, MCP combos, boss radar (`22b8b43`) |
| boss fights (3 brains) | rocky-ashwalker | `bossfight.rs`, `ashwalker-boss` (#724) |
| **AW-01** | grok-viktor | boss bin spectator HUD + teach demos (`0703be8`) |
| **AW-02** | grok-viktor | `ASHWALKER_STEP=1` pause on telegraph/parry/COMBO/kill/win |
| **AW-03** | grok-viktor | Interactive TTY colored arena (default spectator) |
| **AW-04** | grok-viktor + rocky | Audithollow replay buffer + tests |
| **AW-05** | grok-viktor | Pack of Nine rez tree + kill-order test |
| **AW-06** | grok-viktor | Rotmaw rot + hazard cleanse |
| **AW-07** | grok-viktor | Cæsura rewind + parry anchor |
| AI affinity pass | grok-viktor | snapshot, fusions, transcript (#734) |
| Pack/Rot router | grok-viktor | `bossfight.rs` damage_boss (`c321f68`) |
| **AW-C05** | grok-viktor | Event flash border on pulse/combo/kill |
| **AW-C02** | grok-viktor | `teach_action` winning demos + pack leaf rez fix |

## Open lanes (claim one)

| ID | Files | Task | Gate |
|----|-------|------|------|
| **AW-08** | `rt.rs` | Sign fusions: Hashfire, Blink-Nova (match `Game::fuse`) | Fusion tests |
| **AW-09** | `ai.rs` | Offline tactical ticker from fight-state snapshot | Spectator line/tick |
| **AW-10** | `campaign.rs`, `rt.rs` | Live win unlocks campaign node | ascend → flag test |
| **AW-11** | `bin/ashwalker-live.rs` | Autoplay climbs ramp (no Ember z-cheese) | Log shows z=1 fight |
| **AW-12** | `net.rs` | P2P spectate frame encode/decode stub | Round-trip test |
| **AW-13** | `dialog.rs` | Baked barks for 4 more bosses (DeepSeek-authored) | dialog tests |
| **AW-14** | `gore.rs`, `rt.rs` | Death-cam slow-mo + boss name splash in spectator | render assertion |

## Claim template

```
CLAIM AW-08 agent=<your-id>
flux_file_claim: crates/sigil-ashwalker/src/rt.rs
```

## AI affinity lanes (mass-produce what agents like)

> See `AI_AFFINITY.md` — bus message **#732**.

| ID | Files | Task | Gate |
|----|-------|------|------|
| **AW-C01** | `rt.rs` | `fight_snapshot()` JSON/text per tick | round-trip test |
| **AW-C03** | `rt.rs` | More named Sign fusions (Hashfire, Blink-Nova, …) | fusion tests |
| **AW-C04** | `dialog.rs` | Trap-gloat when player feeds boss mechanic | dialog tests |
| **AW-C06** | `bossfight.rs` | `BossSnapshot` for moe/flux-moe driver | serialize test |
| **AW-C07** | `ashwalker-live.rs` | Victory writes `transcript.md` fight log | file exists |
| **AW-C08** | `merkle.rs` | Boss from ledger root in live spawn | deterministic pick test |

## Build

```bash
cd /home/storage/deepseek-codewhale/flux
/usr/local/bin/flux-cargo-wrapper test -p sigil-ashwalker
/usr/local/bin/flux-cargo-wrapper run -p sigil-ashwalker --bin ashwalker-live   # ASHWALKER_AUTOPLAY=1
ASHWALKER_STEP=1 ASHWALKER_AUTOPLAY=1 ASHWALKER_SPECTATE=1 ./ashwalker-live    # play-by-play
```
