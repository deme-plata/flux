//! A BLE *central* for Linux/Windows/macOS via btleplug — the laptop side of the SIGIL
//! link when Chrome is not the client (a CLI, a test rig, sigil-top).
//!
//! Scan for the SIGIL service → connect → read INFO → write our hello to RX → subscribe
//! TX → drive a [`Session`]. Every byte on the wire is produced and consumed by the
//! same `sigil::session` the tests run in loopback; this module only moves chunks.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use btleplug::api::{Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::StreamExt;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::sigil::link::Role;
use crate::sigil::session::{Event, Identity, Session, Step};
use crate::sigil::{INFO_UUID, RX_UUID, SERVICE_UUID, TX_UUID};

fn uuid(s: &str) -> Uuid {
    Uuid::parse_str(s).expect("constant UUIDs are valid")
}

/// A SIGIL peripheral seen while scanning.
#[derive(Debug, Clone)]
pub struct Found {
    pub name: Option<String>,
    pub address: String,
    pub rssi: Option<i16>,
    pub(crate) peripheral: Peripheral,
}

/// Scan for `timeout` and return every device advertising the SIGIL service.
pub async fn scan(timeout: Duration) -> Result<Vec<Found>> {
    let manager = Manager::new().await.context("bluetooth manager (is bluez running?)")?;
    let adapter: Adapter = manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no bluetooth adapter"))?;
    adapter
        .start_scan(ScanFilter { services: vec![uuid(SERVICE_UUID)] })
        .await
        .context("start_scan")?;
    tokio::time::sleep(timeout).await;
    let mut out = Vec::new();
    for p in adapter.peripherals().await? {
        let props = p.properties().await?.unwrap_or_default();
        if !props.services.iter().any(|s| *s == uuid(SERVICE_UUID)) {
            continue;
        }
        out.push(Found { name: props.local_name.clone(), address: p.address().to_string(), rssi: props.rssi, peripheral: p });
    }
    let _ = adapter.stop_scan().await;
    Ok(out)
}

/// An open link to one SIGIL peripheral.
pub struct Client {
    peripheral: Peripheral,
    rx: Characteristic,
    pub session: Session,
    notifications: mpsc::Receiver<Vec<u8>>,
    _pump: tokio::task::JoinHandle<()>,
}

impl Client {
    /// Connect, exchange hellos, and return with the session linked. The returned
    /// [`Event::PeerHello`] carries the 6-digit code to show the human.
    pub async fn connect(found: &Found, me: Identity) -> Result<(Self, Event)> {
        let p = found.peripheral.clone();
        p.connect().await.context("connect")?;
        p.discover_services().await.context("discover_services")?;
        let chars = p.characteristics();
        let find = |u: &str| chars.iter().find(|c| c.uuid == uuid(u)).cloned().ok_or_else(|| anyhow!("missing characteristic {u}"));
        let info = find(INFO_UUID)?;
        let rx = find(RX_UUID)?;
        let tx = find(TX_UUID)?;
        if !tx.properties.contains(CharPropFlags::NOTIFY) {
            return Err(anyhow!("TX characteristic is not notifiable"));
        }

        // btleplug does not expose the negotiated MTU; the peripheral's hello hints it.
        let mut session = Session::new(Role::Central, me, 23);
        let info_bytes = p.read(&info).await.context("read INFO")?;
        let ev = session.on_info(&info_bytes).map_err(|e| anyhow!("hello: {e}"))?;

        p.subscribe(&tx).await.context("subscribe TX")?;
        let mut stream = p.notifications().await?;
        let (ntx, nrx) = mpsc::channel(256);
        let tx_uuid = tx.uuid;
        let pump = tokio::spawn(async move {
            while let Some(n) = stream.next().await {
                if n.uuid == tx_uuid && ntx.send(n.value).await.is_err() {
                    break;
                }
            }
        });

        let mut c = Self { peripheral: p, rx, session, notifications: nrx, _pump: pump };
        let hello = c.session.start().map_err(|e| anyhow!("{e}"))?;
        c.write_all(&hello).await?;
        Ok((c, ev))
    }

    /// Write chunks to RX in order. Without-response when the peripheral allows it —
    /// that is what makes a 200 KB relay take seconds rather than minutes.
    pub async fn write_all(&self, chunks: &[Vec<u8>]) -> Result<()> {
        let wt = if self.rx.properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE) {
            WriteType::WithoutResponse
        } else {
            WriteType::WithResponse
        };
        for c in chunks {
            self.peripheral.write(&self.rx, c, wt).await.context("write RX")?;
        }
        Ok(())
    }

    /// Wait for the next complete message from the peripheral and return its events,
    /// after writing any automatic replies (e.g. a refusal ack).
    pub async fn next_events(&mut self, timeout: Duration) -> Result<Vec<Event>> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let chunk = tokio::time::timeout_at(deadline, self.notifications.recv())
                .await
                .map_err(|_| anyhow!("timed out waiting for the peripheral"))?
                .ok_or_else(|| anyhow!("notification stream closed"))?;
            let Step { events, outgoing } = self.session.on_chunk(&chunk);
            if !outgoing.is_empty() {
                self.write_all(&outgoing).await?;
            }
            if !events.is_empty() {
                return Ok(events);
            }
        }
    }

    /// Hand a coin over and wait for the ack. `Ok(true)` only when the peer says it
    /// stored the coin — the moment the money has left this side.
    pub async fn give_coin(&mut self, uri: &str, memo: Option<String>, timeout: Duration) -> Result<bool> {
        let (id, chunks) = self.session.send_coin(uri, memo).map_err(|e| anyhow!("{e}"))?;
        self.write_all(&chunks).await?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            for ev in self.next_events(left).await? {
                match ev {
                    Event::CoinAcked { id: got, ok, err } if got == id => {
                        if let Some(e) = err {
                            tracing_warn(&e);
                        }
                        return Ok(ok);
                    }
                    Event::Fatal(e) => return Err(anyhow!("session died: {e}")),
                    _ => {}
                }
            }
        }
    }

    pub async fn disconnect(mut self) -> Result<()> {
        if let Ok(bye) = self.session.bye() {
            let _ = self.write_all(&bye).await;
        }
        self.peripheral.disconnect().await.context("disconnect")
    }
}

fn tracing_warn(msg: &str) {
    eprintln!("sigil-ble: peer refused: {msg}");
}
