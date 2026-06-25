//! Standalone TURN relay for ScreenExtend.
//!
//! Deploy this on a host with a public IP and point `turn.screenextend.app` at
//! it. The ScreenExtend desktop app (the "host" that shares its screen) is
//! configured with the *same* shared secret and generates short-lived
//! credentials per session; this server validates them with the long-term
//! credential mechanism (RFC 5389 / the coturn "REST API" scheme), so no
//! per-user accounts or database are needed.
//!
//! Why this fixes the "stuck on Connecting" bug: when the two peers are on
//! different networks, STUN alone often can't punch a path through NAT. With a
//! TURN server the host gathers a *relay* candidate — a public address on this
//! server that forwards packets to the host — which the remote browser can
//! always reach. Only one peer needs to relay, and that's the host.
//!
//! Configuration is via environment variables:
//!   TURN_PUBLIC_IP  (required)  Public address clients reach this box at, handed
//!                               out as the relay address. Accepts either a
//!                               literal IPv4/IPv6 *or a hostname* (e.g.
//!                               "turn.screenextend.app"). A hostname is resolved
//!                               to an IP via DNS at startup — use this on PaaS
//!                               hosts like Railway/Fly/Render where you never see
//!                               the real public IP, only the forwarded domain.
//!   TURN_SECRET     (required)  Shared secret. MUST equal the host app's
//!                               SCREENEXTEND_TURN_SECRET.
//!   TURN_REALM      (optional)  Realm string (default "screenextend.app").
//!   TURN_PORT       (optional)  UDP listen port (default 3478).
//!   TURN_LISTEN_IP  (optional)  Local bind address (default 0.0.0.0).
//!
//! Run: TURN_PUBLIC_IP=turn.screenextend.app TURN_SECRET=... screenextend-turn

use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;

use tokio::net::lookup_host;
use tokio::net::UdpSocket;
use tokio::signal;
use tokio::time::Duration;
use turn::auth::LongTermAuthHandler;
use turn::relay::relay_static::RelayAddressGeneratorStatic;
use turn::server::config::{ConnConfig, ServerConfig};
use turn::server::Server;
use util::vnet::net::Net;

fn require_env(key: &str, hint: &str) -> Result<String, String> {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => Ok(v),
        _ => Err(format!("{key} must be set ({hint})")),
    }
}

fn env_or(key: &str, default: &str) -> String {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => default.to_string(),
    }
}

/// Turn the configured `TURN_PUBLIC_IP` into the IP we hand out as the relay
/// address. A literal IP is used as-is; anything else is treated as a hostname
/// and resolved via DNS.
///
/// This is what makes the server work on PaaS hosts (Railway, Fly, Render, …)
/// where the container only ever sees a private IP and the real public address
/// is reachable only through a forwarded domain like `turn.screenextend.app`.
/// Setting `TURN_PUBLIC_IP` to that domain lets us look up the public IP that
/// clients will actually reach, instead of requiring an IP we can't see.
async fn resolve_public_ip(value: &str) -> Result<IpAddr, String> {
    let value = value.trim();

    // Fast path: already a literal IP address.
    if let Ok(ip) = IpAddr::from_str(value) {
        return Ok(ip);
    }

    // Otherwise treat it as a hostname and resolve it. `lookup_host` needs a
    // port; the value is arbitrary and discarded — we only want the address.
    let mut addrs = lookup_host((value, 0u16))
        .await
        .map_err(|e| format!("TURN_PUBLIC_IP '{value}' is not a valid IP and DNS lookup failed: {e}"))?;

    // Prefer IPv4 (most relay clients are v4); fall back to whatever resolved.
    let resolved: Vec<IpAddr> = addrs.by_ref().map(|s| s.ip()).collect();
    let chosen = resolved
        .iter()
        .find(|ip| ip.is_ipv4())
        .or_else(|| resolved.first())
        .copied()
        .ok_or_else(|| format!("TURN_PUBLIC_IP '{value}' resolved to no addresses"))?;

    log::info!("Resolved TURN_PUBLIC_IP '{value}' -> {chosen} via DNS");
    Ok(chosen)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let public_ip = require_env(
        "TURN_PUBLIC_IP",
        "the public IP *or hostname* clients reach turn.screenextend.app at",
    )?;
    let shared_secret = require_env(
        "TURN_SECRET",
        "must match the host app's SCREENEXTEND_TURN_SECRET",
    )?;
    let realm = env_or("TURN_REALM", "screenextend.app");
    let listen_ip = env_or("TURN_LISTEN_IP", "0.0.0.0");
    let port: u16 = env_or("TURN_PORT", "3478")
        .parse()
        .map_err(|_| "TURN_PORT must be a valid port number")?;

    let relay_address = resolve_public_ip(&public_ip).await?;

    let conn = Arc::new(UdpSocket::bind(format!("{listen_ip}:{port}")).await?);
    log::info!(
        "ScreenExtend TURN relay listening on {listen_ip}:{port}/udp; \
         handing out relay address {relay_address}; realm \"{realm}\""
    );

    let server = Server::new(ServerConfig {
        conn_configs: vec![ConnConfig {
            conn,
            relay_addr_generator: Box::new(RelayAddressGeneratorStatic {
                relay_address,
                address: "0.0.0.0".to_owned(),
                net: Arc::new(Net::new(None)),
            }),
        }],
        realm,
        auth_handler: Arc::new(LongTermAuthHandler::new(shared_secret)),
        // 0 = use the protocol default channel-bind lifetime.
        channel_bind_timeout: Duration::from_secs(0),
        alloc_close_notify: None,
    })
    .await?;

    log::info!("TURN relay ready. Press Ctrl-C to shut down.");
    signal::ctrl_c().await?;
    log::info!("Shutting down TURN relay…");
    server.close().await?;
    Ok(())
}
