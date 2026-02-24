use std::collections::HashMap;
use std::sync::Arc;

use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AuthRole {
    Server,
    Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientToProxy {
    AuthProxy {
        proxy_password: String,
        role: AuthRole,
    },
    RegisterServer {
        server_name: String,
        server_password: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerToProxy {
    CommandOutput {
        session_id: Uuid,
        output: String,
        done: bool,
    },
    ServerDisconnectSession {
        session_id: Uuid,
    },
    ServerSignal {
        session_id: Uuid,
        signal: SignalPayload,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ProxyToPeer {
    AuthOk {
        role: AuthRole,
    },
    AuthError {
        reason: String,
    },
    Registered {
        server_name: String,
    },
    ConnectionError {
        reason: String,
    },
    ClientConnected {
        session_id: Uuid,
        client_id: Uuid,
        via_p2p: bool,
        turn: Option<TurnCredentials>,
    },
    RunCommand {
        session_id: Uuid,
        command: String,
    },
    SessionClosed {
        session_id: Uuid,
        reason: String,
    },
    PeerSignal {
        session_id: Uuid,
        from: AuthRole,
        signal: SignalPayload,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TurnCredentials {
    url: String,
    username: String,
    password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum SignalPayload {
    SdpOffer { sdp: String },
    SdpAnswer { sdp: String },
    IceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
}

#[derive(Clone)]
struct SessionP2pMeta {
    turn: Option<TurnCredentials>,
    use_p2p: bool,
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

    send_json(
        &ws_send_tx,
        &ClientToProxy::AuthProxy {
            proxy_password: args.proxy_password.clone(),
            role: AuthRole::Server,
        },
    )?;

    send_json(
        &ws_send_tx,
        &ClientToProxy::RegisterServer {
            server_name: args.server_name.clone(),
            server_password: args.server_password.clone(),
        },
    )?;

    let p2p_meta = Arc::new(Mutex::new(HashMap::<Uuid, SessionP2pMeta>::new()));
    let peer_connections = Arc::new(Mutex::new(HashMap::<Uuid, Arc<RTCPeerConnection>>::new()));

    while let Some(message) = read.next().await {
        let message = message?;
        let Message::Text(text) = message else {
            continue;
        };

        let parsed = serde_json::from_str::<ProxyToPeer>(&text);
        let Ok(proxy_message) = parsed else {
            continue;
        };

        match proxy_message {
            ProxyToPeer::AuthOk { .. } => {
                println!("proxy authentication succeeded");
            }
            ProxyToPeer::Registered { server_name } => {
                println!("server registered as '{server_name}'");
            }
            ProxyToPeer::AuthError { reason } | ProxyToPeer::ConnectionError { reason } => {
                anyhow::bail!("proxy rejected connection: {reason}");
            }
            ProxyToPeer::ClientConnected {
                session_id,
                client_id,
                via_p2p,
                turn,
            } => {
                println!("client {client_id} joined session {session_id}");
                p2p_meta.lock().await.insert(
                    session_id,
                    SessionP2pMeta {
                        turn,
                        use_p2p: via_p2p,
                    },
                );
            }
            ProxyToPeer::RunCommand {
                session_id,
                command,
            } => {
                let output = execute_command(command).await;
                let msg = ServerToProxy::CommandOutput {
                    session_id,
                    output,
                    done: true,
                };
                send_json(&ws_send_tx, &msg)?;
            }
            ProxyToPeer::PeerSignal {
                session_id,
                from,
                signal,
            } => {
                if from != AuthRole::Client {
                    continue;
                }

                let meta = p2p_meta.lock().await.get(&session_id).cloned();
                if meta.as_ref().map(|m| m.use_p2p).unwrap_or(false) {
                    handle_client_signal(
                        session_id,
                        signal,
                        meta.and_then(|m| m.turn),
                        ws_send_tx.clone(),
                        peer_connections.clone(),
                    )
                    .await?;
                }
            }
            ProxyToPeer::SessionClosed { session_id, reason } => {
                println!("session {session_id} closed: {reason}");
                p2p_meta.lock().await.remove(&session_id);
                if let Some(pc) = peer_connections.lock().await.remove(&session_id) {
                    let _ = pc.close().await;
                }
            }
        }
    }

    writer.abort();
    Ok(())
}

fn send_json(tx: &mpsc::UnboundedSender<String>, payload: &impl Serialize) -> anyhow::Result<()> {
    let text = serde_json::to_string(payload)?;
    let _ = tx.send(text);
    Ok(())
}

async fn handle_client_signal(
    session_id: Uuid,
    signal: SignalPayload,
    turn: Option<TurnCredentials>,
    ws_tx: mpsc::UnboundedSender<String>,
    peer_connections: Arc<Mutex<HashMap<Uuid, Arc<RTCPeerConnection>>>>,
) -> anyhow::Result<()> {
    let existing = peer_connections.lock().await.get(&session_id).cloned();
    let pc = if let Some(existing) = existing {
        existing
    } else {
        let created = create_peer_connection(session_id, turn, ws_tx.clone()).await?;
        peer_connections
            .lock()
            .await
            .insert(session_id, created.clone());
        created
    };

    match signal {
        SignalPayload::SdpOffer { sdp } => {
            let offer = RTCSessionDescription::offer(sdp)?;
            pc.set_remote_description(offer).await?;
            let answer = pc.create_answer(None).await?;
            pc.set_local_description(answer).await?;

            if let Some(local) = pc.local_description().await {
                send_json(
                    &ws_tx,
                    &ServerToProxy::ServerSignal {
                        session_id,
                        signal: SignalPayload::SdpAnswer { sdp: local.sdp },
                    },
                )?;
            }
        }
        SignalPayload::IceCandidate {
            candidate,
            sdp_mid,
            sdp_mline_index,
        } => {
            let init = RTCIceCandidateInit {
                candidate,
                sdp_mid,
                sdp_mline_index,
                username_fragment: None,
            };
            pc.add_ice_candidate(init).await?;
        }
        SignalPayload::SdpAnswer { .. } => {}
    }

    Ok(())
}

async fn create_peer_connection(
    session_id: Uuid,
    turn: Option<TurnCredentials>,
    ws_tx: mpsc::UnboundedSender<String>,
) -> anyhow::Result<Arc<RTCPeerConnection>> {
    let mut media_engine = MediaEngine::default();
    media_engine.register_default_codecs()?;
    let api = APIBuilder::new().with_media_engine(media_engine).build();

    let mut config = RTCConfiguration::default();
    if let Some(turn) = turn {
        config.ice_servers = vec![RTCIceServer {
            urls: vec![turn.url],
            username: turn.username,
            credential: turn.password,
        }];
    }

    let pc = Arc::new(api.new_peer_connection(config).await?);

    let ws_tx_ice = ws_tx.clone();
    pc.on_ice_candidate(Box::new(move |candidate| {
        let ws_tx_inner = ws_tx_ice.clone();
        Box::pin(async move {
            if let Some(candidate) = candidate {
                if let Ok(json) = candidate.to_json() {
                    let _ = send_json(
                        &ws_tx_inner,
                        &ServerToProxy::ServerSignal {
                            session_id,
                            signal: SignalPayload::IceCandidate {
                                candidate: json.candidate,
                                sdp_mid: json.sdp_mid,
                                sdp_mline_index: json.sdp_mline_index,
                            },
                        },
                    );
                }
            }
        })
    }));

    pc.on_data_channel(Box::new(move |dc| {
        Box::pin(async move {
            let dc_for_messages = dc.clone();
            dc.on_message(Box::new(move |msg| {
                let dc_sender = dc_for_messages.clone();
                Box::pin(async move {
                    let command = String::from_utf8_lossy(&msg.data).to_string();
                    let output = execute_command(command).await;
                    let _ = dc_sender.send_text(output).await;
                })
            }));
        })
    }));

    Ok(pc)
}

async fn execute_command(command: String) -> String {
    #[cfg(target_os = "windows")]
    let output_result = tokio::process::Command::new("powershell")
        .arg("-Command")
        .arg(command)
        .output()
        .await;

    #[cfg(not(target_os = "windows"))]
    let output_result = tokio::process::Command::new("sh")
        .arg("-lc")
        .arg(command)
        .output()
        .await;

    match output_result {
        Ok(output) => {
            let mut combined = String::new();
            if !output.stdout.is_empty() {
                combined.push_str(&String::from_utf8_lossy(&output.stdout));
            }
            if !output.stderr.is_empty() {
                combined.push_str(&String::from_utf8_lossy(&output.stderr));
            }
            if combined.is_empty() {
                "<no output>".to_string()
            } else {
                combined
            }
        }
        Err(err) => format!("command execution failed: {err}"),
    }
}