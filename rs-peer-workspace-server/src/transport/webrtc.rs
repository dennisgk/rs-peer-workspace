use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;

use crate::protocol::{PeerToProxy, SignalPayload, TurnCredentials};
use crate::send_json;
use crate::rpc::handle_rpc;
use rs_peer_workspace_shared::app::{AppEnvelope, AppPayload};

pub async fn handle_client_signal(
    session_id: Uuid,
    signal: SignalPayload,
    turn: Option<TurnCredentials>,
    ws_tx: mpsc::UnboundedSender<String>,
    peer_connections: Arc<Mutex<HashMap<Uuid, Arc<RTCPeerConnection>>>>,
    data_channels: Arc<Mutex<HashMap<Uuid, Arc<RTCDataChannel>>>>,
) -> anyhow::Result<()> {
    let existing = peer_connections.lock().await.get(&session_id).cloned();
    let pc = if let Some(existing) = existing {
        existing
    } else {
        let created = create_peer_connection(session_id, turn, ws_tx.clone(), data_channels.clone()).await?;
        peer_connections.lock().await.insert(session_id, created.clone());
        created
    };

    match signal {
        SignalPayload::SdpOffer { sdp } => {
            let offer = RTCSessionDescription::offer(sdp)?;
            pc.set_remote_description(offer).await?;
            let answer = pc.create_answer(None).await?;
            pc.set_local_description(answer).await?;
            if let Some(local) = pc.local_description().await {
                send_json(&ws_tx, &PeerToProxy::Signal {
                    session_id,
                    signal: SignalPayload::SdpAnswer { sdp: local.sdp },
                })?;
            }
        }
        SignalPayload::IceCandidate { candidate, sdp_mid, sdp_mline_index } => {
            let init = RTCIceCandidateInit { candidate, sdp_mid, sdp_mline_index, username_fragment: None };
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
    data_channels: Arc<Mutex<HashMap<Uuid, Arc<RTCDataChannel>>>>,
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
                    let _ = send_json(&ws_tx_inner, &PeerToProxy::Signal {
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

    let channels = data_channels.clone();
    pc.on_data_channel(Box::new(move |dc| {
        let channels = channels.clone();
        Box::pin(async move {
            channels.lock().await.insert(session_id, dc.clone());
            let dc_for_messages = dc.clone();
            dc.on_message(Box::new(move |msg| {
                let dc_sender = dc_for_messages.clone();
                Box::pin(async move {
                    let Ok(envelope) = serde_json::from_slice::<AppEnvelope>(&msg.data) else { return; };
                    if let AppPayload::RpcRequest(request) = envelope.payload {
                        let response = handle_rpc(request).await;
                        let out = AppEnvelope {
                            message_id: Uuid::new_v4(),
                            payload: AppPayload::RpcResponse(response),
                        };
                        if let Ok(bytes) = serde_json::to_vec(&out) {
                            let _ = dc_sender.send(&Bytes::from(bytes)).await;
                        }
                    }
                })
            }));
        })
    }));

    Ok(pc)
}
