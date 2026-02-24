# rs-peer-workspace-client

egui desktop client for connecting to server sessions through the proxy.

## Installed package

- `runmat-runtime` is included in dependencies.

## Run locally

```powershell
cargo run
```

UI flow:
- Top toolbar has `Server` dropdown and `Terminal` button.
- `Terminal` opens connection dialog with:
  - proxy address
  - proxy password
  - server name
  - server password
  - `Use P2P through TURN if possible` (checked by default)
- On success, `Remote Terminal` window opens for command input/output.

## Build binary

Build locally:
```powershell
cargo build --release
```

Output:
- Windows: `target\\release\\rs-peer-workspace-client.exe`
- Linux/macOS: `target/release/rs-peer-workspace-client`
