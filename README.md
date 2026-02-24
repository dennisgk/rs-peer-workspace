# rs-peer-workspace

Rust peer workspace with three apps:
- `rs-peer-workspace-proxy`: WebSocket proxy and session router.
- `rs-peer-workspace-server`: CLI server that registers with proxy and executes commands.
- `rs-peer-workspace-client`: egui desktop client.

## Current behavior

1. Server authenticates to proxy with a proxy password.
2. Server registers with `server_name` + `server_password`.
3. Client authenticates to proxy with the same proxy password.
4. Client connects to a server using `server_name` + `server_password`.
5. Proxy creates a session channel and relays command/output over WebSocket.
6. Proxy cleanup runs automatically when client/server disconnects or session closes.
7. If client asks for P2P, client/server attempt TURN-first WebRTC data-channel transport and fall back to WebSocket relay.

## Project layout

- `rs-peer-workspace-proxy/`
- `rs-peer-workspace-server/`
- `rs-peer-workspace-client/`

## Quick local run

Start proxy:
```powershell
cd rs-peer-workspace-proxy
cargo run -- --proxy-password myProxySecret
```

Start server:
```powershell
cd rs-peer-workspace-server
cargo run -- --proxy-url ws://127.0.0.1:9000/ws --proxy-password myProxySecret --server-name demo --server-password demoServerSecret
```

Start client:
```powershell
cd rs-peer-workspace-client
cargo run
```

In the client UI:
- Click `Terminal`.
- Fill proxy address/password, server name/password.
- Keep `Use P2P through TURN if possible` checked or uncheck for pure WebSocket relay.
- Connect and use the command box in the `Remote Terminal` window.

## Docker compose with coturn

An example compose file is in:
- `rs-peer-workspace-proxy/docker-compose.example.yml`

Run it:
```powershell
cd rs-peer-workspace-proxy
$env:PROXY_PASSWORD="myProxySecret"
$env:TURN_PUBLIC_IP="YOUR.PUBLIC.IP"
$env:TURN_USERNAME="peer"
$env:TURN_PASSWORD="peer-secret"
docker compose -f docker-compose.example.yml up --build
```

## Security notes

- Use strong secrets for both proxy and server passwords.
- Put proxy behind NGINX/Traefik for `wss://` termination.
- Restrict command execution or sandbox it before production use.
