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

## Binary build Dockerfile

`Dockerfile.build` compiles release binaries and places them in `/out`.

Build for Linux x64:
```powershell
docker build -f Dockerfile.build --build-arg TARGET=x86_64-unknown-linux-gnu -t rs-peer-client-build .
docker run --rm -v ${PWD}/out:/out rs-peer-client-build
```

Build for Windows x64 (GNU target):
```powershell
docker build -f Dockerfile.build --build-arg TARGET=x86_64-pc-windows-gnu -t rs-peer-client-build-win .
docker run --rm -v ${PWD}/out-win:/out rs-peer-client-build-win
```

Note: GUI cross-targets can need extra system packages/toolchains.
Use Rust 1.93+ for this Docker build because `runmat-runtime` dependencies require newer integer APIs.
