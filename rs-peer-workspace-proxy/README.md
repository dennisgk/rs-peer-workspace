# rs-peer-workspace-proxy

WebSocket proxy for routing client/server sessions.

## Features

- Proxy-password auth for both clients and servers.
- Server registration by `server_name` and `server_password`.
- Client connect by `server_name` + `server_password`.
- Session relay for command/output messages.
- Session/channel cleanup on disconnect.
- TURN credential delivery for P2P attempts.

## Run locally

```powershell
cargo run -- --bind 0.0.0.0:9000 --proxy-password myProxySecret --turn-port 3478 --turn-username peer --turn-password peer-secret
```

TURN URL behavior:
- If `--turn-url` is provided, proxy advertises that exact URL.
- Otherwise it resolves public IP at startup (or uses `TURN_PUBLIC_IP` / `PUBLIC_IP`) and advertises `turn:<ip>:<turn-port>`.

## Runtime Dockerfile

Build and run proxy container:
```powershell
docker build -t rs-peer-proxy .
docker run --rm -e TURN_PUBLIC_IP=YOUR.PUBLIC.IP -p 9000:9000 rs-peer-proxy --proxy-password myProxySecret --turn-port 3478 --turn-username peer --turn-password peer-secret
```

## Docker Compose (proxy + coturn)

Use `docker-compose.example.yml`:
```powershell
$env:PROXY_PASSWORD="myProxySecret"
$env:TURN_PUBLIC_IP="YOUR.PUBLIC.IP"
$env:TURN_USERNAME="peer"
$env:TURN_PASSWORD="peer-secret"
docker compose -f docker-compose.example.yml up --build
```
Use a public TURN IP/hostname reachable from both client and server.

## NGINX note

Proxy listens on `ws://` and is intended to be fronted by NGINX/another reverse proxy for `wss://`.
