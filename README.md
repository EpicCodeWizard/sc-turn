# ScreenExtend TURN server

A small, standalone TURN relay (built on the same `webrtc-rs` `turn` crate the
desktop app uses). Deploy it on a public host, point `turn.screenextend.app` at
it, and cross-network sessions stop hanging on **Connecting**.

## Why it's needed

The cloud relay (`session.screenextend.app`) only tunnels *signaling*. The video
is direct peer-to-peer WebRTC. When the host and the joining browser are on
different networks, STUN alone usually can't punch through NAT, so ICE never
connects. A TURN server gives the **host** a public *relay candidate* that the
remote browser can always reach. Only one side needs to relay — that's the host.

## Credentials (no accounts, no database)

This uses the long-term-credential / "coturn REST API" scheme. The host app mints
a short-lived credential per session from a **shared secret**; this server
validates it with the same secret. There are no stored users. Keep the secret
private — it's only ever on the host app and this server, never in a browser.

## Build

```sh
cargo build --release
# binary: target/release/screenextend-turn
```

## Run

Configuration is entirely via environment variables:

| Variable          | Required | Default              | Meaning                                                        |
|-------------------|----------|----------------------|----------------------------------------------------------------|
| `TURN_PUBLIC_IP`  | yes      | —                    | The public address clients reach this box at (the relay address). Accepts a literal IP **or a hostname** — a hostname is resolved to an IP via DNS at startup. |
| `TURN_SECRET`     | yes      | —                    | Shared secret; must equal the host app's `SCREENEXTEND_TURN_SECRET`. |
| `TURN_REALM`      | no       | `screenextend.app`   | TURN realm.                                                    |
| `TURN_PORT`       | no       | `3478`               | UDP listen port.                                               |
| `TURN_LISTEN_IP`  | no       | `0.0.0.0`            | Local bind address.                                            |

```sh
export TURN_PUBLIC_IP=203.0.113.10      # a literal public IP …
# …or, if you don't have/know the IP (PaaS hosts), a hostname that resolves to it:
export TURN_PUBLIC_IP=turn.screenextend.app
export TURN_SECRET='a-long-random-shared-secret'
./target/release/screenextend-turn
```

> The relay candidate the server hands out **must** be the address clients can
> actually reach. If you pass a hostname, it's resolved once at startup (IPv4
> preferred) and that IP is used. The startup log prints the resolved address.

Generate a secret once and reuse it on both sides, e.g.
`openssl rand -hex 32`.

### systemd unit (example)

```ini
[Unit]
Description=ScreenExtend TURN
After=network-online.target

[Service]
Environment=TURN_PUBLIC_IP=203.0.113.10
Environment=TURN_SECRET=a-long-random-shared-secret
Environment=RUST_LOG=info
ExecStart=/opt/screenextend-turn/screenextend-turn
Restart=always

[Install]
WantedBy=multi-user.target
```

### Railway (and other PaaS) deployment

On Railway you never get a static public IP — the container only sees a private
address and the real public address is reachable through a forwarded domain.
That's fine: set `TURN_PUBLIC_IP` to the **public hostname** and the server
resolves it to the reachable IP at startup.

1. Point `turn.screenextend.app` (CNAME) at your Railway-provided domain, or use
   Railway's TCP/UDP proxy domain directly.
2. Set service variables:
   - `TURN_PUBLIC_IP=turn.screenextend.app`
   - `TURN_SECRET=<your-shared-secret>`
   - `TURN_PORT=<the port Railway exposes>` (Railway injects a `PORT`; set
     `TURN_PORT` to it, e.g. `TURN_PORT=${{PORT}}`).
3. Confirm the startup log shows `Resolved TURN_PUBLIC_IP '…' -> <ip> via DNS`.

> **UDP caveat:** TURN relay here is UDP, and the relay also needs the high
> ephemeral UDP port range open. Railway's standard HTTP/TCP proxy does **not**
> forward arbitrary inbound UDP, so on Railway you must use a plan/feature that
> exposes raw UDP (or a UDP TCP-proxy) — otherwise media won't relay even though
> the address resolves. A host that gives you direct UDP ingress (a small VPS,
> Fly.io with a dedicated IP, etc.) is the simpler fit for a UDP TURN relay.

### DNS / firewall

- Point `turn.screenextend.app` (A/AAAA) at this host's public IP.
- Open **UDP 3478** inbound, plus the ephemeral UDP relay port range your OS
  hands out (the relay allocates high UDP ports for media). On a cloud firewall,
  allowing inbound UDP `49152–65535` is typical.

## Wiring the host app

Set the matching secret on the machine running the ScreenExtend desktop app:

```sh
# Windows (PowerShell), system or user environment:
setx SCREENEXTEND_TURN_SECRET "a-long-random-shared-secret"
```

That's all that's required — the app already defaults its TURN urls to
`turn.screenextend.app:3478` (udp+tcp) and auto-enables TURN once the secret is
present. Overrides if you need them:

- `SCREENEXTEND_TURN_URLS` — comma-separated TURN urls (default `turn.screenextend.app`).
- `SCREENEXTEND_TURN_TTL` — credential lifetime in seconds (default `600`).

The standalone streamer binary also accepts `--turn-secret`, `--turn-urls`, and
`--turn-ttl`.

## Verifying it works

In the host app logs, the per-join `ICE server configured (...)` line will now
include a `turn:` url with `has_creds=true`, and the peer connection state will
progress past `Connecting` to `Connected`. You can also test the server directly
with [`turnutils_uclient`](https://github.com/coturn/coturn) or the
[Trickle ICE](https://webrtc.github.io/samples/src/content/peerconnection/trickle-ice/)
page using a credential pair printed by the app.

## Scope / limits

This implementation listens on **UDP**. UDP relay covers the large majority of
cross-network NAT cases. If you need to traverse networks that block UDP
entirely (some strict corporate/guest Wi-Fi), front this with TURN-over-TLS on
TCP 443 (e.g. terminate TLS at a TCP proxy, or extend `main.rs` with a TCP
listener) — the credential scheme is unchanged.
