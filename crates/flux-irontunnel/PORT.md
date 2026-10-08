# flux-irontunnel — port notes (2026-10-08)

Ported from IronTunnel v0.0.2, Beta backup
`/home/storage/beta-migration/fs/opt/orobit/shared/IronTunnel` (22.7k lines, 21 binaries).
The upstream tree stays untouched there; this crate is the maintained copy.

## The one thing to know first

**Upstream IronTunnel was not a working VPN.** It shipped a client and a server that agree
on a login handshake over plain TCP, and nothing else:

- The "zero-knowledge STARK proof" is the constant string `mock_zk_proof_response`; the
  server reads up to 1024 bytes and never inspects them. The challenge is a constant too.
- There is no key exchange, no encryption and no TLS. Everything is cleartext TCP on :1194.
- No IP packet is ever carried. The shipped GUI dropped the socket right after login and
  displayed a simulated ~0.5 MB/s. The server's `IpPacket` path is dead code.
- The real crypto (x25519, ed25519, ChaCha20-Poly1305, BLAKE3 KDF) and the `tun`-crate TUN
  device lived in modules nothing imported, written against older crate APIs. They never
  compiled. They are kept in `src/orphan/` as raw material and are NOT compiled here.

The port is faithful: it compiles and runs what upstream had. It does not make it private.

## What was measured

- `fluxc test -p flux-irontunnel --features cli`: **30 passed, 1 ignored, 0 failed**
  (15 lib unit incl. 4 new handshake tests, 14 integration, 1 server-bin).
- **Real server ↔ new client, end to end**, in a network-less container
  (`--network none`, stub `iptables`/`sysctl` that only log): full handshake through
  `TunnelReady` in **122 ms**, assigned `10.8.0.2`. Host iptables unchanged (0 rules for
  10.8.0.0/24 before and after). NAT itself was NOT exercised (stubs).

## Module verdicts

| module | verdict | note |
|---|---|---|
| `handshake` (new) | REAL | the one shared client handshake; wire documented in the file |
| `packet_tunnel` | PARTIAL | framing + `TunnelMessage` real; receive path fabricates `TunnelReady` on "UNKNOWN COMMAND"; `forward_to_system` only logs |
| `ip_pool` | REAL | spawns a lease monitor per `assign_ip` (leak-ish) |
| `network_monitor` | REAL (Linux) | `/proc/net/dev`; stub elsewhere |
| `nat_engine` | PARTIAL | raw socket + `sysctl` + iptables MASQUERADE real; return path TODO; never fed packets |
| `dns_forwarder` | PARTIAL | real upstream query/cache; reply port bug; server never starts it |
| `rest_api` (feature `server-api`) | PARTIAL | real warp routes; health/bandwidth/error-rate hardcoded. CPU/memory now real (sysinfo 0.30) |
| `real_api_client` (feature `api`) | REAL | client for `rest_api` |
| `api_client` | MOCK | targets a fictional `api.irontunnel.com`; offline build returns hardcoded server + zero key |
| `vpn_packet_handler` | STATS ONLY | byte counters real; packets returned unchanged |
| `tun_interface` | BROKEN (Linux) / MOCK (Windows) | never calls TUNSETIFF, fd dangles; Windows write only logs. `RouteManager` real |
| `traffic_forwarder` | PARTIAL, unused | |
| `packet_injector` | PARTIAL, unused | Windows raw `sendto`; never constructed |
| `windows_packet_capture` | PARTIAL | SIO_RCVALL sniffer: sees copies, cannot divert — not a VPN data path |
| `lib_working::{crypto,zk,wireguard,multihop,dns,split_tunnel}` | MOCK | XOR "crypto", `verify_proof` always true, string WG keys, DNS → 127.0.0.1, bypass always false, `throughput_mbps = 100.0`, kill switch only logs |

## Binaries

| bin | feature | from upstream | changes in the port |
|---|---|---|---|
| `irontunnel-server` | `cli` | `src/bin/server.rs` | `test_server_creation` `#[ignore]`d: it builds `NatEngine`, which as root runs `sysctl -w net.ipv4.ip_forward=1` and appends 3 iptables rules on the HOST |
| `irontunnel-client` | `cli` | new (README promised it; it never existed) | handshake CLI on `handshake::client_handshake` |
| `irontunnel-gui` | `gui` | `src/bin/gui_fixed_auth.rs` — md5-matched to every shipped Windows exe | default server 161.35.219.10 → 127.0.0.1 (`IRONTUNNEL_SERVER` overrides); simulated traffic → 0 |

Not ported (still in the Beta backup): 18 other `src/bin` variants (3 byte-identical, most
never compiled), root scratch probes, the abandoned Tauri/React frontend, `ui/` duplicates are
copied but only `ui/main_awesome.slint` is compiled. Stale tests and examples that target APIs
that no longer exist are kept uncompiled in `tests/legacy/` and `examples/legacy/`.

## Safety

**Never run `irontunnel-server` on Epsilon's host network.** Its NAT engine edits host
iptables and enables IP forwarding. Run it in a container or its own network namespace.
The last shipped client pointed at Beta (185.182.185.227), which is decommissioned; there is
no live IronTunnel server anywhere today.

## Features

`default = []` is the light core (tokio, serde, rand, libc, sysinfo, uuid, chrono) — what an
app like ZenTorrent links. `api`, `server-api`, `gui`, `cli` add reqwest, warp, slint, clap.

## What it would take to be a real VPN (not started — needs an operator decision)

1. A real handshake: x25519 key exchange, server identity signed with ed25519, ChaCha20-Poly1305
   per-direction keys (salvage `src/orphan/crypto`, rewrite for dalek 2 / chacha20poly1305 0.10).
2. A data path, either
   - **SOCKS5 over the encrypted tunnel** (no admin rights; what ZenTorrent needs: librqbit
     already takes `connect.proxy_url`). Caveat: UDP (DHT, uTP, UDP trackers) does not ride a
     SOCKS5 CONNECT tunnel, so it must be disabled or it leaks; or
   - **full TUN** (Linux `tun` crate + Windows wintun), routes and a real kill switch — much more work.
