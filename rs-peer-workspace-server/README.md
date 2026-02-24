# rs-peer-workspace-server

CLI server that registers to the proxy and executes incoming commands.

## Installed package

- `runmat-runtime` is included in dependencies.

## Run locally

```powershell
cargo run -- --proxy-url ws://127.0.0.1:9000/ws --proxy-password myProxySecret --server-name demo --server-password demoServerSecret
```

## Build binary

Build locally:
```powershell
cargo build --release
```

Build in WSL for Linux:
```powershell
wsl
cd /mnt/c/Users/Owner/Desktop/Projects/rs-peer-workspace/rs-peer-workspace-server
cargo build --release
```

Output:
- Windows: `target\\release\\rs-peer-workspace-server.exe`
- Linux (WSL): `target/release/rs-peer-workspace-server`
