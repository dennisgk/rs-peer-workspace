# rs-peer-workspace-server

CLI server that registers to the proxy and executes incoming commands.

## Installed package

- `runmat-runtime` is included in dependencies.

## Run locally

```powershell
cargo run -- --proxy-url ws://127.0.0.1:9000/ws --proxy-password myProxySecret --server-name demo --server-password demoServerSecret
```

## Binary build Dockerfile

`Dockerfile.build` compiles release binaries and puts them in `/out`.

Build for Linux x64:
```powershell
docker build -f Dockerfile.build --build-arg TARGET=x86_64-unknown-linux-gnu -t rs-peer-server-build .
docker run --rm -v ${PWD}/out:/out rs-peer-server-build
```

Build for Windows x64 (GNU target):
```powershell
docker build -f Dockerfile.build --build-arg TARGET=x86_64-pc-windows-gnu -t rs-peer-server-build-win .
docker run --rm -v ${PWD}/out-win:/out rs-peer-server-build-win
```

Note: non-native targets may require additional target toolchains depending on platform and dependency requirements.
Use Rust 1.93+ for this Docker build because `runmat-runtime` dependencies require newer integer APIs.
