//! fluxc-webhooks — outbound/inbound webhook machinery (v0.41 split).
//! Lower layers fire events through fluxc_util::hooks; the binary wires
//! `webhook::auto_dispatch` in as the sink at startup.
pub mod swarm;
pub mod webhook;
pub mod webhook_inbound;
pub mod webhook_ssrf;
