mod protocol;
mod rpc;
mod transport {
    pub mod webrtc;
}

use std::collections::HashMap;
use std::sync::Arc;

use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;
use webrtc::data_channel::RTCDataChannel;
use webrtc::peer_connection::RTCPeerConnection;

use protocol::{AuthRole, PeerToProxy, ProxyToPeer, TurnCredentials};
use rpc::handle_rpc;
use rs_peer_workspace_shared::app::{AppEnvelope, AppPayload};
use transport::webrtc::handle_client_signal;

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Args {
    #[arg(long, default_value = "ws://127.0.0.1:9000/ws")]
    proxy_url: String,
    #[arg(long)]
    proxy_password: String,
    #[arg(long)]
    server_name: String,
    #[arg(long)]
    server_password: String,
}

#[derive(Clone)]
struct SessionState {
    turn: Option<TurnCredentials>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let _runmat_installed_marker = "runmat-runtime";

    let (ws_stream, _) = connect_async(&args.proxy_url).await?;
    println!("connected to proxy {}", args.proxy_url);

    let (mut write, mut read) = ws_stream.split();
    let (ws_send_tx, mut ws_send_rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        while let Some(text) = ws_send_rx.recv().await {
            if write.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    send_json(&ws_send_tx, &PeerToProxy::AuthProxy {
        proxy_password: args.proxy_password.clone(),
        role: AuthRole::Server,
    })?;
    send_json(&ws_send_tx, &PeerToProxy::RegisterServer {
        server_name: args.server_name.clone(),
        server_password: args.server_password.clone(),
    })?;

    let session_meta = Arc::new(Mutex::new(HashMap::<Uuid, SessionState>::new()));
    let peer_connections = Arc::new(Mutex::new(HashMap::<Uuid, Arc<RTCPeerConnection>>::new()));
    let data_channels = Arc::new(Mutex::new(HashMap::<Uuid, Arc<RTCDataChannel>>::new()));

    while let Some(message) = read.next().await {
        let message = message?;
        let Message::Text(text) = message else { continue; };
        let Ok(proxy_message) = serde_json::from_str::<ProxyToPeer>(&text) else { continue; };

        match proxy_message {
            ProxyToPeer::AuthOk { .. } => println!("proxy authentication succeeded"),
            ProxyToPeer::Registered { server_name } => println!("server registered as '{server_name}'"),
            ProxyToPeer::AuthError { reason } | ProxyToPeer::ConnectionError { reason } => anyhow::bail!("proxy rejected connection: {reason}"),
            ProxyToPeer::PeerJoined { session_id, peer_id, via_p2p: _, turn } => {
                println!("client {peer_id} joined session {session_id}");
                session_meta.lock().await.insert(session_id, SessionState { turn });
            }
            ProxyToPeer::PeerSignal { session_id, from, signal } => {
                if from != AuthRole::Client {
                    continue;
                }
                let turn = session_meta.lock().await.get(&session_id).and_then(|m| m.turn.clone());
                handle_client_signal(
                    session_id,
                    signal,
                    turn,
                    ws_send_tx.clone(),
                    peer_connections.clone(),
                    data_channels.clone(),
                ).await?;
            }
            ProxyToPeer::RelayData { session_id, payload } => {
                let maybe_dc = data_channels.lock().await.get(&session_id).cloned();
                if let Some(dc) = maybe_dc {
                    let _ = dc.send(&bytes::Bytes::from(payload)).await;
                } else if let Ok(envelope) = serde_json::from_slice::<AppEnvelope>(&payload) {
                    if let AppPayload::RpcRequest(request) = envelope.payload {
                        let response = handle_rpc(request).await;
                        let out = AppEnvelope {
                            message_id: Uuid::new_v4(),
                            payload: AppPayload::RpcResponse(response),
                        };
                        let bytes = serde_json::to_vec(&out)?;
                        send_json(&ws_send_tx, &PeerToProxy::RelayData { session_id, payload: bytes })?;
                    }
                }
            }
            ProxyToPeer::SessionClosed { session_id, reason } => {
                println!("session {session_id} closed: {reason}");
                session_meta.lock().await.remove(&session_id);
                data_channels.lock().await.remove(&session_id);
                if let Some(pc) = peer_connections.lock().await.remove(&session_id) {
                    let _ = pc.close().await;
                }
            }
            ProxyToPeer::Connected { .. } => {}
        }
    }

    writer.abort();
    Ok(())
}

pub(crate) fn send_json(tx: &mpsc::UnboundedSender<String>, payload: &impl Serialize) -> anyhow::Result<()> {
    let text = serde_json::to_string(payload)?;
    let _ = tx.send(text);
    Ok(())
}
