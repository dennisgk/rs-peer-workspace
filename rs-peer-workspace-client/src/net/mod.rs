use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use rs_peer_workspace_shared::app::{AppEnvelope, AppPayload, RpcRequest, RpcResponse};
use rs_peer_workspace_shared::project::ProjectConnection;
use rs_peer_workspace_shared::relay::{
    AuthRole, PeerToProxy, ProxyToPeer, SignalPayload, TurnCredentials,
};
use tokio::sync::{mpsc as tokio_mpsc, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

#[derive(Debug)]
pub enum ConnectionCommand {
    SendRpc(RpcRequest),
    Disconnect,
}

#[derive(Debug)]
pub enum ConnectionEvent {
    Status {
        connection_name: String,
        message: String,
    },
    Transport {
        connection_name: String,
        message: String,
    },
    Connected {
        connection_name: String,
    },
    RpcResponse {
        connection_name: String,
        response: RpcResponse,
    },
    Error {
        connection_name: String,
        message: String,
    },
    Closed {
        connection_name: String,
        reason: String,
    },
}

pub fn spawn_connection(
    connection: ProjectConnection,
    event_tx: Sender<ConnectionEvent>,
) -> tokio_mpsc::UnboundedSender<ConnectionCommand> {
    let (command_tx, command_rx) = tokio_mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new();
        let Ok(runtime) = runtime else {
            let _ = event_tx.send(ConnectionEvent::Error {
                connection_name: connection.name.clone(),
                message: "failed to start tokio runtime".to_string(),
            });
            return;
        };
        runtime.block_on(async move {
            if let Err(err) = connection_task(connection.clone(), command_rx, event_tx.clone()).await
            {
                let _ = event_tx.send(ConnectionEvent::Error {
                    connection_name: connection.name.clone(),
                    message: err.to_string(),
                });
            }
        });
    });
    command_tx
}

async fn connection_task(
    connection: ProjectConnection,
    mut command_rx: tokio_mpsc::UnboundedReceiver<ConnectionCommand>,
    event_tx: Sender<ConnectionEvent>,
) -> anyhow::Result<()> {
    let (ws_stream, _) = connect_async(&connection.proxy_addr).await?;
    let (mut write, mut read) = ws_stream.split();
    let (ws_send_tx, mut ws_send_rx) = tokio_mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(text) = ws_send_rx.recv().await {
            if write.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    send_ws(
        &ws_send_tx,
        &PeerToProxy::AuthProxy {
            proxy_password: connection.proxy_password.clone(),
            role: AuthRole::Client,
        },
    )?;
    send_ws(
        &ws_send_tx,
        &PeerToProxy::ConnectServer {
            server_name: connection.server_name.clone(),
            server_password: connection.server_password.clone(),
            use_p2p: connection.prefer_p2p,
        },
    )?;

    let mut active_session: Option<Uuid> = None;
    let mut peer_connection: Option<Arc<RTCPeerConnection>> = None;
    let data_channel = Arc::new(Mutex::new(None::<Arc<RTCDataChannel>>));
    let p2p_ready = Arc::new(AtomicBool::new(false));

    loop {
        tokio::select! {
            inbound = read.next() => {
                let Some(message) = inbound else {
                    let _ = event_tx.send(ConnectionEvent::Closed {
                        connection_name: connection.name.clone(),
                        reason: "proxy socket closed".to_string(),
                    });
                    break;
                };
                let message = message?;
                let Message::Text(text) = message else { continue; };
                let Ok(parsed) = serde_json::from_str::<ProxyToPeer>(&text) else { continue; };

                match parsed {
                    ProxyToPeer::AuthOk { .. } => {
                        let _ = event_tx.send(ConnectionEvent::Status {
                            connection_name: connection.name.clone(),
                            message: "authenticated to proxy".to_string(),
                        });
                    }
                    ProxyToPeer::AuthError { reason } | ProxyToPeer::ConnectionError { reason } => {
                        let _ = event_tx.send(ConnectionEvent::Error {
                            connection_name: connection.name.clone(),
                            message: reason,
                        });
                        break;
                    }
                    ProxyToPeer::Connected { session_id, via_p2p, turn, .. } => {
                        active_session = Some(session_id);
                        let _ = event_tx.send(ConnectionEvent::Connected {
                            connection_name: connection.name.clone(),
                        });
                        if via_p2p {
                            if let Some(turn_cfg) = turn {
                                let _ = event_tx.send(ConnectionEvent::Transport {
                                    connection_name: connection.name.clone(),
                                    message: "Attempting P2P via TURN".to_string(),
                                });
                                let (pc, dc) = create_client_peer_connection(
                                    session_id,
                                    turn_cfg,
                                    ws_send_tx.clone(),
                                    event_tx.clone(),
                                    connection.name.clone(),
                                    p2p_ready.clone(),
                                )
                                .await?;
                                *data_channel.lock().await = Some(dc);
                                let offer = pc.create_offer(None).await?;
                                pc.set_local_description(offer).await?;
                                if let Some(local) = pc.local_description().await {
                                    send_ws(&ws_send_tx, &PeerToProxy::Signal {
                                        session_id,
                                        signal: SignalPayload::SdpOffer { sdp: local.sdp },
                                    })?;
                                }
                                peer_connection = Some(pc);
                            } else {
                                let _ = event_tx.send(ConnectionEvent::Transport {
                                    connection_name: connection.name.clone(),
                                    message: "WebSocket relay".to_string(),
                                });
                            }
                        } else {
                            let _ = event_tx.send(ConnectionEvent::Transport {
                                connection_name: connection.name.clone(),
                                message: "WebSocket relay".to_string(),
                            });
                        }
                    }
                    ProxyToPeer::PeerSignal { session_id, from, signal } => {
                        if Some(session_id) != active_session || from != AuthRole::Server {
                            continue;
                        }
                        if let Some(pc) = &peer_connection {
                            match signal {
                                SignalPayload::SdpAnswer { sdp } => {
                                    let answer = RTCSessionDescription::answer(sdp)?;
                                    pc.set_remote_description(answer).await?;
                                }
                                SignalPayload::IceCandidate { candidate, sdp_mid, sdp_mline_index } => {
                                    pc.add_ice_candidate(RTCIceCandidateInit {
                                        candidate,
                                        sdp_mid,
                                        sdp_mline_index,
                                        username_fragment: None,
                                    }).await?;
                                }
                                SignalPayload::SdpOffer { .. } => {}
                            }
                        }
                    }
                    ProxyToPeer::RelayData { session_id, payload } => {
                        if Some(session_id) != active_session {
                            continue;
                        }
                        if let Ok(envelope) = serde_json::from_slice::<AppEnvelope>(&payload) {
                            if let AppPayload::RpcResponse(response) = envelope.payload {
                                let _ = event_tx.send(ConnectionEvent::RpcResponse {
                                    connection_name: connection.name.clone(),
                                    response,
                                });
                            }
                        }
                    }
                    ProxyToPeer::SessionClosed { session_id, reason } => {
                        if Some(session_id) == active_session {
                            let _ = event_tx.send(ConnectionEvent::Closed {
                                connection_name: connection.name.clone(),
                                reason,
                            });
                            break;
                        }
                    }
                    ProxyToPeer::Registered { .. } | ProxyToPeer::PeerJoined { .. } => {}
                }
            }
            command = command_rx.recv() => {
                let Some(command) = command else { break; };
                match command {
                    ConnectionCommand::SendRpc(request) => {
                        if let Some(session_id) = active_session {
                            let envelope = AppEnvelope {
                                message_id: Uuid::new_v4(),
                                payload: AppPayload::RpcRequest(request),
                            };
                            let payload = serde_json::to_vec(&envelope)?;
                            if p2p_ready.load(Ordering::SeqCst) {
                                if let Some(dc) = data_channel.lock().await.clone() {
                                    let _ = dc.send_text(String::from_utf8_lossy(&payload).to_string()).await;
                                    continue;
                                }
                            }
                            send_ws(&ws_send_tx, &PeerToProxy::RelayData { session_id, payload })?;
                        }
                    }
                    ConnectionCommand::Disconnect => {
                        if let Some(session_id) = active_session {
                            let _ = send_ws(&ws_send_tx, &PeerToProxy::DisconnectSession { session_id });
                        }
                        if let Some(pc) = &peer_connection {
                            let _ = pc.close().await;
                        }
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

fn send_ws(tx: &tokio_mpsc::UnboundedSender<String>, payload: &impl serde::Serialize) -> anyhow::Result<()> {
    let text = serde_json::to_string(payload)?;
    let _ = tx.send(text);
    Ok(())
}

async fn create_client_peer_connection(
    session_id: Uuid,
    turn: TurnCredentials,
    ws_tx: tokio_mpsc::UnboundedSender<String>,
    event_tx: Sender<ConnectionEvent>,
    connection_name: String,
    p2p_ready: Arc<AtomicBool>,
) -> anyhow::Result<(Arc<RTCPeerConnection>, Arc<RTCDataChannel>)> {
    let mut media_engine = MediaEngine::default();
    media_engine.register_default_codecs()?;
    let api = APIBuilder::new().with_media_engine(media_engine).build();
    let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec![turn.url],
            username: turn.username,
            credential: turn.password,
        }],
        ..Default::default()
    };
    let pc = Arc::new(api.new_peer_connection(config).await?);

    let ws_tx_ice = ws_tx.clone();
    pc.on_ice_candidate(Box::new(move |candidate| {
        let ws_tx_inner = ws_tx_ice.clone();
        Box::pin(async move {
            if let Some(candidate) = candidate {
                if let Ok(json) = candidate.to_json() {
                    let _ = send_ws(&ws_tx_inner, &PeerToProxy::Signal {
                        session_id,
                        signal: SignalPayload::IceCandidate {
                            candidate: json.candidate,
                            sdp_mid: json.sdp_mid,
                            sdp_mline_index: json.sdp_mline_index,
                        },
                    });
                }
            }
        })
    }));

    let dc = pc.create_data_channel(
        "workspace",
        Some(RTCDataChannelInit {
            ordered: Some(true),
            ..Default::default()
        }),
    ).await?;

    let ready_flag = p2p_ready.clone();
    let event_tx_open = event_tx.clone();
    let name_open = connection_name.clone();
    dc.on_open(Box::new(move || {
        let ready_flag = ready_flag.clone();
        let event_tx_open = event_tx_open.clone();
        let name_open = name_open.clone();
        Box::pin(async move {
            ready_flag.store(true, Ordering::SeqCst);
            let _ = event_tx_open.send(ConnectionEvent::Transport {
                connection_name: name_open.clone(),
                message: "P2P data channel".to_string(),
            });
            let _ = event_tx_open.send(ConnectionEvent::Status {
                connection_name: name_open,
                message: "P2P channel established".to_string(),
            });
        })
    }));

    let ready_flag_close = p2p_ready.clone();
    let event_tx_close = event_tx.clone();
    let name_close = connection_name.clone();
    dc.on_close(Box::new(move || {
        let ready_flag_close = ready_flag_close.clone();
        let event_tx_close = event_tx_close.clone();
        let name_close = name_close.clone();
        Box::pin(async move {
            ready_flag_close.store(false, Ordering::SeqCst);
            let _ = event_tx_close.send(ConnectionEvent::Transport {
                connection_name: name_close.clone(),
                message: "WebSocket relay".to_string(),
            });
            let _ = event_tx_close.send(ConnectionEvent::Status {
                connection_name: name_close,
                message: "P2P channel closed; using WebSocket relay".to_string(),
            });
        })
    }));

    let event_tx_msg = event_tx.clone();
    let name_msg = connection_name.clone();
    dc.on_message(Box::new(move |msg| {
        let event_tx_msg = event_tx_msg.clone();
        let name_msg = name_msg.clone();
        Box::pin(async move {
            let Ok(response) = serde_json::from_slice::<AppEnvelope>(&msg.data) else {
                return;
            };
            if let AppPayload::RpcResponse(response) = response.payload {
                let _ = event_tx_msg.send(ConnectionEvent::RpcResponse {
                    connection_name: name_msg,
                    response,
                });
            }
        })
    }));

    Ok((pc, dc))
}
