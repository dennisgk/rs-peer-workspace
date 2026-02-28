use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthRole {
    Server,
    Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PeerToProxy {
    AuthProxy {
        proxy_password: String,
        role: AuthRole,
    },
    RegisterServer {
        server_name: String,
        server_password: String,
    },
    ConnectServer {
        server_name: String,
        server_password: String,
        use_p2p: bool,
    },
    DisconnectSession {
        session_id: Uuid,
    },
    Signal {
        session_id: Uuid,
        signal: SignalPayload,
    },
    RelayData {
        session_id: Uuid,
        payload: Vec<u8>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProxyToPeer {
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
    Connected {
        session_id: Uuid,
        server_name: String,
        via_p2p: bool,
        turn: Option<TurnCredentials>,
    },
    PeerJoined {
        session_id: Uuid,
        peer_id: Uuid,
        via_p2p: bool,
        turn: Option<TurnCredentials>,
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
    RelayData {
        session_id: Uuid,
        payload: Vec<u8>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnCredentials {
    pub url: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SignalPayload {
    SdpOffer { sdp: String },
    SdpAnswer { sdp: String },
    IceCandidate {
        candidate: String,
        sdp_mid: Option<String>,
        sdp_mline_index: Option<u16>,
    },
}
