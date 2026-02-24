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
cargo run -- --bind 0.0.0.0:9000 --proxy-password myProxySecret --turn-url turn:127.0.0.1:3478 --turn-username peer --turn-password peer-secret
```

## Runtime Dockerfile

Build and run proxy container:
```powershell
docker build -t rs-peer-proxy .
docker run --rm -p 9000:9000 rs-peer-proxy --proxy-password myProxySecret --turn-url turn:coturn:3478 --turn-username peer --turn-password peer-secret
```

## Docker Compose (proxy + coturn)

Use `docker-compose.example.yml`:
```powershell
$env:PROXY_PASSWORD="myProxySecret"
$env:TURN_USERNAME="peer"
$env:TURN_PASSWORD="peer-secret"
docker compose -f docker-compose.example.yml up --build
```

## NGINX note

Proxy listens on `ws://` and is intended to be fronted by NGINX/another reverse proxy for `wss://`.
