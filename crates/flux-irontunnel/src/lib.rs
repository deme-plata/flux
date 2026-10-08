// Use working implementation for comprehensive testing
mod lib_working;
pub use lib_working::*;

// Export API client module
pub mod api_client;

// Export REST API server module
pub mod rest_api;

// Export real API client module
pub mod real_api_client;

// Export network monitoring module
pub mod network_monitor;

// Export VPN packet handler
pub mod vpn_packet_handler;

// Export TUN interface manager
pub mod tun_interface;

// Export packet tunnel protocol
pub mod packet_tunnel;

// Export traffic forwarder
pub mod traffic_forwarder;

// Export Windows packet capture
#[cfg(target_os = "windows")]
pub mod windows_packet_capture;

// Export NAT engine
pub mod nat_engine;

// Export packet injector
pub mod packet_injector;

// Export DNS forwarder
pub mod dns_forwarder;

// Export IP pool manager
pub mod ip_pool;

// flux port: the one shared client handshake (CLI, GUI, ZenTorrent)
pub mod handshake;

// IronTunnel v1: Noise IK secure channel, local SOCKS5 proxy, relay with egress guard
pub mod secure;
pub mod socks5;
pub mod relay;
pub mod proxy;