//! Client side of the IronTunnel v0.0.2 control handshake, extracted so the CLI client,
//! the GUI and other apps (ZenTorrent) share ONE implementation instead of the eleven
//! hand-rolled copies that lived in upstream `src/bin/`.
//!
//! The wire, as `irontunnel-server` speaks it (src/bin/irontunnel-server.rs,
//! `perform_zk_authentication` + the client loop). A "frame" is a u32 big-endian length
//! followed by that many bytes.
//!
//! ```text
//!   server → frame "ZK_AUTH_REQUIRED"
//!   server → frame challenge            (static: b"irontunnel_zk_challenge_2024")
//!   client → frame proof                (≤ 1024 bytes; the server never inspects it)
//!   server → frame "ZK_AUTH_SUCCESS"
//!   client → RAW  "CLIENT_AUTH:<id>"    (NOT framed — the server matches a raw read)
//!   server → frame "AUTH_ACK:<id>"
//!   client → frame JSON TunnelInit
//!   server → frame JSON TunnelReady     (assigned 10.8.0.x from the server's IpPool)
//! ```
//!
//! What is still pretend, said plainly: the "ZK proof" is a constant string and the
//! server accepts any bytes, there is no key exchange and no encryption, and nothing
//! after `TunnelReady` carries packets yet. This module proves the two ends agree on
//! the bytes. It does not make the link private. See PORT.md.

use crate::packet_tunnel::TunnelMessage;
use crate::{IronTunnelError, Result};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Largest frame either side accepts (the server caps the proof at 1024; the shipped
/// GUI capped every frame at 64 KiB).
pub const MAX_FRAME: u32 = 64 * 1024;

/// The bytes every upstream client sent as its "proof". Kept so the port stays
/// wire-compatible with v0.0.2 servers; it authenticates nothing.
pub const LEGACY_MOCK_PROOF: &[u8] = b"mock_zk_proof_response";

/// What the server told us once the tunnel was set up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeOutcome {
    pub client_id: String,
    pub challenge: Vec<u8>,
    pub server_ip: String,
    pub assigned_ip: String,
    pub dns_servers: Vec<String>,
}

pub async fn read_frame<S: AsyncRead + Unpin>(stream: &mut S) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).await.map_err(net)?;
    let len = u32::from_be_bytes(len);
    if len > MAX_FRAME {
        return Err(IronTunnelError::Network(format!("frame too large: {len} bytes")));
    }
    let mut buf = vec![0u8; len as usize];
    stream.read_exact(&mut buf).await.map_err(net)?;
    Ok(buf)
}

pub async fn write_frame<S: AsyncWrite + Unpin>(stream: &mut S, data: &[u8]) -> Result<()> {
    let len = u32::try_from(data.len())
        .ok()
        .filter(|l| *l <= MAX_FRAME)
        .ok_or_else(|| IronTunnelError::Network(format!("frame too large: {} bytes", data.len())))?;
    stream.write_all(&len.to_be_bytes()).await.map_err(net)?;
    stream.write_all(data).await.map_err(net)?;
    stream.flush().await.map_err(net)
}

/// Run the whole v0.0.2 handshake on an already-connected stream, up to and including
/// `TunnelReady`. Every read is bounded by `step_timeout`.
pub async fn client_handshake<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    client_id: &str,
    step_timeout: Duration,
) -> Result<HandshakeOutcome> {
    let required = timed(step_timeout, read_frame(stream)).await?;
    expect(&required, "ZK_AUTH_REQUIRED")?;

    let challenge = timed(step_timeout, read_frame(stream)).await?;

    write_frame(stream, LEGACY_MOCK_PROOF).await?;
    let verdict = timed(step_timeout, read_frame(stream)).await?;
    expect(&verdict, "ZK_AUTH_SUCCESS")?;

    // The server reads CLIENT_AUTH with a raw read, so it must go unframed — and the
    // TunnelInit below must wait for AUTH_ACK, or TCP may coalesce the two writes into
    // one read and the server would see neither.
    stream
        .write_all(format!("CLIENT_AUTH:{client_id}").as_bytes())
        .await
        .map_err(net)?;
    stream.flush().await.map_err(net)?;
    let ack = timed(step_timeout, read_frame(stream)).await?;
    expect(&ack, &format!("AUTH_ACK:{client_id}"))?;

    let init = TunnelMessage::TunnelInit {
        client_id: client_id.to_string(),
        client_ip: "0.0.0.0".to_string(),
        capabilities: vec!["ipv4".to_string()],
    };
    let init = serde_json::to_vec(&init).map_err(|e| IronTunnelError::Network(e.to_string()))?;
    write_frame(stream, &init).await?;

    let ready = timed(step_timeout, read_frame(stream)).await?;
    match serde_json::from_slice::<TunnelMessage>(&ready) {
        Ok(TunnelMessage::TunnelReady { server_ip, assigned_ip, dns_servers }) => Ok(HandshakeOutcome {
            client_id: client_id.to_string(),
            challenge,
            server_ip,
            assigned_ip,
            dns_servers,
        }),
        Ok(other) => Err(IronTunnelError::Network(format!("expected TunnelReady, got {other:?}"))),
        Err(e) => Err(IronTunnelError::Network(format!(
            "expected TunnelReady JSON, got {:?}: {e}",
            String::from_utf8_lossy(&ready)
        ))),
    }
}

fn expect(got: &[u8], want: &str) -> Result<()> {
    if got == want.as_bytes() {
        Ok(())
    } else {
        Err(IronTunnelError::Network(format!(
            "protocol: expected {want:?}, got {:?}",
            String::from_utf8_lossy(got)
        )))
    }
}

async fn timed<T>(d: Duration, f: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(d, f)
        .await
        .map_err(|_| IronTunnelError::Network(format!("timed out after {d:?}")))?
}

fn net(e: std::io::Error) -> IronTunnelError {
    IronTunnelError::Network(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for the server's side of the wire, step for step as in
    /// irontunnel-server.rs. It checks the client against the protocol as written; the
    /// real server is exercised by the container test in PORT.md, not here.
    async fn mirror_server<S: AsyncRead + AsyncWrite + Unpin>(mut s: S) -> Result<Vec<u8>> {
        write_frame(&mut s, b"ZK_AUTH_REQUIRED").await?;
        write_frame(&mut s, b"irontunnel_zk_challenge_2024").await?;
        let proof = read_frame(&mut s).await?;
        write_frame(&mut s, b"ZK_AUTH_SUCCESS").await?;
        let mut raw = [0u8; 1024];
        let n = s.read(&mut raw).await.map_err(net)?;
        let auth = String::from_utf8_lossy(&raw[..n]).to_string();
        let id = auth.strip_prefix("CLIENT_AUTH:").expect("raw CLIENT_AUTH").to_string();
        write_frame(&mut s, format!("AUTH_ACK:{id}").as_bytes()).await?;
        let init: TunnelMessage = serde_json::from_slice(&read_frame(&mut s).await?).unwrap();
        assert!(matches!(init, TunnelMessage::TunnelInit { ref client_id, .. } if *client_id == id));
        let ready = TunnelMessage::TunnelReady {
            server_ip: "10.8.0.1".into(),
            assigned_ip: "10.8.0.2".into(),
            dns_servers: vec!["10.8.0.1".into(), "1.1.1.1".into()],
        };
        write_frame(&mut s, &serde_json::to_vec(&ready).unwrap()).await?;
        Ok(proof)
    }

    #[tokio::test]
    async fn handshake_round_trip_against_mirror() {
        let (mut client, server) = tokio::io::duplex(4096);
        let srv = tokio::spawn(mirror_server(server));
        let out = client_handshake(&mut client, "zen-1", Duration::from_secs(2)).await.unwrap();
        assert_eq!(out.assigned_ip, "10.8.0.2");
        assert_eq!(out.server_ip, "10.8.0.1");
        assert_eq!(out.challenge, b"irontunnel_zk_challenge_2024");
        assert_eq!(srv.await.unwrap().unwrap(), LEGACY_MOCK_PROOF);
    }

    #[tokio::test]
    async fn rejects_wrong_greeting() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move { write_frame(&mut server, b"HELLO").await });
        let err = client_handshake(&mut client, "x", Duration::from_secs(2)).await.unwrap_err();
        assert!(err.to_string().contains("ZK_AUTH_REQUIRED"), "{err}");
    }

    #[tokio::test]
    async fn rejects_oversized_frame() {
        let (mut client, mut server) = tokio::io::duplex(64);
        tokio::spawn(async move { server.write_all(&(MAX_FRAME + 1).to_be_bytes()).await });
        let err = read_frame(&mut client).await.unwrap_err();
        assert!(err.to_string().contains("too large"), "{err}");
    }

    #[tokio::test]
    async fn times_out_on_silent_server() {
        let (mut client, _server) = tokio::io::duplex(64);
        let err = client_handshake(&mut client, "x", Duration::from_millis(50)).await.unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
    }
}
