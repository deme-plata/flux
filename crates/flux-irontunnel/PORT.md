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

## IronTunnel v1 — the encrypted tunnel (added 2026-10-08, same day)

New modules: `secure` (Noise `IK_25519_ChaChaPoly_BLAKE2s` via `snow`), `socks5`, `relay`,
`proxy`; bins `irontunnel-keygen`, `irontunnel-relay`, `irontunnel-socks`. The v0.0.2 server,
GUI and handshake above are untouched and still pretend.

How it works:
- The client pins the relay's x25519 public key; the relay only answers client keys on its
  allowlist (`--authorized` file). An unknown key gets silence, so the relay is not an open proxy.
- Each SOCKS5 CONNECT = its own TCP connection + Noise handshake to the relay, then encrypted
  frames (≤16 KiB plaintext + 16-byte tag). Empty frame = half-close.
- Domain names travel to the relay unresolved; the relay resolves them.
- Egress guard: the relay resolves, filters, then dials the address it checked. Loopback,
  RFC1918, link-local, CGNAT, ULA, NAT64/6to4-embedded private, port 25 and port 0 are refused.

Measured (2026-10-08, Epsilon, relay + proxy on loopback, dev build):
- `fluxc test -p flux-irontunnel --features cli`: **50 passed, 0 failed** (2 ignored: the
  host-iptables server test and one doc example).
- Real wire capture on the tunnel port during `curl http://example.com` through the proxy:
  `GET /`, `example.com`, `Example Domain` → **0 occurrences**. Control on the relay's egress
  leg (plain HTTP, should be visible): 1 each. Page arrived intact (HTTP 200, 577 B).
- 50 MB from speed.cloudflare.com, 3 runs: direct 57 / 59 / 71 MB/s, tunnel 72 / 54 / 57 MB/s.
  No measurable cost — but the relay was on the SAME host, so this measures crypto+proxy
  overhead only, not a remote relay path.
- Egress guard, live: `127.0.0.1:8080` (Quillon), `127.0.0.1:18181` (SIGIL), `localhost:8080`,
  `169.254.169.254` → all refused with SOCKS code 0x02; the relay never connected.

Still pretend / not done:
- No relay is deployed anywhere. Where it runs (and that it is reachable from the internet) is
  an operator decision.
- UDP is not tunnelled (SOCKS5 UDP ASSOCIATE is refused on purpose).
- No rekeying and no idle timeout on long-lived connections (nonces are u64; no exhaustion risk).
- Relay timing/size metadata is visible to an observer (one TCP connection per proxied stream).

### ZenTorrent / librqbit 9 integration notes (leaks to close)
librqbit takes `SessionOptions.connect.proxy_url = "socks5://127.0.0.1:PORT"` (only the
`socks5` scheme parses). With the proxy set, TCP peer connections and HTTP(S) trackers go
through the tunnel. These do NOT, and must be turned off while tunnelled:
1. DHT (UDP) → `dht: None` / disabled.
2. UDP trackers (`udp://` announce URLs) → librqbit always builds a UDP tracker client; strip
   `udp://` trackers from torrents/magnets on the ZenTorrent side.
3. Incoming listener (TCP/uTP) + UPnP → `listen: None`; incoming peers would reach the real IP.
4. LSD multicast → `disable_local_service_discovery = true`.
5. Residual: reqwest with a `socks5://` proxy resolves tracker HOSTNAMES locally, so the local
   resolver sees which tracker domains are contacted (the traffic itself is tunnelled).
